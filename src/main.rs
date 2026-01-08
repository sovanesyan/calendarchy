use crossterm::{
    execute,
    terminal::{Clear, ClearType, enable_raw_mode, disable_raw_mode},
    event::{self, Event, KeyCode, KeyEventKind},
    cursor,
    style::{Color, SetForegroundColor, ResetColor, SetAttribute, Attribute},
};
use std::io::{stdout, Write};
use chrono::{Datelike, NaiveDate, Duration, Local};

struct Calendar {
    current_date: NaiveDate,
}

impl Calendar {
    fn new() -> Self {
        Self {
            current_date: Local::now().date_naive(),
        }
    }

    fn render(&self) {
        let mut out = stdout();
        let today = Local::now().date_naive();

        // Clear screen, hide cursor, move to top
        execute!(out, Clear(ClearType::All), cursor::Hide, cursor::MoveTo(0, 0)).unwrap();

        // Get first day of month
        let first_day = self.current_date.with_day(1).unwrap();

        // Calculate starting weekday (Monday = 0, Sunday = 6)
        let start_weekday = first_day.weekday().num_days_from_monday();

        // Print header
        execute!(out, SetForegroundColor(Color::Cyan), SetAttribute(Attribute::Bold)).unwrap();
        print!(" {} {}\r\n",
            self.current_date.format("%B").to_string().to_uppercase(),
            self.current_date.year()
        );
        execute!(out, ResetColor).unwrap();

        // Weekday header
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!(" Mon Tue Wed Thu Fri Sat Sun\r\n");
        execute!(out, ResetColor).unwrap();

        // Print 6 rows (max weeks in a month view)
        let mut current_day = first_day;
        let days_in_month = self.days_in_month();

        for row in 0..6 {
            print!(" ");
            for col in 0..7 {
                let cell = row * 7 + col;
                if cell < start_weekday || cell >= start_weekday + days_in_month {
                    print!("    ");
                } else {
                    let day = (cell - start_weekday + 1) as u32;
                    current_day = first_day.with_day(day).unwrap();
                    let is_today = current_day == today;
                    let is_weekend = col >= 5;

                    if is_today {
                        execute!(out, SetForegroundColor(Color::Green), SetAttribute(Attribute::Bold)).unwrap();
                        print!(" {:2} ", day);
                        execute!(out, ResetColor, SetAttribute(Attribute::Reset)).unwrap();
                    } else if is_weekend {
                        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
                        print!(" {:2} ", day);
                        execute!(out, ResetColor).unwrap();
                    } else {
                        print!(" {:2} ", day);
                    }
                }
            }
            print!("\r\n");
        }

        // Controls
        execute!(out, SetForegroundColor(Color::DarkGrey)).unwrap();
        print!(" j/k t q\r\n");
        execute!(out, ResetColor).unwrap();

        out.flush().unwrap();
    }

    fn goto_today(&mut self) {
        self.current_date = Local::now().date_naive();
    }

    fn days_in_month(&self) -> u32 {
        match self.current_date.month() {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if self.is_leap_year() {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    fn is_leap_year(&self) -> bool {
        let year = self.current_date.year();
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    fn next_month(&mut self) {
        if self.current_date.month() == 12 {
            self.current_date = self.current_date
                .with_year(self.current_date.year() + 1).unwrap()
                .with_month(1).unwrap()
                .with_day(1).unwrap();
        } else {
            self.current_date = self.current_date
                .with_month(self.current_date.month() + 1).unwrap()
                .with_day(1).unwrap();
        }
    }

    fn prev_month(&mut self) {
        if self.current_date.month() == 1 {
            self.current_date = self.current_date
                .with_year(self.current_date.year() - 1).unwrap()
                .with_month(12).unwrap()
                .with_day(1).unwrap();
        } else {
            self.current_date = self.current_date
                .with_month(self.current_date.month() - 1).unwrap()
                .with_day(1).unwrap();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut calendar = Calendar::new();

    // Enable raw mode for single-keypress input
    enable_raw_mode()?;

    loop {
        calendar.render();

        // Wait for a key event
        if let Event::Key(key_event) = event::read()? {
            // Only handle key press events (not release)
            if key_event.kind == KeyEventKind::Press {
                match key_event.code {
                    KeyCode::Char('j') | KeyCode::Down => {
                        calendar.next_month();
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        calendar.prev_month();
                    }
                    KeyCode::Char('t') => {
                        calendar.goto_today();
                    }
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup: restore cursor, clear screen, disable raw mode
    disable_raw_mode()?;
    execute!(stdout(), cursor::Show, Clear(ClearType::All), cursor::MoveTo(0, 0))?;

    Ok(())
}