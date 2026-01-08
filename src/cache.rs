use chrono::{Datelike, NaiveDate};
use std::collections::{HashMap, HashSet};

/// Unified event representation for display
#[derive(Debug, Clone)]
pub struct DisplayEvent {
    pub title: String,
    pub time_str: String,
    pub date: NaiveDate,
    pub accepted: bool, // true if accepted or organizer, false if declined/tentative/needs-action
    pub meeting_url: Option<String>, // Zoom, Meet, Teams link if available
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
        for event in events {
            self.by_date
                .entry(event.date)
                .or_insert_with(Vec::new)
                .push(event);
        }
        self.fetched_months.insert((month_date.year(), month_date.month()));
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
}

impl Default for EventCache {
    fn default() -> Self {
        Self::new()
    }
}
