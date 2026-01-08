use chrono::{DateTime, NaiveDate, Utc};
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
    /// Get the start date (works for both all-day and timed events)
    pub fn start_date(&self) -> Option<NaiveDate> {
        self.start
            .date
            .or_else(|| self.start.date_time.map(|dt| dt.date_naive()))
    }

    /// Get display title
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(No title)")
    }

    /// Get start time as HH:MM or "All day"
    pub fn time_str(&self) -> String {
        self.start
            .date_time
            .map(|dt| format!("{:02}:{:02}", dt.time().hour(), dt.time().minute()))
            .unwrap_or_else(|| "All day".to_string())
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
