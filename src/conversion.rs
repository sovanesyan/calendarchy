use chrono::{Duration, Local, NaiveDate};

use crate::cache::{occurrences, AttendeeStatus, DisplayAttendee, DisplayEvent, EventId, Span, When};
use crate::google;
use crate::icloud::{EventTime, ICalEvent};
use crate::utils::{name_from_email, sort_attendees};

/// Copy an event once per day it covers within `window`
fn per_day(event: DisplayEvent, span: Span, window: (NaiveDate, NaiveDate)) -> Vec<DisplayEvent> {
    let spans_days = span.spans_days();
    occurrences(span, window)
        .into_iter()
        .map(|(date, when)| DisplayEvent { date, when, spans_days, ..event.clone() })
        .collect()
}

/// Convert a Google CalendarEvent to one DisplayEvent per day it covers in `window`
pub fn google_event_to_display(
    event: google::types::CalendarEvent,
    calendar_id: String,
    calendar_name: Option<String>,
    window: (NaiveDate, NaiveDate),
) -> Vec<DisplayEvent> {
    let span = match (event.start.date, event.start.date_time) {
        // All-day: end.date is exclusive
        (Some(first), _) => Span::AllDay {
            first,
            last: event.end.date.map_or(first, |end| (end - Duration::days(1)).max(first)),
        },
        (None, Some(start)) => Span::Timed {
            start: start.with_timezone(&Local),
            end: event.end.date_time.map(|end| end.with_timezone(&Local)),
        },
        (None, None) => return vec![],
    };
    let mut attendees: Vec<DisplayAttendee> = event.attendees.as_ref().map(|atts| {
        atts.iter()
            .filter_map(|a| {
                let email = a.email.clone()?;
                let status = if a.organizer == Some(true) {
                    AttendeeStatus::Organizer
                } else {
                    match a.response_status.as_deref() {
                        Some("accepted") => AttendeeStatus::Accepted,
                        Some("declined") => AttendeeStatus::Declined,
                        Some("tentative") => AttendeeStatus::Tentative,
                        _ => AttendeeStatus::NeedsAction,
                    }
                };
                Some(DisplayAttendee {
                    name: Some(a.display_name.clone().unwrap_or_else(|| name_from_email(&email))),
                    email,
                    status,
                })
            })
            .collect()
    }).unwrap_or_default();
    sort_attendees(&mut attendees);

    let base = DisplayEvent {
        id: EventId::Google {
            calendar_id,
            event_id: event.id.clone(),
            calendar_name,
        },
        title: event.title().to_string(),
        when: When::AllDay,
        date: window.0,
        spans_days: false,
        accepted: event.is_accepted(),
        is_organizer: event.is_organizer(),
        is_free: event.is_free(),
        meeting_url: event.meeting_url(),
        description: event.description.clone(),
        location: event.location.clone(),
        attendees,
    };
    per_day(base, span, window)
}

/// Convert an iCloud ICalEvent to one DisplayEvent per day it covers in `window`
pub fn icloud_event_to_display(
    event: ICalEvent,
    calendar_name: Option<String>,
    window: (NaiveDate, NaiveDate),
) -> Vec<DisplayEvent> {
    let span = match (&event.dtstart, &event.dtend) {
        // All-day: DTEND is exclusive
        (EventTime::Date(first), Some(EventTime::Date(end))) => Span::AllDay {
            first: *first,
            last: (*end - Duration::days(1)).max(*first),
        },
        (EventTime::Date(first), _) => Span::AllDay { first: *first, last: *first },
        (EventTime::DateTime(start), end) => Span::Timed {
            start: start.with_timezone(&Local),
            end: match end {
                Some(EventTime::DateTime(end)) => Some(end.with_timezone(&Local)),
                _ => None,
            },
        },
    };
    let mut attendees: Vec<DisplayAttendee> = event.attendees.iter()
        .map(|a| {
            let status = if a.is_organizer {
                AttendeeStatus::Organizer
            } else {
                match a.partstat.as_str() {
                    "ACCEPTED" => AttendeeStatus::Accepted,
                    "DECLINED" => AttendeeStatus::Declined,
                    "TENTATIVE" => AttendeeStatus::Tentative,
                    _ => AttendeeStatus::NeedsAction,
                }
            };
            DisplayAttendee {
                name: Some(a.name.clone().unwrap_or_else(|| name_from_email(&a.email))),
                email: a.email.clone(),
                status,
            }
        })
        .collect();
    sort_attendees(&mut attendees);

    // For iCloud, if there are no attendees, the user created the event
    let is_organizer = event.attendees.is_empty();

    let base = DisplayEvent {
        id: EventId::ICloud {
            calendar_url: event.calendar_url.clone(),
            event_uid: event.uid.clone(),
            etag: event.etag.clone(),
            calendar_name,
            href: event.href.clone(),
        },
        title: event.title().to_string(),
        when: When::AllDay,
        date: window.0,
        spans_days: false,
        accepted: event.accepted,
        is_organizer,
        is_free: event.is_free(),
        meeting_url: event.meeting_url(),
        description: event.description.clone(),
        location: event.location.clone(),
        attendees,
    };
    per_day(base, span, window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icloud;
    use chrono::{Datelike, NaiveDate};

    fn make_google_event(id: &str, summary: &str, date: NaiveDate) -> google::types::CalendarEvent {
        google::types::CalendarEvent {
            id: id.to_string(),
            summary: Some(summary.to_string()),
            start: google::types::EventDateTime {
                date: Some(date),
                date_time: None,
                time_zone: None,
            },
            end: google::types::EventDateTime {
                date: Some(date + chrono::Duration::days(1)),
                date_time: None,
                time_zone: None,
            },
            location: None,
            description: None,
            status: None,
            transparency: None,
            attendees: None,
            conference_data: None,
            hangout_link: None,
        }
    }

    fn year(y: i32) -> (NaiveDate, NaiveDate) {
        (NaiveDate::from_ymd_opt(y, 1, 1).unwrap(), NaiveDate::from_ymd_opt(y, 12, 31).unwrap())
    }

    #[test]
    fn test_google_multi_day_event_appears_on_every_day() {
        // A week of leave: all-day, end date exclusive
        let mut event = make_google_event("leave", "Leave", NaiveDate::from_ymd_opt(2026, 10, 5).unwrap());
        event.end.date = Some(NaiveDate::from_ymd_opt(2026, 10, 10).unwrap());
        let days: Vec<_> = google_event_to_display(event, "cal".into(), None, year(2026)).iter().map(|e| e.date.day()).collect();
        assert_eq!(days, [5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_google_timed_event_across_midnight() {
        use chrono::TimeZone;
        let mut event = make_google_event("late", "Late shift", NaiveDate::from_ymd_opt(2026, 10, 9).unwrap());
        let at = |d, h| chrono::Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap().with_timezone(&chrono::Utc);
        event.start = google::types::EventDateTime { date: None, date_time: Some(at(9, 22)), time_zone: None };
        event.end = google::types::EventDateTime { date: None, date_time: Some(at(10, 2)), time_zone: None };
        let occ = google_event_to_display(event, "cal".into(), None, year(2026));
        assert_eq!(occ.len(), 2);
        assert_eq!((occ[0].date.day(), occ[0].time_label(), occ[0].when.end_label()), (9, "22:00".into(), Some("24:00".into())));
        assert_eq!((occ[1].date.day(), occ[1].time_label(), occ[1].when.end_label()), (10, "00:00".into(), Some("02:00".into())));
    }

    #[test]
    fn test_icloud_multi_day_all_day_event() {
        let mut event = sample_ical();
        event.dtstart = icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        event.dtend = Some(icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let october = (NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        let days: Vec<_> = icloud_event_to_display(event, None, october).iter().map(|e| e.date.day()).collect();
        assert_eq!(days, [30, 31], "clipped to the fetched month");
    }

    fn sample_ical() -> ICalEvent {
        ICalEvent {
            uid: "uid".to_string(),
            summary: Some("Trip".to_string()),
            dtstart: icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 20).unwrap()),
            dtend: None,
            location: None,
            description: None,
            url: None,
            attendees: vec![],
            accepted: true,
            transp: None,
            calendar_url: String::new(),
            etag: None,
            href: None,
        }
    }

    #[test]
    fn test_google_event_to_display_basic() {
        let event = make_google_event("event-123", "Team Meeting", NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        let result = google_event_to_display(event, "cal-id".to_string(), Some("Work".to_string()), year(2026));

        assert_eq!(result.len(), 1, "a one-day all-day event is one occurrence");
        let display = result.into_iter().next().unwrap();
        assert_eq!(display.when, When::AllDay);
        assert_eq!(display.title, "Team Meeting");
        assert_eq!(display.date, NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        assert!(matches!(display.id, EventId::Google { .. }));
    }

    #[test]
    fn test_google_event_to_display_with_attendees() {
        let mut event = make_google_event("event-456", "Review", NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        event.attendees = Some(vec![
            google::types::Attendee {
                email: Some("organizer@example.com".to_string()),
                display_name: Some("Organizer".to_string()),
                response_status: Some("accepted".to_string()),
                is_self: Some(false),
                organizer: Some(true),
            },
            google::types::Attendee {
                email: Some("attendee@example.com".to_string()),
                display_name: None,
                response_status: Some("tentative".to_string()),
                is_self: Some(true),
                organizer: None,
            },
        ]);

        let result = google_event_to_display(event, "cal-id".to_string(), None, year(2026));
        let display = result.into_iter().next().unwrap();

        assert_eq!(display.attendees.len(), 2);
        // Organizer should be sorted first
        assert_eq!(display.attendees[0].status, AttendeeStatus::Organizer);
        assert_eq!(display.attendees[1].status, AttendeeStatus::Tentative);
    }

    #[test]
    fn test_icloud_event_to_display_basic() {
        let event = ICalEvent {
            uid: "uid-123".to_string(),
            summary: Some("Personal Event".to_string()),
            dtstart: icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 20).unwrap()),
            dtend: Some(icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 1, 21).unwrap())),
            location: None,
            description: None,
            url: None,
            attendees: vec![],
            accepted: true,
            transp: None,
            calendar_url: "https://caldav.example.com/cal".to_string(),
            etag: Some("etag-abc".to_string()),
            href: None,
        };

        let result = icloud_event_to_display(event, Some("Personal".to_string()), year(2026));
        assert_eq!(result.len(), 1, "DTEND is exclusive");
        let display = result.into_iter().next().unwrap();

        assert_eq!(display.title, "Personal Event");
        assert_eq!(display.date, NaiveDate::from_ymd_opt(2026, 1, 20).unwrap());
        assert!(display.is_organizer); // No attendees means organizer
        assert!(matches!(display.id, EventId::ICloud { .. }));
    }

    #[test]
    fn test_icloud_event_to_display_with_attendees() {
        let event = ICalEvent {
            uid: "uid-456".to_string(),
            summary: Some("Meeting".to_string()),
            dtstart: icloud::EventTime::Date(NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()),
            dtend: None,
            location: None,
            description: None,
            url: None,
            attendees: vec![
                icloud::ICalAttendee {
                    email: "person@example.com".to_string(),
                    name: Some("Person".to_string()),
                    partstat: "ACCEPTED".to_string(),
                    is_organizer: false,
                },
            ],
            accepted: true,
            transp: None,
            calendar_url: "https://caldav.example.com/cal".to_string(),
            etag: None,
            href: None,
        };

        let display = icloud_event_to_display(event, None, year(2026)).into_iter().next().unwrap();

        assert!(!display.is_organizer); // Has attendees, not organizer
        assert_eq!(display.attendees.len(), 1);
        assert_eq!(display.attendees[0].status, AttendeeStatus::Accepted);
    }
}
