//! Shared utility functions

/// Common meeting URL patterns
pub const MEETING_URL_PATTERNS: &[&str] = &[
    "https://zoom.us/",
    "https://us02web.zoom.us/",
    "https://us04web.zoom.us/",
    "https://us05web.zoom.us/",
    "https://us06web.zoom.us/",
    "https://meet.google.com/",
    "https://teams.microsoft.com/",
];

/// Check if a URL is a meeting URL (Zoom, Meet, Teams)
pub fn is_meeting_url(url: &str) -> bool {
    url.contains("zoom.us")
        || url.contains("meet.google.com")
        || url.contains("teams.microsoft.com")
}

/// Extract a meeting URL (Zoom, Meet, Teams) from text
pub fn extract_meeting_url(text: &str) -> Option<String> {
    for pattern in MEETING_URL_PATTERNS {
        if let Some(start) = text.find(pattern) {
            // Extract URL until whitespace or end
            let url_part = &text[start..];
            let end = url_part
                .find(|c: char| c.is_whitespace() || c == '"' || c == '>' || c == '<')
                .unwrap_or(url_part.len());
            return Some(url_part[..end].to_string());
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
}
