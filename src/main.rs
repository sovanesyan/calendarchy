mod agenda;
mod app;
mod auth;
mod cache;
mod config;
mod conversion;
mod error;
mod events;
mod google;
mod headless;
mod icloud;
mod keymap;
mod logging;
mod setup;
mod sources;
mod term_bg;
mod ui;
mod update;
mod utils;

use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use reqwest::Client;
use tokio::sync::mpsc;

use app::{App, EventSource};
use cache::CacheWriter;
use config::Config;
use google::GoogleAuth;
use sources::{is_auth_failure, EventVerb, GoogleSession};
use term_bg::TermBg;
use update::{Effect, Msg};
use utils::open_url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("calendarchy {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("events") {
        std::process::exit(events::print(&args[2..]).await);
    }

    let date_arg = |name: &str| arg_value(name).and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());

    if std::env::args().any(|a| a == "--refresh") {
        let month = arg_value("--month").and_then(|m| chrono::NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").ok());
        return headless::refresh(month).await;
    }

    if std::env::args().any(|a| a == "--agenda") {
        let days = arg_value("--days").and_then(|d| d.parse().ok()).unwrap_or(2);
        return agenda::print(date_arg("--from"), days);
    }

    if let Some(verb) = arg_value("--event") {
        let args: Vec<String> = std::env::args().collect();
        let id = args.iter().skip_while(|a| *a != "--event").nth(2).ok_or("usage: calendarchy --event accept|decline|delete <id>")?;
        // Plain message on failure: the shell window shows stderr's last line
        if let Err(e) = headless::event_action(&verb, id).await {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut app = App::new();
    app.config = Config::load().unwrap_or_default();
    // `--date YYYY-MM-DD` opens on that day (the shell plugin's "open")
    if let Some(date) = date_arg("--date") {
        app.goto_date(date);
    }
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
    let remembered = term_bg::load();
    match remembered {
        Some(TermBg::Color(r, g, b)) => ui::set_term_bg(r, g, b),
        Some(TermBg::Unanswered) => {}
        None => {
            if let Some((r, g, b)) = term_bg::probe() {
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
        let probed = tokio::task::spawn_blocking(term_bg::probe).await.ok().flatten();
        if let Some(rgb) = probed
            && ui::get_term_bg() != Some(rgb)
        {
            ui::set_term_bg(rgb.0, rgb.1, rgb.2);
            app.dirty = true;
        }
    }

    let mut input = spawn_input_reader();
    let mut theme_check = tokio::time::interval(THEME_CHECK);

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
            _ = theme_check.tick() => {
                if let Some(palette) = ui::load_palette() {
                    // A theme switch repaints Omarchy's terminals with the
                    // theme's background, so that's the background now (the
                    // terminal can't be asked again while the input thread
                    // owns stdin)
                    if let Some((r, g, b)) = palette.background {
                        ui::set_term_bg(r, g, b);
                    }
                    app.dirty = true;
                }
            }
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

/// How often to look for a desktop theme switch (one small file read)
const THEME_CHECK: Duration = Duration::from_secs(2);

/// The argument after `name`, e.g. `--days 3`
fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip_while(|a| a != name);
    args.next()?;
    args.next()
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
    // ratatui diffs against the previous frame and flushes once; the
    // synchronized-update bracket makes the frame appear atomically
    execute!(stdout(), BeginSynchronizedUpdate)?;
    terminal.draw(|frame| ui::render(frame, app, chrono::Local::now()))?;
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
            logging::log_panic(&info.to_string());
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
            http: sources::http_client(),
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

    /// Run `task` and send its result back; if it panics send `on_panic`
    /// instead, so the app never waits forever on a result that can't arrive
    /// (stuck on "Authenticating..." or "Deleting event...")
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
            Effect::FetchCalDav { month, generation, account } => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                let failed = fetch_failed(EventSource::ICloud, month, generation, "fetch task crashed");
                self.spawn_or(failed, async move {
                    match sources::fetch_caldav_month(&http, &icloud, &account, month).await {
                        Ok(events) => fetched(EventSource::ICloud, month, generation, events),
                        Err(e) => fetch_failed(EventSource::ICloud, month, generation, &e.to_string()),
                    }
                });
            }
            Effect::StartGoogleAuth => {
                let Some(gconfig) = app.config.google.clone() else { return };
                let auth = GoogleAuth::new(http, gconfig);
                open_url(&auth.auth_url());
                self.spawn_or(Msg::GoogleSignInFailed("sign-in task crashed".into()), async move {
                    match auth.authenticate_with_browser().await {
                        Ok(tokens) => Msg::GoogleSignedIn(tokens),
                        Err(e) => Msg::GoogleSignInFailed(e.to_string()),
                    }
                });
            }
            Effect::DiscoverICloud => {
                let Some(icloud) = app.config.icloud.clone() else { return };
                self.spawn_or(Msg::ICloudDiscoveryFailed("discovery task crashed".into()), async move {
                    match sources::discover_icloud(&http, &icloud).await {
                        Ok(account) => Msg::ICloudDiscovered(account),
                        Err(e) => Msg::ICloudDiscoveryFailed(e.to_string()),
                    }
                });
            }
            Effect::EventAction { verb, id, tokens } => {
                let google = app.config.google.clone().zip(tokens);
                let icloud = app.config.icloud.clone();
                self.spawn_or(action_crashed(verb), async move {
                    let mut session = google.map(|(gconfig, tokens)| GoogleSession::new(http.clone(), gconfig, tokens));
                    let result = sources::event_action(&http, session.as_mut(), icloud.as_ref(), &id, verb).await;
                    action_result(result, verb, session.and_then(GoogleSession::into_refreshed))
                });
            }
            Effect::OpenUrl(url) => open_url(&url),
            Effect::SaveCache => self.cache_dirty = true,
            Effect::SaveGoogleTokens(tokens) => {
                if let Err(e) = config::save_google_tokens(&tokens) {
                    app.set_error(format!("Couldn't save Google sign-in: {}", e));
                }
            }
            Effect::SaveICloudAccount(account) => {
                if let Err(e) = config::save_icloud_account(&account) {
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

/// What an event action reports if its task dies
fn action_crashed(verb: EventVerb) -> Msg {
    Msg::ActionFailed { message: format!("Failed to {}: task crashed", verb.name()), auth: false, tokens: None }
}

fn action_result(result: error::Result<()>, verb: EventVerb, tokens: Option<google::TokenInfo>) -> Msg {
    match result {
        Ok(()) => Msg::ActionDone { message: format!("Event {}", verb.done()), tokens },
        Err(e) => Msg::ActionFailed { auth: is_auth_failure(&e), message: format!("Failed to {}: {}", verb.name(), e), tokens },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
