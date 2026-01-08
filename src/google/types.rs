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

    /// Extract meeting URL (Zoom, Google Meet, etc.)
    pub fn meeting_url(&self) -> Option<String> {
        // Check hangout_link first (Google Meet)
        if let Some(ref url) = self.hangout_link {
            return Some(url.clone());
        }

        // Check conference data entry points
        if let Some(ref conf) = self.conference_data {
            if let Some(ref entry_points) = conf.entry_points {
                for ep in entry_points {
                    if ep.entry_point_type.as_deref() == Some("video") {
                        if let Some(ref uri) = ep.uri {
                            return Some(uri.clone());
                        }
                    }
                }
            }
        }

        // Check location for meeting URLs
        if let Some(ref loc) = self.location {
            if let Some(url) = extract_meeting_url(loc) {
                return Some(url);
            }
        }

        // Check description for meeting URLs
        if let Some(ref desc) = self.description {
            if let Some(url) = extract_meeting_url(desc) {
                return Some(url);
            }
        }

        None
    }
}

/// Extract a meeting URL (Zoom, Meet, Teams) from text
fn extract_meeting_url(text: &str) -> Option<String> {
    // Common meeting URL patterns
    let patterns = [
        "https://zoom.us/",
        "https://us02web.zoom.us/",
        "https://us04web.zoom.us/",
        "https://us05web.zoom.us/",
        "https://us06web.zoom.us/",
        "https://meet.google.com/",
        "https://teams.microsoft.com/",
    ];

    for pattern in patterns {
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

use chrono::Timelike;

/// Response from events.list API
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsListResponse {
    pub items: Option<Vec<CalendarEvent>>,
    pub next_page_token: Option<String>,
}
