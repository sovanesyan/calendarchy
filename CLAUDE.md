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

- **`main.rs`** - CLI dispatch (`--version`, `--refresh`, `--agenda`, `--event`; `--date YYYY-MM-DD` opens the TUI on that day, used by the shell window's "open") and the runtime: event-driven loop (input thread, results channel, timer, theme-switch check), executes `Effect`s
- **`app.rs`** - `App` state and navigation
- **`update.rs`** - Pure update logic: `handle_key` / `handle_msg` → `Effect`s, fetch scheduling (no I/O; unit-tested)
- **`keymap.rs`** - Bindings table: keys → `Action` per mode (Bulgarian phonetic keys normalised); also generates the help overlay and applies `"keys"` overrides from config
- **`sources.rs`** - Fetch and event-action layer shared by the TUI and the headless commands: shared HTTP client, `GoogleSession` token refresh, concurrent CalDAV, `event_action` + `check_event_action` (accept/decline/delete and their rules)
- **`headless.rs`** - `--refresh [--month YYYY-MM]`: fetch this month and the next (or one month) into the disk cache, no TUI (used by the shell plugins and the TRMNL job); `--event accept|decline|delete <id>`: one action by agenda id, then refetch its month
- **`agenda.rs`** - `--agenda [--from YYYY-MM-DD] [--days N]`: days as JSON from the disk cache only (no network): events with details, attendees and busy ranges, display prefs, the effective key bindings (after `"keys"` overrides), and the event that's on now or next (always from now, 8 days ahead; `find_next`, which the TUI's status bar shares); read by the shell plugins. The JSON shape is documented at the top of the file
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

## Releases

Personal use, Linux only (Omarchy / Hyprland): build from source with `cargo build --release`.

### Website

- GitHub Pages from `docs/` folder on master
- Self-hosts Cascadia Code SemiBold font for block art logo
- `terminal.svg` (the terminal mockup) was generated by hand from an ANSI capture of the TUI
