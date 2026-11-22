# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (what's installed and run)
cargo test               # Run all tests
cargo test cache         # Run tests in cache module only
cargo test icloud        # Run tests in icloud module only
```

## Architecture

Calendarchy is a terminal calendar app that displays Google Calendar and iCloud Calendar events side by side.

### Core Flow

1. **Startup**: `main.rs` loads config, restores cached events from disk for instant display, starts fetches, draws the first frame
2. **Auth**: Google uses OAuth with a loopback redirect (tokens auto-refresh, incl. on 401); iCloud uses app-specific password with CalDAV discovery
3. **Fetching**: `App::wanted_fetches` decides from state what to fetch — visible month first, then neighbours; visible month refreshes every 5 min. Results are split into per-day `DisplayEvent` occurrences and cached to disk
4. **Rendering**: `ui.rs` renders a month calendar grid and two event panels into a ratatui buffer; ratatui diffs frames and writes only changed cells

### Module Structure

- **`main.rs`** - CLI dispatch (`--version`, `--help`, `events`, `put`, `remove`, `serve`, `--refresh`, `--agenda`, `--event`; `--date YYYY-MM-DD` opens the TUI on that day, used by the shell window's "open"; with no terminal it prints the usage and exits 2 instead of starting the TUI) and the runtime: event-driven loop (input thread, results channel, timer, theme-switch check), executes `Effect`s
- **`app.rs`** - `App` state and navigation
- **`update.rs`** - Pure update logic: `handle_key` / `handle_msg` → `Effect`s, fetch scheduling (no I/O; unit-tested)
- **`keymap.rs`** - Bindings table: keys → `Action` per mode (Bulgarian phonetic keys normalised); also generates the help overlay and applies `"keys"` overrides from config
- **`sources.rs`** - Fetch and event-action layer shared by the TUI and the headless commands: shared HTTP client, `GoogleSession` token refresh, concurrent CalDAV, `event_action` + `check_event_action` (accept/decline/delete and their rules)
- **`headless.rs`** - `--refresh [--month YYYY-MM]`: fetch this month and the next (or one month) into the disk cache, no TUI (used by the shell plugins and the TRMNL job); `--event accept|decline|delete <id>`: one action by agenda id, then refetch its month
- **`agenda.rs`** - `--agenda [--from YYYY-MM-DD] [--days N]`: days as JSON from the disk cache only (no network): events with details, attendees and busy ranges, display prefs, the effective key bindings (after `"keys"` overrides), and the event that's on now or next (always from now, 8 days ahead; `find_next`, which the TUI's status bar shares); read by the shell plugins. The JSON shape is documented at the top of the file
- **`events.rs`** - `events [--since DATE] [--until DATE] [--json]`: the Google calendar's events for agents (Dossier's `/stress`, `/grade`), fetched live with the saved login, never via the cache (the TUI rewrites the cache whole, so it can't hold history). Its JSON follows the connector contract's record shape (`~/Work/my/connectors/README.md`); documented at the top of the file
- **`write.rs`** - `put --calendar NAME --uid UID --start --end --title [--notes] [--alarm MIN]` and `remove --calendar NAME --uid UID`: create, replace or delete an iCloud event we own at `<calendar>/<uid>.ics` (CalDAV `PUT`/`DELETE`), for jobs such as the eBag deliveries on Family (`~/Work/my/deliveries/deliveries-calendar`). Never touches an event it didn't name; test it on `Home`, not the shared `Family`
- **`serve.rs`** - `serve --allow LOGIN... [--port 8795]`: the phone page (`web/`) and its API on 127.0.0.1, behind the kit crate's tailnet gate (`omarchy::web::server`, a path dependency on `~/Work/rb/ghostwheel/kit/crate`). See "Phone page" below
- **`ui.rs`** - Rendering into a ratatui buffer (via a small cursor-style `Pen`), event panels, calendar grid, modals, setup wizard; palette follows the Omarchy theme's `colors.toml`
- **`cache.rs`** - `DisplayEvent` (unified event type), `SourceCache` (per-source), `EventCache` (disk persistence), `CacheWriter` (writes the cache on its own thread)
- **`term_bg.rs`** - The terminal's background colour (OSC 11), remembered per terminal across runs so terminals that never answer aren't asked again
- **`config.rs`** - Config loading from `~/.config/calendarchy/config.json` (incl. optional `display` prefs and `keys`), token storage
- **`auth.rs`** - `GoogleAuthState` / `ICloudAuthState` and `CalendarEntry` (a CalDAV calendar's URL and name)
- **`conversion.rs`** - Google events and iCal events → per-day `DisplayEvent`s
- **`error.rs`** - `CalendarchyError` and HTTP response checking (`check`, `check_google`)
- **`logging.rs`** - In-memory HTTP request log (last 100 lines; the `D` overlay) and panic logging
- **`setup.rs`** - The setup wizard's Hyprland keybinding step (adds a line to `~/.config/hypr/bindings.lua`, or `bindings.conf`)
- **`utils.rs`** - Month arithmetic, local-day bounds, meeting-link detection and Zoom deep links, attendee name/sort helpers, `open_url`
- **`google/`** - OAuth with a loopback redirect on `127.0.0.1` (`auth.rs`), Calendar API client (`calendar.rs`), types (`types.rs`)
- **`icloud/`** - CalDAV client (Basic auth with the app-specific password) with REPORT queries and its XML parsers (`calendar.rs`), iCal parser (`types.rs`)

### Key Types

- `DisplayEvent` - One occurrence of an event on one `date`, with a typed `When` (`AllDay` or start/end minutes). Multi-day events have one per day
- `GoogleAuthState` / `ICloudAuthState` - Auth state machines (`auth.rs`)
- `Msg` / `Effect` - Results from background tasks / I/O requested by the update logic (`update.rs`)

### Data Flow

```
Config → Auth → Fetch Events → Convert to DisplayEvent → Store in SourceCache → Save to disk
                                                                              ↓
UI ← EventCache.get(date) ←──────────────────────────────────────────────────┘
```

### Caching

- Events cached to `~/.cache/calendarchy/events.json`
- Auth tokens stored in `~/.config/calendarchy/tokens.json`
- Cache loads on startup for instant display; `fetched_months` not restored to force refresh
- Format is versioned (`CACHE_VERSION`); mismatches are discarded. Each event still carries `time_str`/`end_time_str` on disk because the TRMNL push (`~/Work/my/trmnl/calendar-push.mjs`) reads them

## Omarchy shell plugin

`shell/` holds two plugins for Omarchy 4's Quickshell `omarchy-shell`:

- `local.calendarchy` (`shell/*`, kind `service`, no UI): runs `--refresh` every 5 min and sends meeting alerts 5 min before via `omarchy-notification-send` (a click joins; Zoom via its `zoommtg://` deep link, the agenda's `joinUrl`; with no link it opens the TUI)
- `local.calendarchy.window` (`shell/window/`, kind `panel`): the TUI's layout as a window (month + week heatmap | Work, Personal, free gaps, Coming up | details), with the TUI's keys. Toggle with `omarchy-shell shell toggle local.calendarchy.window '{}'`; a Hyprland rule floats it (class `org.quickshell`, title `Calendarchy`)

- `shell/window/Layout.js` holds all the pure helpers, for both plugins (`install.sh` copies it into each). It ports the display rules from `ui.rs`/`app.rs` (gaps, heatmap, current/next, Coming up, navigation) and, from `keymap.rs`, key-spec parsing and matching, Bulgarian phonetic keys and help labels; keep them in step
- The window honours the config's `"keys"` overrides: it reads the bindings in effect from the agenda's `keys`. `DEFAULT_KEYS` in `Layout.js` is only the fallback for an agenda without them, and must match `Keymap::effective` with no overrides
- The window makes a single `--agenda` call (the visible six weeks) and takes the footer's "next" from its `next`; it runs `--refresh --month` for months nobody else keeps fresh, `--event` for actions, and opens the TUI with `omarchy-launch-or-focus-tui calendarchy --date <day>`
- `Service.qml` runs `calendarchy --agenda` each minute to find due alerts (sent ones are remembered in `$XDG_RUNTIME_DIR/calendarchy-alerted.json`)
- Install / update both with `shell/install.sh` (copies into `~/.config/omarchy/plugins/<id>/`, then enables them the first time; a symlink would break hot reload and `omarchy plugin validate`)
- Hot reload doesn't always pick up a changed service; `omarchy-restart-shell` does. Logs: `journalctl --user | grep calendarchy`
- The agenda JSON (shape documented at the top of `agenda.rs`) is the contract between the Rust side and the plugins: keep `agenda.rs`, the QML and `Layout.js` in step

## Phone page

`calendarchy serve` (`src/serve.rs`, page in `web/`) is the calendar on the iPhone: a
home-screen web app at https://omarchy.tail00a1a0.ts.net:8795 (tailnet only; `tailscale serve
--bg --https=8795 http://127.0.0.1:8795`). `systemd/calendarchy-web.service` runs it, installed
as a symlink in `~/.config/systemd/user/`; `systemctl --user restart calendarchy-web` after a
release build. Only the `--allow` Tailscale login (and this machine) gets an answer.

- `GET /api/agenda?from&days` is `agenda::build`, the same JSON as `--agenda`; `GET /layout.js`
  is `shell/window/Layout.js` with its `.pragma library` line dropped, so the page uses the
  window's display rules directly (keep Layout.js plain JS with no Qt in it)
- `POST /api/refresh?month=YYYY-MM` and `POST /api/event?verb=accept|decline&id=` run this
  binary's `--refresh --month` and `--event`, as the window does. The phone can't delete.
  A POST needs the page's Origin and `X-Calendarchy: 1`
- `web/` is read from this checkout on every request, so an edit to the page is live on the next
  load. `kit.css`, `theme.js`, `sw.js` and `kit.d.ts` are copies of the kit, put there by
  `~/Work/rb/ghostwheel/kit/sync.sh`: change them there, never here
- Type check: `~/Work/rb/ghostwheel/node_modules/.bin/tsc -p web/jsconfig.json`
- One view, the day (no month or Coming up on the phone, on purpose). A sticky bar holds the
  day, Today (always scrolls back to now), refetch and the week strip (‹ › or a swipe on it moves
  a week; a swipe on the timeline moves a day: the days are the browser's own sideways scroller
  (`.days`, three panes: the day before, the day, the day after, snapping one at a time), so the
  swipe runs at the screen's rate with native momentum, not as a JS transform; at rest on a
  neighbour it becomes the day (`settled`), drawn again in the middle, at the same time of day.
  Taps and keys draw the target in the side pane and smooth-scroll to it (`go`); a redraw waits
  while the days move; none with Reduce Motion). The page asks for the week before too, for the drag back from a Monday. Under it: today's Now/Next card, all-day chips,
  and the day as a timeline at 72 px an hour (a half hour is a tap target): blocks by their
  times, side by side where they overlap, free time left empty with a grey label, a line at now,
  a "next" tag. An empty day is one line, not a grid. An event in both calendars shows once
  (`merge`/`sameEvent` in `web/app.js`: same start and end, or both all day, and the same title
  ignoring case, punctuation and emoji, or one inside the other); the Work copy leads, so its
  Accept/Decline and link are the ones offered. Except an invitation from the Personal calendar
  (a Work event whose organizer is iCloud's relay `imip.me.com` or an iCloud address,
  `fromPersonal`): that's Personal, drawn and acted on as Personal only
- Colour roles: Work blue and Personal orange (`--work`/`--personal`, set in the page, a lighter
  pair under `data-scheme=dark`) and nothing else uses them; state is shape (the now line, the
  "next" tag, faded past, dashed not accepted). Events are soft islands: a light fill of their
  colour and the title in a deep shade of it, rounded, no bar (chosen from five in
  `~/html-plans/calendarchy-two/2026-11-22-event-styles.html`). Green is for now and Join; amber marks a heavy day
- Light or dark follows the phone (`followThePhone`): the Omarchy theme as it is when its mode
  matches, else its accents on a ground for the phone's mode
- The page asks for the week before, the selected week and two after it (`DAYS = 28`) every minute with the last
  ETag; `serve.rs` tags the agenda without its `now`, so an unchanged minute is a 304 with no body
- The page reloads itself when it changes (the kit's `Kit.autoUpdate()`): `serve.rs` serves it
  with `server::file_with`, which stamps it with a version of its files and `Layout.js`; a pull to
  refresh or ↻ asks at once (`Kit.updateNow()`) and reloads instead when there's a newer page
- Alerts on the phone aren't done yet (they'd be the kit's Web Push, as Lumon's)

## Committing

A local, personal project: commit without asking first, and push where there's a remote. The
history is the undo: if something isn't right, it's gone back to from there. Uncommitted work
found here is an earlier session's: look at it, then commit it as its own commit.
`~/.local/bin/calendarchy` links to `target/release`, so a release build goes live; copying
the plugins (`shell/install.sh`) is still its own step.

## Releases

Personal use, Linux only (Omarchy / Hyprland): build from source with `cargo build --release`.

### Website

- GitHub Pages from `docs/` folder on master
- Self-hosts Cascadia Code SemiBold font for block art logo
- `terminal.svg` (the terminal mockup) was generated by hand from an ANSI capture of the TUI
