use crate::google::CalendarEvent;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};

pub struct EventCache {
    by_date: HashMap<NaiveDate, Vec<CalendarEvent>>,
    fetched_months: HashSet<(i32, u32)>, // (year, month)
    last_fetch: Option<DateTime<Utc>>,
}

impl EventCache {
    pub fn new() -> Self {
        Self {
            by_date: HashMap::new(),
            fetched_months: HashSet::new(),
            last_fetch: None,
        }
    }

    /// Check if we have events cached for this month
    pub fn has_range(&self, start: NaiveDate, _end: NaiveDate) -> bool {
        self.fetched_months.contains(&(start.year(), start.month()))
    }

    /// Store events, indexed by date (appends to existing cache)
    pub fn store(&mut self, events: Vec<CalendarEvent>, start: NaiveDate, _end: NaiveDate) {
        for event in events {
            if let Some(date) = event.start_date() {
                self.by_date
                    .entry(date)
                    .or_insert_with(Vec::new)
                    .push(event);
            }
        }

        self.fetched_months.insert((start.year(), start.month()));
        self.last_fetch = Some(Utc::now());
    }

    /// Get events for a specific date
    pub fn get(&self, date: NaiveDate) -> &[CalendarEvent] {
        self.by_date
            .get(&date)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Check if a date has any events
    pub fn has_events(&self, date: NaiveDate) -> bool {
        self.by_date
            .get(&date)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Clear all cached data
    pub fn clear(&mut self) {
        self.by_date.clear();
        self.fetched_months.clear();
        self.last_fetch = None;
    }
}

impl Default for EventCache {
    fn default() -> Self {
        Self::new()
    }
}
