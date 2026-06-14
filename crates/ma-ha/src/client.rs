//! Thin async client for the Home Assistant REST API.
//!
//! Supports both the long-lived-token and Supervisor-proxy auth
//! modes. WebSocket is *not* used by V1 — the client only does
//! REST polling, which is plenty for the
//! `media_player.*`-discovery use case.
//!
//! Reference: <https://developers.home-assistant.io/docs/api/rest/>

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use crate::config::{HaAuthMode, HaConfig};

#[derive(Debug, Error)]
pub enum HaClientError {
    #[error("home assistant is not configured (set MA_HA_URL + MA_HA_TOKEN)")]
    NotConfigured,
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("home assistant returned status {0}: {1}")]
    HttpStatus(u16, String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("auth error: {0}")]
    Auth(String),
}

pub type Result<T> = std::result::Result<T, HaClientError>;

const DEFAULT_USER_AGENT: &str = concat!(
    "MusicAssistantRust/",
    env!("CARGO_PKG_VERSION"),
    " (https://music-assistant.io)"
);

/// A single Home Assistant entity state. We only model the subset of
/// fields we need for the `media_player` import path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HaEntity {
    pub entity_id: String,
    pub state: String,
    pub attributes: serde_json::Value,
    #[serde(default)]
    pub last_changed: String,
    #[serde(default)]
    pub last_updated: String,
}

impl HaEntity {
    /// Whether the entity represents a `media_player` domain.
    pub fn is_media_player(&self) -> bool {
        self.entity_id.starts_with("media_player.")
    }
}

#[derive(Clone, Debug)]
pub struct HaClient {
    pub base: String,
    pub token: String,
    pub client: reqwest::Client,
    pub verify_ssl: bool,
}

impl HaClient {
    /// Build a `HaClient` from a `HaConfig`. Resolves supervisor
    /// proxy vs direct URL based on `auth_mode()`.
    pub fn from_config(config: &HaConfig) -> Result<Self> {
        match config.auth_mode() {
            HaAuthMode::Supervisor => {
                let base = config
                    .supervisor_url
                    .clone()
                    .ok_or(HaClientError::NotConfigured)?;
                // Supervisor token: try `SUPERVISOR_TOKEN` env, fall
                // back to the configured `token` (most addon
                // installs export both).
                let token = std::env::var("SUPERVISOR_TOKEN")
                    .ok()
                    .or_else(|| config.token.clone())
                    .ok_or_else(|| HaClientError::Auth("SUPERVISOR_TOKEN not set".to_string()))?;
                Ok(Self::new(base, token, true)?)
            }
            HaAuthMode::Token | HaAuthMode::OAuth => {
                let base = config.url.clone().ok_or(HaClientError::NotConfigured)?;
                let token = config
                    .token
                    .clone()
                    .ok_or_else(|| HaClientError::Auth("MA_HA_TOKEN is not set".to_string()))?;
                Ok(Self::new(base, token, config.verify_ssl)?)
            }
        }
    }

    /// Build a `HaClient` from a base URL + token. Mostly useful in
    /// tests.
    pub fn new(
        base: impl Into<String>,
        token: impl Into<String>,
        verify_ssl: bool,
    ) -> Result<Self> {
        let base = base.into();
        let token = token.into();
        if base.trim().is_empty() || token.trim().is_empty() {
            return Err(HaClientError::NotConfigured);
        }
        let client = reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .timeout(Duration::from_secs(20))
            .danger_accept_invalid_certs(!verify_ssl)
            .build()?;
        Ok(Self {
            base,
            token,
            client,
            verify_ssl,
        })
    }

    /// Build a `HaClient` with a custom `reqwest::Client` (for
    /// testing).
    pub fn with_client(
        base: impl Into<String>,
        token: impl Into<String>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            base: base.into(),
            token: token.into(),
            client,
            verify_ssl: true,
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    /// GET `/api/` — returns the HA root config, used to validate
    /// authentication. Errors out with `Auth` if the token is wrong.
    pub async fn ping(&self) -> Result<serde_json::Value> {
        let url = format!("{}/api/", self.base.trim_end_matches('/'));
        let resp = self.auth(self.client.get(&url)).send().await?;
        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(HaClientError::Auth("unauthorized (401)".to_string()));
        }
        if !status.is_success() {
            return Err(HaClientError::HttpStatus(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ));
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(body)
    }

    /// GET `/api/states` — returns the full state list. Caller
    /// filters to `media_player.*` + the include globs.
    pub async fn list_states(&self) -> Result<Vec<HaEntity>> {
        let url = format!("{}/api/states", self.base.trim_end_matches('/'));
        let resp = self.auth(self.client.get(&url)).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HaClientError::HttpStatus(
                status.as_u16(),
                resp.text().await.unwrap_or_default(),
            ));
        }
        let body: Vec<HaEntity> = resp.json().await?;
        Ok(body)
    }

    /// POST `/api/services/<domain>/<service>` with a JSON body.
    /// Used to forward `media_player.play_media`, `turn_on`, etc.
    pub async fn call_service(
        &self,
        domain: &str,
        service: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let url = format!(
            "{}/api/services/{}/{}",
            self.base.trim_end_matches('/'),
            domain,
            service
        );
        let resp = self.auth(self.client.post(&url).json(body)).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            warn!(domain, service, %status, "HA call_service failed: {text}");
            return Err(HaClientError::HttpStatus(status.as_u16(), text));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_config() {
        let err = HaClient::new("", "", true).unwrap_err();
        assert!(matches!(err, HaClientError::NotConfigured));
    }

    #[test]
    fn builds_bearer_request() {
        let c = HaClient::new("http://ha.local:8123", "tok", true).unwrap();
        assert_eq!(c.base, "http://ha.local:8123");
        assert_eq!(c.token, "tok");
        assert!(c.verify_ssl);
    }

    #[test]
    fn haentity_is_media_player() {
        let e = HaEntity {
            entity_id: "media_player.living_room".into(),
            ..Default::default()
        };
        assert!(e.is_media_player());
        let e = HaEntity {
            entity_id: "light.kitchen".into(),
            ..Default::default()
        };
        assert!(!e.is_media_player());
    }
}
