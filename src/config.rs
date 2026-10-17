use crate::auth::ICloudAccount;
#[cfg(test)]
use crate::auth::CalendarEntry;
use crate::error::Result;
use crate::google::TokenInfo;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub google: Option<GoogleConfig>,
    #[serde(default)]
    pub icloud: Option<ICloudConfig>,
    #[serde(default, skip_serializing_if = "DisplayConfig::is_default")]
    pub display: DisplayConfig,
    /// Rebound keys: action name → key or keys, e.g. `{ "join": "o" }`
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub keys: std::collections::HashMap<String, crate::keymap::KeySpec>,
}

/// Optional display preferences: `"display": { ... }` in config.json
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DisplayConfig {
    /// IANA zone shown next to local times, e.g. "Australia/Sydney"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_timezone: Option<String>,
    /// Your working day as "HH:MM-HH:MM", e.g. "09:00-18:00"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_hours: Option<String>,
    /// Panel names, for when Google isn't work or iCloud isn't personal
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icloud_label: Option<String>,
    /// "12h" for 9:15am-style times (default 24-hour)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_format: Option<String>,
    /// "sunday" to start weeks on Sunday (default Monday)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<String>,
    /// ISO week numbers beside the month grid
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub week_numbers: bool,
}

impl DisplayConfig {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn twelve_hour(&self) -> bool {
        self.time_format.as_deref().is_some_and(|f| f.trim().eq_ignore_ascii_case("12h"))
    }

    pub fn sunday_first(&self) -> bool {
        self.week_start.as_deref().is_some_and(|w| w.trim().eq_ignore_ascii_case("sunday"))
    }

    pub fn google_label(&self) -> &str {
        self.google_label.as_deref().filter(|l| !l.trim().is_empty()).unwrap_or("Work")
    }

    pub fn icloud_label(&self) -> &str {
        self.icloud_label.as_deref().filter(|l| !l.trim().is_empty()).unwrap_or("Personal")
    }

    /// The second zone, if set and known, with a short label ("Sydney")
    pub fn second_tz(&self) -> Option<(chrono_tz::Tz, &str)> {
        let name = self.second_timezone.as_deref()?;
        let tz: chrono_tz::Tz = name.parse().ok()?;
        let label = name.rsplit('/').next().unwrap_or(name);
        Some((tz, label))
    }

    /// Working hours as (start, end) minutes from midnight; None if unset or malformed
    pub fn working_minutes(&self) -> Option<(u16, u16)> {
        let (start, end) = self.working_hours.as_deref()?.split_once('-')?;
        let parse = |s: &str| -> Option<u16> {
            let (h, m) = s.trim().split_once(':')?;
            let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
            (h <= 24 && m < 60).then_some(h * 60 + m)
        };
        let (start, end) = (parse(start)?, parse(end)?);
        (start < end && end <= 24 * 60).then_some((start, end))
    }
}

/// Built-in Google OAuth credentials (public, identifies the app)
pub const DEFAULT_GOOGLE_CLIENT_ID: &str =
    "313544353824-1g092hbgrmd6pemvklv58ld9radn0rg3.apps.googleusercontent.com";
pub const DEFAULT_GOOGLE_CLIENT_SECRET: &str = "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";

/// Google Calendar configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleConfig {
    #[serde(default = "default_google_client_id")]
    pub client_id: String,
    #[serde(default = "default_google_client_secret")]
    pub client_secret: String,
    #[serde(default = "default_calendar_id")]
    pub calendar_id: String,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            client_id: default_google_client_id(),
            client_secret: default_google_client_secret(),
            calendar_id: "primary".to_string(),
        }
    }
}

fn default_google_client_id() -> String {
    std::env::var("CALENDARCHY_GOOGLE_CLIENT_ID")
        .unwrap_or_else(|_| DEFAULT_GOOGLE_CLIENT_ID.to_string())
}

fn default_google_client_secret() -> String {
    std::env::var("CALENDARCHY_GOOGLE_CLIENT_SECRET")
        .unwrap_or_else(|_| DEFAULT_GOOGLE_CLIENT_SECRET.to_string())
}

/// iCloud Calendar configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICloudConfig {
    /// Required for CalDAV
    #[serde(default)]
    pub apple_id: Option<String>,
    /// Required for CalDAV
    #[serde(default)]
    pub app_password: Option<String>,
}

fn default_calendar_id() -> String {
    "primary".to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoredTokens {
    pub google: Option<GoogleTokens>,
    pub icloud: Option<ICloudAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTokens {
    pub tokens: TokenInfo,
}

impl Config {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("calendarchy")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn token_path() -> PathBuf {
        Self::config_dir().join("tokens.json")
    }

    pub fn load() -> Result<Config> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Config::default());
        }

        let content = fs::read_to_string(&path)?;
        let mut config: Config = serde_json::from_str(&content)?;

        // Env vars always override saved config
        if let Some(ref mut google) = config.google {
            if let Ok(id) = std::env::var("CALENDARCHY_GOOGLE_CLIENT_ID") {
                google.client_id = id;
            }
            if let Ok(secret) = std::env::var("CALENDARCHY_GOOGLE_CLIENT_SECRET") {
                google.client_secret = secret;
            }
        }

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)?;
        write_private(&path, json.as_bytes())
    }
}

/// Save Google tokens
pub fn save_google_tokens(tokens: &TokenInfo) -> Result<()> {
    let mut stored = load_all_tokens().unwrap_or_default();
    stored.google = Some(GoogleTokens { tokens: tokens.clone() });

    save_all_tokens(&stored)
}

/// Save iCloud discovery info
pub fn save_icloud_account(account: &ICloudAccount) -> Result<()> {
    let mut stored = load_all_tokens().unwrap_or_default();
    stored.icloud = Some(account.clone());

    save_all_tokens(&stored)
}

fn save_all_tokens(stored: &StoredTokens) -> Result<()> {
    let path = Config::token_path();
    let json = serde_json::to_string_pretty(stored)?;
    write_private(&path, json.as_bytes())
}

/// Write a secrets-bearing file: created 0600 from the start (no window where
/// it's world-readable), written to a temp file and renamed into place so a
/// crash can't leave it truncated. Creates the config directory if needed.
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    // A leftover temp file would keep its old mode: start from scratch
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&tmp)
        .and_then(|mut f| f.write_all(contents).and_then(|_| f.sync_all()))
        .and_then(|_| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

fn load_all_tokens() -> Result<StoredTokens> {
    let path = Config::token_path();
    if !path.exists() {
        return Ok(StoredTokens::default());
    }

    let content = fs::read_to_string(&path)?;
    let stored: StoredTokens = serde_json::from_str(&content)?;
    Ok(stored)
}

/// Load Google tokens
pub fn load_google_tokens() -> Result<Option<TokenInfo>> {
    let stored = load_all_tokens()?;
    Ok(stored.google.map(|g| g.tokens))
}

/// Load iCloud discovery info
pub fn load_icloud_account() -> Result<Option<ICloudAccount>> {
    let stored = load_all_tokens()?;
    Ok(stored.icloud)
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn test_display_config_parsing() {
        let d = DisplayConfig {
            second_timezone: Some("Australia/Sydney".to_string()),
            working_hours: Some("09:00-18:30".to_string()),
            google_label: Some("Dext".to_string()),
            ..Default::default()
        };
        assert_eq!((d.google_label(), d.icloud_label()), ("Dext", "Personal"));
        assert_eq!(d.second_tz().map(|(_, l)| l), Some("Sydney"));
        assert_eq!(d.working_minutes(), Some((540, 1110)));

        let bad = DisplayConfig {
            second_timezone: Some("Mars/Olympus".to_string()),
            working_hours: Some("18:00-09:00".to_string()),
            ..Default::default()
        };
        assert!(bad.second_tz().is_none());
        assert!(bad.working_minutes().is_none());
    }

    #[test]
    fn test_display_config_left_out_of_saved_config_when_unset() {
        let json = serde_json::to_string(&Config::default()).unwrap();
        assert!(!json.contains("display"));
        let parsed: Config = serde_json::from_str(r#"{"display":{"working_hours":"09:00-17:00"}}"#).unwrap();
        assert_eq!(parsed.display.working_minutes(), Some((540, 1020)));
    }

    #[test]
    fn test_old_token_files_still_load() {
        let old = r#"{
            "google": { "tokens": { "access_token": "a", "refresh_token": "r", "expires_at": "2026-10-16T10:00:00Z", "token_type": "Bearer" }, "stored_at": "2026-10-16T09:00:00Z" },
            "icloud": { "calendar_urls": [], "calendars": [{ "url": "https://caldav.icloud.com/1/calendars/home/", "name": "Home" }], "stored_at": "2026-10-16T09:00:00Z" }
        }"#;
        let stored: StoredTokens = serde_json::from_str(old).unwrap();
        assert_eq!(stored.google.unwrap().tokens.refresh_token.as_deref(), Some("r"));
        let icloud = stored.icloud.unwrap();
        assert_eq!(
            icloud.calendars,
            [CalendarEntry { url: "https://caldav.icloud.com/1/calendars/home/".into(), name: Some("Home".into()) }]
        );
        assert!(icloud.addresses.is_empty(), "saved before addresses were discovered");
    }

    #[test]
    fn test_token_file_round_trips() {
        // The shape tokens.json has today: calendars as { url, name }, an
        // unnamed one with name null, and your calendar-user addresses
        let current = r#"{
            "google": { "tokens": { "access_token": "a", "refresh_token": "r", "expires_at": "2026-10-16T10:00:00Z" } },
            "icloud": { "calendars": [
                { "url": "https://p42-caldav.icloud.com/1/calendars/home/", "name": "Home" },
                { "url": "https://p42-caldav.icloud.com/1/calendars/work/", "name": null }
            ], "addresses": ["mailto:me@icloud.com", "/aMTM=/principal/"] }
        }"#;
        let stored: StoredTokens = serde_json::from_str(current).unwrap();
        let icloud = stored.icloud.as_ref().unwrap();
        assert_eq!(icloud.calendars.len(), 2);
        assert_eq!(icloud.calendars[1], CalendarEntry { url: "https://p42-caldav.icloud.com/1/calendars/work/".into(), name: None });
        assert_eq!(icloud.addresses, ["mailto:me@icloud.com", "/aMTM=/principal/"]);

        let written: serde_json::Value = serde_json::to_value(&stored).unwrap();
        assert_eq!(written, serde_json::from_str::<serde_json::Value>(current).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn test_private_files_are_written_owner_only() {
        // config.json holds the app password, tokens.json the sign-ins
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("calendarchy-test-{}", std::process::id()));
        let path = dir.join("config.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"{\"secret\":1}").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let content = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(mode, 0o600, "an existing world-readable file is replaced, not reused");
        assert_eq!(content, "{\"secret\":1}");
    }

    #[test]
    fn test_icloud_config_ignores_legacy_method_field() {
        let parsed: Config = serde_json::from_str(r#"{"icloud":{"method":"caldav","apple_id":"a@b.c","app_password":"p"}}"#).unwrap();
        assert_eq!(parsed.icloud.and_then(|c| c.apple_id).as_deref(), Some("a@b.c"));
    }
}
