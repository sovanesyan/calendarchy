mod cache;
mod config;
mod error;
mod google;
mod ui;

use cache::EventCache;
use chrono::{Datelike, DateTime, Duration, Local, NaiveDate, Utc};
use config::Config;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use google::{CalendarClient, GoogleAuth, TokenInfo};
use std::io::stdout;
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;

/// Authentication state
#[derive(Debug, Clone)]
pub enum AuthState {
    NotAuthenticated,
    AwaitingUserCode {
        user_code: String,
        verification_url: String,
        device_code: String,
        expires_at: DateTime<Utc>,
    },
    Authenticated(TokenInfo),
    Refreshing,
    Error(String),
}

/// Application state
struct App {
    current_date: NaiveDate,
    selected_date: NaiveDate,
    events: EventCache,
    auth_state: AuthState,
    status_message: Option<String>,
    config: Option<Config>,
    needs_fetch: bool,
}

impl App {
    fn new() -> Self {
        let today = Local::now().date_naive();
        Self {
            current_date: today,
            selected_date: today,
            events: EventCache::new(),
            auth_state: AuthState::NotAuthenticated,
            status_message: None,
            config: None,
            needs_fetch: false,
        }
    }

    fn next_month(&mut self) {
        if self.current_date.month() == 12 {
            self.current_date = self.current_date
                .with_year(self.current_date.year() + 1)
                .unwrap()
                .with_month(1)
                .unwrap()
                .with_day(1)
                .unwrap();
        } else {
            self.current_date = self.current_date
                .with_month(self.current_date.month() + 1)
                .unwrap()
                .with_day(1)
                .unwrap();
        }
        self.selected_date = self.current_date;
        self.needs_fetch = true;
    }

    fn prev_month(&mut self) {
        if self.current_date.month() == 1 {
            self.current_date = self.current_date
                .with_year(self.current_date.year() - 1)
                .unwrap()
                .with_month(12)
                .unwrap()
                .with_day(1)
                .unwrap();
        } else {
            self.current_date = self.current_date
                .with_month(self.current_date.month() - 1)
                .unwrap()
                .with_day(1)
                .unwrap();
        }
        self.selected_date = self.current_date;
        self.needs_fetch = true;
    }

    fn next_day(&mut self) {
        self.selected_date = self.selected_date + Duration::days(1);
        if self.selected_date.month() != self.current_date.month()
            || self.selected_date.year() != self.current_date.year()
        {
            self.current_date = self.selected_date.with_day(1).unwrap();
            self.needs_fetch = true;
        }
    }

    fn prev_day(&mut self) {
        self.selected_date = self.selected_date - Duration::days(1);
        if self.selected_date.month() != self.current_date.month()
            || self.selected_date.year() != self.current_date.year()
        {
            self.current_date = self.selected_date.with_day(1).unwrap();
            self.needs_fetch = true;
        }
    }

    fn goto_today(&mut self) {
        let today = Local::now().date_naive();
        let month_changed = today.month() != self.current_date.month()
            || today.year() != self.current_date.year();
        self.current_date = today;
        self.selected_date = today;
        if month_changed {
            self.needs_fetch = true;
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
    TokensLoaded(Option<TokenInfo>),
    DeviceCodeReceived {
        user_code: String,
        verification_url: String,
        device_code: String,
        expires_at: DateTime<Utc>,
    },
    TokenReceived(TokenInfo),
    AuthPending,
    AuthError(String),
    EventsFetched(Vec<google::CalendarEvent>, NaiveDate, NaiveDate),
    FetchError(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new();

    // Load config
    match Config::load() {
        Ok(Some(cfg)) => {
            app.config = Some(cfg);
            // Try to load saved tokens
            if let Ok(Some(tokens)) = config::load_tokens() {
                if !tokens.is_expired() {
                    app.auth_state = AuthState::Authenticated(tokens);
                    app.needs_fetch = true;
                } else if tokens.refresh_token.is_some() {
                    app.auth_state = AuthState::Refreshing;
                    app.needs_fetch = true;
                }
            }
        }
        Ok(None) => {
            app.status_message = Some("No config found. Create ~/.config/calendarchy/config.json".to_string());
        }
        Err(e) => {
            app.status_message = Some(format!("Config error: {}", e));
        }
    }

    // Channel for async messages
    let (tx, mut rx) = mpsc::channel::<AsyncMessage>(32);

    // Enable raw mode
    enable_raw_mode()?;

    // Main loop
    loop {
        // Render
        ui::render(
            app.current_date,
            app.selected_date,
            &app.events,
            &app.auth_state,
            app.status_message.as_deref(),
        );

        // Check if we need to fetch events
        if app.needs_fetch {
            if let AuthState::Authenticated(ref tokens) = app.auth_state {
                let (start, end) = app.month_range();
                if !app.events.has_range(start, end) || app.events.is_stale() {
                    let tokens = tokens.clone();
                    let calendar_id = app.config.as_ref()
                        .map(|c| c.calendar_id.clone())
                        .unwrap_or_else(|| "primary".to_string());
                    let tx = tx.clone();

                    tokio::spawn(async move {
                        let client = CalendarClient::new();
                        match client.list_events(&tokens, &calendar_id, start, end).await {
                            Ok(events) => {
                                let _ = tx.send(AsyncMessage::EventsFetched(events, start, end)).await;
                            }
                            Err(e) => {
                                let _ = tx.send(AsyncMessage::FetchError(e.to_string())).await;
                            }
                        }
                    });
                }
            }
            app.needs_fetch = false;
        }

        // Handle async messages (non-blocking)
        while let Ok(msg) = rx.try_recv() {
            match msg {
                AsyncMessage::TokensLoaded(Some(tokens)) => {
                    app.auth_state = AuthState::Authenticated(tokens);
                    app.needs_fetch = true;
                }
                AsyncMessage::TokensLoaded(None) => {
                    app.auth_state = AuthState::NotAuthenticated;
                }
                AsyncMessage::DeviceCodeReceived {
                    user_code,
                    verification_url,
                    device_code,
                    expires_at,
                } => {
                    app.auth_state = AuthState::AwaitingUserCode {
                        user_code,
                        verification_url,
                        device_code,
                        expires_at,
                    };
                }
                AsyncMessage::TokenReceived(tokens) => {
                    let _ = config::save_tokens(&tokens);
                    app.auth_state = AuthState::Authenticated(tokens);
                    app.needs_fetch = true;
                    app.status_message = Some("Connected to Google Calendar!".to_string());
                }
                AsyncMessage::AuthPending => {
                    // Still waiting, do nothing
                }
                AsyncMessage::AuthError(msg) => {
                    app.auth_state = AuthState::Error(msg);
                }
                AsyncMessage::EventsFetched(events, start, end) => {
                    app.events.store(events, start, end);
                    app.status_message = None;
                }
                AsyncMessage::FetchError(msg) => {
                    app.status_message = Some(format!("Fetch error: {}", msg));
                }
            }
        }

        // Poll for device code if awaiting
        if let AuthState::AwaitingUserCode { ref device_code, expires_at, .. } = app.auth_state {
            if Utc::now() < expires_at {
                if let Some(ref cfg) = app.config {
                    let auth = GoogleAuth::new(cfg.clone());
                    let device_code = device_code.clone();
                    let tx = tx.clone();

                    tokio::spawn(async move {
                        tokio::time::sleep(StdDuration::from_secs(5)).await;
                        match auth.poll_for_token(&device_code).await {
                            Ok(google::auth::PollResult::Success(tokens)) => {
                                let _ = tx.send(AsyncMessage::TokenReceived(tokens)).await;
                            }
                            Ok(google::auth::PollResult::Pending) => {
                                let _ = tx.send(AsyncMessage::AuthPending).await;
                            }
                            Ok(google::auth::PollResult::Denied) => {
                                let _ = tx.send(AsyncMessage::AuthError("Access denied".to_string())).await;
                            }
                            Ok(google::auth::PollResult::Expired) => {
                                let _ = tx.send(AsyncMessage::AuthError("Code expired".to_string())).await;
                            }
                            Ok(google::auth::PollResult::SlowDown) => {
                                let _ = tx.send(AsyncMessage::AuthPending).await;
                            }
                            Err(e) => {
                                let _ = tx.send(AsyncMessage::AuthError(e.to_string())).await;
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
                            app.next_month();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.prev_month();
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
                            app.needs_fetch = true;
                            app.status_message = Some("Refreshing...".to_string());
                        }
                        KeyCode::Char('a') => {
                            // Start auth flow
                            if let Some(ref cfg) = app.config {
                                let auth = GoogleAuth::new(cfg.clone());
                                let tx = tx.clone();

                                tokio::spawn(async move {
                                    match auth.request_device_code().await {
                                        Ok(resp) => {
                                            let expires_at = Utc::now() + chrono::Duration::seconds(resp.expires_in as i64);
                                            let _ = tx.send(AsyncMessage::DeviceCodeReceived {
                                                user_code: resp.user_code,
                                                verification_url: resp.verification_url,
                                                device_code: resp.device_code,
                                                expires_at,
                                            }).await;
                                        }
                                        Err(e) => {
                                            let _ = tx.send(AsyncMessage::AuthError(e.to_string())).await;
                                        }
                                    }
                                });
                            } else {
                                app.status_message = Some("No config file found".to_string());
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
