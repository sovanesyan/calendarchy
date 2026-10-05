//! Headless refresh (`calendarchy --refresh`): fetch the current month from
//! both sources and update the disk cache, without entering the TUI. Used by
//! unattended consumers of the cache (e.g. the TRMNL e-ink push job).
//!
//! Uses the same fetch layer as the TUI (`sources`), with both sources
//! fetched concurrently.

use chrono::{Datelike, Local};
use reqwest::Client;

use crate::cache::EventCache;
use crate::config::{self, Config};
use crate::sources::{self, GoogleSession};

pub async fn refresh() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let today = Local::now().date_naive();
    let month = today.with_day(1).unwrap();
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
        let result = sources::fetch_google_month(&http, &mut session, &gconfig.calendar_id, None, month).await;
        if let Some(tokens) = session.into_refreshed() {
            let _ = config::save_google_tokens(&tokens);
        }
        match result {
            Ok(m) => Some(m.events),
            Err(e) => {
                eprintln!("google: fetch failed: {e}");
                None
            }
        }
    };

    // EventKit is interactive/macOS-only, so only CalDAV refreshes here
    let icloud = async {
        let icloud = config.icloud.clone().filter(|c| !c.is_eventkit())?;
        let calendars = sources::saved_icloud_calendars();
        if calendars.is_empty() {
            eprintln!("icloud: no discovered calendars (run the app once to authenticate)");
            return None;
        }
        match sources::fetch_caldav_month(&http, &icloud, &calendars, month).await {
            Ok(events) => Some(events),
            Err(e) => {
                eprintln!("icloud: fetch failed: {e}");
                None
            }
        }
    };

    let (google, icloud) = tokio::join!(google, icloud);

    let mut cache = EventCache::new();
    cache.load_from_disk();
    let mut fetched = Vec::new();
    if let Some(events) = google {
        fetched.push(format!("google: {} events", events.len()));
        cache.google.store(events, month);
    }
    if let Some(events) = icloud {
        fetched.push(format!("icloud: {} events", events.len()));
        cache.icloud.store(events, month);
    }

    if fetched.is_empty() {
        return Err("no source refreshed — cache left untouched".into());
    }

    cache.save_to_disk();
    println!("refreshed {}-{:02} — {}", today.year(), today.month(), fetched.join(", "));
    Ok(())
}
