use crate::google::CalendarEvent;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;

pub struct EventCache {
    by_date: HashMap<NaiveDate, Vec<CalendarEvent>>,
    cached_range: Option<(NaiveDate, NaiveDate)>,
    last_fetch: Option<DateTime<Utc>>,
}

impl EventCache {
    pub fn new() -> Self {
        Self {
            by_date: HashMap::new(),
            cached_range: None,
            last_fetch: None,
        }
    }

    /// Check if we have events cached for this range
    pub fn has_range(&self, start: NaiveDate, end: NaiveDate) -> bool {
        if let Some((cached_start, cached_end)) = self.cached_range {
            start >= cached_start && end <= cached_end
        } else {
            false
        }
    }

    /// Store events, indexed by date
    pub fn store(&mut self, events: Vec<CalendarEvent>, start: NaiveDate, end: NaiveDate) {
        self.by_date.clear();

        for event in events {
            if let Some(date) = event.start_date() {
                self.by_date
                    .entry(date)
                    .or_insert_with(Vec::new)
                    .push(event);
            }
        }

        self.cached_range = Some((start, end));
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

    /// Check if cache is stale (older than 5 minutes)
    pub fn is_stale(&self) -> bool {
        self.last_fetch
            .map(|t| Utc::now() - t > chrono::Duration::minutes(5))
            .unwrap_or(true)
    }

    /// Clear all cached data
    pub fn clear(&mut self) {
        self.by_date.clear();
        self.cached_range = None;
        self.last_fetch = None;
    }
}

impl Default for EventCache {
    fn default() -> Self {
        Self::new()
    }
}
