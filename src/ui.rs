use crate::cache::EventCache;
use crate::AuthState;
use chrono::{Datelike, Local, NaiveDate};
use crossterm::{
    cursor,
    execute,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::{stdout, Write};

pub fn render(
    current_date: NaiveDate,
    selected_date: NaiveDate,
    events: &EventCache,
    auth_state: &AuthState,
    status_message: Option<&str>,
    is_loading: bool,
) {
    let mut out = stdout();
    let today = Local::now().date_naive();

    execute!(
        out,
        Clear(ClearType::All),
        cursor::Hide,
        cursor::MoveTo(0, 0)
    )
    .unwrap();

    // Month header
    let first_day = current_date.with_day(1).unwrap();
    execute!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold)
    )
    .unwrap();
    let loading_indicator = if is_loading { " *" } else { "" };
    print!(
        " {} {}{}\r\n",
        current_date
            .format("%B")
            .to_string()
            .to_uppercase(),
        current_date.year(),
        loading_indicator
    );
    execute!(out, ResetColor).unwrap();

    // Weekday header
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!(" Mon Tue Wed Thu Fri Sat Sun\r\n");
    execute!(out, ResetColor).unwrap();

    // Calendar grid
    let start_weekday = first_day.weekday().num_days_from_monday();
    let days_in_month = days_in_month(current_date);

    for row in 0..6 {
        print!(" ");
        for col in 0..7 {
            let cell = row * 7 + col;
            if cell < start_weekday || cell >= start_weekday + days_in_month {
                print!("    ");
            } else {
                let day = (cell - start_weekday + 1) as u32;
                let date = first_day.with_day(day).unwrap();
                let is_today = date == today;
                let is_selected = date == selected_date;
                let is_weekend = col >= 5;
                let has_events = events.has_events(date);

                // Color selection
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

                // Day number with optional dot
                if has_events && !is_selected {
                    print!("{:2}\u{2022} ", day); // bullet point
                } else {
                    print!(" {:2} ", day);
                }

                execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
            }
        }
        print!("\r\n");
    }

    // Separator
    print!("\r\n");

    // Event list for selected date
    render_event_list(&mut out, selected_date, events);

    // Auth status or status message
    render_status(&mut out, auth_state, status_message);

    // Controls
    execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
    print!(" hjkl:nav t:today r:refresh a:auth q:quit\r\n");
    execute!(out, ResetColor).unwrap();

    out.flush().unwrap();
}

fn render_event_list(out: &mut impl Write, date: NaiveDate, events: &EventCache) {
    let day_events = events.get(date);

    execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
    print!(" {} {}\r\n", date.format("%A"), date.format("%b %d"));
    execute!(out, ResetColor).unwrap();

    if day_events.is_empty() {
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!("   No events\r\n");
        execute!(out, ResetColor).unwrap();
    } else {
        for event in day_events.iter().take(5) {
            let time_str = event.time_str();

            execute!(out, SetForegroundColor(Color::White)).unwrap();
            print!("   {:>7} ", time_str);
            execute!(out, ResetColor).unwrap();
            print!("{}\r\n", truncate(event.title(), 35));
        }

        if day_events.len() > 5 {
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!("   ... and {} more\r\n", day_events.len() - 5);
            execute!(out, ResetColor).unwrap();
        }
    }

    print!("\r\n");
}

fn render_status(out: &mut impl Write, auth_state: &AuthState, status_message: Option<&str>) {
    match auth_state {
        AuthState::NotAuthenticated => {
            execute!(out, SetForegroundColor(Color::Yellow)).unwrap();
            print!(" Press 'a' to connect Google Calendar\r\n");
            execute!(out, ResetColor).unwrap();
        }
        AuthState::AwaitingUserCode {
            user_code,
            verification_url,
            ..
        } => {
            execute!(out, SetForegroundColor(Color::Cyan)).unwrap();
            print!(" Visit {} and enter: {}\r\n", verification_url, user_code);
            execute!(out, ResetColor).unwrap();
        }
        AuthState::Authenticated(_) => {
            // Show nothing when authenticated and working
        }
        AuthState::Refreshing => {
            execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
            print!(" Refreshing...\r\n");
            execute!(out, ResetColor).unwrap();
        }
        AuthState::Error(msg) => {
            execute!(out, SetForegroundColor(Color::Red)).unwrap();
            print!(" Error: {}\r\n", msg);
            execute!(out, ResetColor).unwrap();
        }
    }

    if let Some(msg) = status_message {
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!(" {}\r\n", msg);
        execute!(out, ResetColor).unwrap();
    }
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len.saturating_sub(3)]
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
