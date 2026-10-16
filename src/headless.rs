//! Headless commands, without entering the TUI:
//!
//! - `calendarchy --refresh [--month YYYY-MM]`: fetch this month and the next
//!   (or the given month) from both sources into the disk cache. Used by
//!   unattended consumers of the cache (the shell plugin, the TRMNL push job).
//! - `calendarchy --event accept|decline|delete <id>`: act on one event, by
//!   the id `--agenda` prints, then refetch its month. Used by the shell
//!   plugin's window.
//!
//! Uses the same fetch layer as the TUI (`sources`), with both sources
//! fetched concurrently.

use chrono::{Datelike, Local, Months, NaiveDate};
use reqwest::Client;

use crate::cache::{EventCache, EventId};
use crate::config::{self, Config};
use crate::google::CalendarClient;
use crate::icloud::{CalDavClient, ICloudAuth};
use crate::sources::{self, GoogleSession};

/// `--refresh`: this month and the next, so readers looking a few days ahead
/// (the shell plugin's "tomorrow", alerts) don't come up empty at the end of
/// a month; or just `month` when one is asked for
pub async fn refresh(month: Option<NaiveDate>) -> Result<(), Box<dyn std::error::Error>> {
    let this = Local::now().date_naive().with_day(1).unwrap();
    let months = match month {
        Some(m) => vec![m.with_day(1).unwrap()],
        None => [Some(this), this.checked_add_months(Months::new(1))].into_iter().flatten().collect(),
    };
    refresh_months(&months).await
}

async fn refresh_months(months: &[NaiveDate]) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let http = Client::new();

    let google = async {
        let gconfig = config.google.clone()?;
        let tokens = match config::load_google_tokens() {
            Ok(Some(tokens)) => tokens,
            _ => {
                eprintln!("google: no saved tokens (run the app once to authenticate)");
                return None;
            }
        };
        let mut session = GoogleSession::new(http.clone(), gconfig.clone(), tokens);
        let mut fetched = Vec::new();
        for &m in months {
            match sources::fetch_google_month(&http, &mut session, &gconfig.calendar_id, None, m).await {
                Ok(result) => fetched.push((m, result.events)),
                Err(e) => {
                    eprintln!("google: fetch failed: {e}");
                    break;
                }
            }
        }
        if let Some(tokens) = session.into_refreshed() {
            let _ = config::save_google_tokens(&tokens);
        }
        Some(fetched)
    };

    // EventKit is interactive/macOS-only, so only CalDAV refreshes here
    let icloud = async {
        let icloud = config.icloud.clone().filter(|c| !c.is_eventkit())?;
        let calendars = sources::saved_icloud_calendars();
        if calendars.is_empty() {
            eprintln!("icloud: no discovered calendars (run the app once to authenticate)");
            return None;
        }
        let mut fetched = Vec::new();
        for &m in months {
            match sources::fetch_caldav_month(&http, &icloud, &calendars, m).await {
                Ok(events) => fetched.push((m, events)),
                Err(e) => eprintln!("icloud: fetch failed: {e}"),
            }
        }
        Some(fetched)
    };

    let (google, icloud) = tokio::join!(google, icloud);

    let mut cache = EventCache::new();
    cache.load_from_disk();
    let mut summary = Vec::new();
    let count = |fetched: &[(chrono::NaiveDate, Vec<_>)]| fetched.iter().map(|(_, e)| e.len()).sum::<usize>();
    if let Some(fetched) = google.filter(|f| !f.is_empty()) {
        summary.push(format!("google: {} events", count(&fetched)));
        for (m, events) in fetched {
            cache.google.store(events, m);
        }
    }
    if let Some(fetched) = icloud.filter(|f| !f.is_empty()) {
        summary.push(format!("icloud: {} events", count(&fetched)));
        for (m, events) in fetched {
            cache.icloud.store(events, m);
        }
    }

    if summary.is_empty() {
        return Err("no source refreshed — cache left untouched".into());
    }

    cache.save_to_disk();
    let span: Vec<_> = months.iter().map(|m| m.format("%Y-%m").to_string()).collect();
    println!("refreshed {} — {}", span.join(", "), summary.join(", "));
    Ok(())
}

/// `--event <verb> <id>`: accept, decline or delete the event `--agenda`
/// listed as `id`, with the TUI's rules (accept/decline are Google-only),
/// then refetch its month so the cache shows the result
pub async fn event_action(verb: &str, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let mut cache = EventCache::new();
    cache.load_from_disk();
    let found = [("google", &cache.google), ("icloud", &cache.icloud)]
        .into_iter()
        .flat_map(|(key, source)| source.all_events().map(move |e| (key, e)))
        .find(|(key, e)| crate::agenda::event_key(key, e) == id)
        .map(|(_, e)| (e.id.clone(), e.date));
    let Some((event_id, date)) = found else {
        return Err(format!("no event {id} in the cache").into());
    };
    let http = Client::new();

    match (verb, event_id) {
        ("accept" | "decline", EventId::Google { calendar_id, event_id, .. }) => {
            let response = if verb == "accept" { "accepted" } else { "declined" };
            google_call(&config, &http, |t| {
                let (client, calendar_id, event_id) = (CalendarClient::with_client(http.clone()), calendar_id.clone(), event_id.clone());
                async move { client.respond_to_event(&t, &calendar_id, &event_id, response).await.map(|_| ()) }
            })
            .await?;
        }
        ("accept" | "decline", EventId::ICloud { .. }) => {
            return Err(format!("{} not supported for iCloud", if verb == "accept" { "Accept" } else { "Decline" }).into());
        }
        ("delete", EventId::Google { calendar_id, event_id, .. }) => {
            google_call(&config, &http, |t| {
                let (client, calendar_id, event_id) = (CalendarClient::with_client(http.clone()), calendar_id.clone(), event_id.clone());
                async move { client.delete_event(&t, &calendar_id, &event_id).await.map(|_| ()) }
            })
            .await?;
        }
        ("delete", EventId::ICloud { calendar_url, .. }) if calendar_url.is_empty() => {
            return Err("Delete not supported for system calendars".into());
        }
        ("delete", EventId::ICloud { calendar_url, event_uid, href, etag, .. }) => {
            let icloud = config.icloud.clone().ok_or("iCloud not configured")?;
            let client = CalDavClient::with_client(http.clone(), ICloudAuth::new(icloud));
            client.delete_event(&calendar_url, &event_uid, href.as_deref(), etag.as_deref()).await?;
        }
        (other, _) => return Err(format!("unknown action {other} (accept, decline or delete)").into()),
    }

    println!("{verb}d {id}");
    refresh_months(&[date.with_day(1).unwrap()]).await
}

/// One Google API call with the saved tokens, refreshing (and saving) them as needed
async fn google_call<F, Fut>(config: &Config, http: &Client, call: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: Fn(crate::google::TokenInfo) -> Fut,
    Fut: std::future::Future<Output = crate::error::Result<()>>,
{
    let gconfig = config.google.clone().ok_or("Google not configured")?;
    let tokens = config::load_google_tokens().ok().flatten().ok_or("Not signed in to Google — open calendarchy to sign in")?;
    let mut session = GoogleSession::new(http.clone(), gconfig, tokens);
    let result = session.call(call).await;
    if let Some(tokens) = session.into_refreshed() {
        let _ = config::save_google_tokens(&tokens);
    }
    result.map_err(|e| e.into())
}
