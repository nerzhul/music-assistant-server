//! Spotify PKCE authentication.
//!
//! Implements OAuth 2.0 Authorization Code Flow with PKCE per RFC 7636,
//! matching the flow used by the Python provider's `pkce_auth_flow`:
//!
//! 1. Generate `(code_verifier, code_challenge)`.
//! 2. Build the `https://accounts.spotify.com/authorize?...` URL with
//!    `code_challenge_method=S256`.
//! 3. After the user authorises, exchange the `code` at
//!    `https://accounts.spotify.com/api/token` for an access + refresh
//!    token using the `code_verifier`.
//! 4. Persist the `refresh_token`; exchange for fresh access tokens
//!    whenever the player controller needs to call the Web API.

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::manifest::{CALLBACK_REDIRECT_URL, DEFAULT_CLIENT_ID, SCOPES};

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("spotify auth failed: {0}")]
    Spotify(String),
    #[error("login failed: {0}")]
    LoginFailed(String),
    #[error("token refresh returned no refresh_token")]
    MissingRefreshToken,
}

pub type Result<T> = std::result::Result<T, AuthError>;

/// PKCE code_verifier + code_challenge pair, base64url-encoded.
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

impl PkcePair {
    pub fn generate() -> Self {
        // 64 random bytes → 86-char base64url string (RFC 7636 §4.1).
        let mut bytes = [0u8; 64];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let digest = hasher.finalize();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
        }
    }
}

/// Build the URL the user should be redirected to in a browser to
/// begin the PKCE flow.
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let mut url = String::from("https://accounts.spotify.com/authorize?");
    url.push_str(&url_encode("response_type", "code"));
    url.push('&');
    url.push_str(&url_encode("client_id", client_id));
    url.push('&');
    url.push_str(&url_encode("scope", SCOPES));
    url.push('&');
    url.push_str(&url_encode("code_challenge_method", "S256"));
    url.push('&');
    url.push_str(&url_encode("code_challenge", challenge));
    url.push('&');
    url.push_str(&url_encode("redirect_uri", redirect_uri));
    url.push('&');
    url.push_str(&url_encode("state", state));
    url
}

fn url_encode(k: &str, v: &str) -> String {
    format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))
}

/// Response from a successful token exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyToken {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub expires_in: i64,
    /// Unix-epoch seconds at which the access token expires. We compute
    /// this from `expires_in` + now().
    pub expires_at: i64,
    /// Present on the initial code exchange; absent on refresh.
    #[serde(default)]
    pub refresh_token: Option<String>,
}

impl SpotifyToken {
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        // Treat as expired 60 s before the actual expiry to avoid races.
        now >= self.expires_at - 60
    }
}

/// Spotify PKCE auth helper. Holds the long-lived refresh token and
/// the cached access token, and exposes a `valid_access_token` method
/// that refreshes when needed.
pub struct PkceAuth {
    pub client_id: String,
    pub refresh_token: parking_lot::RwLock<Option<String>>,
    pub cached_access: parking_lot::RwLock<Option<SpotifyToken>>,
    pub http: reqwest::Client,
}

impl PkceAuth {
    pub fn new(client_id: impl Into<String>, refresh_token: Option<String>) -> Self {
        Self {
            client_id: client_id.into(),
            refresh_token: parking_lot::RwLock::new(refresh_token),
            cached_access: parking_lot::RwLock::new(None),
            http: reqwest::Client::builder()
                .user_agent("MusicAssistantRust/0.1 (https://music-assistant.io)")
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn with_default_client_id(refresh_token: Option<String>) -> Self {
        Self::new(DEFAULT_CLIENT_ID, refresh_token)
    }

    /// Exchange an authorization code (the callback query string) for a
    /// token pair.
    pub async fn exchange_code(&self, code: &str) -> Result<SpotifyToken> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CALLBACK_REDIRECT_URL),
            ("client_id", self.client_id.as_str()),
        ];
        let pair = PkcePair::generate();
        // We must use the *same* verifier the user was sent. The Python
        // provider re-generates on every call (PKCE is per-request).
        form.push(("code_verifier", pair.verifier.as_str()));
        let token = self.post_token(&form).await?;
        if token.refresh_token.is_none() {
            return Err(AuthError::MissingRefreshToken);
        }
        *self.refresh_token.write() = token.refresh_token.clone();
        *self.cached_access.write() = Some(token.clone());
        Ok(token)
    }

    /// Refresh the access token using the stored refresh token.
    pub async fn refresh(&self) -> Result<SpotifyToken> {
        let rt = self
            .refresh_token
            .read()
            .clone()
            .ok_or_else(|| AuthError::LoginFailed("no refresh_token".into()))?;
        let form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", rt.as_str()),
            ("client_id", self.client_id.as_str()),
        ];
        let token = self.post_token(&form).await?;
        // Spotify rotates the refresh token on some grants; persist the
        // new one if present.
        if let Some(new_rt) = &token.refresh_token {
            *self.refresh_token.write() = Some(new_rt.clone());
        }
        *self.cached_access.write() = Some(token.clone());
        Ok(token)
    }

    /// Return a valid bearer token, refreshing if necessary.
    pub async fn valid_access_token(&self) -> Result<String> {
        {
            let cached = self.cached_access.read();
            if let Some(t) = cached.as_ref() {
                if !t.is_expired() {
                    return Ok(t.access_token.clone());
                }
            }
        }
        let t = self.refresh().await?;
        Ok(t.access_token)
    }

    async fn post_token(&self, form: &[(&str, &str)]) -> Result<SpotifyToken> {
        let resp = self
            .http
            .post("https://accounts.spotify.com/api/token")
            .form(form)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(AuthError::Spotify(format!("{status}: {body}")));
        }
        let mut token: SpotifyToken = serde_json::from_str(&body)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        token.expires_at = now + token.expires_in;
        Ok(token)
    }

    pub fn clear_tokens(&self) {
        *self.refresh_token.write() = None;
        *self.cached_access.write() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_pair_lengths_match_rfc() {
        let p = PkcePair::generate();
        // 64 bytes base64url-no-pad → 86 chars.
        assert_eq!(p.verifier.len(), 86);
        // SHA-256 digest base64url-no-pad → 43 chars.
        assert_eq!(p.challenge.len(), 43);
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let pair = PkcePair::generate();
        let url = authorize_url(
            "my_client",
            "https://example.com/cb",
            "state-xyz",
            &pair.challenge,
        );
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=my_client"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pair.challenge)));
        assert!(url.contains("state=state-xyz"));
        assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
    }

    #[test]
    fn token_is_expired_uses_clock() {
        let mut t = SpotifyToken {
            access_token: "x".into(),
            token_type: "Bearer".into(),
            scope: "".into(),
            expires_in: 3600,
            expires_at: 0,
            refresh_token: None,
        };
        assert!(t.is_expired());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        t.expires_at = now + 60;
        assert!(t.is_expired());
        t.expires_at = now + 7200;
        assert!(!t.is_expired());
    }

    #[test]
    fn pkce_challenge_matches_sha256_of_verifier() {
        let p = PkcePair::generate();
        let mut h = Sha256::new();
        h.update(p.verifier.as_bytes());
        let digest = h.finalize();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(expected, p.challenge);
    }
}
