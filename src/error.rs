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

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("CalDAV error: {0}")]
    CalDav(String),

    #[error("Token expired")]
    TokenExpired,
}

pub type Result<T> = std::result::Result<T, CalendarchyError>;

/// The body of a successful response; otherwise `error` with the context,
/// status and body
pub async fn check(response: Response, context: &str, error: fn(String) -> CalendarchyError) -> Result<String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response.text().await?);
    }
    let body = response.text().await.unwrap_or_default();
    Err(error(format!("{} {}: {}", context, status, body)))
}

/// `check` for Google APIs, where a 401 means the access token has expired
pub async fn check_google(response: Response, context: &str) -> Result<String> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(CalendarchyError::TokenExpired);
    }
    check(response, context, CalendarchyError::Api).await
}
