//! Fetching and acting on events, shared by the TUI and `--refresh`.
//!
//! Everything here takes the process-wide `reqwest::Client`, so connections
//! and TLS sessions are reused across requests instead of re-handshaking per
//! fetch. Google calls go through a `GoogleSession`, which refreshes an
//! expired access token (and retries once on a 401) so a long-running TUI
//! keeps working past the token's one-hour lifetime.

use std::future::Future;
use std::sync::Arc;

use chrono::{Datelike, Duration, NaiveDate};
use reqwest::Client;

use crate::auth::CalendarEntry;
use crate::cache::DisplayEvent;
use crate::config::{GoogleConfig, ICloudConfig};
use crate::conversion::{google_event_to_display, icloud_event_to_display};
use crate::error::{CalendarchyError, Result};
use crate::google::{CalendarClient, GoogleAuth, TokenInfo};
use crate::icloud::{CalDavClient, ICloudAuth};

/// First and last day of the month containing `date`
pub fn month_range(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = date.with_day(1).unwrap();
    let next = if date.month() == 12 {
        NaiveDate::from_ymd_opt(date.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1).unwrap()
    };
    (first, next - Duration::days(1))
}

/// Google access token plus what's needed to renew it
pub struct GoogleSession {
    auth: GoogleAuth,
    pub tokens: TokenInfo,
    /// Set when the tokens were renewed, so the caller can persist them
    pub refreshed: bool,
}

impl GoogleSession {
    pub fn new(http: Client, config: GoogleConfig, tokens: TokenInfo) -> Self {
        Self { auth: GoogleAuth::with_client(http, config), tokens, refreshed: false }
    }

    async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self.tokens.refresh_token.clone().ok_or_else(|| {
            CalendarchyError::Auth("no refresh token — sign in again".to_string())
        })?;
        self.tokens = self.auth.refresh_token(&refresh_token).await?;
        self.refreshed = true;
        Ok(())
    }

    /// Run `call` with a valid access token: refresh first if it's (about to
    /// be) expired, and refresh + retry once if the API still says 401
    pub async fn call<T, F, Fut>(&mut self, call: F) -> Result<T>
    where
        F: Fn(TokenInfo) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if self.tokens.is_expired() {
            self.refresh().await?;
        }
        match call(self.tokens.clone()).await {
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
    let client = CalendarClient::with_client(http.clone());
    let client = &client;

    let (events, calendar_name) = match known_name {
        Some(name) => {
            let events = session.call(|t| async move { client.list_events(&t, calendar_id, start, end).await }).await?;
            (events, Some(name))
        }
        None => {
            // Make sure the token is fresh before firing both requests at once
            if session.tokens.is_expired() {
                session.refresh().await?;
            }
            let tokens = session.tokens.clone();
            let (events, name) = tokio::join!(
                session.call(|t| async move { client.list_events(&t, calendar_id, start, end).await }),
                client.get_calendar_name(&tokens, calendar_id),
            );
            (events?, name.ok().flatten())
        }
    };

    let events = events
        .into_iter()
        .filter_map(|e| google_event_to_display(e, calendar_id.to_string(), calendar_name.clone()))
        .collect();
    Ok(GoogleMonth { events, calendar_name })
}

/// Fetch one month from every CalDAV calendar concurrently. All-or-nothing:
/// a partial result would wipe the failed calendar's cached events.
pub async fn fetch_caldav_month(
    http: &Client,
    config: &ICloudConfig,
    calendars: &[CalendarEntry],
    month: NaiveDate,
) -> Result<Vec<DisplayEvent>> {
    let (start, end) = month_range(month);
    let client = Arc::new(CalDavClient::with_client(http.clone(), ICloudAuth::new(config.clone())));

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
        .flat_map(|(_, name, events)| events.into_iter().map(move |e| icloud_event_to_display(e, name.clone())))
        .collect())
}

/// Fetch one month via the macOS EventKit helper (blocking subprocess)
pub async fn fetch_eventkit_month(month: NaiveDate) -> std::result::Result<Vec<DisplayEvent>, String> {
    let (start, end) = month_range(month);
    tokio::task::spawn_blocking(move || crate::eventkit::fetch_events(start, end))
        .await
        .map_err(|e| e.to_string())?
}

/// Discover the user's CalDAV calendars
pub async fn discover_icloud(http: &Client, config: &ICloudConfig) -> Result<Vec<CalendarEntry>> {
    let client = CalDavClient::with_client(http.clone(), ICloudAuth::new(config.clone()));
    let calendars: Vec<CalendarEntry> = client
        .discover_calendars()
        .await?
        .into_iter()
        .map(|c| CalendarEntry { url: c.url, name: c.name })
        .collect();
    if calendars.is_empty() {
        return Err(CalendarchyError::CalDav("No calendars found".to_string()));
    }
    Ok(calendars)
}

/// Calendars saved by a previous discovery (new format, then legacy URLs)
pub fn saved_icloud_calendars() -> Vec<CalendarEntry> {
    match crate::config::load_icloud_tokens() {
        Ok(Some(tokens)) if !tokens.calendars.is_empty() => tokens
            .calendars
            .into_iter()
            .map(|c| CalendarEntry { url: c.url, name: c.name })
            .collect(),
        Ok(Some(tokens)) => tokens
            .calendar_urls
            .into_iter()
            .map(|url| CalendarEntry { url, name: None })
            .collect(),
        _ => Vec::new(),
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
            token_type: "Bearer".into(),
        }
    }

    #[test]
    fn test_month_range() {
        let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(month_range(d(2026, 2, 14)), (d(2026, 2, 1), d(2026, 2, 28)));
        assert_eq!(month_range(d(2026, 12, 31)), (d(2026, 12, 1), d(2026, 12, 31)));
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
