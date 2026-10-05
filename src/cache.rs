use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

/// Attendee information for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayAttendee {
    pub name: Option<String>,  // Display name if available
    pub email: String,
    pub status: AttendeeStatus,
}

/// Attendee response status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AttendeeStatus {
    Accepted,
    Declined,
    Tentative,
    NeedsAction,
    Organizer,
}

impl AttendeeStatus {
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
        #[serde(default)]
        href: Option<String>,
    },
}

impl EventId {
    /// Stable identity of the underlying event, ignoring display-only fields
    fn identity(&self) -> (&str, &str) {
        match self {
            EventId::Google { calendar_id, event_id, .. } => (calendar_id, event_id),
            EventId::ICloud { calendar_url, event_uid, .. } => (calendar_url, event_uid),
        }
    }
}

/// Unified event representation for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayEvent {
    pub id: EventId,
    pub title: String,
    pub time_str: String,
    pub end_time_str: Option<String>,
    pub date: NaiveDate,
    pub accepted: bool, // true if accepted or organizer, false if declined/tentative/needs-action
    pub is_organizer: bool, // true if the user created/organizes this event
    #[serde(default)] // backwards compat with old cache
    pub is_free: bool, // true if event is marked as "free" (doesn't block time)
    pub meeting_url: Option<String>, // Zoom, Meet, Teams link if available
    pub description: Option<String>,
    pub location: Option<String>,
    pub attendees: Vec<DisplayAttendee>,
}

/// Bump whenever the on-disk shape or meaning changes; mismatched caches are
/// discarded (the cache is disposable, it's refetched on startup anyway).
/// v2: dedupe fix + expanded recurrences — v1 files can hold thousands of duplicates.
const CACHE_VERSION: u32 = 2;

/// Serializable cache format for disk persistence
#[derive(Serialize, Deserialize)]
struct DiskCache {
    #[serde(default)]
    version: u32,
    google: HashMap<NaiveDate, Vec<DisplayEvent>>,
    icloud: HashMap<NaiveDate, Vec<DisplayEvent>>,
}

/// Borrowing twin of DiskCache, so saving doesn't clone the whole map
#[derive(Serialize)]
struct DiskCacheRef<'a> {
    version: u32,
    google: &'a HashMap<NaiveDate, Vec<DisplayEvent>>,
    icloud: &'a HashMap<NaiveDate, Vec<DisplayEvent>>,
}

/// Source-specific event cache
pub struct SourceCache {
    by_date: HashMap<NaiveDate, Vec<DisplayEvent>>,
    fetched_months: HashSet<(i32, u32)>,
}

impl SourceCache {
    pub fn new() -> Self {
        Self {
            by_date: HashMap::new(),
            fetched_months: HashSet::new(),
        }
    }

    pub fn has_month(&self, date: NaiveDate) -> bool {
        self.fetched_months.contains(&(date.year(), date.month()))
    }

    /// Replace the cached data for a fetched month.
    ///
    /// A fetch can return events dated *outside* the month (a multi-day event
    /// that started earlier), so besides clearing the month we also drop any
    /// cached copy of an incoming event wherever it lives — otherwise those
    /// copies pile up on every refresh.
    pub fn store(&mut self, events: Vec<DisplayEvent>, month_date: NaiveDate) {
        let year = month_date.year();
        let month = month_date.month();
        // Identity of one occurrence: event id + date + start time (EventKit ids
        // are synthesized from the title, so two same-titled events need the time)
        let incoming: HashSet<(&str, &str, NaiveDate, &str)> = events
            .iter()
            .map(|e| { let (a, b) = e.id.identity(); (a, b, e.date, e.time_str.as_str()) })
            .collect();
        self.by_date.retain(|date, day| {
            if date.year() == year && date.month() == month {
                return false;
            }
            day.retain(|e| {
                let (a, b) = e.id.identity();
                !incoming.contains(&(a, b, *date, e.time_str.as_str()))
            });
            !day.is_empty()
        });
        drop(incoming);

        let mut seen: HashSet<(String, String, NaiveDate, String)> = HashSet::new();
        for event in events {
            let (a, b) = event.id.identity();
            if !seen.insert((a.to_string(), b.to_string(), event.date, event.time_str.clone())) {
                continue; // the same instance listed twice in one response
            }
            self.by_date.entry(event.date).or_default().push(event);
        }
        // All-day first, then by start time; stable, so same-time order is kept
        for day in self.by_date.values_mut() {
            day.sort_by(|a, b| {
                (a.time_str != "All day", &a.time_str).cmp(&(b.time_str != "All day", &b.time_str))
            });
        }
        self.fetched_months.insert((year, month));
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

    pub fn clear(&mut self) {
        self.by_date.clear();
        self.fetched_months.clear();
    }

    /// Get raw data for serialization
    pub fn raw_data(&self) -> &HashMap<NaiveDate, Vec<DisplayEvent>> {
        &self.by_date
    }

    /// Load from raw data (for cache restore)
    pub fn load_from(&mut self, data: HashMap<NaiveDate, Vec<DisplayEvent>>) {
        self.by_date = data;
        // Don't mark months as fetched - we want to refresh from network
    }
}

impl Default for SourceCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Combined event cache for all sources
pub struct EventCache {
    pub google: SourceCache,
    pub icloud: SourceCache,
}

impl EventCache {
    pub fn new() -> Self {
        Self {
            google: SourceCache::new(),
            icloud: SourceCache::new(),
        }
    }

    /// Check if any source has events on this date
    pub fn has_events(&self, date: NaiveDate) -> bool {
        self.google.has_events(date) || self.icloud.has_events(date)
    }

    /// Clear all caches
    pub fn clear(&mut self) {
        self.google.clear();
        self.icloud.clear();
    }

    /// Get cache file path
    fn cache_path() -> Option<PathBuf> {
        dirs::cache_dir().map(|p| p.join("calendarchy").join("events.json"))
    }

    /// Save cache to disk
    pub fn save_to_disk(&self) {
        let Some(path) = Self::cache_path() else { return };

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let cache = DiskCacheRef {
            version: CACHE_VERSION,
            google: self.google.raw_data(),
            icloud: self.icloud.raw_data(),
        };

        // Write-then-rename so a concurrent reader (or the --refresh timer
        // racing an open TUI) never sees a half-written file
        if let Ok(json) = serde_json::to_vec(&cache) {
            let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
            if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, &path).is_err() {
                let _ = fs::remove_file(&tmp);
            }
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

        self.google.load_from(cache.google);
        self.icloud.load_from(cache.icloud);
        true
    }
}

impl Default for EventCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(title: &str, date: NaiveDate, time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: title.to_string(),
            time_str: time.to_string(),
            end_time_str: None,
            date,
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees: vec![],
        }
    }

    #[test]
    fn test_source_cache_store_and_get() {
        let mut cache = SourceCache::new();
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
    fn test_source_cache_has_month() {
        let mut cache = SourceCache::new();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        assert!(!cache.has_month(month_date));

        cache.store(vec![], month_date);

        assert!(cache.has_month(month_date));
        assert!(!cache.has_month(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap()));
    }

    #[test]
    fn test_source_cache_store_replaces_month_data() {
        let mut cache = SourceCache::new();
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
        let mut cache = SourceCache::new();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let empty_date = NaiveDate::from_ymd_opt(2026, 1, 16).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        cache.store(vec![make_event("Event", date, "10:00")], month_date);

        assert!(cache.has_events(date));
        assert!(!cache.has_events(empty_date));
    }

    #[test]
    fn test_source_cache_clear() {
        let mut cache = SourceCache::new();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        cache.store(vec![make_event("Event", date, "10:00")], month_date);
        assert!(cache.has_month(month_date));
        assert!(cache.has_events(date));

        cache.clear();
        assert!(!cache.has_month(month_date));
        assert!(!cache.has_events(date));
    }

    #[test]
    fn test_source_cache_load_from_does_not_mark_fetched() {
        let mut cache = SourceCache::new();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        let mut data = HashMap::new();
        data.insert(date, vec![make_event("Cached Event", date, "10:00")]);

        cache.load_from(data);

        // Data should be there
        assert_eq!(cache.get(date).len(), 1);
        // But month should NOT be marked as fetched (allows refresh)
        assert!(!cache.has_month(month_date));
    }

    #[test]
    fn test_event_cache_has_events_either_source() {
        let mut cache = EventCache::new();
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let month_date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        assert!(!cache.has_events(date));

        cache.google.store(vec![make_event("Google Event", date, "10:00")], month_date);
        assert!(cache.has_events(date));

        cache.google.clear();
        assert!(!cache.has_events(date));

        cache.icloud.store(vec![make_event("iCloud Event", date, "11:00")], month_date);
        assert!(cache.has_events(date));
    }

    #[test]
    fn test_display_event_serialization() {
        let event = make_event("Test Meeting", NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), "14:30");

        let json = serde_json::to_string(&event).unwrap();
        let parsed: DisplayEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.title, "Test Meeting");
        assert_eq!(parsed.time_str, "14:30");
        assert!(parsed.accepted);
    }

    #[test]
    fn test_store_does_not_duplicate_events_dated_outside_the_month() {
        // A multi-day event that started in August comes back on every
        // September fetch; it must not pile up (the 4,250-copies bug)
        let mut cache = SourceCache::new();
        let aug = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let sep = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        for _ in 0..5 {
            cache.store(vec![make_event("Leave", aug, "All day")], sep);
        }
        assert_eq!(cache.get(aug).len(), 1);
    }

    #[test]
    fn test_store_keeps_other_events_outside_the_month() {
        let mut cache = SourceCache::new();
        let aug = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let mut other = make_event("Other", aug, "10:00");
        other.id = EventId::Google { calendar_id: "test".into(), event_id: "other".into(), calendar_name: None };
        cache.store(vec![other], NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
        cache.store(vec![make_event("Leave", aug, "All day")], NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        assert_eq!(cache.get(aug).len(), 2);
    }

    #[test]
    fn test_store_dedupes_within_one_response_and_sorts() {
        let mut cache = SourceCache::new();
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
    fn test_old_cache_versions_are_discarded() {
        let v1 = r#"{"google":{},"icloud":{}}"#;
        let parsed: DiskCache = serde_json::from_str(v1).unwrap();
        assert_ne!(parsed.version, CACHE_VERSION);
    }
}
