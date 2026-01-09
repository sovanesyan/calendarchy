use crate::cache::{AttendeeStatus, DisplayEvent, EventCache, EventId};
use crate::{get_recent_logs, EventSource, GoogleAuthState, ICloudAuthState, NavigationMode, ViewMode};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, Weekday};
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
    pub view_mode: ViewMode,
    pub show_weekends: bool,
    pub show_logs: bool,
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
}

pub fn render(state: &RenderState) {
    let mut out = stdout();
    let today = Local::now().date_naive();

    // Get terminal size
    let (term_width, term_height) = terminal::size().unwrap_or((80, 24));

    execute!(out, Clear(ClearType::All), cursor::Hide).unwrap();

    match state.view_mode {
        ViewMode::Month => render_month_view(&mut out, state, today, term_width, term_height),
        ViewMode::Week => render_week_view(&mut out, state, today, term_width, term_height),
    }

    // Render HTTP logs if enabled
    let log_height = if state.show_logs { 8 } else { 0 };
    if state.show_logs {
        let logs = get_recent_logs(log_height as usize);
        let log_start_row = term_height.saturating_sub(2 + log_height);

        execute!(out, SetForegroundColor(Color::DarkCyan)).unwrap();
        for (i, log) in logs.iter().rev().enumerate() {
            let row = log_start_row + i as u16;
            if row < term_height.saturating_sub(2) {
                execute!(out, cursor::MoveTo(0, row)).unwrap();
                print!(" {}", truncate_str(log, term_width as usize - 2));
            }
        }
        execute!(out, ResetColor).unwrap();
    }

    // Render status bar at bottom
    let status_row = term_height.saturating_sub(2);
    execute!(out, cursor::MoveTo(0, status_row)).unwrap();

    if let Some(msg) = state.status_message {
        execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
        print!(" {}", truncate_str(msg, term_width as usize - 2));
        execute!(out, ResetColor).unwrap();
    }

    // Render controls based on current mode
    execute!(out, cursor::MoveTo(0, term_height.saturating_sub(1))).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();

    let controls = if state.navigation_mode == NavigationMode::Event {
        // Event navigation mode controls
        let mut c = String::from(" jk:nav");

        // Check if selected event has meeting link and source
        let selected_event = match state.selected_source {
            EventSource::Google => state.events.google.get(state.selected_date).get(state.selected_event_index),
            EventSource::ICloud => state.events.icloud.get(state.selected_date).get(state.selected_event_index),
        };
        if let Some(event) = selected_event {
            if event.meeting_url.is_some() {
                c.push_str(" o:open");
            }
            // Google events support accept/decline
            if matches!(event.id, EventId::Google { .. }) {
                c.push_str(" a:accept d:decline");
            }
            c.push_str(" x:delete");
        }

        c.push_str(" 1:google 2:icloud D:logs Esc:back q:quit");
        c
    } else {
        // Day navigation mode controls
        let mut c = String::from(" hjkl:nav t:today r:refresh v:view 1:google 2:icloud D:logs");
        if state.view_mode == ViewMode::Month {
            c.push_str(" Enter:events");
        }
        if state.view_mode == ViewMode::Week {
            c.push_str(" s:weekends");
        }
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

    if in_event_mode {
        let available = term_width.saturating_sub(CALENDAR_WIDTH + 2);
        events_panel_width = (available * 2 / 5).max(MIN_PANEL_WIDTH);
        details_panel_width = available.saturating_sub(events_panel_width + 1);
    } else {
        events_panel_width = term_width.saturating_sub(CALENDAR_WIDTH + 1);
        details_panel_width = 0;
    }

    let panel_height = term_height.saturating_sub(3) / 2;

    // Render calendar on left
    render_calendar(out, state.current_date, state.selected_date, today, state.events, state.google_loading || state.icloud_loading);

    // Render event panels in the middle
    if events_panel_width >= MIN_PANEL_WIDTH {
        let events_x = CALENDAR_WIDTH + 1;

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

        // Render Work (Google) panel on right top
        render_event_panel_with_selection(
            out,
            events_x,
            0,
            events_panel_width,
            panel_height,
            "Work (Google)",
            state.events.google.get(state.selected_date),
            state.google_auth,
            state.google_loading,
            Color::Blue,
            is_today,
            current_time,
            google_selected,
        );

        // Render Personal (iCloud) panel on right bottom
        render_event_panel_with_selection(
            out,
            events_x,
            panel_height + 1,
            events_panel_width,
            panel_height,
            "Personal (iCloud)",
            state.events.icloud.get(state.selected_date),
            state.icloud_auth,
            state.icloud_loading,
            Color::Magenta,
            is_today,
            current_time,
            icloud_selected,
        );
    }

    // Render details panel on the right when in Event mode
    if in_event_mode && details_panel_width >= MIN_PANEL_WIDTH {
        let details_x = CALENDAR_WIDTH + events_panel_width + 2;
        let details_height = term_height.saturating_sub(3);

        // Get the selected event
        let selected_event = match state.selected_source {
            EventSource::Google => state.events.google.get(state.selected_date).get(state.selected_event_index),
            EventSource::ICloud => state.events.icloud.get(state.selected_date).get(state.selected_event_index),
        };

        render_event_details_column(out, details_x, 0, details_panel_width, details_height, selected_event);
    }
}

fn render_week_view(out: &mut impl Write, state: &RenderState, today: NaiveDate, term_width: u16, term_height: u16) {
    let now = Local::now();
    let current_time = now.time();

    // Calculate the week (Monday to Sunday) containing selected_date
    let days_from_monday = state.selected_date.weekday().num_days_from_monday();
    let week_start = state.selected_date - Duration::days(days_from_monday as i64);

    // Determine which days to show
    let days_to_show: Vec<i64> = if state.show_weekends {
        (0..7).collect() // Mon-Sun
    } else {
        (0..5).collect() // Mon-Fri
    };
    let num_days = days_to_show.len() as u16;

    // Calculate column width
    let col_width = term_width / num_days;
    let panel_height = term_height.saturating_sub(3); // Leave room for status and controls
    let half_height = panel_height / 2;

    // Render header row with day names and dates
    for (col_idx, &day_offset) in days_to_show.iter().enumerate() {
        let date = week_start + Duration::days(day_offset);
        let x = (col_idx as u16) * col_width;
        let is_selected = date == state.selected_date;
        let is_today = date == today;

        execute!(out, cursor::MoveTo(x, 0)).unwrap();

        // Day name
        let day_name = match date.weekday() {
            Weekday::Mon => "Mon",
            Weekday::Tue => "Tue",
            Weekday::Wed => "Wed",
            Weekday::Thu => "Thu",
            Weekday::Fri => "Fri",
            Weekday::Sat => "Sat",
            Weekday::Sun => "Sun",
        };

        if is_selected {
            execute!(out, SetForegroundColor(Color::Black)).unwrap();
            execute!(out, crossterm::style::SetBackgroundColor(Color::White)).unwrap();
        } else if is_today {
            execute!(out, SetForegroundColor(Color::Green)).unwrap();
        } else if date.weekday() == Weekday::Sat || date.weekday() == Weekday::Sun {
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        } else {
            execute!(out, SetForegroundColor(Color::White)).unwrap();
        }

        let header = format!("{} {:02}", day_name, date.day());
        print!("{:^width$}", header, width = col_width as usize);
        execute!(out, ResetColor, crossterm::style::SetBackgroundColor(Color::Reset)).unwrap();
    }

    // Render each day column
    for (col_idx, &day_offset) in days_to_show.iter().enumerate() {
        let date = week_start + Duration::days(day_offset);
        let x = (col_idx as u16) * col_width;
        let is_today = date == today;
        let is_past_day = date < today;

        // Work (Google) - top half
        render_week_day_panel(
            out,
            x,
            1,
            col_width.saturating_sub(1),
            half_height,
            state.events.google.get(date),
            Color::Blue,
            is_today,
            is_past_day,
            current_time,
        );

        // Separator line
        execute!(out, cursor::MoveTo(x, 1 + half_height)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("{}", "─".repeat(col_width.saturating_sub(1) as usize));
        execute!(out, ResetColor).unwrap();

        // Personal (iCloud) - bottom half
        render_week_day_panel(
            out,
            x,
            2 + half_height,
            col_width.saturating_sub(1),
            half_height.saturating_sub(1),
            state.events.icloud.get(date),
            Color::Magenta,
            is_today,
            is_past_day,
            current_time,
        );
    }
}

fn render_week_day_panel(
    out: &mut impl Write,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    events: &[DisplayEvent],
    _color: Color,
    is_today: bool,
    is_past_day: bool,
    current_time: NaiveTime,
) {
    let max_lines = height as usize;
    let (current_event_idx, next_event_idx) = if is_today {
        find_current_and_next_events(events, current_time)
    } else {
        (None, None)
    };

    let mut current_line = 0;
    let mut events_shown = 0;
    let usable_width = width.saturating_sub(1) as usize; // Leave space for indicator

    for (i, event) in events.iter().enumerate() {
        if current_line >= max_lines {
            break;
        }

        let is_current = current_event_idx == Some(i);
        let is_next = next_event_idx == Some(i);
        let is_past_event = is_today && is_event_past(event, current_time) && !is_current;
        let is_unaccepted = !event.accepted;

        // Gray out: past days, past events today, or unaccepted
        let event_color = if is_past_day || is_unaccepted || is_past_event {
            Color::DarkGrey
        } else if is_current {
            Color::Green
        } else if is_next {
            Color::Yellow
        } else {
            Color::Reset
        };

        // Format: "[icon] HH:MM Title" with wrapping
        // Reserve 2 chars at start for meeting icon (consistent spacing whether link exists or not)
        let icon_space = 2;
        let adjusted_width = usable_width.saturating_sub(icon_space);
        let time_title = format!("{} {}", event.time_str, event.title);
        let wrapped_lines = wrap_text(&time_title, adjusted_width);

        for (line_idx, line) in wrapped_lines.iter().enumerate() {
            if current_line >= max_lines {
                break;
            }

            execute!(out, cursor::MoveTo(x, y + current_line as u16)).unwrap();

            // Show indicator only on first line of event
            if line_idx == 0 {
                if is_current && !is_unaccepted {
                    execute!(out, SetForegroundColor(Color::Green)).unwrap();
                    print!("\u{25CF}");
                } else if is_next && !is_unaccepted {
                    execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
                    print!("\u{25CB}");
                } else {
                    print!(" ");
                }

                // Meeting icon to the left of time (consistent spacing)
                if let Some(ref url) = event.meeting_url {
                    print!("\x1b]8;;{}\x1b\\\u{1F4F9}\x1b]8;;\x1b\\", url);
                } else {
                    print!("  "); // Reserve space for alignment
                }
            } else {
                print!("   "); // Indent continuation lines (indicator + icon space)
            }

            execute!(out, SetForegroundColor(event_color)).unwrap();
            print!("{}", line);
            execute!(out, ResetColor).unwrap();

            current_line += 1;
        }

        events_shown += 1;
    }

    // Show overflow indicator if there are more events
    if events_shown < events.len() && current_line < max_lines {
        execute!(out, cursor::MoveTo(x, y + current_line as u16)).unwrap();
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("+{}", events.len() - events_shown);
        execute!(out, ResetColor).unwrap();
    }
}

/// Wrap text to fit within a given width
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = word.chars().count();

        if current_width == 0 {
            // First word on line
            if word_width > max_width {
                // Word is too long, force break it
                for ch in word.chars() {
                    if current_width >= max_width {
                        lines.push(current_line);
                        current_line = String::new();
                        current_width = 0;
                    }
                    current_line.push(ch);
                    current_width += 1;
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        } else if current_width + 1 + word_width <= max_width {
            // Word fits on current line
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            // Word doesn't fit, start new line
            lines.push(current_line);
            if word_width > max_width {
                // Word is too long, force break it
                current_line = String::new();
                current_width = 0;
                for ch in word.chars() {
                    if current_width >= max_width {
                        lines.push(current_line);
                        current_line = String::new();
                        current_width = 0;
                    }
                    current_line.push(ch);
                    current_width += 1;
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
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

/// Render event panel with optional selection highlighting
fn render_event_panel_with_selection<A: AuthDisplay>(
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
    selected_index: Option<usize>,
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

            let is_selected = selected_index == Some(i);
            let is_current = current_event_idx == Some(i);
            let is_next = next_event_idx == Some(i);
            let is_past = is_today && is_event_past(event, current_time) && !is_current;
            let is_unaccepted = !event.accepted;

            // Choose color based on event status
            let event_color = if is_selected {
                Color::Cyan
            } else if is_unaccepted || is_past {
                Color::DarkGrey
            } else if is_current {
                Color::Green
            } else if is_next {
                Color::Yellow
            } else {
                Color::Reset
            };

            // Selection/status indicator
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

            // Meeting icon
            if let Some(ref url) = event.meeting_url {
                print!("\x1b]8;;{}\x1b\\\u{1F4F9}\x1b]8;;\x1b\\", url);
            } else {
                print!("  ");
            }

            execute!(out, SetForegroundColor(event_color)).unwrap();
            if is_selected || ((is_current || is_next) && !is_unaccepted) {
                execute!(out, SetAttribute(Attribute::Bold)).unwrap();
            }
            print!("{:>7} ", event.time_str);
            execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

            execute!(out, SetForegroundColor(event_color)).unwrap();
            if is_selected || ((is_current || is_next) && !is_unaccepted) {
                execute!(out, SetAttribute(Attribute::Bold)).unwrap();
            }

            let title_width = width.saturating_sub(13) as usize;
            print!("{}", truncate_str(&event.title, title_width));
            execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
        }

        if events.len() > max_events {
            execute!(out, cursor::MoveTo(x, content_start + max_events as u16)).unwrap();
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!("... +{} more", events.len() - max_events);
            execute!(out, ResetColor).unwrap();
        }
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
    execute!(out, SetForegroundColor(Color::Cyan), SetAttribute(Attribute::Bold)).unwrap();
    print!("Details");
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();

    // Separator line
    execute!(out, cursor::MoveTo(x, y + 1)).unwrap();
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    for _ in 0..width.min(40) {
        print!("\u{2500}");
    }
    execute!(out, ResetColor).unwrap();

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
    execute!(out, SetForegroundColor(Color::White), SetAttribute(Attribute::Bold)).unwrap();
    print!("{}", truncate_str(&event.title, content_width));
    execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
    current_row += 1;

    // Time
    execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
    execute!(out, SetForegroundColor(Color::White)).unwrap();
    if let Some(ref end) = event.end_time_str {
        print!("\u{1F552} {} - {}", event.time_str, end);
    } else {
        print!("\u{1F552} {}", event.time_str);
    }
    execute!(out, ResetColor).unwrap();
    current_row += 1;

    // Location
    if let Some(ref loc) = event.location {
        if !loc.is_empty() && current_row < y + height - 3 {
            execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
            execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
            print!("\u{1F4CD} {}", truncate_str(loc, content_width.saturating_sub(3)));
            execute!(out, ResetColor).unwrap();
            current_row += 1;
        }
    }

    // Meeting link
    if event.meeting_url.is_some() && current_row < y + height - 3 {
        execute!(out, cursor::MoveTo(content_x, current_row)).unwrap();
        execute!(out, SetForegroundColor(Color::Green)).unwrap();
        print!("\u{1F4F9} [o] Open meeting link");
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
            let (icon, color) = match attendee.status {
                AttendeeStatus::Accepted => ("\u{2713}", Color::Green),
                AttendeeStatus::Organizer => ("\u{2713}", Color::Blue),
                AttendeeStatus::Declined => ("\u{2717}", Color::Red),
                AttendeeStatus::Tentative => ("?", Color::Yellow),
                AttendeeStatus::NeedsAction => ("?", Color::DarkGrey),
            };
            execute!(out, SetForegroundColor(color)).unwrap();
            print!("  {} ", icon);
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn make_event(time: &str) -> DisplayEvent {
        DisplayEvent {
            id: EventId::Google { calendar_id: "test".to_string(), event_id: "test-id".to_string() },
            title: "Test".to_string(),
            time_str: time.to_string(),
            end_time_str: None,
            date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            accepted: true,
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
