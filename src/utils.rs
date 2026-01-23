//! Shared utility functions

use crate::cache::{AttendeeStatus, DisplayAttendee};

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
