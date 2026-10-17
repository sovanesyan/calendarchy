//! `calendarchy --agenda [--from YYYY-MM-DD] [--days N]`: days' events as
//! JSON, read from the disk cache only (no network, no auth), so it's cheap
//! enough to run every minute. The Omarchy shell plugins (`shell/`) draw it;
//! `--refresh` keeps the cache fresh.
//!
//! Times stay typed (minutes since midnight) and "next" is decided here, by
//! `find_next`, the same function the TUI's status bar uses, so the plugins'
//! QML only lays things out.
//!
//! The JSON (the contract with `shell/`; keep the QML in step):
//!
//! ```text
//! {
//!   "today": "2026-10-16",           // local date
//!   "now": 480,                      // minutes since midnight
//!   "twelveHour": false, "sundayFirst": false, "weekNumbers": false,
//!   "workingHours": [540, 1080],     // [start, end] minutes, or null
//!   "sources": [{ "key": "google", "label": "Work", "configured": true },
//!               { "key": "icloud", "label": "Personal", "configured": false }],
//!   "days": [{ "date": "2026-10-16", "google": [Event], "icloud": [Event] }],
//!                                    // --days days from --from (default today, 2)
//!   "next": { "date": "2026-10-16", "current": false, ...Event } or null,
//!                                    // always from now, NEXT_DAYS days ahead,
//!                                    // whatever --from / --days say
//!   "keys": { "day": { "next_day": ["l", "right"], ... },
//!             "event": { "join": ["J"], ... } }
//! }
//!
//! Event: { "id", "title", "allDay", "start", "end" (minutes or null),
//!          "spansDays", "accepted", "free",
//!          "busy": [startMin, endMin] or null (timed, accepted, not free;
//!                  no end = one hour),
//!          "meetingUrl", "joinUrl", "location", "description", "calendarName",
//!          "attendees": [{ "name", "email", "status" }] }
//! ```
//!
//! `keys` are the bindings in effect after the config's `"keys"` overrides,
//! per mode ("day", "event"): action name (as in config) → key specs in the
//! config's own syntax ("J", "ctrl+d", "enter", "esc", "tab", "shift+tab",
//! "left", "space", "f5"). Precedence is already applied, so a key appears
//! under at most one action per mode, and actions left without keys are
//! omitted. Every action is listed, including ones the window hands to the
//! TUI (search, setup, toggle_logs, connect_*).

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike};
use serde::Serialize;

use crate::app::NavigationMode;
use crate::cache::{AttendeeStatus, DisplayEvent, EventCache, EventId, SourceCache};
use crate::config::Config;
use crate::keymap::Keymap;

/// How many days "next" looks at, today included: the TUI's status bar and
/// the agenda's `next` alike
pub const NEXT_DAYS: u32 = 8;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Agenda {
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
    /// The event running now, or else the next one (see `find_next`)
    pub next: Option<Next>,
    /// Effective key bindings: mode → action name → key specs
    pub keys: BTreeMap<&'static str, BTreeMap<String, Vec<String>>>,
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
    pub free: bool,
    /// The minutes it blocks, [start, end]: timed, accepted and not marked
    /// free (no end = one hour); None when it doesn't make you busy
    pub busy: Option<(u16, u16)>,
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
        free: e.is_free,
        busy: e.busy_range(),
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

/// The event "Now:" / "Next:" is about
pub struct Upcoming<'a> {
    /// "google" or "icloud"
    pub source: &'static str,
    pub date: NaiveDate,
    /// Under way at `now` rather than upcoming
    pub current: bool,
    pub event: &'a DisplayEvent,
}

/// The one rule for "Now:" / "Next:", shared by the TUI's status bar and the
/// agenda. Only events with a `countdown_range` count: busy (timed, accepted,
/// not free) and not a day of a multi-day event. Of those, the one under way
/// at `now` (the latest-starting, if several), or else the earliest still to
/// start across both sources: today first, then the following days, `days`
/// days in all.
pub fn find_next(cache: &EventCache, today: NaiveDate, now: u16, days: u32) -> Option<Upcoming<'_>> {
    let sources: [(&'static str, &SourceCache); 2] = [("google", &cache.google), ("icloud", &cache.icloud)];
    for offset in 0..days {
        let date = today + Duration::days(offset as i64);
        let candidates = || {
            sources.iter().flat_map(move |(key, source)| {
                source.get(date).iter().filter_map(move |e| Some((*key, e, e.countdown_range()?)))
            })
        };
        let pick = |source: &'static str, event, current| Upcoming { source, date, current, event };

        if offset == 0
            && let Some((key, e, _)) = candidates().filter(|(_, _, (s, end))| *s <= now && now < *end).max_by_key(|(_, _, (s, _))| *s)
        {
            return Some(pick(key, e, true));
        }
        let later = candidates().filter(|(_, _, (s, _))| offset > 0 || *s > now).min_by_key(|(_, _, (s, _))| *s);
        if let Some((key, e, _)) = later {
            return Some(pick(key, e, false));
        }
    }
    None
}

/// The agenda for `days` days from `from` (today when None). "next" doesn't
/// depend on them: it's always looked for from now, NEXT_DAYS days ahead.
pub fn build(cache: &EventCache, config: &Config, now: DateTime<Local>, from: Option<NaiveDate>, days: u32) -> Agenda {
    let today = now.date_naive();
    let from = from.unwrap_or(today);
    let minute = (now.hour() * 60 + now.minute()) as u16;
    let days = days.max(1);
    let display = &config.display;
    let (keymap, _) = Keymap::with_overrides(&config.keys);
    Agenda {
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
        next: find_next(cache, today, minute, NEXT_DAYS).map(|u| Next { date: u.date, current: u.current, item: item(u.source, u.event) }),
        keys: BTreeMap::from([
            ("day", keymap.effective(NavigationMode::Day)),
            ("event", keymap.effective(NavigationMode::Event)),
        ]),
    }
}

/// Print the agenda for `days` days from `from` (today when None)
pub fn print(from: Option<NaiveDate>, days: u32) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().unwrap_or_default();
    let mut cache = EventCache::default();
    cache.load_from_disk();
    let agenda = build(&cache, &config, Local::now(), from, days);
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
        let mut cache = EventCache::default();
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
        let next = build(&cache, &Config::default(), at(day(), 13, 0), None, 2).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.item.id.as_str(), next.current), ("Dentist", "icloud:primary:Dentist", false));
    }

    #[test]
    fn an_event_under_way_wins_over_the_next_one() {
        let cache = cache_with(
            vec![event("Offsite", day(), timed("09:00", "17:00")), event("Review", day(), timed("13:00", "14:00"))],
            vec![],
        );
        let next = build(&cache, &Config::default(), at(day(), 13, 10), None, 1).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.current), ("Review", true));
        let next = build(&cache, &Config::default(), at(day(), 14, 0), None, 1).next.unwrap();
        assert_eq!((next.item.title.as_str(), next.current), ("Offsite", true));
    }

    #[test]
    fn next_moves_on_to_tomorrow_but_skips_all_day_events() {
        let tomorrow = day().succ_opt().unwrap();
        let cache = cache_with(
            vec![event("Leave", tomorrow, When::AllDay), event("Brunch", tomorrow, timed("11:00", "13:00"))],
            vec![],
        );
        let agenda = build(&cache, &Config::default(), at(day(), 20, 0), None, 1);
        let next = agenda.next.unwrap();
        assert_eq!((next.item.title.as_str(), next.date), ("Brunch", tomorrow), "beyond the days listed");
    }

    #[test]
    fn zoom_links_join_through_the_app() {
        let mut zoom = event("Review", day(), timed("10:00", "11:00"));
        zoom.meeting_url = Some("https://dext.zoom.us/j/123?pwd=abc".into());
        let mut meet = event("Sync", day(), timed("12:00", "12:30"));
        meet.meeting_url = Some("https://meet.google.com/abc-defg-hij".into());
        let cache = cache_with(vec![zoom, meet], vec![]);
        let items = &build(&cache, &Config::default(), at(day(), 8, 0), None, 1).days[0].google;
        assert_eq!(items[0].join_url.as_deref(), Some("zoommtg://dext.zoom.us/join?action=join&confno=123&pwd=abc"));
        assert_eq!(items[1].join_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
    }

    #[test]
    fn days_keep_the_sources_apart_with_stable_ids() {
        let cache = cache_with(vec![event("Standup", day(), timed("09:30", "09:45"))], vec![event("Gym", day(), When::AllDay)]);
        let agenda = build(&cache, &Config::default(), at(day(), 8, 0), None, 3);
        assert_eq!(agenda.days.len(), 3);
        let today = &agenda.days[0];
        assert_eq!(today.google[0].id, "google:primary:Standup");
        assert_eq!((today.google[0].start, today.google[0].end, today.google[0].busy), (Some(570), Some(585), Some((570, 585))));
        assert!(today.icloud[0].all_day && today.icloud[0].busy.is_none());
        assert_eq!(agenda.now, 480);
    }

    #[test]
    fn from_lists_other_days_but_next_is_always_from_now() {
        let last_week = day() - Duration::days(7);
        let mut old = event("Old", last_week, timed("10:00", "11:00"));
        old.id = EventId::Google { calendar_id: "primary".into(), event_id: "Old".into(), calendar_name: Some("Work".into()) };
        old.attendees = vec![crate::cache::DisplayAttendee { name: None, email: "a@b.c".into(), status: AttendeeStatus::Tentative }];
        let cache = cache_with(vec![old, event("Soon", day(), timed("10:00", "11:00"))], vec![]);

        let past = build(&cache, &Config::default(), at(day(), 8, 0), Some(last_week), 1);
        assert_eq!(past.days.len(), 1);
        assert_eq!(past.days[0].google[0].title, "Old");
        assert_eq!(past.days[0].google[0].attendees[0].status, "tentative");
        assert_eq!(past.days[0].google[0].calendar_name.as_deref(), Some("Work"));
        assert_eq!(past.days[0].google[0].join_url, None);
        assert_eq!(past.next.unwrap().item.title, "Soon", "not limited to the days listed");
    }

    #[test]
    fn next_looks_a_week_ahead() {
        let in_a_week = day() + Duration::days(NEXT_DAYS as i64 - 1);
        let cache = cache_with(vec![event("Later", in_a_week, timed("09:00", "10:00"))], vec![]);
        assert_eq!(build(&cache, &Config::default(), at(day(), 8, 0), None, 1).next.unwrap().date, in_a_week);
        let cache = cache_with(vec![event("Too far", in_a_week + Duration::days(1), timed("09:00", "10:00"))], vec![]);
        assert!(build(&cache, &Config::default(), at(day(), 8, 0), None, 1).next.is_none());
    }

    #[test]
    fn the_earlier_source_wins_and_multi_day_events_never_hold_now() {
        let mut conference = event("Conference", day(), timed("00:00", "00:00"));
        conference.spans_days = true;
        let cache = cache_with(
            vec![conference, event("Review", day(), timed("15:00", "16:00"))],
            vec![event("Dentist", day(), timed("11:00", "11:30"))],
        );
        let next = find_next(&cache, day(), 10 * 60, NEXT_DAYS).unwrap();
        assert_eq!((next.event.title.as_str(), next.source, next.current), ("Dentist", "icloud", false));
        // Busy yet not a countdown candidate
        assert_eq!(build(&cache, &Config::default(), at(day(), 10, 0), None, 1).days[0].google[0].busy, Some((0, 1440)));
    }

    #[test]
    fn keys_follow_the_config_overrides() {
        let config: Config = serde_json::from_str(r#"{ "keys": { "join": "o", "next_day": ["ctrl+f", "right"] } }"#).unwrap();
        let agenda = build(&EventCache::default(), &config, at(day(), 8, 0), None, 1);
        assert_eq!(agenda.keys["event"]["join"], ["o"]);
        assert_eq!(agenda.keys["day"]["next_day"], ["ctrl+f", "right"]);
        assert_eq!(agenda.keys["day"]["next_month"], ["L", "ctrl+d"]);
        let json = serde_json::to_value(&agenda).unwrap();
        assert_eq!(json["keys"]["event"]["decline"], serde_json::json!(["d"]));
        assert!(json.get("generated").is_none() && json.get("cacheAge").is_none());
    }
}
