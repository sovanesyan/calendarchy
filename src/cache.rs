use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

/// Unified event representation for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayEvent {
    pub title: String,
    pub time_str: String,
    pub date: NaiveDate,
    pub accepted: bool, // true if accepted or organizer, false if declined/tentative/needs-action
    pub meeting_url: Option<String>, // Zoom, Meet, Teams link if available
}

/// Serializable cache format for disk persistence
#[derive(Serialize, Deserialize)]
struct DiskCache {
    google: HashMap<NaiveDate, Vec<DisplayEvent>>,
    icloud: HashMap<NaiveDate, Vec<DisplayEvent>>,
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

    pub fn store(&mut self, events: Vec<DisplayEvent>, month_date: NaiveDate) {
        // Clear existing events for this month before storing fresh data
        let year = month_date.year();
        let month = month_date.month();
        self.by_date.retain(|date, _| date.year() != year || date.month() != month);

        for event in events {
            self.by_date
                .entry(event.date)
                .or_insert_with(Vec::new)
                .push(event);
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

        let cache = DiskCache {
            google: self.google.raw_data().clone(),
            icloud: self.icloud.raw_data().clone(),
        };

        if let Ok(json) = serde_json::to_string(&cache) {
            let _ = fs::write(&path, json);
        }
    }

    /// Load cache from disk
    pub fn load_from_disk(&mut self) -> bool {
        let Some(path) = Self::cache_path() else { return false };

        let Ok(json) = fs::read_to_string(&path) else { return false };
        let Ok(cache) = serde_json::from_str::<DiskCache>(&json) else { return false };

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
