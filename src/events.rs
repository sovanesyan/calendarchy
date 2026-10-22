//! `calendarchy events [--since DATE] [--until DATE] [--json]`: the Google
//! (work) calendar's events for agents and scripts, fetched live with the
//! app's saved login. It never touches the disk cache: that holds what the
//! UI shows, while this is for history (`/stress`, `/grade` in Dossier).
//!
//! DATE is `YYYY-MM-DD`, `today` or `yesterday`, local time; `--since`
//! includes its day (default today), `--until` doesn't (default a week after
//! `--since`). Events come oldest first. `--json` prints one object a line:
//!
//! ```text
//! {"id", "date" (start, ISO with offset), "source": "calendarchy",
//!  "start", "end" (ISO with offset; all-day: YYYY-MM-DD, end exclusive),
//!  "all_day", "summary", "status", "self_response" ("accepted" when you're
//!  not on the guest list: it's yours), "organizer" (email),
//!  "attendees" (emails, without you and rooms), "recurring_event_id",
//!  "event_type" ("default", "outOfOffice", "focusTime", ...)}
//! ```
//!
//! Exit codes: 0 ok, 1 the fetch failed (no login, network), 2 usage.

use chrono::{DateTime, Duration, Local, NaiveDate, SecondsFormat, Utc};
use serde_json::json;

use crate::config::{self, Config};
use crate::google::types::{CalendarEvent, EventDateTime};
use crate::google::CalendarClient;
use crate::sources::{self, GoogleSession};

const SOURCE: &str = "calendarchy";

pub async fn print(args: &[String]) -> i32 {
    let (since, until, json) = match parse(args, Local::now().date_naive()) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("calendarchy events: {e}\nusage: calendarchy events [--since DATE] [--until DATE] [--json]");
            return 2;
        }
    };
    let events = match fetch(since, until).await {
        Ok(events) => events,
        Err(e) => {
            eprintln!("calendarchy events: {e}");
            return 1;
        }
    };
    for e in &events {
        let line = if json { record(e).to_string() } else { line(e) };
        // A closed pipe (`| head`) ends the output quietly
        if writeln(&line).is_err() {
            break;
        }
    }
    0
}

fn writeln(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    writeln!(std::io::stdout(), "{line}")
}

/// `--since`, `--until` (exclusive) and `--json`
fn parse(args: &[String], today: NaiveDate) -> Result<(NaiveDate, NaiveDate, bool), String> {
    let mut since = None;
    let mut until = None;
    let mut json = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--since" | "--until" => {
                let value = args.next().ok_or(format!("{arg} needs a date"))?;
                let date = parse_date(value, today).ok_or(format!("{arg}: not a date: {value}"))?;
                if arg == "--since" { since = Some(date) } else { until = Some(date) }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let since = since.unwrap_or(today);
    let until = until.unwrap_or(since + Duration::days(7));
    if until <= since {
        return Err("--until must be after --since".into());
    }
    Ok((since, until, json))
}

fn parse_date(value: &str, today: NaiveDate) -> Option<NaiveDate> {
    match value {
        "today" => Some(today),
        "yesterday" => Some(today - Duration::days(1)),
        _ => NaiveDate::parse_from_str(value, "%Y-%m-%d").ok(),
    }
}

async fn fetch(since: NaiveDate, until: NaiveDate) -> Result<Vec<CalendarEvent>, Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let gconfig = config.google.ok_or("no Google calendar configured (run calendarchy once)")?;
    let tokens = config::load_google_tokens()?.ok_or("no saved Google login (run calendarchy once)")?;
    let http = sources::http_client();
    let mut session = GoogleSession::new(http.clone(), gconfig.clone(), tokens);
    let client = CalendarClient::new(http);
    let (client, id) = (&client, gconfig.calendar_id.as_str());
    let last = until - Duration::days(1);
    let events = session.call(|t| async move { client.list_events(&t, id, since, last).await }).await;
    if let Some(tokens) = session.into_refreshed() {
        let _ = config::save_google_tokens(&tokens);
    }
    Ok(events?)
}

/// A start or end: ISO with the local offset, or the date of an all-day one
fn when(t: &EventDateTime) -> Option<String> {
    match (t.date_time, t.date) {
        (Some(dt), _) => Some(iso(dt)),
        (None, Some(d)) => Some(d.to_string()),
        (None, None) => None,
    }
}

fn iso(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// When it starts, as an instant: an all-day event at its day's local midnight
fn start(e: &CalendarEvent) -> Option<String> {
    match (e.start.date_time, e.start.date) {
        (Some(dt), _) => Some(iso(dt)),
        (None, Some(d)) => d.and_hms_opt(0, 0, 0)?.and_local_timezone(Local).earliest().map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, false)),
        (None, None) => None,
    }
}

fn self_response(e: &CalendarEvent) -> &str {
    e.me().and_then(|me| me.response_status.as_deref()).unwrap_or("accepted")
}

fn record(e: &CalendarEvent) -> serde_json::Value {
    let attendees: Vec<&str> = e.attendees.iter().flatten()
        .filter(|a| a.is_self != Some(true))
        .filter_map(|a| a.email.as_deref())
        .filter(|email| !email.ends_with("resource.calendar.google.com"))
        .collect();
    json!({
        "id": e.id,
        "date": start(e),
        "source": SOURCE,
        "start": when(&e.start),
        "end": when(&e.end),
        "all_day": e.start.date_time.is_none(),
        "summary": e.summary,
        "status": e.status,
        "self_response": self_response(e),
        "organizer": e.organizer.as_ref().and_then(|o| o.email.as_deref()),
        "attendees": attendees,
        "recurring_event_id": e.recurring_event_id,
        "event_type": e.event_type.as_deref().unwrap_or("default"),
    })
}

/// `2026-10-22 10:00-10:30 Title (accepted)`
fn line(e: &CalendarEvent) -> String {
    let local = |t: &EventDateTime| t.date_time.map(|dt| dt.with_timezone(&Local));
    let span = match (local(&e.start), local(&e.end)) {
        (Some(s), Some(end)) => format!("{} {}-{}", s.format("%Y-%m-%d"), s.format("%H:%M"), end.format("%H:%M")),
        (Some(s), None) => s.format("%Y-%m-%d %H:%M").to_string(),
        _ => format!("{} all day", e.start.date.map(|d| d.to_string()).unwrap_or_default()),
    };
    let kind = match e.event_type.as_deref() {
        None | Some("default") => String::new(),
        Some(t) => format!(" [{t}]"),
    };
    format!("{span} {}{kind} ({})", e.title(), self_response(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn test_parse_defaults_to_a_week_from_today() {
        let today = day("2026-10-22");
        assert_eq!(parse(&[], today), Ok((today, day("2026-10-29"), false)));
        assert_eq!(parse(&args("--since 2026-01-01 --until 2026-02-01 --json"), today),
                   Ok((day("2026-01-01"), day("2026-02-01"), true)));
        assert_eq!(parse(&args("--since yesterday"), today), Ok((day("2026-10-21"), day("2026-10-28"), false)));
    }

    #[test]
    fn test_parse_rejects_bad_input() {
        let today = day("2026-10-22");
        assert!(parse(&args("--since 2026-13-01"), today).is_err());
        assert!(parse(&args("--since 2026-10-22 --until 2026-10-22"), today).is_err());
        assert!(parse(&args("--until"), today).is_err());
        assert!(parse(&args("--days 3"), today).is_err());
    }

    #[test]
    fn test_record_from_the_api_shape() {
        let event: CalendarEvent = serde_json::from_value(json!({
            "id": "abc_20261022", "summary": "1:1", "status": "confirmed",
            "start": {"dateTime": "2026-10-22T07:00:00Z"}, "end": {"dateTime": "2026-10-22T07:30:00Z"},
            "organizer": {"email": "boss@dext.com"}, "recurringEventId": "abc", "eventType": "default",
            "attendees": [{"email": "me@dext.com", "self": true, "responseStatus": "tentative"},
                          {"email": "boss@dext.com", "responseStatus": "accepted"},
                          {"email": "c_123@resource.calendar.google.com"}]
        })).unwrap();
        let r = record(&event);
        assert_eq!(r["source"], "calendarchy");
        assert_eq!(r["self_response"], "tentative");
        assert_eq!(r["attendees"], json!(["boss@dext.com"]));
        assert_eq!(r["recurring_event_id"], "abc");
        assert_eq!(r["all_day"], false);
        assert_eq!(r["date"], r["start"]);
        assert!(r["start"].as_str().unwrap().starts_with("2026-10-22T"));
    }

    #[test]
    fn test_record_of_an_all_day_event_with_no_guests() {
        let event: CalendarEvent = serde_json::from_value(json!({
            "id": "ooo", "eventType": "outOfOffice",
            "start": {"date": "2026-10-22"}, "end": {"date": "2026-10-24"}
        })).unwrap();
        let r = record(&event);
        assert_eq!(r["all_day"], true);
        assert_eq!(r["start"], "2026-10-22");
        assert_eq!(r["end"], "2026-10-24");
        assert_eq!(r["self_response"], "accepted");
        assert_eq!(r["event_type"], "outOfOffice");
        assert!(r["date"].as_str().unwrap().starts_with("2026-10-22T00:00:00"));
    }
}
