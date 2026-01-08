use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum CalendarchyError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Token expired")]
    TokenExpired,

    #[error("Not authenticated")]
    NotAuthenticated,
}

pub type Result<T> = std::result::Result<T, CalendarchyError>;
