use crate::error::Result;
use crate::google::TokenInfo;
use chrono::{DateTime, Utc};
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
    /// "eventkit" (macOS, zero config) or "caldav" (cross-platform)
    /// Defaults to "caldav" for backward compatibility
    #[serde(default = "default_icloud_method")]
    pub method: String,
    /// Required for caldav method
    #[serde(default)]
    pub apple_id: Option<String>,
    /// Required for caldav method
    #[serde(default)]
    pub app_password: Option<String>,
}

fn default_icloud_method() -> String {
    "caldav".to_string()
}

impl ICloudConfig {
    pub fn is_eventkit(&self) -> bool {
        self.method == "eventkit"
    }

    #[allow(dead_code)]
    pub fn is_caldav(&self) -> bool {
        self.method == "caldav"
    }
}

fn default_calendar_id() -> String {
    "primary".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredTokens {
    pub google: Option<GoogleTokens>,
    pub icloud: Option<ICloudTokens>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleTokens {
    pub tokens: TokenInfo,
    pub stored_at: DateTime<Utc>,
}

/// Stored calendar entry with URL and optional display name
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCalendar {
    pub url: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICloudTokens {
    /// Legacy field for backwards compatibility
    #[serde(default)]
    pub calendar_urls: Vec<String>,
    /// New field with calendar names
    #[serde(default)]
    pub calendars: Vec<StoredCalendar>,
    pub stored_at: DateTime<Utc>,
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
        // The config can hold the iCloud app password: tighten files written
        // by older versions with the default (world-readable) mode
        restrict_permissions(&path);

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

    pub fn ensure_config_dir() -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        Self::ensure_config_dir()?;
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)?;
        write_private(&path, json.as_bytes())
    }
}

/// Save Google tokens
pub fn save_google_tokens(tokens: &TokenInfo) -> Result<()> {
    Config::ensure_config_dir()?;

    let mut stored = load_all_tokens().unwrap_or(StoredTokens {
        google: None,
        icloud: None,
    });

    stored.google = Some(GoogleTokens {
        tokens: tokens.clone(),
        stored_at: Utc::now(),
    });

    save_all_tokens(&stored)
}

/// Save iCloud discovery info
pub fn save_icloud_tokens(calendars: &[StoredCalendar]) -> Result<()> {
    Config::ensure_config_dir()?;

    let mut stored = load_all_tokens().unwrap_or(StoredTokens {
        google: None,
        icloud: None,
    });

    stored.icloud = Some(ICloudTokens {
        calendar_urls: Vec::new(), // Legacy field, keep empty
        calendars: calendars.to_vec(),
        stored_at: Utc::now(),
    });

    save_all_tokens(&stored)
}

fn save_all_tokens(stored: &StoredTokens) -> Result<()> {
    let path = Config::token_path();
    let json = serde_json::to_string_pretty(stored)?;
    write_private(&path, json.as_bytes())
}

/// Write a secrets-bearing file: created 0600 from the start (no window where
/// it's world-readable), written to a temp file and renamed into place so a
/// crash can't leave it truncated
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
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

/// Make a file owner-only (best effort)
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn load_all_tokens() -> Result<StoredTokens> {
    let path = Config::token_path();
    if !path.exists() {
        return Ok(StoredTokens {
            google: None,
            icloud: None,
        });
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
pub fn load_icloud_tokens() -> Result<Option<ICloudTokens>> {
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
}
