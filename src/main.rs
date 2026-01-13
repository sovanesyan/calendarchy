mod app;
mod auth;
mod cache;
mod config;
mod conversion;
mod error;
mod google;
mod icloud;
mod logging;
mod ui;
mod utils;

use app::{App, NavigationMode, PendingAction};
use auth::{CalendarEntry, GoogleAuthState, ICloudAuthState};
use cache::{DisplayEvent, EventId};
use conversion::{google_event_to_display, icloud_event_to_display};
use chrono::{DateTime, NaiveDate, Utc};
use config::Config;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use google::{CalendarClient, GoogleAuth, TokenInfo};
use icloud::{CalDavClient, ICalEvent, ICloudAuth};
use std::io::stdout;
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;

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
    GoogleEvents(Vec<google::CalendarEvent>, NaiveDate, String, Option<String>), // events, month_date, calendar_id, calendar_name
    GoogleFetchError(String),
    GoogleTokenRefreshed(TokenInfo),
    GoogleRefreshFailed(String),

    // iCloud messages
    ICloudDiscovered { calendars: Vec<CalendarEntry> },
    ICloudDiscoveryError(String),
    ICloudEvents(Vec<(ICalEvent, Option<String>)>, NaiveDate), // Events with calendar name
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

    if app.config.google.is_none() && app.config.icloud.is_none() {
        app.set_status("No calendars configured. Edit ~/.config/calendarchy/config.json");
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

    // Enable raw mode and enter alternate screen
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, cursor::Hide)?;

    // Main loop
    loop {
        // Clear expired status messages
        app.clear_expired_status();

        // Render
        let render_state = ui::RenderState {
            current_date: app.current_date,
            selected_date: app.selected_date,
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
            show_weekends: app.show_weekends,
            pending_action: app.pending_action.as_ref(),
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
                        let auth = ICloudAuth::new(icloud_config.clone());
                        let client = CalDavClient::new(auth);
                        let calendars = calendars.clone();
                        let tx = tx.clone();

                        app.icloud_loading = true;
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
                    app.set_status("Connected to Google Calendar!");
                }
                AsyncMessage::GoogleAuthPending => {}
                AsyncMessage::GoogleAuthError(msg) => {
                    app.google_auth = GoogleAuthState::Error(msg);
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
                    app.set_status(format!("Google: {}", msg));
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
                    app.set_status(format!("Token refresh failed: {}", msg));
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
                    app.set_status(format!("iCloud: {}", msg));
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
                    app.set_status(msg);
                }
            }
        }

        // Poll for Google device code if awaiting
        if let GoogleAuthState::AwaitingUserCode { ref device_code, expires_at, .. } = app.google_auth
            && Utc::now() < expires_at
                && let Some(ref google_config) = app.config.google {
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

        // Handle input events with timeout
        if event::poll(StdDuration::from_millis(100))? {
            match event::read()? {
                Event::Resize(_, _) => {
                    // Clear screen on resize - next loop iteration will re-render
                    execute!(stdout(), Clear(ClearType::All)).ok();
                }
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
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
                                // Scroll down 10 events
                                for _ in 0..10 {
                                    app.next_event();
                                }
                            }
                            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                // Scroll up 10 events
                                for _ in 0..10 {
                                    app.prev_event();
                                }
                            }
                            (KeyCode::Char('J'), _) => {
                                // Join meeting
                                if let Some(event) = app.get_selected_event()
                                    && let Some(ref url) = event.meeting_url {
                                        let _ = std::process::Command::new("xdg-open")
                                            .arg(url)
                                            .spawn();
                                    }
                            }
                            (KeyCode::Char('a') | KeyCode::Char('а'), _) => {
                                // Accept event (Google only) - set pending action
                                if let Some(event) = app.get_selected_event() {
                                    if let EventId::Google { calendar_id, event_id, .. } = event.id.clone() {
                                        if matches!(app.google_auth, GoogleAuthState::Authenticated(_)) {
                                            app.pending_action = Some(PendingAction::AcceptEvent { calendar_id, event_id });
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
                                            }
                                        }
                                        EventId::ICloud { calendar_url, event_uid, etag, .. } => {
                                            if app.config.icloud.is_some() {
                                                app.pending_action = Some(PendingAction::DeleteICloudEvent { calendar_url, event_uid, etag });
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
                            (KeyCode::Char('D'), _) => {
                                app.show_logs = !app.show_logs;
                            }
                            (KeyCode::Char('w') | KeyCode::Char('ц'), _) => {
                                app.show_weekends = !app.show_weekends;
                                execute!(stdout(), Clear(ClearType::All)).ok();
                            }
                            (KeyCode::Char('1'), _) => {
                                let _ = std::process::Command::new("xdg-open")
                                    .arg("https://calendar.google.com")
                                    .spawn();
                            }
                            (KeyCode::Char('2'), _) => {
                                let _ = std::process::Command::new("xdg-open")
                                    .arg("https://www.icloud.com/calendar")
                                    .spawn();
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
                        // Navigation keys (with Bulgarian Phonetic equivalents)
                        (KeyCode::Char('j') | KeyCode::Char('й') | KeyCode::Down, _) => {
                            app.next_day();
                        }
                        (KeyCode::Char('k') | KeyCode::Char('к') | KeyCode::Up, _) => {
                            app.prev_day();
                        }
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            app.next_month();
                        }
                        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                            app.prev_month();
                        }
                        (KeyCode::Enter, _) => {
                            app.enter_event_mode();
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
                        (KeyCode::Char('D'), _) => {
                            // Toggle HTTP request logs display
                            app.show_logs = !app.show_logs;
                        }
                        (KeyCode::Char('w') | KeyCode::Char('ц'), _) => {
                            // Toggle weekend visibility
                            app.show_weekends = !app.show_weekends;
                            execute!(stdout(), Clear(ClearType::All)).ok();
                        }
                        (KeyCode::Char('1'), _) => {
                            let _ = std::process::Command::new("xdg-open")
                                .arg("https://calendar.google.com")
                                .spawn();
                        }
                        (KeyCode::Char('2'), _) => {
                            let _ = std::process::Command::new("xdg-open")
                                .arg("https://www.icloud.com/calendar")
                                .spawn();
                        }
                        (KeyCode::Char('g') | KeyCode::Char('г'), _) => {
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
                        (KeyCode::Char('q') | KeyCode::Char('я') | KeyCode::Esc, _) => {
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
