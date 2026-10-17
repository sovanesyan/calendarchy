use crate::google::TokenInfo;

/// Google authentication state
#[derive(Debug, Clone)]
pub enum GoogleAuthState {
    NotConfigured,
    NotAuthenticated,
    Authenticating,
    Authenticated(TokenInfo),
}

/// A CalDAV calendar: its URL and display name. Saved in tokens.json as
/// `{ "url", "name" }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CalendarEntry {
    pub url: String,
    pub name: Option<String>,
}

/// What CalDAV discovery finds for an iCloud account; saved in tokens.json
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ICloudAccount {
    /// The calendars found by the last discovery
    #[serde(default)]
    pub calendars: Vec<CalendarEntry>,
    /// Your calendar-user addresses: the principal and its
    /// `calendar-user-address-set` (mailto: addresses, principal paths,
    /// `urn:uuid:`s), which is how iCloud writes you on invites. Empty in
    /// files saved before they were discovered; that triggers a rediscovery
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// iCloud authentication state
#[derive(Debug, Clone)]
pub enum ICloudAuthState {
    NotConfigured,
    Discovering,
    Authenticated(ICloudAccount),
    /// Discovery failed; finishing setup retries it
    Error,
}
