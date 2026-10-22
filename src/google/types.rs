use chrono::{DateTime, NaiveDate, Utc};
#[cfg(test)]
use chrono::{Local, Timelike};
use serde::{Deserialize, Serialize};

/// OAuth2 tokens from Google
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl TokenInfo {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at - chrono::Duration::minutes(5)
    }
}

/// Token endpoint response
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
}

impl TokenResponse {
    /// The tokens to keep. A refresh usually comes back without a refresh
    /// token (the old one stays valid), so `current_refresh` is kept unless
    /// Google rotated it.
    pub fn into_info(self, current_refresh: Option<String>) -> TokenInfo {
        TokenInfo {
            access_token: self.access_token,
            refresh_token: self.refresh_token.or(current_refresh),
            expires_at: Utc::now() + chrono::Duration::seconds(self.expires_in as i64),
        }
    }
}

/// A calendar event (only the fields we read)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    pub id: String,
    pub summary: Option<String>,
    pub start: EventDateTime,
    pub end: EventDateTime,
    pub location: Option<String>,
    pub description: Option<String>,
    /// "transparent" = free, "opaque" = busy (default)
    pub transparency: Option<String>,
    pub attendees: Option<Vec<Attendee>>,
    pub conference_data: Option<ConferenceData>,
    pub hangout_link: Option<String>,
    /// "confirmed", "tentative" or "cancelled"
    pub status: Option<String>,
    pub organizer: Option<Organizer>,
    /// Set on an occurrence of a recurring event: the series' id
    pub recurring_event_id: Option<String>,
    /// "default", "outOfOffice", "focusTime", "workingLocation", ...
    pub event_type: Option<String>,
}

/// Conference/meeting data
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConferenceData {
    pub entry_points: Option<Vec<EntryPoint>>,
}

/// Conference entry point (video link, phone, etc.)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPoint {
    pub entry_point_type: Option<String>,
    pub uri: Option<String>,
}

/// Who organises an event
#[derive(Debug, Clone, Deserialize)]
pub struct Organizer {
    pub email: Option<String>,
}

/// Event attendee
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub response_status: Option<String>,
    #[serde(rename = "self")]
    pub is_self: Option<bool>,
    pub organizer: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDateTime {
    /// For all-day events
    pub date: Option<NaiveDate>,
    /// For timed events
    pub date_time: Option<DateTime<Utc>>,
}

impl CalendarEvent {
    /// Get display title
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(No title)")
    }

    #[cfg(test)]
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

    /// Your own entry on the guest list, if you're on it
    pub fn me(&self) -> Option<&Attendee> {
        self.attendees.as_ref()?.iter().find(|a| a.is_self == Some(true))
    }

    /// Whether you're going: no guests or not on the list means it's your own
    /// event; otherwise you organise it or accepted
    pub fn is_accepted(&self) -> bool {
        self.me().is_none_or(|me| matches!(me.response_status.as_deref(), Some("accepted") | Some("organizer") | None))
    }

    /// Check if the event is marked as "free" (doesn't block time)
    pub fn is_free(&self) -> bool {
        self.transparency.as_deref() == Some("transparent")
    }

    /// Meeting link: Google Meet, then a conference video entry (Zoom etc.),
    /// then one found in the location or description
    pub fn meeting_url(&self) -> Option<String> {
        let video = || {
            self.conference_data.as_ref()?.entry_points.as_ref()?.iter()
                .find(|ep| ep.entry_point_type.as_deref() == Some("video"))?
                .uri.as_deref()
        };
        meeting_url(
            self.hangout_link.as_deref().or_else(video),
            self.location.as_deref(),
            self.description.as_deref(),
        )
    }
}

use crate::utils::meeting_url;

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
            },
            end: EventDateTime {
                date: None,
                date_time: Some(datetime + chrono::Duration::hours(1)),
            },
            location: None,
            description: None,
            transparency: None,
            attendees: None,
            conference_data: None,
            hangout_link: None,
            status: None,
            organizer: None,
            recurring_event_id: None,
            event_type: None,
        }
    }

    fn make_all_day_event(summary: &str, date: NaiveDate) -> CalendarEvent {
        CalendarEvent {
            id: "test-id".to_string(),
            summary: Some(summary.to_string()),
            start: EventDateTime {
                date: Some(date),
                date_time: None,
            },
            end: EventDateTime {
                date: Some(date + chrono::Duration::days(1)),
                date_time: None,
            },
            location: None,
            description: None,
            transparency: None,
            attendees: None,
            conference_data: None,
            hangout_link: None,
            status: None,
            organizer: None,
            recurring_event_id: None,
            event_type: None,
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
    fn test_refresh_keeps_or_rotates_the_refresh_token() {
        let response = |refresh: Option<&str>| TokenResponse { access_token: "a".into(), refresh_token: refresh.map(Into::into), expires_in: 3600 };
        let kept = response(None).into_info(Some("old".into()));
        assert_eq!(kept.refresh_token.as_deref(), Some("old"));
        assert!(!kept.is_expired());
        assert_eq!(response(Some("new")).into_info(Some("old".into())).refresh_token.as_deref(), Some("new"));
    }

    #[test]
    fn test_is_accepted_when_not_on_the_guest_list() {
        let mut event = make_timed_event("Meeting", Utc::now());
        event.attendees = Some(vec![Attendee {
            email: Some("other@example.com".to_string()),
            display_name: None,
            response_status: Some("declined".to_string()),
            is_self: None,
            organizer: None,
        }]);
        assert!(event.is_accepted());
    }

    #[test]
    fn test_token_is_expired() {
        let expired_token = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() - chrono::Duration::hours(1),
        };
        assert!(expired_token.is_expired());

        let valid_token = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
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
        };
        assert!(almost_expired.is_expired());

        // Token expiring in 6 minutes should be valid
        let still_valid = TokenInfo {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::minutes(6),
        };
        assert!(!still_valid.is_expired());
    }
}
