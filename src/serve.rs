//! `calendarchy serve`: the calendar on the phone. A page (`web/`) that draws the same
//! agenda the shell window reads, served on 127.0.0.1 and put on the tailnet by `tailscale
//! serve`. The family's parts (the gate, the headers, JSON, the page's files with kit.css
//! inline, the theme) are the kit crate's `omarchy::web::server`, as in Lumon.
//!
//! ```text
//! calendarchy serve --allow LOGIN... [--port 8795]
//! ```
//! (`systemd/calendarchy-web.service` runs it; `tailscale serve --bg --https=8795 http://127.0.0.1:8795`)
//!
//! Who gets an answer (`omarchy::web::who`):
//!   - this machine: a loopback request with our own Host and none of the headers `tailscale
//!     serve` adds
//!   - the tailnet: `Tailscale-User-Login` is one of --allow, and there's an X-Forwarded-For
//!   - anyone else: 403
//!
//! A request with an `Origin` that isn't the page's own is refused, and a POST needs that
//! Origin and `X-Calendarchy: 1`, so no other site can act through a browser on the tailnet.
//!
//! ```text
//! GET  /  /app.js  /theme.js  /sw.js  /icon.svg  /apple-touch-icon.png  /icon-192.png  /icon-512.png  /manifest.webmanifest
//! GET  /layout.js                          shell/window/Layout.js, the window's display rules
//! GET  /api/agenda?from=YYYY-MM-DD&days=N  agenda::build from the disk cache (--agenda's JSON), with an
//!                                          ETag that ignores `now`: If-None-Match gets a 304 until it changes
//! GET  /api/theme                          the Omarchy theme's colors.toml as {key: "#hex"}
//! POST /api/refresh?month=YYYY-MM          `calendarchy --refresh --month` (months the sync job skips)
//! POST /api/event?verb=accept|decline&id=ID  `calendarchy --event` (delete stays on the desktop)
//! ```
//!
//! Refreshes and actions run this binary's own headless commands, as the shell window
//! does, one at a time.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use chrono::{Local, NaiveDate};
use omarchy::web::server::{self, Request, Response, Server};
use serde_json::json;

use crate::cache::EventCache;
use crate::config::Config;

const FILES: &server::Files = &[
    ("/", "index.html", "text/html; charset=utf-8"),
    ("/app.js", "app.js", "text/javascript; charset=utf-8"),
    ("/theme.js", "theme.js", "text/javascript; charset=utf-8"),
    ("/sw.js", "sw.js", "text/javascript; charset=utf-8"),
    ("/icon.svg", "icon.svg", "image/svg+xml"),
    ("/apple-touch-icon.png", "apple-touch-icon.png", "image/png"),
    ("/icon-192.png", "icon-192.png", "image/png"),
    ("/icon-512.png", "icon-512.png", "image/png"),
    ("/manifest.webmanifest", "manifest.webmanifest", "application/manifest+json"),
];

pub const DEFAULT_PORT: u16 = 8795;

/// The most days one /api/agenda answers (the page asks for 57: six weeks and a bit)
const MAX_DAYS: u32 = 120;

/// The verbs the phone may use (delete stays on the desktop)
const PHONE_VERBS: [&str; 2] = ["accept", "decline"];

pub struct Web {
    pub port: u16,
    pub allow: Vec<String>,
    /// The page's directory: CALENDARCHY_WEB, else this checkout's web/
    pub web: PathBuf,
    /// The window's display rules, served to the page as /layout.js
    pub layout: PathBuf,
    /// This binary, for `--refresh` and `--event`
    pub exe: PathBuf,
    /// One headless command at a time
    busy: Mutex<()>,
}

impl Web {
    pub fn new(port: u16, allow: Vec<String>) -> Web {
        let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let web = std::env::var_os("CALENDARCHY_WEB").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| checkout.join("web"));
        Web { port, allow, web, layout: checkout.join("shell/window/Layout.js"), exe: own_exe(), busy: Mutex::new(()) }
    }

    /// Run this binary with `args`: Ok, or the last line it said on stderr
    fn run(&self, args: &[&str]) -> Result<(), String> {
        let _one = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        let out = Command::new(&self.exe).args(args).output().map_err(|e| format!("{}: {e}", self.exe.display()))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string())
    }
}

/// This binary as it was started (the ~/.local/bin link survives a rebuild, where
/// /proc/self/exe would point at the replaced file), else what the kernel says
fn own_exe() -> PathBuf {
    std::env::args_os()
        .next()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.exists())
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("calendarchy"))
}

/// `?from=`: a real YYYY-MM-DD date; absent means today
fn from_param(v: Option<&str>) -> Option<Option<NaiveDate>> {
    match v.filter(|v| !v.is_empty()) {
        None => Some(None),
        Some(v) => NaiveDate::parse_from_str(v, "%Y-%m-%d").ok().filter(|_| v.len() == 10).map(Some),
    }
}

/// `?days=`: 1 to MAX_DAYS, 2 when absent (as --agenda)
fn days_param(v: Option<&str>) -> Option<u32> {
    match v.filter(|v| !v.is_empty()) {
        None => Some(2),
        Some(v) => v.parse::<u32>().ok().filter(|d| (1..=MAX_DAYS).contains(d)),
    }
}

/// `?month=YYYY-MM`
fn month_param(v: Option<&str>) -> Option<String> {
    let v = v?;
    (v.len() == 7 && NaiveDate::parse_from_str(&format!("{v}-01"), "%Y-%m-%d").is_ok()).then(|| v.to_string())
}

/// An event id as --agenda prints it: `source:calendar:event`, nothing that could pass for a flag
fn id_ok(id: &str) -> bool {
    (id.starts_with("google:") || id.starts_with("icloud:")) && id.len() < 2048 && !id.chars().any(char::is_control)
}

/// The agenda's JSON and its ETag. The tag leaves out `now` (the minute it was made), so
/// the page's every-minute check is a 304 until something it shows has changed
fn agenda_json(from: Option<NaiveDate>, days: u32) -> (String, String) {
    let config = Config::load().unwrap_or_default();
    let mut cache = EventCache::default();
    cache.load_from_disk();
    let mut agenda = crate::agenda::build(&cache, &config, Local::now(), from, days);
    let now = agenda.now;
    agenda.now = 0;
    let tag = etag(&serde_json::to_string(&agenda).unwrap_or_default());
    agenda.now = now;
    let body = serde_json::to_string(&agenda).unwrap_or_else(|e| json!({"error": e.to_string()}).to_string());
    (body, tag)
}

/// A strong ETag for `body`: its 64-bit FNV-1a, quoted
fn etag(body: &str) -> String {
    let hash = body.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("\"{hash:016x}\"")
}

pub fn handle(w: &Web, req: &Request) -> Response {
    let who = match server::gate(req, w.port, &w.allow) {
        Ok(who) => who,
        Err(r) => return r,
    };
    let origin = req.header("Origin");
    if origin.is_some_and(|o| o != omarchy::web::own_origin(&req.headers, &who)) {
        return Response::error(403, "not this page's origin");
    }
    if req.method == "POST" {
        if origin.is_none() || req.header("X-Calendarchy") != Some("1") {
            return Response::error(403, "a POST needs the page's Origin and X-Calendarchy header");
        }
        return match req.path.as_str() {
            "/api/refresh" => match month_param(req.param("month")) {
                Some(m) => outcome(w.run(&["--refresh", "--month", &m])),
                None => Response::error(400, "month: YYYY-MM"),
            },
            "/api/event" => {
                let Some(verb) = req.param("verb").filter(|v| PHONE_VERBS.contains(v)) else {
                    return Response::error(400, "verb: accept or decline");
                };
                let Some(id) = req.param("id").filter(|id| id_ok(id)) else {
                    return Response::error(400, "id: an event id from the agenda");
                };
                outcome(w.run(&["--event", verb, id]))
            }
            _ => Response::error(404, "no such call in the API"),
        };
    }
    if req.method != "GET" && req.method != "HEAD" {
        return Response::text(405, "only GET and HEAD, and POST /api/refresh and /api/event\n").with("Allow", "GET, HEAD, POST");
    }
    match req.path.as_str() {
        "/api/agenda" => match (from_param(req.param("from")), days_param(req.param("days"))) {
            (Some(from), Some(days)) => {
                let (body, tag) = agenda_json(from, days);
                if req.header("If-None-Match") == Some(tag.as_str()) {
                    Response::new(304, "application/json; charset=utf-8", Vec::new()).with("ETag", &tag).with("Cache-Control", "no-store")
                } else {
                    Response::json_text(200, body).with("ETag", &tag)
                }
            }
            _ => Response::error(400, &format!("from: YYYY-MM-DD, days: 1 to {MAX_DAYS}")),
        },
        "/api/theme" => server::theme("CALENDARCHY_THEME_DIR"),
        "/layout.js" => match std::fs::read_to_string(&w.layout) {
            // QML's library marker is the one line a browser can't read
            Ok(js) => Response::new(200, "text/javascript; charset=utf-8", js.replacen(".pragma library", "", 1)).with("Cache-Control", "no-cache"),
            Err(e) => Response::error(500, &format!("{}: {e}", w.layout.display())),
        },
        path => match server::file(&w.web, FILES, path) {
            None => Response::text(404, "not here\n"),
            Some(Ok(r)) => r,
            Some(Err(e)) => {
                eprintln!("calendarchy serve: {path}: {e}");
                Response::error(500, &e.to_string())
            }
        },
    }
}

fn outcome(r: Result<(), String>) -> Response {
    match r {
        Ok(()) => Response::json(200, &json!({"ok": true})),
        Err(e) => Response::error(502, &e),
    }
}

/// `calendarchy serve`: listen on 127.0.0.1:port until stopped
pub fn serve(args: &[String]) -> i32 {
    let usage = "usage: calendarchy serve --allow LOGIN... [--port N]  (who on the tailnet may see the page)";
    let mut allow = Vec::new();
    let mut port = DEFAULT_PORT;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--allow", Some(login)) => allow.push(login.clone()),
            ("--port", Some(p)) if p.parse::<u16>().is_ok() => port = p.parse().unwrap_or(DEFAULT_PORT),
            _ => {
                eprintln!("{usage}");
                return 2;
            }
        }
    }
    if allow.is_empty() {
        eprintln!("{usage}");
        return 2;
    }
    let server = match Server::bind(&format!("127.0.0.1:{port}")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("calendarchy: can't listen on 127.0.0.1:{port}: {e}");
            return 1;
        }
    };
    let w = Web::new(server.port(), allow);
    eprintln!("calendarchy on http://127.0.0.1:{} (tailnet: {})", w.port, w.allow.join(", "));
    server.run(16 * 1024, move |req| handle(&w, req));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> Web {
        let mut w = Web::new(8795, vec!["me@example.com".into()]);
        // a command that fails loudly if a test ever reaches it
        w.exe = PathBuf::from("/nonexistent/calendarchy");
        w
    }

    fn req(method: &str, url: &str, extra: &[(&str, &str)]) -> Request {
        let mut headers = vec![("Host".to_string(), "127.0.0.1:8795".to_string())];
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Request::new(method, url, headers, true, vec![])
    }

    fn tailnet(method: &str, url: &str, login: &str, extra: &[(&str, &str)]) -> Request {
        let mut headers = vec![
            ("Host".to_string(), "omarchy.example.ts.net:8795".to_string()),
            ("Tailscale-User-Login".to_string(), login.to_string()),
            ("X-Forwarded-For".to_string(), "100.64.0.2".to_string()),
        ];
        headers.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Request::new(method, url, headers, true, vec![])
    }

    #[test]
    fn strangers_get_nothing() {
        let w = web();
        assert_eq!(handle(&w, &tailnet("GET", "/api/agenda", "someone@else.com", &[])).status, 403);
        assert_eq!(handle(&w, &tailnet("GET", "/", "someone@else.com", &[])).status, 403);
        assert_eq!(handle(&w, &tailnet("GET", "/api/theme", "me@example.com", &[])).status, 200);
    }

    #[test]
    fn params_are_checked() {
        assert_eq!(from_param(None), Some(None));
        assert_eq!(from_param(Some("2026-11-02")), Some(NaiveDate::from_ymd_opt(2026, 11, 2)));
        assert_eq!(from_param(Some("2026-11-2")), None);
        assert_eq!(from_param(Some("2026-13-01")), None);
        assert_eq!(days_param(None), Some(2));
        assert_eq!(days_param(Some("57")), Some(57));
        assert_eq!(days_param(Some("0")), None);
        assert_eq!(days_param(Some("121")), None);
        assert_eq!(month_param(Some("2027-01")), Some("2027-01".into()));
        assert_eq!(month_param(Some("2027-1")), None);
        assert_eq!(month_param(Some("--help")), None);
        assert!(id_ok("google:primary:abc123"));
        assert!(!id_ok("--refresh"));
        assert!(!id_ok("google:a\nb"));
        let w = web();
        assert_eq!(handle(&w, &req("GET", "/api/agenda?days=500", &[])).status, 400);
        assert_eq!(handle(&w, &req("GET", "/api/agenda?from=tomorrow", &[])).status, 400);
    }

    #[test]
    fn posts_need_our_origin_and_header() {
        let w = web();
        let own = [("Origin", "http://127.0.0.1:8795")];
        let both = [("Origin", "http://127.0.0.1:8795"), ("X-Calendarchy", "1")];
        assert_eq!(handle(&w, &req("POST", "/api/refresh?month=2027-01", &[])).status, 403);
        assert_eq!(handle(&w, &req("POST", "/api/refresh?month=2027-01", &own)).status, 403);
        assert_eq!(handle(&w, &req("POST", "/api/refresh?month=2027-01", &[("Origin", "https://evil.example"), ("X-Calendarchy", "1")])).status, 403);
        assert_eq!(handle(&w, &req("GET", "/api/agenda", &[("Origin", "https://evil.example")])).status, 403);
        // past the checks it runs the command (here one that isn't there: 502)
        assert_eq!(handle(&w, &req("POST", "/api/refresh?month=2027-01", &both)).status, 502);
        assert_eq!(handle(&w, &req("POST", "/api/refresh?month=jan", &both)).status, 400);
    }

    #[test]
    fn the_phone_never_deletes() {
        let w = web();
        let both = [("Origin", "http://127.0.0.1:8795"), ("X-Calendarchy", "1")];
        assert_eq!(handle(&w, &req("POST", "/api/event?verb=delete&id=google:primary:x", &both)).status, 400);
        assert_eq!(handle(&w, &req("POST", "/api/event?verb=accept&id=--help", &both)).status, 400);
        assert_eq!(handle(&w, &req("POST", "/api/event?verb=accept&id=google:primary:x", &both)).status, 502);
    }

    #[test]
    fn an_unchanged_agenda_is_a_304() {
        let w = web();
        let first = handle(&w, &req("GET", "/api/agenda?from=2026-01-05&days=7", &[]));
        assert_eq!(first.status, 200);
        let tag = first.header("ETag").expect("an ETag").to_string();
        let again = handle(&w, &req("GET", "/api/agenda?from=2026-01-05&days=7", &[("If-None-Match", &tag)]));
        assert_eq!(again.status, 304);
        assert!(again.body.is_empty());
        let other = handle(&w, &req("GET", "/api/agenda?from=2026-01-05&days=7", &[("If-None-Match", "\"stale\"")]));
        assert_eq!(other.status, 200);
    }

    #[test]
    fn layout_js_loses_its_qml_marker() {
        let w = web();
        let r = handle(&w, &req("GET", "/layout.js", &[]));
        assert_eq!(r.status, 200);
        let js = String::from_utf8(r.body).unwrap();
        assert!(!js.contains(".pragma library"));
        assert!(js.contains("function gaps("));
    }

    #[test]
    fn the_page_and_its_files_are_served() {
        let w = web();
        for path in ["/", "/app.js", "/theme.js", "/sw.js", "/manifest.webmanifest", "/icon.svg", "/apple-touch-icon.png", "/icon-192.png", "/icon-512.png"] {
            assert_eq!(handle(&w, &req("GET", path, &[])).status, 200, "{path}");
        }
        let page = String::from_utf8(handle(&w, &req("GET", "/", &[])).body).unwrap();
        assert!(page.contains("@layer kit"), "kit.css is put inline");
        assert_eq!(handle(&w, &req("GET", "/nope", &[])).status, 404);
    }
}
