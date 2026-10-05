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
    let (keymap, key_problems) = keymap::Keymap::with_overrides(&app.config.keys);
    keymap::install(keymap);
    if !key_problems.is_empty() {
        app.set_error(format!("config keys: {}", key_problems.join("; ")));
    }

    // Start the network before touching the terminal, so requests are in
    // flight while the first frame is drawn
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let mut runtime = Runtime::new(tx);
    let startup = app.init_sources();
    runtime.run_all(&mut app, startup);
    let fetches = app.wanted_fetches(Instant::now());
    runtime.run_all(&mut app, fetches);

    // Terminal background: what this terminal said last time. A terminal that
    // never answered is not asked again (that costs ~200ms and its late reply
    // could arrive as keystrokes); with no memory at all, ask before drawing.
    // Answers are remembered per terminal, so one that can't answer (tmux, an
    // ssh session) doesn't stop the others from being asked.
    ui::load_palette();
    let remembered = load_term_bg();
    match remembered {
        Some(TermBg::Color(r, g, b)) => ui::set_term_bg(r, g, b),
        Some(TermBg::Unanswered) => {}
        None => {
            if let Some((r, g, b)) = probe_term_bg() {
                ui::set_term_bg(r, g, b);
            }
        }
    }

    // Raw mode + alternate screen; we restore on every return path below
    let mut terminal = ratatui::try_init()?;
    install_panic_hook();
    let reprobe = matches!(remembered, Some(TermBg::Color(..)));
    let result = run(&mut terminal, &mut app, &mut runtime, &mut rx, reprobe).await;
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
    reprobe_term_bg: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    draw(terminal, app)?;

    // The theme may have changed since last run: ask again now that the first
    // frame is up (a terminal that answered before answers within a few ms).
    // termbg reads the reply from stdin, so this must finish before the
    // input thread starts.
    if reprobe_term_bg {
        let probed = tokio::task::spawn_blocking(probe_term_bg).await.ok().flatten();
        if let Some(rgb) = probed
            && ui::get_term_bg() != Some(rgb)
        {
            ui::set_term_bg(rgb.0, rgb.1, rgb.2);
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
        display: &app.config.display,
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

/// What we learned about the terminal's background on a previous run
enum TermBg {
    Color(u8, u8, u8),
    Unanswered,
}

/// Which terminal we're in, as far as the environment tells: multiplexers and
/// ssh first (they decide whether OSC 11 gets answered), then the emulator
fn terminal_identity() -> String {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let mut parts = Vec::new();
    if var("TMUX").is_some() {
        parts.push("tmux".to_string());
    }
    if var("ZELLIJ").is_some() {
        parts.push("zellij".to_string());
    }
    if var("SSH_TTY").is_some() || var("SSH_CONNECTION").is_some() {
        parts.push("ssh".to_string());
    }
    parts.push(var("TERM_PROGRAM").or_else(|| var("TERM")).unwrap_or_else(|| "unknown".into()));
    // One token, so it can't be confused with the value on its line
    parts.join("+").replace(char::is_whitespace, "_")
}

/// The remembered answer for `terminal` in the term-bg file: one
/// "<terminal> <rrggbb|none>" line per terminal
fn parse_term_bg(text: &str, terminal: &str) -> Option<TermBg> {
    let value = text.lines().find_map(|line| {
        let (who, value) = line.trim().split_once(' ')?;
        (who == terminal).then_some(value.trim())
    })?;
    if value == "none" {
        return Some(TermBg::Unanswered);
    }
    if value.len() != 6 {
        return None;
    }
    let [_, r, g, b] = u32::from_str_radix(value, 16).ok()?.to_be_bytes();
    Some(TermBg::Color(r, g, b))
}

/// The term-bg file with `terminal`'s line set to `value`, others kept.
/// Lines from the old single-value format (no terminal name) are dropped.
fn update_term_bg(text: &str, terminal: &str, value: &str) -> String {
    let mut out: String = text
        .lines()
        .filter(|line| line.trim().split_once(' ').is_some_and(|(who, _)| who != terminal))
        .map(|line| format!("{}\n", line.trim()))
        .collect();
    out.push_str(&format!("{} {}\n", terminal, value));
    out
}

fn load_term_bg() -> Option<TermBg> {
    let text = std::fs::read_to_string(term_bg_path()?).ok()?;
    parse_term_bg(&text, &terminal_identity())
}

/// Ask the terminal for its background via OSC 11, remembering the outcome
fn probe_term_bg() -> Option<(u8, u8, u8)> {
    let answer = termbg::rgb(Duration::from_millis(120))
        .ok()
        .map(|rgb| ((rgb.r >> 8) as u8, (rgb.g >> 8) as u8, (rgb.b >> 8) as u8));
    if let Some(path) = term_bg_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let value = match answer {
            Some((r, g, b)) => format!("{:02x}{:02x}{:02x}", r, g, b),
            None => "none".to_string(),
        };
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::write(path, update_term_bg(&old, &terminal_identity(), &value));
    }
    answer
}

/// Restore the terminal only when the main thread panics. A panic in a
/// background task is caught by tokio (that fetch just fails), so the UI must
/// stay up; its message goes to the request log instead of over the screen.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            ratatui::restore();
            default_hook(info);
        } else {
            logging::log_request("PANIC", &info.to_string());
        }
    }));
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
            // Timeouts so a request stuck on a dead connection (e.g. after the
            // laptop wakes) fails and is retried instead of hanging forever
            http: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .pool_idle_timeout(Duration::from_secs(90))
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

    /// Like `spawn`, but if the task panics send `on_panic` instead, so the
    /// app never waits forever on a result that can't arrive
    fn spawn_or<F>(&self, on_panic: Msg, task: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let handle = tokio::spawn(task);
        tokio::spawn(async move {
            let _ = tx.send(handle.await.unwrap_or(on_panic));
        });
    }

    fn run(&mut self, app: &mut App, effect: Effect) {
        let http = self.http.clone();
        match effect {
            Effect::FetchGoogle { month, generation, tokens, calendar_id, known_name } => {
                let Some(gconfig) = app.config.google.clone() else { return };
                let failed = fetch_failed(EventSource::Google, month, generation, "fetch task crashed");
                self.spawn_or(failed, async move {
                    let mut session = GoogleSession::new(http.clone(), gconfig, tokens);
                    let result = sources::fetch_google_month(&http, &mut session, &calendar_id, known_name, month).await;
                    let tokens = session.into_refreshed();
                    match result {
                        Ok(m) => Msg::Fetched {
                            source: EventSource::Google,
                            month,
                            generation,
                            events: m.events,
                            calendar_name: m.calendar_name,
                            tokens,
                        },
                        Err(e) => Msg::FetchFailed {
                            source: EventSource::Google,
                            month,
                            generation,
                            auth: is_auth_failure(&e),
                            error: e.to_string(),
                            tokens,
                        },
                    }
                });
            }
            Effect::FetchCalDav { month, generation, calendars } => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                let failed = fetch_failed(EventSource::ICloud, month, generation, "fetch task crashed");
                self.spawn_or(failed, async move {
                    match sources::fetch_caldav_month(&http, &icloud, &calendars, month).await {
                        Ok(events) => fetched(EventSource::ICloud, month, generation, events),
                        Err(e) => fetch_failed(EventSource::ICloud, month, generation, &e.to_string()),
                    }
                });
            }
            Effect::FetchEventKit { month, generation } => {
                let failed = fetch_failed(EventSource::ICloud, month, generation, "fetch task crashed");
                self.spawn_or(failed, async move {
                    match sources::fetch_eventkit_month(month).await {
                        Ok(events) => fetched(EventSource::ICloud, month, generation, events),
                        // The helper's errors already say "EventKit: ..."
                        Err(e) => fetch_failed(EventSource::ICloud, month, generation, &e),
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

fn fetched(source: EventSource, month: chrono::NaiveDate, generation: u64, events: Vec<cache::DisplayEvent>) -> Msg {
    Msg::Fetched { source, month, generation, events, calendar_name: None, tokens: None }
}

fn fetch_failed(source: EventSource, month: chrono::NaiveDate, generation: u64, error: &str) -> Msg {
    Msg::FetchFailed { source, month, generation, auth: false, error: error.to_string(), tokens: None }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_term_bg_is_remembered_per_terminal() {
        // tmux couldn't answer; foot could. Each keeps its own answer.
        let text = update_term_bg("", "tmux+foot", "none");
        let text = update_term_bg(&text, "foot", "fafafa");
        assert!(matches!(parse_term_bg(&text, "tmux+foot"), Some(TermBg::Unanswered)));
        assert!(matches!(parse_term_bg(&text, "foot"), Some(TermBg::Color(0xfa, 0xfa, 0xfa))));
        assert!(parse_term_bg(&text, "alacritty").is_none(), "unknown terminals get asked");

        // A new answer replaces that terminal's line only
        let text = update_term_bg(&text, "foot", "1e2026");
        assert!(matches!(parse_term_bg(&text, "foot"), Some(TermBg::Color(0x1e, 0x20, 0x26))));
        assert!(matches!(parse_term_bg(&text, "tmux+foot"), Some(TermBg::Unanswered)));
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn test_old_single_value_term_bg_file_is_ignored() {
        // The previous format held one value for every terminal
        assert!(parse_term_bg("none\n", "foot").is_none());
        assert_eq!(update_term_bg("none\n", "foot", "fafafa"), "foot fafafa\n");
    }

    #[tokio::test]
    async fn test_a_panicking_task_still_reports_back() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = Runtime::new(tx);
        let month = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        runtime.spawn_or(fetch_failed(EventSource::Google, month, 7, "fetch task crashed"), async {
            panic!("boom");
        });
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(Msg::FetchFailed { generation: 7, error, .. })) => assert_eq!(error, "fetch task crashed"),
            _ => panic!("expected the fallback message"),
        }
        runtime.finish();
    }
}
