use crate::error::Result;
use crate::google::TokenInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub google: Option<GoogleConfig>,
    #[serde(default)]
    pub icloud: Option<ICloudConfig>,
}

/// Google Calendar configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_calendar_id")]
    pub calendar_id: String,
}

/// iCloud Calendar configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICloudConfig {
    pub apple_id: String,
    pub app_password: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICloudTokens {
    pub calendar_urls: Vec<String>,
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
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn ensure_config_dir() -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        Ok(())
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
pub fn save_icloud_tokens(calendar_urls: &[String]) -> Result<()> {
    Config::ensure_config_dir()?;

    let mut stored = load_all_tokens().unwrap_or(StoredTokens {
        google: None,
        icloud: None,
    });

    stored.icloud = Some(ICloudTokens {
        calendar_urls: calendar_urls.to_vec(),
        stored_at: Utc::now(),
    });

    save_all_tokens(&stored)
}

fn save_all_tokens(stored: &StoredTokens) -> Result<()> {
    let path = Config::token_path();
    let json = serde_json::to_string_pretty(stored)?;
    fs::write(&path, &json)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
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
