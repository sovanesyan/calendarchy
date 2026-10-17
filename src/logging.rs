use std::collections::VecDeque;
use std::sync::Mutex;

/// How many log lines are kept
const MAX_LOGS: usize = 100;

/// Global log storage for HTTP requests
static HTTP_LOGS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Add a timestamped line, dropping the oldest beyond `MAX_LOGS`
fn push(line: std::fmt::Arguments) {
    if let Ok(mut logs) = HTTP_LOGS.lock() {
        logs.push_back(format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), line));
        if logs.len() > MAX_LOGS {
            logs.pop_front();
        }
    }
}

/// Log an HTTP request
pub fn log_request(method: &str, url: &str) {
    push(format_args!("{} {}", method, url));
}

/// Log an HTTP response
pub fn log_response(status: u16, url: &str) {
    push(format_args!("<- {} {}", status, url));
}

/// Log a panic in a background task (the UI stays up, so it can't print)
pub fn log_panic(message: &str) {
    push(format_args!("PANIC {}", message));
}

/// Get recent logs for display
pub fn get_recent_logs(count: usize) -> Vec<String> {
    if let Ok(logs) = HTTP_LOGS.lock() {
        logs.iter().rev().take(count).cloned().collect()
    } else {
        Vec::new()
    }
}
