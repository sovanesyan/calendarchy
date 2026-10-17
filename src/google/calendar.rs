use crate::error::{check_google, CalendarchyError, Result};
use crate::google::types::{CalendarEvent, EventsListResponse, TokenInfo};
use serde_json::Value;
use crate::logging::{log_request, log_response};
use crate::utils::local_day_bounds;
use chrono::{NaiveDate, SecondsFormat};
use reqwest::{Client, StatusCode};

const CALENDAR_API_BASE: &str = "https://www.googleapis.com/calendar/v3";

pub struct CalendarClient {
    client: Client,
}

impl CalendarClient {
    /// Use a shared client so connections (and TLS sessions) are reused
    pub fn new(client: Client) -> Self {
        Self { client }
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

        let (time_min, time_max) = local_day_bounds(time_min, time_max);
        let time_min_str = time_min.to_rfc3339_opts(SecondsFormat::Secs, true);
        let time_max_str = time_max.to_rfc3339_opts(SecondsFormat::Secs, true);

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

            let body = check_google(response, "Calendar API error").await?;
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

        // First, get the current event to find our attendee entry. It's kept
        // as raw JSON so every field of that entry (optional, comment, ...)
        // survives the round trip
        log_request("GET", &url);
        let get_response = self
            .client
            .get(&url)
            .bearer_auth(&token.access_token)
            .send()
            .await?;
        log_response(get_response.status().as_u16(), &url);

        let body = check_google(get_response, "Failed to get event").await?;
        let event: Value = serde_json::from_str(&body)?;
        let patch = response_patch(event, response)
            .ok_or_else(|| CalendarchyError::Api("You're not a guest of this event".to_string()))?;

        // PATCH only your own reply; the rest of the event is left alone
        log_request("PATCH", &url);
        let patch_response = self
            .client
            .patch(&url)
            .bearer_auth(&token.access_token)
            .query(&[("sendUpdates", "none")]) // Don't send notification emails
            .json(&patch)
            .send()
            .await?;
        log_response(patch_response.status().as_u16(), &url);

        check_google(patch_response, "Failed to update event").await.map(drop)
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

        // Already gone (deleted elsewhere) counts as deleted
        if already_gone(response.status()) {
            return Ok(());
        }
        check_google(response, "Failed to delete event").await.map(drop)
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

/// The PATCH body that sets only your own reply: your attendee entry (every
/// field as Google sent it) with `responseStatus` set, and `attendeesOmitted`
/// so the other guests (and their concurrent replies) are left alone. None
/// when you aren't on the list
fn response_patch(mut event: Value, response: &str) -> Option<Value> {
    let attendees = event.get_mut("attendees")?.as_array_mut()?;
    let mut me = attendees
        .iter_mut()
        .find(|a| a.get("self").and_then(Value::as_bool) == Some(true))?
        .take();
    me["responseStatus"] = response.into();
    Some(serde_json::json!({ "attendees": [me], "attendeesOmitted": true }))
}

/// A DELETE answered 404 or 410: the event was already deleted
fn already_gone(status: StatusCode) -> bool {
    matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_patches_only_your_own_entry() {
        let event = serde_json::json!({
            "id": "e1",
            "summary": "Review",
            "attendees": [
                { "email": "room@resource.calendar.google.com", "resource": true, "responseStatus": "accepted" },
                { "email": "me@example.com", "self": true, "optional": true, "comment": "maybe", "responseStatus": "needsAction" },
                { "email": "boss@example.com", "organizer": true, "additionalGuests": 2, "responseStatus": "accepted" }
            ]
        });
        assert_eq!(
            response_patch(event, "accepted").unwrap(),
            serde_json::json!({
                "attendees": [
                    { "email": "me@example.com", "self": true, "optional": true, "comment": "maybe", "responseStatus": "accepted" }
                ],
                "attendeesOmitted": true
            })
        );
    }

    #[test]
    fn test_response_needs_you_on_the_guest_list() {
        assert!(response_patch(serde_json::json!({ "id": "e1" }), "declined").is_none());
        let others = serde_json::json!({ "attendees": [{ "email": "a@b.c", "responseStatus": "accepted" }] });
        assert!(response_patch(others, "declined").is_none());
        // A truncated list (attendeesOmitted on read) still has your entry
        let truncated = serde_json::json!({ "attendeesOmitted": true, "attendees": [{ "email": "me@x.y", "self": true }] });
        assert_eq!(response_patch(truncated, "declined").unwrap()["attendees"][0]["responseStatus"], "declined");
    }

    #[test]
    fn test_deleting_an_event_already_gone_succeeds() {
        assert!(already_gone(StatusCode::NOT_FOUND));
        assert!(already_gone(StatusCode::GONE));
        assert!(!already_gone(StatusCode::FORBIDDEN));
        assert!(!already_gone(StatusCode::UNAUTHORIZED), "a 401 still refreshes the token");
    }
}
