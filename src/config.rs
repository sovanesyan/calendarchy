use crate::error::Result;
use crate::google::TokenInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_calendar_id")]
    pub calendar_id: String,
}

fn default_calendar_id() -> String {
    "primary".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StoredTokens {
    pub tokens: TokenInfo,
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

    pub fn load() -> Result<Option<Config>> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(Some(config))
    }

    pub fn ensure_config_dir() -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        Ok(())
    }
}

pub fn save_tokens(tokens: &TokenInfo) -> Result<()> {
    Config::ensure_config_dir()?;
    let path = Config::token_path();

    let stored = StoredTokens {
        tokens: tokens.clone(),
        stored_at: Utc::now(),
    };

    let json = serde_json::to_string_pretty(&stored)?;
    fs::write(&path, &json)?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

pub fn load_tokens() -> Result<Option<TokenInfo>> {
    let path = Config::token_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let stored: StoredTokens = serde_json::from_str(&content)?;
    Ok(Some(stored.tokens))
}

