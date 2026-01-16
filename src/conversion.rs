use crate::cache::{AttendeeStatus, DisplayAttendee, DisplayEvent, EventId};
use crate::google;
use crate::icloud::ICalEvent;
use crate::utils::{name_from_email, sort_attendees};

/// Convert a Google CalendarEvent to a DisplayEvent
pub fn google_event_to_display(
    event: google::types::CalendarEvent,
    calendar_id: String,
    calendar_name: Option<String>,
) -> Option<DisplayEvent> {
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

    Some(DisplayEvent {
        id: EventId::Google {
            calendar_id,
            event_id: event.id.clone(),
            calendar_name,
        },
        title: event.title().to_string(),
        time_str: event.time_str(),
        end_time_str: event.end_time_str(),
        date: event.start_date()?,
        accepted: event.is_accepted(),
        is_organizer: event.is_organizer(),
        is_free: event.is_free(),
        meeting_url: event.meeting_url(),
        description: event.description.clone(),
        location: event.location.clone(),
        attendees,
    })
}

/// Convert an iCloud ICalEvent to a DisplayEvent
pub fn icloud_event_to_display(event: ICalEvent, calendar_name: Option<String>) -> DisplayEvent {
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

    DisplayEvent {
        id: EventId::ICloud {
            calendar_url: event.calendar_url.clone(),
            event_uid: event.uid.clone(),
            etag: event.etag.clone(),
            calendar_name,
        },
        title: event.title().to_string(),
        time_str: event.time_str(),
        end_time_str: event.end_time_str(),
        date: event.start_date(),
        accepted: event.accepted,
        is_organizer,
        is_free: event.is_free(),
        meeting_url: event.meeting_url(),
        description: event.description.clone(),
        location: event.location.clone(),
        attendees,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icloud;
    use chrono::NaiveDate;

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

    #[test]
    fn test_google_event_to_display_basic() {
        let event = make_google_event("event-123", "Team Meeting", NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        let result = google_event_to_display(event, "cal-id".to_string(), Some("Work".to_string()));

        assert!(result.is_some());
        let display = result.unwrap();
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

        let result = google_event_to_display(event, "cal-id".to_string(), None);
        assert!(result.is_some());
        let display = result.unwrap();

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
        };

        let display = icloud_event_to_display(event, Some("Personal".to_string()));

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
        };

        let display = icloud_event_to_display(event, None);

        assert!(!display.is_organizer); // Has attendees, not organizer
        assert_eq!(display.attendees.len(), 1);
        assert_eq!(display.attendees[0].status, AttendeeStatus::Accepted);
    }
}
