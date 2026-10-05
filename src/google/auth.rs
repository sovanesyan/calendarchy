use crate::config::GoogleConfig;
use crate::error::{CalendarchyError, Result};
use crate::google::types::{TokenInfo, TokenResponse};
use crate::logging::{log_request, log_response};
use chrono::Utc;
use reqwest::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::time::Duration;
use tokio::net::TcpListener;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
const REDIRECT_URI: &str = "http://127.0.0.1:18457";

/// How long to wait for the browser to come back before giving up (and
/// releasing the loopback port, so sign-in can be retried)
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

pub struct GoogleAuth {
    client: Client,
    config: GoogleConfig,
    /// Random value round-tripped through the browser (CSRF protection)
    state: String,
}

impl GoogleAuth {
    #[cfg(test)]
    pub fn new(config: GoogleConfig) -> Self {
        Self::with_client(Client::new(), config)
    }

    pub fn with_client(client: Client, config: GoogleConfig) -> Self {
        Self { client, config, state: random_state() }
    }

    /// Get the authorization URL that the user should open in their browser
    pub fn auth_url(&self) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
            AUTH_URL,
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(REDIRECT_URI),
            urlencoding::encode(CALENDAR_SCOPE),
            self.state,
        )
    }

    /// Start a localhost server, wait for the OAuth callback, and exchange the code for tokens.
    /// Gives up after SIGN_IN_TIMEOUT so an abandoned sign-in doesn't hold the port.
    pub async fn authenticate_with_browser(&self) -> Result<TokenInfo> {
        let listener = TcpListener::bind("127.0.0.1:18457").await
            .map_err(|e| CalendarchyError::Auth(format!("Failed to start auth server: {}", e)))?;

        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, self.wait_for_code(&listener))
            .await
            .map_err(|_| CalendarchyError::Auth("Sign-in timed out — press g to try again".to_string()))??;

        self.exchange_code(&code).await
    }

    /// Serve loopback requests until the OAuth redirect arrives. Other requests
    /// (favicon, prefetch) get a 404 and are ignored.
    async fn wait_for_code(&self, listener: &TcpListener) -> Result<String> {
        loop {
            let (mut stream, _) = listener.accept().await
                .map_err(|e| CalendarchyError::Auth(format!("Failed to accept connection: {}", e)))?;

            let mut buf = vec![0u8; 8192];
            let n = match stream.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => continue,
            };
            let request = String::from_utf8_lossy(&buf[..n]);

            match parse_callback(&request, &self.state) {
                Callback::Code(code) => {
                    respond(&mut stream, "200 OK", "Authenticated! You can close this tab.").await;
                    return Ok(code);
                }
                Callback::Denied(error) => {
                    respond(&mut stream, "200 OK", "Sign-in was cancelled. You can close this tab.").await;
                    return Err(CalendarchyError::Auth(format!("Sign-in denied: {}", error)));
                }
                Callback::BadState => {
                    respond(&mut stream, "400 Bad Request", "Sign-in link didn't match this session.").await;
                }
                Callback::Other => {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                    let _ = stream.shutdown().await;
                }
            }
        }
    }

    /// Exchange an authorization code for tokens
    async fn exchange_code(&self, code: &str) -> Result<TokenInfo> {
        log_request("POST", TOKEN_URL);
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT_URI),
            ])
            .send()
            .await?;
        log_response(response.status().as_u16(), TOKEN_URL);

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CalendarchyError::Auth(format!(
                "Failed to exchange code: {}",
                body
            )));
        }

        let token_response: TokenResponse = response.json().await?;
        Ok(TokenInfo {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at: Utc::now() + chrono::Duration::seconds(token_response.expires_in as i64),
            token_type: token_response.token_type,
        })
    }

    /// Refresh an expired token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenInfo> {
        log_request("POST", &format!("{} (refresh)", TOKEN_URL));
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
        log_response(response.status().as_u16(), TOKEN_URL);

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(refresh_error(status, body));
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

/// Only a rejected grant (400/401: revoked or expired refresh token) means
/// "sign in again"; 5xx/429 are transient and must not sign the user out
fn refresh_error(status: reqwest::StatusCode, body: String) -> CalendarchyError {
    if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED {
        CalendarchyError::Auth(format!("Failed to refresh token: {}", body))
    } else {
        CalendarchyError::Api(format!("Token refresh failed ({}), will retry: {}", status, body))
    }
}

#[derive(Debug, PartialEq)]
enum Callback {
    Code(String),
    Denied(String),
    BadState,
    Other,
}

/// Classify a request to the loopback server
fn parse_callback(request: &str, expected_state: &str) -> Callback {
    let Some(query) = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| path.split_once('?'))
        .map(|(_, q)| q)
    else {
        return Callback::Other;
    };
    let param = |name: &str| {
        query.split('&').find_map(|p| {
            p.strip_prefix(name)
                .and_then(|v| v.strip_prefix('='))
                .and_then(|v| urlencoding::decode(v).ok())
                .map(|v| v.into_owned())
        })
    };
    let (code, error) = (param("code"), param("error"));
    if code.is_none() && error.is_none() {
        return Callback::Other;
    }
    if param("state").as_deref() != Some(expected_state) {
        return Callback::BadState;
    }
    match (code, error) {
        (Some(code), _) => Callback::Code(code),
        (None, Some(error)) => Callback::Denied(error),
        (None, None) => Callback::Other,
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, message: &str) {
    let body = format!(
        "<html><body style=\"font-family:system-ui;display:flex;justify-content:center;align-items:center;height:100vh;margin:0\">\
         <h2>{}</h2></body></html>",
        message
    );
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Unpredictable token for the OAuth `state` parameter. RandomState is seeded
/// from the OS RNG, so hashing with fresh instances gives random u64s.
fn random_state() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(std::process::id() as u64);
            format!("{:016x}", h.finish())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> String {
        format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:18457\r\n\r\n", path)
    }

    #[test]
    fn test_callback_with_code_and_state() {
        assert_eq!(parse_callback(&get("/?state=abc&code=4%2F0Ab"), "abc"), Callback::Code("4/0Ab".into()));
    }

    #[test]
    fn test_callback_rejects_wrong_state() {
        assert_eq!(parse_callback(&get("/?state=evil&code=x"), "abc"), Callback::BadState);
        assert_eq!(parse_callback(&get("/?code=x"), "abc"), Callback::BadState);
    }

    #[test]
    fn test_callback_denied() {
        assert_eq!(parse_callback(&get("/?error=access_denied&state=abc"), "abc"), Callback::Denied("access_denied".into()));
    }

    #[test]
    fn test_stray_requests_are_ignored() {
        assert_eq!(parse_callback(&get("/favicon.ico"), "abc"), Callback::Other);
        assert_eq!(parse_callback(&get("/?foo=bar"), "abc"), Callback::Other);
        assert_eq!(parse_callback("garbage", "abc"), Callback::Other);
    }

    #[tokio::test]
    async fn test_listener_skips_stray_requests_until_the_redirect() {
        let auth = GoogleAuth::new(GoogleConfig::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = auth.state.clone();
        let client = tokio::spawn(async move {
            use tokio::net::TcpStream;
            for path in ["/favicon.ico".to_string(), format!("/?state={}&code=the-code", state)] {
                let mut s = TcpStream::connect(addr).await.unwrap();
                s.write_all(get(&path).as_bytes()).await.unwrap();
                let mut resp = String::new();
                s.read_to_string(&mut resp).await.unwrap();
                assert!(resp.starts_with("HTTP/1.1"), "{resp}");
            }
        });
        let code = auth.wait_for_code(&listener).await.unwrap();
        client.await.unwrap();
        assert_eq!(code, "the-code");
    }

    #[test]
    fn test_only_rejected_grants_sign_out() {
        use reqwest::StatusCode;
        use crate::sources::is_auth_failure;
        assert!(is_auth_failure(&refresh_error(StatusCode::BAD_REQUEST, "invalid_grant".into())));
        assert!(is_auth_failure(&refresh_error(StatusCode::UNAUTHORIZED, String::new())));
        assert!(!is_auth_failure(&refresh_error(StatusCode::SERVICE_UNAVAILABLE, String::new())));
        assert!(!is_auth_failure(&refresh_error(StatusCode::TOO_MANY_REQUESTS, String::new())));
    }

    #[test]
    fn test_states_are_unique() {
        assert_ne!(random_state(), random_state());
        assert_eq!(random_state().len(), 32);
    }
}
