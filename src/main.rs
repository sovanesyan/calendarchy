mod app;
mod auth;
mod cache;
mod config;
mod conversion;
mod error;
mod eventkit;
mod google;
mod headless;
mod icloud;
mod logging;
mod setup;
mod ui;
mod utils;

use app::{App, ICloudMethod, NavigationMode, PendingAction, SetupState, SetupStep};
use auth::{CalendarEntry, GoogleAuthState, ICloudAuthState};
use cache::{DisplayEvent, EventId};
use conversion::{google_event_to_display, icloud_event_to_display};
use chrono::{NaiveDate, Timelike};
use config::Config;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use google::{CalendarClient, GoogleAuth, TokenInfo};
use icloud::{CalDavClient, ICalEvent, ICloudAuth};
use std::io::stdout;
use std::time::Duration as StdDuration;
use utils::{open_url, to_zoom_deeplink};
use tokio::sync::mpsc;

/// Messages from async tasks to main loop
enum AsyncMessage {
    // Google messages
    GoogleToken(TokenInfo),
    GoogleAuthError(String),
    GoogleEvents(Vec<google::CalendarEvent>, NaiveDate, String, Option<String>), // events, month_date, calendar_id, calendar_name
    GoogleFetchError(String),
    GoogleTokenRefreshed(TokenInfo),
    GoogleRefreshFailed(String),

    // iCloud messages
    ICloudDiscovered { calendars: Vec<CalendarEntry> },
    ICloudDiscoveryError(String),
    ICloudEvents(Vec<(ICalEvent, Option<String>)>, NaiveDate), // Events with calendar name
    ICloudFetchError(String),

    // EventKit messages
    EventKitEvents(Vec<cache::DisplayEvent>, NaiveDate),
    EventKitError(String),

    // Event action messages
    EventActionSuccess(String), // Success message
    EventActionError(String),   // Error message
}

/// Start Google OAuth browser auth flow
fn start_google_auth(app: &mut app::App, google_config: config::GoogleConfig, tx: &mpsc::Sender<AsyncMessage>) {
    app.google_auth = GoogleAuthState::Authenticating;
    app.set_status("Opening browser for Google sign-in...");
    let auth = GoogleAuth::new(google_config);
    let url = auth.auth_url();
    open_url(&url);
    let tx = tx.clone();
    tokio::spawn(async move {
        match auth.authenticate_with_browser().await {
            Ok(tokens) => {
                let _ = tx.send(AsyncMessage::GoogleToken(tokens)).await;
            }
            Err(e) => {
                let _ = tx.send(AsyncMessage::GoogleAuthError(e.to_string())).await;
            }
        }
    });
}

/// Open the setup wizard, skipping to the first relevant step
fn open_setup_wizard(app: &mut app::App) {
    let mut setup = SetupState::new();
    setup.eventkit_available = eventkit::is_available();

    if app.config.google.is_some() || app.config.icloud.is_some() {
        // Re-entering via S key: skip to calendar config
        setup.step = SetupStep::GoogleAsk;
    } else if setup::should_show_shortcut_step() {
        setup.step = SetupStep::ShortcutAsk;
        #[cfg(target_os = "macos")]
        {
            setup.available_terminals = setup::detect_terminal_names();
        }
    } else {
        setup.step = SetupStep::Welcome;
    }

    app.setup = Some(setup);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("calendarchy {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if std::env::args().any(|a| a == "--refresh") {
        return headless::refresh().await;
    }

    #[cfg(target_os = "macos")]
    if std::env::args().any(|a| a == "--remove-setup") {
        return setup::remove_setup();
    }

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

    if let Some(ref icloud_config) = app.config.icloud {
        if icloud_config.is_eventkit() {
            // EventKit: no discovery needed, macOS handles auth
            app.icloud_auth = ICloudAuthState::Authenticated { calendars: vec![] };
            app.icloud_needs_fetch = true;
        } else {
            app.icloud_auth = ICloudAuthState::NotAuthenticated;
            // Try to load saved iCloud discovery info
            if let Ok(Some(icloud_tokens)) = config::load_icloud_tokens() {
                // Use new calendars field if available, fall back to legacy calendar_urls
                let calendars: Vec<CalendarEntry> = if !icloud_tokens.calendars.is_empty() {
                    icloud_tokens.calendars.into_iter()
                        .map(|c| CalendarEntry { url: c.url, name: c.name })
                        .collect()
                } else {
                    icloud_tokens.calendar_urls.into_iter()
                        .map(|url| CalendarEntry { url, name: None })
                        .collect()
                };
                if !calendars.is_empty() {
                    app.icloud_auth = ICloudAuthState::Authenticated { calendars };
                    app.icloud_needs_fetch = true;
                }
            }
        }
    }

    if app.config.google.is_none() && app.config.icloud.is_none() {
        open_setup_wizard(&mut app);
    }

    // Channel for async messages
    let (tx, mut rx) = mpsc::channel::<AsyncMessage>(32);

    // Spawn Google token refresh if needed
    if let Some(refresh_token) = google_needs_refresh
        && let Some(ref google_config) = app.config.google {
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

    // Auto-discover iCloud calendars if configured but no saved tokens
    if matches!(app.icloud_auth, ICloudAuthState::NotAuthenticated)
        && let Some(ref icloud_config) = app.config.icloud
        && !icloud_config.is_eventkit() {
            app.icloud_auth = ICloudAuthState::Discovering;
            let auth = ICloudAuth::new(icloud_config.clone());
            let client = CalDavClient::new(auth);
            let tx = tx.clone();
            tokio::spawn(async move {
                match client.discover_calendars().await {
                    Ok(discovered) => {
                        let calendars: Vec<CalendarEntry> = discovered
                            .into_iter()
                            .map(|c| CalendarEntry { url: c.url, name: c.name })
                            .collect();
                        if calendars.is_empty() {
                            let _ = tx.send(AsyncMessage::ICloudDiscoveryError(
                                "No calendars found".to_string()
                            )).await;
                        } else {
                            let _ = tx.send(AsyncMessage::ICloudDiscovered { calendars }).await;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AsyncMessage::ICloudDiscoveryError(e.to_string())).await;
                    }
                }
            });
        }

    // Learn the terminal background (OSC 11) before entering raw mode so the
    // heatmap can derive theme-adaptive shades; falls back to dark if unanswered
    if let Ok(rgb) = termbg::rgb(StdDuration::from_millis(120)) {
        ui::set_term_bg((rgb.r >> 8) as u8, (rgb.g >> 8) as u8, (rgb.b >> 8) as u8);
    }

    // Enable raw mode and enter alternate screen
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    // Main loop
    loop {
        // Clear expired status messages
        if app.clear_expired_status() {
            app.dirty = true;
        }

        // Re-render once per minute for the countdown timer
        let now_minute = chrono::Local::now().minute();
        if now_minute != app.last_render_minute {
            app.last_render_minute = now_minute;
            app.dirty = true;
        }

        // Render only when something changed
        if app.dirty {
            app.dirty = false;
            let render_state = ui::RenderState {
                current_date: app.current_date,
                selected_date: app.selected_date,
                events: &app.events,
                google_auth: &app.google_auth,
                icloud_auth: &app.icloud_auth,
                status_message: app.status_message.as_deref(),
                status_is_error: app.status_is_error,
                google_loading: app.google_loading,
                icloud_loading: app.icloud_loading,
                navigation_mode: app.navigation_mode,
                selected_source: app.selected_source,
                selected_event_index: app.selected_event_index,
                show_logs: app.show_logs,
                pending_action: app.pending_action.as_ref(),
                search: app.search.as_ref(),
                show_help: app.show_help,
                setup: app.setup.as_ref(),
            };
            ui::render(&render_state);
        }

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
                    app.dirty = true;
                    let calendar_id_clone = calendar_id.clone();
                    tokio::spawn(async move {
                        let client = CalendarClient::new();
                        // Get calendar display name
                        let calendar_name = client.get_calendar_name(&tokens, &calendar_id).await.ok().flatten();
                        match client.list_events(&tokens, &calendar_id, start, end).await {
                            Ok(events) => {
                                let _ = tx.send(AsyncMessage::GoogleEvents(events, start, calendar_id_clone, calendar_name)).await;
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
            if let ICloudAuthState::Authenticated { ref calendars } = app.icloud_auth {
                let (start, end) = app.month_range();
                if !app.events.icloud.has_month(start)
                    && let Some(ref icloud_config) = app.config.icloud {
                        app.icloud_loading = true;
                        app.dirty = true;

                        if icloud_config.is_eventkit() {
                            // EventKit: run in blocking task since it shells out
                            let tx = tx.clone();
                            tokio::spawn(async move {
                                let result = tokio::task::spawn_blocking(move || {
                                    eventkit::fetch_events(start, end)
                                }).await;
                                match result {
                                    Ok(Ok(events)) => {
                                        let _ = tx.send(AsyncMessage::EventKitEvents(events, start)).await;
                                    }
                                    Ok(Err(e)) => {
                                        let _ = tx.send(AsyncMessage::EventKitError(e)).await;
                                    }
                                    Err(e) => {
                                        let _ = tx.send(AsyncMessage::EventKitError(e.to_string())).await;
                                    }
                                }
                            });
                        } else {
                            // CalDAV
                            let auth = ICloudAuth::new(icloud_config.clone());
                            let client = CalDavClient::new(auth);
                            let calendars = calendars.clone();
                            let tx = tx.clone();

                            tokio::spawn(async move {
                                let mut all_events: Vec<(ICalEvent, Option<String>)> = Vec::new();
                                for cal in &calendars {
                                    match client.fetch_events(&cal.url, start, end).await {
                                        Ok(events) => {
                                            for e in events {
                                                all_events.push((e, cal.name.clone()));
                                            }
                                        }
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
            app.dirty = true;
            match msg {
                // Google messages
                AsyncMessage::GoogleToken(tokens) => {
                    let _ = config::save_google_tokens(&tokens);
                    app.google_auth = GoogleAuthState::Authenticated(tokens);
                    app.google_needs_fetch = true;
                    app.set_status("Connected to Google Calendar!");
                    // Advance setup wizard past auth waiting
                    if let Some(ref mut setup) = app.setup {
                        if setup.step == SetupStep::GoogleAuthWaiting {
                            setup.step = SetupStep::ICloudAsk;
                        }
                    }
                }
                AsyncMessage::GoogleAuthError(msg) => {
                    app.google_auth = GoogleAuthState::Error(msg.clone());
                    app.set_error(format!("Google: {}", msg));
                    // Advance setup wizard past auth waiting on error too
                    if let Some(ref mut setup) = app.setup {
                        if setup.step == SetupStep::GoogleAuthWaiting {
                            setup.error = Some(format!("Google auth failed: {}", msg));
                            setup.step = SetupStep::ICloudAsk;
                        }
                    }
                }
                AsyncMessage::GoogleEvents(events, month_date, calendar_id, calendar_name) => {
                    let display_events: Vec<DisplayEvent> = events
                        .into_iter()
                        .filter_map(|e| google_event_to_display(e, calendar_id.clone(), calendar_name.clone()))
                        .collect();
                    app.events.google.store(display_events, month_date);
                    app.events.save_to_disk();
                    app.google_loading = false;
                }
                AsyncMessage::GoogleFetchError(msg) => {
                    app.set_error(format!("Google: {}", msg));
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
                    app.set_error(format!("Token refresh failed: {}", msg));
                    app.google_loading = false;
                }

                // iCloud messages
                AsyncMessage::ICloudDiscovered { calendars } => {
                    let stored: Vec<config::StoredCalendar> = calendars.iter()
                        .map(|c| config::StoredCalendar { url: c.url.clone(), name: c.name.clone() })
                        .collect();
                    let _ = config::save_icloud_tokens(&stored);
                    let count = calendars.len();
                    app.icloud_auth = ICloudAuthState::Authenticated { calendars };
                    app.icloud_needs_fetch = true;
                    app.set_status(format!("Connected to {} iCloud calendar(s)!", count));
                }
                AsyncMessage::ICloudDiscoveryError(msg) => {
                    app.set_error(format!("iCloud: {}", msg));
                    app.icloud_auth = ICloudAuthState::Error(msg);
                }
                AsyncMessage::ICloudEvents(events, month_date) => {
                    let display_events: Vec<DisplayEvent> = events
                        .into_iter()
                        .map(|(e, calendar_name)| icloud_event_to_display(e, calendar_name))
                        .collect();
                    app.events.icloud.store(display_events, month_date);
                    app.events.save_to_disk();
                    app.icloud_loading = false;
                }
                AsyncMessage::ICloudFetchError(msg) => {
                    app.set_error(format!("iCloud: {}", msg));
                    app.icloud_loading = false;
                }

                // EventKit messages
                AsyncMessage::EventKitEvents(events, month_date) => {
                    app.events.icloud.store(events, month_date);
                    app.events.save_to_disk();
                    app.icloud_loading = false;
                }
                AsyncMessage::EventKitError(msg) => {
                    app.set_error(format!("EventKit: {}", msg));
                    app.icloud_loading = false;
                }

                // Event action messages
                AsyncMessage::EventActionSuccess(msg) => {
                    app.set_status(msg);
                    // Refresh events to reflect the change
                    app.events.clear();
                    app.google_needs_fetch = true;
                    app.icloud_needs_fetch = true;
                    // Exit event mode after action
                    app.exit_event_mode();
                }
                AsyncMessage::EventActionError(msg) => {
                    app.set_error(msg);
                }
            }
        }

        // Handle input events with timeout
        if event::poll(StdDuration::from_millis(100))? {
            match event::read()? {
                Event::Resize(_, _) => {
                    app.dirty = true;
                }
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    app.dirty = true;
                    // Sticky error messages are dismissed by any keypress
                    app.clear_error();
                    // Handle interactive setup wizard
                    if app.setup.is_some() {
                        match handle_setup_input(&mut app, key_event.code, &tx) {
                            SetupAction::Continue => {}
                            SetupAction::Quit => break,
                            SetupAction::Finished => {
                                // Reload config and initialize auth states
                                app.config = Config::load().unwrap_or_default();
                                app.setup = None;

                                // Auto-start Google browser auth if newly configured
                                if !matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                    if let Some(gc) = app.config.google.clone() {
                                        start_google_auth(&mut app, gc, &tx);
                                    }
                                }

                                // Auto-start iCloud discovery if newly configured
                                if let Some(ref icloud_config) = app.config.icloud {
                                    if matches!(app.icloud_auth, ICloudAuthState::NotConfigured | ICloudAuthState::NotAuthenticated) {
                                        if icloud_config.is_eventkit() {
                                            app.icloud_auth = ICloudAuthState::Authenticated { calendars: vec![] };
                                            app.icloud_needs_fetch = true;
                                        } else {
                                            app.icloud_auth = ICloudAuthState::Discovering;
                                            let auth = ICloudAuth::new(icloud_config.clone());
                                            let client = CalDavClient::new(auth);
                                            let tx = tx.clone();
                                            tokio::spawn(async move {
                                                match client.discover_calendars().await {
                                                    Ok(discovered) => {
                                                        let calendars: Vec<CalendarEntry> = discovered
                                                            .into_iter()
                                                            .map(|c| CalendarEntry { url: c.url, name: c.name })
                                                            .collect();
                                                        if calendars.is_empty() {
                                                            let _ = tx.send(AsyncMessage::ICloudDiscoveryError(
                                                                "No calendars found".to_string()
                                                            )).await;
                                                        } else {
                                                            let _ = tx.send(AsyncMessage::ICloudDiscovered { calendars }).await;
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let _ = tx.send(AsyncMessage::ICloudDiscoveryError(e.to_string())).await;
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    // Help overlay: any key closes it
                    if app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    // Handle search mode input first
                    if app.search.is_some() {
                        match key_event.code {
                            KeyCode::Esc => {
                                app.close_search();
                            }
                            KeyCode::Enter => {
                                app.select_search_result();
                            }
                            KeyCode::Backspace => {
                                if let Some(ref mut search) = app.search {
                                    search.query.pop();
                                }
                                app.update_search_results();
                            }
                            KeyCode::Down | KeyCode::Tab => {
                                if let Some(ref mut search) = app.search {
                                    if !search.results.is_empty() {
                                        search.selected_index = (search.selected_index + 1).min(search.results.len() - 1);
                                    }
                                }
                            }
                            KeyCode::Up | KeyCode::BackTab => {
                                if let Some(ref mut search) = app.search {
                                    search.selected_index = search.selected_index.saturating_sub(1);
                                }
                            }
                            KeyCode::Char(c) => {
                                if let Some(ref mut search) = app.search {
                                    search.query.push(c);
                                }
                                app.update_search_results();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Handle pending confirmation first
                    if let Some(action) = app.pending_action.take() {
                        match key_event.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                                // Execute the confirmed action
                                match action {
                                    PendingAction::AcceptEvent { calendar_id, event_id } => {
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
                                            app.set_status("Accepting event...");
                                        }
                                    }
                                    PendingAction::DeclineEvent { calendar_id, event_id } => {
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
                                            app.set_status("Declining event...");
                                        }
                                    }
                                    PendingAction::DeleteGoogleEvent { calendar_id, event_id } => {
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
                                            app.set_status("Deleting event...");
                                        }
                                    }
                                    PendingAction::DeleteICloudEvent { calendar_url, event_uid, etag } => {
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
                                            app.set_status("Deleting event...");
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                // Cancel - action already taken from pending_action
                                app.set_status("Cancelled");
                            }
                            _ => {
                                // Put the action back if not confirmed/cancelled
                                app.pending_action = Some(action);
                            }
                        }
                        continue;
                    }

                    // Handle Event navigation mode
                    if app.navigation_mode == NavigationMode::Event {
                        match (key_event.code, key_event.modifiers) {
                            (KeyCode::Char('j') | KeyCode::Char('й') | KeyCode::Down, _) => {
                                app.next_event();
                            }
                            (KeyCode::Char('k') | KeyCode::Char('к') | KeyCode::Up, _) => {
                                app.prev_event();
                            }
                            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                // Scroll down 10 events; flash where we landed since this can jump days
                                for _ in 0..10 {
                                    app.next_event();
                                }
                                app.set_status(format!("Jumped to {}", app.selected_date.format("%a %b %d")));
                            }
                            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                // Scroll up 10 events; flash where we landed since this can jump days
                                for _ in 0..10 {
                                    app.prev_event();
                                }
                                app.set_status(format!("Jumped to {}", app.selected_date.format("%a %b %d")));
                            }
                            (KeyCode::Char('L') | KeyCode::Char('Л'), _) => {
                                // Month jump invalidates the event selection, so drop to Day mode
                                app.next_month();
                                app.exit_event_mode();
                            }
                            (KeyCode::Char('H') | KeyCode::Char('Х'), _) => {
                                app.prev_month();
                                app.exit_event_mode();
                            }
                            (KeyCode::Char('J') | KeyCode::Char('Й'), _) => {
                                // Join meeting; Zoom links deep-link into the app instead of the browser
                                if let Some(event) = app.get_selected_event()
                                    && let Some(ref url) = event.meeting_url {
                                        let url = to_zoom_deeplink(url).unwrap_or_else(|| url.clone());
                                        open_url(&url);
                                    }
                            }
                            (KeyCode::Char('a') | KeyCode::Char('а'), _) => {
                                // Accept event (Google only) - set pending action
                                if let Some(event) = app.get_selected_event() {
                                    if let EventId::Google { calendar_id, event_id, .. } = event.id.clone() {
                                        if matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                            app.pending_action = Some(PendingAction::AcceptEvent { calendar_id, event_id });
                                        } else {
                                            app.set_status("Not signed in to Google — press S for setup");
                                        }
                                    } else {
                                        app.set_status("Accept not supported for iCloud");
                                    }
                                }
                            }
                            (KeyCode::Char('d') | KeyCode::Char('д'), m) if !m.contains(KeyModifiers::CONTROL) => {
                                // Decline event (Google only) - set pending action
                                if let Some(event) = app.get_selected_event() {
                                    if let EventId::Google { calendar_id, event_id, .. } = event.id.clone() {
                                        if matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                            app.pending_action = Some(PendingAction::DeclineEvent { calendar_id, event_id });
                                        } else {
                                            app.set_status("Not signed in to Google — press S for setup");
                                        }
                                    } else {
                                        app.set_status("Decline not supported for iCloud");
                                    }
                                }
                            }
                            (KeyCode::Char('x') | KeyCode::Char('ь'), _) => {
                                // Delete event - set pending action
                                if let Some(event) = app.get_selected_event() {
                                    match event.id.clone() {
                                        EventId::Google { calendar_id, event_id, .. } => {
                                            if matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                                app.pending_action = Some(PendingAction::DeleteGoogleEvent { calendar_id, event_id });
                                            } else {
                                                app.set_status("Not signed in to Google — press S for setup");
                                            }
                                        }
                                        EventId::ICloud { calendar_url, event_uid, etag, .. } => {
                                            if app.config.icloud.is_some() {
                                                app.pending_action = Some(PendingAction::DeleteICloudEvent { calendar_url, event_uid, etag });
                                            } else {
                                                app.set_status("iCloud not configured — press S for setup");
                                            }
                                        }
                                    }
                                }
                            }
                            (KeyCode::Char('t') | KeyCode::Char('т'), _) => {
                                app.goto_today();
                            }
                            (KeyCode::Char('r') | KeyCode::Char('р'), _) => {
                                app.events.clear();
                                app.google_needs_fetch = true;
                                app.icloud_needs_fetch = true;
                                app.set_status("Refreshing...");
                            }
                            (KeyCode::Char('n') | KeyCode::Char('н'), _) => {
                                app.goto_now();
                            }
                            (KeyCode::Esc, _) => {
                                app.exit_event_mode();
                            }
                            (KeyCode::Char('D') | KeyCode::Char('Д'), _) => {
                                app.show_logs = !app.show_logs;
                            }
                            (KeyCode::Char('f') | KeyCode::Char('ф'), _) => {
                                app.open_search();
                            }
                            (KeyCode::Char('?'), _) => {
                                app.show_help = true;
                            }
                            (KeyCode::Char('1'), _) => {
                                open_url("https://calendar.google.com");
                            }
                            (KeyCode::Char('2'), _) => {
                                open_url("https://www.icloud.com/calendar");
                            }
                            (KeyCode::Char('S') | KeyCode::Char('С'), _) => {
                                open_setup_wizard(&mut app);
                            }
                            (KeyCode::Char('q') | KeyCode::Char('я'), _) => {
                                break;
                            }
                            _ => {}
                        }
                        continue;
                    }


                    // Day navigation mode (default)
                    match (key_event.code, key_event.modifiers) {
                        // Navigation matches the month grid (with Bulgarian Phonetic equivalents):
                        // h/l move a day sideways, j/k move a week down/up
                        (KeyCode::Char('l') | KeyCode::Char('л') | KeyCode::Right, _) => {
                            app.next_day();
                        }
                        (KeyCode::Char('h') | KeyCode::Char('х') | KeyCode::Left, _) => {
                            app.prev_day();
                        }
                        (KeyCode::Char('j') | KeyCode::Char('й') | KeyCode::Down, _) => {
                            app.next_week();
                        }
                        (KeyCode::Char('k') | KeyCode::Char('к') | KeyCode::Up, _) => {
                            app.prev_week();
                        }
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            app.next_month();
                        }
                        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                            app.prev_month();
                        }
                        (KeyCode::Char('L') | KeyCode::Char('Л'), _) => {
                            app.next_month();
                        }
                        (KeyCode::Char('H') | KeyCode::Char('Х'), _) => {
                            app.prev_month();
                        }
                        (KeyCode::Enter, _) => {
                            if app.events.has_events(app.selected_date) {
                                app.enter_event_mode();
                            } else {
                                app.set_status("No events on this day");
                            }
                        }
                        (KeyCode::Char('t') | KeyCode::Char('т'), _) => {
                            app.goto_today();
                        }
                        (KeyCode::Char('r') | KeyCode::Char('р'), _) => {
                            app.events.clear();
                            app.google_needs_fetch = true;
                            app.icloud_needs_fetch = true;
                            app.set_status("Refreshing...");
                        }
                        (KeyCode::Char('n') | KeyCode::Char('н'), _) => {
                            app.goto_now();
                        }
                        (KeyCode::Char('D') | KeyCode::Char('Д'), _) => {
                            // Toggle HTTP request logs display
                            app.show_logs = !app.show_logs;
                        }
                        (KeyCode::Char('f') | KeyCode::Char('ф'), _) => {
                            app.open_search();
                        }
                        (KeyCode::Char('?'), _) => {
                            app.show_help = true;
                        }
                        (KeyCode::Char('1'), _) => {
                            open_url("https://calendar.google.com");
                        }
                        (KeyCode::Char('2'), _) => {
                            open_url("https://www.icloud.com/calendar");
                        }
                        (KeyCode::Char('g') | KeyCode::Char('г'), _) => {
                            if !matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                if let Some(gc) = app.config.google.clone() {
                                    start_google_auth(&mut app, gc, &tx);
                                } else {
                                    app.set_status("Google not configured — press S for setup");
                                }
                            }
                        }
                        (KeyCode::Char('S') | KeyCode::Char('С'), _) => {
                            open_setup_wizard(&mut app);
                        }
                        (KeyCode::Char('i') | KeyCode::Char('и'), _) if app.config.icloud.is_none() => {
                            app.set_status("iCloud not configured — press S for setup");
                        }
                        (KeyCode::Char('i') | KeyCode::Char('и'), _) => {
                            // Start iCloud discovery (re-run to refresh calendar names)
                            if let Some(ref icloud_config) = app.config.icloud {
                                app.icloud_auth = ICloudAuthState::Discovering;
                                let auth = ICloudAuth::new(icloud_config.clone());
                                let client = CalDavClient::new(auth);
                                let tx = tx.clone();

                                tokio::spawn(async move {
                                    match client.discover_calendars().await {
                                        Ok(discovered) => {
                                            let calendars: Vec<CalendarEntry> = discovered
                                                .into_iter()
                                                .map(|c| CalendarEntry { url: c.url, name: c.name })
                                                .collect();
                                            if calendars.is_empty() {
                                                let _ = tx.send(AsyncMessage::ICloudDiscoveryError(
                                                    "No calendars found".to_string()
                                                )).await;
                                            } else {
                                                let _ = tx.send(AsyncMessage::ICloudDiscovered { calendars }).await;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AsyncMessage::ICloudDiscoveryError(e.to_string())).await;
                                        }
                                    }
                                });
                            }
                        }
                        // Esc deliberately does NOT quit: it means "back" everywhere else,
                        // and double-Esc from Event mode would exit the app by accident
                        (KeyCode::Char('q') | KeyCode::Char('я'), _) => {
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, cursor::Show)?;

    Ok(())
}

enum SetupAction {
    Continue,
    Quit,
    Finished,
}

fn handle_setup_input(app: &mut App, key: KeyCode, tx: &mpsc::Sender<AsyncMessage>) -> SetupAction {
    // Handle Google auth start separately to avoid borrow conflicts
    if let Some(ref setup) = app.setup {
        if setup.step == SetupStep::GoogleAsk
            && matches!(key, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter)
        {
            // Save config and start auth
            if app.config.google.is_none() {
                app.config.google = Some(config::GoogleConfig::default());
            }
            let _ = app.config.save();
            if let Some(gc) = app.config.google.clone() {
                start_google_auth(app, gc, tx);
            }
            let setup = app.setup.as_mut().unwrap();
            setup.google_enabled = true;
            setup.step = SetupStep::GoogleAuthWaiting;
            return SetupAction::Continue;
        }
    }

    let setup = app.setup.as_mut().unwrap();
    setup.error = None;

    match setup.step {
        SetupStep::ShortcutAsk => match key {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                #[cfg(target_os = "macos")]
                {
                    if setup.available_terminals.is_empty() {
                        setup.error = Some("No supported terminals found".to_string());
                    } else {
                        setup.step = SetupStep::ShortcutTerminalChoice;
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    match setup::install_shortcut() {
                        Ok(()) => setup.step = SetupStep::Welcome,
                        Err(e) => setup.error = Some(format!("Failed: {}", e)),
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                setup.step = SetupStep::Welcome;
            }
            KeyCode::Char('q') | KeyCode::Esc => return SetupAction::Quit,
            _ => {}
        },
        SetupStep::ShortcutTerminalChoice => match key {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as u8 - b'1') as usize;
                if idx < setup.available_terminals.len() {
                    #[cfg(target_os = "macos")]
                    match setup::install_shortcut(idx) {
                        Ok(()) => setup.step = SetupStep::Welcome,
                        Err(e) => setup.error = Some(e),
                    }
                }
            }
            KeyCode::Esc => { setup.step = SetupStep::ShortcutAsk; }
            _ => {}
        },
        SetupStep::Welcome => match key {
            KeyCode::Enter => {
                setup.step = SetupStep::GoogleAsk;
            }
            KeyCode::Char('q') | KeyCode::Esc => return SetupAction::Quit,
            _ => {}
        },
        SetupStep::GoogleAsk => match key {
            KeyCode::Char('n') | KeyCode::Char('N') => {
                setup.step = SetupStep::ICloudAsk;
            }
            KeyCode::Esc => { setup.step = SetupStep::Welcome; }
            _ => {}
        },
        SetupStep::GoogleAuthWaiting => match key {
            KeyCode::Esc => {
                setup.step = SetupStep::ICloudAsk;
            }
            _ => {}
        },
        SetupStep::ICloudAsk => match key {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if setup.eventkit_available {
                    setup.step = SetupStep::ICloudMethod;
                } else {
                    setup.step = SetupStep::ICloudOpenUrl;
                    open_url("https://appleid.apple.com/account/manage");
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                setup.step = SetupStep::Done;
            }
            KeyCode::Esc => {
                setup.step = SetupStep::GoogleAsk;
            }
            _ => {}
        },
        SetupStep::ICloudMethod => match key {
            KeyCode::Char('1') | KeyCode::Enter => {
                // EventKit - zero config, done
                setup.icloud_method = Some(ICloudMethod::EventKit);
                setup.step = SetupStep::Done;
            }
            KeyCode::Char('2') => {
                // CalDAV - need credentials
                setup.icloud_method = Some(ICloudMethod::CalDav);
                setup.step = SetupStep::ICloudOpenUrl;
                open_url("https://appleid.apple.com/account/manage");
            }
            KeyCode::Esc => { setup.step = SetupStep::ICloudAsk; }
            _ => {}
        },
        SetupStep::ICloudOpenUrl => match key {
            KeyCode::Enter => { setup.step = SetupStep::ICloudAppleId; }
            KeyCode::Esc => {
                if setup.eventkit_available {
                    setup.step = SetupStep::ICloudMethod;
                } else {
                    setup.step = SetupStep::ICloudAsk;
                }
            }
            _ => {}
        },
        SetupStep::ICloudAppleId => match key {
            KeyCode::Enter => {
                let val = setup.input.trim().to_string();
                if val.is_empty() {
                    setup.error = Some("Apple ID cannot be empty".to_string());
                } else {
                    setup.icloud_apple_id = Some(val);
                    setup.input.clear();
                    setup.step = SetupStep::ICloudPassword;
                }
            }
            KeyCode::Esc => {
                setup.input.clear();
                setup.step = SetupStep::ICloudOpenUrl;
            }
            KeyCode::Backspace => { setup.input.pop(); }
            KeyCode::Char(c) => { setup.input.push(c); }
            _ => {}
        },
        SetupStep::ICloudPassword => match key {
            KeyCode::Enter => {
                let val = setup.input.trim().to_string();
                if val.is_empty() {
                    setup.error = Some("App password cannot be empty".to_string());
                } else {
                    setup.icloud_password = Some(val);
                    setup.input.clear();
                    setup.step = SetupStep::Done;
                }
            }
            KeyCode::Esc => {
                setup.input.clear();
                setup.step = SetupStep::ICloudAppleId;
            }
            KeyCode::Backspace => { setup.input.pop(); }
            KeyCode::Char(c) => { setup.input.push(c); }
            _ => {}
        },
        SetupStep::Done => {}
    }

    // Save config when we reach Done
    if setup.step == SetupStep::Done {
        let has_google = setup.google_enabled;
        let has_eventkit = setup.icloud_method == Some(ICloudMethod::EventKit);
        let has_caldav = setup.icloud_apple_id.is_some();
        let already_has_google = app.config.google.is_some();
        let already_has_icloud = app.config.icloud.is_some();

        if !has_google && !has_eventkit && !has_caldav && !already_has_google && !already_has_icloud {
            setup.error = Some("Set up at least one calendar".to_string());
            setup.step = SetupStep::GoogleAsk;
            return SetupAction::Continue;
        }

        let mut config = app.config.clone();
        if has_google && config.google.is_none() {
            config.google = Some(config::GoogleConfig::default());
        }
        if has_eventkit {
            config.icloud = Some(config::ICloudConfig {
                method: "eventkit".to_string(),
                apple_id: None,
                app_password: None,
            });
        } else if let (Some(apple_id), Some(app_password)) =
            (setup.icloud_apple_id.take(), setup.icloud_password.take())
        {
            config.icloud = Some(config::ICloudConfig {
                method: "caldav".to_string(),
                apple_id: Some(apple_id),
                app_password: Some(app_password),
            });
        }

        if let Err(e) = config.save() {
            setup.error = Some(format!("Failed to save config: {}", e));
            setup.step = SetupStep::GoogleAsk;
            return SetupAction::Continue;
        }

        return SetupAction::Finished;
    }

    SetupAction::Continue
}
