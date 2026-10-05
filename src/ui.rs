use crate::app::{EventSource, MatchType, NavigationMode, PendingAction, SearchState, SetupState, SetupStep};
use crate::auth::{AuthDisplay, GoogleAuthState, ICloudAuthState};
use crate::cache::{AttendeeStatus, DisplayEvent, EventCache, EventId};
use crate::logging::get_recent_logs;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use std::collections::HashSet;
use std::sync::OnceLock;
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
static TERM_BG: OnceLock<(u8, u8, u8)> = OnceLock::new();

pub fn set_term_bg(r: u8, g: u8, b: u8) {
    let _ = TERM_BG.set((r, g, b));
}

/// Falls back to a dark background if the terminal never answered the query
fn term_bg() -> (u8, u8, u8) {
    *TERM_BG.get().unwrap_or(&(30, 32, 38))
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

const CALENDAR_WIDTH: u16 = 23;
const MIN_PANEL_WIDTH: u16 = 25;


// Semantic color constants
mod colors {
    use ratatui::style::Color;

    // Calendar sources (muted so panel labels read as chrome, not content)
    pub const GOOGLE_ACCENT: Color = Color::Rgb(96, 125, 168);
    pub const ICLOUD_ACCENT: Color = Color::Rgb(152, 115, 168);

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
    pub const TITLE: Color = Color::White;
    pub const TIME: Color = Color::White;
    pub const ACTION: Color = Color::LightGreen;

    // Overlap indicator
    pub const OVERLAP_EVENT: Color = Color::LightRed;

    // Week availability. Mid-tone marks that read on light and dark themes;
    // the free shade and past fading are derived from the real terminal
    // background at runtime (see free_block_color / blend_toward_bg).
    pub const BUSY_RGB: (u8, u8, u8) = (84, 113, 156);
    pub const HEATMAP_OVERLAP_RGB: (u8, u8, u8) = (156, 85, 85);
    pub const BUSY_BLOCK: Color = Color::Rgb(BUSY_RGB.0, BUSY_RGB.1, BUSY_RGB.2);
    pub const HEATMAP_OVERLAP: Color = Color::Rgb(HEATMAP_OVERLAP_RGB.0, HEATMAP_OVERLAP_RGB.1, HEATMAP_OVERLAP_RGB.2);

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

    // Find current or next event today
    for event in &all_today {
        if event.time_str == "All day" {
            continue;
        }

        let Some(start_time) = parse_event_time(&event.time_str) else {
            continue;
        };

        // Calculate end time
        let end_time = event.end_time_str.as_ref()
            .and_then(|s| parse_event_time(s))
            .unwrap_or_else(|| start_time + chrono::Duration::hours(1));

        if current_time < end_time {
            // This event hasn't ended yet
            let minutes_until = (start_time - current_time).num_minutes();
            let is_current = current_time >= start_time;

            return Some(NextEventInfo {
                event,
                is_current,
                minutes_until,
            });
        }
    }

    // Check future days (up to 7 days ahead)
    for days_ahead in 1..=7 {
        let check_date = today + Duration::days(days_ahead);
        let future_events: Vec<&DisplayEvent> = events.google.get(check_date).iter()
            .chain(events.icloud.get(check_date).iter())
            .filter(|e| e.accepted && e.time_str != "All day")
            .collect();

        if let Some(event) = future_events.first()
            && let Some(start_time) = parse_event_time(&event.time_str)
        {
            // Calculate minutes from now until the event
            // Remaining today + full days + time into target day
            let remaining_today = (NaiveTime::from_hms_opt(23, 59, 59).unwrap() - current_time).num_minutes();
            let full_days_minutes = (days_ahead - 1) * 24 * 60;
            let target_day_minutes = (start_time - NaiveTime::from_hms_opt(0, 0, 0).unwrap()).num_minutes();
            let minutes_until = remaining_today + full_days_minutes + target_day_minutes + 1;

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
        // Calm footer: the full keymap lives in the ? overlay
        let mut c = String::from(" ? help \u{00B7} q quit");
        if state.navigation_mode == NavigationMode::Day {
            if !state.google_auth.is_authenticated() {
                c.push_str(" \u{00B7} g connect work");
            }
            if !state.icloud_auth.is_authenticated() {
                c.push_str(" \u{00B7} i connect personal");
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

    let cal_width = CALENDAR_WIDTH;

    if in_event_mode {
        let available = term_width.saturating_sub(cal_width + 2);
        // Details panel: fixed width or 1/3 of available
        details_panel_width = (available / 3).clamp(MIN_PANEL_WIDTH, 40);
        events_panel_width = available.saturating_sub(details_panel_width + 1);
    } else {
        events_panel_width = term_width.saturating_sub(cal_width + 1);
        details_panel_width = 0;
    }

    // Reserve 2 rows for column headers
    let header_rows = 2u16;

    // Render calendar on left
    render_calendar(p, state.current_date, state.selected_date, state.now, state.events, state.google_loading || state.icloud_loading, term_height);

    // Render event panels in the middle
    if events_panel_width >= MIN_PANEL_WIDTH {
        let events_x = cal_width + 1;

        // Events column header: selected date
        p.move_to(events_x, 0);
        p.bold();
        p.print(&format!("{}", state.selected_date.format("%a %b %d")));
        p.reset();

        let google_events = state.events.google.get(state.selected_date);
        let icloud_events = state.events.icloud.get(state.selected_date);
        let is_past_day = state.selected_date < today;
        let (google_overlaps, icloud_overlaps) = compute_overlapping_events(google_events, icloud_events);

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
        let google_needed = google_events.len().max(1);
        let icloud_needed = icloud_events.len().max(1);
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
            "Work",
            google_events,
            state.google_loading,
            colors::GOOGLE_ACCENT,
            is_today,
            is_past_day,
            current_time,
            google_selected,
            &google_overlaps,
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
            "Personal",
            icloud_events,
            state.icloud_loading,
            colors::ICLOUD_ACCENT,
            is_today,
            is_past_day,
            current_time,
            icloud_selected,
            &icloud_overlaps,
            icloud_rows,
        );
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

fn render_calendar(
    p: &mut Pen,
    current_date: NaiveDate,
    selected_date: NaiveDate,
    now: DateTime<Local>,
    events: &EventCache,
    is_loading: bool,
    term_height: u16,
) {
    let today = now.date_naive();
    p.move_to(0, 0);

    // Month header
    p.bold();

    let cal_width = CALENDAR_WIDTH;
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
    p.move_to(0, 2);
    p.fg(Color::DarkGray);
    p.print("Mo Tu We Th Fr Sa Su");
    p.reset();

    // Calendar grid
    let first_day = current_date.with_day(1).unwrap();
    let start_weekday = first_day.weekday().num_days_from_monday();
    let days_in_month = days_in_month(current_date);
    let cols = 7;

    for row in 0..6 {
        p.move_to(0, 3 + row as u16);

        for col in 0..cols {
            let cell = row * 7 + col; // Always use 7-day weeks for calculation
            if cell < start_weekday || cell >= start_weekday + days_in_month {
                p.print("   ");
            } else {
                let day = cell - start_weekday + 1;
                let date = first_day.with_day(day).unwrap();
                let is_today = date == today;
                let is_selected = date == selected_date;
                let is_weekend = col >= 5;

                if is_selected {
                    // Explicit colors: Reverse over a dark theme made the cursor nearly invisible
                    p.bg(Color::LightCyan); p.fg(Color::Black);
                } else if is_today {
                    p.fg(Color::LightGreen); p.bold();
                } else if is_weekend {
                    p.fg(Color::DarkGray);
                }

                p.print(&format!("{:2} ", day));

                p.reset();
            }
        }
    }

    // Render week availability below the calendar grid
    render_week_availability(p, events, selected_date, now, term_height);
}

/// Parse an event's time range into (start_minutes, end_minutes) from midnight.
/// Returns None for all-day, free, or unaccepted events (not time-blocking).
fn parse_event_range(event: &DisplayEvent) -> Option<(u32, u32)> {
    if event.time_str == "All day" || event.is_free || !event.accepted {
        return None;
    }

    let start_time = parse_event_time(&event.time_str)?;
    let event_start = start_time.hour() * 60 + start_time.minute();

    let event_end = if let Some(ref end_str) = event.end_time_str {
        if end_str == "All day" {
            return None;
        }
        parse_event_time(end_str)
            .map(|t| {
                let mins = t.hour() * 60 + t.minute();
                if mins == 0 { 24 * 60 } else { mins }
            })
            .unwrap_or(event_start + 60)
    } else {
        event_start + 60
    };

    Some((event_start, event_end))
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

/// Count how many time-blocking events cover a given slot (across both sources).
fn count_slot_events(google_events: &[DisplayEvent], icloud_events: &[DisplayEvent], slot_start: u32, slot_end: u32) -> usize {
    google_events.iter().chain(icloud_events.iter())
        .filter_map(parse_event_range)
        .filter(|(es, ee)| slot_start < *ee && slot_end > *es)
        .count()
}

/// Get the Monday of the week containing the given date
fn get_week_monday(date: NaiveDate) -> NaiveDate {
    let weekday = date.weekday().num_days_from_monday();
    date - Duration::days(weekday as i64)
}

/// Render week availability grid below the calendar
fn render_week_availability(
    p: &mut Pen,
    events: &EventCache,
    selected_date: NaiveDate,
    now: DateTime<Local>,
    term_height: u16,
) {
    let start_row = 10u16; // Below the calendar grid
    let monday = get_week_monday(selected_date);
    let today = now.date_naive();
    let current_minutes = now.hour() * 60 + now.minute();
    let num_days = 7;
    let max_row = term_height.saturating_sub(2); // don't collide with the status bar

    // Header row: highlight the selected day's column (and today's)
    p.move_to(0, start_row);
    p.print("   ");
    for day_offset in 0..7i64 {
        let date = monday + Duration::days(day_offset);
        let letter = ["M", "T", "W", "T", "F", "S", "S"][day_offset as usize];
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
                let rgb = if count >= 2 { colors::HEATMAP_OVERLAP_RGB } else { colors::BUSY_RGB };
                if past {
                    blend_toward_bg(rgb, 0.55)
                } else {
                    Color::Rgb(rgb.0, rgb.1, rgb.2)
                }
            };
            let free = free_block_color();

            // Vertical half-blocks: ▀ = first half-hour busy, ▄ = second.
            // The free half is painted with the derived free shade via bg color.
            match (first_half_busy, second_half_busy) {
                (true, true) => {
                    let top = color_for(first_half_count, first_half_past);
                    let bot = color_for(second_half_count, second_half_past);
                    if top == bot {
                        p.fg(top);
                        p.print("██");
                    } else {
                        p.fg(top); p.bg(bot);
                        p.print("▀▀");
                    }
                }
                (true, false) => {
                    p.fg(color_for(first_half_count, first_half_past)); p.bg(free);
                    p.print("▀▀");
                }
                (false, true) => {
                    p.fg(color_for(second_half_count, second_half_past)); p.bg(free);
                    p.print("▄▄");
                }
                (false, false) => {
                    p.fg(free);
                    p.print("██");
                }
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
fn render_event_panel(
    p: &mut Pen,
    x: u16,
    y: u16,
    width: u16,
    title: &str,
    events: &[DisplayEvent],
    is_loading: bool,
    accent_color: Color,
    is_today: bool,
    is_past_day: bool,
    current_time: NaiveTime,
    selected_index: Option<usize>,
    overlapping_indices: &HashSet<usize>,
    max_rows: usize,
) {
    // Panel header: just the label in a muted accent — no rules
    p.move_to(x, y);
    p.fg(accent_color);
    let loading_str = if is_loading { "*" } else { "" };
    p.print(&format!("{}{}", title, loading_str));
    p.reset();

    let content_start = y + 1;

    if events.is_empty() {
        p.move_to(x, content_start);
        p.fg(Color::DarkGray);
        if is_loading {
            p.print("Loading...");
        } else {
            p.print("No events");
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

    // Scroll window: keep the selected event visible, reserve the last row
    // for a "+N more" indicator when the panel can't fit everything
    let total = events.len();
    let (start, visible) = if total <= max_rows {
        (0usize, total)
    } else {
        let visible = max_rows.saturating_sub(1).max(1);
        let sel = selected_index.unwrap_or(0);
        let mut start = if sel >= visible { sel + 1 - visible } else { 0 };
        if start + visible > total {
            start = total - visible;
        }
        (start, visible)
    };

    for (row, i) in (start..start + visible).enumerate() {
        let event = &events[i];
        p.move_to(x, content_start + row as u16);

        let is_selected = selected_index == Some(i);
        let is_current = current_event_idx == Some(i);
        let is_next = next_event_idx == Some(i);
        let is_past_event = is_today && is_event_past(event, current_time) && !is_current;
        let is_unaccepted = !event.accepted;
        let is_free_event = event.is_free;
        let is_overlapping = overlapping_indices.contains(&i);

        // Choose color based on event status
        // Priority: Selected > Past/Unaccepted > Free > Current (Green) > Overlap (Red) > Next (Yellow) > Default
        // "Happening now" beats the overlap warning — the red still shows on the other event
        let event_color = if is_selected {
            colors::SELECTED
        } else if is_past_day || is_unaccepted || is_past_event {
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
        p.print(&format!("{:>7}  ", event.time_str));
        p.reset();

        // Title stays uncolored unless the row is selected or receding —
        // status colors live on the marker and time only
        let title_color = if is_selected {
            colors::SELECTED
        } else if is_past_day || is_unaccepted || is_past_event {
            colors::PAST_EVENT
        } else if is_free_event {
            colors::FREE_EVENT
        } else {
            Color::Reset
        };
        p.fg(title_color);
        if is_selected {
            p.bold();
        }
        let title_width = width.saturating_sub(11) as usize;
        p.print(&truncate_str(&event.title, title_width).to_string());
        p.reset();
    }

    // Clipped-events indicator on the reserved last row
    if total > visible {
        let below = total - (start + visible);
        p.move_to(x, content_start + visible as u16);
        p.fg(Color::DarkGray);
        let indicator = match (start > 0, below > 0) {
            (true, true) => format!(" \u{2026} {} above \u{00B7} {} more", start, below),
            (true, false) => format!(" \u{2026} {} above", start),
            _ => format!(" \u{2026} +{} more", below),
        };
        p.print(&truncate_str(&indicator, width as usize).to_string());
        p.reset();
    }
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

    // Title doubles as the panel header
    p.move_to(content_x, current_row);
    p.fg(colors::TITLE); p.bold();
    p.print(&truncate_str(&event.title, content_width).to_string());
    p.reset();
    current_row += 1;

    // Time, with the calendar source as a dim suffix
    p.move_to(content_x, current_row);
    let time_text = match event.end_time_str {
        Some(ref end) => format!("{} \u{2013} {}", event.time_str, end),
        None => event.time_str.clone(),
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

    // Actions on one dim line
    current_row += 1; // blank line before actions
    if current_row < max_row {
        let mut actions: Vec<&str> = Vec::new();
        if event.meeting_url.is_some() {
            actions.push("J join");
        }
        if matches!(event.id, EventId::Google { .. }) {
            actions.push(if event.accepted { "d decline" } else { "a accept" });
        }
        actions.push("x delete");

        p.move_to(content_x, current_row);
        p.fg(Color::DarkGray);
        p.print(&truncate_str(&actions.join("  "), content_width).to_string());
        p.reset();
        current_row += 1;
    }

    // Participants
    current_row += 1; // blank line before participants
    if !event.attendees.is_empty() && current_row < max_row {
        p.move_to(content_x, current_row);
        p.fg(Color::DarkGray);
        p.print("Participants");
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

/// Parse time string like "14:30" into NaiveTime
fn parse_event_time(time_str: &str) -> Option<NaiveTime> {
    if time_str == "All day" {
        return NaiveTime::from_hms_opt(0, 0, 0);
    }
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() == 2 {
        let hour: u32 = parts[0].parse().ok()?;
        let minute: u32 = parts[1].parse().ok()?;
        NaiveTime::from_hms_opt(hour, minute, 0)
    } else {
        None
    }
}

/// Check if an event is in the past
fn is_event_past(event: &DisplayEvent, current_time: NaiveTime) -> bool {
    if let Some(event_time) = parse_event_time(&event.time_str) {
        if event.time_str == "All day" {
            return false; // All-day events are never "past" during the day
        }
        event_time < current_time
    } else {
        false
    }
}

/// Find indices of current (happening now) and next upcoming event
/// Returns (current_index, next_index)
pub fn find_current_and_next_events(events: &[DisplayEvent], current_time: NaiveTime) -> (Option<usize>, Option<usize>) {
    let mut current_idx: Option<usize> = None;
    let mut next_idx: Option<usize> = None;

    for (i, event) in events.iter().enumerate() {
        if let Some(event_time) = parse_event_time(&event.time_str) {
            if event.time_str == "All day" {
                continue; // Skip all-day events
            }

            // Check if event is currently happening (started but not ended)
            if event_time <= current_time {
                // Check if event has ended
                let has_ended = event.end_time_str.as_ref().map_or(false, |end_str| {
                    parse_event_time(end_str).map_or(false, |end_time| current_time >= end_time)
                });

                if !has_ended {
                    // Event is still ongoing - it's the current candidate
                    current_idx = Some(i);
                }
            } else if next_idx.is_none() {
                // First event that hasn't started yet
                next_idx = Some(i);
                break; // No need to continue
            }
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
fn format_smart_when(date: NaiveDate, time_str: &str, today: NaiveDate) -> String {
    let days = (date - today).num_days();
    let is_all_day = time_str == "All day";

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
                        p.fg(Color::White);
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
    p.fg(Color::White); p.bold();
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
                let when = format_smart_when(result.event.date, &result.event.time_str, today);
                p.fg(if is_selected { colors::SELECTED } else { Color::DarkGray });
                p.print(&format!("{:>11} ", when));

                // Source color indicator
                let source_color = match result.source {
                    EventSource::Google => colors::GOOGLE_ACCENT,
                    EventSource::ICloud => colors::ICLOUD_ACCENT,
                };
                p.fg(source_color);
                let source_char = match result.event.id {
                    EventId::Google { .. } => "G",
                    EventId::ICloud { .. } => "I",
                };
                p.print(&format!("{} ", source_char));

                // Title
                let title_space = content_width.saturating_sub(2 + 12 + 2);
                p.fg(if is_selected { colors::SELECTED } else { Color::White });
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
    enum Line {
        Section(&'static str),
        Item(&'static str, &'static str),
        Legend,
        Note(&'static str),
    }
    use Line::*;

    let lines = [
        Section("Navigate"),
        Item("h/l ← →", "previous / next day"),
        Item("j/k ↑ ↓", "previous / next week (or event)"),
        Item("H/L", "previous / next month"),
        Item("Enter / Esc", "browse events / back"),
        Item("t / n", "go to today / current event"),
        Item("^d / ^u", "month (days) · jump 10 (events)"),
        Section("Event actions"),
        Item("J", "join meeting & quit"),
        Item("a / d", "accept / decline (Google)"),
        Item("x", "delete event"),
        Section("Search & misc"),
        Item("f", "search titles & people"),
        Item("r / D / S", "refresh / logs / setup"),
        Item("1 / 2", "open Google / iCloud in browser"),
        Item("q", "quit"),
        Section("Week availability grid"),
        Legend,
        Item("▀ / ▄", "first / second half-hour busy"),
        Item("▴ ▾", "events before 08:00 / after 20:00"),
        Note("Bulgarian phonetic keys work too · any key closes"),
    ];

    let modal_width = 54u16.min(term_width.saturating_sub(2));
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
                p.fg(Color::White);
                p.print(&format!("{:>12}", keys));
                p.fg(Color::DarkGray);
                p.print(&format!("  {}", desc));
                p.reset();
            }
            Legend => {
                p.fg(colors::BUSY_BLOCK);
                p.print(&format!("{:>12}", "██"));
                p.fg(Color::DarkGray);
                p.print(" busy  ");
                p.fg(colors::HEATMAP_OVERLAP);
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
    p.fg(Color::White);
    p.print(" Yes  ");
    p.fg(Color::DarkGray);
    p.print("[n/Esc]");
    p.fg(Color::White);
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
    use chrono::Timelike;

    fn make_event(time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string(), calendar_name: None },
            title: "Test".to_string(),
            time_str: time.to_string(),
            end_time_str: None,
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
    fn test_parse_event_time_valid() {
        let time = parse_event_time("14:30").unwrap();
        assert_eq!(time.hour(), 14);
        assert_eq!(time.minute(), 30);
    }

    #[test]
    fn test_parse_event_time_all_day() {
        let time = parse_event_time("All day").unwrap();
        assert_eq!(time.hour(), 0);
        assert_eq!(time.minute(), 0);
    }

    #[test]
    fn test_parse_event_time_invalid() {
        assert!(parse_event_time("invalid").is_none());
        assert!(parse_event_time("25:00").is_none());
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
        e.end_time_str = Some(end.to_string());
        e
    }

    fn make_icloud_event(time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::ICloud { calendar_url: "test".to_string(), event_uid: "test-uid".to_string(), etag: None, calendar_name: None, href: None },
            title: "iCloud Test".to_string(),
            time_str: time.to_string(),
            end_time_str: None,
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
        e.end_time_str = Some(end.to_string());
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
        for w in 0..=30 {
            for h in 0..=12 {
                for mode in [NavigationMode::Day, NavigationMode::Event] {
                    draw(w, h, &cache, mode, false);
                    draw(w, h, &cache, mode, true);
                }
            }
        }
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
}
