use crate::error::{check_google_response, check_google_response_no_body, CalendarchyError, Result};
use crate::google::types::{CalendarEvent, EventsListResponse, TokenInfo};
use crate::{log_request, log_response};
use chrono::NaiveDate;
use reqwest::{Client, StatusCode};

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

            log_request("GET", &url);
            let response = request.send().await?;
            log_response(response.status().as_u16(), &url);

            let body = check_google_response(response, "Calendar API error").await?;
            let events_response: EventsListResponse = serde_json::from_str(&body)?;

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

    /// Update the current user's response status for an event
    pub async fn respond_to_event(
        &self,
        token: &TokenInfo,
        calendar_id: &str,
        event_id: &str,
        response: &str, // "accepted", "declined", "tentative"
    ) -> Result<()> {
        let url = format!(
            "{}/calendars/{}/events/{}",
            CALENDAR_API_BASE,
            urlencoding::encode(calendar_id),
            urlencoding::encode(event_id)
        );

        // First, get the current event to find our attendee entry
        log_request("GET", &url);
        let get_response = self
            .client
            .get(&url)
            .bearer_auth(&token.access_token)
            .send()
            .await?;
        log_response(get_response.status().as_u16(), &url);

        let body = check_google_response(get_response, "Failed to get event").await?;
        let mut event: CalendarEvent = serde_json::from_str(&body)?;

        // Update the self attendee's response status
        if let Some(ref mut attendees) = event.attendees {
            for attendee in attendees.iter_mut() {
                if attendee.is_self == Some(true) {
                    attendee.response_status = Some(response.to_string());
                    break;
                }
            }
        }

        // PATCH the event back
        log_request("PATCH", &url);
        let patch_response = self
            .client
            .patch(&url)
            .bearer_auth(&token.access_token)
            .query(&[("sendUpdates", "none")]) // Don't send notification emails
            .json(&event)
            .send()
            .await?;
        log_response(patch_response.status().as_u16(), &url);

        check_google_response_no_body(patch_response, "Failed to update event").await
    }

    /// Delete an event
    pub async fn delete_event(
        &self,
        token: &TokenInfo,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/calendars/{}/events/{}",
            CALENDAR_API_BASE,
            urlencoding::encode(calendar_id),
            urlencoding::encode(event_id)
        );

        log_request("DELETE", &url);
        let response = self
            .client
            .delete(&url)
            .bearer_auth(&token.access_token)
            .query(&[("sendUpdates", "none")]) // Don't send notification emails
            .send()
            .await?;
        log_response(response.status().as_u16(), &url);

        check_google_response_no_body(response, "Failed to delete event").await
    }

    /// Get calendar display name
    pub async fn get_calendar_name(
        &self,
        token: &TokenInfo,
        calendar_id: &str,
    ) -> Result<Option<String>> {
        let url = format!(
            "{}/calendars/{}",
            CALENDAR_API_BASE,
            urlencoding::encode(calendar_id)
        );

        log_request("GET", &url);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&token.access_token)
            .send()
            .await?;
        log_response(response.status().as_u16(), &url);

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(CalendarchyError::TokenExpired);
        }

        if !response.status().is_success() {
            return Ok(None);
        }

        #[derive(serde::Deserialize)]
        struct CalendarMeta {
            summary: Option<String>,
        }

        let meta: CalendarMeta = response.json().await?;
        Ok(meta.summary)
    }
}

impl Default for CalendarClient {
    fn default() -> Self {
        Self::new()
    }
}
