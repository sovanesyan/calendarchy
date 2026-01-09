mod cache;
mod config;
mod error;
mod google;
mod icloud;
mod ui;

use cache::{AttendeeStatus, DisplayAttendee, DisplayEvent, EventCache, EventId};
use chrono::{Datelike, DateTime, Duration, Local, NaiveDate, NaiveTime, Utc};
use config::Config;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use google::{CalendarClient, GoogleAuth, TokenInfo};
use icloud::{CalDavClient, ICalEvent, ICloudAuth};
use std::io::stdout;
use std::sync::Mutex;
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;
use ui::AuthDisplay;

/// Global log storage for HTTP requests
static HTTP_LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sort order for attendee status (lower = first)
fn status_sort_order(status: &AttendeeStatus) -> u8 {
    match status {
        AttendeeStatus::Organizer => 0,
        AttendeeStatus::Accepted => 1,
        AttendeeStatus::Tentative => 2,
        AttendeeStatus::NeedsAction => 3,
        AttendeeStatus::Declined => 4,
    }
}

/// Sort attendees by status (accepted first, declined last), then by name
fn sort_attendees(attendees: &mut [DisplayAttendee]) {
    attendees.sort_by(|a, b| {
        let status_cmp = status_sort_order(&a.status).cmp(&status_sort_order(&b.status));
        if status_cmp != std::cmp::Ordering::Equal {
            status_cmp
        } else {
            a.name.cmp(&b.name)
        }
    });
}

/// Extract a display name from an email address
/// e.g., "john.smith@example.com" -> "John Smith"
///       "jsmith@example.com" -> "Jsmith"
fn name_from_email(email: &str) -> String {
    // Get the part before @
    let local = email.split('@').next().unwrap_or(email);

    // Split by common separators (., _, -)
    let parts: Vec<&str> = local.split(|c| c == '.' || c == '_' || c == '-').collect();

    // Capitalize each part and join with space
    parts
        .iter()
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Log an HTTP request
pub fn log_request(method: &str, url: &str) {
    if let Ok(mut logs) = HTTP_LOGS.lock() {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        logs.push(format!("[{}] {} {}", timestamp, method, url));
        // Keep only last 100 logs
        if logs.len() > 100 {
            logs.remove(0);
        }
    }
}

/// Log an HTTP response
pub fn log_response(status: u16, url: &str) {
    if let Ok(mut logs) = HTTP_LOGS.lock() {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        logs.push(format!("[{}] <- {} {}", timestamp, status, url));
        // Keep only last 100 logs
        if logs.len() > 100 {
            logs.remove(0);
        }
    }
}

/// Get recent logs for display
pub fn get_recent_logs(count: usize) -> Vec<String> {
    if let Ok(logs) = HTTP_LOGS.lock() {
        logs.iter().rev().take(count).cloned().collect()
    } else {
        Vec::new()
    }
}

/// View mode for the calendar
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    Month,
    Week,
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

/// Google authentication state
#[derive(Debug, Clone)]
pub enum GoogleAuthState {
    NotConfigured,
    NotAuthenticated,
    AwaitingUserCode {
        user_code: String,
        verification_url: String,
        device_code: String,
        expires_at: DateTime<Utc>,
    },
    Authenticated(TokenInfo),
    Error(String),
}

impl AuthDisplay for GoogleAuthState {
    fn is_authenticated(&self) -> bool {
        matches!(self, GoogleAuthState::Authenticated(_))
    }

    fn status_message(&self) -> String {
        match self {
            GoogleAuthState::NotConfigured => "Not configured".to_string(),
            GoogleAuthState::NotAuthenticated => "Press 'g' to connect".to_string(),
            GoogleAuthState::AwaitingUserCode { user_code, verification_url, .. } => {
                format!("{} → {}", verification_url, user_code)
            }
            GoogleAuthState::Authenticated(_) => String::new(),
            GoogleAuthState::Error(msg) => msg.clone(),
        }
    }
}

/// iCloud authentication state
#[derive(Debug, Clone)]
pub enum ICloudAuthState {
    NotConfigured,
    NotAuthenticated,
    Discovering,
    Authenticated { calendar_urls: Vec<String> },
    Error(String),
}

impl AuthDisplay for ICloudAuthState {
    fn is_authenticated(&self) -> bool {
        matches!(self, ICloudAuthState::Authenticated { .. })
    }

    fn status_message(&self) -> String {
        match self {
            ICloudAuthState::NotConfigured => "Not configured".to_string(),
            ICloudAuthState::NotAuthenticated => "Press 'i' to connect".to_string(),
            ICloudAuthState::Discovering => "Discovering...".to_string(),
            ICloudAuthState::Authenticated { .. } => String::new(),
            ICloudAuthState::Error(msg) => msg.clone(),
        }
    }
}

/// Application state
struct App {
    current_date: NaiveDate,
    selected_date: NaiveDate,
    view_mode: ViewMode,
    show_weekends: bool,
    show_logs: bool, // Toggle HTTP request logs display
    events: EventCache,
    google_auth: GoogleAuthState,
    icloud_auth: ICloudAuthState,
    status_message: Option<String>,
    config: Config,
    google_needs_fetch: bool,
    icloud_needs_fetch: bool,
    google_loading: bool,
    icloud_loading: bool,
    // Two-level navigation state
    navigation_mode: NavigationMode,
    selected_source: EventSource,
    selected_event_index: usize, // Index within the selected source
}

impl App {
    fn new() -> Self {
        let today = Local::now().date_naive();
        let mut events = EventCache::new();
        // Load cached events from disk for instant display
        events.load_from_disk();

        let mut app = Self {
            current_date: today,
            selected_date: today,
            view_mode: ViewMode::Month,
            show_weekends: false,
            show_logs: false,
            events,
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status_message: None,
            config: Config::default(),
            google_needs_fetch: false,
            icloud_needs_fetch: false,
            google_loading: false,
            icloud_loading: false,
            navigation_mode: NavigationMode::Day,
            selected_source: EventSource::Google,
            selected_event_index: 0,
        };

        // Auto-enter event mode with current/next event selected
        app.enter_event_mode();

        app
    }

    fn next_day(&mut self) {
        self.selected_date = self.selected_date + Duration::days(1);
        self.sync_month_if_needed();
    }

    fn prev_day(&mut self) {
        self.selected_date = self.selected_date - Duration::days(1);
        self.sync_month_if_needed();
    }

    fn next_week(&mut self) {
        self.selected_date = self.selected_date + Duration::days(7);
        self.sync_month_if_needed();
    }

    fn prev_week(&mut self) {
        self.selected_date = self.selected_date - Duration::days(7);
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

    fn goto_today(&mut self) {
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

    fn month_range(&self) -> (NaiveDate, NaiveDate) {
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

    /// Get events for the current source
    fn get_current_source_events(&self) -> &[DisplayEvent] {
        match self.selected_source {
            EventSource::Google => self.events.google.get(self.selected_date),
            EventSource::ICloud => self.events.icloud.get(self.selected_date),
        }
    }

    /// Get the currently selected event if in Event mode
    fn get_selected_event(&self) -> Option<&DisplayEvent> {
        if self.navigation_mode == NavigationMode::Event {
            self.get_current_source_events().get(self.selected_event_index)
        } else {
            None
        }
    }

    /// Enter event navigation mode, selecting the next upcoming event
    fn enter_event_mode(&mut self) {
        let google_events = self.events.google.get(self.selected_date);
        let icloud_events = self.events.icloud.get(self.selected_date);

        if google_events.is_empty() && icloud_events.is_empty() {
            return;
        }

        self.navigation_mode = NavigationMode::Event;

        // If today, try to find current or next event
        let today = Local::now().date_naive();
        if self.selected_date == today {
            let current_time = Local::now().time();

            // Check Google events for current/next
            if let Some((idx, is_current_or_next)) = find_current_or_next_event(google_events, current_time) {
                if is_current_or_next {
                    self.selected_source = EventSource::Google;
                    self.selected_event_index = idx;
                    return;
                }
            }

            // Check iCloud events for current/next
            if let Some((idx, is_current_or_next)) = find_current_or_next_event(icloud_events, current_time) {
                if is_current_or_next {
                    self.selected_source = EventSource::ICloud;
                    self.selected_event_index = idx;
                    return;
                }
            }

            // Compare the next events from both sources to find the earliest
            let google_next = find_current_or_next_event(google_events, current_time);
            let icloud_next = find_current_or_next_event(icloud_events, current_time);

            match (google_next, icloud_next) {
                (Some((g_idx, _)), Some((i_idx, _))) => {
                    // Compare times to pick the earlier one
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

        // Fallback: select first event in first non-empty source
        if !google_events.is_empty() {
            self.selected_source = EventSource::Google;
            self.selected_event_index = 0;
        } else {
            self.selected_source = EventSource::ICloud;
            self.selected_event_index = 0;
        }
    }

    /// Exit event navigation mode
    fn exit_event_mode(&mut self) {
        self.navigation_mode = NavigationMode::Day;
        self.selected_source = EventSource::Google;
        self.selected_event_index = 0;
    }

    /// Navigate to next event (crosses from Google to iCloud)
    fn next_event(&mut self) {
        let current_events = self.get_current_source_events();

        if self.selected_event_index < current_events.len().saturating_sub(1) {
            // Move within current source
            self.selected_event_index += 1;
        } else if self.selected_source == EventSource::Google {
            // At end of Google, try to move to iCloud
            let icloud_events = self.events.icloud.get(self.selected_date);
            if !icloud_events.is_empty() {
                self.selected_source = EventSource::ICloud;
                self.selected_event_index = 0;
            }
        }
        // At end of iCloud - do nothing
    }

    /// Navigate to previous event (crosses from iCloud to Google)
    fn prev_event(&mut self) {
        if self.selected_event_index > 0 {
            // Move within current source
            self.selected_event_index -= 1;
        } else if self.selected_source == EventSource::ICloud {
            // At start of iCloud, try to move to Google
            let google_events = self.events.google.get(self.selected_date);
            if !google_events.is_empty() {
                self.selected_source = EventSource::Google;
                self.selected_event_index = google_events.len().saturating_sub(1);
            }
        }
        // At start of Google - do nothing
    }
}

/// Find current or next event in a list, returns (index, is_current)
fn find_current_or_next_event(events: &[DisplayEvent], current_time: NaiveTime) -> Option<(usize, bool)> {
    for (i, event) in events.iter().enumerate() {
        if event.time_str == "All day" {
            continue;
        }

        // Parse event time
        let parts: Vec<&str> = event.time_str.split(':').collect();
        if parts.len() != 2 {
            continue;
        }
        let hour: u32 = parts[0].parse().ok()?;
        let minute: u32 = parts[1].parse().ok()?;
        let event_time = NaiveTime::from_hms_opt(hour, minute, 0)?;

        // Check if current (within event time range)
        if let Some(ref end_str) = event.end_time_str {
            let end_parts: Vec<&str> = end_str.split(':').collect();
            if end_parts.len() == 2 {
                if let (Ok(eh), Ok(em)) = (end_parts[0].parse::<u32>(), end_parts[1].parse::<u32>()) {
                    if let Some(end_time) = NaiveTime::from_hms_opt(eh, em, 0) {
                        if event_time <= current_time && current_time < end_time {
                            return Some((i, true)); // Current event
                        }
                    }
                }
            }
        }

        // Check if next (starts after current time)
        if event_time > current_time {
            return Some((i, false)); // Next event
        }
    }
    None
}

/// Messages from async tasks to main loop
enum AsyncMessage {
    // Google messages
    GoogleDeviceCode {
        user_code: String,
        verification_url: String,
        device_code: String,
        expires_at: DateTime<Utc>,
    },
    GoogleToken(TokenInfo),
    GoogleAuthPending,
    GoogleAuthError(String),
    GoogleEvents(Vec<google::CalendarEvent>, NaiveDate, String), // events, month_date, calendar_id
    GoogleFetchError(String),
    GoogleTokenRefreshed(TokenInfo),
    GoogleRefreshFailed(String),

    // iCloud messages
    ICloudDiscovered { calendar_urls: Vec<String> },
    ICloudDiscoveryError(String),
    ICloudEvents(Vec<ICalEvent>, NaiveDate),
    ICloudFetchError(String),

    // Event action messages
    EventActionSuccess(String), // Success message
    EventActionError(String),   // Error message
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new();

    // Load config
    app.config = Config::load().unwrap_or_default();

    // Initialize auth states based on config
    // Track if we need to refresh Google token
    let mut google_needs_refresh: Option<String> = None;

    if app.config.google.is_some() {
        app.google_auth = GoogleAuthState::NotAuthenticated;
        // Try to load saved Google tokens
        if let Ok(Some(tokens)) = config::load_google_tokens() {
            if !tokens.is_expired() {
                app.google_auth = GoogleAuthState::Authenticated(tokens);
                app.google_needs_fetch = true;
            } else if let Some(ref refresh_token) = tokens.refresh_token {
                // Token expired but we have a refresh token - will refresh after channel is created
                google_needs_refresh = Some(refresh_token.clone());
                app.google_loading = true;
            }
        }
    }

    if app.config.icloud.is_some() {
        app.icloud_auth = ICloudAuthState::NotAuthenticated;
        // Try to load saved iCloud discovery info
        if let Ok(Some(icloud_tokens)) = config::load_icloud_tokens() {
            if !icloud_tokens.calendar_urls.is_empty() {
                app.icloud_auth = ICloudAuthState::Authenticated {
                    calendar_urls: icloud_tokens.calendar_urls,
                };
                app.icloud_needs_fetch = true;
            }
        }
    }

    if app.config.google.is_none() && app.config.icloud.is_none() {
        app.status_message = Some("No calendars configured. Edit ~/.config/calendarchy/config.json".to_string());
    }

    // Channel for async messages
    let (tx, mut rx) = mpsc::channel::<AsyncMessage>(32);

    // Spawn Google token refresh if needed
    if let Some(refresh_token) = google_needs_refresh {
        if let Some(ref google_config) = app.config.google {
            let auth = GoogleAuth::new(google_config.clone());
            let tx = tx.clone();
            tokio::spawn(async move {
                match auth.refresh_token(&refresh_token).await {
                    Ok(new_tokens) => {
                        let _ = tx.send(AsyncMessage::GoogleTokenRefreshed(new_tokens)).await;
                    }
                    Err(e) => {
                        let _ = tx.send(AsyncMessage::GoogleRefreshFailed(e.to_string())).await;
                    }
                }
            });
        }
    }

    // Enable raw mode
    enable_raw_mode()?;

    // Main loop
    loop {
        // Render
        let render_state = ui::RenderState {
            current_date: app.current_date,
            selected_date: app.selected_date,
            view_mode: app.view_mode,
            show_weekends: app.show_weekends,
            events: &app.events,
            google_auth: &app.google_auth,
            icloud_auth: &app.icloud_auth,
            status_message: app.status_message.as_deref(),
            google_loading: app.google_loading,
            icloud_loading: app.icloud_loading,
            navigation_mode: app.navigation_mode,
            selected_source: app.selected_source,
            selected_event_index: app.selected_event_index,
            show_logs: app.show_logs,
        };
        ui::render(&render_state);

        // Check if we need to fetch Google events
        if app.google_needs_fetch {
            if let GoogleAuthState::Authenticated(ref tokens) = app.google_auth {
                let (start, end) = app.month_range();
                if !app.events.google.has_month(start) {
                    let tokens = tokens.clone();
                    let calendar_id = app.config.google.as_ref()
                        .map(|c| c.calendar_id.clone())
                        .unwrap_or_else(|| "primary".to_string());
                    let tx = tx.clone();

                    app.google_loading = true;
                    let calendar_id_clone = calendar_id.clone();
                    tokio::spawn(async move {
                        let client = CalendarClient::new();
                        match client.list_events(&tokens, &calendar_id, start, end).await {
                            Ok(events) => {
                                let _ = tx.send(AsyncMessage::GoogleEvents(events, start, calendar_id_clone)).await;
                            }
                            Err(e) => {
                                let _ = tx.send(AsyncMessage::GoogleFetchError(e.to_string())).await;
                            }
                        }
                    });
                }
            }
            app.google_needs_fetch = false;
        }

        // Check if we need to fetch iCloud events
        if app.icloud_needs_fetch {
            if let ICloudAuthState::Authenticated { ref calendar_urls } = app.icloud_auth {
                let (start, end) = app.month_range();
                if !app.events.icloud.has_month(start) {
                    if let Some(ref icloud_config) = app.config.icloud {
                        let auth = ICloudAuth::new(icloud_config.clone());
                        let client = CalDavClient::new(auth);
                        let calendar_urls = calendar_urls.clone();
                        let tx = tx.clone();

                        app.icloud_loading = true;
                        tokio::spawn(async move {
                            let mut all_events = Vec::new();
                            for url in &calendar_urls {
                                match client.fetch_events(url, start, end).await {
                                    Ok(events) => all_events.extend(events),
                                    Err(e) => {
                                        let _ = tx.send(AsyncMessage::ICloudFetchError(e.to_string())).await;
                                        return;
                                    }
                                }
                            }
                            let _ = tx.send(AsyncMessage::ICloudEvents(all_events, start)).await;
                        });
                    }
                }
            }
            app.icloud_needs_fetch = false;
        }

        // Handle async messages (non-blocking)
        while let Ok(msg) = rx.try_recv() {
            match msg {
                // Google messages
                AsyncMessage::GoogleDeviceCode {
                    user_code,
                    verification_url,
                    device_code,
                    expires_at,
                } => {
                    app.google_auth = GoogleAuthState::AwaitingUserCode {
                        user_code,
                        verification_url,
                        device_code,
                        expires_at,
                    };
                }
                AsyncMessage::GoogleToken(tokens) => {
                    let _ = config::save_google_tokens(&tokens);
                    app.google_auth = GoogleAuthState::Authenticated(tokens);
                    app.google_needs_fetch = true;
                    app.status_message = Some("Connected to Google Calendar!".to_string());
                }
                AsyncMessage::GoogleAuthPending => {}
                AsyncMessage::GoogleAuthError(msg) => {
                    app.google_auth = GoogleAuthState::Error(msg);
                }
                AsyncMessage::GoogleEvents(events, month_date, calendar_id) => {
                    let display_events: Vec<DisplayEvent> = events
                        .into_iter()
                        .filter_map(|e| {
                            let mut attendees: Vec<DisplayAttendee> = e.attendees.as_ref().map(|atts| {
                                atts.iter()
                                    .filter_map(|a| {
                                        let email = a.email.clone()?;
                                        let status = if a.organizer == Some(true) {
                                            AttendeeStatus::Organizer
                                        } else {
                                            match a.response_status.as_deref() {
                                                Some("accepted") => AttendeeStatus::Accepted,
                                                Some("declined") => AttendeeStatus::Declined,
                                                Some("tentative") => AttendeeStatus::Tentative,
                                                _ => AttendeeStatus::NeedsAction,
                                            }
                                        };
                                        Some(DisplayAttendee {
                                            name: Some(a.display_name.clone()
                                                .unwrap_or_else(|| name_from_email(&email))),
                                            email,
                                            status,
                                        })
                                    })
                                    .collect()
                            }).unwrap_or_default();
                            sort_attendees(&mut attendees);

                            Some(DisplayEvent {
                                id: EventId::Google {
                                    calendar_id: calendar_id.clone(),
                                    event_id: e.id.clone(),
                                },
                                title: e.title().to_string(),
                                time_str: e.time_str(),
                                end_time_str: e.end_time_str(),
                                date: e.start_date()?,
                                accepted: e.is_accepted(),
                                meeting_url: e.meeting_url(),
                                description: e.description.clone(),
                                location: e.location.clone(),
                                attendees,
                            })
                        })
                        .collect();
                    app.events.google.store(display_events, month_date);
                    app.events.save_to_disk();
                    app.google_loading = false;
                }
                AsyncMessage::GoogleFetchError(msg) => {
                    app.status_message = Some(format!("Google: {}", msg));
                    app.google_loading = false;
                }
                AsyncMessage::GoogleTokenRefreshed(tokens) => {
                    let _ = config::save_google_tokens(&tokens);
                    app.google_auth = GoogleAuthState::Authenticated(tokens);
                    app.google_needs_fetch = true;
                    app.google_loading = false;
                }
                AsyncMessage::GoogleRefreshFailed(msg) => {
                    app.google_auth = GoogleAuthState::NotAuthenticated;
                    app.status_message = Some(format!("Token refresh failed: {}", msg));
                    app.google_loading = false;
                }

                // iCloud messages
                AsyncMessage::ICloudDiscovered { calendar_urls } => {
                    let _ = config::save_icloud_tokens(&calendar_urls);
                    let count = calendar_urls.len();
                    app.icloud_auth = ICloudAuthState::Authenticated { calendar_urls };
                    app.icloud_needs_fetch = true;
                    app.status_message = Some(format!("Connected to {} iCloud calendar(s)!", count));
                }
                AsyncMessage::ICloudDiscoveryError(msg) => {
                    app.icloud_auth = ICloudAuthState::Error(msg);
                }
                AsyncMessage::ICloudEvents(events, month_date) => {
                    let display_events: Vec<DisplayEvent> = events
                        .into_iter()
                        .map(|e| {
                            let mut attendees: Vec<DisplayAttendee> = e.attendees.iter()
                                .map(|a| {
                                    let status = if a.is_organizer {
                                        AttendeeStatus::Organizer
                                    } else {
                                        match a.partstat.as_str() {
                                            "ACCEPTED" => AttendeeStatus::Accepted,
                                            "DECLINED" => AttendeeStatus::Declined,
                                            "TENTATIVE" => AttendeeStatus::Tentative,
                                            _ => AttendeeStatus::NeedsAction,
                                        }
                                    };
                                    DisplayAttendee {
                                        name: Some(a.name.clone()
                                            .unwrap_or_else(|| name_from_email(&a.email))),
                                        email: a.email.clone(),
                                        status,
                                    }
                                })
                                .collect();
                            sort_attendees(&mut attendees);

                            DisplayEvent {
                                id: EventId::ICloud {
                                    calendar_url: e.calendar_url.clone(),
                                    event_uid: e.uid.clone(),
                                    etag: e.etag.clone(),
                                },
                                title: e.title().to_string(),
                                time_str: e.time_str(),
                                end_time_str: e.end_time_str(),
                                date: e.start_date(),
                                accepted: e.accepted,
                                meeting_url: e.meeting_url(),
                                description: e.description.clone(),
                                location: e.location.clone(),
                                attendees,
                            }
                        })
                        .collect();
                    app.events.icloud.store(display_events, month_date);
                    app.events.save_to_disk();
                    app.icloud_loading = false;
                }
                AsyncMessage::ICloudFetchError(msg) => {
                    app.status_message = Some(format!("iCloud: {}", msg));
                    app.icloud_loading = false;
                }

                // Event action messages
                AsyncMessage::EventActionSuccess(msg) => {
                    app.status_message = Some(msg);
                    // Refresh events to reflect the change
                    app.events.clear();
                    app.google_needs_fetch = true;
                    app.icloud_needs_fetch = true;
                    // Exit event mode after action
                    app.exit_event_mode();
                }
                AsyncMessage::EventActionError(msg) => {
                    app.status_message = Some(msg);
                }
            }
        }

        // Poll for Google device code if awaiting
        if let GoogleAuthState::AwaitingUserCode { ref device_code, expires_at, .. } = app.google_auth {
            if Utc::now() < expires_at {
                if let Some(ref google_config) = app.config.google {
                    let auth = GoogleAuth::new(google_config.clone());
                    let device_code = device_code.clone();
                    let tx = tx.clone();

                    tokio::spawn(async move {
                        tokio::time::sleep(StdDuration::from_secs(5)).await;
                        match auth.poll_for_token(&device_code).await {
                            Ok(google::auth::PollResult::Success(tokens)) => {
                                let _ = tx.send(AsyncMessage::GoogleToken(tokens)).await;
                            }
                            Ok(google::auth::PollResult::Pending) => {
                                let _ = tx.send(AsyncMessage::GoogleAuthPending).await;
                            }
                            Ok(google::auth::PollResult::Denied) => {
                                let _ = tx.send(AsyncMessage::GoogleAuthError("Access denied".to_string())).await;
                            }
                            Ok(google::auth::PollResult::Expired) => {
                                let _ = tx.send(AsyncMessage::GoogleAuthError("Code expired".to_string())).await;
                            }
                            Ok(google::auth::PollResult::SlowDown) => {
                                let _ = tx.send(AsyncMessage::GoogleAuthPending).await;
                            }
                            Err(e) => {
                                let _ = tx.send(AsyncMessage::GoogleAuthError(e.to_string())).await;
                            }
                        }
                    });
                }
            }
        }

        // Handle keyboard input with timeout
        if event::poll(StdDuration::from_millis(100))? {
            if let Event::Key(key_event) = event::read()? {
                if key_event.kind == KeyEventKind::Press {
                    // Handle Event navigation mode (month view only)
                    if app.navigation_mode == NavigationMode::Event && app.view_mode == ViewMode::Month {
                        match key_event.code {
                            KeyCode::Char('j') | KeyCode::Char('й') | KeyCode::Down => {
                                app.next_event();
                            }
                            KeyCode::Char('k') | KeyCode::Char('к') | KeyCode::Up => {
                                app.prev_event();
                            }
                            KeyCode::Char('o') | KeyCode::Char('о') => {
                                // Open meeting link
                                if let Some(event) = app.get_selected_event() {
                                    if let Some(ref url) = event.meeting_url {
                                        let _ = std::process::Command::new("xdg-open")
                                            .arg(url)
                                            .spawn();
                                    }
                                }
                            }
                            KeyCode::Char('a') | KeyCode::Char('а') => {
                                // Accept event (Google only)
                                if let Some(event) = app.get_selected_event() {
                                    if let EventId::Google { calendar_id, event_id } = event.id.clone() {
                                        if let GoogleAuthState::Authenticated(ref tokens) = app.google_auth {
                                            let tokens = tokens.clone();
                                            let tx = tx.clone();
                                            tokio::spawn(async move {
                                                let client = CalendarClient::new();
                                                match client.respond_to_event(&tokens, &calendar_id, &event_id, "accepted").await {
                                                    Ok(()) => {
                                                        let _ = tx.send(AsyncMessage::EventActionSuccess("Event accepted".to_string())).await;
                                                    }
                                                    Err(e) => {
                                                        let _ = tx.send(AsyncMessage::EventActionError(format!("Failed to accept: {}", e))).await;
                                                    }
                                                }
                                            });
                                            app.status_message = Some("Accepting event...".to_string());
                                        }
                                    } else {
                                        app.status_message = Some("Accept not supported for iCloud".to_string());
                                    }
                                }
                            }
                            KeyCode::Char('d') | KeyCode::Char('д') => {
                                // Decline event (Google only)
                                if let Some(event) = app.get_selected_event() {
                                    if let EventId::Google { calendar_id, event_id } = event.id.clone() {
                                        if let GoogleAuthState::Authenticated(ref tokens) = app.google_auth {
                                            let tokens = tokens.clone();
                                            let tx = tx.clone();
                                            tokio::spawn(async move {
                                                let client = CalendarClient::new();
                                                match client.respond_to_event(&tokens, &calendar_id, &event_id, "declined").await {
                                                    Ok(()) => {
                                                        let _ = tx.send(AsyncMessage::EventActionSuccess("Event declined".to_string())).await;
                                                    }
                                                    Err(e) => {
                                                        let _ = tx.send(AsyncMessage::EventActionError(format!("Failed to decline: {}", e))).await;
                                                    }
                                                }
                                            });
                                            app.status_message = Some("Declining event...".to_string());
                                        }
                                    } else {
                                        app.status_message = Some("Decline not supported for iCloud".to_string());
                                    }
                                }
                            }
                            KeyCode::Char('x') | KeyCode::Char('ь') => {
                                // Delete event
                                if let Some(event) = app.get_selected_event() {
                                    match event.id.clone() {
                                        EventId::Google { calendar_id, event_id } => {
                                            if let GoogleAuthState::Authenticated(ref tokens) = app.google_auth {
                                                let tokens = tokens.clone();
                                                let tx = tx.clone();
                                                tokio::spawn(async move {
                                                    let client = CalendarClient::new();
                                                    match client.delete_event(&tokens, &calendar_id, &event_id).await {
                                                        Ok(()) => {
                                                            let _ = tx.send(AsyncMessage::EventActionSuccess("Event deleted".to_string())).await;
                                                        }
                                                        Err(e) => {
                                                            let _ = tx.send(AsyncMessage::EventActionError(format!("Failed to delete: {}", e))).await;
                                                        }
                                                    }
                                                });
                                                app.status_message = Some("Deleting event...".to_string());
                                            }
                                        }
                                        EventId::ICloud { calendar_url, event_uid, etag } => {
                                            if let Some(ref icloud_config) = app.config.icloud {
                                                let auth = ICloudAuth::new(icloud_config.clone());
                                                let client = CalDavClient::new(auth);
                                                let tx = tx.clone();
                                                tokio::spawn(async move {
                                                    match client.delete_event(&calendar_url, &event_uid, etag.as_deref()).await {
                                                        Ok(()) => {
                                                            let _ = tx.send(AsyncMessage::EventActionSuccess("Event deleted".to_string())).await;
                                                        }
                                                        Err(e) => {
                                                            let _ = tx.send(AsyncMessage::EventActionError(format!("Failed to delete: {}", e))).await;
                                                        }
                                                    }
                                                });
                                                app.status_message = Some("Deleting event...".to_string());
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                app.exit_event_mode();
                            }
                            KeyCode::Char('q') | KeyCode::Char('я') => {
                                break;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Day navigation mode (default)
                    match key_event.code {
                        // Navigation keys (with Bulgarian Phonetic equivalents)
                        KeyCode::Char('j') | KeyCode::Char('й') | KeyCode::Down => {
                            app.next_week();
                        }
                        KeyCode::Char('k') | KeyCode::Char('к') | KeyCode::Up => {
                            app.prev_week();
                        }
                        KeyCode::Char('h') | KeyCode::Char('х') | KeyCode::Left => {
                            app.prev_day();
                        }
                        KeyCode::Char('l') | KeyCode::Char('л') | KeyCode::Right => {
                            app.next_day();
                        }
                        KeyCode::Enter => {
                            // Enter event mode in month view
                            if app.view_mode == ViewMode::Month {
                                app.enter_event_mode();
                            }
                        }
                        KeyCode::Char('t') | KeyCode::Char('т') => {
                            app.goto_today();
                        }
                        KeyCode::Char('r') | KeyCode::Char('р') => {
                            app.events.clear();
                            app.google_needs_fetch = true;
                            app.icloud_needs_fetch = true;
                            app.status_message = Some("Refreshing...".to_string());
                        }
                        KeyCode::Char('v') | KeyCode::Char('ж') => {
                            // Toggle between month and week view
                            app.view_mode = match app.view_mode {
                                ViewMode::Month => ViewMode::Week,
                                ViewMode::Week => ViewMode::Month,
                            };
                            // Exit event mode when switching views
                            app.exit_event_mode();
                        }
                        KeyCode::Char('s') | KeyCode::Char('с') => {
                            // Toggle weekends (only meaningful in week view)
                            app.show_weekends = !app.show_weekends;
                        }
                        KeyCode::Char('D') => {
                            // Toggle HTTP request logs display
                            app.show_logs = !app.show_logs;
                        }
                        KeyCode::Char('g') | KeyCode::Char('г') => {
                            // Start Google auth flow (only if not already authenticated)
                            if matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                // Already authenticated, ignore
                            } else if let Some(ref google_config) = app.config.google {
                                let auth = GoogleAuth::new(google_config.clone());
                                let tx = tx.clone();

                                tokio::spawn(async move {
                                    match auth.request_device_code().await {
                                        Ok(resp) => {
                                            let expires_at = Utc::now() + chrono::Duration::seconds(resp.expires_in as i64);
                                            let _ = tx.send(AsyncMessage::GoogleDeviceCode {
                                                user_code: resp.user_code,
                                                verification_url: resp.verification_url,
                                                device_code: resp.device_code,
                                                expires_at,
                                            }).await;
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AsyncMessage::GoogleAuthError(e.to_string())).await;
                                        }
                                    }
                                });
                            }
                        }
                        KeyCode::Char('i') | KeyCode::Char('и') => {
                            // Start iCloud discovery (only if not already authenticated)
                            if matches!(app.icloud_auth, ICloudAuthState::Authenticated { .. }) {
                                // Already authenticated, ignore
                            } else if let Some(ref icloud_config) = app.config.icloud {
                                app.icloud_auth = ICloudAuthState::Discovering;
                                let auth = ICloudAuth::new(icloud_config.clone());
                                let client = CalDavClient::new(auth);
                                let tx = tx.clone();

                                tokio::spawn(async move {
                                    match client.discover_calendars().await {
                                        Ok(calendars) => {
                                            let urls: Vec<String> = calendars.into_iter().map(|c| c.url).collect();
                                            if urls.is_empty() {
                                                let _ = tx.send(AsyncMessage::ICloudDiscoveryError(
                                                    "No calendars found".to_string()
                                                )).await;
                                            } else {
                                                let _ = tx.send(AsyncMessage::ICloudDiscovered {
                                                    calendar_urls: urls,
                                                }).await;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AsyncMessage::ICloudDiscoveryError(e.to_string())).await;
                                        }
                                    }
                                });
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Char('я') | KeyCode::Esc => {
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(
        stdout(),
        cursor::Show,
        Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )?;

    Ok(())
}
