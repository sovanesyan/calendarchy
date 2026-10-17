use crate::auth::{GoogleAuthState, ICloudAuthState};
use crate::cache::{clock, DisplayEvent, EventCache, EventId};
#[cfg(test)]
use crate::cache::When;
use crate::config::Config;
use crate::sources::EventVerb;
use crate::update::STATUS_TTL;
use crate::utils::month_start;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
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

/// Whether a search result matched on title or participant, in result
/// order (title matches first)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    Welcome,
    GoogleAsk,
    GoogleAuthWaiting,
    ICloudAsk,
    ICloudOpenUrl,
    ICloudAppleId,
    ICloudPassword,
    Done,
}

/// State for the interactive setup wizard
pub struct SetupState {
    pub step: SetupStep,
    pub input: String,
    pub icloud_apple_id: Option<String>,
    pub icloud_password: Option<String>,
    pub error: Option<String>,
}

impl SetupState {
    pub fn new(step: SetupStep) -> Self {
        Self {
            step,
            input: String::new(),
            icloud_apple_id: None,
            icloud_password: None,
            error: None,
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

impl EventSource {
    pub fn other(self) -> Self {
        match self {
            EventSource::Google => EventSource::ICloud,
            EventSource::ICloud => EventSource::Google,
        }
    }
}

/// An event action awaiting confirmation
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub verb: EventVerb,
    pub id: EventId,
}

/// The status bar message
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Disappears `STATUS_TTL` after it was set
    Info(String, Instant),
    /// Stays on screen until the next keypress
    Error(String),
}

impl Status {
    pub fn text(&self) -> &str {
        match self {
            Status::Info(text, _) | Status::Error(text) => text,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Status::Error(_))
    }
}

/// Application state
pub struct App {
    /// The selected day; its month is the one shown (see `visible_month`)
    pub selected_date: NaiveDate,
    pub show_logs: bool,
    pub events: EventCache,
    pub google_auth: GoogleAuthState,
    pub icloud_auth: ICloudAuthState,
    pub status: Option<Status>,
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
    /// Today, with the cached events loaded and the current or next event
    /// selected
    pub fn new() -> Self {
        let mut app = Self::on(Local::now().date_naive());
        app.events.load_from_disk();
        app.enter_event_mode();
        app
    }

    /// An app on `date` with nothing loaded, configured or signed in
    pub fn on(date: NaiveDate) -> Self {
        Self {
            selected_date: date,
            show_logs: false,
            events: EventCache::default(),
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status: None,
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
        }
    }

    /// The month on screen (its 1st): the selected day's
    pub fn visible_month(&self) -> NaiveDate {
        month_start(self.selected_date)
    }

    /// Whether the visible month is being fetched for a source and has no
    /// fresh data yet — background refreshes of a loaded month stay silent
    pub fn is_loading(&self, source: EventSource) -> bool {
        let visible = self.visible_month();
        self.in_flight.contains_key(&(source, month_key(visible)))
            && self.events.source(source).fetched_at(visible).is_none()
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Info(msg.into(), Instant::now()));
    }

    /// Errors stay on screen until the next keypress (see clear_error)
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Error(msg.into()));
    }

    pub fn clear_expired_status(&mut self) -> bool {
        if let Some(Status::Info(_, time)) = self.status
            && time.elapsed() > STATUS_TTL
        {
            self.status = None;
            return true;
        }
        false
    }

    /// Dismiss a sticky error message; returns true if one was showing
    pub fn clear_error(&mut self) -> bool {
        if self.status.as_ref().is_some_and(Status::is_error) {
            self.status = None;
            return true;
        }
        false
    }

    /// Open on `date`, in event mode like startup
    pub fn goto_date(&mut self, date: NaiveDate) {
        self.selected_date = date;
        self.enter_event_mode();
    }

    pub fn get_current_source_events(&self) -> &[DisplayEvent] {
        self.events.source(self.selected_source).get(self.selected_date)
    }

    pub fn get_selected_event(&self) -> Option<&DisplayEvent> {
        if self.navigation_mode == NavigationMode::Event {
            self.get_current_source_events().get(self.selected_event_index)
        } else {
            None
        }
    }

    /// Select an event on the selected day: today, the one under way (Google
    /// first) or else the next to start; otherwise the day's first. False,
    /// leaving the mode alone, if the day has no events.
    pub fn enter_event_mode(&mut self) -> bool {
        self.enter_event_mode_at(Local::now().naive_local())
    }

    fn enter_event_mode_at(&mut self, now: NaiveDateTime) -> bool {
        let date = self.selected_date;
        if !self.events.has_events(date) {
            return false;
        }
        self.navigation_mode = NavigationMode::Event;

        if date == now.date() {
            let found = [EventSource::Google, EventSource::ICloud]
                .map(|source| (source, find_current_or_next_event(self.events.source(source).get(date), now.time())));
            let current = found.iter().find_map(|&(source, f)| match f {
                Some((index, true)) => Some((source, index)),
                _ => None,
            });
            // Ties go to Google: min_by_key keeps the first minimum
            let next = || {
                found
                    .iter()
                    .filter_map(|&(source, f)| f.map(|(index, _)| (source, index)))
                    .min_by_key(|&(source, index)| self.events.source(source).get(date)[index].when.sort_key())
            };
            if let Some((source, index)) = current.or_else(next) {
                self.selected_source = source;
                self.selected_event_index = index;
                return true;
            }
        }
        self.select_first();
        true
    }

    /// Select the selected day's first event: Google's first, else iCloud's
    fn select_first(&mut self) {
        let google_empty = self.events.google.get(self.selected_date).is_empty();
        self.selected_source = if google_empty { EventSource::ICloud } else { EventSource::Google };
        self.selected_event_index = 0;
    }

    /// Select the selected day's last event: iCloud's last, else Google's
    fn select_last(&mut self) {
        let icloud_empty = self.events.icloud.get(self.selected_date).is_empty();
        self.selected_source = if icloud_empty { EventSource::Google } else { EventSource::ICloud };
        self.selected_event_index = self.get_current_source_events().len().saturating_sub(1);
    }

    /// Jump to the other panel, landing on the event closest in time to the
    /// selected one (the first starting at or after it, else the last)
    pub fn switch_panel(&mut self) {
        let other = self.selected_source.other();
        let other_events = self.events.source(other).get(self.selected_date);
        if other_events.is_empty() {
            let label = match other {
                EventSource::Google => self.config.display.google_label(),
                EventSource::ICloud => self.config.display.icloud_label(),
            };
            let msg = format!("Nothing in {} on this day", label);
            self.set_status(msg);
            return;
        }
        let from = self.get_selected_event().map_or((false, 0), |e| e.when.sort_key());
        let index = other_events
            .iter()
            .position(|e| e.when.sort_key() >= from)
            .unwrap_or(other_events.len() - 1);
        self.selected_source = other;
        self.selected_event_index = index;
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
                self.jump_to_day_with_events(1);
            }
        } else {
            self.jump_to_day_with_events(1);
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
                self.jump_to_day_with_events(-1);
            }
        } else {
            self.jump_to_day_with_events(-1);
        }
    }

    /// Move to the nearest day with events within 90 days, forwards (`step`
    /// 1, landing on its first event) or backwards (-1, on its last)
    fn jump_to_day_with_events(&mut self, step: i64) {
        let found = (1..=90)
            .map(|n| self.selected_date + Duration::days(n * step))
            .find(|&date| self.events.has_events(date));
        if let Some(date) = found {
            self.selected_date = date;
            if step > 0 { self.select_first() } else { self.select_last() }
        }
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
                a.match_type.cmp(&b.match_type)
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

        // Enter event mode on the correct source/index
        self.navigation_mode = NavigationMode::Event;
        self.selected_source = source;
        self.selected_event_index = self.events.source(source).get(date).iter()
            .position(|e| e.id == event_id)
            .unwrap_or(0);

        self.close_search();
    }
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

        // Compared as clock times (sub-second precision); an end of midnight
        // means "until the end of the day"
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
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees,
        }
    }

    #[test]
    fn test_event_match_type_title_words() {
        let event = make_event_with_attendees("Sprint Planning", vec![]);
        assert_eq!(event_match_type(&event, "sprint"), Some(MatchType::Title));
        assert_eq!(event_match_type(&event, "planning"), Some(MatchType::Title));
    }

    #[test]
    fn test_event_match_type_attendee_name() {
        let event = make_event_with_attendees("Meeting", vec![
            DisplayAttendee {
                name: Some("Alice Johnson".to_string()),
                email: "alice@example.com".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "alice"), Some(MatchType::Participant));
        assert_eq!(event_match_type(&event, "johnson"), Some(MatchType::Participant));
    }

    #[test]
    fn test_event_match_type_attendee_email() {
        let event = make_event_with_attendees("Meeting", vec![
            DisplayAttendee {
                name: None,
                email: "bob@company.org".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "bob@company"), Some(MatchType::Participant));
        assert_eq!(event_match_type(&event, "company.org"), Some(MatchType::Participant));
    }

    #[test]
    fn test_event_match_type_case_insensitive() {
        let event = make_event_with_attendees("Team Standup", vec![
            DisplayAttendee {
                name: Some("Charlie Brown".to_string()),
                email: "Charlie@Example.COM".to_string(),
                status: AttendeeStatus::Accepted,
            },
        ]);
        assert_eq!(event_match_type(&event, "team standup"), Some(MatchType::Title));
        assert_eq!(event_match_type(&event, "charlie brown"), Some(MatchType::Participant));
        assert_eq!(event_match_type(&event, "charlie@example.com"), Some(MatchType::Participant));
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
        assert_eq!(event_match_type(&event, "retro"), None);
        assert_eq!(event_match_type(&event, "xyz"), None);
    }

    #[test]
    fn test_switch_panel_lands_on_the_nearest_event() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut app = App::on(day);
        let at = |label: &str, id: &str, icloud: bool| {
            let mut e = make_event_with_attendees(label, vec![]);
            e.when = When::parse_label(label);
            if icloud {
                e.id = EventId::ICloud { calendar_url: "c".into(), event_uid: id.into(), etag: None, calendar_name: None, href: None };
            } else {
                e.id = EventId::Google { calendar_id: "c".into(), event_id: id.into(), calendar_name: None };
            }
            e
        };
        app.events.google.store(vec![at("09:00", "g1", false), at("14:00", "g2", false)], day);
        app.events.icloud.store(vec![at("All day", "i0", true), at("12:00", "i1", true), at("15:00", "i2", true)], day);
        app.navigation_mode = NavigationMode::Event;
        app.selected_event_index = 1; // Google 14:00

        app.switch_panel();
        assert_eq!((app.selected_source, app.selected_event_index), (EventSource::ICloud, 2)); // 15:00
        app.selected_event_index = 0; // the all-day one
        app.switch_panel();
        assert_eq!((app.selected_source, app.selected_event_index), (EventSource::Google, 0));

        // Nothing over there: stay put and say so
        let mut app = App::on(day);
        app.events.google.store(vec![at("09:00", "g1", false)], day);
        app.navigation_mode = NavigationMode::Event;
        app.switch_panel();
        assert_eq!(app.selected_source, EventSource::Google);
        assert!(app.status.as_ref().unwrap().text().contains("Personal"));
    }

    #[test]
    fn test_month_jumps_move_to_the_first_of_the_month() {
        let mut app = App::on(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap());
        app.apply(crate::keymap::Action::NextMonth);
        assert_eq!(app.selected_date, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(app.visible_month(), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());

        let mut app = App::on(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        app.apply(crate::keymap::Action::PrevMonth);
        assert_eq!(app.visible_month(), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
    }

    #[test]
    fn test_event_jump_into_next_month_switches_month() {
        let mut app = App::on(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        let target = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let mut ev = make_event_with_attendees("Later", vec![]);
        ev.date = target;
        app.events.google.store(vec![ev], target);
        app.navigation_mode = NavigationMode::Event;
        app.next_event();
        assert_eq!(app.selected_date, target);
        assert_eq!(app.visible_month(), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
    }

    fn timed(start: &str, end: &str, icloud: bool) -> DisplayEvent {
        let mut e = make_event_with_attendees(start, vec![]);
        e.when = When::parse_label(start).ending(end);
        if icloud {
            e.id = EventId::ICloud { calendar_url: "c".into(), event_uid: start.into(), etag: None, calendar_name: None, href: None };
        }
        e
    }

    #[test]
    fn test_enter_event_mode_prefers_the_current_event_then_the_next() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let at = |hm: &str| day.and_time(NaiveTime::parse_from_str(hm, "%H:%M").unwrap());
        let entered = |google: Vec<DisplayEvent>, icloud: Vec<DisplayEvent>, now: NaiveDateTime| {
            let mut app = App::on(day);
            app.events.google.store(google, day);
            app.events.icloud.store(icloud, day);
            assert!(app.enter_event_mode_at(now));
            assert_eq!(app.navigation_mode, NavigationMode::Event);
            (app.selected_source, app.selected_event_index)
        };
        let g = |s, e| timed(s, e, false);
        let i = |s, e| timed(s, e, true);

        // Under way in iCloud beats Google's next
        assert_eq!(entered(vec![g("09:00", "10:00"), g("12:00", "13:00")], vec![i("10:00", "11:00")], at("10:30")), (EventSource::ICloud, 0));
        // Under way in both: Google's
        assert_eq!(entered(vec![g("08:00", "09:00"), g("10:15", "11:00")], vec![i("10:00", "11:00")], at("10:30")), (EventSource::Google, 1));
        // Nothing under way: the earliest next, Google on a tie
        assert_eq!(entered(vec![g("12:00", "13:00")], vec![i("09:00", "09:30"), i("11:00", "12:00")], at("10:30")), (EventSource::ICloud, 1));
        assert_eq!(entered(vec![g("12:00", "13:00")], vec![i("12:00", "12:30")], at("10:30")), (EventSource::Google, 0));
        // All over, or another day: the day's first
        assert_eq!(entered(vec![g("08:00", "09:00")], vec![i("07:00", "08:00")], at("20:00")), (EventSource::Google, 0));
        assert_eq!(entered(vec![], vec![i("12:00", "13:00"), i("14:00", "15:00")], at("10:30") + Duration::days(1)), (EventSource::ICloud, 0));

        // No events: stays in Day mode
        let mut app = App::on(day);
        assert!(!app.enter_event_mode_at(at("10:30")));
        assert_eq!(app.navigation_mode, NavigationMode::Day);
    }

    #[test]
    fn test_stepping_past_a_day_lands_on_the_nearest_day_with_events() {
        let d = |day| NaiveDate::from_ymd_opt(2026, 1, day).unwrap();
        let on = |mut e: DisplayEvent, day| {
            e.date = d(day);
            e
        };
        let mut app = App::on(d(15));
        app.events.google.store(vec![on(timed("09:00", "10:00", false), 10), on(timed("09:00", "10:00", false), 13), on(timed("11:00", "12:00", false), 13)], d(1));
        app.events.icloud.store(vec![on(timed("10:00", "11:00", true), 10), on(timed("12:00", "13:00", true), 10)], d(1));
        app.navigation_mode = NavigationMode::Event;

        // Backwards: the day's last event, iCloud's if it has any
        app.prev_event();
        assert_eq!((app.selected_date, app.selected_source, app.selected_event_index), (d(13), EventSource::Google, 1));
        app.selected_event_index = 0;
        app.prev_event();
        assert_eq!((app.selected_date, app.selected_source, app.selected_event_index), (d(10), EventSource::ICloud, 1));
        // Forwards: the day's first, Google's if it has any
        app.next_event();
        assert_eq!((app.selected_date, app.selected_source, app.selected_event_index), (d(13), EventSource::Google, 0));
        // Nothing further within 90 days: stay put
        app.selected_event_index = 1;
        app.next_event();
        assert_eq!((app.selected_date, app.selected_event_index), (d(13), 1));
    }

    #[test]
    fn test_search_lists_a_multi_day_event_once() {
        // Five upcoming days of leave, stored per month they fall in
        let today = Local::now().date_naive();
        let mut app = App::on(today);
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
