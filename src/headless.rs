//! Headless refresh (`calendarchy --refresh`): fetch the current month from
//! both sources and update the disk cache, without entering the TUI. Used by
//! unattended consumers of the cache (e.g. the TRMNL e-ink push job).
//!
//! Mirrors the TUI fetch path exactly: refresh expired Google tokens, fetch
//! per-source, convert to DisplayEvent, `store()` the month, `save_to_disk()`.

use chrono::{Datelike, Duration, Local, NaiveDate};

use crate::auth::CalendarEntry;
use crate::cache::EventCache;
use crate::config::{self, Config};
use crate::conversion::{google_event_to_display, icloud_event_to_display};
use crate::google::{CalendarClient, GoogleAuth, TokenInfo};
use crate::icloud::{CalDavClient, ICalEvent, ICloudAuth};

fn month_range(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = date.with_day(1).unwrap();
    let last = if date.month() == 12 {
        NaiveDate::from_ymd_opt(date.year() + 1, 1, 1).unwrap() - Duration::days(1)
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1).unwrap() - Duration::days(1)
    };
    (first, last)
}

async fn google_tokens(config: &Config) -> Option<TokenInfo> {
    let gconfig = config.google.as_ref()?;
    match config::load_google_tokens() {
        Ok(Some(tokens)) if !tokens.is_expired() => Some(tokens),
        Ok(Some(tokens)) => {
            let refresh_token = tokens.refresh_token.as_ref()?.clone();
            let auth = GoogleAuth::new(gconfig.clone());
            match auth.refresh_token(&refresh_token).await {
                Ok(new_tokens) => {
                    let _ = config::save_google_tokens(&new_tokens);
                    Some(new_tokens)
                }
                Err(e) => {
                    eprintln!("google: token refresh failed: {e}");
                    None
                }
            }
        }
        _ => {
            eprintln!("google: no saved tokens (run the app once to authenticate)");
            None
        }
    }
}

fn icloud_calendars() -> Vec<CalendarEntry> {
    match config::load_icloud_tokens() {
        Ok(Some(tokens)) => {
            if !tokens.calendars.is_empty() {
                tokens
                    .calendars
                    .into_iter()
                    .map(|c| CalendarEntry { url: c.url, name: c.name })
                    .collect()
            } else {
                tokens
                    .calendar_urls
                    .into_iter()
                    .map(|url| CalendarEntry { url, name: None })
                    .collect()
            }
        }
        _ => Vec::new(),
    }
}

pub async fn refresh() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let today = Local::now().date_naive();
    let (start, end) = month_range(today);

    let mut cache = EventCache::new();
    cache.load_from_disk();

    let mut fetched = Vec::new();

    // --- Google -------------------------------------------------------------
    if config.google.is_some() {
        if let Some(tokens) = google_tokens(&config).await {
            let calendar_id = config
                .google
                .as_ref()
                .map(|c| c.calendar_id.clone())
                .unwrap_or_else(|| "primary".to_string());
            let client = CalendarClient::new();
            let calendar_name = client.get_calendar_name(&tokens, &calendar_id).await.ok().flatten();
            match client.list_events(&tokens, &calendar_id, start, end).await {
                Ok(events) => {
                    let display_events: Vec<_> = events
                        .into_iter()
                        .filter_map(|e| google_event_to_display(e, calendar_id.clone(), calendar_name.clone()))
                        .collect();
                    fetched.push(format!("google: {} events", display_events.len()));
                    cache.google.store(display_events, start);
                }
                Err(e) => eprintln!("google: fetch failed: {e}"),
            }
        }
    }

    // --- iCloud (CalDAV; EventKit is interactive/macOS-only) -----------------
    if let Some(ref icloud_config) = config.icloud {
        if !icloud_config.is_eventkit() {
            let calendars = icloud_calendars();
            if calendars.is_empty() {
                eprintln!("icloud: no discovered calendars (run the app once to authenticate)");
            } else {
                let auth = ICloudAuth::new(icloud_config.clone());
                let client = CalDavClient::new(auth);
                let mut all_events: Vec<(ICalEvent, Option<String>)> = Vec::new();
                let mut failed = false;
                for cal in &calendars {
                    match client.fetch_events(&cal.url, start, end).await {
                        Ok(events) => all_events.extend(events.into_iter().map(|e| (e, cal.name.clone()))),
                        Err(e) => {
                            eprintln!("icloud: fetch failed for {}: {e}", cal.url);
                            failed = true;
                            break;
                        }
                    }
                }
                if !failed {
                    let display_events: Vec<_> = all_events
                        .into_iter()
                        .map(|(e, name)| icloud_event_to_display(e, name))
                        .collect();
                    fetched.push(format!("icloud: {} events", display_events.len()));
                    cache.icloud.store(display_events, start);
                }
            }
        }
    }

    if fetched.is_empty() {
        return Err("no source refreshed — cache left untouched".into());
    }

    cache.save_to_disk();
    println!("refreshed {}-{:02} — {}", today.year(), today.month(), fetched.join(", "));
    Ok(())
}
