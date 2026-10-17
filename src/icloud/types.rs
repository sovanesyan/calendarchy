use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
#[cfg(test)]
use chrono::Timelike;

/// Attendee from iCal ATTENDEE line
#[derive(Debug, Clone)]
pub struct ICalAttendee {
    pub name: Option<String>,
    /// The address to show: the mailto: address, else the EMAIL= parameter,
    /// else the raw value (e.g. an iCloud principal path)
    pub email: String,
    /// The calendar-user address as written (`mailto:...`, a principal path,
    /// `urn:uuid:...`), for recognising yourself
    pub address: String,
    pub partstat: String,  // ACCEPTED, DECLINED, TENTATIVE, NEEDS-ACTION
    pub is_organizer: bool,
}

/// An event from iCloud Calendar (parsed from iCal/VCALENDAR format)
#[derive(Debug, Clone)]
pub struct ICalEvent {
    pub uid: String,
    pub summary: Option<String>,
    pub dtstart: EventTime,
    pub dtend: Option<EventTime>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    /// You accepted, or it's yours (you organise it or aren't listed)
    pub accepted: bool,
    pub attendees: Vec<ICalAttendee>,
    /// "TRANSPARENT" = free, "OPAQUE" = busy (default)
    pub transp: Option<String>,
    /// The calendar URL this event belongs to (set by CalDavClient)
    pub calendar_url: String,
    /// The etag for conditional updates
    pub etag: Option<String>,
    /// The CalDAV resource URL (from the REPORT response href), used for deletes
    pub href: Option<String>,
}

/// Event time - can be all-day (date only) or specific time
#[derive(Debug, Clone)]
pub enum EventTime {
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
}

impl ICalEvent {
    #[cfg(test)]
    /// Get the start date (works for both all-day and timed events)
    pub fn start_date(&self) -> NaiveDate {
        match &self.dtstart {
            EventTime::Date(d) => *d,
            EventTime::DateTime(dt) => dt.with_timezone(&Local).date_naive(),
        }
    }

    /// Get display title
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(No title)")
    }

    #[cfg(test)]
    /// Get start time as HH:MM or "All day"
    pub fn time_str(&self) -> String {
        match &self.dtstart {
            EventTime::Date(_) => "All day".to_string(),
            EventTime::DateTime(dt) => format_local_hm(dt),
        }
    }

    #[cfg(test)]
    /// Get end time as HH:MM or None for all-day events
    pub fn end_time_str(&self) -> Option<String> {
        match &self.dtend {
            Some(EventTime::DateTime(dt)) => Some(format_local_hm(dt)),
            _ => None,
        }
    }

    /// Meeting link: the URL property if it's a meeting link, then one found
    /// in the location or description
    pub fn meeting_url(&self) -> Option<String> {
        meeting_url(
            self.url.as_deref().filter(|url| is_meeting_url(url)),
            self.location.as_deref(),
            self.description.as_deref(),
        )
    }

    /// Check if the event is marked as "free" (doesn't block time)
    pub fn is_free(&self) -> bool {
        self.transp.as_deref() == Some("TRANSPARENT")
    }

    /// Parse an iCal VCALENDAR string into events (test-only), as seen by
    /// me@example.com
    #[cfg(test)]
    pub fn parse_ical(ical_data: &str) -> Vec<ICalEvent> {
        Self::parse_ical_with_source(ical_data, String::new(), None, None, &Me::new(["me@example.com"]))
    }

    /// Parse an iCal VCALENDAR string into events with calendar source info.
    /// `me` picks out your own attendee entry, for `accepted`.
    pub fn parse_ical_with_source(
        ical_data: &str,
        calendar_url: String,
        etag: Option<String>,
        href: Option<String>,
        me: &Me,
    ) -> Vec<ICalEvent> {
        let mut events = Vec::new();
        let mut current_event: Option<ICalEventBuilder> = None;
        // Depth of components nested inside the VEVENT (VALARM etc.), whose
        // properties (UID, DESCRIPTION, ...) must not leak into the event
        let mut nested = 0usize;

        for line in unfold_ical_lines(ical_data) {
            let line = line.trim();

            if line == "BEGIN:VEVENT" {
                nested = 0;
                current_event = Some(ICalEventBuilder {
                    calendar_url: calendar_url.clone(),
                    etag: etag.clone(),
                    href: href.clone(),
                    ..Default::default()
                });
            } else if line == "END:VEVENT" {
                if let Some(builder) = current_event.take()
                    && let Some(event) = builder.build(me) {
                        events.push(event);
                    }
            } else if current_event.is_some() && line.starts_with("BEGIN:") {
                nested += 1;
            } else if current_event.is_some() && line.starts_with("END:") {
                nested = nested.saturating_sub(1);
            } else if nested == 0
                && let Some(ref mut builder) = current_event
                && let Some((key, value)) = parse_ical_line(line) {
                    let base_key = key.split(';').next().unwrap_or(key);
                    match base_key {
                        "UID" => builder.uid = Some(value.to_string()),
                        "SUMMARY" => builder.summary = Some(unescape_ical(value)),
                        "DTSTART" => builder.dtstart = parse_ical_datetime(key, value),
                        "DTEND" => builder.dtend = parse_ical_datetime(key, value),
                        "LOCATION" => builder.location = Some(unescape_ical(value)),
                        "DESCRIPTION" => builder.description = Some(unescape_ical(value)),
                        "URL" => builder.url = Some(unescape_ical(value)),
                        "ATTENDEE" => {
                            if let Some(attendee) = parse_attendee(key, value) {
                                builder.attendees.push(attendee);
                            }
                        }
                        "ORGANIZER" => {
                            // Parse organizer as an attendee
                            if let Some(mut attendee) = parse_attendee(key, value) {
                                attendee.is_organizer = true;
                                builder.attendees.push(attendee);
                            }
                        }
                        "TRANSP" => builder.transp = Some(value.to_string()),
                        _ => {}
                    }
                }
        }

        events
    }
}

#[derive(Default)]
struct ICalEventBuilder {
    uid: Option<String>,
    summary: Option<String>,
    dtstart: Option<EventTime>,
    dtend: Option<EventTime>,
    location: Option<String>,
    description: Option<String>,
    url: Option<String>,
    attendees: Vec<ICalAttendee>,
    transp: Option<String>,
    calendar_url: String,
    etag: Option<String>,
    href: Option<String>,
}

impl ICalEventBuilder {
    fn build(self, me: &Me) -> Option<ICalEvent> {
        let accepted = self_accepted(&self.attendees, me);
        Some(ICalEvent {
            uid: self.uid?,
            summary: self.summary,
            dtstart: self.dtstart?,
            dtend: self.dtend,
            location: self.location,
            description: self.description,
            url: self.url,
            accepted,
            attendees: self.attendees,
            transp: self.transp,
            calendar_url: self.calendar_url,
            etag: self.etag,
            href: self.href,
        })
    }
}

/// Who you are on invites: every calendar-user address of the account (the
/// Apple ID, the `calendar-user-address-set` from discovery: mailto:
/// addresses, principal paths, `urn:uuid:`s), normalised for comparison
#[derive(Debug, Clone, Default)]
pub struct Me {
    addresses: Vec<String>,
}

impl Me {
    pub fn new<S: AsRef<str>>(addresses: impl IntoIterator<Item = S>) -> Self {
        let mut addresses: Vec<String> = addresses
            .into_iter()
            .map(|a| normalise_address(a.as_ref()))
            .filter(|a| !a.is_empty())
            .collect();
        addresses.sort();
        addresses.dedup();
        Self { addresses }
    }

    /// Whether this attendee is you: its value, or its EMAIL= address
    fn is(&self, attendee: &ICalAttendee) -> bool {
        [&attendee.address, &attendee.email]
            .into_iter()
            .map(|a| normalise_address(a))
            .any(|a| !a.is_empty() && self.addresses.contains(&a))
    }
}

/// A calendar-user address in comparable form: emails (`mailto:` stripped)
/// and `urn:uuid:`s lowercased; URLs and paths reduced to the path (any host,
/// e.g. a pXX-caldav.icloud.com partition, dropped) without trailing slash
fn normalise_address(address: &str) -> String {
    let address = address.trim();
    if let Some(email) = strip_prefix_ignore_case(address, "mailto:") {
        return email.trim().to_lowercase();
    }
    let path = ["https://", "http://"]
        .into_iter()
        .find_map(|scheme| strip_prefix_ignore_case(address, scheme))
        .map(|rest| rest.find('/').map_or("/", |i| &rest[i..]));
    match path.or_else(|| address.starts_with('/').then_some(address)) {
        Some(path) => path.trim_end_matches('/').to_string(),
        None => address.to_lowercase(),
    }
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

/// Whether you're going. An event you organise is yours; otherwise your own
/// attendee entry's PARTSTAT decides. One that doesn't list you (or anyone),
/// or with no idea who you are, counts as accepted.
fn self_accepted(attendees: &[ICalAttendee], me: &Me) -> bool {
    let mine: Vec<&ICalAttendee> = attendees.iter().filter(|a| me.is(a)).collect();
    mine.is_empty()
        || mine.iter().any(|a| a.is_organizer || !matches!(a.partstat.as_str(), "NEEDS-ACTION" | "TENTATIVE" | "DECLINED"))
}

/// Unfold iCal lines (lines starting with space/tab are continuations)
fn unfold_ical_lines(data: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for line in data.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // Continuation line
            current.push_str(line.trim_start());
        } else {
            if !current.is_empty() {
                result.push(current);
            }
            current = line.to_string();
        }
    }
    if !current.is_empty() {
        result.push(current);
    }

    result
}

/// Parse a single iCal line into key and value, split at the first colon
/// outside a quoted parameter (`SENT-BY="mailto:..."` has one inside)
fn parse_ical_line(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    let colon_pos = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(i),
        _ => None,
    })?;
    Some((&line[..colon_pos], &line[colon_pos + 1..]))
}

/// Parse iCal datetime value
fn parse_ical_datetime(key: &str, value: &str) -> Option<EventTime> {
    // Check if it's a date-only value (VALUE=DATE parameter or 8-digit date)
    if key.contains("VALUE=DATE") && !key.contains("VALUE=DATE-TIME") {
        // Parse YYYYMMDD
        let year = value.get(0..4)?.parse().ok()?;
        let month = value.get(4..6)?.parse().ok()?;
        let day = value.get(6..8)?.parse().ok()?;
        return NaiveDate::from_ymd_opt(year, month, day).map(EventTime::Date);
    }

    // Handle pure date without time (8 digits, no T)
    if value.len() == 8 && !value.contains('T') {
        let year = value.get(0..4)?.parse().ok()?;
        let month = value.get(4..6)?.parse().ok()?;
        let day = value.get(6..8)?.parse().ok()?;
        return NaiveDate::from_ymd_opt(year, month, day).map(EventTime::Date);
    }

    // Parse datetime: YYYYMMDDTHHMMSS, YYYYMMDDTHHMMSSZ, or with TZID
    // Handles: DTSTART:20260108T200000Z
    //          DTSTART;TZID=Europe/Sofia:20260108T200000
    let is_utc = value.ends_with('Z');
    let value = value.trim_end_matches('Z');
    if value.contains('T') {
        let t_pos = value.find('T')?;
        let date_part = &value[..t_pos];
        let time_part = &value[t_pos + 1..];

        if date_part.len() >= 8 && time_part.len() >= 6 {
            let year = date_part.get(0..4)?.parse().ok()?;
            let month = date_part.get(4..6)?.parse().ok()?;
            let day = date_part.get(6..8)?.parse().ok()?;

            let hour = time_part.get(0..2)?.parse().ok()?;
            let minute = time_part.get(2..4)?.parse().ok()?;
            let second = time_part.get(4..6)?.parse().ok()?;

            let naive = NaiveDate::from_ymd_opt(year, month, day)?
                .and_hms_opt(hour, minute, second)?;
            return Some(EventTime::DateTime(resolve_datetime(naive, is_utc, extract_tzid(key))));
        }
    }

    None
}

/// Interpret a parsed wall-clock time: `Z` values are UTC, `TZID` values are in
/// that zone, and floating values (or zones chrono-tz doesn't know, like
/// Windows names from Outlook invites) are taken as local time
fn resolve_datetime(naive: NaiveDateTime, is_utc: bool, tzid: Option<&str>) -> DateTime<Utc> {
    if is_utc {
        return DateTime::from_naive_utc_and_offset(naive, Utc);
    }
    let resolved = match tzid.and_then(|t| t.parse::<chrono_tz::Tz>().ok()) {
        Some(tz) => tz.from_local_datetime(&naive).earliest().map(|dt| dt.with_timezone(&Utc)),
        None => Local.from_local_datetime(&naive).earliest().map(|dt| dt.with_timezone(&Utc)),
    };
    // A wall time inside a DST gap doesn't exist; fall back to reading it as UTC
    resolved.unwrap_or_else(|| DateTime::from_naive_utc_and_offset(naive, Utc))
}

/// Extract the TZID parameter from a property key like "DTSTART;TZID=Europe/Sofia"
fn extract_tzid(key: &str) -> Option<&str> {
    key.split(';')
        .find_map(|part| part.strip_prefix("TZID="))
        .map(|tz| tz.trim_matches('"'))
}

/// Format a UTC instant as local HH:MM
#[cfg(test)]
fn format_local_hm(dt: &DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

/// Unescape iCal text values
fn unescape_ical(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

/// Extract PARTSTAT value from an ATTENDEE line key
/// e.g., "ATTENDEE;PARTSTAT=ACCEPTED;CN=..." -> "ACCEPTED"
fn extract_partstat(key: &str) -> Option<String> {
    for part in key.split(';') {
        if let Some(stripped) = part.strip_prefix("PARTSTAT=") {
            return Some(stripped.to_string());
        }
    }
    None
}

/// Extract CN (Common Name) from ATTENDEE/ORGANIZER line key
/// e.g., "ATTENDEE;CN=John Smith;PARTSTAT=ACCEPTED" -> "John Smith"
fn extract_cn(key: &str) -> Option<String> {
    for part in key.split(';') {
        if let Some(name) = part.strip_prefix("CN=") {
            // Remove surrounding quotes if present
            let name = name.trim_matches('"');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Extract a parameter's value from a property key, quotes removed
/// e.g. ("ATTENDEE;EMAIL=\"a@b.c\";CN=A", "EMAIL") -> "a@b.c"
fn extract_param<'a>(key: &'a str, name: &str) -> Option<&'a str> {
    key.split(';')
        .skip(1)
        .find_map(|part| part.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(name)))
        .map(|(_, v)| v.trim_matches('"'))
        .filter(|v| !v.is_empty())
}

/// Parse ATTENDEE or ORGANIZER line into ICalAttendee
/// key: "ATTENDEE;PARTSTAT=ACCEPTED;CN=John Smith"
/// value: "mailto:john@example.com", or for iCloud users often a principal
/// path ("/aMTM...=/principal/") or "urn:uuid:..." with the address in EMAIL=
fn parse_attendee(key: &str, value: &str) -> Option<ICalAttendee> {
    let address = value.trim();
    if address.is_empty() {
        return None;
    }
    let email = match strip_prefix_ignore_case(address, "mailto:") {
        Some(email) => email,
        None => extract_param(key, "EMAIL").and_then(|e| strip_prefix_ignore_case(e, "mailto:").or(Some(e))).unwrap_or(address),
    }
    .to_string();

    // Skip if no valid email
    if email.is_empty() {
        return None;
    }

    // Extract display name (CN)
    let name = extract_cn(key);

    // Extract participation status
    let partstat = extract_partstat(key).unwrap_or_else(|| "NEEDS-ACTION".to_string());

    Some(ICalAttendee {
        name,
        email,
        address: address.to_string(),
        partstat,
        is_organizer: false, // Caller sets this for ORGANIZER lines
    })
}

use crate::utils::{is_meeting_url, meeting_url};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_ical_event() {
        let ical = r#"BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:test-123@example.com
SUMMARY:Team Meeting
DTSTART:20260115T143000Z
DTEND:20260115T153000Z
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title(), "Team Meeting");
        assert_eq!(events[0].uid, "test-123@example.com");
    }

    #[test]
    fn test_parse_all_day_event() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:holiday-123
SUMMARY:Company Holiday
DTSTART;VALUE=DATE:20260101
DTEND;VALUE=DATE:20260102
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].time_str(), "All day");
        assert_eq!(events[0].start_date(), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    }

    #[test]
    fn test_parse_event_with_timezone() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:tz-event
SUMMARY:Sofia Meeting
DTSTART;TZID=Europe/Sofia:20260108T200000
DTEND;TZID=Europe/Sofia:20260108T210000
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title(), "Sofia Meeting");
        // 20:00 in Sofia, shown in whatever the local zone is
        let expected = chrono_tz::Europe::Sofia
            .with_ymd_and_hms(2026, 1, 8, 20, 0, 0).unwrap()
            .with_timezone(&Local);
        assert_eq!(events[0].time_str(), expected.format("%H:%M").to_string());
        assert_eq!(events[0].start_date(), expected.date_naive());
    }

    #[test]
    fn test_parse_event_no_title() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:no-title
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title(), "(No title)");
    }

    #[test]
    fn test_parse_event_with_location_and_description() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:full-event
SUMMARY:Office Meeting
DTSTART:20260115T140000Z
DTEND:20260115T150000Z
LOCATION:Conference Room A
DESCRIPTION:Weekly sync meeting
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].location, Some("Conference Room A".to_string()));
        assert_eq!(events[0].description, Some("Weekly sync meeting".to_string()));
    }

    #[test]
    fn test_parse_event_with_escaped_characters() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:escaped
SUMMARY:Meeting\, with comma
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
DESCRIPTION:Line 1\nLine 2
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, Some("Meeting, with comma".to_string()));
        assert_eq!(events[0].description, Some("Line 1\nLine 2".to_string()));
    }

    #[test]
    fn test_parse_multiple_events() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:event-1
SUMMARY:First
DTSTART:20260115T090000Z
DTEND:20260115T100000Z
END:VEVENT
BEGIN:VEVENT
UID:event-2
SUMMARY:Second
DTSTART:20260115T110000Z
DTEND:20260115T120000Z
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title(), "First");
        assert_eq!(events[1].title(), "Second");
    }

    #[test]
    fn test_parse_folded_lines() {
        let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:folded\r\nSUMMARY:This is a very long summary that has been\r\n  folded across multiple lines\r\nDTSTART:20260115T100000Z\r\nDTEND:20260115T110000Z\r\nEND:VEVENT\r\nEND:VCALENDAR";

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert!(events[0].title().contains("folded across multiple lines"));
    }

    #[test]
    fn test_partstat_accepted() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:accepted-event
SUMMARY:Meeting
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
ATTENDEE;PARTSTAT=ACCEPTED;CN=Me:mailto:me@example.com
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert!(events[0].accepted);
    }

    #[test]
    fn test_partstat_declined() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:declined-event
SUMMARY:Meeting
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
ATTENDEE;PARTSTAT=DECLINED;CN=Me:mailto:me@example.com
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert!(!events[0].accepted);
    }

    #[test]
    fn test_partstat_needs_action() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:needs-action
SUMMARY:Meeting
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
ATTENDEE;PARTSTAT=NEEDS-ACTION;CN=Me:mailto:me@example.com
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert!(!events[0].accepted);
    }

    #[test]
    fn test_meeting_url_from_url_field() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:zoom-event
SUMMARY:Zoom Call
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
URL:https://zoom.us/j/123456789
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].meeting_url(), Some("https://zoom.us/j/123456789".to_string()));
    }

    #[test]
    fn test_meeting_url_from_location() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:meet-event
SUMMARY:Google Meet
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
LOCATION:https://meet.google.com/abc-defg-hij
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].meeting_url(), Some("https://meet.google.com/abc-defg-hij".to_string()));
    }

    #[test]
    fn test_unfold_ical_lines() {
        let folded = "SUMMARY:This is\r\n  a folded line";
        let unfolded = unfold_ical_lines(folded);
        assert_eq!(unfolded.len(), 1);
        assert_eq!(unfolded[0], "SUMMARY:This isa folded line");
    }

    #[test]
    fn test_parse_ical_line() {
        assert_eq!(parse_ical_line("SUMMARY:Test"), Some(("SUMMARY", "Test")));
        assert_eq!(parse_ical_line("DTSTART;VALUE=DATE:20260101"), Some(("DTSTART;VALUE=DATE", "20260101")));
        assert_eq!(parse_ical_line("no colon here"), None);
    }

    #[test]
    fn test_unescape_ical() {
        assert_eq!(unescape_ical("test\\nline"), "test\nline");
        assert_eq!(unescape_ical("a\\,b\\;c"), "a,b;c");
        assert_eq!(unescape_ical("path\\\\to\\\\file"), "path\\to\\file");
    }

    #[test]
    fn test_extract_partstat() {
        assert_eq!(extract_partstat("ATTENDEE;PARTSTAT=ACCEPTED;CN=Test"), Some("ACCEPTED".to_string()));
        assert_eq!(extract_partstat("ATTENDEE;CN=Test;PARTSTAT=DECLINED"), Some("DECLINED".to_string()));
        assert_eq!(extract_partstat("ATTENDEE;CN=Test"), None);
    }

    #[test]
    fn test_extract_cn() {
        assert_eq!(extract_cn("ATTENDEE;CN=John Smith;PARTSTAT=ACCEPTED"), Some("John Smith".to_string()));
        assert_eq!(extract_cn("ATTENDEE;PARTSTAT=ACCEPTED;CN=Jane Doe"), Some("Jane Doe".to_string()));
        assert_eq!(extract_cn("ATTENDEE;CN=\"Quoted Name\""), Some("Quoted Name".to_string()));
        assert_eq!(extract_cn("ATTENDEE;PARTSTAT=ACCEPTED"), None);
    }

    #[test]
    fn test_parse_attendee() {
        let attendee = parse_attendee("ATTENDEE;PARTSTAT=ACCEPTED;CN=John Smith", "mailto:john@example.com").unwrap();
        assert_eq!(attendee.name, Some("John Smith".to_string()));
        assert_eq!(attendee.email, "john@example.com");
        assert_eq!(attendee.partstat, "ACCEPTED");
        assert!(!attendee.is_organizer);
    }

    #[test]
    fn test_parse_attendee_no_cn() {
        let attendee = parse_attendee("ATTENDEE;PARTSTAT=DECLINED", "mailto:bob@example.com").unwrap();
        assert_eq!(attendee.name, None);
        assert_eq!(attendee.email, "bob@example.com");
        assert_eq!(attendee.partstat, "DECLINED");
    }

    #[test]
    fn test_parse_attendee_no_partstat() {
        let attendee = parse_attendee("ATTENDEE;CN=Unknown", "mailto:unknown@example.com").unwrap();
        assert_eq!(attendee.partstat, "NEEDS-ACTION");
    }

    #[test]
    fn test_parse_event_with_attendees() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:meeting-with-attendees
SUMMARY:Team Standup
DTSTART:20260115T100000Z
DTEND:20260115T110000Z
ORGANIZER;CN=Alice Manager:mailto:alice@example.com
ATTENDEE;PARTSTAT=ACCEPTED;CN=Bob Developer:mailto:bob@example.com
ATTENDEE;PARTSTAT=DECLINED;CN=Charlie Designer:mailto:charlie@example.com
ATTENDEE;PARTSTAT=TENTATIVE:mailto:dave@example.com
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].attendees.len(), 4);

        // Check organizer
        let organizer = events[0].attendees.iter().find(|a| a.is_organizer).unwrap();
        assert_eq!(organizer.name, Some("Alice Manager".to_string()));
        assert_eq!(organizer.email, "alice@example.com");

        // Check accepted attendee
        let bob = events[0].attendees.iter().find(|a| a.email == "bob@example.com").unwrap();
        assert_eq!(bob.name, Some("Bob Developer".to_string()));
        assert_eq!(bob.partstat, "ACCEPTED");

        // Check declined attendee
        let charlie = events[0].attendees.iter().find(|a| a.email == "charlie@example.com").unwrap();
        assert_eq!(charlie.partstat, "DECLINED");

        // Check attendee without name
        let dave = events[0].attendees.iter().find(|a| a.email == "dave@example.com").unwrap();
        assert_eq!(dave.name, None);
        assert_eq!(dave.partstat, "TENTATIVE");

        // You aren't on the list: whoever replied last doesn't decide for you
        assert!(events[0].accepted);
    }

    fn event_with(lines: &str) -> ICalEvent {
        let ical = format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:u\nDTSTART:20260115T100000Z\n{lines}\nEND:VEVENT\nEND:VCALENDAR");
        ICalEvent::parse_ical(&ical).remove(0)
    }

    #[test]
    fn test_acceptance_is_your_own_reply_not_the_last_one() {
        let accepted = event_with(
            "ATTENDEE;PARTSTAT=ACCEPTED:mailto:Me@Example.com\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:late@example.com",
        );
        assert!(accepted.accepted, "another guest not replying doesn't make it unaccepted");
        let declined = event_with(
            "ATTENDEE;PARTSTAT=DECLINED:MAILTO:me@example.com\nATTENDEE;PARTSTAT=ACCEPTED:mailto:other@example.com",
        );
        assert!(!declined.accepted, "matched case-insensitively, MAILTO: too");
        assert_eq!(declined.attendees[0].email, "me@example.com");
    }

    #[test]
    fn test_events_you_organise_are_accepted() {
        // Organizer lines rarely carry a PARTSTAT
        let mine = event_with("ORGANIZER;CN=Me:mailto:me@example.com\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:guest@example.com");
        assert!(mine.accepted);
        assert!(event_with("SUMMARY:No guests").accepted);
    }

    /// Parse one event as the iCloud account with the Apple ID
    /// me@gmail.com, the address me@icloud.com and the given principal
    fn icloud_event_with(lines: &str) -> ICalEvent {
        let me = Me::new(["mailto:me@gmail.com", "mailto:me@icloud.com", "/aMTM4NTk=/principal/", "urn:uuid:AB12-CD34"]);
        let ical = format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:u\nDTSTART:20260115T100000Z\n{lines}\nEND:VEVENT\nEND:VCALENDAR");
        ICalEvent::parse_ical_with_source(&ical, String::new(), None, None, &me).remove(0)
    }

    #[test]
    fn test_invites_from_icloud_users_find_you_by_principal() {
        // Shape iCloud writes: organizer and guests as principal paths
        let organiser = "ORGANIZER;CN=Friend;EMAIL=friend@icloud.com:/aMTI3NzI=/principal/\nATTENDEE;CN=Friend;PARTSTAT=ACCEPTED;ROLE=CHAIR:/aMTI3NzI=/principal/";
        let declined = icloud_event_with(&format!("{organiser}\nATTENDEE;CN=Me;PARTSTAT=DECLINED:/aMTM4NTk=/principal/"));
        assert!(!declined.accepted, "declined by principal path");
        let pending = icloud_event_with(&format!(
            "{organiser}\nATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:https://p42-caldav.icloud.com:443/aMTM4NTk=/principal"
        ));
        assert!(!pending.accepted, "full URL with a partition host and no trailing slash still matches");
        let accepted = icloud_event_with(&format!("{organiser}\nATTENDEE;PARTSTAT=ACCEPTED:urn:uuid:ab12-cd34"));
        assert!(accepted.accepted, "urn:uuid matched case-insensitively");
        let tentative = icloud_event_with(&format!("{organiser}\nATTENDEE;PARTSTAT=TENTATIVE:urn:uuid:AB12-CD34"));
        assert!(!tentative.accepted);
    }

    #[test]
    fn test_email_param_identifies_you_and_is_shown() {
        let event = icloud_event_with(
            "ORGANIZER;CN=Friend:mailto:friend@example.com\nATTENDEE;CN=Me;PARTSTAT=DECLINED;EMAIL=\"Me@iCloud.com\":/aOTHER=/principal/",
        );
        assert!(!event.accepted, "matched by EMAIL=, not the unknown path");
        let me = &event.attendees[1];
        assert_eq!((me.email.as_str(), me.address.as_str()), ("Me@iCloud.com", "/aOTHER=/principal/"));
        // Without EMAIL= the path is all there is
        let path_only = parse_attendee("ATTENDEE;CN=X", "/aX=/principal/").unwrap();
        assert_eq!(path_only.email, "/aX=/principal/");
        let quoted = parse_attendee("ATTENDEE;EMAIL=\"mailto:x@y.z\"", "urn:uuid:1").unwrap();
        assert_eq!(quoted.email, "x@y.z");
    }

    #[test]
    fn test_organising_with_another_of_your_addresses_is_accepted() {
        // Your own event, with your work address invited and not yet replied
        let event = icloud_event_with(
            "ORGANIZER;CN=Me;EMAIL=me@icloud.com:/aMTM4NTk=/principal/\nATTENDEE;CN=Me;PARTSTAT=ACCEPTED:/aMTM4NTk=/principal/\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@work.com",
        );
        assert!(event.accepted);
        let only_organiser = icloud_event_with(
            "ORGANIZER:/aMTM4NTk=/principal/\nATTENDEE;PARTSTAT=DECLINED:mailto:guest@example.com",
        );
        assert!(only_organiser.accepted, "a guest declining doesn't decline your own event");
    }

    #[test]
    fn test_invites_not_naming_you_count_as_accepted() {
        let event = icloud_event_with(
            "ORGANIZER:/aMTI3NzI=/principal/\nATTENDEE;PARTSTAT=DECLINED:/aOTHER=/principal/\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:someone@example.com",
        );
        assert!(event.accepted, "the last attendee's reply isn't yours");
    }

    #[test]
    fn test_quoted_parameters_may_contain_colons() {
        assert_eq!(
            parse_ical_line("ORGANIZER;SENT-BY=\"mailto:pa@example.com\";CN=Boss:mailto:boss@example.com"),
            Some(("ORGANIZER;SENT-BY=\"mailto:pa@example.com\";CN=Boss", "mailto:boss@example.com"))
        );
    }

    #[test]
    fn test_address_normalisation() {
        assert_eq!(normalise_address(" MAILTO:Me@Example.COM "), "me@example.com");
        assert_eq!(normalise_address("https://p42-caldav.icloud.com:443/aX=/principal/"), "/aX=/principal");
        assert_eq!(normalise_address("/aX=/principal/"), "/aX=/principal");
        assert_eq!(normalise_address("urn:uuid:AB-CD"), "urn:uuid:ab-cd");
        assert!(Me::new(["", "mailto:"]).addresses.is_empty());
    }

    #[test]
    fn test_end_time_str() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:timed-event
SUMMARY:Meeting
DTSTART:20260115T143000
DTEND:20260115T160000
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].end_time_str(), Some("16:00".to_string()));
    }

    #[test]
    fn test_end_time_str_all_day() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:all-day
SUMMARY:Holiday
DTSTART;VALUE=DATE:20260115
DTEND;VALUE=DATE:20260116
END:VEVENT
END:VCALENDAR"#;

        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].end_time_str(), None);
    }

    #[test]
    fn test_utc_times_are_shown_in_local_time() {
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:utc\nSUMMARY:UTC\nDTSTART:20260115T143000Z\nDTEND:20260115T153000Z\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        let expected = Utc.with_ymd_and_hms(2026, 1, 15, 14, 30, 0).unwrap().with_timezone(&Local);
        assert_eq!(events[0].time_str(), expected.format("%H:%M").to_string());
    }

    #[test]
    fn test_floating_times_are_local_wall_time() {
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:float\nSUMMARY:Floating\nDTSTART:20260115T090000\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events[0].time_str(), "09:00");
        assert_eq!(events[0].start_date(), NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
    }

    #[test]
    fn test_foreign_tzid_is_converted() {
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:ny\nSUMMARY:NY\nDTSTART;TZID=America/New_York:20260115T100000\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        let expected = chrono_tz::America::New_York
            .with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap()
            .with_timezone(&Local);
        assert_eq!(events[0].time_str(), expected.format("%H:%M").to_string());
    }

    #[test]
    fn test_unknown_tzid_falls_back_to_local() {
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:win\nSUMMARY:Outlook\nDTSTART;TZID=\"W. Europe Standard Time\":20260115T100000\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events[0].time_str(), "10:00");
    }

    #[test]
    fn test_valarm_properties_do_not_leak_into_event() {
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:event-uid\nSUMMARY:Real\nDTSTART:20260115T100000\nBEGIN:VALARM\nUID:alarm-uid\nDESCRIPTION:Reminder\nACTION:DISPLAY\nEND:VALARM\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "event-uid");
        assert_eq!(events[0].description, None);
    }

    #[test]
    fn test_expanded_recurrence_instance() {
        // Shape of an instance returned by iCloud for a REPORT with <c:expand>
        let ical = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:bday\nSUMMARY:Birthday\nDTSTART;VALUE=DATE:20261023\nDTEND;VALUE=DATE:20261024\nRECURRENCE-ID:20261023T000000Z\nEND:VEVENT\nEND:VCALENDAR";
        let events = ICalEvent::parse_ical(ical);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start_date(), NaiveDate::from_ymd_opt(2026, 10, 23).unwrap());
        assert_eq!(events[0].time_str(), "All day");
    }
}
