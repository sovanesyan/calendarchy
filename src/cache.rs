use crate::app::EventSource;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

/// Attendee information for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayAttendee {
    pub name: Option<String>,  // Display name if available
    pub email: String,
    pub status: AttendeeStatus,
}

/// Attendee response status, in display order (organizer first, declined
/// last). Stored on disk by name, so the order is free to change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttendeeStatus {
    Organizer,
    Accepted,
    Tentative,
    NeedsAction,
    Declined,
}

impl AttendeeStatus {
    /// A reply as either source spells it: Google's "needsAction", iCal's
    /// "NEEDS-ACTION". Anything unknown counts as not replied yet.
    pub fn parse(reply: &str) -> Self {
        let reply: String = reply.chars().filter(|c| !matches!(c, '-' | '_')).collect();
        match reply.to_ascii_lowercase().as_str() {
            "accepted" => Self::Accepted,
            "declined" => Self::Declined,
            "tentative" => Self::Tentative,
            _ => Self::NeedsAction,
        }
    }

    /// Get the display icon for this status
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Accepted | Self::Organizer => "\u{2713}", // ✓
            Self::Declined => "\u{2717}",                   // ✗
            Self::Tentative | Self::NeedsAction => "?",
        }
    }

    /// Get the display color for this status
    pub fn color(&self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Self::Accepted => Color::LightGreen,
            Self::Organizer => Color::LightBlue,
            Self::Declined => Color::LightRed,
            Self::Tentative => Color::LightYellow,
            Self::NeedsAction => Color::DarkGray,
        }
    }
}

/// Event identifier for API actions (accept/decline/delete)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventId {
    /// Google Calendar event (calendar_id, event_id, calendar_name for display)
    Google { calendar_id: String, event_id: String, calendar_name: Option<String> },
    /// iCloud CalDAV event (calendar_url, event_uid, etag for updates, calendar_name for display,
    /// href = the server's resource URL, used for deletes)
    ICloud {
        calendar_url: String,
        event_uid: String,
        etag: Option<String>,
        calendar_name: Option<String>,
        href: Option<String>,
    },
}

impl EventId {
    /// Stable identity of the underlying event, ignoring display-only fields
    pub fn identity(&self) -> (&str, &str) {
        match self {
            EventId::Google { calendar_id, event_id, .. } => (calendar_id, event_id),
            EventId::ICloud { calendar_url, event_uid, .. } => (calendar_url, event_uid),
        }
    }
}

/// Minutes in a day; a `When` end of 1440 means "until midnight"
pub const DAY_MINUTES: u16 = 24 * 60;

/// When an occurrence happens on its date, in local minutes since midnight.
/// Events spanning several days become one occurrence per day: all-day
/// events are `AllDay` on each, timed ones are split at midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum When {
    AllDay,
    /// `end` is exclusive, in (start, 1440]; None when the source gave no end
    Timed { start: u16, end: Option<u16> },
}

impl When {
    pub fn is_all_day(&self) -> bool {
        matches!(self, When::AllDay)
    }

    pub fn start(&self) -> Option<u16> {
        match self {
            When::AllDay => None,
            When::Timed { start, .. } => Some(*start),
        }
    }

    pub fn end(&self) -> Option<u16> {
        match self {
            When::AllDay => None,
            When::Timed { end, .. } => *end,
        }
    }

    /// Ordering within a day: all-day first, then by start
    pub fn sort_key(&self) -> (bool, u16) {
        (!self.is_all_day(), self.start().unwrap_or(0))
    }

    /// "All day" or the start as "HH:MM"
    pub fn label(&self) -> String {
        match self {
            When::AllDay => "All day".to_string(),
            When::Timed { start, .. } => hm(*start),
        }
    }

    /// The end as "HH:MM" ("24:00" for midnight), if known
    pub fn end_label(&self) -> Option<String> {
        self.end().map(hm)
    }

    /// Minutes this occurrence occupies (no end given = one hour)
    pub fn range(&self) -> Option<(u16, u16)> {
        let start = self.start()?;
        Some((start, self.end().unwrap_or(start + 60).min(DAY_MINUTES)))
    }
}

#[cfg(test)]
impl When {
    /// Test helper: "All day" or "HH:MM" (start only)
    pub fn parse_label(label: &str) -> When {
        if label == "All day" {
            return When::AllDay;
        }
        let (h, m) = label.split_once(':').unwrap();
        When::Timed { start: h.parse::<u16>().unwrap() * 60 + m.parse::<u16>().unwrap(), end: None }
    }

    /// Test helper: the same start with an "HH:MM" end
    pub fn ending(self, label: &str) -> When {
        let When::Timed { start, .. } = self else { return self };
        let end = When::parse_label(label).start().unwrap();
        When::Timed { start, end: Some(if end == 0 { DAY_MINUTES } else { end }) }
    }
}

/// Minutes since midnight as a clock time; None for 1440 (end of day)
pub fn clock(minutes: u16) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(minutes as u32 * 60, 0).filter(|_| minutes < DAY_MINUTES)
}

fn hm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Unified event representation for display: one occurrence on one day.
/// On disk it's written through `DiskEventRef`; reading ignores the extra
/// display strings that adds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayEvent {
    pub id: EventId,
    pub title: String,
    pub when: When,
    pub date: NaiveDate,
    /// Part of an event covering several days (leave, a conference).
    /// Defaulted: the first v3 caches were written without it
    #[serde(default)]
    pub spans_days: bool,
    pub accepted: bool, // true if accepted or organizer, false if declined/tentative/needs-action
    pub is_free: bool, // true if event is marked as "free" (doesn't block time)
    pub meeting_url: Option<String>, // Zoom, Meet, Teams link if available
    pub description: Option<String>,
    pub location: Option<String>,
    pub attendees: Vec<DisplayAttendee>,
}

impl DisplayEvent {
    /// Minutes the event blocks on its day; None for all-day, free or
    /// not-accepted events, which don't make you busy
    pub fn busy_range(&self) -> Option<(u16, u16)> {
        if self.is_free || !self.accepted {
            return None;
        }
        self.when.range()
    }

    /// The busy range, if this occurrence can be "Now:" / "Next:". Days of a
    /// multi-day event (leave, a conference) are banners like all-day ones:
    /// they'd hold "Now:" all day and hide the countdown to actual meetings.
    pub fn countdown_range(&self) -> Option<(u16, u16)> {
        if self.spans_days {
            return None;
        }
        self.busy_range()
    }
}

/// How an event is written to disk: all of it, plus the `time_str` /
/// `end_time_str` display strings that external readers of the cache (the
/// TRMNL push job) rely on. Borrows the event, so saving copies nothing.
#[derive(Serialize)]
struct DiskEventRef<'a> {
    #[serde(flatten)]
    event: &'a DisplayEvent,
    time_str: String,
    end_time_str: Option<String>,
}

impl<'a> DiskEventRef<'a> {
    fn new(event: &'a DisplayEvent) -> Self {
        DiskEventRef { event, time_str: event.when.label(), end_time_str: event.when.end_label() }
    }
}

/// When an event happens, before it's split into days
pub enum Span {
    /// Inclusive first and last day
    AllDay { first: NaiveDate, last: NaiveDate },
    Timed { start: chrono::DateTime<chrono::Local>, end: Option<chrono::DateTime<chrono::Local>> },
}

impl Span {
    /// Whether the event covers more than one day (regardless of the window)
    pub fn spans_days(&self) -> bool {
        match self {
            Span::AllDay { first, last } => last > first,
            Span::Timed { start, end: Some(end) } if end > start => last_day(*end) > start.date_naive(),
            Span::Timed { .. } => false,
        }
    }
}

/// The last day a timed event occupies: an end within the first minute of
/// a day (e.g. exactly midnight) belongs to the day before
fn last_day(end: chrono::DateTime<chrono::Local>) -> NaiveDate {
    use chrono::Timelike;
    if end.hour() == 0 && end.minute() == 0 {
        end.date_naive().pred_opt().unwrap_or(end.date_naive())
    } else {
        end.date_naive()
    }
}

/// Split an event's span into per-day occurrences inside `window` (inclusive)
pub fn occurrences(span: Span, window: (NaiveDate, NaiveDate)) -> Vec<(NaiveDate, When)> {
    use chrono::Timelike;
    let minutes = |t: chrono::NaiveTime| (t.hour() * 60 + t.minute()) as u16;
    let days = |first: NaiveDate, last: NaiveDate| {
        let (from, to) = (first.max(window.0), last.min(window.1));
        from.iter_days().take_while(move |d| *d <= to)
    };
    match span {
        Span::AllDay { first, last } => days(first, last.max(first)).map(|d| (d, When::AllDay)).collect(),
        Span::Timed { start, end: None } => {
            let date = start.date_naive();
            if date < window.0 || date > window.1 {
                return vec![];
            }
            vec![(date, When::Timed { start: minutes(start.time()), end: None })]
        }
        Span::Timed { start, end: Some(end) } => {
            let (first, start_min) = (start.date_naive(), minutes(start.time()));
            if end <= start {
                return occurrences(Span::Timed { start, end: None }, window)
                    .into_iter()
                    .map(|(d, _)| (d, When::Timed { start: start_min, end: Some(start_min) }))
                    .collect();
            }
            let last = last_day(end);
            days(first, last.max(first))
                .map(|d| {
                    let s = if d == first { start_min } else { 0 };
                    let e = if d == end.date_naive() { minutes(end.time()) } else { DAY_MINUTES };
                    (d, When::Timed { start: s, end: Some(e.max(s)) })
                })
                .collect()
        }
    }
}

/// Bump whenever the on-disk shape or meaning changes; mismatched caches are
/// discarded (the cache is disposable, it's refetched on startup anyway).
/// v2: dedupe fix + expanded recurrences — v1 files can hold thousands of duplicates.
/// v3: typed `when` per day; multi-day events stored on every day they cover.
const CACHE_VERSION: u32 = 3;

/// Serializable cache format for disk persistence
#[derive(Serialize, Deserialize)]
struct DiskCache {
    #[serde(default)]
    version: u32,
    google: HashMap<NaiveDate, Vec<DisplayEvent>>,
    icloud: HashMap<NaiveDate, Vec<DisplayEvent>>,
}

/// Write side of DiskCache: borrows the caches, writing each event as a
/// `DiskEventRef`
#[derive(Serialize)]
struct DiskCacheRef<'a> {
    version: u32,
    #[serde(serialize_with = "serialize_days")]
    google: &'a HashMap<NaiveDate, Vec<DisplayEvent>>,
    #[serde(serialize_with = "serialize_days")]
    icloud: &'a HashMap<NaiveDate, Vec<DisplayEvent>>,
}

fn serialize_days<S: serde::Serializer>(days: &&HashMap<NaiveDate, Vec<DisplayEvent>>, s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(days.iter().map(|(date, events)| (date, events.iter().map(DiskEventRef::new).collect::<Vec<_>>())))
}

/// Source-specific event cache
#[derive(Default)]
pub struct SourceCache {
    by_date: HashMap<NaiveDate, Vec<DisplayEvent>>,
    /// When each month was last fetched successfully (not restored from disk,
    /// so a cached month still gets refreshed on startup)
    fetched_months: HashMap<(i32, u32), Instant>,
}

impl SourceCache {
    /// When the month containing `date` was last fetched, if ever
    pub fn fetched_at(&self, date: NaiveDate) -> Option<Instant> {
        self.fetched_months.get(&(date.year(), date.month())).copied()
    }

    /// Mark everything stale so it's refetched, but keep showing the data
    pub fn invalidate(&mut self) {
        self.fetched_months.clear();
    }

    /// Replace the cached data for a fetched month. Occurrences dated outside
    /// it are dropped: they belong to (and are refreshed by) their own month's
    /// fetch, so nothing can pile up across refreshes.
    pub fn store(&mut self, events: Vec<DisplayEvent>, month_date: NaiveDate) {
        let year = month_date.year();
        let month = month_date.month();
        let in_month = |d: &NaiveDate| d.year() == year && d.month() == month;
        self.by_date.retain(|date, _| !in_month(date));

        // Identity of one occurrence: event id + date + time (the time keeps
        // apart instances that share an id and a date)
        let mut seen: HashSet<(String, String, NaiveDate, When)> = HashSet::new();
        for event in events {
            if !in_month(&event.date) {
                continue;
            }
            let (a, b) = event.id.identity();
            if !seen.insert((a.to_string(), b.to_string(), event.date, event.when)) {
                continue; // the same instance listed twice in one response
            }
            self.by_date.entry(event.date).or_default().push(event);
        }
        // All-day first, then by start time; stable, so same-time order is kept
        for (date, day) in self.by_date.iter_mut() {
            if in_month(date) {
                day.sort_by_key(|e| e.when.sort_key());
            }
        }
        self.fetched_months.insert((year, month), Instant::now());
    }

    pub fn get(&self, date: NaiveDate) -> &[DisplayEvent] {
        self.by_date
            .get(&date)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn has_events(&self, date: NaiveDate) -> bool {
        self.by_date
            .get(&date)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    pub fn all_events(&self) -> impl Iterator<Item = &DisplayEvent> {
        self.by_date.values().flat_map(|v| v.iter())
    }
}

/// Combined event cache for all sources
#[derive(Default)]
pub struct EventCache {
    pub google: SourceCache,
    pub icloud: SourceCache,
}

impl EventCache {
    pub fn source(&self, source: EventSource) -> &SourceCache {
        match source {
            EventSource::Google => &self.google,
            EventSource::ICloud => &self.icloud,
        }
    }

    pub fn source_mut(&mut self, source: EventSource) -> &mut SourceCache {
        match source {
            EventSource::Google => &mut self.google,
            EventSource::ICloud => &mut self.icloud,
        }
    }

    /// Check if any source has events on this date
    pub fn has_events(&self, date: NaiveDate) -> bool {
        self.google.has_events(date) || self.icloud.has_events(date)
    }

    /// Get cache file path
    pub fn cache_path() -> Option<PathBuf> {
        dirs::cache_dir().map(|p| p.join("calendarchy").join("events.json"))
    }

    /// Save cache to disk (synchronously)
    pub fn save_to_disk(&self) {
        if let Some(bytes) = self.to_disk_bytes() {
            Self::write_disk_bytes(&bytes);
        }
    }

    /// Serialize for disk; cheap enough for the UI thread (the cache is small),
    /// so only the file write needs to move off it
    pub fn to_disk_bytes(&self) -> Option<Vec<u8>> {
        let cache = DiskCacheRef {
            version: CACHE_VERSION,
            google: &self.google.by_date,
            icloud: &self.icloud.by_date,
        };
        serde_json::to_vec(&cache).ok()
    }

    /// Write serialized cache bytes. Write-then-rename so a concurrent reader
    /// (or the --refresh timer racing an open TUI) never sees a half-written file
    pub fn write_disk_bytes(bytes: &[u8]) {
        let Some(path) = Self::cache_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        if fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Load cache from disk
    pub fn load_from_disk(&mut self) -> bool {
        let Some(path) = Self::cache_path() else { return false };

        let Ok(json) = fs::read_to_string(&path) else { return false };
        let Ok(cache) = serde_json::from_str::<DiskCache>(&json) else { return false };
        if cache.version != CACHE_VERSION {
            return false;
        }

        // Months aren't marked fetched, so everything is still refreshed
        self.google.by_date = cache.google;
        self.icloud.by_date = cache.icloud;
        true
    }
}

/// Writes the event cache on its own thread. Saves that pile up while one is
/// being written collapse into the newest, so a burst of results costs one write.
pub struct CacheWriter {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    handle: std::thread::JoinHandle<()>,
}

impl CacheWriter {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let handle = std::thread::spawn(move || {
            while let Ok(mut bytes) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    bytes = newer;
                }
                EventCache::write_disk_bytes(&bytes);
            }
        });
        Self { tx, handle }
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(bytes);
    }

    /// Wait for the last write to land
    pub fn finish(self) {
        drop(self.tx);
        let _ = self.handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(title: &str, date: NaiveDate, time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: title.to_string(),
            when: When::parse_label(time),
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

    #[test]
    fn test_source_cache_store_and_get() {
        let mut cache = SourceCache::default();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        let events = vec![
            make_event("Meeting 1", date, "10:00"),
            make_event("Meeting 2", date, "14:00"),
        ];

        cache.store(events, month_date);

        let retrieved = cache.get(date);
        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].title, "Meeting 1");
        assert_eq!(retrieved[1].title, "Meeting 2");
    }

    #[test]
    fn test_source_cache_records_fetched_months() {
        let mut cache = SourceCache::default();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        assert!(cache.fetched_at(month_date).is_none());

        cache.store(vec![], month_date);

        assert!(cache.fetched_at(month_date).is_some());
        assert!(cache.fetched_at(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()).is_none());
    }

    #[test]
    fn test_source_cache_store_replaces_month_data() {
        let mut cache = SourceCache::default();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        // Store first batch
        cache.store(vec![make_event("Old Event", date, "09:00")], month_date);
        assert_eq!(cache.get(date).len(), 1);
        assert_eq!(cache.get(date)[0].title, "Old Event");

        // Store second batch - should replace
        cache.store(vec![make_event("New Event", date, "10:00")], month_date);
        assert_eq!(cache.get(date).len(), 1);
        assert_eq!(cache.get(date)[0].title, "New Event");
    }

    #[test]
    fn test_source_cache_has_events() {
        let mut cache = SourceCache::default();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let empty_date = NaiveDate::from_ymd_opt(2026, 1, 16).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        cache.store(vec![make_event("Event", date, "10:00")], month_date);

        assert!(cache.has_events(date));
        assert!(!cache.has_events(empty_date));
    }

    #[test]
    fn test_disk_round_trip_keeps_events_and_display_strings() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut cache = EventCache::default();
        let mut meeting = make_event("Meeting", date, "14:30");
        meeting.when = meeting.when.ending("15:15");
        cache.google.store(vec![make_event("Holiday", date, "All day"), meeting], date);
        let bytes = cache.to_disk_bytes().unwrap();

        // What the TRMNL push job reads: google/icloud → date → events with
        // title, accepted, time_str and end_time_str
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["version"], CACHE_VERSION);
        let day = &json["google"]["2026-01-15"];
        assert_eq!(day[0]["time_str"], "All day");
        assert_eq!(day[0]["end_time_str"], serde_json::Value::Null);
        assert_eq!((&day[1]["title"], &day[1]["accepted"]), (&"Meeting".into(), &true.into()));
        assert_eq!((&day[1]["time_str"], &day[1]["end_time_str"]), (&"14:30".into(), &"15:15".into()));
        assert!(json["icloud"].as_object().unwrap().is_empty());

        // Reading it back ignores the display strings; months aren't marked
        // fetched, so a restored cache is still refreshed
        let parsed: DiskCache = serde_json::from_slice(&bytes).unwrap();
        let mut restored = EventCache::default();
        restored.google.by_date = parsed.google;
        assert_eq!(restored.google.get(date).len(), 2);
        assert_eq!(restored.google.get(date)[1].when, When::parse_label("14:30").ending("15:15"));
        assert!(restored.google.fetched_at(date).is_none());
    }

    #[test]
    fn test_event_cache_has_events_either_source() {
        let mut cache = EventCache::default();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        assert!(!cache.has_events(date));

        cache.google.store(vec![make_event("Google Event", date, "10:00")], month_date);
        assert!(cache.has_events(date));

        cache.google.store(vec![], month_date);
        assert!(!cache.has_events(date));

        cache.icloud.store(vec![make_event("iCloud Event", date, "11:00")], month_date);
        assert!(cache.has_events(date));
    }

    #[test]
    fn test_display_event_serialization() {
        let event = make_event("Test Meeting", NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), "14:30");

        let json = serde_json::to_string(&DiskEventRef::new(&event)).unwrap();
        let parsed: DisplayEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.title, "Test Meeting");
        assert_eq!(parsed.when, When::parse_label("14:30"));
        // External readers (TRMNL push) still get the display strings
        assert!(json.contains("\"time_str\":\"14:30\""), "{json}");
        assert!(parsed.accepted);

        // Caches written before is_organizer was dropped still load
        let old = json.replacen("\"accepted\":true", "\"accepted\":true,\"is_organizer\":true", 1);
        assert_ne!(old, json);
        assert_eq!(serde_json::from_str::<DisplayEvent>(&old).unwrap().title, "Test Meeting");
    }

    #[test]
    fn test_store_keeps_only_the_fetched_month() {
        // A multi-day event that started in August comes back on every
        // September fetch; only its September days belong to that fetch
        let mut cache = SourceCache::default();
        let aug = NaiveDate::from_ymd_opt(2026, 8, 31).unwrap();
        let sep = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        for _ in 0..5 {
            cache.store(vec![make_event("Leave", aug, "All day"), make_event("Leave", sep, "All day")], sep);
        }
        assert!(cache.get(aug).is_empty());
        assert_eq!(cache.get(sep).len(), 1);
    }

    #[test]
    fn test_store_leaves_other_months_alone() {
        let mut cache = SourceCache::default();
        let aug = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        cache.store(vec![make_event("August", aug, "10:00")], aug);
        cache.store(vec![], NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        assert_eq!(cache.get(aug).len(), 1);
    }

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn oct(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    const OCTOBER: (NaiveDate, NaiveDate) = (
        NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
    );

    #[test]
    fn test_multi_day_all_day_event_covers_every_day() {
        let span = Span::AllDay { first: oct(5), last: oct(9) };
        let days: Vec<_> = occurrences(span, OCTOBER).into_iter().map(|(d, _)| d).collect();
        assert_eq!(days, [oct(5), oct(6), oct(7), oct(8), oct(9)]);
    }

    #[test]
    fn test_occurrences_are_clipped_to_the_window() {
        let span = Span::AllDay { first: NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(), last: oct(2) };
        let days: Vec<_> = occurrences(span, OCTOBER).into_iter().map(|(d, _)| d).collect();
        assert_eq!(days, [oct(1), oct(2)]);
        // A year-long leave yields at most a month of occurrences
        let long = Span::AllDay { first: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), last: NaiveDate::from_ymd_opt(2026, 12, 31).unwrap() };
        assert_eq!(occurrences(long, OCTOBER).len(), 31);
    }

    #[test]
    fn test_timed_event_across_midnight_splits_into_two_days() {
        let span = Span::Timed { start: local(2026, 10, 9, 22, 0), end: Some(local(2026, 10, 10, 2, 30)) };
        assert_eq!(
            occurrences(span, OCTOBER),
            [(oct(9), When::Timed { start: 22 * 60, end: Some(DAY_MINUTES) }), (oct(10), When::Timed { start: 0, end: Some(150) })]
        );
    }

    #[test]
    fn test_timed_event_ending_at_midnight_stays_on_one_day() {
        let span = Span::Timed { start: local(2026, 10, 9, 20, 0), end: Some(local(2026, 10, 10, 0, 0)) };
        assert_eq!(occurrences(span, OCTOBER), [(oct(9), When::Timed { start: 20 * 60, end: Some(DAY_MINUTES) })]);
    }

    #[test]
    fn test_multi_day_timed_event_fills_the_middle_days() {
        let span = Span::Timed { start: local(2026, 10, 5, 9, 0), end: Some(local(2026, 10, 7, 12, 0)) };
        assert_eq!(
            occurrences(span, OCTOBER),
            [
                (oct(5), When::Timed { start: 540, end: Some(DAY_MINUTES) }),
                (oct(6), When::Timed { start: 0, end: Some(DAY_MINUTES) }),
                (oct(7), When::Timed { start: 0, end: Some(720) }),
            ]
        );
    }

    #[test]
    fn test_when_labels() {
        assert_eq!(When::AllDay.label(), "All day");
        let w = When::Timed { start: 22 * 60, end: Some(DAY_MINUTES) };
        assert_eq!(w.label(), "22:00");
        assert_eq!(w.end_label().as_deref(), Some("24:00"));
        assert_eq!(When::Timed { start: 600, end: None }.range(), Some((600, 660)));
        assert_eq!(When::Timed { start: 23 * 60 + 30, end: None }.range(), Some((1410, DAY_MINUTES)));
    }

    #[test]
    fn test_store_dedupes_within_one_response_and_sorts() {
        let mut cache = SourceCache::default();
        let d = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap();
        let mut late = make_event("Late", d, "15:00");
        late.id = EventId::Google { calendar_id: "test".into(), event_id: "late".into(), calendar_name: None };
        let early = make_event("Early", d, "09:00");
        let mut allday = make_event("All", d, "All day");
        allday.id = EventId::Google { calendar_id: "test".into(), event_id: "all".into(), calendar_name: None };
        cache.store(vec![late, early.clone(), early, allday], NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        let titles: Vec<_> = cache.get(d).iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["All", "Early", "Late"]);
    }

    #[test]
    fn test_attendee_status_parse() {
        assert_eq!(AttendeeStatus::parse("accepted"), AttendeeStatus::Accepted);
        assert_eq!(AttendeeStatus::parse("DECLINED"), AttendeeStatus::Declined);
        assert_eq!(AttendeeStatus::parse("Tentative"), AttendeeStatus::Tentative);
        assert_eq!(AttendeeStatus::parse("needsAction"), AttendeeStatus::NeedsAction);
        assert_eq!(AttendeeStatus::parse("NEEDS-ACTION"), AttendeeStatus::NeedsAction);
        assert_eq!(AttendeeStatus::parse("DELEGATED"), AttendeeStatus::NeedsAction);
        assert_eq!(AttendeeStatus::parse(""), AttendeeStatus::NeedsAction);
    }

    #[test]
    fn test_old_cache_versions_are_discarded() {
        let v1 = r#"{"google":{},"icloud":{}}"#;
        let parsed: DiskCache = serde_json::from_str(v1).unwrap();
        assert_ne!(parsed.version, CACHE_VERSION);
    }

    #[test]
    fn test_end_seconds_after_midnight_stay_on_the_previous_day() {
        use chrono::TimeZone;
        let end = chrono::Local.with_ymd_and_hms(2026, 10, 10, 0, 0, 30).unwrap();
        let span = Span::Timed { start: local(2026, 10, 9, 21, 0), end: Some(end) };
        assert!(!span.spans_days());
        assert_eq!(occurrences(span, OCTOBER), [(oct(9), When::Timed { start: 21 * 60, end: Some(DAY_MINUTES) })]);
    }

    #[test]
    fn test_spans_days() {
        assert!(Span::AllDay { first: oct(5), last: oct(6) }.spans_days());
        assert!(!Span::AllDay { first: oct(5), last: oct(5) }.spans_days());
        assert!(Span::Timed { start: local(2026, 10, 9, 22, 0), end: Some(local(2026, 10, 10, 2, 0)) }.spans_days());
        assert!(!Span::Timed { start: local(2026, 10, 9, 9, 0), end: Some(local(2026, 10, 9, 10, 0)) }.spans_days());
    }
}
