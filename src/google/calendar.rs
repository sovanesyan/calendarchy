use crate::error::{CalendarchyError, Result};
use crate::google::types::{CalendarEvent, EventsListResponse, TokenInfo};
use chrono::NaiveDate;
use reqwest::Client;

const CALENDAR_API_BASE: &str = "https://www.googleapis.com/calendar/v3";

pub struct CalendarClient {
    client: Client,
}

impl CalendarClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Fetch events for a date range
    pub async fn list_events(
        &self,
        token: &TokenInfo,
        calendar_id: &str,
        time_min: NaiveDate,
        time_max: NaiveDate,
    ) -> Result<Vec<CalendarEvent>> {
        let url = format!(
            "{}/calendars/{}/events",
            CALENDAR_API_BASE,
            urlencoding::encode(calendar_id)
        );

        // Convert dates to RFC3339 format
        let time_min_str = format!("{}T00:00:00Z", time_min);
        let time_max_str = format!("{}T23:59:59Z", time_max);

        let mut all_events = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut request = self
                .client
                .get(&url)
                .bearer_auth(&token.access_token)
                .query(&[
                    ("timeMin", time_min_str.as_str()),
                    ("timeMax", time_max_str.as_str()),
                    ("singleEvents", "true"),
                    ("orderBy", "startTime"),
                    ("maxResults", "250"),
                ]);

            if let Some(ref pt) = page_token {
                request = request.query(&[("pageToken", pt.as_str())]);
            }

            let response = request.send().await?;

            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(CalendarchyError::TokenExpired);
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(CalendarchyError::Api(format!(
                    "Calendar API error {}: {}",
                    status, body
                )));
            }

            let events_response: EventsListResponse = response.json().await?;

            if let Some(items) = events_response.items {
                all_events.extend(items);
            }

            page_token = events_response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        Ok(all_events)
    }
}

impl Default for CalendarClient {
    fn default() -> Self {
        Self::new()
    }
}
