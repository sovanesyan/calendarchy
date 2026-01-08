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
    #[allow(dead_code)]
    pub interval: u64,
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
pub struct CalendarEvent {
    pub id: String,
    pub summary: Option<String>,
    pub start: EventDateTime,
    pub end: EventDateTime,
    pub location: Option<String>,
    pub status: Option<String>,
    pub attendees: Option<Vec<Attendee>>,
}

/// Event attendee
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email: Option<String>,
    pub response_status: Option<String>,
    #[serde(rename = "self")]
    pub is_self: Option<bool>,
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
}

use chrono::Timelike;

/// Response from events.list API
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsListResponse {
    pub items: Option<Vec<CalendarEvent>>,
    pub next_page_token: Option<String>,
}
