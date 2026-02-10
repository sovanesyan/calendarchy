use crate::auth::{GoogleAuthState, ICloudAuthState};
use crate::cache::{DisplayEvent, EventCache};
use crate::config::Config;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime};

/// Search state for the interactive search modal
pub struct SearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected_index: usize,
    pub scroll_offset: usize,
}

/// Whether a search result matched on title or participant
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchType {
    Title,
    Participant,
}

/// A single search result with its source
pub struct SearchResult {
    pub event: DisplayEvent,
    pub source: EventSource,
    pub match_type: MatchType,
}

/// Navigation mode for two-level navigation in month view
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavigationMode {
    Day,   // Navigate between days with h/j/k/l
    Event, // Navigate between events within selected day with j/k
}

/// Which event source/panel is currently selected
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventSource {
    Google,
    ICloud,
}

/// Pending action awaiting confirmation
#[derive(Debug, Clone)]
pub enum PendingAction {
    AcceptEvent { calendar_id: String, event_id: String },
    DeclineEvent { calendar_id: String, event_id: String },
    DeleteGoogleEvent { calendar_id: String, event_id: String },
    DeleteICloudEvent { calendar_url: String, event_uid: String, etag: Option<String> },
}

/// Application state
pub struct App {
    pub current_date: NaiveDate,
    pub selected_date: NaiveDate,
    pub show_logs: bool,
    pub show_weekends: bool,
    pub events: EventCache,
    pub google_auth: GoogleAuthState,
    pub icloud_auth: ICloudAuthState,
    pub status_message: Option<String>,
    pub status_message_time: Option<std::time::Instant>,
    pub config: Config,
    pub google_needs_fetch: bool,
    pub icloud_needs_fetch: bool,
    pub google_loading: bool,
    pub icloud_loading: bool,
    pub navigation_mode: NavigationMode,
    pub selected_source: EventSource,
    pub selected_event_index: usize,
    pub pending_action: Option<PendingAction>,
    pub search: Option<SearchState>,
}

impl App {
    pub fn new() -> Self {
        let today = Local::now().date_naive();
        let mut events = EventCache::new();
        events.load_from_disk();

        let mut app = Self {
            current_date: today,
            selected_date: today,
            show_logs: false,
            show_weekends: false,
            events,
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status_message: None,
            status_message_time: None,
            config: Config::default(),
            google_needs_fetch: false,
            icloud_needs_fetch: false,
            google_loading: false,
            icloud_loading: false,
            navigation_mode: NavigationMode::Day,
            selected_source: EventSource::Google,
            selected_event_index: 0,
            pending_action: None,
            search: None,
        };

        app.enter_event_mode();
        app
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_time = Some(std::time::Instant::now());
    }

    pub fn clear_expired_status(&mut self) {
        if let Some(time) = self.status_message_time
            && time.elapsed() > std::time::Duration::from_secs(3)
        {
            self.status_message = None;
            self.status_message_time = None;
        }
    }

    pub fn next_day(&mut self) {
        self.selected_date += Duration::days(1);
        self.sync_month_if_needed();
    }

    pub fn prev_day(&mut self) {
        self.selected_date -= Duration::days(1);
        self.sync_month_if_needed();
    }

    fn sync_month_if_needed(&mut self) {
        if self.selected_date.month() != self.current_date.month()
            || self.selected_date.year() != self.current_date.year()
        {
            self.current_date = self.selected_date.with_day(1).unwrap();
            self.google_needs_fetch = true;
            self.icloud_needs_fetch = true;
        }
    }

    pub fn goto_today(&mut self) {
        let today = Local::now().date_naive();
        let month_changed = today.month() != self.current_date.month()
            || today.year() != self.current_date.year();
        self.current_date = today;
        self.selected_date = today;
        if month_changed {
            self.google_needs_fetch = true;
            self.icloud_needs_fetch = true;
        }
    }

    pub fn goto_now(&mut self) {
        self.goto_today();
        self.enter_event_mode();
    }

    pub fn month_range(&self) -> (NaiveDate, NaiveDate) {
        let first = self.current_date.with_day(1).unwrap();
        let last = if self.current_date.month() == 12 {
            NaiveDate::from_ymd_opt(self.current_date.year() + 1, 1, 1)
                .unwrap()
                - Duration::days(1)
        } else {
            NaiveDate::from_ymd_opt(self.current_date.year(), self.current_date.month() + 1, 1)
                .unwrap()
                - Duration::days(1)
        };
        (first, last)
    }

    pub fn get_current_source_events(&self) -> &[DisplayEvent] {
        match self.selected_source {
            EventSource::Google => self.events.google.get(self.selected_date),
            EventSource::ICloud => self.events.icloud.get(self.selected_date),
        }
    }

    pub fn get_selected_event(&self) -> Option<&DisplayEvent> {
        if self.navigation_mode == NavigationMode::Event {
            self.get_current_source_events().get(self.selected_event_index)
        } else {
            None
        }
    }

    pub fn enter_event_mode(&mut self) {
        let google_events = self.events.google.get(self.selected_date);
        let icloud_events = self.events.icloud.get(self.selected_date);

        if google_events.is_empty() && icloud_events.is_empty() {
            return;
        }

        self.navigation_mode = NavigationMode::Event;

        let today = Local::now().date_naive();
        if self.selected_date == today {
            let current_time = Local::now().time();

            if let Some((idx, is_current_or_next)) = find_current_or_next_event(google_events, current_time)
                && is_current_or_next {
                    self.selected_source = EventSource::Google;
                    self.selected_event_index = idx;
                    return;
                }

            if let Some((idx, is_current_or_next)) = find_current_or_next_event(icloud_events, current_time)
                && is_current_or_next {
                    self.selected_source = EventSource::ICloud;
                    self.selected_event_index = idx;
                    return;
                }

            let google_next = find_current_or_next_event(google_events, current_time);
            let icloud_next = find_current_or_next_event(icloud_events, current_time);

            match (google_next, icloud_next) {
                (Some((g_idx, _)), Some((i_idx, _))) => {
                    let g_time = &google_events[g_idx].time_str;
                    let i_time = &icloud_events[i_idx].time_str;
                    if g_time <= i_time {
                        self.selected_source = EventSource::Google;
                        self.selected_event_index = g_idx;
                    } else {
                        self.selected_source = EventSource::ICloud;
                        self.selected_event_index = i_idx;
                    }
                    return;
                }
                (Some((idx, _)), None) => {
                    self.selected_source = EventSource::Google;
                    self.selected_event_index = idx;
                    return;
                }
                (None, Some((idx, _))) => {
                    self.selected_source = EventSource::ICloud;
                    self.selected_event_index = idx;
                    return;
                }
                (None, None) => {}
            }
        }

        if !google_events.is_empty() {
            self.selected_source = EventSource::Google;
            self.selected_event_index = 0;
        } else {
            self.selected_source = EventSource::ICloud;
            self.selected_event_index = 0;
        }
    }

    pub fn exit_event_mode(&mut self) {
        self.navigation_mode = NavigationMode::Day;
        self.selected_source = EventSource::Google;
        self.selected_event_index = 0;
    }

    pub fn next_event(&mut self) {
        let current_events = self.get_current_source_events();

        if self.selected_event_index < current_events.len().saturating_sub(1) {
            self.selected_event_index += 1;
        } else if self.selected_source == EventSource::Google {
            let icloud_events = self.events.icloud.get(self.selected_date);
            if !icloud_events.is_empty() {
                self.selected_source = EventSource::ICloud;
                self.selected_event_index = 0;
            } else {
                self.navigate_to_next_day_with_events();
            }
        } else {
            self.navigate_to_next_day_with_events();
        }
    }

    pub fn prev_event(&mut self) {
        if self.selected_event_index > 0 {
            self.selected_event_index -= 1;
        } else if self.selected_source == EventSource::ICloud {
            let google_events = self.events.google.get(self.selected_date);
            if !google_events.is_empty() {
                self.selected_source = EventSource::Google;
                self.selected_event_index = google_events.len().saturating_sub(1);
            } else {
                self.navigate_to_prev_day_with_events();
            }
        } else {
            self.navigate_to_prev_day_with_events();
        }
    }

    fn navigate_to_next_day_with_events(&mut self) {
        let mut check_date = self.selected_date + Duration::days(1);
        let limit = self.selected_date + Duration::days(90);

        while check_date <= limit {
            if self.events.has_events(check_date) {
                self.selected_date = check_date;
                if check_date.month() != self.current_date.month() || check_date.year() != self.current_date.year() {
                    self.current_date = check_date;
                }
                let google_events = self.events.google.get(check_date);
                if !google_events.is_empty() {
                    self.selected_source = EventSource::Google;
                    self.selected_event_index = 0;
                } else {
                    self.selected_source = EventSource::ICloud;
                    self.selected_event_index = 0;
                }
                return;
            }
            check_date += Duration::days(1);
        }
    }

    fn navigate_to_prev_day_with_events(&mut self) {
        let mut check_date = self.selected_date - Duration::days(1);
        let limit = self.selected_date - Duration::days(90);

        while check_date >= limit {
            if self.events.has_events(check_date) {
                self.selected_date = check_date;
                if check_date.month() != self.current_date.month() || check_date.year() != self.current_date.year() {
                    self.current_date = check_date;
                }
                let icloud_events = self.events.icloud.get(check_date);
                let google_events = self.events.google.get(check_date);
                if !icloud_events.is_empty() {
                    self.selected_source = EventSource::ICloud;
                    self.selected_event_index = icloud_events.len().saturating_sub(1);
                } else {
                    self.selected_source = EventSource::Google;
                    self.selected_event_index = google_events.len().saturating_sub(1);
                }
                return;
            }
            check_date -= Duration::days(1);
        }
    }

    pub fn next_month(&mut self) {
        let (year, month) = if self.current_date.month() == 12 {
            (self.current_date.year() + 1, 1)
        } else {
            (self.current_date.year(), self.current_date.month() + 1)
        };
        self.current_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        self.selected_date = self.current_date;
    }

    pub fn prev_month(&mut self) {
        let (year, month) = if self.current_date.month() == 1 {
            (self.current_date.year() - 1, 12)
        } else {
            (self.current_date.year(), self.current_date.month() - 1)
        };
        self.current_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        self.selected_date = self.current_date;
    }

    pub fn open_search(&mut self) {
        self.search = Some(SearchState {
            query: String::new(),
            results: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
        });
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    pub fn update_search_results(&mut self) {
        let search = match self.search.as_ref() {
            Some(s) => s,
            None => return,
        };

        let query_lower = search.query.to_lowercase();
        let mut results: Vec<SearchResult> = Vec::new();
        let today = Local::now().date_naive();

        if !query_lower.is_empty() {
            for event in self.events.google.all_events() {
                if event.date >= today {
                    if let Some(match_type) = event_match_type(event, &query_lower) {
                        results.push(SearchResult {
                            event: event.clone(),
                            source: EventSource::Google,
                            match_type,
                        });
                    }
                }
            }
            for event in self.events.icloud.all_events() {
                if event.date >= today {
                    if let Some(match_type) = event_match_type(event, &query_lower) {
                        results.push(SearchResult {
                            event: event.clone(),
                            source: EventSource::ICloud,
                            match_type,
                        });
                    }
                }
            }
            results.sort_by(|a, b| {
                let a_title = a.event.title.to_lowercase().contains(&query_lower);
                let b_title = b.event.title.to_lowercase().contains(&query_lower);
                b_title.cmp(&a_title)
                    .then_with(|| a.event.date.cmp(&b.event.date))
                    .then_with(|| a.event.time_str.cmp(&b.event.time_str))
            });
        }

        if let Some(ref mut search) = self.search {
            search.results = results;
            if search.selected_index >= search.results.len() {
                search.selected_index = search.results.len().saturating_sub(1);
            }
            // Clamp scroll offset
            if search.selected_index < search.scroll_offset {
                search.scroll_offset = search.selected_index;
            }
        }
    }

    pub fn select_search_result(&mut self) {
        let (date, source, event_title) = match self.search.as_ref() {
            Some(s) => {
                match s.results.get(s.selected_index) {
                    Some(r) => (r.event.date, r.source, r.event.title.clone()),
                    None => return,
                }
            }
            None => return,
        };

        // Navigate to the date
        let month_changed = date.month() != self.current_date.month()
            || date.year() != self.current_date.year();
        self.selected_date = date;
        if month_changed {
            self.current_date = date.with_day(1).unwrap();
            self.google_needs_fetch = true;
            self.icloud_needs_fetch = true;
        }

        // Enter event mode on the correct source/index
        self.navigation_mode = NavigationMode::Event;
        self.selected_source = source;

        let events = match source {
            EventSource::Google => self.events.google.get(date),
            EventSource::ICloud => self.events.icloud.get(date),
        };
        self.selected_event_index = events.iter()
            .position(|e| e.title == event_title)
            .unwrap_or(0);

        self.close_search();
    }
}

/// Check if an event matches the search query (case-insensitive)
#[cfg(test)]
fn event_matches_query(event: &DisplayEvent, query_lower: &str) -> bool {
    event_match_type(event, query_lower).is_some()
}

/// Determine how an event matches the search query, returning the match type.
/// Title matches take priority over participant matches.
pub fn event_match_type(event: &DisplayEvent, query_lower: &str) -> Option<MatchType> {
    if event.title.to_lowercase().contains(query_lower) {
        return Some(MatchType::Title);
    }
    for attendee in &event.attendees {
        if let Some(ref name) = attendee.name {
            if name.to_lowercase().contains(query_lower) {
                return Some(MatchType::Participant);
            }
        }
        if attendee.email.to_lowercase().contains(query_lower) {
            return Some(MatchType::Participant);
        }
    }
    None
}

/// Find current or next event in a list, returns (index, is_current)
fn find_current_or_next_event(events: &[DisplayEvent], current_time: NaiveTime) -> Option<(usize, bool)> {
    let mut best_current: Option<(usize, NaiveTime)> = None;
    let mut first_next: Option<usize> = None;

    for (i, event) in events.iter().enumerate() {
        if event.time_str == "All day" {
            continue;
        }

        let parts: Vec<&str> = event.time_str.split(':').collect();
        if parts.len() != 2 {
            continue;
        }
        let hour: u32 = match parts[0].parse() {
            Ok(h) => h,
            Err(_) => continue,
        };
        let minute: u32 = match parts[1].parse() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let event_time = match NaiveTime::from_hms_opt(hour, minute, 0) {
            Some(t) => t,
            None => continue,
        };

        if let Some(ref end_str) = event.end_time_str {
            let end_parts: Vec<&str> = end_str.split(':').collect();
            if end_parts.len() == 2
                && let (Ok(eh), Ok(em)) = (end_parts[0].parse::<u32>(), end_parts[1].parse::<u32>())
                && let Some(end_time) = NaiveTime::from_hms_opt(eh, em, 0)
                && event_time <= current_time
                && current_time < end_time
            {
                match best_current {
                    None => best_current = Some((i, event_time)),
                    Some((_, best_time)) if event_time > best_time => {
                        best_current = Some((i, event_time));
                    }
                    _ => {}
                }
            }
        }

        if first_next.is_none() && event_time > current_time {
            first_next = Some(i);
        }
    }

    if let Some((idx, _)) = best_current {
        Some((idx, true))
    } else {
        first_next.map(|idx| (idx, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{DisplayAttendee, AttendeeStatus, EventId};

    fn make_event_with_attendees(title: &str, attendees: Vec<DisplayAttendee>) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: title.to_string(),
            time_str: "10:00".to_string(),
            end_time_str: None,
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees,
        }
    }

    #[test]
    fn test_event_matches_query_title() {
        let event = make_event_with_attendees("Sprint Planning", vec![]);
        assert!(event_matches_query(&event, "sprint"));
        assert!(event_matches_query(&event, "planning"));
    }

    #[test]
    fn test_event_matches_query_attendee_name() {
        let event = make_event_with_attendees("Meeting", vec![
            DisplayAttendee {
                name: Some("Alice Johnson".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert!(event_matches_query(&event, "alice"));
        assert!(event_matches_query(&event, "johnson"));
    }

    #[test]
    fn test_event_matches_query_attendee_email() {
        let event = make_event_with_attendees("Meeting", vec![
            DisplayAttendee {
                name: None,
                email: "bob@company.org".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert!(event_matches_query(&event, "bob@company"));
        assert!(event_matches_query(&event, "company.org"));
    }

    #[test]
    fn test_event_matches_query_case_insensitive() {
        let event = make_event_with_attendees("Team Standup", vec![
            DisplayAttendee {
                name: Some("Charlie Brown".to_string()),
                email: "Charlie@Example.COM".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert!(event_matches_query(&event, "team standup"));
        assert!(event_matches_query(&event, "charlie brown"));
        assert!(event_matches_query(&event, "charlie@example.com"));
    }

    #[test]
    fn test_event_match_type_title() {
        let event = make_event_with_attendees("Sprint Planning", vec![
            DisplayAttendee {
                name: Some("Alice".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "sprint"), Some(MatchType::Title));
    }

    #[test]
    fn test_event_match_type_participant() {
        let event = make_event_with_attendees("Sprint Planning", vec![
            DisplayAttendee {
                name: Some("Alice Johnson".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "alice"), Some(MatchType::Participant));
    }

    #[test]
    fn test_event_match_type_title_takes_priority() {
        // "Alice" appears in both title and attendees — title wins
        let event = make_event_with_attendees("Meeting with Alice", vec![
            DisplayAttendee {
                name: Some("Alice Johnson".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "alice"), Some(MatchType::Title));
    }

    #[test]
    fn test_event_match_type_no_match() {
        let event = make_event_with_attendees("Sprint Planning", vec![
            DisplayAttendee {
                name: Some("Alice".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "bob"), None);
    }

    #[test]
    fn test_event_matches_query_no_match() {
        let event = make_event_with_attendees("Sprint Planning", vec![
            DisplayAttendee {
                name: Some("Alice".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert!(!event_matches_query(&event, "retro"));
        assert!(!event_matches_query(&event, "bob"));
        assert!(!event_matches_query(&event, "xyz"));
    }
}
