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

use chrono::{Local, NaiveDate};

use crate::cache::EventCache;
use crate::config::{self, Config};
use crate::sources::{self, EventVerb, GoogleSession};
use crate::utils::{add_months, month_start};

/// `--refresh`: this month and the next, so readers looking a few days ahead
/// (the shell plugin's "tomorrow", alerts) don't come up empty at the end of
/// a month; or just `month` when one is asked for
pub async fn refresh(month: Option<NaiveDate>) -> Result<(), Box<dyn std::error::Error>> {
    let this = month_start(Local::now().date_naive());
    let months = match month {
        Some(m) => vec![month_start(m)],
        None => vec![this, add_months(this, 1)],
    };
    refresh_months(&months).await
}

async fn refresh_months(months: &[NaiveDate]) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let http = sources::http_client();

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

    let icloud = async {
        let icloud = config.icloud.clone()?;
        let mut account = sources::saved_icloud_account();
        if account.calendars.is_empty() {
            eprintln!("icloud: no discovered calendars (run the app once to authenticate)");
            return None;
        }
        if account.addresses.is_empty() {
            // Saved before your addresses were discovered: find them once,
            // keeping the saved calendars
            match sources::discover_icloud(&http, &icloud).await {
                Ok(found) => {
                    account.addresses = found.addresses;
                    if let Err(e) = config::save_icloud_account(&account) {
                        eprintln!("icloud: couldn't save your addresses: {e}");
                    }
                }
                Err(e) => eprintln!("icloud: couldn't discover your addresses: {e}"),
            }
        }
        let mut fetched = Vec::new();
        for &m in months {
            match sources::fetch_caldav_month(&http, &icloud, &account, m).await {
                Ok(events) => fetched.push((m, events)),
                Err(e) => eprintln!("icloud: fetch failed: {e}"),
            }
        }
        Some(fetched)
    };

    let (google, icloud) = tokio::join!(google, icloud);

    let mut cache = EventCache::default();
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
    let verb = EventVerb::parse(verb).ok_or_else(|| format!("unknown action {verb} (accept, decline or delete)"))?;
    let config = Config::load().unwrap_or_default();
    let mut cache = EventCache::default();
    cache.load_from_disk();
    let found = [("google", &cache.google), ("icloud", &cache.icloud)]
        .into_iter()
        .flat_map(|(key, source)| source.all_events().map(move |e| (key, e)))
        .find(|(key, e)| crate::agenda::event_key(key, e) == id)
        .map(|(_, e)| (e.id.clone(), e.date));
    let Some((event_id, date)) = found else {
        return Err(format!("no event {id} in the cache").into());
    };

    let google = config.google.clone().zip(config::load_google_tokens().ok().flatten());
    sources::check_event_action(verb, &event_id, google.is_some(), config.icloud.is_some())
        .map_err(|refused| refused.message("open calendarchy to sign in"))?;

    let http = sources::http_client();
    let mut session = google.map(|(gconfig, tokens)| GoogleSession::new(http.clone(), gconfig, tokens));
    let result = sources::event_action(&http, session.as_mut(), config.icloud.as_ref(), &event_id, verb).await;
    if let Some(tokens) = session.and_then(GoogleSession::into_refreshed) {
        let _ = config::save_google_tokens(&tokens);
    }
    result?;

    println!("{} {id}", verb.done());
    refresh_months(&[month_start(date)]).await
}
