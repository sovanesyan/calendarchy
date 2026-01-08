use crate::config::GoogleConfig;
use crate::error::{CalendarchyError, Result};
use crate::google::types::{DeviceCodeResponse, TokenInfo, TokenResponse};
use chrono::Utc;
use reqwest::Client;

const DEVICE_CODE_URL: &str = "https://oauth2.googleapis.com/device/code";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";

pub struct GoogleAuth {
    client: Client,
    config: GoogleConfig,
}

#[derive(Debug)]
pub enum PollResult {
    Success(TokenInfo),
    Pending,
    SlowDown,
    Denied,
    Expired,
}

impl GoogleAuth {
    pub fn new(config: GoogleConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Step 1: Request device code
    pub async fn request_device_code(&self) -> Result<DeviceCodeResponse> {
        let response = self
            .client
            .post(DEVICE_CODE_URL)
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("scope", CALENDAR_SCOPE),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CalendarchyError::Auth(format!(
                "Failed to get device code: {}",
                body
            )));
        }

        let device_code: DeviceCodeResponse = response.json().await?;
        Ok(device_code)
    }

    /// Step 2: Poll for token (call this repeatedly)
    pub async fn poll_for_token(&self, device_code: &str) -> Result<PollResult> {
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;

        if response.status().is_success() {
            let token_response: TokenResponse = response.json().await?;
            let token_info = TokenInfo {
                access_token: token_response.access_token,
                refresh_token: token_response.refresh_token,
                expires_at: Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64),
                token_type: token_response.token_type,
            };
            Ok(PollResult::Success(token_info))
        } else {
            let error: serde_json::Value = response.json().await?;
            match error.get("error").and_then(|e| e.as_str()) {
                Some("authorization_pending") => Ok(PollResult::Pending),
                Some("slow_down") => Ok(PollResult::SlowDown),
                Some("access_denied") => Ok(PollResult::Denied),
                Some("expired_token") => Ok(PollResult::Expired),
                _ => Err(CalendarchyError::Auth(format!(
                    "Unknown error: {:?}",
                    error
                ))),
            }
        }
    }

    /// Refresh an expired token
    #[allow(dead_code)]
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenInfo> {
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CalendarchyError::Auth(format!(
                "Failed to refresh token: {}",
                body
            )));
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(TokenInfo {
            access_token: token_response.access_token,
            refresh_token: Some(refresh_token.to_string()), // Keep original
            expires_at: Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64),
            token_type: token_response.token_type,
        })
    }
}
