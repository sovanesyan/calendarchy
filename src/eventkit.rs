//! EventKit integration via Swift helper binary (macOS only)

use crate::cache::{AttendeeStatus, DisplayAttendee, DisplayEvent, EventId, When, DAY_MINUTES};
use crate::utils;
use chrono::NaiveDate;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct EKEvent {
    title: String,
    date: String,
    all_day: bool,
    calendar_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    calendar_type: String,
    #[serde(default = "default_true")]
    accepted: bool,
    #[serde(default)]
    is_organizer: bool,
    #[serde(default)]
    is_free: bool,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    meeting_url: Option<String>,
    #[serde(default)]
    attendees: Vec<EKAttendee>,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
struct EKAttendee {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: String,
    #[serde(default)]
    status: String,
}

/// Find the calendarchy-eventkit binary
pub fn find_binary() -> Option<PathBuf> {
    // Check next to the current executable
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap().join("calendarchy-eventkit");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    // Check PATH
    if let Ok(output) = Command::new("which").arg("calendarchy-eventkit").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

/// Check if EventKit is available (macOS only, binary exists)
pub fn is_available() -> bool {
    cfg!(target_os = "macos") && find_binary().is_some()
}

/// Fetch events for a date range via the EventKit helper
pub fn fetch_events(start: NaiveDate, end: NaiveDate) -> Result<Vec<DisplayEvent>, String> {
    let binary = find_binary().ok_or("calendarchy-eventkit binary not found")?;

    let output = Command::new(&binary)
        .arg(start.format("%Y-%m-%d").to_string())
        .arg(end.format("%Y-%m-%d").to_string())
        .output()
        .map_err(|e| format!("Failed to run eventkit helper: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("EventKit: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ek_events: Vec<EKEvent> = serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse eventkit output: {}", e))?;

    let events = ek_events.into_iter().map(|e| ek_event_to_display(e)).collect();
    Ok(events)
}

fn ek_event_to_display(e: EKEvent) -> DisplayEvent {
    let date = NaiveDate::parse_from_str(&e.date, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Local::now().date_naive());

    // The helper reports one day per event with "HH:MM" times; an end before
    // the start means it runs past midnight (equal = zero-length)
    let minutes = |t: &str| {
        let (h, m) = t.split_once(':')?;
        Some(h.parse::<u16>().ok()? * 60 + m.parse::<u16>().ok()?)
    };
    let when = match (e.all_day, e.start_time.as_deref().and_then(minutes)) {
        (false, Some(start)) => When::Timed {
            start,
            end: e.end_time.as_deref().and_then(minutes).map(|end| if end < start { DAY_MINUTES } else { end }),
        },
        _ => When::AllDay,
    };

    let meeting_url = e.meeting_url.or_else(|| {
        e.location.as_deref().and_then(|l| utils::extract_meeting_url(l))
            .or_else(|| e.description.as_deref().and_then(|d| utils::extract_meeting_url(d)))
    });

    let attendees: Vec<DisplayAttendee> = e.attendees.into_iter().map(|a| {
        let status = match a.status.as_str() {
            "accepted" => AttendeeStatus::Accepted,
            "declined" => AttendeeStatus::Declined,
            "tentative" => AttendeeStatus::Tentative,
            _ => AttendeeStatus::NeedsAction,
        };
        DisplayAttendee {
            name: a.name,
            email: a.email,
            status,
        }
    }).collect();

    DisplayEvent {
        id: EventId::ICloud {
            calendar_url: String::new(),
            event_uid: format!("eventkit-{}-{}-{}", e.calendar_name, date, e.title),
            etag: None,
            calendar_name: Some(e.calendar_name),
            href: None,
        },
        title: e.title,
        when,
        date,
        spans_days: false,
        accepted: e.accepted,
        is_organizer: e.is_organizer,
        is_free: e.is_free,
        meeting_url,
        description: e.description,
        location: e.location,
        attendees,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ek(start: &str, end: &str) -> EKEvent {
        EKEvent {
            title: "t".into(), date: "2026-10-05".into(), all_day: false, calendar_name: "c".into(),
            calendar_type: String::new(), accepted: true, is_organizer: false, is_free: false,
            start_time: Some(start.into()), end_time: Some(end.into()),
            location: None, description: None, meeting_url: None, attendees: vec![],
        }
    }

    #[test]
    fn test_zero_length_event_stays_zero_length() {
        assert_eq!(ek_event_to_display(ek("10:00", "10:00")).when, When::Timed { start: 600, end: Some(600) });
    }

    #[test]
    fn test_end_before_start_runs_to_midnight() {
        assert_eq!(ek_event_to_display(ek("22:00", "01:00")).when, When::Timed { start: 1320, end: Some(DAY_MINUTES) });
    }
}
