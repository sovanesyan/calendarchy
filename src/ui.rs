use crate::app::{EventSource, MatchType, NavigationMode, PendingAction, SearchState, SetupState, SetupStep};
use crate::auth::{AuthDisplay, GoogleAuthState, ICloudAuthState};
use crate::cache::{AttendeeStatus, DisplayEvent, EventCache, EventId};
use crate::logging::get_recent_logs;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike};
use crossterm::{
    cursor,
    execute,
    style::{Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::collections::HashSet;
use std::io::{stdout, Write};
use std::sync::OnceLock;

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
    Color::Rgb { r: mix(color.0, bg.0), g: mix(color.1, bg.1), b: mix(color.2, bg.2) }
}

/// The "free slot" shade: the real background nudged just enough to be visible,
/// darker on light themes and lighter on dark ones
fn free_block_color() -> Color {
    let (r, g, b) = term_bg();
    let luma = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    let shift: i16 = if luma > 128.0 { -20 } else { 24 };
    let adj = |c: u8| -> u8 { (c as i16 + shift).clamp(0, 255) as u8 };
    Color::Rgb { r: adj(r), g: adj(g), b: adj(b) }
}

const CALENDAR_WIDTH: u16 = 23;
const MIN_PANEL_WIDTH: u16 = 25;


// Semantic color constants
mod colors {
    use crossterm::style::Color;

    // Calendar sources (muted so panel labels read as chrome, not content)
    pub const GOOGLE_ACCENT: Color = Color::Rgb { r: 96, g: 125, b: 168 };
    pub const ICLOUD_ACCENT: Color = Color::Rgb { r: 152, g: 115, b: 168 };

    // Event states
    pub const CURRENT_EVENT: Color = Color::Green;
    pub const NEXT_EVENT: Color = Color::Yellow;
    pub const PAST_EVENT: Color = Color::DarkGrey;
    pub const FREE_EVENT: Color = Color::DarkGrey;
    pub const SELECTED: Color = Color::Cyan;

    // UI elements
    pub const HEADER: Color = Color::Cyan;
    pub const SEPARATOR: Color = Color::DarkGrey;

    // Details panel
    pub const TITLE: Color = Color::White;
    pub const TIME: Color = Color::White;
    pub const ACTION: Color = Color::Green;

    // Overlap indicator
    pub const OVERLAP_EVENT: Color = Color::Red;

    // Week availability. Mid-tone marks that read on light and dark themes;
    // the free shade and past fading are derived from the real terminal
    // background at runtime (see free_block_color / blend_toward_bg).
    pub const BUSY_RGB: (u8, u8, u8) = (84, 113, 156);
    pub const HEATMAP_OVERLAP_RGB: (u8, u8, u8) = (156, 85, 85);
    pub const BUSY_BLOCK: Color = Color::Rgb { r: BUSY_RGB.0, g: BUSY_RGB.1, b: BUSY_RGB.2 };
    pub const HEATMAP_OVERLAP: Color = Color::Rgb { r: HEATMAP_OVERLAP_RGB.0, g: HEATMAP_OVERLAP_RGB.1, b: HEATMAP_OVERLAP_RGB.2 };

    // Status bar
    pub const LOG_TEXT: Color = Color::DarkCyan;
    pub const STATUS_MESSAGE: Color = Color::Yellow;
}

// Terminal write helpers
fn draw_section_header(out: &mut impl Write, x: u16, y: u16, label: &str, width: usize) {
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!("\u{2500} {} ", label);
    let remaining = width.saturating_sub(label.len() + 3);
    for _ in 0..remaining {
        print!("\u{2500}");
    }
    execute!(out, ResetColor).unwrap();
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

pub fn render(state: &RenderState) {
    let mut out = stdout();
    let today = Local::now().date_naive();

    // Get terminal size
    let (term_width, term_height) = terminal::size().unwrap_or((80, 24));

    // Batch the whole frame so the clear+redraw appears atomically (no flicker
    // on terminals supporting synchronized output — alacritty/ghostty/kitty/foot)
    execute!(out, terminal::BeginSynchronizedUpdate).unwrap();

    // Setup wizard takes over the whole screen
    if let Some(setup) = state.setup {
        execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
        render_setup_wizard(&mut out, setup, term_width, term_height);
        execute!(out, terminal::EndSynchronizedUpdate).unwrap();
        out.flush().unwrap();
        return;
    }

    // When search modal is active, skip redrawing underlying content to avoid flicker
    if let Some(search) = state.search {
        render_search_modal(&mut out, search, term_width, term_height);
    } else {
        // Clear and move to home position
        execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();

        // Month view handles both normal and day timeline modes
        render_month_view(&mut out, state, today, term_width, term_height);

        // Render HTTP logs if enabled
        let log_height = if state.show_logs { 8 } else { 0 };
        if state.show_logs {
            let logs = get_recent_logs(log_height as usize);
            let log_start_row = term_height.saturating_sub(2 + log_height);

            execute!(out, SetForegroundColor(colors::LOG_TEXT)).unwrap();
            for (i, log) in logs.iter().rev().enumerate() {
                let row = log_start_row + i as u16;
                if row < term_height.saturating_sub(2) {
                    execute!(out, cursor::MoveTo(0, row)).unwrap();
                    print!(" {}", truncate_str(log, term_width as usize - 2));
                }
            }
            execute!(out, ResetColor).unwrap();
        }

        // Render confirmation modal if there's a pending action
        if let Some(action) = state.pending_action {
            render_confirmation_modal(&mut out, action, term_width, term_height);
        }

        // Render help overlay on top of everything
        if state.show_help {
            render_help_modal(&mut out, term_width, term_height);
        }
    }

    // Render status bar at bottom
    let status_row = term_height.saturating_sub(2);
    execute!(out, cursor::MoveTo(0, status_row)).unwrap();

    if let Some(msg) = state.status_message {
        let color = if state.status_is_error { Color::Red } else { colors::STATUS_MESSAGE };
        execute!(out, SetForegroundColor(color)).unwrap();
        print!(" {}", truncate_str(msg, term_width as usize - 2));
        execute!(out, ResetColor).unwrap();
    } else {
        // Show countdown to next event when no status message
        let current_time = Local::now().time();
        if let Some(next_info) = find_next_event(state.events, today, current_time) {
            let title = truncate_str(&next_info.event.title, 30);
            if next_info.is_current || next_info.minutes_until <= 0 {
                execute!(out, SetForegroundColor(colors::CURRENT_EVENT)).unwrap();
                print!(" Now: {}", title);
            } else if next_info.minutes_until <= 15 {
                execute!(out, SetForegroundColor(colors::NEXT_EVENT)).unwrap();
                print!(" Next: {} in {}", title, format_duration(next_info.minutes_until));
            } else {
                // Calm default: only the event title at full brightness
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!(" Next: ");
                execute!(out, ResetColor).unwrap();
                print!("{}", title);
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!(" in {}", format_duration(next_info.minutes_until));
            }
            execute!(out, ResetColor).unwrap();
        }
    }

    // Render controls based on current mode
    execute!(out, cursor::MoveTo(0, term_height.saturating_sub(1))).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();

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
    print!("{}", controls);
    execute!(out, ResetColor).unwrap();

    execute!(out, terminal::EndSynchronizedUpdate).unwrap();
    out.flush().unwrap();
}

fn render_month_view(out: &mut impl Write, state: &RenderState, today: NaiveDate, term_width: u16, term_height: u16) {
    let now = Local::now();
    let current_time = now.time();
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
    render_calendar(out, state.current_date, state.selected_date, today, state.events, state.google_loading || state.icloud_loading, term_height);

    // Render event panels in the middle
    if events_panel_width >= MIN_PANEL_WIDTH {
        let events_x = cal_width + 1;

        // Events column header: selected date
        execute!(out, cursor::MoveTo(events_x, 0)).unwrap();
        execute!(out, SetAttribute(Attribute::Bold)).unwrap();
        print!("{}", state.selected_date.format("%a %b %d"));
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

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
            out,
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
            out,
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
        execute!(out, cursor::MoveTo(cal_width + 1, 0)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("{}", truncate_str("Too narrow for events", events_panel_width as usize));
        execute!(out, ResetColor).unwrap();
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

        render_event_details_column(out, details_x, 0, details_panel_width, details_height, selected_event);
    }

}

fn render_calendar(
    out: &mut impl Write,
    current_date: NaiveDate,
    selected_date: NaiveDate,
    today: NaiveDate,
    events: &EventCache,
    is_loading: bool,
    term_height: u16,
) {
    execute!(out, cursor::MoveTo(0, 0)).unwrap();

    // Month header
    execute!(out, SetAttribute(Attribute::Bold)).unwrap();

    let cal_width = CALENDAR_WIDTH;
    let loading_indicator = if is_loading { " *" } else { "" };
    let header = format!(
        "{} {}{}",
        current_date.format("%B"),
        current_date.year(),
        loading_indicator
    );
    print!("{}", truncate_str(&header, cal_width as usize));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Weekday header
    execute!(out, cursor::MoveTo(0, 2)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!("Mo Tu We Th Fr Sa Su");
    execute!(out, ResetColor).unwrap();

    // Calendar grid
    let first_day = current_date.with_day(1).unwrap();
    let start_weekday = first_day.weekday().num_days_from_monday();
    let days_in_month = days_in_month(current_date);
    let cols = 7;

    for row in 0..6 {
        execute!(out, cursor::MoveTo(0, 3 + row as u16)).unwrap();

        for col in 0..cols {
            let cell = row * 7 + col; // Always use 7-day weeks for calculation
            if cell < start_weekday || cell >= start_weekday + days_in_month {
                print!("   ");
            } else {
                let day = cell - start_weekday + 1;
                let date = first_day.with_day(day).unwrap();
                let is_today = date == today;
                let is_selected = date == selected_date;
                let is_weekend = col >= 5;

                if is_selected {
                    // Explicit colors: Reverse over a dark theme made the cursor nearly invisible
                    execute!(
                        out,
                        SetBackgroundColor(Color::Cyan),
                        SetForegroundColor(Color::Black)
                    )
                    .unwrap();
                } else if is_today {
                    execute!(
                        out,
                        SetForegroundColor(Color::Green),
                        SetAttribute(Attribute::Bold)
                    )
                    .unwrap();
                } else if is_weekend {
                    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                }

                print!("{:2} ", day);

                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }
        }
    }

    // Render week availability below the calendar grid
    render_week_availability(out, events, selected_date, term_height);
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
    out: &mut impl Write,
    events: &EventCache,
    selected_date: NaiveDate,
    term_height: u16,
) {
    let start_row = 10u16; // Below the calendar grid
    let monday = get_week_monday(selected_date);
    let today = Local::now().date_naive();
    let current_minutes = {
        let now = Local::now().time();
        now.hour() * 60 + now.minute()
    };
    let num_days = 7;
    let max_row = term_height.saturating_sub(2); // don't collide with the status bar

    // Header row: highlight the selected day's column (and today's)
    execute!(out, cursor::MoveTo(0, start_row)).unwrap();
    print!("   ");
    for day_offset in 0..7i64 {
        let date = monday + Duration::days(day_offset);
        let letter = ["M", "T", "W", "T", "F", "S", "S"][day_offset as usize];
        if date == selected_date {
            execute!(out, SetForegroundColor(colors::SELECTED), SetAttribute(Attribute::Bold)).unwrap();
        } else if date == today {
            execute!(out, SetForegroundColor(Color::Green)).unwrap();
        } else {
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        }
        print!(" {} ", letter);
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
    }

    // Render each hour row (8am - 7pm = 12 rows)
    // Each cell shows 30-min resolution using half-blocks
    for hour_offset in 0..12u32 {
        let hour = 8 + hour_offset;
        let row = start_row + 1 + hour_offset as u16;
        if row >= max_row {
            break;
        }

        execute!(out, cursor::MoveTo(0, row)).unwrap();

        // Hour label
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("{:2} ", hour);
        execute!(out, ResetColor).unwrap();

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
                    Color::Rgb { r: rgb.0, g: rgb.1, b: rgb.2 }
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
                        execute!(out, SetForegroundColor(top)).unwrap();
                        print!("██");
                    } else {
                        execute!(out, SetForegroundColor(top), SetBackgroundColor(bot)).unwrap();
                        print!("▀▀");
                    }
                }
                (true, false) => {
                    execute!(out, SetForegroundColor(color_for(first_half_count, first_half_past)), SetBackgroundColor(free)).unwrap();
                    print!("▀▀");
                }
                (false, true) => {
                    execute!(out, SetForegroundColor(color_for(second_half_count, second_half_past)), SetBackgroundColor(free)).unwrap();
                    print!("▄▄");
                }
                (false, false) => {
                    execute!(out, SetForegroundColor(free)).unwrap();
                    print!("██");
                }
            }
            execute!(out, ResetColor).unwrap();
            print!(" ");
        }
        execute!(out, ResetColor).unwrap();
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
            execute!(out, cursor::MoveTo(0, marker_row)).unwrap();
            execute!(out, SetForegroundColor(Color::DarkYellow)).unwrap();
            print!("   ");
            for marker in markers {
                print!(" {} ", marker);
            }
            execute!(out, ResetColor).unwrap();
        }
    }
}

/// Render event panel with title and events
fn render_event_panel(
    out: &mut impl Write,
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
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(out, SetForegroundColor(accent_color)).unwrap();
    let loading_str = if is_loading { "*" } else { "" };
    print!("{}{}", title, loading_str);
    execute!(out, ResetColor).unwrap();

    let content_start = y + 1;

    if events.is_empty() {
        execute!(out, cursor::MoveTo(x, content_start)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        if is_loading {
            print!("Loading...");
        } else {
            print!("No events");
        }
        execute!(out, ResetColor).unwrap();
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
        execute!(out, cursor::MoveTo(x, content_start + row as u16)).unwrap();

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
            execute!(out, SetForegroundColor(Color::Cyan)).unwrap();
            print!("\u{25B6}"); // Right-pointing triangle
        } else if is_current && !is_unaccepted && !is_free_event {
            execute!(out, SetForegroundColor(Color::Green)).unwrap();
            print!("\u{25CF}"); // Filled circle
        } else if is_overlapping && !is_past_day && !is_unaccepted && !is_free_event && !is_past_event {
            execute!(out, SetForegroundColor(colors::OVERLAP_EVENT)).unwrap();
            print!("!");
        } else if is_next && !is_unaccepted && !is_free_event {
            execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
            print!("\u{25CB}"); // Empty circle
        } else {
            print!(" ");
        }

        // Time (carries the status color; two-space gutter before the title)
        execute!(out, SetForegroundColor(event_color)).unwrap();
        if is_selected || ((is_current || is_next) && !is_unaccepted && !is_free_event) {
            execute!(out, SetAttribute(Attribute::Bold)).unwrap();
        }
        print!("{:>7}  ", event.time_str);
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

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
        execute!(out, SetForegroundColor(title_color)).unwrap();
        if is_selected {
            execute!(out, SetAttribute(Attribute::Bold)).unwrap();
        }
        let title_width = width.saturating_sub(11) as usize;
        print!("{}", truncate_str(&event.title, title_width));
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
    }

    // Clipped-events indicator on the reserved last row
    if total > visible {
        let below = total - (start + visible);
        execute!(out, cursor::MoveTo(x, content_start + visible as u16)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        let indicator = match (start > 0, below > 0) {
            (true, true) => format!(" \u{2026} {} above \u{00B7} {} more", start, below),
            (true, false) => format!(" \u{2026} {} above", start),
            _ => format!(" \u{2026} +{} more", below),
        };
        print!("{}", truncate_str(&indicator, width as usize));
        execute!(out, ResetColor).unwrap();
    }
}

/// Render event details in a column
fn render_event_details_column(
    out: &mut impl Write,
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
        execute!(out, cursor::MoveTo(content_x, y)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("No event selected");
        execute!(out, ResetColor).unwrap();
        return;
    };

    let mut current_row = y;

    // Title doubles as the panel header
    execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
    execute!(out, SetForegroundColor(colors::TITLE), SetAttribute(Attribute::Bold)).unwrap();
    print!("{}", truncate_str(&event.title, content_width));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
    current_row += 1;

    // Time, with the calendar source as a dim suffix
    execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
    let time_text = match event.end_time_str {
        Some(ref end) => format!("{} \u{2013} {}", event.time_str, end),
        None => event.time_str.clone(),
    };
    execute!(out, SetForegroundColor(colors::TIME)).unwrap();
    print!("{}", truncate_str(&time_text, content_width));
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
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("{}", truncate_str(&source_text, remaining));
    }
    execute!(out, ResetColor).unwrap();
    current_row += 1;

    // Location
    if let Some(ref loc) = event.location
        && !loc.is_empty() && current_row < max_row {
            execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!("{}", truncate_str(loc, content_width));
            execute!(out, ResetColor).unwrap();
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

        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("{}", truncate_str(&actions.join("  "), content_width));
        execute!(out, ResetColor).unwrap();
        current_row += 1;
    }

    // Participants
    current_row += 1; // blank line before participants
    if !event.attendees.is_empty() && current_row < max_row {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("Participants");
        execute!(out, ResetColor).unwrap();
        current_row += 1;

        let total = event.attendees.len();
        for (idx, attendee) in event.attendees.iter().enumerate() {
            if current_row >= max_row {
                break;
            }
            // On the last available row, summarize the rest instead of showing one more name
            let remaining = total - idx;
            if current_row == max_row - 1 && remaining > 1 {
                execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!("  \u{2026} +{} more", remaining);
                execute!(out, ResetColor).unwrap();
                break;
            }

            execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();

            // Status icon
            execute!(out, SetForegroundColor(attendee.status.color())).unwrap();
            print!("  {} ", attendee.status.icon());
            execute!(out, ResetColor).unwrap();

            // Name or email
            let display_name = attendee.name.as_ref().unwrap_or(&attendee.email);
            let status_str = match attendee.status {
                AttendeeStatus::Organizer => " (org)",
                _ => "",
            };
            let name_width = content_width.saturating_sub(5 + status_str.len());
            print!("{}", truncate_str(display_name, name_width));
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!("{}", status_str);
            execute!(out, ResetColor).unwrap();
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
fn render_setup_wizard(out: &mut impl Write, setup: &SetupState, term_width: u16, term_height: u16) {
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

        execute!(out, cursor::MoveTo(base_x, row)).unwrap();

        // Check if this is the input line placeholder
        if !input_rendered {
            if let Some(ref il) = input_line {
                if text.is_empty() && matches!(style, Style::Normal) && i > 0 {
                    let prev = &lines[i - 1];
                    if matches!(prev.1, Style::Normal) && (prev.0.contains("Paste") || prev.0.contains("Enter")) {
                        execute!(out, SetForegroundColor(Color::White)).unwrap();
                        let display = truncate_str(il, max_content_width as usize);
                        print!("{}", display);
                        execute!(out, ResetColor).unwrap();
                        input_rendered = true;
                        continue;
                    }
                }
            }
        }

        // Check if this is the error placeholder
        if matches!(style, Style::Error) {
            if let Some(ref err) = setup.error {
                execute!(out, SetForegroundColor(Color::Red)).unwrap();
                print!("{}", err);
                execute!(out, ResetColor).unwrap();
                continue;
            }
        }

        match style {
            Style::Header => {
                execute!(out, SetForegroundColor(colors::HEADER), SetAttribute(Attribute::Bold)).unwrap();
                print!("{}", text);
                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }
            Style::Accent => {
                execute!(out, SetForegroundColor(Color::Green)).unwrap();
                print!("{}", text);
                execute!(out, ResetColor).unwrap();
            }
            Style::Dim => {
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!("{}", text);
                execute!(out, ResetColor).unwrap();
            }
            Style::Error => {} // handled above
            Style::Normal => {
                print!("{}", text);
            }
        }
    }
}

/// Render a centered search modal
fn render_search_modal(out: &mut impl Write, search: &SearchState, term_width: u16, term_height: u16) {
    use crate::app::EventSource;
    use crate::cache::EventId;

    let modal_width = 60u16.min(term_width.saturating_sub(4));
    let modal_height = (term_height * 3 / 4).max(10).min(term_height.saturating_sub(4));
    let start_x = (term_width.saturating_sub(modal_width)) / 2;
    let start_y = (term_height.saturating_sub(modal_height)) / 2;

    execute!(out, SetForegroundColor(colors::HEADER)).unwrap();

    // Top border with title
    execute!(out, cursor::MoveTo(start_x, start_y)).unwrap();
    print!("┌─ Search ");
    let remaining_top = modal_width.saturating_sub(11);
    for _ in 0..remaining_top {
        print!("─");
    }
    print!("┐");

    // Empty rows
    for row in 1..modal_height - 1 {
        execute!(out, cursor::MoveTo(start_x, start_y + row)).unwrap();
        print!("│");
        for _ in 0..modal_width - 2 {
            print!(" ");
        }
        print!("│");
    }

    // Bottom border
    execute!(out, cursor::MoveTo(start_x, start_y + modal_height - 1)).unwrap();
    print!("└");
    for _ in 0..modal_width - 2 {
        print!("─");
    }
    print!("┘");

    execute!(out, ResetColor).unwrap();

    // Input field
    let content_x = start_x + 2;
    let content_width = (modal_width - 4) as usize;
    execute!(out, cursor::MoveTo(content_x, start_y + 1)).unwrap();
    execute!(out, SetForegroundColor(Color::White), SetAttribute(Attribute::Bold)).unwrap();
    let query_display = truncate_str(&search.query, content_width.saturating_sub(3));
    print!("> {}_ ", query_display);
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Separator
    execute!(out, cursor::MoveTo(content_x, start_y + 2)).unwrap();
    execute!(out, SetForegroundColor(colors::SEPARATOR)).unwrap();
    for _ in 0..content_width {
        print!("─");
    }
    execute!(out, ResetColor).unwrap();

    // Results area
    let results_start_y = start_y + 3;
    let results_height = (modal_height - 5) as usize; // 3 top (border+input+sep) + 2 bottom (hint+border)

    if search.query.is_empty() {
        execute!(out, cursor::MoveTo(content_x, results_start_y)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("Type to search events...");
        execute!(out, ResetColor).unwrap();
    } else if search.results.is_empty() {
        execute!(out, cursor::MoveTo(content_x, results_start_y)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("No matching events");
        execute!(out, ResetColor).unwrap();
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

        let today = Local::now().date_naive();
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
                    draw_section_header(out, content_x, screen_row, label, content_width);
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
                execute!(out, cursor::MoveTo(content_x, row)).unwrap();

                // Selection indicator
                if is_selected {
                    execute!(out, SetForegroundColor(colors::SELECTED)).unwrap();
                    print!("▶ ");
                } else {
                    print!("  ");
                }

                // Smart when column
                let when = format_smart_when(result.event.date, &result.event.time_str, today);
                execute!(out, SetForegroundColor(if is_selected { colors::SELECTED } else { Color::DarkGrey })).unwrap();
                print!("{:>11} ", when);

                // Source color indicator
                let source_color = match result.source {
                    EventSource::Google => colors::GOOGLE_ACCENT,
                    EventSource::ICloud => colors::ICLOUD_ACCENT,
                };
                execute!(out, SetForegroundColor(source_color)).unwrap();
                let source_char = match result.event.id {
                    EventId::Google { .. } => "G",
                    EventId::ICloud { .. } => "I",
                };
                print!("{} ", source_char);

                // Title
                let title_space = content_width.saturating_sub(2 + 12 + 2);
                execute!(out, SetForegroundColor(if is_selected { colors::SELECTED } else { Color::White })).unwrap();
                if is_selected {
                    execute!(out, SetAttribute(Attribute::Bold)).unwrap();
                }
                print!("{}", truncate_str(&result.event.title, title_space));
                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }

            result_idx += 1;
            visual_row += 1;
        }
    }

    // Bottom hint
    let hint_y = start_y + modal_height - 2;
    execute!(out, cursor::MoveTo(content_x, hint_y)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    let count_str = if search.results.is_empty() {
        String::new()
    } else {
        format!("{}/{} ", search.selected_index + 1, search.results.len())
    };
    print!("{}\u{2191}\u{2193}:navigate Enter:select Esc:close", count_str);
    execute!(out, ResetColor).unwrap();
}

/// Render the help overlay listing all keybindings and the availability legend
fn render_help_modal(out: &mut impl Write, term_width: u16, term_height: u16) {
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
        Item("J", "join meeting"),
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
    execute!(out, SetForegroundColor(colors::HEADER)).unwrap();
    execute!(out, cursor::MoveTo(start_x, start_y)).unwrap();
    print!("┌─ Help ");
    for _ in 0..modal_width.saturating_sub(9) {
        print!("─");
    }
    print!("┐");
    for row in 1..modal_height - 1 {
        execute!(out, cursor::MoveTo(start_x, start_y + row)).unwrap();
        print!("│");
        for _ in 0..modal_width - 2 {
            print!(" ");
        }
        print!("│");
    }
    execute!(out, cursor::MoveTo(start_x, start_y + modal_height - 1)).unwrap();
    print!("└");
    for _ in 0..modal_width - 2 {
        print!("─");
    }
    print!("┘");
    execute!(out, ResetColor).unwrap();

    let content_x = start_x + 2;
    let max_row = start_y + modal_height - 1;
    let mut row = start_y + 1;
    for line in &lines {
        if row >= max_row {
            break;
        }
        execute!(out, cursor::MoveTo(content_x, row)).unwrap();
        match line {
            Section(title) => {
                execute!(out, SetForegroundColor(colors::HEADER), SetAttribute(Attribute::Bold)).unwrap();
                print!("{}", title);
                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }
            Item(keys, desc) => {
                execute!(out, SetForegroundColor(Color::White)).unwrap();
                print!("{:>12}", keys);
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!("  {}", desc);
                execute!(out, ResetColor).unwrap();
            }
            Legend => {
                execute!(out, SetForegroundColor(colors::BUSY_BLOCK)).unwrap();
                print!("{:>12}", "██");
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!(" busy  ");
                execute!(out, SetForegroundColor(colors::HEATMAP_OVERLAP)).unwrap();
                print!("██");
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!(" double-booked  ");
                execute!(out, SetForegroundColor(free_block_color())).unwrap();
                print!("██");
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!(" free");
                execute!(out, ResetColor).unwrap();
            }
            Note(text) => {
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                print!("{}", text);
                execute!(out, ResetColor).unwrap();
            }
        }
        row += 1;
    }
}

/// Render a centered confirmation modal
fn render_confirmation_modal(out: &mut impl Write, action: &PendingAction, term_width: u16, term_height: u16) {
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
    execute!(out, SetForegroundColor(colors::HEADER)).unwrap();

    // Top border
    execute!(out, cursor::MoveTo(start_x, start_y)).unwrap();
    print!("┌");
    for _ in 0..modal_width - 2 {
        print!("─");
    }
    print!("┐");

    // Middle rows
    for row in 1..modal_height - 1 {
        execute!(out, cursor::MoveTo(start_x, start_y + row)).unwrap();
        print!("│");
        for _ in 0..modal_width - 2 {
            print!(" ");
        }
        print!("│");
    }

    // Bottom border
    execute!(out, cursor::MoveTo(start_x, start_y + modal_height - 1)).unwrap();
    print!("└");
    for _ in 0..modal_width - 2 {
        print!("─");
    }
    print!("┘");

    // Title
    execute!(out, cursor::MoveTo(start_x + 2, start_y + 1)).unwrap();
    execute!(out, SetForegroundColor(colors::NEXT_EVENT), SetAttribute(Attribute::Bold)).unwrap();
    print!("{}", prompt);
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Options
    execute!(out, cursor::MoveTo(start_x + 2, start_y + 3)).unwrap();
    execute!(out, SetForegroundColor(colors::ACTION)).unwrap();
    print!("[y/Enter]");
    execute!(out, SetForegroundColor(Color::White)).unwrap();
    print!(" Yes  ");
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!("[n/Esc]");
    execute!(out, SetForegroundColor(Color::White)).unwrap();
    print!(" No");
    execute!(out, ResetColor).unwrap();
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
            id: EventId::ICloud { calendar_url: "test".to_string(), event_uid: "test-uid".to_string(), etag: None, calendar_name: None },
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
}
