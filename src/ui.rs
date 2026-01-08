use crate::cache::{DisplayEvent, EventCache};
use crate::{GoogleAuthState, ICloudAuthState};
use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use crossterm::{
    cursor,
    execute,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{stdout, Write};

const CALENDAR_WIDTH: u16 = 30;
const MIN_PANEL_WIDTH: u16 = 25;

pub struct RenderState<'a> {
    pub current_date: NaiveDate,
    pub selected_date: NaiveDate,
    pub events: &'a EventCache,
    pub google_auth: &'a GoogleAuthState,
    pub icloud_auth: &'a ICloudAuthState,
    pub status_message: Option<&'a str>,
    pub google_loading: bool,
    pub icloud_loading: bool,
}

pub fn render(state: &RenderState) {
    let mut out = stdout();
    let today = Local::now().date_naive();

    // Get terminal size
    let (term_width, term_height) = terminal::size().unwrap_or((80, 24));
    let right_panel_width = term_width.saturating_sub(CALENDAR_WIDTH + 1);
    let panel_height = term_height.saturating_sub(3) / 2; // Split right side in two

    execute!(out, Clear(ClearType::All), cursor::Hide).unwrap();

    // Render calendar on left
    render_calendar(&mut out, state.current_date, state.selected_date, today, state.events, state.google_loading || state.icloud_loading);

    // Render Work (Google) panel on right top
    if right_panel_width >= MIN_PANEL_WIDTH {
        let now = Local::now();
        let current_time = now.time();
        let is_today = state.selected_date == today;

        render_event_panel(
            &mut out,
            CALENDAR_WIDTH + 1,
            0,
            right_panel_width,
            panel_height,
            "Work (Google)",
            state.events.google.get(state.selected_date),
            state.google_auth,
            state.google_loading,
            Color::Blue,
            is_today,
            current_time,
        );

        // Render Personal (iCloud) panel on right bottom
        render_event_panel(
            &mut out,
            CALENDAR_WIDTH + 1,
            panel_height + 1,
            right_panel_width,
            panel_height,
            "Personal (iCloud)",
            state.events.icloud.get(state.selected_date),
            state.icloud_auth,
            state.icloud_loading,
            Color::Magenta,
            is_today,
            current_time,
        );
    }

    // Render status bar at bottom
    let status_row = term_height.saturating_sub(2);
    execute!(out, cursor::MoveTo(0, status_row)).unwrap();

    if let Some(msg) = state.status_message {
        execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
        print!(" {}", truncate_str(msg, term_width as usize - 2));
        execute!(out, ResetColor).unwrap();
    }

    // Render controls - only show g/i if not authenticated
    execute!(out, cursor::MoveTo(0, term_height.saturating_sub(1))).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    let mut controls = String::from(" hjkl:nav t:today r:refresh");
    if !state.google_auth.is_authenticated() {
        controls.push_str(" g:work");
    }
    if !state.icloud_auth.is_authenticated() {
        controls.push_str(" i:personal");
    }
    controls.push_str(" q:quit");
    print!("{}", controls);
    execute!(out, ResetColor).unwrap();

    out.flush().unwrap();
}

fn render_calendar(
    out: &mut impl Write,
    current_date: NaiveDate,
    selected_date: NaiveDate,
    today: NaiveDate,
    events: &EventCache,
    is_loading: bool,
) {
    execute!(out, cursor::MoveTo(0, 0)).unwrap();

    // Month header
    let first_day = current_date.with_day(1).unwrap();
    execute!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold)
    )
    .unwrap();

    let loading_indicator = if is_loading { " *" } else { "" };
    let header = format!(
        " {} {}{}",
        current_date.format("%B").to_string().to_uppercase(),
        current_date.year(),
        loading_indicator
    );
    print!("{}", truncate_str(&header, CALENDAR_WIDTH as usize));
    execute!(out, ResetColor).unwrap();

    // Weekday header
    execute!(out, cursor::MoveTo(0, 1)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!(" Mo Tu We Th Fr Sa Su");
    execute!(out, ResetColor).unwrap();

    // Calendar grid
    let start_weekday = first_day.weekday().num_days_from_monday();
    let days_in_month = days_in_month(current_date);

    for row in 0..6 {
        execute!(out, cursor::MoveTo(0, 2 + row as u16)).unwrap();
        print!(" ");

        for col in 0..7 {
            let cell = row * 7 + col;
            if cell < start_weekday || cell >= start_weekday + days_in_month {
                print!("   ");
            } else {
                let day = (cell - start_weekday + 1) as u32;
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
                } else if is_weekend {
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

    // Selected date info
    execute!(out, cursor::MoveTo(0, 9)).unwrap();
    execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
    print!(
        " {} {}",
        selected_date.format("%a"),
        selected_date.format("%b %d")
    );
    execute!(out, ResetColor).unwrap();
}

fn render_event_panel<A: AuthDisplay>(
    out: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    title: &str,
    events: &[DisplayEvent],
    auth_state: &A,
    is_loading: bool,
    accent_color: Color,
    is_today: bool,
    current_time: NaiveTime,
) {
    // Panel header
    execute!(out, cursor::MoveTo(x, y)).unwrap();
    execute!(
        out,
        SetForegroundColor(accent_color),
        SetAttribute(Attribute::Bold)
    )
    .unwrap();

    let loading_str = if is_loading { " *" } else { "" };
    let header = format!("{}{}", title, loading_str);
    print!("{}", truncate_str(&header, width as usize));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Separator line
    execute!(out, cursor::MoveTo(x, y + 1)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    for _ in 0..width.min(40) {
        print!("\u{2500}");
    }
    execute!(out, ResetColor).unwrap();

    // Auth status or events
    let content_start = y + 2;
    let max_events = (height.saturating_sub(3)) as usize;

    if !auth_state.is_authenticated() {
        execute!(out, cursor::MoveTo(x, content_start)).unwrap();
        execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
        print!("{}", auth_state.status_message());
        execute!(out, ResetColor).unwrap();
    } else if events.is_empty() {
        execute!(out, cursor::MoveTo(x, content_start)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("No events");
        execute!(out, ResetColor).unwrap();
    } else {
        // Find current and next event indices (only for today)
        let (current_event_idx, next_event_idx) = if is_today {
            find_current_and_next_events(events, current_time)
        } else {
            (None, None)
        };

        for (i, event) in events.iter().take(max_events).enumerate() {
            execute!(out, cursor::MoveTo(x, content_start + i as u16)).unwrap();

            let is_current = current_event_idx == Some(i);
            let is_next = next_event_idx == Some(i);
            let is_past = is_today && is_event_past(event, current_time) && !is_current;
            let is_unaccepted = !event.accepted;

            // Choose color based on event status
            // Priority: unaccepted (grey) > past (grey) > current (green) > next (orange) > normal
            let event_color = if is_unaccepted || is_past {
                Color::DarkGrey
            } else if is_current {
                Color::Green
            } else if is_next {
                Color::Yellow // Orange-ish
            } else {
                Color::Reset
            };

            // Dot indicator for current (green) or next (orange) event
            if is_current && !is_unaccepted {
                execute!(out, SetForegroundColor(Color::Green)).unwrap();
                print!("\u{25CF} "); // Filled circle
            } else if is_next && !is_unaccepted {
                execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
                print!("\u{25CB} "); // Empty circle
            } else {
                print!("  ");
            }

            execute!(out, SetForegroundColor(event_color)).unwrap();
            if (is_current || is_next) && !is_unaccepted {
                execute!(out, SetAttribute(Attribute::Bold)).unwrap();
            }
            print!("{:>7} ", event.time_str);
            execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

            execute!(out, SetForegroundColor(event_color)).unwrap();
            if (is_current || is_next) && !is_unaccepted {
                execute!(out, SetAttribute(Attribute::Bold)).unwrap();
            }

            // Calculate title width, leaving room for video emoji if present
            let has_meeting = event.meeting_url.is_some();
            let join_width = if has_meeting { 3 } else { 0 }; // " 📹"
            let title_width = width.saturating_sub(11 + join_width as u16) as usize;
            print!("{}", truncate_str(&event.title, title_width));
            execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

            // Show clickable video call link if meeting URL available
            if let Some(ref url) = event.meeting_url {
                print!(" ");
                // OSC 8 hyperlink using ST terminator: \x1b]8;;URL\x1b\\TEXT\x1b]8;;\x1b\\
                print!("\x1b]8;;{}\x1b\\\u{1F4F9}\x1b]8;;\x1b\\", url); // 📹 camera emoji
            }
        }

        if events.len() > max_events {
            execute!(
                out,
                cursor::MoveTo(x, content_start + max_events as u16)
            )
            .unwrap();
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!("... +{} more", events.len() - max_events);
            execute!(out, ResetColor).unwrap();
        }
    }
}

/// Parse time string like "14:30" into NaiveTime
fn parse_event_time(time_str: &str) -> Option<NaiveTime> {
    if time_str == "All day" {
        return Some(NaiveTime::from_hms_opt(0, 0, 0)?);
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
fn find_current_and_next_events(events: &[DisplayEvent], current_time: NaiveTime) -> (Option<usize>, Option<usize>) {
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

/// Trait for auth state display
pub trait AuthDisplay {
    fn is_authenticated(&self) -> bool;
    fn status_message(&self) -> String;
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
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
