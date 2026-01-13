use std::sync::Mutex;

/// Global log storage for HTTP requests
static HTTP_LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Log an HTTP request
pub fn log_request(method: &str, url: &str) {
    if let Ok(mut logs) = HTTP_LOGS.lock() {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        logs.push(format!("[{}] {} {}", timestamp, method, url));
        // Keep only last 100 logs
        if logs.len() > 100 {
            logs.remove(0);
        }
    }
}

/// Log an HTTP response
pub fn log_response(status: u16, url: &str) {
    if let Ok(mut logs) = HTTP_LOGS.lock() {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        logs.push(format!("[{}] <- {} {}", timestamp, status, url));
        // Keep only last 100 logs
        if logs.len() > 100 {
            logs.remove(0);
        }
    }
}

/// Get recent logs for display
pub fn get_recent_logs(count: usize) -> Vec<String> {
    if let Ok(logs) = HTTP_LOGS.lock() {
        logs.iter().rev().take(count).cloned().collect()
    } else {
        Vec::new()
    }
}
