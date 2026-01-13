use crate::app::{EventSource, NavigationMode, PendingAction};
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
use std::io::{stdout, Write};
use std::sync::Mutex;

const CALENDAR_WIDTH_WITH_WEEKENDS: u16 = 23;
const CALENDAR_WIDTH_NO_WEEKENDS: u16 = 19;
const MIN_PANEL_WIDTH: u16 = 25;

fn calendar_width(show_weekends: bool) -> u16 {
    if show_weekends { CALENDAR_WIDTH_WITH_WEEKENDS } else { CALENDAR_WIDTH_NO_WEEKENDS }
}

// Track previous render state to avoid unnecessary clearing
#[derive(Default)]
struct PrevRenderState {
    selected_date: Option<NaiveDate>,
    selected_source: Option<EventSource>,
    selected_event_index: Option<usize>,
    navigation_mode: Option<NavigationMode>,
}

static PREV_STATE: Mutex<PrevRenderState> = Mutex::new(PrevRenderState {
    selected_date: None,
    selected_source: None,
    selected_event_index: None,
    navigation_mode: None,
});

// Semantic color constants
mod colors {
    use crossterm::style::Color;

    // Calendar sources
    pub const GOOGLE_ACCENT: Color = Color::Blue;
    pub const ICLOUD_ACCENT: Color = Color::Magenta;

    // Event states
    pub const CURRENT_EVENT: Color = Color::Green;
    pub const NEXT_EVENT: Color = Color::Yellow;
    pub const PAST_EVENT: Color = Color::DarkGrey;
    pub const SELECTED: Color = Color::Cyan;

    // UI elements
    pub const HEADER: Color = Color::Cyan;
    pub const SEPARATOR: Color = Color::DarkGrey;

    // Details panel
    pub const TITLE: Color = Color::White;
    pub const TIME: Color = Color::White;
    pub const LOCATION: Color = Color::Yellow;
    pub const ACTION: Color = Color::Green;

    // Week availability
    pub const BUSY_BLOCK: Color = Color::Blue;
    pub const FREE_BLOCK: Color = Color::Rgb { r: 200, g: 200, b: 200 };

    // Status bar
    pub const LOG_TEXT: Color = Color::DarkCyan;
    pub const STATUS_MESSAGE: Color = Color::Yellow;
}

// Terminal write helpers
fn draw_separator(out: &mut impl Write, x: u16, y: u16, width: u16) {
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(out, SetForegroundColor(colors::SEPARATOR)).unwrap();
    for _ in 0..width.min(40) {
        print!("\u{2500}");
    }
    execute!(out, ResetColor).unwrap();
}

pub struct RenderState<'a> {
    pub current_date: NaiveDate,
    pub selected_date: NaiveDate,
    pub show_logs: bool,
    pub show_weekends: bool,
    pub events: &'a EventCache,
    pub google_auth: &'a GoogleAuthState,
    pub icloud_auth: &'a ICloudAuthState,
    pub status_message: Option<&'a str>,
    pub google_loading: bool,
    pub icloud_loading: bool,
    // Two-level navigation state
    pub navigation_mode: NavigationMode,
    pub selected_source: EventSource,
    pub selected_event_index: usize,
    // Confirmation state
    pub pending_action: Option<&'a PendingAction>,
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

/// Format the countdown string for display
fn format_countdown(info: &NextEventInfo, max_title_len: usize) -> String {
    let title = truncate_str(&info.event.title, max_title_len);

    if info.is_current || info.minutes_until <= 0 {
        format!("Now: {}", title)
    } else if info.minutes_until < 60 {
        format!("Next: {} in {}m", title, info.minutes_until)
    } else if info.minutes_until < 24 * 60 {
        let hours = info.minutes_until / 60;
        let mins = info.minutes_until % 60;
        if mins > 0 {
            format!("Next: {} in {}h {}m", title, hours, mins)
        } else {
            format!("Next: {} in {}h", title, hours)
        }
    } else {
        let days = info.minutes_until / (24 * 60);
        let hours = (info.minutes_until % (24 * 60)) / 60;
        if hours > 0 {
            format!("Next: {} in {}d {}h", title, days, hours)
        } else {
            format!("Next: {} in {}d", title, days)
        }
    }
}

pub fn render(state: &RenderState) {
    let mut out = stdout();
    let today = Local::now().date_naive();

    // Get terminal size
    let (term_width, term_height) = terminal::size().unwrap_or((80, 24));

    // Move to home position instead of clearing (alternate screen handles buffer)
    execute!(out, cursor::MoveTo(0, 0)).unwrap();

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

    // Render status bar at bottom
    let status_row = term_height.saturating_sub(2);
    execute!(out, cursor::MoveTo(0, status_row)).unwrap();

    if let Some(msg) = state.status_message {
        execute!(out, SetForegroundColor(colors::STATUS_MESSAGE)).unwrap();
        print!(" {}", truncate_str(msg, term_width as usize - 2));
        execute!(out, ResetColor).unwrap();
    } else {
        // Show countdown to next event when no status message
        let current_time = Local::now().time();
        if let Some(next_info) = find_next_event(state.events, today, current_time) {
            let countdown = format_countdown(&next_info, 30);
            if next_info.is_current {
                execute!(out, SetForegroundColor(colors::CURRENT_EVENT)).unwrap();
            } else if next_info.minutes_until <= 15 {
                execute!(out, SetForegroundColor(colors::NEXT_EVENT)).unwrap();
            } else {
                execute!(out, SetForegroundColor(Color::White)).unwrap();
            }
            print!(" {}", countdown);
            execute!(out, ResetColor).unwrap();
        }
    }

    // Render controls based on current mode
    execute!(out, cursor::MoveTo(0, term_height.saturating_sub(1))).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();

    let controls = if state.pending_action.is_some() {
        // Confirmation mode controls
        " y/Enter:confirm n/Esc:cancel".to_string()
    } else if state.navigation_mode == NavigationMode::Event {
        // Event navigation mode controls
        " jk:nav ^d/^u:scroll n:now t:today r:refresh Esc:back q:quit".to_string()
    } else {
        // Day navigation mode controls
        let mut c = String::from(" jk:day ^d/^u:month n:now t:today r:refresh Enter:events");
        if !state.google_auth.is_authenticated() {
            c.push_str(" g:work");
        }
        if !state.icloud_auth.is_authenticated() {
            c.push_str(" i:personal");
        }
        c.push_str(" q:quit");
        c
    };
    print!("{}", controls);
    execute!(out, ResetColor).unwrap();

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

    let cal_width = calendar_width(state.show_weekends);

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
    render_calendar(out, state.current_date, state.selected_date, today, state.events, state.google_loading || state.icloud_loading, state.show_weekends);

    // Check if we need to clear (only when state changes)
    let needs_clear = {
        let prev = PREV_STATE.lock().unwrap();
        prev.selected_date != Some(state.selected_date)
            || prev.selected_source != Some(state.selected_source)
            || prev.selected_event_index != Some(state.selected_event_index)
            || prev.navigation_mode != Some(state.navigation_mode)
    };

    // Render event panels in the middle
    if events_panel_width >= MIN_PANEL_WIDTH {
        let events_x = cal_width + 1;

        // Clear the events panel area only when content changes
        if needs_clear {
            for row in 0..term_height.saturating_sub(2) {
                execute!(out, cursor::MoveTo(events_x, row), Clear(ClearType::UntilNewLine)).unwrap();
            }
        }

        // Events column header: selected date
        execute!(out, cursor::MoveTo(events_x, 0)).unwrap();
        execute!(out, SetForegroundColor(colors::HEADER), SetAttribute(Attribute::Bold)).unwrap();
        print!("{}", state.selected_date.format("%a %b %d"));
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

        // Separator line
        draw_separator(out, events_x, 1, events_panel_width);

        let google_events = state.events.google.get(state.selected_date);
        let icloud_events = state.events.icloud.get(state.selected_date);
        let is_past_day = state.selected_date < today;

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
        );

        // Calculate Personal panel position: after Work header (1) + events + spacing (1)
        let work_panel_rows = 1 + google_events.len().max(1) as u16;
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
        );
    }

    // Render details panel on the right when in Event mode
    if in_event_mode && details_panel_width >= MIN_PANEL_WIDTH {
        let details_x = cal_width + events_panel_width + 2;
        let details_height = term_height.saturating_sub(3);

        // Clear the details panel area only when content changes
        if needs_clear {
            for row in 0..term_height.saturating_sub(2) {
                execute!(out, cursor::MoveTo(details_x, row), Clear(ClearType::UntilNewLine)).unwrap();
            }
        }

        // Get the selected event
        let selected_event = match state.selected_source {
            EventSource::Google => state.events.google.get(state.selected_date).get(state.selected_event_index),
            EventSource::ICloud => state.events.icloud.get(state.selected_date).get(state.selected_event_index),
        };

        render_event_details_column(out, details_x, 0, details_panel_width, details_height, selected_event);
    }

    // Update previous state
    {
        let mut prev = PREV_STATE.lock().unwrap();
        prev.selected_date = Some(state.selected_date);
        prev.selected_source = Some(state.selected_source);
        prev.selected_event_index = Some(state.selected_event_index);
        prev.navigation_mode = Some(state.navigation_mode);
    }
}

fn render_calendar(
    out: &mut impl Write,
    current_date: NaiveDate,
    selected_date: NaiveDate,
    today: NaiveDate,
    events: &EventCache,
    is_loading: bool,
    show_weekends: bool,
) {
    execute!(out, cursor::MoveTo(0, 0)).unwrap();

    // Month header
    execute!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold)
    )
    .unwrap();

    let cal_width = calendar_width(show_weekends);
    let loading_indicator = if is_loading { " *" } else { "" };
    let header = format!(
        "{} {}{}",
        current_date.format("%B").to_string().to_uppercase(),
        current_date.year(),
        loading_indicator
    );
    print!("{}", truncate_str(&header, cal_width as usize));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Separator line
    draw_separator(out, 0, 1, cal_width - 1);

    // Weekday header
    execute!(out, cursor::MoveTo(0, 2)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    if show_weekends {
        print!("Mo Tu We Th Fr Sa Su");
    } else {
        print!("Mo Tu We Th Fr");
    }
    execute!(out, ResetColor).unwrap();

    // Calendar grid
    let first_day = current_date.with_day(1).unwrap();
    let start_weekday = first_day.weekday().num_days_from_monday();
    let days_in_month = days_in_month(current_date);
    let cols = if show_weekends { 7 } else { 5 };

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
                let has_events = events.has_events(date);

                if is_selected {
                    execute!(
                        out,
                        SetForegroundColor(Color::Black),
                        SetAttribute(Attribute::Reverse)
                    )
                    .unwrap();
                } else if is_today {
                    execute!(
                        out,
                        SetForegroundColor(Color::Green),
                        SetAttribute(Attribute::Bold)
                    )
                    .unwrap();
                } else if is_weekend && show_weekends {
                    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                }

                if has_events && !is_selected {
                    print!("{:2}\u{2022}", day);
                } else {
                    print!("{:2} ", day);
                }

                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }
        }
    }

    // Render week availability below the calendar grid
    render_week_availability(out, events, selected_date, show_weekends);
}

/// Check if a given 30-minute slot is busy
/// slot_start is minutes from midnight (e.g., 8*60 = 480 for 8:00am)
fn is_slot_busy(events: &[DisplayEvent], slot_start: u32, slot_end: u32) -> bool {
    for event in events {
        // Skip all-day events - they don't block specific hours
        if event.time_str == "All day" {
            continue;
        }

        // Parse start time
        if let Some(start_time) = parse_event_time(&event.time_str) {
            let event_start = start_time.hour() * 60 + start_time.minute();

            // Parse end time if available
            let event_end = if let Some(ref end_str) = event.end_time_str {
                if end_str == "All day" {
                    continue;
                }
                parse_event_time(end_str).map(|t| {
                    let mins = t.hour() * 60 + t.minute();
                    // Midnight means end of day
                    if mins == 0 { 24 * 60 } else { mins }
                }).unwrap_or(event_start + 60)
            } else {
                event_start + 60 // Assume 1 hour duration if no end time
            };

            // Check if the slot overlaps with this event
            if slot_start < event_end && slot_end > event_start {
                return true;
            }
        }
    }
    false
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
    show_weekends: bool,
) {
    let start_row = 10u16; // Below the calendar grid
    let monday = get_week_monday(selected_date);
    let num_days = if show_weekends { 7 } else { 5 };

    // Header row
    execute!(out, cursor::MoveTo(0, start_row)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    if show_weekends {
        print!("    M  T  W  T  F  S  S");
    } else {
        print!("    M  T  W  T  F");
    }
    execute!(out, ResetColor).unwrap();

    // Render each hour row (8am - 7pm = 12 rows)
    // Each cell shows 30-min resolution using half-blocks
    for hour_offset in 0..12u32 {
        let hour = 8 + hour_offset;
        let row = start_row + 1 + hour_offset as u16;

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

            let first_half_busy = is_slot_busy(google_events, slot1_start, slot1_end)
                || is_slot_busy(icloud_events, slot1_start, slot1_end);
            let second_half_busy = is_slot_busy(google_events, slot2_start, slot2_end)
                || is_slot_busy(icloud_events, slot2_start, slot2_end);

            // Vertical half-blocks: top = first 30 min, bottom = second 30 min
            // ▀ draws top with fg, bottom with bg
            match (first_half_busy, second_half_busy) {
                (true, true) => {
                    execute!(out, SetForegroundColor(colors::BUSY_BLOCK)).unwrap();
                    print!("██");
                }
                (true, false) => {
                    execute!(out, SetForegroundColor(colors::BUSY_BLOCK), SetBackgroundColor(colors::FREE_BLOCK)).unwrap();
                    print!("▀▀");
                }
                (false, true) => {
                    execute!(out, SetForegroundColor(colors::FREE_BLOCK), SetBackgroundColor(colors::BUSY_BLOCK)).unwrap();
                    print!("▀▀");
                }
                (false, false) => {
                    execute!(out, SetForegroundColor(colors::FREE_BLOCK)).unwrap();
                    print!("██");
                }
            }
            execute!(out, ResetColor).unwrap();
            print!(" ");
        }
        execute!(out, ResetColor).unwrap();
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
) {
    // Panel header: ─ Title ─────────
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!("\u{2500} ");
    execute!(out, SetForegroundColor(accent_color)).unwrap();
    let loading_str = if is_loading { "*" } else { "" };
    print!("{}{}", title, loading_str);
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!(" ");
    let remaining = width.saturating_sub(title.len() as u16 + 4 + loading_str.len() as u16);
    for _ in 0..remaining.min(40) {
        print!("\u{2500}");
    }
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

    for (i, event) in events.iter().enumerate() {
        execute!(out, cursor::MoveTo(x, content_start + i as u16)).unwrap();

        let is_selected = selected_index == Some(i);
        let is_current = current_event_idx == Some(i);
        let is_next = next_event_idx == Some(i);
        let is_past_event = is_today && is_event_past(event, current_time) && !is_current;
        let is_unaccepted = !event.accepted;

        // Choose color based on event status
        // Gray out: past days, past events today, or unaccepted
        let event_color = if is_selected {
            colors::SELECTED
        } else if is_past_day || is_unaccepted || is_past_event {
            colors::PAST_EVENT
        } else if is_current {
            colors::CURRENT_EVENT
        } else if is_next {
            colors::NEXT_EVENT
        } else {
            Color::Reset
        };

        // Selection indicator
        if is_selected {
            execute!(out, SetForegroundColor(Color::Cyan)).unwrap();
            print!("\u{25B6}"); // Right-pointing triangle
        } else if is_current && !is_unaccepted {
            execute!(out, SetForegroundColor(Color::Green)).unwrap();
            print!("\u{25CF}"); // Filled circle
        } else if is_next && !is_unaccepted {
            execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
            print!("\u{25CB}"); // Empty circle
        } else {
            print!(" ");
        }

        // Time
        execute!(out, SetForegroundColor(event_color)).unwrap();
        if is_selected || ((is_current || is_next) && !is_unaccepted) {
            execute!(out, SetAttribute(Attribute::Bold)).unwrap();
        }
        print!("{:>7} ", event.time_str);
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

        // Title
        execute!(out, SetForegroundColor(event_color)).unwrap();
        if is_selected || ((is_current || is_next) && !is_unaccepted) {
            execute!(out, SetAttribute(Attribute::Bold)).unwrap();
        }
        let title_width = width.saturating_sub(10) as usize;
        print!("{}", truncate_str(&event.title, title_width));
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
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
    // Header
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(out, SetForegroundColor(colors::HEADER), SetAttribute(Attribute::Bold)).unwrap();
    print!("Details");
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Separator line
    draw_separator(out, x, y + 1, width);

    let content_x = x;
    let content_width = width as usize;
    let mut current_row = y + 2;

    let Some(event) = event else {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("No event selected");
        execute!(out, ResetColor).unwrap();
        return;
    };

    // Title
    execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
    execute!(out, SetForegroundColor(colors::TITLE), SetAttribute(Attribute::Bold)).unwrap();
    print!("{}", truncate_str(&event.title, content_width));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
    current_row += 1;

    // Time
    execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
    execute!(out, SetForegroundColor(colors::TIME)).unwrap();
    if let Some(ref end) = event.end_time_str {
        print!("\u{1F552} {} - {}", event.time_str, end);
    } else {
        print!("\u{1F552} {}", event.time_str);
    }
    execute!(out, ResetColor).unwrap();
    current_row += 1;

    // Location
    if let Some(ref loc) = event.location
        && !loc.is_empty() && current_row < y + height - 3 {
            execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
            execute!(out, SetForegroundColor(colors::LOCATION)).unwrap();
            print!("\u{1F4CD} {}", truncate_str(loc, content_width.saturating_sub(3)));
            execute!(out, ResetColor).unwrap();
            current_row += 1;
        }

    // Calendar source
    if current_row < y + height - 3 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        match &event.id {
            EventId::Google { calendar_name, .. } => {
                if let Some(name) = calendar_name {
                    print!("Google - {}", name);
                } else {
                    print!("Google");
                }
            }
            EventId::ICloud { calendar_name, .. } => {
                if let Some(name) = calendar_name {
                    print!("iCloud - {}", name);
                } else {
                    print!("iCloud");
                }
            }
        }
        execute!(out, ResetColor).unwrap();
        current_row += 1;
    }

    // Actions section
    current_row += 1; // Blank line before actions

    // Meeting link
    if event.meeting_url.is_some() && current_row < y + height - 3 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(colors::ACTION)).unwrap();
        print!("[J] Join");
        execute!(out, ResetColor).unwrap();
        current_row += 1;
    }

    // Accept/Decline (Google events only)
    if matches!(event.id, EventId::Google { .. }) && current_row < y + height - 3 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        if event.accepted {
            print!("[d] Decline");
        } else {
            print!("[a] Accept");
        }
        execute!(out, ResetColor).unwrap();
        current_row += 1;
    }

    // Delete
    if current_row < y + height - 3 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("[x] Delete");
        execute!(out, ResetColor).unwrap();
        current_row += 1;
    }

    // Separator
    if current_row < y + height - 2 {
        current_row += 1;
    }

    // Participants
    if !event.attendees.is_empty() && current_row < y + height - 2 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::White), SetAttribute(Attribute::Bold)).unwrap();
        print!("Participants:");
        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
        current_row += 1;

        let max_row = y + height - 1;
        for attendee in &event.attendees {
            if current_row >= max_row {
                execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
                execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                let remaining = event.attendees.len() - (current_row - y - 7) as usize;
                if remaining > 0 {
                    print!("  ... +{} more", remaining);
                }
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
            print!("{}{}", truncate_str(display_name, name_width), status_str);
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
            if event_time <= current_time {
                // This event has started - it's the current candidate
                current_idx = Some(i);
            } else if next_idx.is_none() {
                // First event that hasn't started yet
                next_idx = Some(i);
                break; // No need to continue
            }
        }
    }

    (current_idx, next_idx)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}…", truncated)
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
}
