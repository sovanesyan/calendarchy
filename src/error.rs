use reqwest::{Response, StatusCode};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CalendarchyError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("API error: {0}")]
    Api(String),

    #[allow(dead_code)]
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("XML error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("CalDAV error: {0}")]
    CalDav(String),

    #[error("Token expired")]
    TokenExpired,

    #[allow(dead_code)]
    #[error("Not authenticated")]
    NotAuthenticated,
}

pub type Result<T> = std::result::Result<T, CalendarchyError>;

/// Check Google API response status and return appropriate error
/// Returns the response body as text on success
pub async fn check_google_response(response: Response, context: &str) -> Result<String> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(CalendarchyError::TokenExpired);
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CalendarchyError::Api(format!("{} {}: {}", context, status, body)));
    }

    Ok(response.text().await?)
}

/// Check Google API response for success, allowing NO_CONTENT (for DELETE)
pub async fn check_google_response_no_body(response: Response, context: &str) -> Result<()> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(CalendarchyError::TokenExpired);
    }

    if !response.status().is_success() && response.status() != StatusCode::NO_CONTENT {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CalendarchyError::Api(format!("{} {}: {}", context, status, body)));
    }

    Ok(())
}

/// Check CalDAV response status and return appropriate error
/// Returns the response body as text on success
pub async fn check_caldav_response(response: Response, context: &str) -> Result<String> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CalendarchyError::CalDav(format!("{} {}: {}", context, status, body)));
    }

    Ok(response.text().await?)
}

/// Check CalDAV response for success, allowing NO_CONTENT and NOT_FOUND (for DELETE)
pub async fn check_caldav_response_no_body(response: Response, context: &str) -> Result<()> {
    // 404 means already deleted, consider success
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(());
    }

    if !response.status().is_success() && response.status() != StatusCode::NO_CONTENT {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(CalendarchyError::CalDav(format!("{} {}: {}", context, status, body)));
    }

    Ok(())
}
