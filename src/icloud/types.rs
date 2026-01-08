use chrono::{DateTime, NaiveDate, Utc};

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
    pub accepted: bool, // true if accepted or no PARTSTAT found
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
                        "ATTENDEE" => {
                            // Extract PARTSTAT from ATTENDEE line
                            if let Some(partstat) = extract_partstat(key) {
                                builder.partstat = Some(partstat);
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
    partstat: Option<String>, // NEEDS-ACTION, ACCEPTED, DECLINED, TENTATIVE
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
            accepted,
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
