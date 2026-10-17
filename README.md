# Calendarchy

A terminal calendar app that displays Google Calendar and iCloud Calendar events side by side. Linux only (built for Omarchy / Hyprland); iCloud is read over CalDAV.

## Installation

Personal project; build from source. Requires [Rust](https://rustup.rs/).

```bash
git clone https://github.com/sovanesyan/calendarchy.git
cd calendarchy
cargo build --release
sudo cp target/release/calendarchy /usr/local/bin/
```

To add Calendarchy to your application launcher, copy the desktop entry (it opens Calendarchy with Omarchy's `omarchy-launch-or-focus-tui`, so a running one is focused instead):

```bash
sudo cp calendarchy.desktop /usr/share/applications/
```

## Configuration

On first run with no config, a setup wizard connects Google (in the browser) and iCloud (Apple ID and an [app-specific password](https://appleid.apple.com/account/manage)) and writes `~/.config/calendarchy/config.json`; press `S` to run it again. Or write the file yourself:

```json
{
  "google": {
    "client_id": "YOUR_CLIENT_ID",
    "client_secret": "YOUR_CLIENT_SECRET"
  },
  "icloud": {
    "apple_id": "YOUR_APPLE_ID",
    "app_password": "YOUR_APP_SPECIFIC_PASSWORD"
  }
}
```

Either source can be omitted if you only use one. Google's `client_id` / `client_secret` are optional (a built-in OAuth client is used by default; `CALENDARCHY_GOOGLE_CLIENT_ID` / `CALENDARCHY_GOOGLE_CLIENT_SECRET` override it), as is `calendar_id` (default `primary`), so `"google": {}` is enough.

### Optional settings

Everything below is optional; leave it out for the defaults.

```json
{
  "display": {
    "second_timezone": "Australia/Sydney",
    "working_hours": "09:00-18:00",
    "google_label": "Work",
    "icloud_label": "Personal",
    "time_format": "12h",
    "week_start": "sunday",
    "week_numbers": true
  },
  "keys": {
    "join": "o",
    "search": ["/", "f"]
  }
}
```

- `second_timezone` adds a column with each event's start in another zone, plus that zone's current time in the header.
- `working_hours` limits the "free" rows to your working day and fades the hours outside it in the week grid.
- `keys` rebinds any action: `next_day`, `prev_day`, `next_week`, `prev_week`, `next_month`, `prev_month`, `enter_events`, `exit_events`, `next_event`, `prev_event`, `jump_events_forward`, `jump_events_back`, `switch_panel`, `join`, `accept`, `decline`, `delete`, `today`, `now`, `refresh`, `toggle_logs`, `search`, `help`, `open_google_web`, `open_icloud_web`, `connect_google`, `connect_icloud`, `setup`, `quit`. Keys are written as `J`, `ctrl+d`, `enter`, `esc`, `tab`, `shift+tab`, `left`, `space`, `f5`, …; the help overlay (`?`) shows whatever is bound.

On Omarchy, the panel accents follow the active theme's colours, and its background is used when the terminal doesn't report one.

## Usage

```bash
calendarchy
```

Navigate with arrow keys. Events from both calendars are displayed in side-by-side panels.

Pass `--date YYYY-MM-DD` to open on that day; `--version` prints the version.

### Keyboard shortcut (Hyprland)

On first run, the setup wizard offers to add a Super+Alt+J binding to `~/.config/hypr/bindings.lua` (or `bindings.conf`) that launches or focuses Calendarchy. It's skipped when that file already has a line launching `calendarchy` (the shell window's toggle doesn't count). To remove it, delete the `calendarchy` line from that file.

## Headless

These run without the TUI and exit:

- `calendarchy --refresh [--month YYYY-MM]`: fetch this month and the next (or just the given month) from both sources into the disk cache (`~/.cache/calendarchy/events.json`).
- `calendarchy --agenda [--from YYYY-MM-DD] [--days N]`: print days as JSON from the disk cache only, no network (`--from` defaults to today, `--days` to 2). Includes event details, display prefs, the key bindings in effect and the next meeting. The shape is documented at the top of `src/agenda.rs`.
- `calendarchy --event accept|decline|delete <id>`: act on one event by the id `--agenda` prints, then refetch its month. Accept and decline are Google only.

## Omarchy shell

`shell/` holds two plugins for the Omarchy 4 shell (`omarchy-shell`):

- `local.calendarchy`, a background service: runs `calendarchy --refresh` every 5 minutes and sends a notification 5 minutes before each meeting. Clicking it joins the meeting (Zoom opens in the app), or opens Calendarchy when there's no link.
- `local.calendarchy.window`, the TUI's layout as a window (month and week heatmap, Work and Personal panels, free gaps, coming up, details), with the same keys, including your `keys` overrides.

Install or update both (the `calendarchy` binary must be on your `PATH`):

```bash
shell/install.sh
```

Toggle the window, e.g. from a Hyprland binding:

```bash
omarchy-shell shell toggle local.calendarchy.window '{}'
```

To float it, add a window rule for class `org.quickshell`, title `Calendarchy`; in `~/.config/hypr/hyprland.lua`:

```lua
o.window({ class = "^org\\.quickshell$", title = "^Calendarchy$" }, { float = true, size = { 1500, 900 }, center = true })
```
