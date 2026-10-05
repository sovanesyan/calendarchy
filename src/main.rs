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
mod keymap;
mod logging;
mod setup;
mod sources;
mod ui;
mod update;
mod utils;

use std::io::stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use reqwest::Client;
use tokio::sync::mpsc;

use app::{App, EventSource};
use cache::EventCache;
use config::Config;
use google::{CalendarClient, GoogleAuth};
use icloud::{CalDavClient, ICloudAuth};
use sources::{is_auth_failure, GoogleSession};
use update::{Effect, Msg};
use utils::open_url;

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
    app.config = Config::load().unwrap_or_default();

    // Start the network before touching the terminal, so requests are in
    // flight while the first frame is drawn
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut runtime = Runtime::new(tx);
    let startup = app.init_sources();
    runtime.run_all(&mut app, startup);
    let fetches = app.wanted_fetches(Instant::now());
    runtime.run_all(&mut app, fetches);

    // Draw with last run's terminal background until the terminal answers
    if let Some((r, g, b)) = load_term_bg() {
        ui::set_term_bg(r, g, b);
    }

    // Raw mode + alternate screen; ratatui also installs a panic hook that
    // restores the terminal, and we restore on every return path below
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal, &mut app, &mut runtime, &mut rx).await;
    ratatui::restore();
    runtime.finish();
    result
}

/// Main loop: sleep until a key, a background result or a timer is due;
/// apply everything that's pending; draw once.
async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    runtime: &mut Runtime,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
) -> Result<(), Box<dyn std::error::Error>> {
    draw(terminal, app)?;

    // Ask the terminal for its background (OSC 11) now that the first frame
    // is up. termbg reads the reply from stdin, so this must finish before
    // the input thread starts; terminals that never answer cost ~200ms here
    // instead of before the first frame.
    let probed = tokio::task::spawn_blocking(|| termbg::rgb(Duration::from_millis(120))).await;
    if let Ok(Ok(rgb)) = probed {
        let rgb = ((rgb.r >> 8) as u8, (rgb.g >> 8) as u8, (rgb.b >> 8) as u8);
        if ui::get_term_bg() != Some(rgb) {
            ui::set_term_bg(rgb.0, rgb.1, rgb.2);
            save_term_bg(rgb);
            app.dirty = true;
        }
    }

    let mut input = spawn_input_reader();

    loop {
        if app.dirty {
            draw(terminal, app)?;
        }
        if app.quit {
            return Ok(());
        }

        tokio::select! {
            event = input.recv() => match event {
                Some(event) => handle_event(app, runtime, event),
                None => return Ok(()), // stdin closed
            },
            Some(msg) = rx.recv() => {
                let effects = app.handle_msg(msg);
                runtime.run_all(app, effects);
            }
            _ = tokio::time::sleep(app.next_wakeup()) => {}
        }

        // Apply everything already queued before drawing again, so key
        // repeat or a burst of results costs one frame, not one per item
        while !app.quit
            && let Ok(event) = input.try_recv()
        {
            handle_event(app, runtime, event);
        }
        while let Ok(msg) = rx.try_recv() {
            let effects = app.handle_msg(msg);
            runtime.run_all(app, effects);
        }

        app.tick();
        let fetches = app.wanted_fetches(Instant::now());
        runtime.run_all(app, fetches);
        runtime.flush_cache(app);
    }
}

fn handle_event(app: &mut App, runtime: &mut Runtime, event: Event) {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            let effects = app.handle_key(key);
            runtime.run_all(app, effects);
        }
        Event::Resize(_, _) => app.dirty = true,
        _ => {}
    }
}

fn draw(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    app.dirty = false;
    let state = ui::RenderState {
        current_date: app.current_date,
        selected_date: app.selected_date,
        events: &app.events,
        google_auth: &app.google_auth,
        icloud_auth: &app.icloud_auth,
        status_message: app.status_message.as_deref(),
        status_is_error: app.status_is_error,
        google_loading: app.is_loading(EventSource::Google),
        icloud_loading: app.is_loading(EventSource::ICloud),
        navigation_mode: app.navigation_mode,
        selected_source: app.selected_source,
        selected_event_index: app.selected_event_index,
        show_logs: app.show_logs,
        pending_action: app.pending_action.as_ref(),
        search: app.search.as_ref(),
        show_help: app.show_help,
        setup: app.setup.as_ref(),
        now: chrono::Local::now(),
    };
    // ratatui diffs against the previous frame and flushes once; the
    // synchronized-update bracket makes the frame appear atomically
    execute!(stdout(), BeginSynchronizedUpdate)?;
    terminal.draw(|frame| ui::render(frame, &state))?;
    execute!(stdout(), EndSynchronizedUpdate)?;
    Ok(())
}

/// Blocking terminal reads on a dedicated thread, forwarded to the loop
fn spawn_input_reader() -> mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    rx
}

fn term_bg_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("calendarchy").join("term-bg"))
}

fn load_term_bg() -> Option<(u8, u8, u8)> {
    let text = std::fs::read_to_string(term_bg_path()?).ok()?;
    let packed = u32::from_str_radix(text.trim(), 16).ok()?;
    let [_, r, g, b] = packed.to_be_bytes();
    Some((r, g, b))
}

fn save_term_bg((r, g, b): (u8, u8, u8)) {
    if let Some(path) = term_bg_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, format!("{:02x}{:02x}{:02x}\n", r, g, b));
    }
}

/// Performs effects: spawns network tasks (sharing one HTTP client), writes
/// files, opens URLs. Results come back to the loop as `Msg`s.
struct Runtime {
    http: Client,
    tx: mpsc::UnboundedSender<Msg>,
    cache_dirty: bool,
    cache_writer: CacheWriter,
}

impl Runtime {
    fn new(tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self {
            http: Client::builder()
                .pool_idle_timeout(Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
            tx,
            cache_dirty: false,
            cache_writer: CacheWriter::spawn(),
        }
    }

    fn run_all(&mut self, app: &mut App, effects: Vec<Effect>) {
        for effect in effects {
            self.run(app, effect);
        }
    }

    fn spawn<F>(&self, task: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(task.await);
        });
    }

    fn run(&mut self, app: &mut App, effect: Effect) {
        let http = self.http.clone();
        match effect {
            Effect::FetchGoogle { month, tokens, calendar_id, known_name } => {
                let Some(gconfig) = app.config.google.clone() else { return };
                self.spawn(async move {
                    let mut session = GoogleSession::new(http.clone(), gconfig, tokens);
                    let result = sources::fetch_google_month(&http, &mut session, &calendar_id, known_name, month).await;
                    let tokens = session.into_refreshed();
                    match result {
                        Ok(m) => Msg::Fetched {
                            source: EventSource::Google,
                            month,
                            events: m.events,
                            calendar_name: m.calendar_name,
                            tokens,
                        },
                        Err(e) => Msg::FetchFailed {
                            source: EventSource::Google,
                            month,
                            auth: is_auth_failure(&e),
                            error: e.to_string(),
                            tokens,
                        },
                    }
                });
            }
            Effect::FetchCalDav { month, calendars } => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                self.spawn(async move {
                    match sources::fetch_caldav_month(&http, &icloud, &calendars, month).await {
                        Ok(events) => Msg::Fetched { source: EventSource::ICloud, month, events, calendar_name: None, tokens: None },
                        Err(e) => Msg::FetchFailed { source: EventSource::ICloud, month, auth: false, error: e.to_string(), tokens: None },
                    }
                });
            }
            Effect::FetchEventKit { month } => {
                self.spawn(async move {
                    match sources::fetch_eventkit_month(month).await {
                        Ok(events) => Msg::Fetched { source: EventSource::ICloud, month, events, calendar_name: None, tokens: None },
                        Err(e) => Msg::FetchFailed {
                            source: EventSource::ICloud,
                            month,
                            auth: false,
                            error: format!("EventKit: {}", e),
                            tokens: None,
                        },
                    }
                });
            }
            Effect::StartGoogleAuth => {
                let Some(gconfig) = app.config.google.clone() else { return };
                let auth = GoogleAuth::with_client(http, gconfig);
                open_url(&auth.auth_url());
                self.spawn(async move {
                    match auth.authenticate_with_browser().await {
                        Ok(tokens) => Msg::GoogleSignedIn(tokens),
                        Err(e) => Msg::GoogleSignInFailed(e.to_string()),
                    }
                });
            }
            Effect::DiscoverICloud => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                self.spawn(async move {
                    match sources::discover_icloud(&http, &icloud).await {
                        Ok(calendars) => Msg::ICloudDiscovered(calendars),
                        Err(e) => Msg::ICloudDiscoveryFailed(e.to_string()),
                    }
                });
            }
            Effect::RespondToEvent { tokens, calendar_id, event_id, response } => {
                let Some(gconfig) = app.config.google.clone() else { return };
                let (done, verb) = if response == "accepted" { ("Event accepted", "accept") } else { ("Event declined", "decline") };
                self.spawn(async move {
                    let client = CalendarClient::with_client(http.clone());
                    let client = &client;
                    let (calendar_id, event_id) = (&calendar_id, &event_id);
                    let mut session = GoogleSession::new(http, gconfig, tokens);
                    let result = session
                        .call(|t| async move { client.respond_to_event(&t, calendar_id, event_id, response).await })
                        .await;
                    action_result(result, done, verb, session.into_refreshed())
                });
            }
            Effect::DeleteGoogleEvent { tokens, calendar_id, event_id } => {
                let Some(gconfig) = app.config.google.clone() else { return };
                self.spawn(async move {
                    let client = CalendarClient::with_client(http.clone());
                    let client = &client;
                    let (calendar_id, event_id) = (&calendar_id, &event_id);
                    let mut session = GoogleSession::new(http, gconfig, tokens);
                    let result = session
                        .call(|t| async move { client.delete_event(&t, calendar_id, event_id).await })
                        .await;
                    action_result(result, "Event deleted", "delete", session.into_refreshed())
                });
            }
            Effect::DeleteICloudEvent { calendar_url, event_uid, href, etag } => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                self.spawn(async move {
                    let client = CalDavClient::with_client(http, ICloudAuth::new(icloud));
                    let result = client.delete_event(&calendar_url, &event_uid, href.as_deref(), etag.as_deref()).await;
                    action_result(result, "Event deleted", "delete", None)
                });
            }
            Effect::OpenUrl(url) => open_url(&url),
            Effect::SaveCache => self.cache_dirty = true,
            Effect::SaveGoogleTokens(tokens) => {
                if let Err(e) = config::save_google_tokens(&tokens) {
                    app.set_error(format!("Couldn't save Google sign-in: {}", e));
                }
            }
            Effect::SaveICloudCalendars(calendars) => {
                let stored: Vec<config::StoredCalendar> = calendars
                    .iter()
                    .map(|c| config::StoredCalendar { url: c.url.clone(), name: c.name.clone() })
                    .collect();
                if let Err(e) = config::save_icloud_tokens(&stored) {
                    app.set_error(format!("Couldn't save iCloud calendars: {}", e));
                }
            }
        }
    }

    /// Hand the cache to the writer thread if anything changed this round
    fn flush_cache(&mut self, app: &App) {
        if std::mem::take(&mut self.cache_dirty)
            && let Some(bytes) = app.events.to_disk_bytes()
        {
            self.cache_writer.write(bytes);
        }
    }

    /// Wait for the last cache write to land
    fn finish(self) {
        self.cache_writer.finish();
    }
}

fn action_result<T>(
    result: error::Result<T>,
    done: &str,
    verb: &str,
    tokens: Option<google::TokenInfo>,
) -> Msg {
    match result {
        Ok(_) => Msg::ActionDone { message: done.to_string(), tokens },
        Err(e) => Msg::ActionFailed { auth: is_auth_failure(&e), message: format!("Failed to {}: {}", verb, e), tokens },
    }
}

/// Writes the event cache on its own thread. Saves that pile up while one is
/// being written collapse into the newest, so a burst of results costs one write.
struct CacheWriter {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
    handle: std::thread::JoinHandle<()>,
}

impl CacheWriter {
    fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let handle = std::thread::spawn(move || {
            while let Ok(mut bytes) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    bytes = newer;
                }
                EventCache::write_disk_bytes(&bytes);
            }
        });
        Self { tx, handle }
    }

    fn write(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(bytes);
    }

    fn finish(self) {
        drop(self.tx);
        let _ = self.handle.join();
    }
}
