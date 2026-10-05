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

1. **Startup**: `main.rs` loads config, restores cached events from disk for instant display, then authenticates
2. **Auth**: Google uses OAuth device flow; iCloud uses app-specific password with CalDAV discovery
3. **Fetching**: Events are fetched per-month via async tasks, converted to `DisplayEvent`, cached to disk
4. **Rendering**: `ui.rs` renders a month calendar grid and two event panels into a ratatui buffer; ratatui diffs frames and writes only changed cells

### Module Structure

- **`main.rs`** - App state machine, async message handling, keyboard input loop
- **`ui.rs`** - Rendering into a ratatui buffer (via a small cursor-style `Pen`), event panels, calendar grid, modals
- **`cache.rs`** - `DisplayEvent` (unified event type), `SourceCache` (per-source), `EventCache` (disk persistence)
- **`config.rs`** - Config loading from `~/.config/calendarchy/config.json`, token storage
- **`google/`** - OAuth device flow (`auth.rs`), Calendar API client (`calendar.rs`), types (`types.rs`)
- **`icloud/`** - Basic auth (`auth.rs`), CalDAV client with REPORT queries (`calendar.rs`), iCal parser (`types.rs`)

### Key Types

- `DisplayEvent` - Normalized event with title, time_str, date, accepted, meeting_url
- `GoogleAuthState` / `ICloudAuthState` - Auth state machines (enums in main.rs)
- `AsyncMessage` - Channel messages from background tasks to main loop

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

## Release Process

To release a new version (e.g., 0.1.5 -> 0.1.6):

1. **Bump version** in `Cargo.toml`
2. **Commit and push** to master
3. **Tag and push**: `git tag v0.1.6 && git push origin v0.1.6`
4. **Done** — GitHub Actions handles everything:
   - Builds macOS ARM/Intel + Linux binaries
   - Creates the GitHub Release
   - Updates the Homebrew tap (`HOMEBREW_TAP_TOKEN` secret)
   - Updates both AUR packages (`AUR_SSH_KEY` secret)

### Package Locations

- **Homebrew**: `sovanesyan/homebrew-calendarchy` (auto-updated by workflow)
- **AUR binary**: `ssh://aur@aur.archlinux.org/calendarchy-bin.git` (auto-updated by workflow)
- **AUR source**: `ssh://aur@aur.archlinux.org/calendarchy.git` (auto-updated by workflow)
- **Local PKGBUILDs**: `pkg/arch/` and `pkg/arch-bin/` are reference copies (workflow generates its own)

### Website

- GitHub Pages from `docs/` folder on master
- Self-hosts Cascadia Code SemiBold font for block art logo
- Terminal mockup SVG generated from ANSI capture via `/tmp/ansi2svg.py`
