//! Shared utility functions

use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, NaiveTime, TimeZone, Utc};

use crate::cache::DisplayAttendee;

/// Open a URL in the default browser
pub fn open_url(url: &str) {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .process_group(0)
        .spawn();
}

/// The first of the month containing `date`
pub fn month_start(date: NaiveDate) -> NaiveDate {
    date - Days::new(date.day0().into())
}

/// The first of the month `delta` months after (before, if negative) the
/// one containing `date`
pub fn add_months(date: NaiveDate, delta: i32) -> NaiveDate {
    let first = month_start(date);
    let months = Months::new(delta.unsigned_abs());
    let shifted = if delta < 0 { first.checked_sub_months(months) } else { first.checked_add_months(months) };
    shifted.unwrap_or(first)
}

/// First and last day of the month containing `date`
pub fn month_range(date: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = month_start(date);
    let last = first.checked_add_months(Months::new(1)).and_then(|next| next.pred_opt()).unwrap_or(first);
    (first, last)
}

/// Local midnight at the start of `start` and at the end of `end`, as UTC
/// instants: fetches cover whole local days, so events in the first or last
/// hours of a month aren't lost to the UTC offset
pub fn local_day_bounds(start: NaiveDate, end: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = |d: NaiveDate| {
        let naive = d.and_time(NaiveTime::MIN);
        // No local midnight that day (a DST jump): fall back to UTC midnight
        Local.from_local_datetime(&naive).earliest().map_or_else(|| naive.and_utc(), |dt| dt.with_timezone(&Utc))
    };
    (midnight(start), midnight(end + Days::new(1)))
}

/// A meeting link: the event's own (`explicit`), else the first one found in
/// its location, else in its description
pub fn meeting_url(explicit: Option<&str>, location: Option<&str>, description: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .or_else(|| location.and_then(extract_meeting_url))
        .or_else(|| description.and_then(extract_meeting_url))
}

/// Sort attendees by status (organizer first, declined last), then by name
pub fn sort_attendees(attendees: &mut [DisplayAttendee]) {
    attendees.sort_by(|a, b| (&a.status, &a.name).cmp(&(&b.status, &b.name)));
}

/// Extract a display name from an email address
/// e.g., "john.smith@example.com" -> "John Smith"
///       "jsmith@example.com" -> "Jsmith"
pub fn name_from_email(email: &str) -> String {
    // Get the part before @
    let local = email.split('@').next().unwrap_or(email);

    // Split by common separators (., _, -)
    let parts: Vec<&str> = local.split(['.', '_', '-']).collect();

    // Capitalize each part and join with space
    parts
        .iter()
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if a URL is a meeting URL (Zoom, Meet, Teams)
pub fn is_meeting_url(url: &str) -> bool {
    url.contains("zoom.us")
        || url.contains("meet.google.com")
        || url.contains("teams.microsoft.com")
}

/// Extract a meeting URL (Zoom, Meet, Teams) from text
pub fn extract_meeting_url(text: &str) -> Option<String> {
    // First try flexible patterns that match any subdomain
    let flexible_patterns = ["zoom.us/j/", "meet.google.com/", "teams.microsoft.com/"];

    for pattern in flexible_patterns {
        if let Some(pattern_pos) = text.find(pattern) {
            // Find the start of the URL (search backwards for https://)
            let before = &text[..pattern_pos];
            if let Some(https_offset) = before.rfind("https://") {
                let url_part = &text[https_offset..];
                let end = url_part
                    .find(|c: char| c.is_whitespace() || c == '"' || c == '>' || c == '<')
                    .unwrap_or(url_part.len());
                return Some(url_part[..end].to_string());
            }
        }
    }
    None
}

/// Convert a Zoom join URL to its zoommtg:// deep link so the Zoom app
/// (or the system's zoommtg handler) opens directly, skipping the browser
/// landing page. Returns None for URLs that aren't Zoom /j/ links.
/// e.g., "https://dext.zoom.us/j/123?pwd=abc" -> "zoommtg://dext.zoom.us/join?action=join&confno=123&pwd=abc"
pub fn to_zoom_deeplink(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    if host != "zoom.us" && !host.ends_with(".zoom.us") {
        return None;
    }
    let after_j = path.strip_prefix("j/")?;
    let (confno, query) = match after_j.split_once('?') {
        Some((c, q)) => (c, Some(q)),
        None => (after_j, None),
    };
    if confno.is_empty() || !confno.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut deeplink = format!("zoommtg://{host}/join?action=join&confno={confno}");
    if let Some(query) = query
        && let Some(pwd) = query.split('&').find_map(|p| p.strip_prefix("pwd=")) {
            deeplink.push_str("&pwd=");
            deeplink.push_str(pwd);
        }
    Some(deeplink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::AttendeeStatus;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn test_month_range() {
        assert_eq!(month_range(d(2026, 2, 14)), (d(2026, 2, 1), d(2026, 2, 28)));
        assert_eq!(month_range(d(2028, 2, 29)), (d(2028, 2, 1), d(2028, 2, 29)));
        assert_eq!(month_range(d(2026, 12, 31)), (d(2026, 12, 1), d(2026, 12, 31)));
    }

    #[test]
    fn test_add_months() {
        assert_eq!(add_months(d(2026, 12, 1), 1), d(2027, 1, 1));
        assert_eq!(add_months(d(2026, 1, 1), -1), d(2025, 12, 1));
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 1), "always the first of the month");
        assert_eq!(add_months(d(2026, 3, 31), -13), d(2025, 2, 1));
        assert_eq!(month_start(d(2026, 10, 16)), d(2026, 10, 1));
    }

    #[test]
    fn test_local_day_bounds_cover_whole_local_days() {
        let (start, end) = local_day_bounds(d(2026, 10, 1), d(2026, 10, 31));
        assert_eq!(start.with_timezone(&Local).date_naive(), d(2026, 10, 1));
        assert_eq!(end.with_timezone(&Local).date_naive(), d(2026, 11, 1));
        use chrono::Timelike;
        assert_eq!((start.with_timezone(&Local).hour(), end.with_timezone(&Local).hour()), (0, 0));
    }

    #[test]
    fn test_meeting_url_order() {
        let zoom = "https://zoom.us/j/1";
        let meet = "see https://meet.google.com/abc-defg-hij";
        assert_eq!(meeting_url(Some("https://x.test/own"), Some(zoom), None).as_deref(), Some("https://x.test/own"));
        assert_eq!(meeting_url(None, Some(zoom), Some(meet)).as_deref(), Some(zoom));
        assert_eq!(meeting_url(None, Some("Room 4"), Some(meet)).as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(meeting_url(None, None, None), None);
    }

    #[test]
    fn test_is_meeting_url() {
        assert!(is_meeting_url("https://zoom.us/j/123"));
        assert!(is_meeting_url("https://meet.google.com/abc"));
        assert!(is_meeting_url("https://teams.microsoft.com/l/meetup"));
        assert!(!is_meeting_url("https://example.com"));
    }

    #[test]
    fn test_extract_meeting_url_zoom_variants() {
        assert_eq!(
            extract_meeting_url("https://us02web.zoom.us/j/123"),
            Some("https://us02web.zoom.us/j/123".to_string())
        );
        assert_eq!(
            extract_meeting_url("https://us04web.zoom.us/j/456"),
            Some("https://us04web.zoom.us/j/456".to_string())
        );
        // Custom corporate subdomain
        assert_eq!(
            extract_meeting_url("https://dext.zoom.us/j/98429926780?pwd=abc"),
            Some("https://dext.zoom.us/j/98429926780?pwd=abc".to_string())
        );
    }

    #[test]
    fn test_extract_meeting_url_with_surrounding_text() {
        let text = "Join meeting at https://meet.google.com/abc-def-ghi and bring notes";
        assert_eq!(
            extract_meeting_url(text),
            Some("https://meet.google.com/abc-def-ghi".to_string())
        );
    }

    #[test]
    fn test_extract_meeting_url_none() {
        assert_eq!(extract_meeting_url("No meeting link here"), None);
        assert_eq!(extract_meeting_url("https://example.com/not-a-meeting"), None);
    }

    #[test]
    fn test_to_zoom_deeplink_with_pwd() {
        assert_eq!(
            to_zoom_deeplink("https://dext.zoom.us/j/98429926780?pwd=abc.123"),
            Some("zoommtg://dext.zoom.us/join?action=join&confno=98429926780&pwd=abc.123".to_string())
        );
    }

    #[test]
    fn test_to_zoom_deeplink_without_pwd() {
        assert_eq!(
            to_zoom_deeplink("https://us02web.zoom.us/j/123456789"),
            Some("zoommtg://us02web.zoom.us/join?action=join&confno=123456789".to_string())
        );
        assert_eq!(
            to_zoom_deeplink("https://zoom.us/j/123456789"),
            Some("zoommtg://zoom.us/join?action=join&confno=123456789".to_string())
        );
    }

    #[test]
    fn test_to_zoom_deeplink_ignores_other_query_params() {
        assert_eq!(
            to_zoom_deeplink("https://zoom.us/j/123?uname=Serge&pwd=xyz&omn=1"),
            Some("zoommtg://zoom.us/join?action=join&confno=123&pwd=xyz".to_string())
        );
    }

    #[test]
    fn test_to_zoom_deeplink_non_join_urls() {
        // Personal room links and non-Zoom URLs stay in the browser
        assert_eq!(to_zoom_deeplink("https://dext.zoom.us/my/serge"), None);
        assert_eq!(to_zoom_deeplink("https://meet.google.com/abc-def-ghi"), None);
        assert_eq!(to_zoom_deeplink("https://notzoom.us/j/123"), None);
        assert_eq!(to_zoom_deeplink("https://evil.com/zoom.us/j/123"), None);
        assert_eq!(to_zoom_deeplink("https://zoom.us/j/not-digits"), None);
    }

    #[test]
    fn test_name_from_email_with_dots() {
        assert_eq!(name_from_email("john.smith@example.com"), "John Smith");
    }

    #[test]
    fn test_name_from_email_with_underscore() {
        assert_eq!(name_from_email("john_smith@example.com"), "John Smith");
    }

    #[test]
    fn test_name_from_email_simple() {
        assert_eq!(name_from_email("jsmith@example.com"), "Jsmith");
    }

    #[test]
    fn test_sort_attendees_by_status() {
        let mut attendees = vec![
            DisplayAttendee {
                name: Some("Bob".to_string()),
                email: "bob@example.com".to_string(),
                status: AttendeeStatus::Declined,
            },
            DisplayAttendee {
                name: Some("Alice".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
            DisplayAttendee {
                name: Some("Charlie".to_string()),
                email: "charlie@example.com".to_string(),
                status: AttendeeStatus::Organizer,
            },
        ];

        sort_attendees(&mut attendees);

        assert_eq!(attendees[0].name, Some("Charlie".to_string())); // Organizer
        assert_eq!(attendees[1].name, Some("Alice".to_string()));   // Accepted
        assert_eq!(attendees[2].name, Some("Bob".to_string()));     // Declined
    }
}
