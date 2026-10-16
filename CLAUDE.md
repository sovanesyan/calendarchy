# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (used by keyboard shortcut)
cargo test               # Run all tests
cargo test cache         # Run tests in cache module only
cargo test icloud        # Run tests in icloud module only
```

## Architecture

Calendarchy is a terminal calendar app that displays Google Calendar and iCloud Calendar events side by side.

### Core Flow

1. **Startup**: `main.rs` loads config, restores cached events from disk for instant display, starts fetches, draws the first frame
2. **Auth**: Google uses OAuth with a loopback redirect (tokens auto-refresh, incl. on 401); iCloud uses app-specific password with CalDAV discovery (or EventKit on macOS)
3. **Fetching**: `App::wanted_fetches` decides from state what to fetch — visible month first, then neighbours; visible month refreshes every 5 min. Results are split into per-day `DisplayEvent` occurrences and cached to disk
4. **Rendering**: `ui.rs` renders a month calendar grid and two event panels into a ratatui buffer; ratatui diffs frames and writes only changed cells

### Module Structure

- **`main.rs`** - Runtime: event-driven loop (input thread, results channel, timer), executes `Effect`s, cache writer thread
- **`app.rs`** - `App` state and navigation
- **`update.rs`** - Pure update logic: `handle_key` / `handle_msg` → `Effect`s, fetch scheduling (no I/O; unit-tested)
- **`keymap.rs`** - Bindings table: keys → `Action` per mode (Bulgarian phonetic keys normalised); also generates the help overlay and applies `"keys"` overrides from config
- **`sources.rs`** - Fetch layer shared by TUI and `--refresh`: shared HTTP client, `GoogleSession` token refresh, concurrent CalDAV
- **`headless.rs`** - `--refresh [--month YYYY-MM]`: fetch this month and the next (or one month) into the disk cache, no TUI (used by the shell plugins and the TRMNL job); `--event accept|decline|delete <id>`: one action by agenda id, then refetch its month
- **`agenda.rs`** - `--agenda [--from YYYY-MM-DD] [--days N]`: days as JSON from the disk cache only (no network): events with details and attendees, display prefs, and the busy event that's next; read by the shell plugins
- **`ui.rs`** - Rendering into a ratatui buffer (via a small cursor-style `Pen`), event panels, calendar grid, modals
- **`cache.rs`** - `DisplayEvent` (unified event type), `SourceCache` (per-source), `EventCache` (disk persistence)
- **`config.rs`** - Config loading from `~/.config/calendarchy/config.json` (incl. optional `display` prefs and `keys`), token storage
- **`google/`** - OAuth device flow (`auth.rs`), Calendar API client (`calendar.rs`), types (`types.rs`)
- **`icloud/`** - Basic auth (`auth.rs`), CalDAV client with REPORT queries (`calendar.rs`), iCal parser (`types.rs`)

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

- `local.calendarchy` (`shell/*`, kind `service`, no UI): runs `--refresh` every 5 min and sends meeting alerts 5 min before via `omarchy-notification-send` (a click joins; Zoom via its `zoommtg://` deep link, the agenda's `joinUrl`)
- `local.calendarchy.window` (`shell/window/`, kind `panel`): the TUI's layout as a window (month + week heatmap | Work, Personal, free gaps, Coming up | details), with the TUI's keys. `Layout.js` ports the display rules from `ui.rs`/`app.rs` (gaps, heatmap, current/next, Coming up, navigation); keep the two in step. Toggle with `omarchy-shell shell toggle local.calendarchy.window '{}'`; a Hyprland rule floats it (class `org.quickshell`, title `Calendarchy`)

- `Service.qml` runs `calendarchy --agenda` each minute to find due alerts (sent ones are remembered in `$XDG_RUNTIME_DIR/calendarchy-alerted.json`); `Model.js` holds the pure helpers
- Install / update both with `shell/install.sh` (copies into `~/.config/omarchy/plugins/<id>/`; a symlink would break hot reload and `omarchy plugin validate`)
- Hot reload doesn't always pick up a changed service; `omarchy-restart-shell` does. Logs: `journalctl --user | grep calendarchy`
- The agenda JSON is the contract between the two: keep `agenda.rs` and the QML in step

## Releases

Personal use only: no packaging or publishing (the Homebrew tap, AUR packages and release workflow were retired). Build with `cargo build --release`; on macOS the hotkey helper is `swiftc -O swift/main.swift -o calendarchy-hotkey`.

### Website

- GitHub Pages from `docs/` folder on master
- Self-hosts Cascadia Code SemiBold font for block art logo
- Terminal mockup SVG generated from ANSI capture via `/tmp/ansi2svg.py`
