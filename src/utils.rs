//! Shared utility functions

use crate::cache::{AttendeeStatus, DisplayAttendee};

/// Open a URL in the default browser (platform-specific)
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("open")
        .arg(url)
        .process_group(0)
        .spawn();
}

#[cfg(target_os = "linux")]
pub fn open_url(url: &str) {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .process_group(0)
        .spawn();
}

/// Sort order for attendee status (lower = first)
pub fn status_sort_order(status: &AttendeeStatus) -> u8 {
    match status {
        AttendeeStatus::Organizer => 0,
        AttendeeStatus::Accepted => 1,
        AttendeeStatus::Tentative => 2,
        AttendeeStatus::NeedsAction => 3,
        AttendeeStatus::Declined => 4,
    }
}

/// Sort attendees by status (accepted first, declined last), then by name
pub fn sort_attendees(attendees: &mut [DisplayAttendee]) {
    attendees.sort_by(|a, b| {
        let status_cmp = status_sort_order(&a.status).cmp(&status_sort_order(&b.status));
        if status_cmp != std::cmp::Ordering::Equal {
            status_cmp
        } else {
            a.name.cmp(&b.name)
        }
    });
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
    fn test_status_sort_order() {
        assert!(status_sort_order(&AttendeeStatus::Organizer) < status_sort_order(&AttendeeStatus::Accepted));
        assert!(status_sort_order(&AttendeeStatus::Accepted) < status_sort_order(&AttendeeStatus::Tentative));
        assert!(status_sort_order(&AttendeeStatus::Tentative) < status_sort_order(&AttendeeStatus::NeedsAction));
        assert!(status_sort_order(&AttendeeStatus::NeedsAction) < status_sort_order(&AttendeeStatus::Declined));
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
