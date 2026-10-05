use crate::app::{EventSource, MatchType, NavigationMode, PendingAction, SearchState, SetupState, SetupStep};
use crate::auth::{AuthDisplay, GoogleAuthState, ICloudAuthState};
use crate::cache::{clock, AttendeeStatus, DisplayEvent, EventCache, EventId, When, DAY_MINUTES};
use crate::config::DisplayConfig;
use crate::logging::get_recent_logs;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use unicode_width::UnicodeWidthStr;

/// Cursor-style drawing onto a ratatui buffer: move, set style, print.
///
/// The layout code positions everything absolutely (it predates ratatui), so
/// rather than rewriting it into widgets this gives it the same move/print
/// vocabulary it was written against. ratatui then diffs the finished buffer
/// against the previous frame and writes only the changed cells, in one flush.
pub struct Pen<'a> {
    buf: &'a mut Buffer,
    x: u16,
    y: u16,
    style: Style,
}

impl<'a> Pen<'a> {
    pub fn new(buf: &'a mut Buffer) -> Self {
        Self { buf, x: 0, y: 0, style: Style::reset() }
    }

    fn move_to(&mut self, x: u16, y: u16) {
        self.x = x;
        self.y = y;
    }

    fn fg(&mut self, color: Color) {
        self.style.fg = Some(color);
    }

    fn bg(&mut self, color: Color) {
        self.style.bg = Some(color);
    }

    fn bold(&mut self) {
        self.style = self.style.add_modifier(Modifier::BOLD);
    }

    /// Back to the terminal defaults: colors and attributes (SGR 0)
    fn reset(&mut self) {
        self.style = Style::reset();
    }

    /// Print at the cursor and advance it; anything past the edge is clipped.
    ///
    /// Widths are measured per *character*, not per grapheme cluster, to match
    /// how foot (default `grapheme-width-method=wcswidth`), alacritty and xterm
    /// advance the cursor — e.g. 👶🏻 is 2+2 columns there. Measuring the cluster
    /// as 2 (ratatui's default) would shift the rest of the line. truncate_str
    /// uses the same per-char widths, so truncation and layout agree.
    fn print(&mut self, s: &str) {
        use unicode_width::UnicodeWidthChar;
        let area = *self.buf.area();
        if self.y >= area.bottom() {
            self.x = self.x.saturating_add(s.width() as u16);
            return;
        }
        let mut last: Option<u16> = None;
        let mut tmp = [0u8; 4];
        for c in s.chars() {
            if c.is_control() {
                continue;
            }
            let w = c.width().unwrap_or(0) as u16;
            if w == 0 {
                // Combining mark / ZWJ / variation selector: rides on the previous cell
                if let Some(px) = last {
                    let cell = &mut self.buf[(px, self.y)];
                    let sym = format!("{}{}", cell.symbol(), c);
                    cell.set_symbol(&sym);
                }
                continue;
            }
            if self.x + w > area.right() {
                self.x = self.x.saturating_add(w);
                last = None;
                continue;
            }
            self.buf[(self.x, self.y)].set_symbol(c.encode_utf8(&mut tmp)).set_style(self.style);
            // Cells covered by a wide char are blanked, as Buffer::set_stringn does
            for dx in 1..w {
                self.buf[(self.x + dx, self.y)].reset();
            }
            last = Some(self.x);
            self.x += w;
        }
    }
}

/// Terminal background color, queried once at startup via OSC 11.
/// Used to derive theme-adaptive shades (free slots, past fading).
/// Packed 0x00RRGGBB; u32::MAX = not known yet. Updatable, because the first
/// frame is drawn with last run's color before the terminal is asked again.
static TERM_BG: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn set_term_bg(r: u8, g: u8, b: u8) {
    TERM_BG.store(u32::from_be_bytes([0, r, g, b]), Ordering::Relaxed);
}

pub fn get_term_bg() -> Option<(u8, u8, u8)> {
    match TERM_BG.load(Ordering::Relaxed) {
        u32::MAX => None,
        packed => {
            let [_, r, g, b] = packed.to_be_bytes();
            Some((r, g, b))
        }
    }
}

/// The terminal's background; if it never answered the query, the desktop
/// theme's, and failing that a dark one
fn term_bg() -> (u8, u8, u8) {
    get_term_bg().or(palette().background).unwrap_or((30, 32, 38))
}

/// Blend a color toward the terminal background (0.0 = unchanged, 1.0 = background)
fn blend_toward_bg(color: (u8, u8, u8), amount: f32) -> Color {
    let bg = term_bg();
    let mix = |c: u8, b: u8| -> u8 { (c as f32 + (b as f32 - c as f32) * amount).round() as u8 };
    Color::Rgb(mix(color.0, bg.0), mix(color.1, bg.1), mix(color.2, bg.2))
}

/// The "free slot" shade: the real background nudged just enough to be visible,
/// darker on light themes and lighter on dark ones
fn free_block_color() -> Color {
    let (r, g, b) = term_bg();
    let luma = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    let shift: i16 = if luma > 128.0 { -20 } else { 24 };
    let adj = |c: u8| -> u8 { (c as i16 + shift).clamp(0, 255) as u8 };
    Color::Rgb(adj(r), adj(g), adj(b))
}

/// Free time outside working hours: halfway between the free shade and the
/// background, so it still reads as a grid cell but clearly not bookable
fn off_hours_color() -> Color {
    let (r, g, b) = term_bg();
    let luma = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    let shift: i16 = if luma > 128.0 { -9 } else { 10 };
    let adj = |c: u8| -> u8 { (c as i16 + shift).clamp(0, 255) as u8 };
    Color::Rgb(adj(r), adj(g), adj(b))
}

/// Source accents, and the background to assume when the terminal doesn't
/// say. Defaults are mid-tones that read on light and dark terminals; on
/// Omarchy they come from the active theme. (The week grid keeps its own
/// tuned shades: themes remap "red" freely, and vivid theme blues made busy
/// slots shout.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    google: (u8, u8, u8),
    icloud: (u8, u8, u8),
    /// The theme's background, used when the terminal doesn't answer OSC 11
    background: Option<(u8, u8, u8)>,
    /// Theme colors are vivid; blend them toward the background so panel
    /// labels stay calm. Defaults are already muted.
    mute: f32,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            google: (96, 125, 168),
            icloud: (152, 115, 168),
            background: None,
            mute: 0.0,
        }
    }
}

impl Palette {
    /// Read an Omarchy `colors.toml` (`key = "#rrggbb"` lines). Any color it
    /// doesn't define keeps its default.
    fn from_omarchy_colors(text: &str) -> Self {
        let hex = |key: &str| -> Option<(u8, u8, u8)> {
            let line = text.lines().find(|l| l.split('=').next().is_some_and(|k| k.trim() == key))?;
            let value = line.split_once('=')?.1.trim().trim_matches('"').strip_prefix('#')?;
            let n = u32::from_str_radix(value.get(..6)?, 16).ok()?;
            Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
        };
        let d = Self::default();
        Self {
            google: hex("blue").unwrap_or(d.google),
            icloud: hex("magenta").unwrap_or(d.icloud),
            background: hex("background"),
            mute: 0.3,
        }
    }

    /// The active Omarchy theme's palette, if there is one
    fn from_omarchy() -> Option<Self> {
        let home = dirs::home_dir()?;
        [".local/state/omarchy/current/theme/colors.toml", ".config/omarchy/current/theme/colors.toml"]
            .iter()
            .find_map(|rel| std::fs::read_to_string(home.join(rel)).ok())
            .map(|text| Self::from_omarchy_colors(&text))
    }

    fn color(&self, rgb: (u8, u8, u8)) -> Color {
        blend_toward_bg(rgb, self.mute)
    }
}

static PALETTE: std::sync::OnceLock<Palette> = std::sync::OnceLock::new();

/// Pick up the desktop theme's colors; call once at startup
pub fn load_palette() {
    let _ = PALETTE.set(Palette::from_omarchy().unwrap_or_default());
}

fn palette() -> Palette {
    PALETTE.get().copied().unwrap_or_default()
}

fn google_accent() -> Color {
    let p = palette();
    p.color(p.google)
}

fn icloud_accent() -> Color {
    let p = palette();
    p.color(p.icloud)
}

/// Heatmap busy / double-booked shades, before any fading for past slots
fn busy_rgb() -> (u8, u8, u8) {
    colors::BUSY_RGB
}

fn overlap_rgb() -> (u8, u8, u8) {
    colors::HEATMAP_OVERLAP_RGB
}

/// Formatting preferences for the frame being drawn. render() sets them from
/// the config at the start of each frame, so the many small formatting
/// helpers don't each need the config passed down.
#[derive(Debug, Clone, Copy, Default)]
struct Prefs {
    twelve_hour: bool,
    sunday_first: bool,
    week_numbers: bool,
}

thread_local! {
    static PREFS: std::cell::Cell<Prefs> = std::cell::Cell::new(Prefs::default());
}

fn prefs() -> Prefs {
    PREFS.with(|p| p.get())
}

fn set_prefs(display: &DisplayConfig) {
    let prefs = Prefs {
        twelve_hour: display.twelve_hour(),
        sunday_first: display.sunday_first(),
        week_numbers: display.week_numbers,
    };
    PREFS.with(|p| p.set(prefs));
}

/// A clock time from minutes after midnight: "14:30", or "2:30pm" in 12-hour
/// mode (midnight as the end of a day reads "24:00" / "12:00am")
fn time_text(minutes: u16) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    if prefs().twelve_hour {
        let (h12, half) = match h % 24 {
            0 => (12, "am"),
            h @ 1..=11 => (h, "am"),
            12 => (12, "pm"),
            h => (h - 12, "pm"),
        };
        format!("{}:{:02}{}", h12, m, half)
    } else {
        format!("{:02}:{:02}", h, m)
    }
}

/// The time column for an event: "All day" or its start
fn when_text(when: &When) -> String {
    match when.start() {
        None => "All day".to_string(),
        Some(start) => time_text(start),
    }
}

/// strftime pattern for a wall-clock time in another zone
fn zone_time_pattern(with_day: bool) -> &'static str {
    match (prefs().twelve_hour, with_day) {
        (false, false) => "%H:%M",
        (false, true) => "%a %H:%M",
        (true, false) => "%-I:%M%P",
        (true, true) => "%a %-I:%M%P",
    }
}

/// Day of the week as a column, 0-based from the configured week start
fn weekday_column(date: NaiveDate) -> u32 {
    if prefs().sunday_first {
        date.weekday().num_days_from_sunday()
    } else {
        date.weekday().num_days_from_monday()
    }
}

/// Width of the month grid column, including week numbers when shown
fn calendar_width() -> u16 {
    if prefs().week_numbers { CALENDAR_WIDTH + 3 } else { CALENDAR_WIDTH }
}

const CALENDAR_WIDTH: u16 = 23;
const MIN_PANEL_WIDTH: u16 = 25;


// Semantic color constants
mod colors {
    use ratatui::style::Color;

    // Event states
    pub const CURRENT_EVENT: Color = Color::LightGreen;
    pub const NEXT_EVENT: Color = Color::LightYellow;
    pub const PAST_EVENT: Color = Color::DarkGray;
    pub const FREE_EVENT: Color = Color::DarkGray;
    pub const SELECTED: Color = Color::LightCyan;

    // UI elements
    pub const HEADER: Color = Color::LightCyan;
    pub const SEPARATOR: Color = Color::DarkGray;

    // Details panel
    pub const TITLE: Color = Color::Reset;
    pub const TIME: Color = Color::Reset;
    pub const ACTION: Color = Color::LightGreen;

    // Overlap indicator
    pub const OVERLAP_EVENT: Color = Color::LightRed;

    // Week availability. Mid-tone marks that read on light and dark themes;
    // the free shade and past fading are derived from the real terminal
    // background at runtime (see free_block_color / blend_toward_bg).
    pub const BUSY_RGB: (u8, u8, u8) = (84, 113, 156);
    pub const HEATMAP_OVERLAP_RGB: (u8, u8, u8) = (156, 85, 85);

    // Status bar
    pub const LOG_TEXT: Color = Color::Cyan;
    pub const STATUS_MESSAGE: Color = Color::LightYellow;
}

// Terminal write helpers
fn draw_section_header(p: &mut Pen, x: u16, y: u16, label: &str, width: usize) {
    p.move_to(x, y);
    p.fg(Color::DarkGray);
    p.print(&format!("\u{2500} {} ", label));
    let remaining = width.saturating_sub(label.len() + 3);
    for _ in 0..remaining {
        p.print(&"\u{2500}".to_string());
    }
    p.reset();
}

pub struct RenderState<'a> {
    pub current_date: NaiveDate,
    pub selected_date: NaiveDate,
    pub show_logs: bool,
    pub events: &'a EventCache,
    pub google_auth: &'a GoogleAuthState,
    pub icloud_auth: &'a ICloudAuthState,
    pub status_message: Option<&'a str>,
    pub status_is_error: bool,
    pub google_loading: bool,
    pub icloud_loading: bool,
    // Two-level navigation state
    pub navigation_mode: NavigationMode,
    pub selected_source: EventSource,
    pub selected_event_index: usize,
    // Confirmation state
    pub pending_action: Option<&'a PendingAction>,
    // Search state
    pub search: Option<&'a SearchState>,
    // Help overlay
    pub show_help: bool,
    // Setup wizard
    pub setup: Option<&'a SetupState>,
    /// Optional display preferences (second time zone, working hours)
    pub display: &'a DisplayConfig,
    /// The clock for this frame (injected so rendering is deterministic in tests)
    pub now: DateTime<Local>,
}

/// Information about an upcoming event for the countdown display
pub struct NextEventInfo<'a> {
    pub event: &'a DisplayEvent,
    pub is_current: bool,      // Event is happening right now
    pub minutes_until: i64,    // Minutes until start (negative if already started)
}

/// Find the next upcoming event across all sources
fn find_next_event<'a>(events: &'a EventCache, today: NaiveDate, current_time: NaiveTime) -> Option<NextEventInfo<'a>> {
    // Check today's events first
    let all_today: Vec<&DisplayEvent> = events.google.get(today).iter()
        .chain(events.icloud.get(today).iter())
        .filter(|e| e.accepted) // Only show accepted events
        .collect();

    // Find current or next event today. Multi-day events (leave, a
    // conference) are banners like all-day ones: they'd otherwise hold "Now:"
    // all day and hide the countdown to actual meetings.
    for event in &all_today {
        if event.spans_days {
            continue;
        }
        let Some((start, end)) = event.when.range() else { continue };
        let start_time = clock(start).unwrap_or(NaiveTime::MIN);
        let not_ended = clock(end).is_none_or(|end_time| current_time < end_time);

        if not_ended {
            // Clock-time differences (with sub-second precision) truncate the
            // countdown exactly as before
            return Some(NextEventInfo {
                event,
                is_current: current_time >= start_time,
                minutes_until: (start_time - current_time).num_minutes(),
            });
        }
    }

    // Check future days (up to 7 days ahead)
    for days_ahead in 1..=7 {
        let check_date = today + Duration::days(days_ahead);
        let first_timed = events.google.get(check_date).iter()
            .chain(events.icloud.get(check_date).iter())
            .find(|e| e.accepted && !e.when.is_all_day() && !e.spans_days);

        if let Some(event) = first_timed
            && let Some(start) = event.when.start()
        {
            // Remaining today + full days + time into target day
            let remaining_today = (NaiveTime::from_hms_opt(23, 59, 59).unwrap() - current_time).num_minutes();
            let full_days_minutes = (days_ahead - 1) * 24 * 60;
            let minutes_until = remaining_today + full_days_minutes + start as i64 + 1;

            return Some(NextEventInfo {
                event,
                is_current: false,
                minutes_until,
            });
        }
    }

    None
}

/// Format a minutes-until value like "43m", "2h 15m", "3d 2h"
fn format_duration(minutes: i64) -> String {
    if minutes < 60 {
        format!("{}m", minutes)
    } else if minutes < 24 * 60 {
        let hours = minutes / 60;
        let mins = minutes % 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    } else {
        let days = minutes / (24 * 60);
        let hours = (minutes % (24 * 60)) / 60;
        if hours > 0 {
            format!("{}d {}h", days, hours)
        } else {
            format!("{}d", days)
        }
    }
}

pub fn render(frame: &mut Frame, state: &RenderState) {
    let area = frame.area();
    let (term_width, term_height) = (area.width, area.height);
    let p = &mut Pen::new(frame.buffer_mut());
    let today = state.now.date_naive();
    set_prefs(state.display);

    // Setup wizard takes over the whole screen
    if let Some(setup) = state.setup {
        render_setup_wizard(p, setup, term_width, term_height);
        return;
    }

    // Month view handles both normal and day timeline modes
    render_month_view(p, state, today, term_width, term_height);

    if let Some(search) = state.search {
        render_search_modal(p, search, state.now, term_width, term_height);
    } else {

        // Render HTTP logs if enabled
        let log_height = if state.show_logs { 8 } else { 0 };
        if state.show_logs {
            let logs = get_recent_logs(log_height as usize);
            let log_start_row = term_height.saturating_sub(2 + log_height);

            p.fg(colors::LOG_TEXT);
            for (i, log) in logs.iter().rev().enumerate() {
                let row = log_start_row + i as u16;
                if row < term_height.saturating_sub(2) {
                    p.move_to(0, row);
                    p.print(&format!(" {}", truncate_str(log, (term_width as usize).saturating_sub(2))));
                }
            }
            p.reset();
        }

        // Render confirmation modal if there's a pending action
        if let Some(action) = state.pending_action {
            render_confirmation_modal(p, action, term_width, term_height);
        }

        // Render help overlay on top of everything
        if state.show_help {
            render_help_modal(p, term_width, term_height);
        }
    }

    // Render status bar at bottom
    let status_row = term_height.saturating_sub(2);
    p.move_to(0, status_row);

    if let Some(msg) = state.status_message {
        let color = if state.status_is_error { Color::LightRed } else { colors::STATUS_MESSAGE };
        p.fg(color);
        p.print(&format!(" {}", truncate_str(msg, (term_width as usize).saturating_sub(2))));
        p.reset();
    } else {
        // Show countdown to next event when no status message
        let current_time = state.now.time();
        if let Some(next_info) = find_next_event(state.events, today, current_time) {
            let title = truncate_str(&next_info.event.title, 30);
            if next_info.is_current || next_info.minutes_until <= 0 {
                p.fg(colors::CURRENT_EVENT);
                p.print(&format!(" Now: {}", title));
            } else if next_info.minutes_until <= 15 {
                p.fg(colors::NEXT_EVENT);
                p.print(&format!(" Next: {} in {}", title, format_duration(next_info.minutes_until)));
            } else {
                // Calm default: only the event title at full brightness
                p.fg(Color::DarkGray);
                p.print(" Next: ");
                p.reset();
                p.print(&title.to_string());
                p.fg(Color::DarkGray);
                p.print(&format!(" in {}", format_duration(next_info.minutes_until)));
            }
            p.reset();
        }
    }

    // Render controls based on current mode
    p.move_to(0, term_height.saturating_sub(1));
    p.fg(Color::DarkGray);

    let controls = if state.show_help {
        // Help overlay controls
        " any key:close".to_string()
    } else if state.pending_action.is_some() {
        // Confirmation mode controls
        " y/Enter:confirm n/Esc:cancel".to_string()
    } else {
        // Calm footer: the keys that apply right now; the full keymap lives
        // in the ? overlay
        let mut c = String::from(" ? help \u{00B7} q quit");
        if state.navigation_mode == NavigationMode::Event {
            let selected = match state.selected_source {
                EventSource::Google => state.events.google.get(state.selected_date),
                EventSource::ICloud => state.events.icloud.get(state.selected_date),
            }
            .get(state.selected_event_index);
            if let Some(event) = selected {
                for action in event_actions(event) {
                    c.push_str(" \u{00B7} ");
                    c.push_str(action);
                }
            }
            c.push_str(" \u{00B7} Tab other panel \u{00B7} Esc back");
        } else if state.navigation_mode == NavigationMode::Day {
            c.push_str(" \u{00B7} Enter events");
            if !state.google_auth.is_authenticated() {
                c.push_str(&format!(" \u{00B7} g connect {}", state.display.google_label().to_lowercase()));
            }
            if !state.icloud_auth.is_authenticated() {
                c.push_str(&format!(" \u{00B7} i connect {}", state.display.icloud_label().to_lowercase()));
            }
        }
        c
    };
    p.print(&controls);
    p.reset();
}

fn render_month_view(p: &mut Pen, state: &RenderState, today: NaiveDate, term_width: u16, term_height: u16) {
    let current_time = state.now.time();
    let is_today = state.selected_date == today;
    let in_event_mode = state.navigation_mode == NavigationMode::Event;

    // Calculate column widths based on mode
    // Day mode: calendar | events (two stacked panels)
    // Event mode: calendar | events (two stacked panels) | details
    let events_panel_width: u16;
    let details_panel_width: u16;

    let cal_width = calendar_width();

    if in_event_mode {
        let available = term_width.saturating_sub(cal_width + 2);
        // Details panel: two fifths of the space, wider on wide terminals
        details_panel_width = (available * 2 / 5).clamp(MIN_PANEL_WIDTH, 60);
        events_panel_width = available.saturating_sub(details_panel_width + 1);
    } else {
        events_panel_width = term_width.saturating_sub(cal_width + 1);
        details_panel_width = 0;
    }

    // Reserve 2 rows for column headers
    let header_rows = 2u16;

    // Render calendar on left
    render_calendar(p, state.current_date, state.selected_date, state.now, state.events, state.google_loading || state.icloud_loading, state.display.working_minutes(), term_height);

    // Render event panels in the middle
    if events_panel_width >= MIN_PANEL_WIDTH {
        let events_x = cal_width + 1;

        // Events column header: selected date, and the time in the second zone
        p.move_to(events_x, 0);
        p.bold();
        p.print(&format!("{}", state.selected_date.format("%a %b %d")));
        p.reset();
        let second_tz = state.display.second_tz();
        if let Some((tz, label)) = second_tz {
            let there = state.now.with_timezone(&tz);
            let when = there.format(zone_time_pattern(there.date_naive() != today)).to_string();
            p.fg(Color::DarkGray);
            p.print(&format!("   {} {}", label, when));
            p.reset();
        }

        let google_events = state.events.google.get(state.selected_date);
        let icloud_events = state.events.icloud.get(state.selected_date);
        let is_past_day = state.selected_date < today;
        let (google_overlaps, icloud_overlaps) = compute_overlapping_events(google_events, icloud_events);

        // Free stretches between events; on today only what's still ahead
        let gaps = if is_past_day {
            Vec::new()
        } else {
            let from = if is_today { (current_time.hour() * 60 + current_time.minute()) as u16 } else { 0 };
            free_gaps(google_events, icloud_events, from, state.display.working_minutes())
        };
        let google_list = panel_rows(google_events, EventSource::Google, &gaps);
        let icloud_list = panel_rows(icloud_events, EventSource::ICloud, &gaps);

        // Selection info for highlighting
        let google_selected = if in_event_mode && state.selected_source == EventSource::Google {
            Some(state.selected_event_index)
        } else {
            None
        };
        let icloud_selected = if in_event_mode && state.selected_source == EventSource::ICloud {
            Some(state.selected_event_index)
        } else {
            None
        };

        // Budget vertical space between the two panels so a busy day can't push
        // the Personal panel (or the status bar) off screen
        let log_rows: u16 = if state.show_logs { 8 } else { 0 };
        let reserved_bottom = 2 + log_rows; // status + controls rows
        let available = term_height.saturating_sub(header_rows + reserved_bottom) as usize;
        // two panel headers + one blank row between panels
        let content_budget = available.saturating_sub(3).max(2);
        let google_needed = google_list.len().max(1);
        let icloud_needed = icloud_list.len().max(1);
        let (google_rows, icloud_rows) = if google_needed + icloud_needed <= content_budget {
            (google_needed, icloud_needed)
        } else {
            let half = content_budget / 2;
            if google_needed <= half {
                (google_needed, content_budget - google_needed)
            } else if icloud_needed <= content_budget - half {
                (content_budget - icloud_needed, icloud_needed)
            } else {
                (half.max(1), content_budget.saturating_sub(half).max(1))
            }
        };

        // Render Work (Google) panel
        render_event_panel(
            p,
            events_x,
            header_rows,
            events_panel_width,
            state.display.google_label(),
            google_events,
            &google_list,
            state.selected_date,
            state.google_loading,
            google_accent(),
            is_today,
            is_past_day,
            current_time,
            google_selected,
            &google_overlaps,
            second_tz,
            next_up(&state.events.google, state.selected_date, today).as_deref(),
            google_rows,
        );

        // Calculate Personal panel position: after Work header (1) + rendered rows + spacing (1)
        let work_panel_rows = 1 + google_needed.min(google_rows) as u16;
        let personal_y = header_rows + work_panel_rows + 1;

        // Render Personal (iCloud) panel below
        render_event_panel(
            p,
            events_x,
            personal_y,
            events_panel_width,
            state.display.icloud_label(),
            icloud_events,
            &icloud_list,
            state.selected_date,
            state.icloud_loading,
            icloud_accent(),
            is_today,
            is_past_day,
            current_time,
            icloud_selected,
            &icloud_overlaps,
            second_tz,
            next_up(&state.events.icloud, state.selected_date, today).as_deref(),
            icloud_rows,
        );

        // Whatever height is left goes to a glance at the next few days
        let upcoming_y = personal_y + 1 + icloud_needed.min(icloud_rows) as u16 + 1;
        let upcoming_end = term_height.saturating_sub(reserved_bottom);
        if upcoming_end >= upcoming_y + 3 {
            let days = (upcoming_end - upcoming_y - 1).min(5) as i64;
            let width = events_panel_width.min(MAX_ROW_WIDTH + tz_width_for(second_tz));
            render_upcoming(p, events_x, upcoming_y, width, state.events, state.selected_date, days);
        }
    } else if events_panel_width >= 4 {
        // Terminal too narrow for the event panels — say so instead of showing nothing
        p.move_to(cal_width + 1, 0);
        p.fg(Color::DarkGray);
        p.print(&truncate_str("Too narrow for events", events_panel_width as usize).to_string());
        p.reset();
    }

    // Render details panel on the right when in Event mode
    if in_event_mode && details_panel_width >= MIN_PANEL_WIDTH {
        let details_x = cal_width + events_panel_width + 2;
        let details_height = term_height.saturating_sub(3);


        // Get the selected event
        let selected_event = match state.selected_source {
            EventSource::Google => state.events.google.get(state.selected_date).get(state.selected_event_index),
            EventSource::ICloud => state.events.icloud.get(state.selected_date).get(state.selected_event_index),
        };

        render_event_details_column(p, details_x, 0, details_panel_width, details_height, selected_event);
    }

}

/// For an empty panel: the next thing in that calendar within two weeks,
/// e.g. "Wed 14:00 Interview" (all-day entries like working location don't count)
fn next_up(source: &crate::cache::SourceCache, after: NaiveDate, today: NaiveDate) -> Option<String> {
    (1..=14).find_map(|offset| {
        let date = after + Duration::days(offset);
        source.get(date).iter()
            .find(|e| e.accepted && !e.when.is_all_day())
            .map(|e| format!("{} {}", format_smart_when(date, &e.when, today), e.title))
    })
}

/// Compact agenda for the days after the selected one, one line per day:
/// "Tue 06  09:00 Standup · 11:00 1:1 Ana · +2"
fn render_upcoming(p: &mut Pen, x: u16, y: u16, width: u16, events: &EventCache, after: NaiveDate, days: i64) {
    p.move_to(x, y);
    p.fg(Color::DarkGray);
    p.print("Coming up");
    p.reset();

    let width = width as usize;
    for offset in 1..=days {
        let date = after + Duration::days(offset);
        let row = y + offset as u16;
        let weekend = date.weekday().num_days_from_monday() >= 5;

        p.move_to(x, row);
        if weekend {
            p.fg(Color::DarkGray);
        } else {
            p.bold();
        }
        p.print(&date.format("%a %d").to_string());
        p.reset();

        // Timed events you're going to, both calendars, in order. All-day
        // entries (working location, holidays) would crowd out the meetings.
        let mut day: Vec<&DisplayEvent> = events.google.get(date).iter()
            .chain(events.icloud.get(date))
            .filter(|e| e.accepted && !e.when.is_all_day() && !e.spans_days)
            .collect();
        day.sort_by_key(|e| e.when.sort_key());

        p.move_to(x + 8, row);
        if day.is_empty() {
            p.fg(Color::DarkGray);
            p.print("free");
            p.reset();
            continue;
        }

        // Fit as many as the line allows, leaving room for "+N"
        let budget = width.saturating_sub(8);
        let mut used = 0usize;
        for (n, event) in day.iter().enumerate() {
            let time = when_text(&event.when);
            let item = format!("{} {}", time, event.title);
            let sep = if n == 0 { 0 } else { 3 };
            let left = day.len() - n - 1;
            let reserve = if left > 0 { 5 } else { 0 };
            let room = budget.saturating_sub(used + sep + reserve);
            // Truncate the last item that fits rather than dropping it, but
            // don't bother with a stub shorter than the time itself
            if room < 12 {
                p.fg(Color::DarkGray);
                p.print(&format!(" +{}", day.len() - n));
                p.reset();
                break;
            }
            if sep > 0 {
                p.fg(Color::DarkGray);
                p.print(" \u{00B7} ");
                p.reset();
            }
            let item = truncate_str(&item, room);
            p.fg(Color::DarkGray);
            p.print(&time);
            p.reset();
            p.print(&item[time.len()..]);
            used += sep + item.width();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_calendar(
    p: &mut Pen,
    current_date: NaiveDate,
    selected_date: NaiveDate,
    now: DateTime<Local>,
    events: &EventCache,
    is_loading: bool,
    hours: Option<(u16, u16)>,
    term_height: u16,
) {
    let today = now.date_naive();
    p.move_to(0, 0);

    // Month header
    p.bold();

    let cal_width = calendar_width();
    let grid_x = cal_width - CALENDAR_WIDTH; // room for week numbers
    let loading_indicator = if is_loading { " *" } else { "" };
    let header = format!(
        "{} {}{}",
        current_date.format("%B"),
        current_date.year(),
        loading_indicator
    );
    p.print(&truncate_str(&header, cal_width as usize).to_string());
    p.reset();

    // Weekday header
    p.move_to(grid_x, 2);
    p.fg(Color::DarkGray);
    p.print(if prefs().sunday_first { "Su Mo Tu We Th Fr Sa" } else { "Mo Tu We Th Fr Sa Su" });
    p.reset();

    // Calendar grid
    let first_day = current_date.with_day(1).unwrap();
    let start_weekday = weekday_column(first_day);
    let days_in_month = days_in_month(current_date);
    let cols = 7;

    for row in 0..6 {
        // ISO week number of the row (its Monday), dim, when enabled
        let row_start = first_day - Duration::days(start_weekday as i64) + Duration::days(row as i64 * 7);
        let in_month = row * 7 < start_weekday + days_in_month;
        p.move_to(0, 3 + row as u16);
        if prefs().week_numbers && in_month {
            let monday = row_start + Duration::days(if prefs().sunday_first { 1 } else { 0 });
            p.fg(Color::DarkGray);
            p.print(&format!("{:2} ", monday.iso_week().week()));
            p.reset();
        }
        p.move_to(grid_x, 3 + row as u16);

        for col in 0..cols {
            let cell = row * 7 + col; // Always use 7-day weeks for calculation
            if cell < start_weekday || cell >= start_weekday + days_in_month {
                p.print("   ");
            } else {
                let day = cell - start_weekday + 1;
                let date = first_day.with_day(day).unwrap();
                let is_today = date == today;
                let is_selected = date == selected_date;
                // How booked the day is sets the number's weight: nothing
                // booked recedes, a heavy day stands out
                let booked = busy_minutes(events.google.get(date), events.icloud.get(date));

                if is_selected {
                    // Explicit colors: Reverse over a dark theme made the cursor nearly invisible
                    p.bg(Color::LightCyan); p.fg(Color::Black);
                } else if is_today {
                    p.fg(Color::LightGreen); p.bold();
                } else if booked == 0 {
                    p.fg(Color::DarkGray);
                } else if booked >= HEAVY_DAY_MINUTES {
                    p.fg(Color::Reset); p.bold();
                }

                p.print(&format!("{:2} ", day));

                p.reset();
            }
        }
    }

    // Render week availability below the calendar grid
    render_week_availability(p, events, selected_date, now, hours, term_height);
}

/// An event's busy range in minutes from midnight; None for all-day, free
/// or unaccepted events (not time-blocking)
fn parse_event_range(event: &DisplayEvent) -> Option<(u32, u32)> {
    event.busy_range().map(|(start, end)| (start as u32, end as u32))
}

/// Detect overlapping events across two source panels.
/// Returns sets of indices into google_events and icloud_events that overlap with any other event.
fn compute_overlapping_events(
    google_events: &[DisplayEvent],
    icloud_events: &[DisplayEvent],
) -> (HashSet<usize>, HashSet<usize>) {
    let mut google_overlaps = HashSet::new();
    let mut icloud_overlaps = HashSet::new();

    // Parse ranges once
    let google_ranges: Vec<Option<(u32, u32)>> = google_events.iter().map(parse_event_range).collect();
    let icloud_ranges: Vec<Option<(u32, u32)>> = icloud_events.iter().map(parse_event_range).collect();

    // Check within Google events
    for i in 0..google_ranges.len() {
        for j in (i + 1)..google_ranges.len() {
            if let (Some((s_a, e_a)), Some((s_b, e_b))) = (google_ranges[i], google_ranges[j]) {
                if s_a < e_b && s_b < e_a {
                    google_overlaps.insert(i);
                    google_overlaps.insert(j);
                }
            }
        }
    }

    // Check within iCloud events
    for i in 0..icloud_ranges.len() {
        for j in (i + 1)..icloud_ranges.len() {
            if let (Some((s_a, e_a)), Some((s_b, e_b))) = (icloud_ranges[i], icloud_ranges[j]) {
                if s_a < e_b && s_b < e_a {
                    icloud_overlaps.insert(i);
                    icloud_overlaps.insert(j);
                }
            }
        }
    }

    // Check cross-source overlaps
    for (gi, g_range) in google_ranges.iter().enumerate() {
        for (ii, i_range) in icloud_ranges.iter().enumerate() {
            if let (Some((s_a, e_a)), Some((s_b, e_b))) = (g_range, i_range) {
                if s_a < e_b && s_b < e_a {
                    google_overlaps.insert(gi);
                    icloud_overlaps.insert(ii);
                }
            }
        }
    }

    (google_overlaps, icloud_overlaps)
}

/// Booked time from which a day reads as heavy on the month grid
const HEAVY_DAY_MINUTES: u16 = 5 * 60;

/// Shortest stretch of free time worth calling out between events
const MIN_GAP_MINUTES: u16 = 30;
/// ...and for free time that's already under way, which marks "now"
const MIN_NOW_GAP_MINUTES: u16 = 5;

/// One line of an event panel: an event (index into the day's list), or the
/// free time that follows one
#[derive(Debug, Clone, Copy, PartialEq)]
enum PanelRow {
    Event(usize),
    /// Free minutes; `now` when the free time has already begun (today)
    Gap { minutes: u16, now: bool },
}

/// A stretch of free time: the panel and event it follows, its length, and
/// whether it's under way
#[derive(Debug, Clone, Copy, PartialEq)]
struct FreeGap {
    source: EventSource,
    after: usize,
    minutes: u16,
    now: bool,
}

/// Free time between busy blocks, across both calendars.
///
/// Each gap is attributed to the event that ends right before it, so it's
/// listed once, under that event's panel. Only
/// free time at or after `from` counts (pass the current time for today), and
/// only within `hours` when working hours are set.
fn free_gaps(
    google: &[DisplayEvent],
    icloud: &[DisplayEvent],
    from: u16,
    hours: Option<(u16, u16)>,
) -> Vec<FreeGap> {
    let mut busy: Vec<(u16, u16, EventSource, usize)> = google.iter().enumerate()
        .filter_map(|(i, e)| e.busy_range().map(|(s, end)| (s, end, EventSource::Google, i)))
        .chain(icloud.iter().enumerate()
            .filter_map(|(i, e)| e.busy_range().map(|(s, end)| (s, end, EventSource::ICloud, i))))
        .collect();
    busy.sort_by_key(|b| (b.0, b.1));

    let (lo, hi) = hours.unwrap_or((0, DAY_MINUTES));
    let mut gaps = Vec::new();
    let mut iter = busy.into_iter();
    let Some((_, mut block_end, mut src, mut idx)) = iter.next() else { return gaps };
    for (start, end, s, i) in iter {
        if start > block_end {
            let free_from = block_end.max(from).max(lo);
            let free_to = start.min(hi);
            // Free time already under way marks "now", so even a short one shows
            let now = from > block_end && from > lo;
            let min = if now { MIN_NOW_GAP_MINUTES } else { MIN_GAP_MINUTES };
            if free_to >= free_from + min {
                gaps.push(FreeGap { source: src, after: idx, minutes: free_to - free_from, now });
            }
        }
        if end > block_end {
            (block_end, src, idx) = (end, s, i);
        }
    }
    gaps
}

/// Interleave a panel's events with the gaps anchored in it. A gap goes after
/// its anchor event and after anything else that starts before the free time
/// does (a declined or "free" event inside the busy block).
fn panel_rows(events: &[DisplayEvent], source: EventSource, gaps: &[FreeGap]) -> Vec<PanelRow> {
    let mut rows: Vec<PanelRow> = (0..events.len()).map(PanelRow::Event).collect();
    let mut anchored: Vec<(usize, PanelRow)> = gaps.iter()
        .filter(|g| g.source == source && g.after < events.len())
        .map(|g| {
            let gap_start = events[g.after].busy_range().map_or(0, |(_, end)| end);
            let mut pos = g.after + 1;
            while pos < events.len() && events[pos].when.start().is_some_and(|s| s < gap_start) {
                pos += 1;
            }
            (pos, PanelRow::Gap { minutes: g.minutes, now: g.now })
        })
        .collect();
    // Insert from the back so earlier positions stay valid
    anchored.sort_by_key(|&(pos, _)| std::cmp::Reverse(pos));
    for (pos, row) in anchored {
        rows.insert(pos, row);
    }
    rows
}

/// Widest an event row gets (without the second-zone column)
const MAX_ROW_WIDTH: u16 = 64;

/// Columns the second-zone time takes in an event row
fn tz_width_for(second_tz: Option<(chrono_tz::Tz, &str)>) -> u16 {
    match (second_tz.is_some(), prefs().twelve_hour) {
        (false, _) => 0,
        (true, false) => 10, // "Tue 01:00 "
        (true, true) => 12,  // "Tue 12:30pm "
    }
}

/// Compact length for the duration column: "30m", "1h", "1h30"
fn format_length(minutes: u16) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{}m", m),
        (h, 0) => format!("{}h", h),
        (h, m) => format!("{}h{:02}", h, m),
    }
}

/// Short name of the meeting service behind a link
fn meeting_kind(url: &str) -> &'static str {
    if url.contains("zoom.us") || url.starts_with("zoommtg:") {
        "zoom"
    } else if url.contains("meet.google.com") {
        "meet"
    } else if url.contains("teams.microsoft.com") || url.contains("teams.live.com") {
        "teams"
    } else {
        "link"
    }
}

/// A location worth showing in a list row: not empty and not just a URL
/// (meeting links already show as the meeting kind)
fn display_location(event: &DisplayEvent) -> Option<&str> {
    let loc = event.location.as_deref()?.trim();
    (!loc.is_empty() && !loc.contains("://")).then_some(loc)
}

/// Total minutes the day is booked, across both calendars (overlaps counted once)
fn busy_minutes(google: &[DisplayEvent], icloud: &[DisplayEvent]) -> u16 {
    let mut ranges: Vec<(u16, u16)> = google.iter().chain(icloud).filter_map(|e| e.busy_range()).collect();
    ranges.sort();
    let mut total = 0;
    let mut covered_to = 0;
    for (start, end) in ranges {
        let start = start.max(covered_to);
        if end > start {
            total += end - start;
            covered_to = end;
        }
    }
    total
}

/// Event descriptions as plain text: Google sends HTML, iCloud plain text.
/// Line-breaking tags become newlines, other tags are dropped, common
/// entities decoded, and runs of blank lines collapsed.
fn description_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            out.push_str(&rest[lt..]);
            rest = "";
            break;
        };
        let tag = rest[lt + 1..lt + gt].trim_start_matches('/').to_ascii_lowercase();
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        match name {
            "br" | "p" | "div" | "tr" | "h1" | "h2" | "h3" => out.push('\n'),
            "li" if !rest[lt + 1..].starts_with('/') => out.push_str("\n\u{2022} "),
            _ => {}
        }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(rest);

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");

    let mut lines: Vec<&str> = Vec::new();
    for line in decoded.lines().map(str::trim_end) {
        if line.trim().is_empty() && lines.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Word-wrap text to a display width; words longer than a line are split
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0usize;
        for word in paragraph.split_whitespace() {
            let w = word.width();
            if used > 0 && used + 1 + w > width {
                out.push(std::mem::take(&mut line));
                used = 0;
            }
            if used > 0 {
                line.push(' ');
                used += 1;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if used + cw > width {
                    out.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(c);
                used += cw;
            }
        }
        out.push(line);
    }
    out
}

/// "6 going · 1 declined · 2 no reply"
fn attendee_summary(attendees: &[crate::cache::DisplayAttendee]) -> String {
    let count = |f: fn(&AttendeeStatus) -> bool| attendees.iter().filter(|a| f(&a.status)).count();
    let parts = [
        (count(|s| matches!(s, AttendeeStatus::Accepted | AttendeeStatus::Organizer)), "going"),
        (count(|s| matches!(s, AttendeeStatus::Tentative)), "maybe"),
        (count(|s| matches!(s, AttendeeStatus::Declined)), "declined"),
        (count(|s| matches!(s, AttendeeStatus::NeedsAction)), "no reply"),
    ];
    parts.iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{} {}", n, label))
        .collect::<Vec<_>>()
        .join(" \u{00B7} ")
}

/// A local wall-clock time on `date` shown in another zone: "22:00", or
/// "Tue 01:00" when it falls on a different day there
fn time_in_zone(date: NaiveDate, minutes: u16, tz: chrono_tz::Tz) -> Option<String> {
    use chrono::TimeZone;
    let local = Local.from_local_datetime(&date.and_time(clock(minutes)?)).earliest()?;
    let there = local.with_timezone(&tz);
    Some(there.format(zone_time_pattern(there.date_naive() != date)).to_string())
}

/// Count how many time-blocking events cover a given slot (across both sources).
fn count_slot_events(google_events: &[DisplayEvent], icloud_events: &[DisplayEvent], slot_start: u32, slot_end: u32) -> usize {
    google_events.iter().chain(icloud_events.iter())
        .filter_map(parse_event_range)
        .filter(|(es, ee)| slot_start < *ee && slot_end > *es)
        .count()
}

/// Get the Monday of the week containing the given date
fn get_week_start(date: NaiveDate) -> NaiveDate {
    date - Duration::days(weekday_column(date) as i64)
}

/// Render week availability grid below the calendar
fn render_week_availability(
    p: &mut Pen,
    events: &EventCache,
    selected_date: NaiveDate,
    now: DateTime<Local>,
    hours: Option<(u16, u16)>,
    term_height: u16,
) {
    let start_row = 10u16; // Below the calendar grid
    let monday = get_week_start(selected_date);
    let today = now.date_naive();
    let current_minutes = now.hour() * 60 + now.minute();
    let num_days = 7;
    let max_row = term_height.saturating_sub(2); // don't collide with the status bar

    // Header row: highlight the selected day's column (and today's)
    p.move_to(0, start_row);
    p.print("   ");
    for day_offset in 0..7i64 {
        let date = monday + Duration::days(day_offset);
        let letter = date.format("%a").to_string().chars().next().unwrap_or(' ');
        if date == selected_date {
            p.fg(colors::SELECTED); p.bold();
        } else if date == today {
            p.fg(Color::LightGreen);
        } else {
            p.fg(Color::DarkGray);
        }
        p.print(&format!(" {} ", letter));
        p.reset();
    }

    // Render each hour row (8am - 7pm = 12 rows)
    // Each cell shows 30-min resolution using half-blocks
    for hour_offset in 0..12u32 {
        let hour = 8 + hour_offset;
        let row = start_row + 1 + hour_offset as u16;
        if row >= max_row {
            break;
        }

        p.move_to(0, row);

        // Hour label
        p.fg(Color::DarkGray);
        p.print(&format!("{:2} ", hour));
        p.reset();

        // Check each weekday
        for day_offset in 0..num_days as i64 {
            let date = monday + Duration::days(day_offset);

            // Get events for this date from both sources
            let google_events = events.google.get(date);
            let icloud_events = events.icloud.get(date);

            // Check 30-minute slots
            let slot1_start = hour * 60;       // :00
            let slot1_end = hour * 60 + 30;    // :30
            let slot2_start = hour * 60 + 30;  // :30
            let slot2_end = (hour + 1) * 60;   // :00 next hour

            let first_half_count = count_slot_events(google_events, icloud_events, slot1_start, slot1_end);
            let second_half_count = count_slot_events(google_events, icloud_events, slot2_start, slot2_end);

            let first_half_busy = first_half_count > 0;
            let second_half_busy = second_half_count > 0;

            let is_past_day = date < today;
            let first_half_past = is_past_day || (date == today && current_minutes >= slot1_end);
            let second_half_past = is_past_day || (date == today && current_minutes >= slot2_end);

            // Past slots fade toward the real terminal background
            let color_for = |count: usize, past: bool| -> Color {
                let rgb = if count >= 2 { overlap_rgb() } else { busy_rgb() };
                if past {
                    blend_toward_bg(rgb, 0.55)
                } else {
                    Color::Rgb(rgb.0, rgb.1, rgb.2)
                }
            };
            // Free time outside working hours fades further, so the hours
            // you'd actually book stand out
            let free_for = |slot_start: u32, slot_end: u32| -> Color {
                match hours {
                    Some((lo, hi)) if slot_start < lo as u32 || slot_end > hi as u32 => off_hours_color(),
                    _ => free_block_color(),
                }
            };
            let top = if first_half_busy { color_for(first_half_count, first_half_past) } else { free_for(slot1_start, slot1_end) };
            let bot = if second_half_busy { color_for(second_half_count, second_half_past) } else { free_for(slot2_start, slot2_end) };

            // Vertical half-blocks: ▀ in the first half-hour's color over the
            // second's as background; a solid block when both match
            if top == bot {
                p.fg(top);
                p.print("██");
            } else {
                p.fg(top); p.bg(bot);
                p.print("▀▀");
            }
            p.reset();
            p.print(" ");
        }
        p.reset();
    }

    // Events outside the 08:00–20:00 window would otherwise be invisible here —
    // mark the affected days with ▴ (earlier) / ▾ (later)
    let marker_row = start_row + 13;
    if marker_row < max_row {
        let mut markers = [" "; 7];
        let mut any = false;
        for day_offset in 0..7i64 {
            let date = monday + Duration::days(day_offset);
            let mut early = false;
            let mut late = false;
            for (start, end) in events.google.get(date).iter()
                .chain(events.icloud.get(date).iter())
                .filter_map(parse_event_range)
            {
                early |= start < 8 * 60;
                late |= end > 20 * 60;
            }
            markers[day_offset as usize] = match (early, late) {
                (true, true) => "\u{2195}",   // ↕
                (true, false) => "\u{25B4}",  // ▴
                (false, true) => "\u{25BE}",  // ▾
                (false, false) => " ",
            };
            any |= early || late;
        }
        if any {
            p.move_to(0, marker_row);
            p.fg(Color::Yellow);
            p.print("   ");
            for marker in markers {
                p.print(&format!(" {} ", marker));
            }
            p.reset();
        }
    }
}

/// Render event panel with title and events
#[allow(clippy::too_many_arguments)]
fn render_event_panel(
    p: &mut Pen,
    x: u16,
    y: u16,
    width: u16,
    title: &str,
    events: &[DisplayEvent],
    rows: &[PanelRow],
    date: NaiveDate,
    is_loading: bool,
    accent_color: Color,
    is_today: bool,
    is_past_day: bool,
    current_time: NaiveTime,
    selected_index: Option<usize>,
    overlapping_indices: &HashSet<usize>,
    second_tz: Option<(chrono_tz::Tz, &str)>,
    next_up: Option<&str>,
    max_rows: usize,
) {
    // Panel header: just the label in a muted accent — no rules
    p.move_to(x, y);
    p.fg(accent_color);
    let loading_str = if is_loading { "*" } else { "" };
    p.print(&format!("{}{}", title, loading_str));
    p.reset();

    let content_start = y + 1;
    // Rows stop at a readable length on wide terminals, keeping the duration
    // column within eye reach of the titles
    let width = width.min(MAX_ROW_WIDTH + tz_width_for(second_tz));

    if events.is_empty() {
        p.move_to(x, content_start);
        p.fg(Color::DarkGray);
        if is_loading {
            p.print("Loading...");
        } else {
            let text = match next_up {
                Some(next) => format!("Nothing scheduled \u{00B7} next {}", next),
                None => "Nothing scheduled".to_string(),
            };
            p.print(&truncate_str(&text, width as usize));
        }
        p.reset();
        return;
    }

    // Find current and next event indices
    let (current_event_idx, next_event_idx) = if is_today {
        find_current_and_next_events(events, current_time)
    } else {
        (None, None)
    };

    // Columns: marker + time + gutter, an optional second-zone time, the title,
    // then (when there's room) a duration and the meeting service on the right
    let tz_width = tz_width_for(second_tz);
    let title_x = x + 10 + tz_width;
    let show_length = width >= 40 + tz_width;
    // Reserved even when this panel has no links, so both panels' columns line up
    let show_kind = width >= 52 + tz_width;
    let trail_width: u16 = if show_length { 6 } else { 0 } + if show_kind { 6 } else { 0 };
    let title_width = width.saturating_sub(10 + tz_width + trail_width + 1) as usize;

    // Scroll window: keep the selected event visible, reserve the last row
    // for a "+N more" indicator when the panel can't fit everything
    let total = rows.len();
    let (start, visible) = if total <= max_rows {
        (0usize, total)
    } else {
        let visible = max_rows.saturating_sub(1).max(1);
        let sel = selected_index
            .and_then(|s| rows.iter().position(|r| *r == PanelRow::Event(s)))
            .unwrap_or(0);
        let mut start = if sel >= visible { sel + 1 - visible } else { 0 };
        if start + visible > total {
            start = total - visible;
        }
        (start, visible)
    };

    for (row, panel_row) in rows[start..start + visible].iter().enumerate() {
        let row_y = content_start + row as u16;
        let i = match *panel_row {
            PanelRow::Gap { minutes, now } => {
                p.move_to(title_x, row_y);
                // Free time under way doubles as the "now" marker in today's list
                let text = if now {
                    p.fg(colors::CURRENT_EVENT);
                    format!("\u{2500}\u{2500} free now \u{00B7} {} \u{2500}\u{2500}", format_duration(minutes as i64))
                } else {
                    p.fg(Color::DarkGray);
                    format!("\u{2500}\u{2500} {} free \u{2500}\u{2500}", format_duration(minutes as i64))
                };
                p.print(&truncate_str(&text, title_width + trail_width as usize));
                p.reset();
                continue;
            }
            PanelRow::Event(i) => i,
        };
        let event = &events[i];
        p.move_to(x, row_y);

        let is_selected = selected_index == Some(i);
        let is_current = current_event_idx == Some(i);
        let is_next = next_event_idx == Some(i);
        let is_past_event = is_today && is_event_past(event, current_time) && !is_current;
        let is_unaccepted = !event.accepted;
        let is_free_event = event.is_free;
        let is_overlapping = overlapping_indices.contains(&i);
        let is_receding = is_past_day || is_unaccepted || is_past_event;

        // Choose color based on event status
        // Priority: Selected > Past/Unaccepted > Free > Current (Green) > Overlap (Red) > Next (Yellow) > Default
        // "Happening now" beats the overlap warning — the red still shows on the other event
        let event_color = if is_selected {
            colors::SELECTED
        } else if is_receding {
            colors::PAST_EVENT
        } else if is_free_event {
            colors::FREE_EVENT
        } else if is_current {
            colors::CURRENT_EVENT
        } else if is_overlapping {
            colors::OVERLAP_EVENT
        } else if is_next {
            colors::NEXT_EVENT
        } else {
            Color::Reset
        };

        // Selection indicator
        if is_selected {
            p.fg(Color::LightCyan);
            p.print(&"\u{25B6}".to_string()); // Right-pointing triangle
        } else if is_current && !is_unaccepted && !is_free_event {
            p.fg(Color::LightGreen);
            p.print(&"\u{25CF}".to_string()); // Filled circle
        } else if is_overlapping && !is_past_day && !is_unaccepted && !is_free_event && !is_past_event {
            p.fg(colors::OVERLAP_EVENT);
            p.print("!");
        } else if is_next && !is_unaccepted && !is_free_event {
            p.fg(Color::LightYellow);
            p.print(&"\u{25CB}".to_string()); // Empty circle
        } else {
            p.print(" ");
        }

        // Time (carries the status color; two-space gutter before the title)
        p.fg(event_color);
        if is_selected || ((is_current || is_next) && !is_unaccepted && !is_free_event) {
            p.bold();
        }
        p.print(&format!("{:>7}  ", when_text(&event.when)));
        p.reset();

        // The same start in the second zone, dim
        if let Some((tz, _)) = second_tz
            && let Some(there) = event.when.start().and_then(|m| time_in_zone(date, m, tz))
        {
            p.fg(Color::DarkGray);
            p.print(&format!("{:>w$} ", there, w = tz_width as usize - 1));
            p.reset();
        }

        // Title stays uncolored unless the row is selected or receding —
        // status colors live on the marker and time only
        let title_color = if is_selected {
            colors::SELECTED
        } else if is_receding {
            colors::PAST_EVENT
        } else if is_free_event {
            colors::FREE_EVENT
        } else {
            Color::Reset
        };
        p.move_to(title_x, row_y);
        p.fg(title_color);
        if is_selected {
            p.bold();
        }
        let title = truncate_str(&event.title, title_width);
        p.print(&title);
        p.reset();

        // Location right-aligned in whatever the title leaves free
        let spare = title_width.saturating_sub(title.width() + 2);
        if let Some(loc) = display_location(event)
            && spare >= 8
        {
            let loc = truncate_str(loc, spare.min(30));
            p.move_to(title_x + (title_width - loc.width()) as u16, row_y);
            p.fg(Color::DarkGray);
            p.print(&loc);
            p.reset();
        }

        // Duration and meeting service, dim, in fixed right-hand columns
        let mut trail_x = x + width - trail_width;
        p.fg(Color::DarkGray);
        if show_length {
            let length = match event.when {
                When::Timed { start, end: Some(end) } if !event.spans_days => format_length(end - start),
                _ => String::new(),
            };
            p.move_to(trail_x, row_y);
            p.print(&format!("{:>5} ", length));
            trail_x += 6;
        }
        if show_kind && let Some(ref url) = event.meeting_url {
            p.move_to(trail_x, row_y);
            p.print(meeting_kind(url));
        }
        p.reset();
    }

    // Clipped-rows indicator on the reserved last row, counting events only
    if total > visible {
        let count = |r: &[PanelRow]| r.iter().filter(|r| matches!(r, PanelRow::Event(_))).count();
        let above = count(&rows[..start]);
        let below = count(&rows[start + visible..]);
        p.move_to(x, content_start + visible as u16);
        p.fg(Color::DarkGray);
        let indicator = match (above > 0, below > 0) {
            (true, true) => format!(" \u{2026} {} above \u{00B7} {} more", above, below),
            (true, false) => format!(" \u{2026} {} above", above),
            (false, true) => format!(" \u{2026} +{} more", below),
            (false, false) => " \u{2026}".to_string(),
        };
        p.print(&truncate_str(&indicator, width as usize).to_string());
        p.reset();
    }
}

/// Keys that act on an event, for the footer: join if it has a link, RSVP
/// for Google invites, delete where the source supports it
fn event_actions(event: &DisplayEvent) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if event.meeting_url.is_some() {
        actions.push("J join");
    }
    match &event.id {
        EventId::Google { .. } => {
            actions.push(if event.accepted { "d decline" } else { "a accept" });
            actions.push("x delete");
        }
        // EventKit events have no CalDAV resource to delete
        EventId::ICloud { calendar_url, .. } if calendar_url.is_empty() => {}
        EventId::ICloud { .. } => actions.push("x delete"),
    }
    actions
}

/// Render event details in a column
fn render_event_details_column(
    p: &mut Pen,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    event: Option<&DisplayEvent>,
) {
    let content_x = x;
    let content_width = width as usize;
    let max_row = y + height.saturating_sub(1);

    let Some(event) = event else {
        p.move_to(content_x, y);
        p.fg(Color::DarkGray);
        p.print("No event selected");
        p.reset();
        return;
    };

    let mut current_row = y;

    // Title doubles as the panel header; long ones wrap onto a second line
    let mut title_lines = wrap_text(&event.title, content_width);
    if title_lines.len() > 2 {
        let rest = title_lines[1..].join(" ");
        title_lines.truncate(1);
        title_lines.push(truncate_str(&rest, content_width));
    }
    p.fg(colors::TITLE); p.bold();
    for line in &title_lines {
        p.move_to(content_x, current_row);
        p.print(line);
        current_row += 1;
    }
    p.reset();

    // Time, with the calendar source as a dim suffix
    p.move_to(content_x, current_row);
    let time_text = match event.when.end() {
        Some(end) => format!("{} \u{2013} {}", when_text(&event.when), time_text(end)),
        None => when_text(&event.when),
    };
    p.fg(colors::TIME);
    p.print(&truncate_str(&time_text, content_width).to_string());
    let (source, calendar_name) = match &event.id {
        EventId::Google { calendar_name, .. } => ("Google", calendar_name),
        EventId::ICloud { calendar_name, .. } => ("iCloud", calendar_name),
    };
    let source_text = match calendar_name {
        Some(name) => format!(" \u{00B7} {} \u{00B7} {}", source, name),
        None => format!(" \u{00B7} {}", source),
    };
    let remaining = content_width.saturating_sub(time_text.len());
    if remaining > 4 {
        p.fg(Color::DarkGray);
        p.print(&truncate_str(&source_text, remaining).to_string());
    }
    p.reset();
    current_row += 1;

    // Location
    if let Some(ref loc) = event.location
        && !loc.is_empty() && current_row < max_row {
            p.move_to(content_x, current_row);
            p.fg(Color::DarkGray);
            p.print(&truncate_str(loc, content_width).to_string());
            p.reset();
            current_row += 1;
        }

    // (The keys for this event live in the footer)

    // Description: agendas, dial-ins and doc links live here. It shares the
    // remaining height with the participant list.
    let description = event.description.as_deref().map(description_text).unwrap_or_default();
    if !description.is_empty() && current_row + 1 < max_row {
        current_row += 1; // blank line before the description
        let room = (max_row - current_row) as usize;
        let max_lines = if event.attendees.is_empty() { room } else { (room / 2).max(3).min(room) };
        let lines = wrap_text(&description, content_width);
        let shown = lines.len().min(max_lines);
        p.fg(Color::Reset);
        for (n, line) in lines.iter().take(shown).enumerate() {
            p.move_to(content_x, current_row);
            // Last visible line of a longer description ends in an ellipsis
            if n + 1 == shown && lines.len() > shown {
                p.print(&truncate_str(&format!("{} \u{2026}", line), content_width));
            } else {
                p.print(line);
            }
            current_row += 1;
        }
        p.reset();
    }

    // Participants, headed by a one-line tally of responses
    current_row += 1; // blank line before participants
    if !event.attendees.is_empty() && current_row < max_row {
        p.move_to(content_x, current_row);
        p.fg(Color::DarkGray);
        p.print(&truncate_str(&attendee_summary(&event.attendees), content_width));
        p.reset();
        current_row += 1;

        let total = event.attendees.len();
        for (idx, attendee) in event.attendees.iter().enumerate() {
            if current_row >= max_row {
                break;
            }
            // On the last available row, summarize the rest instead of showing one more name
            let remaining = total - idx;
            if current_row == max_row - 1 && remaining > 1 {
                p.move_to(content_x, current_row);
                p.fg(Color::DarkGray);
                p.print(&format!("  \u{2026} +{} more", remaining));
                p.reset();
                break;
            }

            p.move_to(content_x, current_row);

            // Status icon
            p.fg(attendee.status.color());
            p.print(&format!("  {} ", attendee.status.icon()));
            p.reset();

            // Name or email
            let display_name = attendee.name.as_ref().unwrap_or(&attendee.email);
            let status_str = match attendee.status {
                AttendeeStatus::Organizer => " (org)",
                _ => "",
            };
            let name_width = content_width.saturating_sub(5 + status_str.len());
            p.print(&truncate_str(display_name, name_width).to_string());
            p.fg(Color::DarkGray);
            p.print(&status_str.to_string());
            p.reset();
            current_row += 1;
        }
    }
}

/// Check if an event is in the past (has started; all-day events never are)
fn is_event_past(event: &DisplayEvent, current_time: NaiveTime) -> bool {
    event.when.start().and_then(clock).is_some_and(|start| start < current_time)
}

/// Find indices of current (happening now) and next upcoming event
/// Returns (current_index, next_index)
pub fn find_current_and_next_events(events: &[DisplayEvent], current_time: NaiveTime) -> (Option<usize>, Option<usize>) {
    let mut current_idx: Option<usize> = None;
    let mut next_idx: Option<usize> = None;

    for (i, event) in events.iter().enumerate() {
        let Some(start) = event.when.start().and_then(clock) else { continue }; // Skip all-day events

        // Check if event is currently happening (started but not ended)
        if start <= current_time {
            let has_ended = event.when.end().and_then(clock).is_some_and(|end| current_time >= end);
            if !has_ended {
                // Event is still ongoing - it's the current candidate
                current_idx = Some(i);
            }
        } else {
            // First event that hasn't started yet
            next_idx = Some(i);
            break;
        }
    }

    (current_idx, next_idx)
}

/// Truncate a string to a maximum *display* width (terminal columns), appending
/// an ellipsis. Counts double-width characters (emoji, CJK) as 2 columns so
/// truncated titles can't bleed into the neighboring panel.
fn truncate_str(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let width: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if width <= max_width {
        return s.to_string();
    }
    let budget = max_width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > budget {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// Format a smart "when" string combining date and time based on proximity
fn format_smart_when(date: NaiveDate, when: &When, today: NaiveDate) -> String {
    let days = (date - today).num_days();
    let is_all_day = when.is_all_day();
    let time_str = when_text(when);

    if days == 0 {
        if is_all_day { "today".to_string() } else { format!("today {}", time_str) }
    } else if days == 1 {
        if is_all_day { "tmrw".to_string() } else { format!("tmrw {}", time_str) }
    } else if days >= 2 && days <= 6 {
        let weekday = date.format("%a").to_string();
        if is_all_day { weekday } else { format!("{} {}", weekday, time_str) }
    } else {
        date.format("%b %d").to_string()
    }
}

/// Render the interactive setup wizard
fn render_setup_wizard(p: &mut Pen, setup: &SetupState, term_width: u16, term_height: u16) {
    // Collect lines to render: (text, style)
    enum Style { Header, Normal, Dim, Accent, Error }

    let mut lines: Vec<(String, Style)> = Vec::new();
    let mut input_line: Option<String> = None;

    match setup.step {
        SetupStep::ShortcutAsk => {
            lines.push(("Keyboard Shortcut".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            #[cfg(target_os = "macos")]
            {
                lines.push(("Install Cmd+Shift+J to launch Calendarchy from anywhere?".into(), Style::Normal));
            }
            #[cfg(target_os = "linux")]
            {
                lines.push(("Install Super+Shift+J to launch Calendarchy from anywhere?".into(), Style::Normal));
                lines.push(("Adds a keybinding to ~/.config/hypr/bindings.conf".into(), Style::Dim));
            }
            lines.push(("".into(), Style::Normal));
            lines.push(("(y/n)".into(), Style::Accent));
        }
        SetupStep::ShortcutTerminalChoice => {
            lines.push(("Terminal Emulator".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Which terminal should the shortcut open?".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            for (i, name) in setup.available_terminals.iter().enumerate() {
                lines.push((format!("{}. {}", i + 1, name), Style::Accent));
            }
        }
        SetupStep::Welcome => {
            lines.push(("Calendarchy".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("No calendars configured yet.".into(), Style::Normal));
            lines.push(("This wizard will guide you through the setup.".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("Press Enter to start, q to quit.".into(), Style::Dim));
        }
        SetupStep::GoogleAsk => {
            lines.push(("Google Calendar".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Connect your Google Calendar?".into(), Style::Normal));
            lines.push(("You'll sign in with your Google account.".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("(y/n)".into(), Style::Accent));
        }
        SetupStep::GoogleAuthWaiting => {
            lines.push(("Google Calendar".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Sign in with your Google account in the browser.".into(), Style::Normal));
            lines.push(("Waiting for authorization...".into(), Style::Accent));
            lines.push(("".into(), Style::Normal));
            lines.push(("Press Esc to skip.".into(), Style::Dim));
        }
        SetupStep::ICloudAsk => {
            lines.push(("iCloud Calendar Setup".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Set up iCloud / personal calendar? (y/n)".into(), Style::Accent));
        }
        SetupStep::ICloudMethod => {
            lines.push(("iCloud Calendar Setup".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Choose how to connect:".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("1. System Calendars (recommended)".into(), Style::Accent));
            lines.push(("   Reads from macOS Calendar app. Zero configuration.".into(), Style::Normal));
            lines.push(("   Includes all calendars you've added in System Settings.".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("2. CalDAV (manual setup)".into(), Style::Dim));
            lines.push(("   Connect directly with Apple ID + app-specific password.".into(), Style::Normal));
            lines.push(("   Works on Linux. Only shows iCloud calendars.".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("Press 1 or 2 to choose.".into(), Style::Dim));
        }
        SetupStep::ICloudOpenUrl => {
            lines.push(("iCloud Calendar Setup".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("A browser window should have opened to:".into(), Style::Normal));
            lines.push(("Apple ID > Account Management".into(), Style::Accent));
            lines.push(("".into(), Style::Normal));
            lines.push(("Follow these steps:".into(), Style::Normal));
            lines.push(("1. Sign in to your Apple ID".into(), Style::Normal));
            lines.push(("2. Go to App-Specific Passwords".into(), Style::Normal));
            lines.push(("3. Generate a new password (name it \"Calendarchy\")".into(), Style::Normal));
            lines.push(("4. Copy the generated password (xxxx-xxxx-xxxx-xxxx)".into(), Style::Normal));
            lines.push(("".into(), Style::Normal));
            lines.push(("Press Enter when ready to paste credentials.".into(), Style::Dim));
        }
        SetupStep::ICloudAppleId => {
            lines.push(("iCloud Calendar Setup".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Enter your Apple ID (email):".into(), Style::Normal));
            input_line = Some(format!("> {}_", setup.input));
        }
        SetupStep::ICloudPassword => {
            lines.push(("iCloud Calendar Setup".into(), Style::Header));
            lines.push(("".into(), Style::Normal));
            lines.push(("Paste your app-specific password:".into(), Style::Normal));
            let masked: String = setup.input.chars().map(|_| '*').collect();
            input_line = Some(format!("> {}_", masked));
        }
        SetupStep::Done => {}
    }

    // Add input line
    if let Some(ref il) = input_line {
        lines.push(("".into(), Style::Normal)); // placeholder, we'll render input_line specially
        let _ = il; // used below
    }

    // Add error if any
    if setup.error.is_some() {
        lines.push(("".into(), Style::Normal));
        lines.push(("".into(), Style::Error)); // placeholder for error
    }

    // Calculate vertical centering
    let total_lines = lines.len() as u16;
    let start_y = term_height.saturating_sub(total_lines) / 2;
    let max_content_width = 60u16;
    let base_x = (term_width.saturating_sub(max_content_width)) / 2;

    let mut input_rendered = false;
    for (i, (text, style)) in lines.iter().enumerate() {
        let row = start_y + i as u16;
        if row >= term_height { break; }

        p.move_to(base_x, row);

        // Check if this is the input line placeholder
        if !input_rendered {
            if let Some(ref il) = input_line {
                if text.is_empty() && matches!(style, Style::Normal) && i > 0 {
                    let prev = &lines[i - 1];
                    if matches!(prev.1, Style::Normal) && (prev.0.contains("Paste") || prev.0.contains("Enter")) {
                        p.fg(Color::Reset);
                        let display = truncate_str(il, max_content_width as usize);
                        p.print(&display.to_string());
                        p.reset();
                        input_rendered = true;
                        continue;
                    }
                }
            }
        }

        // Check if this is the error placeholder
        if matches!(style, Style::Error) {
            if let Some(ref err) = setup.error {
                p.fg(Color::LightRed);
                p.print(&err.to_string());
                p.reset();
                continue;
            }
        }

        match style {
            Style::Header => {
                p.fg(colors::HEADER); p.bold();
                p.print(&text.to_string());
                p.reset();
            }
            Style::Accent => {
                p.fg(Color::LightGreen);
                p.print(&text.to_string());
                p.reset();
            }
            Style::Dim => {
                p.fg(Color::DarkGray);
                p.print(&text.to_string());
                p.reset();
            }
            Style::Error => {} // handled above
            Style::Normal => {
                p.print(&text.to_string());
            }
        }
    }
}

/// Render a centered search modal
fn render_search_modal(p: &mut Pen, search: &SearchState, now: DateTime<Local>, term_width: u16, term_height: u16) {
    use crate::app::EventSource;
    use crate::cache::EventId;

    let modal_width = 60u16.min(term_width.saturating_sub(4));
    let modal_height = (term_height * 3 / 4).max(10).min(term_height.saturating_sub(4));
    let start_x = (term_width.saturating_sub(modal_width)) / 2;
    let start_y = (term_height.saturating_sub(modal_height)) / 2;

    p.fg(colors::HEADER);

    // Top border with title
    p.move_to(start_x, start_y);
    p.print("┌─ Search ");
    let remaining_top = modal_width.saturating_sub(11);
    for _ in 0..remaining_top {
        p.print("─");
    }
    p.print("┐");

    // Empty rows
    for row in 1..modal_height.saturating_sub(1) {
        p.move_to(start_x, start_y + row);
        p.print("│");
        for _ in 0..modal_width.saturating_sub(2) {
            p.print(" ");
        }
        p.print("│");
    }

    // Bottom border
    p.move_to(start_x, start_y + modal_height.saturating_sub(1));
    p.print("└");
    for _ in 0..modal_width.saturating_sub(2) {
        p.print("─");
    }
    p.print("┘");

    p.reset();

    // Input field
    let content_x = start_x + 2;
    let content_width = (modal_width.saturating_sub(4)) as usize;
    p.move_to(content_x, start_y + 1);
    p.fg(Color::Reset); p.bold();
    let query_display = truncate_str(&search.query, content_width.saturating_sub(3));
    p.print(&format!("> {}_ ", query_display));
    p.reset();

    // Separator
    p.move_to(content_x, start_y + 2);
    p.fg(colors::SEPARATOR);
    for _ in 0..content_width {
        p.print("─");
    }
    p.reset();

    // Results area
    let results_start_y = start_y + 3;
    let results_height = (modal_height.saturating_sub(5)) as usize; // 3 top (border+input+sep) + 2 bottom (hint+border)

    if search.query.is_empty() {
        p.move_to(content_x, results_start_y);
        p.fg(Color::DarkGray);
        p.print("Type to search events...");
        p.reset();
    } else if search.results.is_empty() {
        p.move_to(content_x, results_start_y);
        p.fg(Color::DarkGray);
        p.print("No matching events");
        p.reset();
    } else {
        let num_title_matches = search.results.iter()
            .filter(|r| r.match_type == MatchType::Title)
            .count();
        let has_title_header = num_title_matches > 0;
        let has_people_header = num_title_matches < search.results.len();

        // Total visual rows = results + header rows
        let num_headers = has_title_header as usize + has_people_header as usize;
        let total_visual_rows = search.results.len() + num_headers;

        // Map selected_index to its visual row (accounting for headers above it)
        let selected_visual_row = {
            let mut row = search.selected_index;
            if has_title_header { row += 1; } // title header before first result
            if has_people_header && search.selected_index >= num_title_matches {
                row += 1; // people header before participant results
            }
            row
        };

        // Calculate visible window based on visual rows
        let visible_start = if selected_visual_row >= results_height {
            selected_visual_row - results_height + 1
        } else {
            0
        };

        let today = now.date_naive();
        let mut visual_row: usize = 0;
        let mut result_idx: usize = 0;
        let people_header_row = num_title_matches + has_title_header as usize;

        // Build visual rows: headers interleaved with results
        while visual_row < total_visual_rows && (visual_row < visible_start + results_height) {
            // Check if we need a section header at this visual row
            let is_header = (has_title_header && visual_row == 0)
                || (has_people_header && visual_row == people_header_row);
            if is_header {
                if visual_row >= visible_start {
                    let screen_row = results_start_y + (visual_row - visible_start) as u16;
                    let label = if visual_row == 0 { "Titles" } else { "People" };
                    draw_section_header(p, content_x, screen_row, label, content_width);
                }
                visual_row += 1;
                continue;
            }

            // Render a result row
            if result_idx >= search.results.len() {
                break;
            }
            let result = &search.results[result_idx];
            let is_selected = result_idx == search.selected_index;

            if visual_row >= visible_start {
                let row = results_start_y + (visual_row - visible_start) as u16;
                p.move_to(content_x, row);

                // Selection indicator
                if is_selected {
                    p.fg(colors::SELECTED);
                    p.print("▶ ");
                } else {
                    p.print("  ");
                }

                // Smart when column
                let when = format_smart_when(result.event.date, &result.event.when, today);
                p.fg(if is_selected { colors::SELECTED } else { Color::DarkGray });
                p.print(&format!("{:>11} ", when));

                // Source color indicator
                let source_color = match result.source {
                    EventSource::Google => google_accent(),
                    EventSource::ICloud => icloud_accent(),
                };
                p.fg(source_color);
                let source_char = match result.event.id {
                    EventId::Google { .. } => "G",
                    EventId::ICloud { .. } => "I",
                };
                p.print(&format!("{} ", source_char));

                // Title
                let title_space = content_width.saturating_sub(2 + 12 + 2);
                p.fg(if is_selected { colors::SELECTED } else { Color::Reset });
                if is_selected {
                    p.bold();
                }
                p.print(&truncate_str(&result.event.title, title_space).to_string());
                p.reset();
            }

            result_idx += 1;
            visual_row += 1;
        }
    }

    // Bottom hint
    let hint_y = start_y + modal_height.saturating_sub(2);
    p.move_to(content_x, hint_y);
    p.fg(Color::DarkGray);
    let count_str = if search.results.is_empty() {
        String::new()
    } else {
        format!("{}/{} ", search.selected_index + 1, search.results.len())
    };
    p.print(&format!("{}\u{2191}\u{2193}:navigate Enter:select Esc:close", count_str));
    p.reset();
}

/// Render the help overlay listing all keybindings and the availability legend
fn render_help_modal(p: &mut Pen, term_width: u16, term_height: u16) {
    use crate::keymap::Action::*;
    enum Line {
        Section(&'static str),
        Item(String, &'static str),
        Legend,
        Note(&'static str),
    }
    use Line::*;

    // Key labels come from the keymap in use, so rebound keys show as bound
    let keys = crate::keymap::active();
    let item = |actions: &[crate::keymap::Action], desc: &'static str| Item(keys.label_for(actions), desc);
    let lines = [
        Section("Navigate"),
        item(&[PrevDay, NextDay], "previous / next day"),
        item(&[PrevWeek, NextWeek], "previous / next week"),
        item(&[PrevMonth, NextMonth], "previous / next month"),
        item(&[EnterEvents, ExitEvents], "browse events / back"),
        item(&[Today, Now], "go to today / current event"),
        Section("Events"),
        item(&[PrevEvent, NextEvent], "previous / next event"),
        item(&[JumpEventsBack, JumpEventsForward], "jump 10 events"),
        item(&[SwitchPanel], "other panel"),
        item(&[Join], "join meeting & quit"),
        item(&[Accept, Decline, Delete], "accept / decline / delete"),
        Section("Search & misc"),
        item(&[Search], "search titles & people"),
        item(&[Refresh, ToggleLogs, Setup], "refresh / logs / setup"),
        item(&[OpenGoogleWeb, OpenICloudWeb], "open Google / iCloud in browser"),
        item(&[Quit], "quit"),
        Section("Week availability grid"),
        Legend,
        Item("▀ / ▄".to_string(), "first / second half-hour busy"),
        Item("▴ ▾".to_string(), "events before 08:00 / after 20:00"),
        Note("Bulgarian phonetic keys work too · any key closes"),
    ];

    let modal_width = 56u16.min(term_width.saturating_sub(2));
    let modal_height = (lines.len() as u16 + 2).min(term_height.saturating_sub(1));
    let start_x = (term_width.saturating_sub(modal_width)) / 2;
    let start_y = (term_height.saturating_sub(modal_height)) / 2;

    // Box with title, blank interior
    p.fg(colors::HEADER);
    p.move_to(start_x, start_y);
    p.print("┌─ Help ");
    for _ in 0..modal_width.saturating_sub(9) {
        p.print("─");
    }
    p.print("┐");
    for row in 1..modal_height.saturating_sub(1) {
        p.move_to(start_x, start_y + row);
        p.print("│");
        for _ in 0..modal_width.saturating_sub(2) {
            p.print(" ");
        }
        p.print("│");
    }
    p.move_to(start_x, start_y + modal_height.saturating_sub(1));
    p.print("└");
    for _ in 0..modal_width.saturating_sub(2) {
        p.print("─");
    }
    p.print("┘");
    p.reset();

    let content_x = start_x + 2;
    let max_row = start_y + modal_height.saturating_sub(1);
    let mut row = start_y + 1;
    for line in &lines {
        if row >= max_row {
            break;
        }
        p.move_to(content_x, row);
        match line {
            Section(title) => {
                p.fg(colors::HEADER); p.bold();
                p.print(&title.to_string());
                p.reset();
            }
            Item(keys, desc) => {
                p.fg(Color::Reset);
                p.print(&format!("{:>14}", truncate_str(keys, 14)));
                p.fg(Color::DarkGray);
                p.print(&format!("  {}", desc));
                p.reset();
            }
            Legend => {
                p.fg(Color::Rgb(busy_rgb().0, busy_rgb().1, busy_rgb().2));
                p.print(&format!("{:>14}", "██"));
                p.fg(Color::DarkGray);
                p.print(" busy  ");
                p.fg(Color::Rgb(overlap_rgb().0, overlap_rgb().1, overlap_rgb().2));
                p.print("██");
                p.fg(Color::DarkGray);
                p.print(" double-booked  ");
                p.fg(free_block_color());
                p.print("██");
                p.fg(Color::DarkGray);
                p.print(" free");
                p.reset();
            }
            Note(text) => {
                p.fg(Color::DarkGray);
                p.print(&text.to_string());
                p.reset();
            }
        }
        row += 1;
    }
}

/// Render a centered confirmation modal
fn render_confirmation_modal(p: &mut Pen, action: &PendingAction, term_width: u16, term_height: u16) {
    let prompt = match action {
        PendingAction::AcceptEvent { .. } => "Accept this event?",
        PendingAction::DeclineEvent { .. } => "Decline this event?",
        PendingAction::DeleteGoogleEvent { .. } | PendingAction::DeleteICloudEvent { .. } => "Delete this event?",
    };

    // Modal dimensions
    let modal_width = 30u16;
    let modal_height = 5u16;
    let start_x = (term_width.saturating_sub(modal_width)) / 2;
    let start_y = (term_height.saturating_sub(modal_height)) / 2;

    // Draw modal box
    p.fg(colors::HEADER);

    // Top border
    p.move_to(start_x, start_y);
    p.print("┌");
    for _ in 0..modal_width.saturating_sub(2) {
        p.print("─");
    }
    p.print("┐");

    // Middle rows
    for row in 1..modal_height.saturating_sub(1) {
        p.move_to(start_x, start_y + row);
        p.print("│");
        for _ in 0..modal_width.saturating_sub(2) {
            p.print(" ");
        }
        p.print("│");
    }

    // Bottom border
    p.move_to(start_x, start_y + modal_height.saturating_sub(1));
    p.print("└");
    for _ in 0..modal_width.saturating_sub(2) {
        p.print("─");
    }
    p.print("┘");

    // Title
    p.move_to(start_x + 2, start_y + 1);
    p.fg(colors::NEXT_EVENT); p.bold();
    p.print(&prompt.to_string());
    p.reset();

    // Options
    p.move_to(start_x + 2, start_y + 3);
    p.fg(colors::ACTION);
    p.print("[y/Enter]");
    p.fg(Color::Reset);
    p.print(" Yes  ");
    p.fg(Color::DarkGray);
    p.print("[n/Esc]");
    p.fg(Color::Reset);
    p.print(" No");
    p.reset();
}

fn days_in_month(date: NaiveDate) -> u32 {
    match date.month() {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let year = date.year();
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: "Test".to_string(),
            when: When::parse_label(time),
            spans_days: false,
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees: vec![],
        }
    }

    #[test]
    fn test_event_ending_at_midnight_is_current_in_the_evening() {
        // Previously an end of "00:00" made an evening event look already over
        let events = vec![make_event_with_end("21:00", "00:00")];
        let current = NaiveTime::from_hms_opt(22, 30, 0).unwrap();
        assert_eq!(find_current_and_next_events(&events, current), (Some(0), None));
    }

    #[test]
    fn test_is_event_past_before_current() {
        let event = make_event("09:00");
        let current = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        assert!(is_event_past(&event, current));
    }

    #[test]
    fn test_is_event_past_after_current() {
        let event = make_event("14:00");
        let current = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        assert!(!is_event_past(&event, current));
    }

    #[test]
    fn test_is_event_past_all_day_never_past() {
        let event = make_event("All day");
        let current = NaiveTime::from_hms_opt(23, 59, 0).unwrap();
        assert!(!is_event_past(&event, current));
    }

    #[test]
    fn test_find_current_and_next_no_events() {
        let events: Vec<DisplayEvent> = vec![];
        let current = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let (current_idx, next_idx) = find_current_and_next_events(&events, current);
        assert!(current_idx.is_none());
        assert!(next_idx.is_none());
    }

    #[test]
    fn test_find_current_and_next_all_future() {
        let events = vec![
            make_event("14:00"),
            make_event("15:00"),
            make_event("16:00"),
        ];
        let current = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let (current_idx, next_idx) = find_current_and_next_events(&events, current);
        assert!(current_idx.is_none());
        assert_eq!(next_idx, Some(0));
    }

    #[test]
    fn test_find_current_and_next_all_past() {
        let events = vec![
            make_event("08:00"),
            make_event("09:00"),
            make_event("10:00"),
        ];
        let current = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let (current_idx, next_idx) = find_current_and_next_events(&events, current);
        assert_eq!(current_idx, Some(2)); // Last started event
        assert!(next_idx.is_none());
    }

    #[test]
    fn test_find_current_and_next_mixed() {
        let events = vec![
            make_event("08:00"),
            make_event("10:00"), // current (started at 10:00)
            make_event("14:00"), // next
            make_event("16:00"),
        ];
        let current = NaiveTime::from_hms_opt(10, 30, 0).unwrap();
        let (current_idx, next_idx) = find_current_and_next_events(&events, current);
        assert_eq!(current_idx, Some(1));
        assert_eq!(next_idx, Some(2));
    }

    #[test]
    fn test_find_current_and_next_skips_all_day() {
        let events = vec![
            make_event("All day"),
            make_event("10:00"),
            make_event("14:00"),
        ];
        let current = NaiveTime::from_hms_opt(10, 30, 0).unwrap();
        let (current_idx, next_idx) = find_current_and_next_events(&events, current);
        assert_eq!(current_idx, Some(1)); // Skipped all-day
        assert_eq!(next_idx, Some(2));
    }

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("Hello", 10), "Hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("Hello", 5), "Hello");
    }

    #[test]
    fn test_truncate_str_long() {
        assert_eq!(truncate_str("Hello World", 8), "Hello W…");
    }

    #[test]
    fn test_days_in_month_january() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(days_in_month(date), 31);
    }

    #[test]
    fn test_days_in_month_april() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert_eq!(days_in_month(date), 30);
    }

    #[test]
    fn test_days_in_month_february_non_leap() {
        let date = NaiveDate::from_ymd_opt(2025, 2, 1).unwrap();
        assert_eq!(days_in_month(date), 28);
    }

    #[test]
    fn test_days_in_month_february_leap() {
        let date = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        assert_eq!(days_in_month(date), 29);
    }

    #[test]
    fn test_days_in_month_february_century_non_leap() {
        let date = NaiveDate::from_ymd_opt(1900, 2, 1).unwrap();
        assert_eq!(days_in_month(date), 28);
    }

    #[test]
    fn test_days_in_month_february_400_year_leap() {
        let date = NaiveDate::from_ymd_opt(2000, 2, 1).unwrap();
        assert_eq!(days_in_month(date), 29);
    }

    fn make_event_with_end(time: &str, end: &str) -> DisplayEvent {
        let mut e = make_event(time);
        e.when = e.when.ending(end);
        e
    }

    fn make_icloud_event(time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::ICloud { calendar_url: "test".to_string(), event_uid: "test-uid".to_string(), etag: None, calendar_name: None, href: None },
            title: "iCloud Test".to_string(),
            when: When::parse_label(time),
            spans_days: false,
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            accepted: true,
            is_organizer: false,
            is_free: false,
            meeting_url: None,
            description: None,
            location: None,
            attendees: vec![],
        }
    }

    fn make_icloud_event_with_end(time: &str, end: &str) -> DisplayEvent {
        let mut e = make_icloud_event(time);
        e.when = e.when.ending(end);
        e
    }

    #[test]
    fn test_overlap_no_events() {
        let (g, i) = compute_overlapping_events(&[], &[]);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_non_overlapping() {
        let google = vec![make_event_with_end("09:00", "10:00")];
        let icloud = vec![make_icloud_event_with_end("10:00", "11:00")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_cross_source() {
        let google = vec![make_event_with_end("09:00", "10:00")];
        let icloud = vec![make_icloud_event_with_end("09:30", "10:30")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.contains(&0));
        assert!(i.contains(&0));
    }

    #[test]
    fn test_overlap_same_source() {
        let google = vec![
            make_event_with_end("09:00", "10:00"),
            make_event_with_end("09:30", "10:30"),
        ];
        let (g, i) = compute_overlapping_events(&google, &[]);
        assert!(g.contains(&0));
        assert!(g.contains(&1));
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_adjacent_no_overlap() {
        // end == start → strict inequality means no overlap
        let google = vec![make_event_with_end("09:00", "10:00")];
        let icloud = vec![make_icloud_event_with_end("10:00", "11:00")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_skips_all_day() {
        let google = vec![make_event("All day")];
        let icloud = vec![make_icloud_event_with_end("09:00", "10:00")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_skips_free() {
        let mut google = vec![make_event_with_end("09:00", "10:00")];
        google[0].is_free = true;
        let icloud = vec![make_icloud_event_with_end("09:00", "10:00")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_skips_unaccepted() {
        let mut google = vec![make_event_with_end("09:00", "10:00")];
        google[0].accepted = false;
        let icloud = vec![make_icloud_event_with_end("09:00", "10:00")];
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_overlap_default_1hr_duration() {
        // No end time → defaults to start + 60 min
        let google = vec![make_event("09:00")]; // 09:00-10:00
        let icloud = vec![make_icloud_event("09:30")]; // 09:30-10:30
        let (g, i) = compute_overlapping_events(&google, &icloud);
        assert!(g.contains(&0));
        assert!(i.contains(&0));
    }

    // ---- rendering through ratatui's TestBackend -------------------------

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 1, 15, 9, 30, 0).unwrap()
    }

    fn sample_cache() -> EventCache {
        let mut cache = EventCache::new();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut standup = make_event_with_end("10:00", "10:30");
        standup.title = "Standup 👶🏻 with a rather long title that must be truncated".into();
        let mut lunch = make_event_with_end("12:00", "13:00");
        lunch.title = "Lunch".into();
        lunch.id = EventId::Google { calendar_id: "test".into(), event_id: "lunch".into(), calendar_name: None };
        cache.google.store(vec![standup, lunch], day);
        cache.icloud.store(vec![make_icloud_event_with_end("12:30", "13:30")], day);
        cache
    }

    fn draw(width: u16, height: u16, events: &EventCache, mode: NavigationMode, show_help: bool) -> String {
        draw_with(width, height, events, mode, show_help, &DisplayConfig::default())
    }

    fn draw_with(
        width: u16,
        height: u16,
        events: &EventCache,
        mode: NavigationMode,
        show_help: bool,
        display: &DisplayConfig,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let state = RenderState {
            current_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            selected_date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            show_logs: false,
            events,
            google_auth: &GoogleAuthState::NotConfigured,
            icloud_auth: &ICloudAuthState::NotConfigured,
            status_message: None,
            status_is_error: false,
            google_loading: false,
            icloud_loading: false,
            navigation_mode: mode,
            selected_source: EventSource::Google,
            selected_event_index: 0,
            pending_action: None,
            search: None,
            show_help,
            setup: None,
            display,
            now: fixed_now(),
        };
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_render_day_view() {
        let cache = sample_cache();
        let screen = draw(120, 40, &cache, NavigationMode::Day, false);
        assert!(screen.contains("January 2026"), "{screen}");
        assert!(screen.contains("Thu Jan 15"));
        assert!(screen.contains("Work"));
        assert!(screen.contains("Personal"));
        assert!(screen.contains("10:00  Standup"));
        assert!(screen.contains("12:00  Lunch"));
        // Countdown in the status bar uses the injected clock (09:30 → 10:00)
        assert!(screen.contains("Next: Standup"), "{screen}");
        assert!(screen.contains("in 30m"));
        assert!(screen.contains("? help"));
    }

    #[test]
    fn test_render_event_mode_shows_details() {
        let cache = sample_cache();
        let screen = draw(140, 40, &cache, NavigationMode::Event, false);
        assert!(screen.contains("10:00 \u{2013} 10:30"), "{screen}");
        assert!(screen.contains("x delete"));
    }

    #[test]
    fn test_render_help_overlay() {
        let cache = sample_cache();
        let screen = draw(120, 40, &cache, NavigationMode::Day, true);
        assert!(screen.contains("Help"));
        assert!(screen.contains("join meeting & quit"));
    }

    #[test]
    fn test_render_never_panics_at_tiny_sizes() {
        let cache = sample_cache();
        let display = sydney_nine_to_six();
        for w in 0..=30 {
            for h in 0..=12 {
                for mode in [NavigationMode::Day, NavigationMode::Event] {
                    draw(w, h, &cache, mode, false);
                    draw(w, h, &cache, mode, true);
                    draw_with(w, h, &cache, mode, false, &display);
                }
            }
        }
        // Widths around the column thresholds (duration, meeting kind, row cap)
        for w in 30..=120 {
            draw_with(w, 30, &cache, NavigationMode::Day, false, &display);
            draw_with(w, 30, &cache, NavigationMode::Event, false, &display);
        }
    }

    fn sydney_nine_to_six() -> DisplayConfig {
        DisplayConfig {
            second_timezone: Some("Australia/Sydney".to_string()),
            working_hours: Some("09:00-18:00".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_render_rows_show_length_meeting_kind_and_free_time() {
        let mut cache = sample_cache();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut events = cache.google.get(day).to_vec();
        events[0].meeting_url = Some("https://dext.zoom.us/j/123".into());
        cache.google.store(events, day);

        let screen = draw(120, 40, &cache, NavigationMode::Day, false);
        let standup = screen.lines().find(|l| l.contains("10:00  Standup")).unwrap();
        assert!(standup.trim_end().ends_with("30m zoom"), "{standup}");
        let lunch = screen.lines().find(|l| l.contains("12:00  Lunch")).unwrap();
        assert!(lunch.trim_end().ends_with("1h"), "{lunch}");
        // 10:30 → 12:00 is free across both calendars, listed under the standup
        assert!(screen.contains("── 1h 30m free ──"), "{screen}");
        let lines: Vec<&str> = screen.lines().collect();
        let at = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert_eq!(at("1h 30m free"), at("10:00  Standup") + 1);
    }

    #[test]
    fn test_render_coming_up_lists_the_following_days() {
        let mut cache = sample_cache();
        let friday = NaiveDate::from_ymd_opt(2026, 1, 16).unwrap();
        let mut review = make_event_with_end("15:00", "16:00");
        review.date = friday;
        review.title = "Design review".into();
        cache.google.store(vec![review], friday);

        let screen = draw(120, 40, &cache, NavigationMode::Day, false);
        assert!(screen.contains("Coming up"), "{screen}");
        assert!(screen.contains("Fri 16  15:00 Design review"), "{screen}");
        assert!(screen.contains("Sat 17  free"), "{screen}");
    }

    #[test]
    fn test_render_second_zone_in_rows_and_header() {
        let cache = sample_cache();
        let screen = draw_with(120, 40, &cache, NavigationMode::Day, false, &sydney_nine_to_six());
        // 15 Jan is summer in Sydney (UTC+11); the expected time depends on the
        // machine's zone, so compute it the same way the UI does
        let there = time_in_zone(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), 600, chrono_tz::Australia::Sydney).unwrap();
        let standup = screen.lines().find(|l| l.contains("Standup")).unwrap();
        assert!(standup.contains(&format!("10:00  {:>9} Standup", there)), "{standup}");
        assert!(screen.lines().next().unwrap().contains("Sydney"), "{screen}");
    }

    #[test]
    fn test_render_details_show_description_and_tally() {
        let mut cache = EventCache::new();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut e = make_event_with_end("10:00", "11:00");
        e.title = "Planning".into();
        e.description = Some("<p>Agenda:</p><ul><li>Roadmap</li><li>Hiring</li></ul>".into());
        e.attendees = vec![
            crate::cache::DisplayAttendee { name: Some("Ana".into()), email: "a@x".into(), status: AttendeeStatus::Organizer },
            crate::cache::DisplayAttendee { name: Some("Bo".into()), email: "b@x".into(), status: AttendeeStatus::Accepted },
            crate::cache::DisplayAttendee { name: Some("Cy".into()), email: "c@x".into(), status: AttendeeStatus::NeedsAction },
        ];
        cache.google.store(vec![e], day);

        let screen = draw(140, 40, &cache, NavigationMode::Event, false);
        assert!(screen.contains("Agenda:"), "{screen}");
        assert!(screen.contains("\u{2022} Roadmap"), "{screen}");
        assert!(screen.contains("2 going \u{00B7} 1 no reply"), "{screen}");
    }

    #[test]
    fn test_render_twelve_hour_sunday_first_week_numbers() {
        let cache = sample_cache();
        let display = DisplayConfig {
            time_format: Some("12h".into()),
            week_start: Some("sunday".into()),
            week_numbers: true,
            ..Default::default()
        };
        let screen = draw_with(120, 40, &cache, NavigationMode::Day, false, &display);
        assert!(screen.contains("10:00am  Standup"), "{screen}");
        assert!(screen.contains("12:00pm  Lunch"), "{screen}");
        assert!(screen.contains("   Su Mo Tu We Th Fr Sa"), "{screen}");
        // Jan 2026 starts on a Thursday: first row is ISO week 1, laid out from Sunday
        let first_row = screen.lines().nth(3).unwrap();
        // week number (3) + Su..We blank (4 × 3) + " 1"
        assert!(first_row.starts_with(&format!(" 1 {} 1  2  3", " ".repeat(12))), "{first_row:?}");
        // Heatmap columns follow the week start too
        assert!(screen.contains(" S  M  T  W  T  F  S"), "{screen}");
        // Prefs are per frame: the next default draw is back to 24h
        assert!(draw(120, 40, &cache, NavigationMode::Day, false).contains("10:00  Standup"));
    }

    #[test]
    fn test_time_text_twelve_hour() {
        set_prefs(&DisplayConfig { time_format: Some("12h".into()), ..Default::default() });
        assert_eq!(time_text(0), "12:00am");
        assert_eq!(time_text(9 * 60 + 5), "9:05am");
        assert_eq!(time_text(12 * 60), "12:00pm");
        assert_eq!(time_text(23 * 60 + 59), "11:59pm");
        assert_eq!(time_text(DAY_MINUTES), "12:00am");
        set_prefs(&DisplayConfig::default());
        assert_eq!(time_text(9 * 60 + 5), "09:05");
    }

    #[test]
    fn test_palette_reads_omarchy_colors() {
        let palette = Palette::from_omarchy_colors(
            "mode = \"light\"\naccent = \"#3264eb\"\nblue = \"#3264eb\"\nred=\"#c900c4\"\n# magenta missing\n",
        );
        assert_eq!(palette.google, (0x32, 0x64, 0xeb));
        assert_eq!(palette.icloud, Palette::default().icloud, "missing keys keep the default");
        assert_eq!(palette.background, None);
        let light = Palette::from_omarchy_colors("lighter_background = \"#ffffff\"\nbackground = \"#fafafa\"\n");
        assert_eq!(light.background, Some((0xfa, 0xfa, 0xfa)), "exact key, not a prefix match");
    }

    #[test]
    fn test_footer_lists_keys_for_the_selected_event() {
        let mut cache = sample_cache();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut events = cache.google.get(day).to_vec();
        events[0].meeting_url = Some("https://meet.google.com/abc".into());
        cache.google.store(events, day);
        let screen = draw(140, 40, &cache, NavigationMode::Event, false);
        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("J join \u{00B7} d decline \u{00B7} x delete"), "{footer}");
        assert!(footer.contains("Tab other panel"), "{footer}");
    }

    #[test]
    fn test_empty_panel_points_to_the_next_event() {
        let cache = sample_cache();
        let screen = draw(120, 40, &cache, NavigationMode::Day, false);
        // Nothing after the 15th in the sample, so just the plain note... until there is
        assert!(!screen.contains("No events"), "{screen}");
        let mut cache = EventCache::new();
        let next = NaiveDate::from_ymd_opt(2026, 1, 19).unwrap();
        let mut interview = make_icloud_event_with_end("14:00", "15:00");
        interview.date = next;
        interview.title = "Interview".into();
        cache.icloud.store(vec![interview], next);
        let screen = draw(120, 40, &cache, NavigationMode::Day, false);
        assert!(screen.contains("Nothing scheduled \u{00B7} next Mon 14:00 Interview"), "{screen}");
    }

    #[test]
    fn test_free_gaps_merge_both_calendars() {
        // Work 09–10 and 14–15; Personal 11–12. Free: 10–11 and 12–14.
        let google = [make_event_with_end("09:00", "10:00"), make_event_with_end("14:00", "15:00")];
        let icloud = [make_icloud_event_with_end("11:00", "12:00")];
        let gap = |source, after, minutes, now| FreeGap { source, after, minutes, now };
        assert_eq!(
            free_gaps(&google, &icloud, 0, None),
            vec![gap(EventSource::Google, 0, 60, false), gap(EventSource::ICloud, 0, 120, false)]
        );
        // At 13:00 (today) the second gap is under way with an hour left
        assert_eq!(free_gaps(&google, &icloud, 13 * 60, None), vec![gap(EventSource::ICloud, 0, 60, true)]);
        // At 10:45 the first gap is under way: short, but shown as "now"
        assert_eq!(
            free_gaps(&google, &icloud, 10 * 60 + 45, None),
            vec![gap(EventSource::Google, 0, 15, true), gap(EventSource::ICloud, 0, 120, false)]
        );
        // Working hours 09–13 cut the second gap down to 12–13
        assert_eq!(
            free_gaps(&google, &icloud, 0, Some((9 * 60, 13 * 60))),
            vec![gap(EventSource::Google, 0, 60, false), gap(EventSource::ICloud, 0, 60, false)]
        );
    }

    #[test]
    fn test_free_gaps_ignore_short_and_nonblocking_time() {
        let mut declined = make_event_with_end("10:30", "11:30");
        declined.accepted = false;
        // 10:00–10:20 is a 20m gap (too short); the declined event doesn't block
        let google = [make_event_with_end("09:00", "10:00"), declined, make_event_with_end("10:20", "11:00")];
        assert!(free_gaps(&google, &[], 0, None).is_empty());

        // An event inside a longer one: the gap anchors on the one ending last
        let google = [make_event_with_end("09:00", "12:00"), make_event_with_end("10:00", "11:00"), make_event_with_end("13:00", "14:00")];
        assert_eq!(
            free_gaps(&google, &[], 0, None),
            vec![FreeGap { source: EventSource::Google, after: 0, minutes: 60, now: false }]
        );
    }

    #[test]
    fn test_panel_rows_place_gap_after_overlapping_events() {
        // A declined event starting inside the busy block is listed before the
        // free time; one starting later is listed after it, in time order
        let mut declined_early = make_event_with_end("09:30", "10:30");
        declined_early.accepted = false;
        let mut declined_late = make_event_with_end("10:15", "10:45");
        declined_late.accepted = false;
        let events = [make_event_with_end("09:00", "10:00"), declined_early, declined_late, make_event_with_end("12:00", "13:00")];
        let gaps = free_gaps(&events, &[], 0, None);
        assert_eq!(
            panel_rows(&events, EventSource::Google, &gaps),
            vec![PanelRow::Event(0), PanelRow::Event(1), PanelRow::Gap { minutes: 120, now: false }, PanelRow::Event(2), PanelRow::Event(3)]
        );
        // Gaps anchored in the other panel don't show here
        assert_eq!(panel_rows(&events, EventSource::ICloud, &gaps).len(), 4);
    }

    #[test]
    fn test_busy_minutes_counts_overlaps_once() {
        let google = [make_event_with_end("09:00", "11:00"), make_event_with_end("10:00", "12:00")];
        let icloud = [make_icloud_event_with_end("11:30", "12:30"), make_icloud_event("All day")];
        assert_eq!(busy_minutes(&google, &icloud), 210);
    }

    #[test]
    fn test_format_length() {
        assert_eq!(format_length(30), "30m");
        assert_eq!(format_length(60), "1h");
        assert_eq!(format_length(90), "1h30");
        assert_eq!(format_length(125), "2h05");
    }

    #[test]
    fn test_meeting_kind() {
        assert_eq!(meeting_kind("https://dext.zoom.us/j/1"), "zoom");
        assert_eq!(meeting_kind("https://meet.google.com/abc"), "meet");
        assert_eq!(meeting_kind("https://teams.microsoft.com/l/x"), "teams");
        assert_eq!(meeting_kind("https://example.com"), "link");
    }

    #[test]
    fn test_description_text_strips_html() {
        let html = "Hi&nbsp;all<br>Notes: <a href=\"https://x.y\">doc</a><br><br><br>Q&amp;A<ul><li>One</li><li>Two</li></ul>";
        assert_eq!(description_text(html), "Hi all\nNotes: doc\n\nQ&A\n\u{2022} One\n\u{2022} Two");
        assert_eq!(description_text("plain\n\n\n\ntext\n"), "plain\n\ntext");
    }

    #[test]
    fn test_wrap_text() {
        assert_eq!(wrap_text("the quick brown fox", 9), vec!["the quick", "brown fox"]);
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn test_pen_measures_per_char_and_clips() {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 6, 1));
        let mut p = Pen::new(&mut buf);
        p.print("👶🏻ab"); // 2 + 2 columns for the emoji, as foot/alacritty draw it
        assert_eq!(buf[(0, 0)].symbol(), "👶");
        assert_eq!(buf[(2, 0)].symbol(), "🏻");
        assert_eq!(buf[(4, 0)].symbol(), "a");
        assert_eq!(buf[(5, 0)].symbol(), "b");
        let mut p = Pen::new(&mut buf);
        p.print("abcdefgh"); // clipped at the edge, no panic
        assert_eq!(buf[(5, 0)].symbol(), "f");
    }

    #[test]
    fn test_countdown_truncates_like_a_clock_difference() {
        // At 10:00:00.05 an event at 10:05 is 4m59.95s away: shown as "4m"
        let mut cache = EventCache::new();
        let today = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut ev = make_event("10:05");
        ev.date = today;
        cache.google.store(vec![ev], today);
        let now = NaiveTime::from_hms_nano_opt(10, 0, 0, 50_000_000).unwrap();
        assert_eq!(find_next_event(&cache, today, now).unwrap().minutes_until, 4);
    }

    #[test]
    fn test_multi_day_events_do_not_hide_the_next_meeting() {
        let mut cache = EventCache::new();
        let today = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let mut conference = make_event_with_end("00:00", "00:00");
        conference.title = "Conference".into();
        conference.spans_days = true;
        conference.date = today;
        let mut meeting = make_icloud_event_with_end("14:00", "15:00");
        meeting.title = "Meeting".into();
        meeting.date = today;
        cache.google.store(vec![conference], today);
        cache.icloud.store(vec![meeting], today);
        let now = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        assert_eq!(find_next_event(&cache, today, now).unwrap().event.title, "Meeting");
    }
}
