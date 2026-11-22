//! Writing events we own to an iCloud calendar, for scripts and jobs (the
//! eBag deliveries on Family):
//!
//! - `calendarchy put --calendar NAME --uid UID --start WHEN --end WHEN
//!   --title TEXT [--notes TEXT] [--alarm MINUTES]`: create or replace the
//!   event at `<calendar>/<uid>.ics`. The same uid again overwrites it, so a
//!   caller can re-run it after any change.
//! - `calendarchy remove --calendar NAME --uid UID`: delete that event (gone
//!   already counts as removed).
//!
//! NAME is a calendar's name as discovery saved it (`Family`), matched
//! exactly. WHEN is RFC 3339 (`2026-11-15T16:00:00+02:00`) or local
//! `YYYY-MM-DD HH:MM`. Only an event at the path its uid gives is ever
//! touched, so other people's events on a shared calendar are safe.
//!
//! Exit codes: 0 ok, 1 the request failed (no login, network, iCloud), 2 usage.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

use crate::config::Config;
use crate::icloud::CalDavClient;
use crate::sources;

const USAGE: &str = "\
usage: calendarchy put --calendar NAME --uid UID --start WHEN --end WHEN --title TEXT [--notes TEXT] [--alarm MINUTES]
       calendarchy remove --calendar NAME --uid UID
       (WHEN: 2026-11-15T16:00:00+02:00, or local \"2026-11-15 16:00\")";

/// An event to save
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub uid: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub title: String,
    pub notes: Option<String>,
    /// Minutes before the start for an alert
    pub alarm: Option<u32>,
}

#[derive(Debug, PartialEq)]
enum Command {
    Put { calendar: String, event: NewEvent },
    Remove { calendar: String, uid: String },
}

/// `put` and `remove`: `verb` is which, `args` what follows it
pub async fn run(verb: &str, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let command = match parse(verb, args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("calendarchy {verb}: {e}\n{USAGE}");
            return 2;
        }
    };
    match execute(&command).await {
        Ok(done) => {
            println!("{done}");
            0
        }
        Err(e) => {
            eprintln!("calendarchy {verb}: {e}");
            1
        }
    }
}

async fn execute(command: &Command) -> Result<String, String> {
    let config = Config::load().unwrap_or_default();
    let icloud = config.icloud.ok_or("no iCloud account in the config")?;
    let (Command::Put { calendar, .. } | Command::Remove { calendar, .. }) = command;
    let account = sources::saved_icloud_account();
    let entry = account
        .calendars
        .iter()
        .find(|c| c.name.as_deref() == Some(calendar.as_str()))
        .ok_or_else(|| {
            let names: Vec<_> = account.calendars.iter().filter_map(|c| c.name.as_deref()).collect();
            if names.is_empty() {
                "no discovered iCloud calendars (run calendarchy once to sign in)".to_string()
            } else {
                format!("no iCloud calendar named {calendar:?} (there are: {})", names.join(", "))
            }
        })?;
    let client = CalDavClient::new(sources::http_client(), &icloud);
    match command {
        Command::Put { event, .. } => {
            client.put_event(&entry.url, &event.uid, ics(event, Utc::now())).await.map_err(|e| e.to_string())?;
            Ok(format!("put {} in {calendar}", event.uid))
        }
        Command::Remove { uid, .. } => {
            client.delete_event(&entry.url, uid, None, None).await.map_err(|e| e.to_string())?;
            Ok(format!("removed {uid} from {calendar}"))
        }
    }
}

fn parse(verb: &str, args: &[String]) -> Result<Command, String> {
    let mut values: Vec<(&str, &str)> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let known = match verb {
            "put" => ["--calendar", "--uid", "--start", "--end", "--title", "--notes", "--alarm"].as_slice(),
            _ => ["--calendar", "--uid"].as_slice(),
        };
        if !known.contains(&arg.as_str()) {
            return Err(format!("unknown argument {arg}"));
        }
        let value = it.next().ok_or(format!("{arg} needs a value"))?;
        values.push((arg.as_str(), value.as_str()));
    }
    let get = |name: &str| values.iter().rev().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
    let need = |name: &str| get(name).filter(|v| !v.trim().is_empty()).ok_or(format!("{name} is required"));

    let calendar = need("--calendar")?;
    let uid = need("--uid")?;
    // The uid is the resource name too: keep it to what's safe in a URL path
    if !uid.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@".contains(c)) {
        return Err(format!("--uid: only letters, digits and - _ . @ (got {uid:?})"));
    }
    if verb == "remove" {
        return Ok(Command::Remove { calendar, uid });
    }
    let when = |name: &str| -> Result<DateTime<Utc>, String> {
        let value = need(name)?;
        parse_when(&value).ok_or(format!("{name}: not a time: {value}"))
    };
    let (start, end) = (when("--start")?, when("--end")?);
    if end <= start {
        return Err("--end must be after --start".into());
    }
    let alarm = match get("--alarm") {
        Some(m) => Some(m.parse::<u32>().map_err(|_| format!("--alarm: minutes, not {m:?}"))?),
        None => None,
    };
    let notes = get("--notes").filter(|n| !n.is_empty());
    Ok(Command::Put { calendar, event: NewEvent { uid, start, end, title: need("--title")?, notes, alarm } })
}

fn parse_when(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(value) {
        return Some(t.with_timezone(&Utc));
    }
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").ok()?;
    Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc))
}

/// The event as a VCALENDAR (times in UTC, CRLF, long lines folded)
pub fn ics(e: &NewEvent, now: DateTime<Utc>) -> String {
    let stamp = |t: &DateTime<Utc>| t.format("%Y%m%dT%H%M%SZ").to_string();
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//calendarchy//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", e.uid),
        format!("DTSTAMP:{}", stamp(&now)),
        format!("DTSTART:{}", stamp(&e.start)),
        format!("DTEND:{}", stamp(&e.end)),
        format!("SUMMARY:{}", escape(&e.title)),
    ];
    if let Some(notes) = &e.notes {
        lines.push(format!("DESCRIPTION:{}", escape(notes)));
    }
    if let Some(minutes) = e.alarm {
        lines.extend([
            "BEGIN:VALARM".to_string(),
            "ACTION:DISPLAY".into(),
            format!("DESCRIPTION:{}", escape(&e.title)),
            format!("TRIGGER:-PT{minutes}M"),
            "END:VALARM".into(),
        ]);
    }
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    lines.iter().map(|l| fold(l)).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

/// RFC 5545 text escaping
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

/// Fold a content line at 75 octets, never inside a UTF-8 character
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn test_put_parses_rfc3339_times_alarm_and_notes() {
        let got = parse(
            "put",
            &args(&["--calendar", "Family", "--uid", "ebag-ABC123", "--start", "2026-11-15T16:00:00+02:00",
                "--end", "2026-11-15T17:00:00+02:00", "--title", "🛒 eBag delivery", "--alarm", "30"]),
        )
        .unwrap();
        let Command::Put { calendar, event } = got else { panic!("not a put") };
        assert_eq!(calendar, "Family");
        assert_eq!(event.start, utc("2026-11-15T14:00:00Z"));
        assert_eq!(event.end, utc("2026-11-15T15:00:00Z"));
        assert_eq!(event.alarm, Some(30));
        assert_eq!(event.notes, None);
    }

    #[test]
    fn test_put_and_remove_refuse_bad_input() {
        let base = ["--calendar", "Family", "--uid", "x", "--title", "t"];
        let with = |extra: &[&str]| parse("put", &args(&[&base[..], extra].concat()));
        assert!(with(&["--start", "2026-11-15 16:00"]).unwrap_err().contains("--end is required"));
        assert!(with(&["--start", "2026-11-15 17:00", "--end", "2026-11-15 16:00"]).unwrap_err().contains("after"));
        assert!(with(&["--start", "tomorrow", "--end", "2026-11-15 16:00"]).unwrap_err().contains("not a time"));
        assert!(with(&["--start", "2026-11-15 16:00", "--end", "2026-11-15 17:00", "--alarm", "soon"]).is_err());
        assert!(parse("put", &args(&["--calendar", "Family", "--uid", "../x"])).unwrap_err().contains("--uid"));
        assert!(parse("remove", &args(&["--calendar", "Family"])).unwrap_err().contains("--uid is required"));
        assert!(parse("remove", &args(&["--calendar", "Family", "--uid", "a", "--title", "t"])).unwrap_err().contains("unknown"));
        assert_eq!(
            parse("remove", &args(&["--calendar", "Family", "--uid", "ebag-1"])).unwrap(),
            Command::Remove { calendar: "Family".into(), uid: "ebag-1".into() }
        );
    }

    #[test]
    fn test_ics_has_utc_times_alarm_escaping_and_crlf() {
        let e = NewEvent {
            uid: "ebag-ABC".into(),
            start: utc("2026-11-15T14:00:00Z"),
            end: utc("2026-11-15T15:00:00Z"),
            title: "eBag, delivery; now".into(),
            notes: Some("line one\nline two".into()),
            alarm: Some(30),
        };
        let text = ics(&e, utc("2026-11-14T10:00:00Z"));
        assert!(text.ends_with("END:VCALENDAR\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
        for want in ["UID:ebag-ABC", "DTSTAMP:20261114T100000Z", "DTSTART:20261115T140000Z", "DTEND:20261115T150000Z",
            "SUMMARY:eBag\\, delivery\\; now", "DESCRIPTION:line one\\nline two", "TRIGGER:-PT30M"] {
            assert!(text.contains(&format!("{want}\r\n")), "missing {want} in {text}");
        }
        let plain = NewEvent { notes: None, alarm: None, ..e };
        assert!(!ics(&plain, Utc::now()).contains("VALARM"));
    }

    #[test]
    fn test_fold_keeps_lines_to_75_octets_and_characters_whole() {
        let folded = fold(&format!("SUMMARY:{}", "доставка ".repeat(20)));
        for line in folded.split("\r\n") {
            assert!(line.len() <= 75, "{} octets: {line}", line.len());
        }
        assert_eq!(folded.replace("\r\n ", ""), format!("SUMMARY:{}", "доставка ".repeat(20)));
    }
}
