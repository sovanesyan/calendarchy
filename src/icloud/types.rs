use chrono::{DateTime, NaiveDate, Utc};

/// Attendee from iCal ATTENDEE line
#[derive(Debug, Clone)]
pub struct ICalAttendee {
    pub name: Option<String>,
    pub email: String,
    pub partstat: String,  // ACCEPTED, DECLINED, TENTATIVE, NEEDS-ACTION
    pub is_organizer: bool,
}

/// An event from iCloud Calendar (parsed from iCal/VCALENDAR format)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ICalEvent {
    pub uid: String,
    pub summary: Option<String>,
    pub dtstart: EventTime,
    pub dtend: Option<EventTime>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub accepted: bool, // true if accepted or no PARTSTAT found
    pub attendees: Vec<ICalAttendee>,
}

/// Event time - can be all-day (date only) or specific time
#[derive(Debug, Clone)]
pub enum EventTime {
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
}

impl ICalEvent {
    /// Get the start date (works for both all-day and timed events)
    pub fn start_date(&self) -> NaiveDate {
        match &self.dtstart {
            EventTime::Date(d) => *d,
            EventTime::DateTime(dt) => dt.date_naive(),
        }
    }

    /// Get display title
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(No title)")
    }

    /// Get start time as HH:MM or "All day"
    pub fn time_str(&self) -> String {
        match &self.dtstart {
            EventTime::Date(_) => "All day".to_string(),
            EventTime::DateTime(dt) => {
                use chrono::Timelike;
                format!("{:02}:{:02}", dt.time().hour(), dt.time().minute())
            }
        }
    }

    /// Get end time as HH:MM or None for all-day events
    pub fn end_time_str(&self) -> Option<String> {
        match &self.dtend {
            Some(EventTime::DateTime(dt)) => {
                use chrono::Timelike;
                Some(format!("{:02}:{:02}", dt.time().hour(), dt.time().minute()))
            }
            _ => None,
        }
    }

    /// Extract meeting URL (Zoom, Google Meet, etc.)
    pub fn meeting_url(&self) -> Option<String> {
        // Check URL field first
        if let Some(ref url) = self.url {
            if is_meeting_url(url) {
                return Some(url.clone());
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

    /// Parse an iCal VCALENDAR string into events
    pub fn parse_ical(ical_data: &str) -> Vec<ICalEvent> {
        let mut events = Vec::new();
        let mut current_event: Option<ICalEventBuilder> = None;

        for line in unfold_ical_lines(ical_data) {
            let line = line.trim();

            if line == "BEGIN:VEVENT" {
                current_event = Some(ICalEventBuilder::default());
            } else if line == "END:VEVENT" {
                if let Some(builder) = current_event.take() {
                    if let Some(event) = builder.build() {
                        events.push(event);
                    }
                }
            } else if let Some(ref mut builder) = current_event {
                if let Some((key, value)) = parse_ical_line(line) {
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
                            // Extract PARTSTAT from ATTENDEE line for self acceptance
                            if let Some(partstat) = extract_partstat(key) {
                                builder.partstat = Some(partstat.clone());
                            }
                            // Parse attendee details
                            if let Some(attendee) = parse_attendee(key, value) {
                                builder.attendees.push(attendee);
                            }
                        }
                        "ORGANIZER" => {
                            // Parse organizer as an attendee
                            if let Some(mut attendee) = parse_attendee(key, value) {
                                attendee.is_organizer = true;
                                attendee.partstat = "ACCEPTED".to_string();
                                builder.attendees.push(attendee);
                            }
                        }
                        _ => {}
                    }
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
    partstat: Option<String>, // NEEDS-ACTION, ACCEPTED, DECLINED, TENTATIVE
    attendees: Vec<ICalAttendee>,
}

impl ICalEventBuilder {
    fn build(self) -> Option<ICalEvent> {
        // Default to accepted if no PARTSTAT or if ACCEPTED
        let accepted = match self.partstat.as_deref() {
            None => true,
            Some("ACCEPTED") => true,
            Some("NEEDS-ACTION") | Some("TENTATIVE") | Some("DECLINED") => false,
            _ => true,
        };

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
        })
    }
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

/// Parse a single iCal line into key and value
fn parse_ical_line(line: &str) -> Option<(&str, &str)> {
    let colon_pos = line.find(':')?;
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
            return Some(EventTime::DateTime(DateTime::from_naive_utc_and_offset(naive, Utc)));
        }
    }

    None
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
        if part.starts_with("PARTSTAT=") {
            return Some(part[9..].to_string());
        }
    }
    None
}

/// Extract CN (Common Name) from ATTENDEE/ORGANIZER line key
/// e.g., "ATTENDEE;CN=John Smith;PARTSTAT=ACCEPTED" -> "John Smith"
fn extract_cn(key: &str) -> Option<String> {
    for part in key.split(';') {
        if part.starts_with("CN=") {
            let name = &part[3..];
            // Remove surrounding quotes if present
            let name = name.trim_matches('"');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Parse ATTENDEE or ORGANIZER line into ICalAttendee
/// key: "ATTENDEE;PARTSTAT=ACCEPTED;CN=John Smith"
/// value: "mailto:john@example.com"
fn parse_attendee(key: &str, value: &str) -> Option<ICalAttendee> {
    // Extract email from mailto: value
    let email = if value.starts_with("mailto:") {
        value[7..].to_string()
    } else {
        value.to_string()
    };

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
        partstat,
        is_organizer: false, // Caller sets this for ORGANIZER lines
    })
}

/// Check if a URL is a meeting URL
fn is_meeting_url(url: &str) -> bool {
    url.contains("zoom.us")
        || url.contains("meet.google.com")
        || url.contains("teams.microsoft.com")
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
        assert_eq!(events[0].time_str(), "20:00");
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
    fn test_is_meeting_url() {
        assert!(is_meeting_url("https://zoom.us/j/123"));
        assert!(is_meeting_url("https://meet.google.com/abc"));
        assert!(is_meeting_url("https://teams.microsoft.com/l/meetup"));
        assert!(!is_meeting_url("https://example.com"));
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
        assert_eq!(organizer.partstat, "ACCEPTED");

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
    }

    #[test]
    fn test_end_time_str() {
        let ical = r#"BEGIN:VCALENDAR
BEGIN:VEVENT
UID:timed-event
SUMMARY:Meeting
DTSTART:20260115T143000Z
DTEND:20260115T160000Z
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
}
