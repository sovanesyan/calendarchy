use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// OAuth2 tokens from Google
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub token_type: String,
}

impl TokenInfo {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at - chrono::Duration::minutes(5)
    }
}

/// Device code response from Google
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
}

/// Token endpoint response
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

/// A calendar event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub summary: Option<String>,
    pub start: EventDateTime,
    pub end: EventDateTime,
    pub location: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub attendees: Option<Vec<Attendee>>,
    pub conference_data: Option<ConferenceData>,
    pub hangout_link: Option<String>,
}

/// Conference/meeting data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConferenceData {
    pub entry_points: Option<Vec<EntryPoint>>,
}

/// Conference entry point (video link, phone, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPoint {
    pub entry_point_type: Option<String>,
    pub uri: Option<String>,
}

/// Event attendee
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub response_status: Option<String>,
    #[serde(rename = "self")]
    pub is_self: Option<bool>,
    pub organizer: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDateTime {
    /// For all-day events
    pub date: Option<NaiveDate>,
    /// For timed events
    pub date_time: Option<DateTime<Utc>>,
    pub time_zone: Option<String>,
}

impl CalendarEvent {
    /// Get the start date (works for both all-day and timed events, in local timezone)
    pub fn start_date(&self) -> Option<NaiveDate> {
        self.start.date.or_else(|| {
            self.start.date_time.map(|dt| {
                let local: DateTime<Local> = dt.with_timezone(&Local);
                local.date_naive()
            })
        })
    }

    /// Get display title
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(No title)")
    }

    /// Get start time as HH:MM or "All day" (converted to local timezone)
    pub fn time_str(&self) -> String {
        self.start
            .date_time
            .map(|dt| {
                let local: DateTime<Local> = dt.with_timezone(&Local);
                format!("{:02}:{:02}", local.time().hour(), local.time().minute())
            })
            .unwrap_or_else(|| "All day".to_string())
    }

    /// Get end time as HH:MM or None for all-day events (converted to local timezone)
    pub fn end_time_str(&self) -> Option<String> {
        self.end.date_time.map(|dt| {
            let local: DateTime<Local> = dt.with_timezone(&Local);
            format!("{:02}:{:02}", local.time().hour(), local.time().minute())
        })
    }

    /// Check if the current user has accepted this event
    /// Returns true if: no attendees (own event), user is organizer, or user accepted
    pub fn is_accepted(&self) -> bool {
        match &self.attendees {
            None => true, // No attendees means it's your own event
            Some(attendees) => {
                // Find the current user's attendance
                for attendee in attendees {
                    if attendee.is_self == Some(true) {
                        return matches!(
                            attendee.response_status.as_deref(),
                            Some("accepted") | Some("organizer") | None
                        );
                    }
                }
                // If no "self" attendee found, assume accepted (organizer or own event)
                true
            }
        }
    }

    /// Check if the current user is the organizer of this event
    pub fn is_organizer(&self) -> bool {
        match &self.attendees {
            None => true, // No attendees means it's your own event
            Some(attendees) => {
                // Check if any attendee is both self and organizer
                for attendee in attendees {
                    if attendee.is_self == Some(true) && attendee.organizer == Some(true) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Extract meeting URL (Zoom, Google Meet, etc.)
    pub fn meeting_url(&self) -> Option<String> {
        // Check hangout_link first (Google Meet)
        if let Some(ref url) = self.hangout_link {
            return Some(url.clone());
        }

        // Check conference data entry points
        if let Some(ref conf) = self.conference_data
            && let Some(ref entry_points) = conf.entry_points {
                for ep in entry_points {
                    if ep.entry_point_type.as_deref() == Some("video")
                        && let Some(ref uri) = ep.uri {
                            return Some(uri.clone());
                        }
                }
            }

        // Check location for meeting URLs
        if let Some(ref loc) = self.location
            && let Some(url) = extract_meeting_url(loc) {
                return Some(url);
            }

        // Check description for meeting URLs
        if let Some(ref desc) = self.description
            && let Some(url) = extract_meeting_url(desc) {
                return Some(url);
            }

        None
    }
}

use crate::utils::extract_meeting_url;

use chrono::Timelike;

/// Response from events.list API
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsListResponse {
    pub items: Option<Vec<CalendarEvent>>,
    pub next_page_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_timed_event(summary: &str, datetime: DateTime<Utc>) -> CalendarEvent {
        CalendarEvent {
            id: "test-id".to_string(),
            summary: Some(summary.to_string()),
            start: EventDateTime {
                date: None,
                date_time: Some(datetime),
                time_zone: None,
            },
            end: EventDateTime {
                date: None,
                date_time: Some(datetime + chrono::Duration::hours(1)),
                time_zone: None,
            },
            location: None,
            description: None,
            status: None,
            attendees: None,
            conference_data: None,
            hangout_link: None,
        }
    }

    fn make_all_day_event(summary: &str, date: NaiveDate) -> CalendarEvent {
        CalendarEvent {
            id: "test-id".to_string(),
            summary: Some(summary.to_string()),
            start: EventDateTime {
                date: Some(date),
                date_time: None,
                time_zone: None,
            },
            end: EventDateTime {
                date: Some(date + chrono::Duration::days(1)),
                date_time: None,
                time_zone: None,
            },
            location: None,
            description: None,
            status: None,
            attendees: None,
            conference_data: None,
            hangout_link: None,
        }
    }

    #[test]
    fn test_event_title_with_summary() {
        let event = make_timed_event("Team Standup", Utc::now());
        assert_eq!(event.title(), "Team Standup");
    }

    #[test]
    fn test_event_title_without_summary() {
        let mut event = make_timed_event("", Utc::now());
        event.summary = None;
        assert_eq!(event.title(), "(No title)");
    }

    #[test]
    fn test_time_str_all_day() {
        let event = make_all_day_event("Holiday", NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert_eq!(event.time_str(), "All day");
    }

    #[test]
    fn test_start_date_timed_event() {
        let dt = DateTime::parse_from_rfc3339("2026-01-15T14:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let event = make_timed_event("Meeting", dt);
        // Note: start_date converts to local time, so this test may vary by timezone
        assert!(event.start_date().is_some());
    }

    #[test]
    fn test_start_date_all_day_event() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let event = make_all_day_event("Holiday", date);
        assert_eq!(event.start_date(), Some(date));
    }

    #[test]
    fn test_is_accepted_no_attendees() {
        let event = make_timed_event("My Event", Utc::now());
        assert!(event.is_accepted());
    }

    #[test]
    fn test_is_accepted_user_accepted() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.attendees = Some(vec![Attendee {
            email: Some("me@example.com".to_string()),
            display_name: None,
            response_status: Some("accepted".to_string()),
            is_self: Some(true),
            organizer: None,
        }]);
        assert!(event.is_accepted());
    }

    #[test]
    fn test_is_accepted_user_declined() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.attendees = Some(vec![Attendee {
            email: Some("me@example.com".to_string()),
            display_name: None,
            response_status: Some("declined".to_string()),
            is_self: Some(true),
            organizer: None,
        }]);
        assert!(!event.is_accepted());
    }

    #[test]
    fn test_is_accepted_user_tentative() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.attendees = Some(vec![Attendee {
            email: Some("me@example.com".to_string()),
            display_name: None,
            response_status: Some("tentative".to_string()),
            is_self: Some(true),
            organizer: None,
        }]);
        assert!(!event.is_accepted());
    }

    #[test]
    fn test_meeting_url_from_hangout_link() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.hangout_link = Some("https://meet.google.com/abc-defg-hij".to_string());
        assert_eq!(
            event.meeting_url(),
            Some("https://meet.google.com/abc-defg-hij".to_string())
        );
    }

    #[test]
    fn test_meeting_url_from_conference_data() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.conference_data = Some(ConferenceData {
            entry_points: Some(vec![EntryPoint {
                entry_point_type: Some("video".to_string()),
                uri: Some("https://zoom.us/j/123456789".to_string()),
            }]),
        });
        assert_eq!(
            event.meeting_url(),
            Some("https://zoom.us/j/123456789".to_string())
        );
    }

    #[test]
    fn test_meeting_url_from_location() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.location = Some("Join at https://zoom.us/j/987654321".to_string());
        assert_eq!(
            event.meeting_url(),
            Some("https://zoom.us/j/987654321".to_string())
        );
    }

    #[test]
    fn test_meeting_url_from_description() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.description = Some("Click here: https://teams.microsoft.com/l/meetup-join/123".to_string());
        assert_eq!(
            event.meeting_url(),
            Some("https://teams.microsoft.com/l/meetup-join/123".to_string())
        );
    }

    #[test]
    fn test_token_is_expired() {
        let expired_token = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() - chrono::Duration::hours(1),
            token_type: "Bearer".to_string(),
        };
        assert!(expired_token.is_expired());

        let valid_token = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            token_type: "Bearer".to_string(),
        };
        assert!(!valid_token.is_expired());
    }

    #[test]
    fn test_token_expires_within_5_min_buffer() {
        // Token expiring in 4 minutes should be considered expired (5 min buffer)
        let almost_expired = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::minutes(4),
            token_type: "Bearer".to_string(),
        };
        assert!(almost_expired.is_expired());

        // Token expiring in 6 minutes should be valid
        let still_valid = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::minutes(6),
            token_type: "Bearer".to_string(),
        };
        assert!(!still_valid.is_expired());
    }
}
