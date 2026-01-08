mod cache;
mod config;
mod error;
mod google;
mod icloud;
mod ui;

use cache::{DisplayEvent, EventCache};
use chrono::{Datelike, DateTime, Duration, Local, NaiveDate, Utc};
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
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;
use ui::AuthDisplay;

/// View mode for the calendar
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    Month,
    Week,
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
    events: EventCache,
    google_auth: GoogleAuthState,
    icloud_auth: ICloudAuthState,
    status_message: Option<String>,
    config: Config,
    google_needs_fetch: bool,
    icloud_needs_fetch: bool,
    google_loading: bool,
    icloud_loading: bool,
}

impl App {
    fn new() -> Self {
        let today = Local::now().date_naive();
        let mut events = EventCache::new();
        // Load cached events from disk for instant display
        events.load_from_disk();

        Self {
            current_date: today,
            selected_date: today,
            view_mode: ViewMode::Month,
            show_weekends: false,
            events,
            google_auth: GoogleAuthState::NotConfigured,
            icloud_auth: ICloudAuthState::NotConfigured,
            status_message: None,
            config: Config::default(),
            google_needs_fetch: false,
            icloud_needs_fetch: false,
            google_loading: false,
            icloud_loading: false,
        }
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
    GoogleEvents(Vec<google::CalendarEvent>, NaiveDate),
    GoogleFetchError(String),
    GoogleTokenRefreshed(TokenInfo),
    GoogleRefreshFailed(String),

    // iCloud messages
    ICloudDiscovered { calendar_urls: Vec<String> },
    ICloudDiscoveryError(String),
    ICloudEvents(Vec<ICalEvent>, NaiveDate),
    ICloudFetchError(String),
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
                    tokio::spawn(async move {
                        let client = CalendarClient::new();
                        match client.list_events(&tokens, &calendar_id, start, end).await {
                            Ok(events) => {
                                let _ = tx.send(AsyncMessage::GoogleEvents(events, start)).await;
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
                AsyncMessage::GoogleEvents(events, month_date) => {
                    let display_events: Vec<DisplayEvent> = events
                        .into_iter()
                        .filter_map(|e| {
                            Some(DisplayEvent {
                                title: e.title().to_string(),
                                time_str: e.time_str(),
                                date: e.start_date()?,
                                accepted: e.is_accepted(),
                                meeting_url: e.meeting_url(),
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
                        .map(|e| DisplayEvent {
                            title: e.title().to_string(),
                            time_str: e.time_str(),
                            date: e.start_date(),
                            accepted: e.accepted,
                            meeting_url: e.meeting_url(),
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
                    match key_event.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            app.next_week();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.prev_week();
                        }
                        KeyCode::Char('h') | KeyCode::Left => {
                            app.prev_day();
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                            app.next_day();
                        }
                        KeyCode::Char('t') => {
                            app.goto_today();
                        }
                        KeyCode::Char('r') => {
                            app.events.clear();
                            app.google_needs_fetch = true;
                            app.icloud_needs_fetch = true;
                            app.status_message = Some("Refreshing...".to_string());
                        }
                        KeyCode::Char('v') => {
                            // Toggle between month and week view
                            app.view_mode = match app.view_mode {
                                ViewMode::Month => ViewMode::Week,
                                ViewMode::Week => ViewMode::Month,
                            };
                        }
                        KeyCode::Char('s') => {
                            // Toggle weekends (only meaningful in week view)
                            app.show_weekends = !app.show_weekends;
                        }
                        KeyCode::Char('g') => {
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
                        KeyCode::Char('i') => {
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
                        KeyCode::Char('q') | KeyCode::Esc => {
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
