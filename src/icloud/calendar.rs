use crate::auth::{CalendarEntry, ICloudAccount};
use crate::config::ICloudConfig;
use crate::error::{check, CalendarchyError, Result};
use crate::icloud::types::{ICalEvent, Me};
use crate::logging::{log_request, log_response};
use crate::utils::local_day_bounds;
use chrono::NaiveDate;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::{Client, Method, StatusCode, Url};

const CALDAV_SERVER: &str = "https://caldav.icloud.com";

const LIST_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>"#;

/// CalDAV client for iCloud Calendar, signed in with an app-specific password
pub struct CalDavClient {
    client: Client,
    apple_id: String,
    app_password: String,
    /// Who you are on invites, for `accepted`: the Apple ID until
    /// `with_addresses` adds the discovered ones
    me: Me,
}

impl CalDavClient {
    /// Use a shared client so connections (and TLS sessions) are reused
    pub fn new(client: Client, config: &ICloudConfig) -> Self {
        let apple_id = config.apple_id.clone().unwrap_or_default();
        Self {
            client,
            me: Me::new([format!("mailto:{}", apple_id)]),
            apple_id,
            app_password: config.app_password.clone().unwrap_or_default(),
        }
    }

    /// Also recognise you by these calendar-user addresses (from discovery)
    pub fn with_addresses(mut self, addresses: &[String]) -> Self {
        let apple_id = format!("mailto:{}", self.apple_id);
        self.me = Me::new(addresses.iter().map(String::as_str).chain([apple_id.as_str()]));
        self
    }

    /// Find the user's calendars and addresses: principal, then its calendar
    /// home and calendar-user addresses, then the calendars in the home
    pub async fn discover(&self) -> Result<ICloudAccount> {
        let xml = self
            .propfind(CALDAV_SERVER, "<d:current-user-principal/>", "current-user-principal")
            .await?;
        let principal = extract_href(&xml, "current-user-principal")
            .ok_or_else(|| CalendarchyError::CalDav("No current-user-principal in the response".to_string()))?;
        let principal_url = resolve_url(CALDAV_SERVER, &principal);

        let xml = self
            .propfind(
                &principal_url,
                "<c:calendar-home-set/>\n    <c:calendar-user-address-set/>",
                "calendar-home-set",
            )
            .await?;
        let home = extract_href(&xml, "calendar-home-set")
            .ok_or_else(|| CalendarchyError::CalDav("No calendar-home-set in the response".to_string()))?;
        // The principal is you too: iCloud writes iCloud users on invites as
        // their principal path
        let mut addresses = extract_hrefs(&xml, "calendar-user-address-set");
        if !addresses.iter().any(|a| a.trim_end_matches('/') == principal.trim_end_matches('/')) {
            addresses.push(principal);
        }

        let home_url = resolve_url(&principal_url, &home);
        let xml = self
            .dav("PROPFIND", &home_url, "1", LIST_CALENDARS, "Calendar list failed")
            .await?;
        Ok(ICloudAccount { calendars: parse_calendar_list(&xml, &home_url), addresses })
    }

    /// Fetch events for a date range
    pub async fn fetch_events(
        &self,
        calendar_url: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<ICalEvent>> {
        let (range_start, range_end) = local_day_bounds(start, end);
        let start_str = range_start.format("%Y%m%dT%H%M%SZ").to_string();
        let end_str = range_end.format("%Y%m%dT%H%M%SZ").to_string();

        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data>
      <c:expand start="{start}" end="{end}"/>
    </c:calendar-data>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{start}" end="{end}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#,
            start = start_str,
            end = end_str
        );

        let xml = self.dav("REPORT", calendar_url, "1", body, "REPORT failed").await?;
        parse_calendar_query(&xml, calendar_url, &self.me)
    }

    /// Delete an event by its UID
    pub async fn delete_event(
        &self,
        calendar_url: &str,
        event_uid: &str,
        href: Option<&str>,
        etag: Option<&str>,
    ) -> Result<()> {
        // Prefer the resource URL the server gave us; resource names aren't
        // guaranteed to be "<uid>.ics", so that's only a fallback
        let event_url = match href {
            Some(href) => href.to_string(),
            None => format!("{}/{}.ics", calendar_url.trim_end_matches('/'), event_uid),
        };

        log_request("DELETE", &event_url);
        let mut request = self
            .client
            .delete(&event_url)
            .basic_auth(&self.apple_id, Some(&self.app_password));

        // Use etag for conditional delete if available
        if let Some(tag) = etag {
            request = request.header("If-Match", format!("\"{}\"", tag));
        }

        let response = request.send().await?;
        log_response(response.status().as_u16(), &event_url);

        // Already gone counts as deleted
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(response, "Failed to delete event", CalendarchyError::CalDav).await.map(drop)
    }

    /// One WebDAV request with an XML body; the response body on success
    async fn dav(
        &self,
        method: &'static str,
        url: &str,
        depth: &'static str,
        body: impl Into<reqwest::Body>,
        context: &str,
    ) -> Result<String> {
        let method = Method::from_bytes(method.as_bytes()).expect("a valid HTTP method");
        log_request(method.as_str(), url);
        let response = self
            .client
            .request(method, url)
            .basic_auth(&self.apple_id, Some(&self.app_password))
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", depth)
            .body(body)
            .send()
            .await?;
        log_response(response.status().as_u16(), url);
        check(response, context, CalendarchyError::CalDav).await
    }

    /// PROPFIND properties of `url` (`props`, e.g. `<c:calendar-home-set/>`);
    /// `what` names them in errors
    async fn propfind(&self, url: &str, props: &str, what: &str) -> Result<String> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    {props}
  </d:prop>
</d:propfind>"#
        );
        self.dav("PROPFIND", url, "0", body, &format!("PROPFIND {} failed", what)).await
    }
}

/// Resolve an href against the URL of the request it came back from:
/// relative hrefs stay on that host (e.g. a pXX-caldav.icloud.com partition).
/// Absolute ones are kept verbatim, so saved calendar URLs don't change form
fn resolve_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    Url::parse(base)
        .or_else(|_| Url::parse(CALDAV_SERVER))
        .and_then(|base| base.join(href))
        .map(String::from)
        .unwrap_or_else(|_| href.to_string())
}

/// The first href inside a `parent_tag` element (namespaces ignored)
fn extract_href(xml: &str, parent_tag: &str) -> Option<String> {
    extract_hrefs(xml, parent_tag).into_iter().next()
}

/// Every href inside `parent_tag` elements, in order (namespaces ignored)
fn extract_hrefs(xml: &str, parent_tag: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut in_parent = false;
    let mut in_href = false;
    let mut hrefs = Vec::new();

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
                    hrefs.push(e.unescape().unwrap_or_default().trim().to_string());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    hrefs
}

/// The calendars in a calendar-home PROPFIND (Depth 1) multistatus: the
/// responses whose resourcetype includes `calendar`. `home_url` is the URL
/// that was PROPFINDed, which relative hrefs resolve against
fn parse_calendar_list(xml: &str, home_url: &str) -> Vec<CalendarEntry> {
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
                            calendars.push(CalendarEntry {
                                url: resolve_url(home_url, &href),
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

/// The events in a calendar-query REPORT multistatus: each response's
/// calendar-data, with its href (resolved against `calendar_url`, the URL
/// that was queried) and etag. `me` picks out your own attendee entry.
fn parse_calendar_query(xml: &str, calendar_url: &str, me: &Me) -> Result<Vec<ICalEvent>> {
    let mut events = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut in_calendar_data = false;
    let mut calendar_data = String::new();
    let mut current_etag: Option<String> = None;
    let mut current_href: Option<String> = None;
    let mut current_tag = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                current_tag = name.clone();
                if name == "calendar-data" {
                    in_calendar_data = true;
                    calendar_data.clear();
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "calendar-data" && in_calendar_data {
                    let parsed = ICalEvent::parse_ical_with_source(
                        &calendar_data,
                        calendar_url.to_string(),
                        current_etag.clone(),
                        current_href.clone(),
                        me,
                    );
                    events.extend(parsed);
                    in_calendar_data = false;
                } else if name == "response" {
                    current_etag = None;
                    current_href = None;
                }
                current_tag.clear();
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_calendar_data {
                    calendar_data.push_str(&text);
                } else if current_tag == "getetag" {
                    current_etag = Some(text.trim_matches('"').to_string());
                } else if current_tag == "href" && current_href.is_none() {
                    current_href = Some(resolve_url(calendar_url, &text));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_principal_href_is_the_one_inside_the_property() {
        // The response's own href ("/") comes first and must be skipped
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/</href>
    <propstat>
      <prop>
        <current-user-principal>
          <href>/123456789/principal/</href>
        </current-user-principal>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;
        assert_eq!(extract_href(xml, "current-user-principal").as_deref(), Some("/123456789/principal/"));
        assert_eq!(extract_href(xml, "calendar-home-set"), None);
    }

    #[test]
    fn test_calendar_home_href_with_prefixes_and_absolute_url() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/123456789/principal/</d:href>
    <d:propstat>
      <d:prop>
        <cal:calendar-home-set>
          <d:href>https://p42-caldav.icloud.com:443/123456789/calendars/</d:href>
        </cal:calendar-home-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
        let home = extract_href(xml, "calendar-home-set").unwrap();
        assert_eq!(home, "https://p42-caldav.icloud.com:443/123456789/calendars/");
        assert_eq!(resolve_url("https://caldav.icloud.com/1/principal/", &home), home, "absolute URLs are kept verbatim");
        assert_eq!(resolve_url(CALDAV_SERVER, "/1/principal/"), "https://caldav.icloud.com/1/principal/");
    }

    #[test]
    fn test_relative_hrefs_resolve_against_the_requested_url() {
        // A partition host must be kept, not swapped for caldav.icloud.com
        let home = "https://p42-caldav.icloud.com:443/123456789/calendars/";
        assert_eq!(resolve_url(home, "/123456789/calendars/home/"), "https://p42-caldav.icloud.com/123456789/calendars/home/");
        assert_eq!(resolve_url(home, "work/"), "https://p42-caldav.icloud.com/123456789/calendars/work/");
        assert_eq!(
            resolve_url("https://p42-caldav.icloud.com/1/calendars/home/", " /1/calendars/home/A%20B.ics "),
            "https://p42-caldav.icloud.com/1/calendars/home/A%20B.ics"
        );
        // Not a URL (tests, broken config): fall back to the main server
        assert_eq!(resolve_url("cal", "/1/x.ics"), "https://caldav.icloud.com/1/x.ics");
    }

    #[test]
    fn test_principal_props_give_home_and_every_address() {
        // Shape of iCloud's answer to the principal PROPFIND for both props
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/123456789/principal/</href>
    <propstat>
      <prop>
        <calendar-home-set xmlns="urn:ietf:params:xml:ns:caldav"><href xmlns="DAV:">https://p42-caldav.icloud.com:443/123456789/calendars/</href></calendar-home-set>
        <calendar-user-address-set xmlns="urn:ietf:params:xml:ns:caldav">
          <href xmlns="DAV:">mailto:me@icloud.com</href>
          <href xmlns="DAV:">mailto:me@gmail.com</href>
          <href xmlns="DAV:">/aMTM4NTk=/principal/</href>
          <href xmlns="DAV:">urn:uuid:1A2B3C4D-0000-1111-2222-333344445555</href>
        </calendar-user-address-set>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;
        assert_eq!(extract_href(xml, "calendar-home-set").as_deref(), Some("https://p42-caldav.icloud.com:443/123456789/calendars/"));
        assert_eq!(
            extract_hrefs(xml, "calendar-user-address-set"),
            ["mailto:me@icloud.com", "mailto:me@gmail.com", "/aMTM4NTk=/principal/", "urn:uuid:1A2B3C4D-0000-1111-2222-333344445555"]
        );
        assert!(extract_hrefs(xml, "current-user-principal").is_empty());
    }

    #[test]
    fn test_calendar_list_keeps_only_calendars() {
        // The home collection itself, two calendars (one unnamed), the
        // scheduling inbox and the notifications collection
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/123456789/calendars/</href>
    <propstat>
      <prop>
        <displayname>Calendars</displayname>
        <resourcetype><collection/></resourcetype>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/123456789/calendars/home/</href>
    <propstat>
      <prop>
        <displayname>Home &amp; Family</displayname>
        <resourcetype><collection/><calendar xmlns="urn:ietf:params:xml:ns:caldav"/></resourcetype>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>https://p42-caldav.icloud.com/123456789/calendars/work/</href>
    <propstat>
      <prop>
        <resourcetype><collection/><C:calendar xmlns:C="urn:ietf:params:xml:ns:caldav"></C:calendar></resourcetype>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
    <propstat>
      <prop><displayname/></prop>
      <status>HTTP/1.1 404 Not Found</status>
    </propstat>
  </response>
  <response>
    <href>/123456789/calendars/inbox/</href>
    <propstat>
      <prop>
        <displayname>Inbox</displayname>
        <resourcetype><collection/><schedule-inbox xmlns="urn:ietf:params:xml:ns:caldav"/></resourcetype>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/123456789/calendars/notification/</href>
    <propstat>
      <prop>
        <resourcetype><collection/><notification xmlns="http://calendarserver.org/ns/"/></resourcetype>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;
        assert_eq!(
            parse_calendar_list(xml, "https://p42-caldav.icloud.com:443/123456789/calendars/"),
            [
                CalendarEntry { url: "https://p42-caldav.icloud.com/123456789/calendars/home/".into(), name: Some("Home & Family".into()) },
                CalendarEntry { url: "https://p42-caldav.icloud.com/123456789/calendars/work/".into(), name: None },
            ]
        );
    }

    /// A calendar-query REPORT response like iCloud's: CRs escaped as
    /// `&#13;`, etags quoted, one response's data in CDATA
    const REPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/123456789/calendars/home/A1B2C3.ics</href>
    <propstat>
      <prop>
        <getetag>"mfk2l0yw"</getetag>
        <calendar-data xmlns="urn:ietf:params:xml:ns:caldav">BEGIN:VCALENDAR&#13;
VERSION:2.0&#13;
BEGIN:VEVENT&#13;
UID:A1B2C3&#13;
SUMMARY:Dentist &amp; check-up&#13;
DTSTART:20261020T090000Z&#13;
DTEND:20261020T100000Z&#13;
END:VEVENT&#13;
END:VCALENDAR&#13;
</calendar-data>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/123456789/calendars/home/party.ics</href>
    <propstat>
      <prop>
        <getetag>&quot;nq7r1x&quot;</getetag>
        <calendar-data xmlns="urn:ietf:params:xml:ns:caldav"><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
UID:party-1
SUMMARY:Party
DTSTART;VALUE=DATE:20261024
DTEND;VALUE=DATE:20261025
ORGANIZER;CN=Friend:mailto:friend@example.com
ATTENDEE;PARTSTAT=DECLINED;CN=Me:mailto:Me@Example.com
END:VEVENT
END:VCALENDAR
]]></calendar-data>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

    #[test]
    fn test_calendar_query_events_carry_their_href_and_etag() {
        let cal = "https://p42-caldav.icloud.com/123456789/calendars/home/";
        let events = parse_calendar_query(REPORT, cal, &Me::new(["mailto:me@example.com"])).unwrap();
        assert_eq!(events.len(), 2);

        let dentist = &events[0];
        assert_eq!((dentist.uid.as_str(), dentist.title()), ("A1B2C3", "Dentist & check-up"));
        assert_eq!(dentist.etag.as_deref(), Some("mfk2l0yw"));
        assert_eq!(dentist.href.as_deref(), Some("https://p42-caldav.icloud.com/123456789/calendars/home/A1B2C3.ics"));
        assert_eq!(dentist.calendar_url, cal);
        assert!(dentist.accepted, "no attendees: your own event");

        let party = &events[1];
        assert_eq!(party.uid, "party-1");
        assert_eq!(party.etag.as_deref(), Some("nq7r1x"));
        assert_eq!(party.href.as_deref(), Some("https://p42-caldav.icloud.com/123456789/calendars/home/party.ics"));
        assert!(!party.accepted, "your own reply (matched by Apple ID) is DECLINED");
    }

    #[test]
    fn test_calendar_query_without_apple_id_counts_as_accepted() {
        let events = parse_calendar_query(REPORT, "cal", &Me::default()).unwrap();
        assert!(events.iter().all(|e| e.accepted));
    }

    #[test]
    fn test_calendar_query_empty_and_broken_responses() {
        let empty = r#"<?xml version="1.0" encoding="UTF-8"?><multistatus xmlns="DAV:"/>"#;
        assert!(parse_calendar_query(empty, "cal", &Me::default()).unwrap().is_empty());
        assert!(parse_calendar_query("<multistatus><response></multistatus>", "cal", &Me::default()).is_err());
    }

    #[test]
    fn test_client_knows_you_by_apple_id_and_discovered_addresses() {
        // An invite from another iCloud user: you're on it as your principal
        // path, with your Apple ID nowhere in it
        let report = REPORT.replace(
            "ATTENDEE;PARTSTAT=DECLINED;CN=Me:mailto:Me@Example.com",
            "ATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION;EMAIL=me@icloud.com:/aMTM4NTk=/principal/",
        );
        let config = ICloudConfig { apple_id: Some("me@gmail.com".into()), app_password: Some("p".into()) };
        let apple_id_only = CalDavClient::new(Client::new(), &config);
        let events = parse_calendar_query(&report, "cal", &apple_id_only.me).unwrap();
        assert!(events[1].accepted, "unknown to the Apple ID alone");

        let addresses = ["mailto:me@icloud.com".to_string(), "/aMTM4NTk=/principal/".to_string()];
        let client = CalDavClient::new(Client::new(), &config).with_addresses(&addresses);
        let events = parse_calendar_query(&report, "cal", &client.me).unwrap();
        assert!(!events[1].accepted, "a pending invite isn't accepted");
    }
}
