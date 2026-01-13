use crate::error::{check_caldav_response, check_caldav_response_no_body, CalendarchyError, Result};
use crate::icloud::auth::ICloudAuth;
use crate::icloud::types::ICalEvent;
use crate::{log_request, log_response};
use chrono::NaiveDate;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;

const CALDAV_SERVER: &str = "https://caldav.icloud.com";

/// CalDAV client for iCloud Calendar
pub struct CalDavClient {
    client: Client,
    auth: ICloudAuth,
}

impl CalDavClient {
    pub fn new(auth: ICloudAuth) -> Self {
        Self {
            client: Client::new(),
            auth,
        }
    }

    /// Discover the user's principal URL and calendar home
    pub async fn discover_calendars(&self) -> Result<Vec<CalendarInfo>> {
        // Step 1: Get principal URL
        let principal_url = self.discover_principal().await?;

        // Step 2: Get calendar home set
        let calendar_home = self.get_calendar_home(&principal_url).await?;

        // Step 3: List calendars
        let calendars = self.list_calendars(&calendar_home).await?;

        Ok(calendars)
    }

    /// Fetch events for a date range
    pub async fn fetch_events(
        &self,
        calendar_url: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<ICalEvent>> {
        let start_str = format!("{}T000000Z", start.format("%Y%m%d"));
        let end_str = format!("{}T235959Z", end.format("%Y%m%d"));

        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{}" end="{}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
            start_str, end_str
        );

        log_request("REPORT", calendar_url);
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), calendar_url)
            .header("Authorization", self.auth.auth_header())
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .body(body)
            .send()
            .await?;
        log_response(response.status().as_u16(), calendar_url);

        let xml = check_caldav_response(response, "REPORT failed").await?;
        let events = self.parse_calendar_multiget(&xml, calendar_url)?;

        Ok(events)
    }

    /// Discover principal URL
    async fn discover_principal(&self) -> Result<String> {
        let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:current-user-principal/>
  </d:prop>
</d:propfind>"#;

        log_request("PROPFIND", CALDAV_SERVER);
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), CALDAV_SERVER)
            .header("Authorization", self.auth.auth_header())
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "0")
            .body(body)
            .send()
            .await?;
        log_response(response.status().as_u16(), CALDAV_SERVER);

        let xml = check_caldav_response(response, "Principal discovery failed").await?;
        self.extract_href(&xml, "current-user-principal")
            .ok_or_else(|| CalendarchyError::CalDav("Could not find principal URL".to_string()))
    }

    /// Get calendar home set from principal
    async fn get_calendar_home(&self, principal_url: &str) -> Result<String> {
        let url = self.resolve_url(principal_url);

        let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <c:calendar-home-set/>
  </d:prop>
</d:propfind>"#;

        log_request("PROPFIND", &url);
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Authorization", self.auth.auth_header())
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "0")
            .body(body)
            .send()
            .await?;
        log_response(response.status().as_u16(), &url);

        let xml = check_caldav_response(response, "Calendar home discovery failed").await?;
        self.extract_href(&xml, "calendar-home-set")
            .ok_or_else(|| CalendarchyError::CalDav("Could not find calendar home".to_string()))
    }

    /// List calendars in calendar home
    async fn list_calendars(&self, calendar_home: &str) -> Result<Vec<CalendarInfo>> {
        let url = self.resolve_url(calendar_home);

        let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <cs:getctag/>
  </d:prop>
</d:propfind>"#;

        log_request("PROPFIND", &url);
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Authorization", self.auth.auth_header())
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", "1")
            .body(body)
            .send()
            .await?;
        log_response(response.status().as_u16(), &url);

        let xml = check_caldav_response(response, "Calendar list failed").await?;
        Ok(self.parse_calendar_list(&xml))
    }

    /// Parse calendar list response
    fn parse_calendar_list(&self, xml: &str) -> Vec<CalendarInfo> {
        let mut calendars = Vec::new();
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut current_href: Option<String> = None;
        let mut current_name: Option<String> = None;
        let mut is_calendar = false;
        let mut in_response = false;
        let mut current_tag = String::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    current_tag = name.clone();

                    if name == "response" {
                        in_response = true;
                        current_href = None;
                        current_name = None;
                        is_calendar = false;
                    } else if name == "calendar" && in_response {
                        is_calendar = true;
                    }
                }
                Ok(Event::End(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if name == "response" && in_response {
                        if is_calendar
                            && let Some(href) = current_href.take() {
                                calendars.push(CalendarInfo {
                                    url: self.resolve_url(&href),
                                    name: current_name.take(),
                                });
                            }
                        in_response = false;
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_response {
                        let text = e.unescape().unwrap_or_default().to_string();
                        if current_tag == "href" && current_href.is_none() {
                            current_href = Some(text);
                        } else if current_tag == "displayname" {
                            current_name = Some(text);
                        }
                    }
                }
                Ok(Event::Empty(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if name == "calendar" && in_response {
                        is_calendar = true;
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        calendars
    }

    /// Parse calendar-multiget response to extract events
    fn parse_calendar_multiget(&self, xml: &str, calendar_url: &str) -> Result<Vec<ICalEvent>> {
        let mut events = Vec::new();
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_calendar_data = false;
        let mut in_etag = false;
        let mut calendar_data = String::new();
        let mut current_etag: Option<String> = None;
        let mut current_tag = String::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    current_tag = name.clone();
                    if name == "calendar-data" {
                        in_calendar_data = true;
                        calendar_data.clear();
                    } else if name == "getetag" {
                        in_etag = true;
                    }
                }
                Ok(Event::End(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if name == "calendar-data" && in_calendar_data {
                        let parsed = ICalEvent::parse_ical_with_source(
                            &calendar_data,
                            calendar_url.to_string(),
                            current_etag.clone(),
                        );
                        events.extend(parsed);
                        in_calendar_data = false;
                    } else if name == "getetag" {
                        in_etag = false;
                    } else if name == "response" {
                        current_etag = None;
                    }
                }
                Ok(Event::Text(e)) => {
                    let text = e.unescape().unwrap_or_default().to_string();
                    if in_calendar_data {
                        calendar_data.push_str(&text);
                    } else if in_etag || current_tag == "getetag" {
                        current_etag = Some(text.trim_matches('"').to_string());
                    }
                }
                Ok(Event::CData(e)) => {
                    if in_calendar_data {
                        calendar_data.push_str(&String::from_utf8_lossy(&e));
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(CalendarchyError::CalDav(format!("XML parse error: {}", e))),
                _ => {}
            }
            buf.clear();
        }

        Ok(events)
    }

    /// Extract href from XML response
    fn extract_href(&self, xml: &str, parent_tag: &str) -> Option<String> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let mut in_parent = false;
        let mut in_href = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if name == parent_tag {
                        in_parent = true;
                    } else if name == "href" && in_parent {
                        in_href = true;
                    }
                }
                Ok(Event::End(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                    if name == parent_tag {
                        in_parent = false;
                    } else if name == "href" {
                        in_href = false;
                    }
                }
                Ok(Event::Text(e)) => {
                    if in_href {
                        return Some(e.unescape().unwrap_or_default().to_string());
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        None
    }

    /// Resolve relative URL to absolute
    fn resolve_url(&self, path: &str) -> String {
        if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", CALDAV_SERVER, path)
        }
    }

    /// Delete an event by its UID
    pub async fn delete_event(
        &self,
        calendar_url: &str,
        event_uid: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        // Construct event URL: calendar_url + uid + ".ics"
        let event_url = format!(
            "{}{}.ics",
            calendar_url.trim_end_matches('/').to_string() + "/",
            event_uid
        );

        log_request("DELETE", &event_url);
        let mut request = self
            .client
            .delete(&event_url)
            .header("Authorization", self.auth.auth_header());

        // Use etag for conditional delete if available
        if let Some(tag) = etag {
            request = request.header("If-Match", format!("\"{}\"", tag));
        }

        let response = request.send().await?;
        log_response(response.status().as_u16(), &event_url);

        check_caldav_response_no_body(response, "Failed to delete event").await
    }
}

/// Information about a calendar
#[derive(Debug, Clone)]
pub struct CalendarInfo {
    pub url: String,
    pub name: Option<String>,
}
