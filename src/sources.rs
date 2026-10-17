//! Fetching and acting on events, shared by the TUI and the headless
//! commands (`--refresh`, `--event`).
//!
//! Everything here takes the process-wide `reqwest::Client`, so connections
//! and TLS sessions are reused across requests instead of re-handshaking per
//! fetch. Google calls go through a `GoogleSession`, which refreshes an
//! expired access token (and retries once on a 401) so a long-running TUI
//! keeps working past the token's one-hour lifetime.

use std::future::Future;
use std::sync::Arc;

use chrono::NaiveDate;
use reqwest::Client;

use crate::auth::ICloudAccount;
use crate::cache::{DisplayEvent, EventId};
use crate::config::{GoogleConfig, ICloudConfig};
use crate::conversion::{google_event_to_display, icloud_event_to_display};
use crate::error::{CalendarchyError, Result};
use crate::google::{CalendarClient, GoogleAuth, TokenInfo};
use crate::icloud::CalDavClient;
use crate::utils::month_range;

/// The process-wide HTTP client. Timeouts so a request stuck on a dead
/// connection (e.g. after the laptop wakes, or a stalled CalDAV server)
/// fails and is retried instead of hanging forever
pub fn http_client() -> Client {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_default()
}

/// Google access token plus what's needed to renew it
pub struct GoogleSession {
    auth: GoogleAuth,
    tokens: TokenInfo,
    /// Set when the tokens were renewed, so the caller can persist them
    refreshed: bool,
}

impl GoogleSession {
    pub fn new(http: Client, config: GoogleConfig, tokens: TokenInfo) -> Self {
        Self { auth: GoogleAuth::new(http, config), tokens, refreshed: false }
    }

    async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self.tokens.refresh_token.clone().ok_or_else(|| {
            CalendarchyError::Auth("no refresh token — sign in again".to_string())
        })?;
        self.tokens = self.auth.refresh_token(&refresh_token).await?;
        self.refreshed = true;
        Ok(())
    }

    /// Valid tokens, refreshed first if they're (about to be) expired
    async fn fresh_tokens(&mut self) -> Result<TokenInfo> {
        if self.tokens.is_expired() {
            self.refresh().await?;
        }
        Ok(self.tokens.clone())
    }

    /// Run `call` with a valid access token: refresh first if it's (about to
    /// be) expired, and refresh + retry once if the API still says 401
    pub async fn call<T, F, Fut>(&mut self, call: F) -> Result<T>
    where
        F: Fn(TokenInfo) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        match call(self.fresh_tokens().await?).await {
            Err(CalendarchyError::TokenExpired) => {
                self.refresh().await?;
                call(self.tokens.clone()).await
            }
            other => other,
        }
    }

    /// The renewed tokens, if a refresh happened
    pub fn into_refreshed(self) -> Option<TokenInfo> {
        self.refreshed.then_some(self.tokens)
    }
}

/// True when an error means the user has to sign in again
pub fn is_auth_failure(e: &CalendarchyError) -> bool {
    matches!(e, CalendarchyError::Auth(_) | CalendarchyError::TokenExpired)
}

pub struct GoogleMonth {
    pub events: Vec<DisplayEvent>,
    pub calendar_name: Option<String>,
}

/// Fetch one month of Google events. `known_name` skips the calendar-name
/// lookup; otherwise it runs concurrently with the event listing.
pub async fn fetch_google_month(
    http: &Client,
    session: &mut GoogleSession,
    calendar_id: &str,
    known_name: Option<String>,
    month: NaiveDate,
) -> Result<GoogleMonth> {
    let (start, end) = month_range(month);
    let client = CalendarClient::new(http.clone());
    let client = &client;

    let (events, calendar_name) = match known_name {
        Some(name) => {
            let events = session.call(|t| async move { client.list_events(&t, calendar_id, start, end).await }).await?;
            (events, Some(name))
        }
        None => {
            // Make sure the token is fresh before firing both requests at once
            let tokens = session.fresh_tokens().await?;
            let (events, name) = tokio::join!(
                session.call(|t| async move { client.list_events(&t, calendar_id, start, end).await }),
                client.get_calendar_name(&tokens, calendar_id),
            );
            (events?, name.ok().flatten())
        }
    };

    let events = events
        .into_iter()
        .flat_map(|e| google_event_to_display(e, calendar_id.to_string(), calendar_name.clone(), (start, end)))
        .collect();
    Ok(GoogleMonth { events, calendar_name })
}

/// Fetch one month from every CalDAV calendar of the account concurrently.
/// All-or-nothing: a partial result would wipe the failed calendar's cached
/// events. The account's addresses tell which attendee is you.
pub async fn fetch_caldav_month(
    http: &Client,
    config: &ICloudConfig,
    account: &ICloudAccount,
    month: NaiveDate,
) -> Result<Vec<DisplayEvent>> {
    let (start, end) = month_range(month);
    let calendars = &account.calendars;
    let client = Arc::new(CalDavClient::new(http.clone(), config).with_addresses(&account.addresses));

    let mut tasks = tokio::task::JoinSet::new();
    for (index, cal) in calendars.iter().enumerate() {
        let client = Arc::clone(&client);
        let cal = cal.clone();
        tasks.spawn(async move { (index, cal.name, client.fetch_events(&cal.url, start, end).await) });
    }

    let mut per_calendar = Vec::with_capacity(calendars.len());
    while let Some(joined) = tasks.join_next().await {
        let (index, name, result) = joined.map_err(|e| CalendarchyError::CalDav(e.to_string()))?;
        per_calendar.push((index, name, result?));
    }
    // Keep calendar order stable regardless of which response came back first
    per_calendar.sort_by_key(|(index, _, _)| *index);

    Ok(per_calendar
        .into_iter()
        .flat_map(|(_, name, events)| events.into_iter().flat_map(move |e| icloud_event_to_display(e, name.clone(), (start, end))))
        .collect())
}

/// Discover the user's CalDAV calendars and calendar-user addresses
pub async fn discover_icloud(http: &Client, config: &ICloudConfig) -> Result<ICloudAccount> {
    let account = CalDavClient::new(http.clone(), config).discover().await?;
    if account.calendars.is_empty() {
        return Err(CalendarchyError::CalDav("No calendars found".to_string()));
    }
    Ok(account)
}

/// The account as a previous discovery saved it (empty if never discovered)
pub fn saved_icloud_account() -> ICloudAccount {
    crate::config::load_icloud_account().ok().flatten().unwrap_or_default()
}

/// What can be done to an event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventVerb {
    Accept,
    Decline,
    Delete,
}

impl EventVerb {
    /// The verb as `--event` takes it
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Accept, Self::Decline, Self::Delete].into_iter().find(|v| v.name() == name)
    }

    /// "accept"
    pub fn name(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Delete => "delete",
        }
    }

    /// "Accept"
    pub fn title(self) -> &'static str {
        match self {
            Self::Accept => "Accept",
            Self::Decline => "Decline",
            Self::Delete => "Delete",
        }
    }

    /// "accepted"; for accept/decline also the response Google is sent
    pub fn done(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Decline => "declined",
            Self::Delete => "deleted",
        }
    }

    /// "Accepting event..."
    pub fn in_progress(self) -> &'static str {
        match self {
            Self::Accept => "Accepting event...",
            Self::Decline => "Declining event...",
            Self::Delete => "Deleting event...",
        }
    }
}

/// Why an event action can't be done
#[derive(Debug, PartialEq)]
pub enum ActionRefused {
    /// Only Google events can be accepted or declined
    NotForICloud(EventVerb),
    GoogleSignedOut,
    ICloudNotConfigured,
}

impl ActionRefused {
    /// What to tell the user; `setup_hint` says how to set up the source
    pub fn message(&self, setup_hint: &str) -> String {
        match self {
            Self::NotForICloud(verb) => format!("{} not supported for iCloud", verb.title()),
            Self::GoogleSignedOut => format!("Not signed in to Google — {}", setup_hint),
            Self::ICloudNotConfigured => format!("iCloud not configured — {}", setup_hint),
        }
    }
}

/// The rules for acting on an event, the same in the TUI and `--event`:
/// accept/decline are Google-only, and the event's source must be set up
pub fn check_event_action(
    verb: EventVerb,
    id: &EventId,
    google_signed_in: bool,
    icloud_configured: bool,
) -> std::result::Result<(), ActionRefused> {
    match id {
        EventId::Google { .. } if !google_signed_in => Err(ActionRefused::GoogleSignedOut),
        EventId::Google { .. } => Ok(()),
        EventId::ICloud { .. } if verb != EventVerb::Delete => Err(ActionRefused::NotForICloud(verb)),
        EventId::ICloud { .. } if !icloud_configured => Err(ActionRefused::ICloudNotConfigured),
        EventId::ICloud { .. } => Ok(()),
    }
}

/// Accept, decline or delete one event: through `google` for Google events
/// (which may refresh its tokens), with the `icloud` account for CalDAV ones
pub async fn event_action(
    http: &Client,
    google: Option<&mut GoogleSession>,
    icloud: Option<&ICloudConfig>,
    id: &EventId,
    verb: EventVerb,
) -> Result<()> {
    check_event_action(verb, id, google.is_some(), icloud.is_some())
        .map_err(|refused| CalendarchyError::Api(refused.message("set it up first")))?;
    match (id, google, icloud) {
        (EventId::Google { calendar_id, event_id, .. }, Some(session), _) => {
            let client = &CalendarClient::new(http.clone());
            if verb == EventVerb::Delete {
                session.call(|t| async move { client.delete_event(&t, calendar_id, event_id).await }).await
            } else {
                let response = verb.done();
                session.call(|t| async move { client.respond_to_event(&t, calendar_id, event_id, response).await }).await
            }
        }
        (EventId::ICloud { calendar_url, event_uid, href, etag, .. }, _, Some(config)) => {
            CalDavClient::new(http.clone(), config)
                .delete_event(calendar_url, event_uid, href.as_deref(), etag.as_deref())
                .await
        }
        _ => unreachable!("check_event_action needs the event's account"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tokens(expired: bool) -> TokenInfo {
        TokenInfo {
            access_token: "old".into(),
            refresh_token: None, // refresh would fail fast without network
            expires_at: chrono::Utc::now() + chrono::Duration::hours(if expired { -1 } else { 1 }),
        }
    }

    fn google_id() -> EventId {
        EventId::Google { calendar_id: "primary".into(), event_id: "e1".into(), calendar_name: None }
    }

    fn icloud_id() -> EventId {
        EventId::ICloud { calendar_url: "https://cal/1".into(), event_uid: "u1".into(), etag: None, calendar_name: None, href: None }
    }

    #[test]
    fn test_event_action_rules() {
        use EventVerb::*;
        for verb in [Accept, Decline, Delete] {
            assert_eq!(check_event_action(verb, &google_id(), true, false), Ok(()));
            assert_eq!(check_event_action(verb, &google_id(), false, true), Err(ActionRefused::GoogleSignedOut));
        }
        assert_eq!(check_event_action(Delete, &icloud_id(), false, true), Ok(()));
        assert_eq!(check_event_action(Delete, &icloud_id(), true, false), Err(ActionRefused::ICloudNotConfigured));
        assert_eq!(check_event_action(Accept, &icloud_id(), true, true), Err(ActionRefused::NotForICloud(Accept)));
        assert_eq!(ActionRefused::NotForICloud(Decline).message("x"), "Decline not supported for iCloud");
        assert_eq!(ActionRefused::GoogleSignedOut.message("press S for setup"), "Not signed in to Google — press S for setup");
    }

    #[test]
    fn test_event_verbs_by_name() {
        for verb in [EventVerb::Accept, EventVerb::Decline, EventVerb::Delete] {
            assert_eq!(EventVerb::parse(verb.name()), Some(verb));
        }
        assert_eq!(EventVerb::parse("tentative"), None);
        assert_eq!((EventVerb::Accept.done(), EventVerb::Decline.done()), ("accepted", "declined"));
    }

    #[tokio::test]
    async fn test_event_action_refuses_before_any_request() {
        let err = event_action(&Client::new(), None, None, &icloud_id(), EventVerb::Accept).await.unwrap_err();
        assert!(err.to_string().contains("Accept not supported for iCloud"), "{err}");
        assert!(!is_auth_failure(&err));
        let err = event_action(&Client::new(), None, None, &google_id(), EventVerb::Delete).await.unwrap_err();
        assert!(err.to_string().contains("Not signed in to Google"), "{err}");
    }

    #[tokio::test]
    async fn test_session_passes_valid_tokens_through() {
        let mut s = GoogleSession::new(Client::new(), GoogleConfig::default(), tokens(false));
        let out = s.call(|t| async move { Ok::<_, CalendarchyError>(t.access_token) }).await.unwrap();
        assert_eq!(out, "old");
        assert!(s.into_refreshed().is_none());
    }

    #[tokio::test]
    async fn test_session_retries_once_on_401_and_reports_auth_failure() {
        // A 401 triggers a refresh; with no refresh token that's an auth error
        // the UI turns into "sign in again", and the call isn't retried blindly
        let calls = AtomicUsize::new(0);
        let mut s = GoogleSession::new(Client::new(), GoogleConfig::default(), tokens(false));
        let err = s
            .call(|_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Err::<(), _>(CalendarchyError::TokenExpired) }
            })
            .await
            .unwrap_err();
        assert!(is_auth_failure(&err));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_session_refreshes_expired_tokens_before_calling() {
        let calls = AtomicUsize::new(0);
        let mut s = GoogleSession::new(Client::new(), GoogleConfig::default(), tokens(true));
        let err = s
            .call(|_| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<(), CalendarchyError>(()) }
            })
            .await
            .unwrap_err();
        assert!(is_auth_failure(&err));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "must not call the API with an expired token");
    }
}
