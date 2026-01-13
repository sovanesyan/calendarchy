use chrono::{DateTime, Utc};
use crate::google::TokenInfo;

/// Trait for auth state display
pub trait AuthDisplay {
    fn is_authenticated(&self) -> bool;
}

/// Google authentication state
#[derive(Debug, Clone)]
pub enum GoogleAuthState {
    NotConfigured,
    NotAuthenticated,
    AwaitingUserCode {
        #[allow(dead_code)]
        user_code: String,
        #[allow(dead_code)]
        verification_url: String,
        device_code: String,
        expires_at: DateTime<Utc>,
    },
    Authenticated(TokenInfo),
    #[allow(dead_code)]
    Error(String),
}

impl AuthDisplay for GoogleAuthState {
    fn is_authenticated(&self) -> bool {
        matches!(self, GoogleAuthState::Authenticated(_))
    }
}

/// Calendar with URL and display name
#[derive(Debug, Clone)]
pub struct CalendarEntry {
    pub url: String,
    pub name: Option<String>,
}

/// iCloud authentication state
#[derive(Debug, Clone)]
pub enum ICloudAuthState {
    NotConfigured,
    NotAuthenticated,
    Discovering,
    Authenticated { calendars: Vec<CalendarEntry> },
    #[allow(dead_code)]
    Error(String),
}

impl AuthDisplay for ICloudAuthState {
    fn is_authenticated(&self) -> bool {
        matches!(self, ICloudAuthState::Authenticated { .. })
    }
}
