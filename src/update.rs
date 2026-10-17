//! How the app reacts: keys and background results go in, state changes and
//! `Effect`s (I/O to perform) come out. Nothing here touches the network,
//! the terminal or tokio, so it's all unit-testable; `main.rs` runs effects.

use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeDelta, Timelike};
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{month_key, App, EventSource, NavigationMode, PendingAction, SetupState, SetupStep, Status};
use crate::auth::{GoogleAuthState, ICloudAccount, ICloudAuthState};
use crate::cache::{DisplayEvent, EventId, SourceCache};
use crate::config::{self, Config};
use crate::google::TokenInfo;
use crate::keymap::{action_for, Action};
use crate::sources::{check_event_action, EventVerb};
use crate::utils::{add_months, to_zoom_deeplink};

/// Refetch the visible month this often while the app is open
pub const VISIBLE_STALE_AFTER: Duration = Duration::from_secs(5 * 60);
/// Neighbouring (prefetched) months go stale more slowly
pub const NEIGHBOUR_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
/// Don't retry a (source, month) more often than this — e.g. while offline
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Non-error status messages disappear after this long
pub const STATUS_TTL: Duration = Duration::from_secs(3);

/// Results from background tasks
pub enum Msg {
    Fetched {
        source: EventSource,
        month: NaiveDate,
        generation: u64,
        events: Vec<DisplayEvent>,
        calendar_name: Option<String>,
        tokens: Option<TokenInfo>,
    },
    FetchFailed {
        source: EventSource,
        month: NaiveDate,
        generation: u64,
        error: String,
        /// The user has to sign in again
        auth: bool,
        tokens: Option<TokenInfo>,
    },
    GoogleSignedIn(TokenInfo),
    GoogleSignInFailed(String),
    ICloudDiscovered(ICloudAccount),
    ICloudDiscoveryFailed(String),
    ActionDone { message: String, tokens: Option<TokenInfo> },
    ActionFailed { message: String, auth: bool, tokens: Option<TokenInfo> },
}

/// I/O the runtime performs on the app's behalf
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    FetchGoogle { month: NaiveDate, generation: u64, tokens: TokenInfo, calendar_id: String, known_name: Option<String> },
    FetchCalDav { month: NaiveDate, generation: u64, account: ICloudAccount },
    StartGoogleAuth,
    DiscoverICloud,
    /// `tokens` are the Google sign-in, for Google events
    EventAction { verb: EventVerb, id: EventId, tokens: Option<TokenInfo> },
    OpenUrl(String),
    SaveCache,
    SaveGoogleTokens(TokenInfo),
    SaveICloudAccount(ICloudAccount),
}

/// How the TUI tells you to set a source up
const SETUP_HINT: &str = "press S for setup";

impl App {
    // ---- startup / configuration -----------------------------------------

    /// Derive auth states from config and saved credentials. Expired Google
    /// tokens count as signed in: the first fetch refreshes them.
    pub fn init_sources(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();

        self.google_auth = match self.config.google {
            None => GoogleAuthState::NotConfigured,
            Some(_) => match config::load_google_tokens() {
                Ok(Some(tokens)) => GoogleAuthState::Authenticated(tokens),
                _ => GoogleAuthState::NotAuthenticated,
            },
        };

        self.icloud_auth = match self.config.icloud {
            None => ICloudAuthState::NotConfigured,
            Some(_) => {
                let account = crate::sources::saved_icloud_account();
                if account.calendars.is_empty() {
                    effects.push(Effect::DiscoverICloud);
                    ICloudAuthState::Discovering
                } else {
                    if account.addresses.is_empty() {
                        // Saved before your addresses were discovered: find
                        // them in the background, fetching meanwhile
                        effects.push(Effect::DiscoverICloud);
                    }
                    ICloudAuthState::Authenticated(account)
                }
            }
        };

        if self.config.google.is_none() && self.config.icloud.is_none() {
            self.open_setup_wizard();
        }
        effects
    }

    pub fn open_setup_wizard(&mut self) {
        let step = if self.config.google.is_some() || self.config.icloud.is_some() {
            // Re-entering via S key: skip to calendar config
            SetupStep::GoogleAsk
        } else if crate::setup::should_show_shortcut_step() {
            SetupStep::ShortcutAsk
        } else {
            SetupStep::Welcome
        };
        self.setup = Some(SetupState::new(step));
    }

    fn start_google_auth(&mut self) -> Vec<Effect> {
        self.google_auth = GoogleAuthState::Authenticating;
        self.set_status("Opening browser for Google sign-in...");
        vec![Effect::StartGoogleAuth]
    }

    fn discover_icloud(&mut self) -> Vec<Effect> {
        self.icloud_auth = ICloudAuthState::Discovering;
        vec![Effect::DiscoverICloud]
    }

    /// Switched to another Apple ID: the old account's events go now (from
    /// the disk cache too), and fetches still running for it are dropped
    /// when they land. Discovery for the new account follows.
    fn forget_icloud_account(&mut self) -> Vec<Effect> {
        self.events.icloud = SourceCache::default();
        self.new_fetch_generation();
        vec![Effect::SaveCache]
    }

    // ---- fetch scheduling ------------------------------------------------

    /// The fetch for a source and month; None if the source isn't signed in
    fn fetch_effect(&self, source: EventSource, month: NaiveDate) -> Option<Effect> {
        match source {
            EventSource::Google => {
                let GoogleAuthState::Authenticated(ref tokens) = self.google_auth else { return None };
                Some(Effect::FetchGoogle {
                    month,
                    generation: self.fetch_generation,
                    tokens: tokens.clone(),
                    calendar_id: self.config.google.as_ref().map_or_else(|| "primary".to_string(), |g| g.calendar_id.clone()),
                    known_name: self.google_calendar_name.clone(),
                })
            }
            EventSource::ICloud => {
                let ICloudAuthState::Authenticated(ref account) = self.icloud_auth else { return None };
                self.config.icloud.is_some().then(|| Effect::FetchCalDav { month, generation: self.fetch_generation, account: account.clone() })
            }
        }
    }

    /// Fetches the current state calls for: the visible month when it's
    /// missing or stale, then its neighbours once it has loaded (so jumping a
    /// month is instant). Marks them in flight; call after every state change.
    pub fn wanted_fetches(&mut self, now: Instant) -> Vec<Effect> {
        let visible = self.visible_month();
        let mut effects = Vec::new();

        for source in [EventSource::Google, EventSource::ICloud] {
            let cache = self.events.source(source);
            let mut months = vec![(visible, VISIBLE_STALE_AFTER)];
            if cache.fetched_at(visible).is_some() {
                months.push((add_months(visible, 1), NEIGHBOUR_STALE_AFTER));
                months.push((add_months(visible, -1), NEIGHBOUR_STALE_AFTER));
            }

            for (month, stale_after) in months {
                let key = (source, month_key(month));
                let stale = cache.fetched_at(month).is_none_or(|t| now.duration_since(t) >= stale_after);
                let tried_recently = self.attempts.get(&key).is_some_and(|t| now.duration_since(*t) < RETRY_AFTER);
                if self.in_flight.contains_key(&key) || !stale || tried_recently {
                    continue;
                }
                if let Some(effect) = self.fetch_effect(source, month) {
                    self.in_flight.insert(key, self.fetch_generation);
                    self.attempts.insert(key, now);
                    effects.push(effect);
                }
            }
        }

        if !effects.is_empty() {
            self.dirty = true; // loading indicators
        }
        effects
    }

    /// Refetch everything, keeping the current data on screen meanwhile.
    /// Fetches already running are superseded: their results are dropped.
    fn invalidate_all(&mut self) {
        self.events.google.invalidate();
        self.events.icloud.invalidate();
        self.new_fetch_generation();
    }

    fn new_fetch_generation(&mut self) {
        self.fetch_generation += 1;
        self.in_flight.clear();
        self.attempts.clear();
    }

    /// Settle a finished fetch; true if its result is current and should be used
    fn finish_fetch(&mut self, source: EventSource, month: NaiveDate, generation: u64) -> bool {
        let key = (source, month_key(month));
        if self.in_flight.get(&key) == Some(&generation) {
            self.in_flight.remove(&key);
        }
        generation == self.fetch_generation
    }

    // ---- timers ----------------------------------------------------------

    /// Time-driven state: expire status messages, redraw each minute (the
    /// countdown and "now" markers). Returns true if a redraw is needed.
    pub fn tick(&mut self) -> bool {
        let mut changed = self.clear_expired_status();
        let minute = Local::now().minute();
        if minute != self.last_render_minute {
            self.last_render_minute = minute;
            changed = true;
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// How long the event loop may sleep before something time-driven is due:
    /// the next minute boundary or a status message expiring
    pub fn next_wakeup(&self) -> Duration {
        let now = Local::now();
        let to_next_minute = Duration::from_millis(
            60_000 - (now.second() as u64 * 1000 + (now.nanosecond() as u64 / 1_000_000).min(999)),
        );
        let to_status_expiry = match self.status {
            Some(Status::Info(_, t)) => STATUS_TTL.saturating_sub(t.elapsed()) + Duration::from_millis(10),
            _ => Duration::MAX,
        };
        to_next_minute.min(to_status_expiry).max(Duration::from_millis(10))
    }

    // ---- background results ----------------------------------------------

    fn adopt_refreshed_tokens(&mut self, tokens: Option<TokenInfo>, effects: &mut Vec<Effect>) {
        if let Some(tokens) = tokens {
            if matches!(self.google_auth, GoogleAuthState::Authenticated(_)) {
                self.google_auth = GoogleAuthState::Authenticated(tokens.clone());
            }
            effects.push(Effect::SaveGoogleTokens(tokens));
        }
    }

    fn google_signed_out(&mut self, error: &str) {
        self.google_auth = GoogleAuthState::NotAuthenticated;
        self.set_error(format!("Google sign-in expired — press g to sign in again ({})", error));
    }

    pub fn handle_msg(&mut self, msg: Msg) -> Vec<Effect> {
        self.dirty = true;
        let mut effects = Vec::new();
        match msg {
            Msg::Fetched { source, month, generation, events, calendar_name, tokens } => {
                self.adopt_refreshed_tokens(tokens, &mut effects);
                if !self.finish_fetch(source, month, generation) {
                    return effects;
                }
                self.last_fetch_error.remove(&source);
                if source == EventSource::Google && calendar_name.is_some() {
                    self.google_calendar_name = calendar_name;
                }
                self.events.source_mut(source).store(events, month);
                effects.push(Effect::SaveCache);
            }
            Msg::FetchFailed { source, month, generation, error, auth, tokens } => {
                self.adopt_refreshed_tokens(tokens, &mut effects);
                if !self.finish_fetch(source, month, generation) {
                    return effects;
                }
                let visible = month_key(month) == month_key(self.visible_month());
                if source == EventSource::Google && auth {
                    self.google_signed_out(&error);
                } else if visible {
                    // Prefetch failures stay quiet; the visible month says why it's
                    // stale — once, not on every retry the user already dismissed
                    let label = if source == EventSource::Google { "Google" } else { "iCloud" };
                    let text = format!("{}: {}", label, error);
                    if self.last_fetch_error.get(&source) != Some(&text) {
                        self.set_error(text.clone());
                        self.last_fetch_error.insert(source, text);
                    }
                }
            }
            Msg::GoogleSignedIn(tokens) => {
                self.google_auth = GoogleAuthState::Authenticated(tokens.clone());
                effects.push(Effect::SaveGoogleTokens(tokens));
                // Results of requests made with the old session must not sign
                // the user straight back out
                self.new_fetch_generation();
                self.set_status("Connected to Google Calendar!");
                // Advance setup wizard past auth waiting
                if let Some(ref mut setup) = self.setup
                    && setup.step == SetupStep::GoogleAuthWaiting
                {
                    setup.step = SetupStep::ICloudAsk;
                }
            }
            Msg::GoogleSignInFailed(error) => {
                self.google_auth = GoogleAuthState::NotAuthenticated;
                self.set_error(format!("Google: {}", error));
                // Advance setup wizard past auth waiting on error too
                if let Some(ref mut setup) = self.setup
                    && setup.step == SetupStep::GoogleAuthWaiting
                {
                    setup.error = Some(format!("Google auth failed: {}", error));
                    setup.step = SetupStep::ICloudAsk;
                }
            }
            Msg::ICloudDiscovered(mut account) => {
                if let ICloudAuthState::Authenticated(ref old) = self.icloud_auth {
                    // A background lookup of your addresses (calendars already
                    // in use; a discovery you start goes through Discovering):
                    // keep the calendars, whose URLs are in the events' ids
                    account.calendars = old.calendars.clone();
                    // Knowing you by other addresses changes which events count
                    // as accepted: refetch, and drop fetches made without them
                    if old.addresses != account.addresses {
                        self.events.icloud.invalidate();
                        self.new_fetch_generation();
                    }
                }
                let count = account.calendars.len();
                effects.push(Effect::SaveICloudAccount(account.clone()));
                self.icloud_auth = ICloudAuthState::Authenticated(account);
                self.attempts.retain(|(source, _), _| *source != EventSource::ICloud);
                self.set_status(format!("Connected to {} iCloud calendar(s)!", count));
            }
            Msg::ICloudDiscoveryFailed(error) => {
                self.set_error(format!("iCloud: {}", error));
                // A background rediscovery failing leaves the saved calendars in use
                if !matches!(self.icloud_auth, ICloudAuthState::Authenticated(_)) {
                    self.icloud_auth = ICloudAuthState::Error;
                }
            }
            Msg::ActionDone { message, tokens } => {
                self.adopt_refreshed_tokens(tokens, &mut effects);
                self.set_status(message);
                // Refetch to reflect the change, keeping the old data meanwhile
                self.invalidate_all();
                self.exit_event_mode();
            }
            Msg::ActionFailed { message, auth, tokens } => {
                self.adopt_refreshed_tokens(tokens, &mut effects);
                if auth {
                    self.google_signed_out(&message);
                } else {
                    self.set_error(message);
                }
            }
        }
        effects
    }

    // ---- keys --------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        // Sticky error messages are dismissed by any keypress
        self.clear_error();

        if self.setup.is_some() {
            return self.handle_setup_key(key.code);
        }

        // Help overlay: any key closes it
        if self.show_help {
            self.show_help = false;
            return vec![];
        }

        if self.search.is_some() {
            self.handle_search_key(key.code);
            return vec![];
        }

        if let Some(action) = self.pending_action.take() {
            return self.handle_confirmation_key(key.code, action);
        }

        match action_for(self.navigation_mode, key) {
            Some(action) => self.apply(action),
            None => vec![],
        }
    }

    fn handle_search_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.close_search(),
            KeyCode::Enter => self.select_search_result(),
            KeyCode::Backspace => {
                if let Some(ref mut search) = self.search {
                    search.query.pop();
                }
                self.update_search_results();
            }
            KeyCode::Down | KeyCode::Tab => {
                if let Some(ref mut search) = self.search
                    && !search.results.is_empty()
                {
                    search.selected_index = (search.selected_index + 1).min(search.results.len() - 1);
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(ref mut search) = self.search {
                    search.selected_index = search.selected_index.saturating_sub(1);
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut search) = self.search {
                    search.query.push(c);
                }
                self.update_search_results();
            }
            _ => {}
        }
    }

    fn handle_confirmation_key(&mut self, code: KeyCode, action: PendingAction) -> Vec<Effect> {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirm(action),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                // Cancel - action already taken from pending_action
                self.set_status("Cancelled");
                vec![]
            }
            _ => {
                // Put the action back if not confirmed/cancelled
                self.pending_action = Some(action);
                vec![]
            }
        }
    }

    fn confirm(&mut self, PendingAction { verb, id }: PendingAction) -> Vec<Effect> {
        let tokens = match self.google_auth {
            GoogleAuthState::Authenticated(ref t) => Some(t.clone()),
            _ => None,
        };
        // The Google session may have ended while the question was up
        if let Err(refused) = check_event_action(verb, &id, tokens.is_some(), self.config.icloud.is_some()) {
            self.set_status(refused.message(SETUP_HINT));
            return vec![];
        }
        self.set_status(verb.in_progress());
        vec![Effect::EventAction { verb, id, tokens }]
    }

    /// Ask to confirm an accept/decline/delete on the selected event
    fn request_event_action(&mut self, verb: EventVerb) {
        let Some(event) = self.get_selected_event() else { return };
        let id = event.id.clone();
        let google_signed_in = matches!(self.google_auth, GoogleAuthState::Authenticated(_));
        match check_event_action(verb, &id, google_signed_in, self.config.icloud.is_some()) {
            Ok(()) => self.pending_action = Some(PendingAction { verb, id }),
            Err(refused) => self.set_status(refused.message(SETUP_HINT)),
        }
    }

    pub fn apply(&mut self, action: Action) -> Vec<Effect> {
        use Action::*;
        match action {
            NextDay => self.selected_date += TimeDelta::days(1),
            PrevDay => self.selected_date -= TimeDelta::days(1),
            NextWeek => self.selected_date += TimeDelta::days(7),
            PrevWeek => self.selected_date -= TimeDelta::days(7),
            NextMonth | PrevMonth => {
                // To the 1st of that month
                self.selected_date = add_months(self.selected_date, if action == NextMonth { 1 } else { -1 });
                // Month jump invalidates the event selection, so drop to Day mode
                if self.navigation_mode == NavigationMode::Event {
                    self.exit_event_mode();
                }
            }
            EnterEvents => {
                if !self.enter_event_mode() {
                    self.set_status("No events on this day");
                }
            }
            NextEvent => self.next_event(),
            PrevEvent => self.prev_event(),
            JumpEventsForward | JumpEventsBack => {
                // Scroll 10 events; flash where we landed since this can jump days
                for _ in 0..10 {
                    if action == JumpEventsForward { self.next_event() } else { self.prev_event() }
                }
                self.set_status(format!("Jumped to {}", self.selected_date.format("%a %b %d")));
            }
            ExitEvents => self.exit_event_mode(),
            SwitchPanel => self.switch_panel(),
            Join => {
                // Join, then quit: once you're in the call the app has done its job.
                // Zoom links deep-link into the app instead of the browser
                if let Some(url) = self.get_selected_event().and_then(|e| e.meeting_url.clone()) {
                    self.quit = true;
                    return vec![Effect::OpenUrl(to_zoom_deeplink(&url).unwrap_or(url))];
                }
            }
            Accept => self.request_event_action(EventVerb::Accept),
            Decline => self.request_event_action(EventVerb::Decline),
            Delete => self.request_event_action(EventVerb::Delete),
            Today => self.selected_date = Local::now().date_naive(),
            Now => self.goto_date(Local::now().date_naive()),
            Refresh => {
                self.invalidate_all();
                self.set_status("Refreshing...");
            }
            ToggleLogs => self.show_logs = !self.show_logs,
            Search => self.open_search(),
            Help => self.show_help = true,
            OpenGoogleWeb => return vec![Effect::OpenUrl("https://calendar.google.com".into())],
            OpenICloudWeb => return vec![Effect::OpenUrl("https://www.icloud.com/calendar".into())],
            ConnectGoogle => {
                if !matches!(self.google_auth, GoogleAuthState::Authenticated(_)) {
                    if self.config.google.is_some() {
                        return self.start_google_auth();
                    }
                    self.set_status("Google not configured — press S for setup");
                }
            }
            ConnectICloud => match self.config.icloud {
                None => self.set_status("iCloud not configured — press S for setup"),
                // Re-run discovery (refreshes calendar names)
                Some(_) => return self.discover_icloud(),
            },
            Setup => self.open_setup_wizard(),
            Quit => self.quit = true,
        }
        vec![]
    }

    // ---- setup wizard ------------------------------------------------------

    fn handle_setup_key(&mut self, key: KeyCode) -> Vec<Effect> {
        let google_yes = self.setup.as_ref().is_some_and(|s| s.step == SetupStep::GoogleAsk)
            && matches!(key, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter);
        if google_yes {
            if let Some(ref mut setup) = self.setup {
                setup.step = SetupStep::GoogleAuthWaiting;
                setup.error = None;
            }
            if self.config.google.is_none() {
                self.config.google = Some(config::GoogleConfig::default());
            }
            let _ = self.config.save();
            return self.start_google_auth();
        }

        let mut effects = Vec::new();
        let mut quit = false;
        let setup = self.setup.as_mut().unwrap();
        setup.error = None;

        match setup.step {
            SetupStep::ShortcutAsk => match key {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => match crate::setup::install_shortcut() {
                    Ok(()) => setup.step = SetupStep::Welcome,
                    Err(e) => setup.error = Some(format!("Failed: {}", e)),
                },
                KeyCode::Char('n') | KeyCode::Char('N') => setup.step = SetupStep::Welcome,
                KeyCode::Char('q') | KeyCode::Esc => quit = true,
                _ => {}
            },
            SetupStep::Welcome => match key {
                KeyCode::Enter => setup.step = SetupStep::GoogleAsk,
                KeyCode::Char('q') | KeyCode::Esc => quit = true,
                _ => {}
            },
            SetupStep::GoogleAsk => match key {
                KeyCode::Char('n') | KeyCode::Char('N') => setup.step = SetupStep::ICloudAsk,
                KeyCode::Esc => setup.step = SetupStep::Welcome,
                _ => {}
            },
            SetupStep::GoogleAuthWaiting => {
                if key == KeyCode::Esc {
                    setup.step = SetupStep::ICloudAsk;
                }
            }
            SetupStep::ICloudAsk => match key {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    setup.step = SetupStep::ICloudOpenUrl;
                    effects.push(Effect::OpenUrl("https://appleid.apple.com/account/manage".into()));
                }
                KeyCode::Char('n') | KeyCode::Char('N') => setup.step = SetupStep::Done,
                KeyCode::Esc => setup.step = SetupStep::GoogleAsk,
                _ => {}
            },
            SetupStep::ICloudOpenUrl => match key {
                KeyCode::Enter => setup.step = SetupStep::ICloudAppleId,
                KeyCode::Esc => setup.step = SetupStep::ICloudAsk,
                _ => {}
            },
            SetupStep::ICloudAppleId | SetupStep::ICloudPassword => {
                let is_id = setup.step == SetupStep::ICloudAppleId;
                match key {
                    KeyCode::Enter => {
                        let val = setup.input.trim().to_string();
                        if val.is_empty() {
                            setup.error = Some(if is_id { "Apple ID cannot be empty" } else { "App password cannot be empty" }.to_string());
                        } else if is_id {
                            setup.icloud_apple_id = Some(val);
                            setup.input.clear();
                            setup.step = SetupStep::ICloudPassword;
                        } else {
                            setup.icloud_password = Some(val);
                            setup.input.clear();
                            setup.step = SetupStep::Done;
                        }
                    }
                    KeyCode::Esc => {
                        setup.input.clear();
                        setup.step = if is_id { SetupStep::ICloudOpenUrl } else { SetupStep::ICloudAppleId };
                    }
                    KeyCode::Backspace => {
                        setup.input.pop();
                    }
                    KeyCode::Char(c) => setup.input.push(c),
                    _ => {}
                }
            }
            SetupStep::Done => {}
        }

        let done = setup.step == SetupStep::Done;
        if quit {
            self.quit = true;
        } else if done {
            effects.extend(self.finish_setup());
        }
        effects
    }

    /// Save the wizard's choices and connect whatever was newly configured
    fn finish_setup(&mut self) -> Vec<Effect> {
        let setup = self.setup.as_mut().unwrap();
        // Saying yes to Google already saved it to the config
        let has_icloud = setup.icloud_apple_id.is_some();

        if !has_icloud && self.config.google.is_none() && self.config.icloud.is_none() {
            setup.error = Some("Set up at least one calendar".to_string());
            setup.step = SetupStep::GoogleAsk;
            return vec![];
        }

        let mut new_config = self.config.clone();
        if let (Some(apple_id), Some(app_password)) = (setup.icloud_apple_id.take(), setup.icloud_password.take()) {
            new_config.icloud = Some(config::ICloudConfig { apple_id: Some(apple_id), app_password: Some(app_password) });
        }

        if let Err(e) = new_config.save() {
            setup.error = Some(format!("Failed to save config: {}", e));
            setup.step = SetupStep::GoogleAsk;
            return vec![];
        }

        // Reload config and initialize auth states
        let old_apple_id = self.config.icloud.as_ref().and_then(|c| c.apple_id.clone());
        self.config = Config::load().unwrap_or(new_config);
        let icloud_account_changed = self.config.icloud.as_ref().and_then(|c| c.apple_id.clone()) != old_apple_id;
        self.setup = None;
        let mut effects = Vec::new();

        // Auto-start Google browser auth if newly configured
        if self.config.google.is_some()
            && !matches!(self.google_auth, GoogleAuthState::Authenticated(_) | GoogleAuthState::Authenticating)
        {
            effects.extend(self.start_google_auth());
        }

        if icloud_account_changed && old_apple_id.is_some() {
            effects.extend(self.forget_icloud_account());
        }

        // Auto-start iCloud discovery if newly configured, failed before, or
        // switched to another Apple ID
        if self.config.icloud.is_some()
            && (icloud_account_changed
                || matches!(self.icloud_auth, ICloudAuthState::NotConfigured | ICloudAuthState::Error))
        {
            effects.extend(self.discover_icloud());
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::CalendarEntry;
    use crate::config::{GoogleConfig, ICloudConfig};
    use crossterm::event::KeyModifiers;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn tokens() -> TokenInfo {
        TokenInfo {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        }
    }

    /// App on 2026-10-15 with Google signed in and CalDAV with one calendar
    fn ready_app() -> App {
        let mut app = App::on(d(2026, 10, 15));
        app.config.google = Some(GoogleConfig::default());
        app.config.icloud = Some(ICloudConfig { apple_id: Some("a".into()), app_password: Some("p".into()) });
        app.google_auth = GoogleAuthState::Authenticated(tokens());
        app.icloud_auth = ICloudAuthState::Authenticated(ICloudAccount {
            calendars: vec![CalendarEntry { url: "https://cal/1".into(), name: Some("Home".into()) }],
            addresses: vec!["/1/principal/".into()],
        });
        app
    }

    fn fetched_months(effects: &[Effect]) -> Vec<(&'static str, NaiveDate)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::FetchGoogle { month, .. } => Some(("google", *month)),
                Effect::FetchCalDav { month, .. } => Some(("icloud", *month)),
                _ => None,
            })
            .collect()
    }

    fn deliver(app: &mut App, source: EventSource, month: NaiveDate) -> Vec<Effect> {
        let generation = app.fetch_generation;
        app.handle_msg(Msg::Fetched { source, month, generation, events: vec![], calendar_name: Some("Work".into()), tokens: None })
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn test_visible_month_first_then_neighbours() {
        let mut app = ready_app();
        let now = Instant::now();
        assert_eq!(fetched_months(&app.wanted_fetches(now)), [("google", d(2026, 10, 1)), ("icloud", d(2026, 10, 1))]);
        assert!(app.is_loading(EventSource::Google));
        // Nothing new while those are in flight
        assert!(app.wanted_fetches(now).is_empty());

        deliver(&mut app, EventSource::Google, d(2026, 10, 1));
        assert!(!app.is_loading(EventSource::Google));
        // A background refresh of a loaded month doesn't show as loading
        let later = Instant::now() + VISIBLE_STALE_AFTER + Duration::from_secs(1);
        assert!(!fetched_months(&app.wanted_fetches(later)).is_empty());
        assert!(!app.is_loading(EventSource::Google));
        assert!(app.in_flight.contains_key(&(EventSource::Google, (2026, 11))), "neighbours prefetched once loaded");
        assert!(app.in_flight.contains_key(&(EventSource::Google, (2026, 9))));
    }

    #[test]
    fn test_fresh_months_are_not_refetched_until_stale() {
        let mut app = ready_app();
        let t0 = Instant::now();
        app.wanted_fetches(t0);
        deliver(&mut app, EventSource::Google, d(2026, 10, 1));
        deliver(&mut app, EventSource::ICloud, d(2026, 10, 1));
        for m in [d(2026, 11, 1), d(2026, 9, 1)] {
            app.wanted_fetches(t0);
            deliver(&mut app, EventSource::Google, m);
            deliver(&mut app, EventSource::ICloud, m);
        }
        assert!(app.wanted_fetches(t0 + Duration::from_secs(60)).is_empty());
        // The visible month refreshes in the background after 5 minutes...
        let later = Instant::now() + VISIBLE_STALE_AFTER + Duration::from_secs(1);
        assert_eq!(fetched_months(&app.wanted_fetches(later)), [("google", d(2026, 10, 1)), ("icloud", d(2026, 10, 1))]);
    }

    #[test]
    fn test_failed_fetches_back_off() {
        let mut app = ready_app();
        let t0 = Instant::now();
        app.wanted_fetches(t0);
        app.handle_msg(Msg::FetchFailed { source: EventSource::Google, month: d(2026, 10, 1), generation: 0, error: "offline".into(), auth: false, tokens: None });
        assert!(app.status.as_ref().is_some_and(Status::is_error), "visible-month failures are shown");
        assert!(fetched_months(&app.wanted_fetches(t0 + Duration::from_secs(5))).iter().all(|(s, _)| *s != "google"));
        assert_eq!(fetched_months(&app.wanted_fetches(t0 + RETRY_AFTER)), [("google", d(2026, 10, 1))]);
    }

    #[test]
    fn test_month_jump_fetches_new_month() {
        let mut app = ready_app();
        app.wanted_fetches(Instant::now());
        app.apply(Action::NextMonth);
        let months = fetched_months(&app.wanted_fetches(Instant::now()));
        assert!(months.contains(&("google", d(2026, 11, 1))), "{months:?}");
        assert!(months.contains(&("icloud", d(2026, 11, 1))));
    }

    #[test]
    fn test_day_steps_follow_the_visible_month() {
        let mut app = ready_app();
        app.selected_date = d(2026, 10, 31);
        assert_eq!(fetched_months(&app.wanted_fetches(Instant::now())), [("google", d(2026, 10, 1)), ("icloud", d(2026, 10, 1))]);
        app.apply(Action::NextDay);
        assert_eq!(app.visible_month(), d(2026, 11, 1));
        assert_eq!(fetched_months(&app.wanted_fetches(Instant::now())), [("google", d(2026, 11, 1)), ("icloud", d(2026, 11, 1))]);
        assert!(app.is_loading(EventSource::Google), "loading shows for the month on screen");
        // Back into October: already in flight, nothing new
        app.apply(Action::PrevWeek);
        assert_eq!(app.selected_date, d(2026, 10, 25));
        assert!(app.wanted_fetches(Instant::now()).is_empty());
        // A failure for a month no longer on screen stays quiet
        let generation = app.fetch_generation;
        app.handle_msg(Msg::FetchFailed { source: EventSource::ICloud, month: d(2026, 11, 1), generation, error: "offline".into(), auth: false, tokens: None });
        assert!(app.status.is_none());
    }

    #[test]
    fn test_refresh_keeps_data_on_screen() {
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        let mut ev = crate::app::tests::make_event_with_attendees("Standup", vec![]);
        ev.date = day;
        app.events.google.store(vec![ev], day);
        app.apply(Action::Refresh);
        assert_eq!(app.events.google.get(day).len(), 1, "stale data stays visible");
        assert!(fetched_months(&app.wanted_fetches(Instant::now())).contains(&("google", d(2026, 10, 1))));
    }

    #[test]
    fn test_refreshed_tokens_are_adopted_and_saved() {
        let mut app = ready_app();
        app.wanted_fetches(Instant::now());
        let mut renewed = tokens();
        renewed.access_token = "renewed".into();
        let effects = app.handle_msg(Msg::Fetched {
            source: EventSource::Google,
            month: d(2026, 10, 1),
            generation: 0,
            events: vec![],
            calendar_name: None,
            tokens: Some(renewed.clone()),
        });
        assert!(effects.contains(&Effect::SaveGoogleTokens(renewed)));
        assert!(matches!(app.google_auth, GoogleAuthState::Authenticated(ref t) if t.access_token == "renewed"));
        assert!(effects.contains(&Effect::SaveCache));
    }

    #[test]
    fn test_auth_failure_signs_out_and_stops_google_fetches() {
        let mut app = ready_app();
        app.wanted_fetches(Instant::now());
        app.handle_msg(Msg::FetchFailed { source: EventSource::Google, month: d(2026, 10, 1), generation: 0, error: "revoked".into(), auth: true, tokens: None });
        assert!(matches!(app.google_auth, GoogleAuthState::NotAuthenticated));
        assert!(app.status.as_ref().unwrap().text().contains("press g"));
        let later = Instant::now() + RETRY_AFTER * 2;
        assert!(fetched_months(&app.wanted_fetches(later)).iter().all(|(s, _)| *s != "google"));
    }

    #[test]
    fn test_calendar_name_is_looked_up_once() {
        let mut app = ready_app();
        let first = app.wanted_fetches(Instant::now());
        assert!(matches!(&first[0], Effect::FetchGoogle { known_name: None, .. }));
        deliver(&mut app, EventSource::Google, d(2026, 10, 1));
        let next = app.wanted_fetches(Instant::now());
        assert!(matches!(&next[0], Effect::FetchGoogle { known_name: Some(n), .. } if n == "Work"));
    }

    #[test]
    fn test_quit_and_esc() {
        let mut app = ready_app();
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.quit, "Esc never quits");
        app.handle_key(key('q'));
        assert!(app.quit);
    }

    #[test]
    fn test_join_opens_the_meeting_and_quits() {
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        let mut ev = crate::app::tests::make_event_with_attendees("Call", vec![]);
        ev.date = day;
        ev.meeting_url = Some("https://zoom.us/j/123".into());
        app.events.google.store(vec![ev], day);
        app.navigation_mode = NavigationMode::Event;
        let effects = app.apply(Action::Join);
        assert_eq!(effects, [Effect::OpenUrl("zoommtg://zoom.us/join?action=join&confno=123".into())]);
        assert!(app.quit);
    }

    #[test]
    fn test_join_without_a_link_does_not_quit() {
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        let mut ev = crate::app::tests::make_event_with_attendees("No link", vec![]);
        ev.date = day;
        app.events.google.store(vec![ev], day);
        app.navigation_mode = NavigationMode::Event;
        assert!(app.apply(Action::Join).is_empty());
        assert!(!app.quit);
    }

    #[test]
    fn test_help_overlay_swallows_next_key() {
        let mut app = ready_app();
        app.handle_key(key('?'));
        assert!(app.show_help);
        app.handle_key(key('q'));
        assert!(!app.show_help);
        assert!(!app.quit);
    }

    #[test]
    fn test_fetch_started_before_an_action_is_discarded() {
        // A background refresh in flight while the user deletes an event must
        // not bring the deleted event back
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        app.wanted_fetches(Instant::now());
        let old_generation = app.fetch_generation;
        app.handle_msg(Msg::ActionDone { message: "Event deleted".into(), tokens: None });
        let mut stale = crate::app::tests::make_event_with_attendees("Deleted", vec![]);
        stale.date = day;
        app.handle_msg(Msg::Fetched {
            source: EventSource::Google, month: d(2026, 10, 1), generation: old_generation,
            events: vec![stale], calendar_name: None, tokens: None,
        });
        assert!(app.events.google.get(day).is_empty());
        // ...and the refetch it triggered is still scheduled
        assert!(fetched_months(&app.wanted_fetches(Instant::now())).contains(&("google", d(2026, 10, 1))));
    }

    #[test]
    fn test_late_auth_failure_does_not_undo_a_new_sign_in() {
        let mut app = ready_app();
        app.wanted_fetches(Instant::now());
        let old_generation = app.fetch_generation;
        app.handle_msg(Msg::GoogleSignedIn(tokens()));
        app.handle_msg(Msg::FetchFailed {
            source: EventSource::Google, month: d(2026, 10, 1), generation: old_generation,
            error: "revoked".into(), auth: true, tokens: None,
        });
        assert!(matches!(app.google_auth, GoogleAuthState::Authenticated(_)));
    }

    #[test]
    fn test_repeated_failures_raise_the_error_once() {
        let mut app = ready_app();
        let fail = |app: &mut App| {
            let generation = app.fetch_generation;
            app.handle_msg(Msg::FetchFailed {
                source: EventSource::Google, month: d(2026, 10, 1), generation,
                error: "offline".into(), auth: false, tokens: None,
            });
        };
        let showing_error = |app: &App| app.status.as_ref().is_some_and(Status::is_error);
        fail(&mut app);
        assert!(showing_error(&app));
        app.clear_error(); // user dismisses it
        fail(&mut app);
        assert!(!showing_error(&app), "same error isn't re-raised on retry");
        deliver(&mut app, EventSource::Google, d(2026, 10, 1));
        fail(&mut app);
        assert!(showing_error(&app), "a new failure after recovering is shown again");
    }

    fn icloud_event(title: &str, day: NaiveDate) -> DisplayEvent {
        let mut ev = crate::app::tests::make_event_with_attendees(title, vec![]);
        ev.date = day;
        ev
    }

    #[test]
    fn test_switching_apple_id_drops_the_old_accounts_events() {
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        app.events.icloud.store(vec![icloud_event("Old account", day)], day);
        app.wanted_fetches(Instant::now());
        let old_generation = app.fetch_generation;

        let mut effects = app.forget_icloud_account();
        effects.extend(app.discover_icloud());
        assert!(app.events.icloud.get(day).is_empty(), "gone at once");
        assert!(effects.contains(&Effect::SaveCache), "and from the disk cache the shell reads");
        assert!(effects.contains(&Effect::DiscoverICloud));

        // A fetch the old account had running doesn't bring them back
        app.handle_msg(Msg::Fetched {
            source: EventSource::ICloud, month: d(2026, 10, 1), generation: old_generation,
            events: vec![icloud_event("Old account", day)], calendar_name: None, tokens: None,
        });
        assert!(app.events.icloud.get(day).is_empty());

        // Once the new account is discovered its events are fetched
        app.handle_msg(Msg::ICloudDiscovered(ICloudAccount {
            calendars: vec![CalendarEntry { url: "https://cal/2".into(), name: None }],
            addresses: vec!["/2/principal/".into()],
        }));
        let fetches = app.wanted_fetches(Instant::now());
        assert!(fetches.iter().any(|e| matches!(e, Effect::FetchCalDav { account, .. } if account.calendars[0].url == "https://cal/2")));
    }

    #[test]
    fn test_rediscovery_with_new_addresses_refetches_icloud() {
        // Upgrading from a tokens.json without addresses: what's cached and
        // in flight was judged without knowing all of you
        let mut app = ready_app();
        let day = d(2026, 10, 15);
        app.wanted_fetches(Instant::now());
        deliver(&mut app, EventSource::ICloud, d(2026, 10, 1));
        app.events.icloud.store(vec![icloud_event("Invite", day)], day);
        let ICloudAuthState::Authenticated(ref account) = app.icloud_auth else { unreachable!() };
        let same = account.clone();

        let generation = app.fetch_generation;
        let effects = app.handle_msg(Msg::ICloudDiscovered(same.clone()));
        assert!(effects.contains(&Effect::SaveICloudAccount(same.clone())));
        assert_eq!(app.fetch_generation, generation, "nothing changed: nothing refetched");

        let wider = ICloudAccount { addresses: vec!["mailto:me@icloud.com".into(), "/1/principal/".into()], ..same };
        // Discovery may spell the calendar URLs differently (another host);
        // the ones in use are kept
        let moved = ICloudAccount {
            calendars: vec![CalendarEntry { url: "https://p42-cal/1".into(), name: Some("Home".into()) }],
            ..wider.clone()
        };
        let effects = app.handle_msg(Msg::ICloudDiscovered(moved));
        assert!(effects.contains(&Effect::SaveICloudAccount(wider.clone())));
        assert!(app.fetch_generation > generation);
        assert_eq!(app.events.icloud.get(day).len(), 1, "kept on screen until the refetch lands");
        let fetches = app.wanted_fetches(Instant::now());
        assert!(fetches.contains(&Effect::FetchCalDav { month: d(2026, 10, 1), generation: app.fetch_generation, account: wider }));
    }

    #[test]
    fn test_background_rediscovery_failing_keeps_the_saved_calendars() {
        let mut app = ready_app();
        app.handle_msg(Msg::ICloudDiscoveryFailed("offline".into()));
        assert!(matches!(app.icloud_auth, ICloudAuthState::Authenticated(_)));
        assert!(fetched_months(&app.wanted_fetches(Instant::now())).contains(&("icloud", d(2026, 10, 1))));
        // A discovery the user started (no calendars yet) still fails
        app.discover_icloud();
        app.handle_msg(Msg::ICloudDiscoveryFailed("bad password".into()));
        assert!(matches!(app.icloud_auth, ICloudAuthState::Error));
    }

    #[test]
    fn test_refresh_restarts_stuck_fetches() {
        let mut app = ready_app();
        app.wanted_fetches(Instant::now());
        assert!(app.is_loading(EventSource::Google));
        app.apply(Action::Refresh);
        assert_eq!(fetched_months(&app.wanted_fetches(Instant::now())), [("google", d(2026, 10, 1)), ("icloud", d(2026, 10, 1))]);
    }
}
