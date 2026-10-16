//! `calendarchy --agenda [--from YYYY-MM-DD] [--days N]`: days' events as
//! JSON, read from the disk cache only (no network, no auth), so it's cheap
//! enough to run every minute. The Omarchy shell plugin (`shell/`) draws it;
//! `--refresh` keeps the cache fresh.
//!
//! Times stay typed (minutes since midnight) and "next" is decided here, with
//! the same busy rule the TUI uses, so the plugin's QML only lays things out.

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike};
use serde::Serialize;

use crate::cache::{AttendeeStatus, DisplayEvent, EventCache, EventId, SourceCache};
use crate::config::Config;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Agenda {
    pub generated: String,
    /// The first day listed (today unless `--from` says otherwise)
    pub from: NaiveDate,
    /// Seconds since the cache was last written; None when there is no cache
    pub cache_age: Option<u64>,
    pub today: NaiveDate,
    /// Minutes since midnight when this was generated
    pub now: u16,
    pub twelve_hour: bool,
    /// Weeks start on Sunday (default Monday)
    pub sunday_first: bool,
    pub week_numbers: bool,
    /// Working hours as [start, end] minutes, if set
    pub working_hours: Option<(u16, u16)>,
    pub sources: Vec<SourceInfo>,
    pub days: Vec<Day>,
    /// The event running now, or else the next one, among busy events
    pub next: Option<Next>,
}

#[derive(Serialize)]
pub struct SourceInfo {
    pub key: &'static str,
    pub label: String,
    pub configured: bool,
}

#[derive(Serialize)]
pub struct Day {
    pub date: NaiveDate,
    pub google: Vec<Item>,
    pub icloud: Vec<Item>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Stable across refreshes: source, calendar and event id
    pub id: String,
    pub title: String,
    pub all_day: bool,
    pub start: Option<u16>,
    pub end: Option<u16>,
    pub spans_days: bool,
    pub accepted: bool,
    pub organizer: bool,
    pub free: bool,
    /// Blocks time: timed, accepted and not marked free
    pub busy: bool,
    pub meeting_url: Option<String>,
    /// What to open to join: Zoom links as zoommtg:// deep links (the app
    /// opens directly, no browser page), others as they are; as the TUI joins
    pub join_url: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub calendar_name: Option<String>,
    pub attendees: Vec<Attendee>,
}

#[derive(Serialize, Clone)]
pub struct Attendee {
    pub name: Option<String>,
    pub email: String,
    /// accepted, declined, tentative, needsAction or organizer
    pub status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Next {
    pub source: &'static str,
    pub date: NaiveDate,
    /// Under way now rather than upcoming
    pub current: bool,
    #[serde(flatten)]
    pub item: Item,
}

/// The id `--agenda` prints and `--event` takes: source, calendar, event
pub fn event_key(source: &str, e: &DisplayEvent) -> String {
    let (calendar, event) = e.id.identity();
    format!("{source}:{calendar}:{event}")
}

fn item(source: &str, e: &DisplayEvent) -> Item {
    let calendar_name = match &e.id {
        EventId::Google { calendar_name, .. } | EventId::ICloud { calendar_name, .. } => calendar_name.clone(),
    };
    Item {
        id: event_key(source, e),
        title: e.title.clone(),
        all_day: e.when.is_all_day(),
        start: e.when.start(),
        end: e.when.end(),
        spans_days: e.spans_days,
        accepted: e.accepted,
        organizer: e.is_organizer,
        free: e.is_free,
        busy: e.busy_range().is_some(),
        meeting_url: e.meeting_url.clone(),
        join_url: e.meeting_url.as_deref().map(|url| crate::utils::to_zoom_deeplink(url).unwrap_or_else(|| url.to_string())),
        location: e.location.clone(),
        description: e.description.clone(),
        calendar_name,
        attendees: e
            .attendees
            .iter()
            .map(|a| Attendee {
                name: a.name.clone(),
                email: a.email.clone(),
                status: match a.status {
                    AttendeeStatus::Accepted => "accepted",
                    AttendeeStatus::Declined => "declined",
                    AttendeeStatus::Tentative => "tentative",
                    AttendeeStatus::NeedsAction => "needsAction",
                    AttendeeStatus::Organizer => "organizer",
                },
            })
            .collect(),
    }
}

/// The busy event under way at `now` (the latest-starting, if several), or
/// else the first one still to start, today first and then the later days
fn find_next(cache: &EventCache, today: NaiveDate, now: u16, days: u32) -> Option<Next> {
    let sources: [(&'static str, &SourceCache); 2] = [("google", &cache.google), ("icloud", &cache.icloud)];
    for offset in 0..days {
        let date = today + Duration::days(offset as i64);
        let busy = || {
            sources.iter().flat_map(move |(key, source)| {
                source.get(date).iter().filter_map(move |e| Some((*key, e, e.busy_range()?)))
            })
        };
        let pick = |key: &'static str, e: &DisplayEvent, current: bool| Next { source: key, date, current, item: item(key, e) };

        if offset == 0
            && let Some((key, e, _)) = busy().filter(|(_, _, (s, end))| *s <= now && now < *end).max_by_key(|(_, _, (s, _))| *s)
        {
            return Some(pick(key, e, true));
        }
        let later = busy().filter(|(_, _, (s, _))| offset > 0 || *s > now).min_by_key(|(_, _, (s, _))| *s);
        if let Some((key, e, _)) = later {
            return Some(pick(key, e, false));
        }
    }
    None
}

/// The agenda for `days` days from `from` (today when None). "next" is only
/// looked for from today on, within the days listed.
pub fn build(cache: &EventCache, config: &Config, now: DateTime<Local>, from: Option<NaiveDate>, days: u32, cache_age: Option<u64>) -> Agenda {
    let today = now.date_naive();
    let from = from.unwrap_or(today);
    let minute = (now.hour() * 60 + now.minute()) as u16;
    let days = days.max(1);
    let display = &config.display;
    Agenda {
        generated: now.to_rfc3339(),
        from,
        cache_age,
        today,
        now: minute,
        twelve_hour: display.twelve_hour(),
        sunday_first: display.sunday_first(),
        week_numbers: display.week_numbers,
        working_hours: display.working_minutes(),
        sources: vec![
            SourceInfo { key: "google", label: display.google_label().to_string(), configured: config.google.is_some() },
            SourceInfo { key: "icloud", label: display.icloud_label().to_string(), configured: config.icloud.is_some() },
        ],
        days: (0..days)
            .map(|offset| {
                let date = from + Duration::days(offset as i64);
                Day {
                    date,
                    google: cache.google.get(date).iter().map(|e| item("google", e)).collect(),
                    icloud: cache.icloud.get(date).iter().map(|e| item("icloud", e)).collect(),
                }
            })
            .collect(),
        next: {
            // The days listed that are today or later
            let last = from + Duration::days(days as i64 - 1);
            if last < today { None } else { find_next(cache, today, minute, ((last - today).num_days() + 1) as u32) }
        },
    }
}

/// Print the agenda for `days` days from `from` (today when None)
pub fn print(from: Option<NaiveDate>, days: u32) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let mut cache = EventCache::new();
    let loaded = cache.load_from_disk();
    let cache_age = EventCache::cache_path()
        .filter(|_| loaded)
        .and_then(|p| std::fs::metadata(p).ok()?.modified().ok()?.elapsed().ok())
        .map(|age| age.as_secs());
    let agenda = build(&cache, &config, Local::now(), from, days, cache_age);
    println!("{}", serde_json::to_string(&agenda)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::When;
    use chrono::TimeZone;

    fn event(title: &str, date: NaiveDate, when: When) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "primary".into(), event_id: title.into(), calendar_name: None },
            title: title.into(),
            when,
            date,
            spans_days: false,
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees: vec![],
        }
    }

    fn timed(label: &str, end: &str) -> When {
        When::parse_label(label).ending(end)
    }

    fn at(date: NaiveDate, h: u32, m: u32) -> DateTime<Local> {
        Local.from_local_datetime(&date.and_hms_opt(h, m, 0).unwrap()).unwrap()
    }

    fn cache_with(google: Vec<DisplayEvent>, icloud: Vec<DisplayEvent>) -> EventCache {
        let mut cache = EventCache::new();
        if let Some(month) = google.first().map(|e| e.date) {
            cache.google.store(google, month);
        }
        if let Some(month) = icloud.first().map(|e| e.date) {
            cache.icloud.store(icloud, month);
        }
        cache
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
    }

    #[test]
    fn next_is_the_first_busy_event_still_to_start() {
        let mut declined = event("Vendor demo", day(), timed("14:00", "15:00"));
        declined.accepted = false;
        let mut free = event("Focus", day(), timed("14:30", "15:00"));
        free.is_free = true;
        let cache = cache_with(
            vec![event("Standup", day(), timed("09:30", "09:45")), declined, free, event("Review", day(), timed("16:00", "17:00"))],
            vec![event("Dentist", day(), timed("15:00", "15:30"))],
        );
        let next = build(&cache, &Config::default(), at(day(), 13, 0), None, 2, None).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.source, next.current), ("Dentist", "icloud", false));
    }

    #[test]
    fn an_event_under_way_wins_over_the_next_one() {
        let cache = cache_with(
            vec![event("Offsite", day(), timed("09:00", "17:00")), event("Review", day(), timed("13:00", "14:00"))],
            vec![],
        );
        let next = build(&cache, &Config::default(), at(day(), 13, 10), None, 1, None).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.current), ("Review", true));
        let next = build(&cache, &Config::default(), at(day(), 14, 0), None, 1, None).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.current), ("Offsite", true));
    }

    #[test]
    fn next_moves_on_to_tomorrow_but_skips_all_day_events() {
        let tomorrow = day().succ_opt().unwrap();
        let cache = cache_with(
            vec![event("Leave", tomorrow, When::AllDay), event("Brunch", tomorrow, timed("11:00", "13:00"))],
            vec![],
        );
        let agenda = build(&cache, &Config::default(), at(day(), 20, 0), None, 2, None);
        let next = agenda.next.unwrap();
        assert_eq!((next.item.title.as_str(), next.date), ("Brunch", tomorrow));
        assert!(build(&cache, &Config::default(), at(day(), 20, 0), None, 1, None).next.is_none());
    }

    #[test]
    fn zoom_links_join_through_the_app() {
        let mut zoom = event("Review", day(), timed("10:00", "11:00"));
        zoom.meeting_url = Some("https://dext.zoom.us/j/123?pwd=abc".into());
        let mut meet = event("Sync", day(), timed("12:00", "12:30"));
        meet.meeting_url = Some("https://meet.google.com/abc-defg-hij".into());
        let cache = cache_with(vec![zoom, meet], vec![]);
        let items = &build(&cache, &Config::default(), at(day(), 8, 0), None, 1, None).days[0].google;
        assert_eq!(items[0].join_url.as_deref(), Some("zoommtg://dext.zoom.us/join?action=join&confno=123&pwd=abc"));
        assert_eq!(items[1].join_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
    }

    #[test]
    fn days_keep_the_sources_apart_with_stable_ids() {
        let cache = cache_with(vec![event("Standup", day(), timed("09:30", "09:45"))], vec![event("Gym", day(), When::AllDay)]);
        let agenda = build(&cache, &Config::default(), at(day(), 8, 0), None, 3, None);
        assert_eq!(agenda.days.len(), 3);
        let today = &agenda.days[0];
        assert_eq!(today.google[0].id, "google:primary:Standup");
        assert_eq!((today.google[0].start, today.google[0].end, today.google[0].busy), (Some(570), Some(585), true));
        assert!(today.icloud[0].all_day && !today.icloud[0].busy);
        assert_eq!(agenda.now, 480);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use crate::cache::When;
    use chrono::TimeZone;

    #[test]
    fn from_lists_other_days_and_next_only_looks_ahead() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let last_week = day - Duration::days(7);
        let mut cache = EventCache::new();
        let event = |title: &str, date| DisplayEvent {
            id: EventId::Google { calendar_id: "primary".into(), event_id: title.into(), calendar_name: Some("Work".into()) },
            title: title.into(),
            when: When::parse_label("10:00").ending("11:00"),
            date,
            spans_days: false,
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: Some("notes".into()),
            location: None,
            attendees: vec![crate::cache::DisplayAttendee { name: None, email: "a@b.c".into(), status: AttendeeStatus::Tentative }],
        };
        cache.google.store(vec![event("Old", last_week), event("Soon", day)], day);
        let now = Local.from_local_datetime(&day.and_hms_opt(8, 0, 0).unwrap()).unwrap();

        let past = build(&cache, &Config::default(), now, Some(last_week), 1, None);
        assert_eq!(past.days[0].google[0].title, "Old");
        assert_eq!(past.days[0].google[0].attendees[0].status, "tentative");
        assert_eq!(past.days[0].google[0].calendar_name.as_deref(), Some("Work"));
        assert_eq!(past.days[0].google[0].join_url, None);
        assert!(past.next.is_none());

        let week = build(&cache, &Config::default(), now, Some(last_week), 8, None);
        assert_eq!(week.next.unwrap().item.title, "Soon");
    }
}
