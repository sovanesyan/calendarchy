use crate::auth::{GoogleAuthState, ICloudAuthState};
use crate::cache::{clock, DisplayEvent, EventCache, EventId};
#[cfg(test)]
use crate::cache::When;
use crate::config::Config;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike};
use std::collections::HashMap;
use std::time::Instant;

/// (year, month) of a date — the unit of fetching and caching
pub fn month_key(date: NaiveDate) -> (i32, u32) {
    (date.year(), date.month())
}

/// Search state for the interactive search modal
pub struct SearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected_index: usize,
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

/// Interactive setup wizard step
#[derive(Debug, Clone, PartialEq)]
pub enum SetupStep {
    ShortcutAsk,
    ShortcutTerminalChoice, // macOS only — pick terminal emulator
    Welcome,
    GoogleAsk,
    GoogleAuthWaiting,
    ICloudAsk,
    ICloudMethod,     // Choose EventKit vs CalDAV (macOS only)
    ICloudOpenUrl,
    ICloudAppleId,
    ICloudPassword,
    Done,
}

/// Which iCloud method was chosen
#[derive(Debug, Clone, PartialEq)]
pub enum ICloudMethod {
    EventKit,
    CalDav,
}

/// State for the interactive setup wizard
pub struct SetupState {
    pub step: SetupStep,
    pub input: String,
    pub google_enabled: bool,
    pub icloud_method: Option<ICloudMethod>,
    pub icloud_apple_id: Option<String>,
    pub icloud_password: Option<String>,
    pub error: Option<String>,
    pub eventkit_available: bool,
    pub available_terminals: Vec<String>,
}

impl SetupState {
    pub fn new() -> Self {
        Self {
            step: SetupStep::Welcome,
            input: String::new(),
            google_enabled: false,
            icloud_method: None,
            icloud_apple_id: None,
            icloud_password: None,
            error: None,
            eventkit_available: false,
            available_terminals: Vec::new(),
        }
    }
}

/// Navigation mode for two-level navigation in month view
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavigationMode {
    Day,   // Navigate between days with h/j/k/l
    Event, // Navigate between events within selected day with j/k
}

/// Which event source/panel is currently selected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    DeleteICloudEvent { calendar_url: String, event_uid: String, href: Option<String>, etag: Option<String> },
}

/// Application state
pub struct App {
    pub current_date: NaiveDate,
    pub selected_date: NaiveDate,
    pub show_logs: bool,
    pub events: EventCache,
    pub google_auth: GoogleAuthState,
    pub icloud_auth: ICloudAuthState,
    pub status_message: Option<String>,
    pub status_message_time: Option<std::time::Instant>,
    /// Error messages stick until the next keypress instead of expiring
    pub status_is_error: bool,
    pub config: Config,
    /// Month fetches currently running, per source, with the generation
    /// they were started in
    pub in_flight: HashMap<(EventSource, (i32, u32)), u64>,
    /// Bumped by refresh, actions and sign-in: results of fetches started
    /// before are stale and dropped (e.g. a refresh racing a delete)
    pub fetch_generation: u64,
    /// Last fetch error shown per source, so a retry loop doesn't re-raise
    /// an error the user already dismissed
    pub last_fetch_error: HashMap<EventSource, String>,
    /// When each (source, month) fetch was last started — throttles retries
    pub attempts: HashMap<(EventSource, (i32, u32)), Instant>,
    /// Google calendar display name, looked up once per session
    pub google_calendar_name: Option<String>,
    /// Set when the user asked to quit
    pub quit: bool,
    pub navigation_mode: NavigationMode,
    pub selected_source: EventSource,
    pub selected_event_index: usize,
    pub pending_action: Option<PendingAction>,
    pub search: Option<SearchState>,
    pub show_help: bool,
    pub dirty: bool,
    /// Tracks the last minute we rendered, so the countdown timer triggers a re-render each minute
    pub last_render_minute: u32,
    /// Interactive setup wizard state (None = not in setup mode)
    pub setup: Option<SetupState>,
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
            events,
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status_message: None,
            status_message_time: None,
            status_is_error: false,
            config: Config::default(),
            in_flight: HashMap::new(),
            fetch_generation: 0,
            last_fetch_error: HashMap::new(),
            attempts: HashMap::new(),
            google_calendar_name: None,
            quit: false,
            navigation_mode: NavigationMode::Day,
            selected_source: EventSource::Google,
            selected_event_index: 0,
            pending_action: None,
            search: None,
            show_help: false,
            dirty: true,
            last_render_minute: Local::now().minute(),
            setup: None,
        };

        app.enter_event_mode();
        app
    }

    /// Whether the visible month is being fetched for a source and has no
    /// fresh data yet — background refreshes of a loaded month stay silent
    pub fn is_loading(&self, source: EventSource) -> bool {
        let cache = match source {
            EventSource::Google => &self.events.google,
            EventSource::ICloud => &self.events.icloud,
        };
        self.in_flight.contains_key(&(source, month_key(self.current_date)))
            && cache.fetched_at(self.current_date).is_none()
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_time = Some(std::time::Instant::now());
        self.status_is_error = false;
    }

    /// Errors stay on screen until the next keypress (see clear_error)
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
        self.status_message_time = None;
        self.status_is_error = true;
    }

    pub fn clear_expired_status(&mut self) -> bool {
        if !self.status_is_error
            && let Some(time) = self.status_message_time
            && time.elapsed() > std::time::Duration::from_secs(3)
        {
            self.status_message = None;
            self.status_message_time = None;
            return true;
        }
        false
    }

    /// Dismiss a sticky error message; returns true if one was showing
    pub fn clear_error(&mut self) -> bool {
        if self.status_is_error {
            self.status_message = None;
            self.status_message_time = None;
            self.status_is_error = false;
            return true;
        }
        false
    }

    pub fn next_day(&mut self) {
        self.selected_date += Duration::days(1);
        self.sync_month_if_needed();
    }

    pub fn prev_day(&mut self) {
        self.selected_date -= Duration::days(1);
        self.sync_month_if_needed();
    }

    pub fn next_week(&mut self) {
        self.selected_date += Duration::days(7);
        self.sync_month_if_needed();
    }

    pub fn prev_week(&mut self) {
        self.selected_date -= Duration::days(7);
        self.sync_month_if_needed();
    }

    fn sync_month_if_needed(&mut self) {
        self.show_month_of(self.selected_date);
    }

    /// Display the month containing `date`. Fetching follows from state (see
    /// `App::wanted_fetches`), so changing the month is all that's needed.
    fn show_month_of(&mut self, date: NaiveDate) {
        if date.month() != self.current_date.month() || date.year() != self.current_date.year() {
            self.current_date = date.with_day(1).unwrap();
        }
    }

    pub fn goto_today(&mut self) {
        let today = Local::now().date_naive();
        self.current_date = today;
        self.selected_date = today;
    }

    pub fn goto_now(&mut self) {
        self.goto_today();
        self.enter_event_mode();
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
                    let g_time = google_events[g_idx].when.sort_key();
                    let i_time = icloud_events[i_idx].when.sort_key();
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
                self.show_month_of(check_date);
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
                self.show_month_of(check_date);
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
        self.selected_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        self.show_month_of(self.selected_date);
    }

    pub fn prev_month(&mut self) {
        let (year, month) = if self.current_date.month() == 1 {
            (self.current_date.year() - 1, 12)
        } else {
            (self.current_date.year(), self.current_date.month() - 1)
        };
        self.selected_date = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        self.show_month_of(self.selected_date);
    }

    pub fn open_search(&mut self) {
        self.search = Some(SearchState {
            query: String::new(),
            results: Vec::new(),
            selected_index: 0,
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
            let matched_events = self.events.google.all_events().map(|e| (e, EventSource::Google))
                .chain(self.events.icloud.all_events().map(|e| (e, EventSource::ICloud)));
            for (event, source) in matched_events {
                if event.date >= today
                    && let Some(match_type) = event_match_type(event, &query_lower)
                {
                    results.push(SearchResult {
                        event: event.clone(),
                        source,
                        match_type,
                    });
                }
            }
            results.sort_by(|a, b| {
                let a_title = a.event.title.to_lowercase().contains(&query_lower);
                let b_title = b.event.title.to_lowercase().contains(&query_lower);
                b_title.cmp(&a_title)
                    .then_with(|| a.event.date.cmp(&b.event.date))
                    // Timed before all-day within a date, then by start
                    .then_with(|| {
                        let key = |e: &DisplayEvent| (e.when.is_all_day(), e.when.start().unwrap_or(0));
                        key(&a.event).cmp(&key(&b.event))
                    })
            });
            // A multi-day event is one result (its next day), not one per day
            let mut seen_multi_day: Vec<EventId> = Vec::new();
            results.retain(|r| {
                if !r.event.spans_days {
                    return true;
                }
                if seen_multi_day.contains(&r.event.id) {
                    return false;
                }
                seen_multi_day.push(r.event.id.clone());
                true
            });
        }

        if let Some(ref mut search) = self.search {
            search.results = results;
            if search.selected_index >= search.results.len() {
                search.selected_index = search.results.len().saturating_sub(1);
            }
        }
    }

    pub fn select_search_result(&mut self) {
        let (date, source, event_id) = match self.search.as_ref() {
            Some(s) => {
                match s.results.get(s.selected_index) {
                    Some(r) => (r.event.date, r.source, r.event.id.clone()),
                    None => return,
                }
            }
            None => return,
        };

        // Navigate to the date
        self.selected_date = date;
        self.show_month_of(date);

        // Enter event mode on the correct source/index
        self.navigation_mode = NavigationMode::Event;
        self.selected_source = source;

        let events = match source {
            EventSource::Google => self.events.google.get(date),
            EventSource::ICloud => self.events.icloud.get(date),
        };
        self.selected_event_index = events.iter()
            .position(|e| e.id == event_id)
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
        if let Some(ref name) = attendee.name
            && name.to_lowercase().contains(query_lower)
        {
            return Some(MatchType::Participant);
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
        let Some(start) = event.when.start().and_then(clock) else { continue };

        // Compared as clock times (sub-second precision), as before; an end of
        // midnight means "until the end of the day"
        if let Some(end) = event.when.end()
            && start <= current_time
            && clock(end).is_none_or(|end| current_time < end)
            && best_current.is_none_or(|(_, best)| start > best)
        {
            best_current = Some((i, start));
        }

        if first_next.is_none() && start > current_time {
            first_next = Some(i);
        }
    }

    match best_current {
        Some((idx, _)) => Some((idx, true)),
        None => first_next.map(|idx| (idx, false)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cache::{DisplayAttendee, AttendeeStatus, EventId};

    pub(crate) fn make_event_with_attendees(title: &str, attendees: Vec<DisplayAttendee>) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: title.to_string(),
            when: When::parse_label("10:00"),
            spans_days: false,
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

    pub(crate) fn app_on(date: NaiveDate) -> App {
        let mut app = App {
            current_date: date,
            selected_date: date,
            show_logs: false,
            events: EventCache::new(),
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status_message: None,
            status_message_time: None,
            status_is_error: false,
            config: Config::default(),
            in_flight: HashMap::new(),
            fetch_generation: 0,
            last_fetch_error: HashMap::new(),
            attempts: HashMap::new(),
            google_calendar_name: None,
            quit: false,
            navigation_mode: NavigationMode::Day,
            selected_source: EventSource::Google,
            selected_event_index: 0,
            pending_action: None,
            search: None,
            show_help: false,
            dirty: true,
            last_render_minute: 0,
            setup: None,
        };
        app
    }

    #[test]
    fn test_month_jumps_move_to_the_first_of_the_month() {
        let mut app = app_on(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap());
        app.next_month();
        assert_eq!(app.current_date, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());

        let mut app = app_on(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        app.prev_month();
        assert_eq!(app.current_date, NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
    }

    #[test]
    fn test_event_jump_into_next_month_switches_month() {
        let mut app = app_on(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        let target = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let mut ev = make_event_with_attendees("Later", vec![]);
        ev.date = target;
        app.events.google.store(vec![ev], target);
        app.navigation_mode = NavigationMode::Event;
        app.next_event();
        assert_eq!(app.selected_date, target);
        assert_eq!(app.current_date, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    }

    #[test]
    fn test_search_lists_a_multi_day_event_once() {
        // Five upcoming days of leave, stored per month they fall in
        let today = Local::now().date_naive();
        let mut app = app_on(today);
        for offset in 1..=5 {
            let mut e = make_event_with_attendees("Leave", vec![]);
            e.date = today + Duration::days(offset);
            e.when = When::AllDay;
            e.spans_days = true;
            // store() replaces a whole month, so re-store it with the new day
            let month: Vec<_> = app.events.google.all_events()
                .filter(|x| x.date.month() == e.date.month())
                .cloned()
                .chain([e.clone()])
                .collect();
            app.events.google.store(month, e.date);
        }
        app.open_search();
        app.search.as_mut().unwrap().query = "leave".into();
        app.update_search_results();
        let results = &app.search.as_ref().unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event.date, today + Duration::days(1), "the next day of it");
    }
}
