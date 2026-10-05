# Calendarchy

A terminal calendar app that displays Google Calendar and iCloud Calendar events side by side.

## Installation

Personal project; build from source. Requires [Rust](https://rustup.rs/).

```bash
git clone https://github.com/sovanesyan/calendarchy.git
cd calendarchy
cargo build --release
sudo cp target/release/calendarchy /usr/local/bin/
```

On macOS, also build the global-hotkey helper (needs Xcode command line tools) and put it next to the binary:

```bash
swiftc -O swift/main.swift -o calendarchy-hotkey
sudo cp calendarchy-hotkey /usr/local/bin/
```

On Linux, to add Calendarchy to your application launcher, copy the desktop entry:

```bash
sudo cp calendarchy.desktop /usr/share/applications/
```

## Configuration

Create `~/.config/calendarchy/config.json`:

```json
{
  "google": {
    "client_id": "YOUR_CLIENT_ID",
    "client_secret": "YOUR_CLIENT_SECRET"
  },
  "icloud": {
    "username": "YOUR_APPLE_ID",
    "app_password": "YOUR_APP_SPECIFIC_PASSWORD"
  }
}
```

Either source can be omitted if you only use one.

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

### Keyboard shortcut (macOS)

Set up a global Cmd+Shift+J shortcut to launch Calendarchy from anywhere:

```bash
calendarchy --setup
```

This installs a background helper that listens for the hotkey and opens Calendarchy in your terminal. macOS will prompt for Accessibility permission on first use — grant it in System Settings > Privacy & Security > Accessibility.

To remove the shortcut:

```bash
calendarchy --remove-setup
```
