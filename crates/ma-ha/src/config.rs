//! Configuration for the Home Assistant integration.

use serde::{Deserialize, Serialize};

/// How we authenticate against Home Assistant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HaAuthMode {
    /// Long-lived access token (recommended for self-hosted HA).
    Token,
    /// Run as a Home Assistant add-on; use the supervisor proxy.
    Supervisor,
    /// Custom OAuth2 bearer (V2 — not implemented yet).
    #[serde(other)]
    OAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaConfig {
    /// Base URL of the Home Assistant REST API, e.g.
    /// `http://homeassistant.local:8123`. Mutually exclusive with
    /// `supervisor_url`.
    pub url: Option<String>,
    /// Long-lived access token.
    pub token: Option<String>,
    /// URL of the supervisor proxy (`http://supervisor`). Mutually
    /// exclusive with `url`.
    pub supervisor_url: Option<String>,
    /// Whether to verify TLS.
    pub verify_ssl: bool,
    /// Discovery mode: `auto` (default), `rest`, or `websocket`.
    pub discovery_mode: String,
    /// Glob patterns to include (empty = all `media_player.*`).
    pub include_entity_id_globs: Vec<String>,
    /// Whether to import HA players into MA (the V1 direction).
    pub import_players: bool,
    /// Whether to expose MA players back to HA (V2 direction).
    pub expose_players: bool,
}

impl Default for HaConfig {
    fn default() -> Self {
        Self {
            url: std::env::var("MA_HA_URL").ok().filter(|s| !s.is_empty()),
            token: std::env::var("MA_HA_TOKEN").ok().filter(|s| !s.is_empty()),
            supervisor_url: std::env::var("MA_SUPERVISOR_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            verify_ssl: std::env::var("MA_HA_VERIFY_SSL")
                .ok()
                .and_then(|s| s.parse::<bool>().ok())
                .unwrap_or(true),
            discovery_mode: std::env::var("MA_HA_DISCOVERY_MODE").unwrap_or_else(|_| "auto".into()),
            include_entity_id_globs: std::env::var("MA_HA_INCLUDE_GLOBS")
                .ok()
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            import_players: std::env::var("MA_HA_IMPORT_PLAYERS")
                .ok()
                .and_then(|s| s.parse::<bool>().ok())
                .unwrap_or(true),
            expose_players: std::env::var("MA_HA_EXPOSE_PLAYERS")
                .ok()
                .and_then(|s| s.parse::<bool>().ok())
                .unwrap_or(false),
        }
    }
}

impl HaConfig {
    /// Which auth mode to use. Auto-detected from env in V1.
    pub fn auth_mode(&self) -> HaAuthMode {
        if self.supervisor_url.is_some() {
            HaAuthMode::Supervisor
        } else {
            HaAuthMode::Token // default; `HaClient::new` will return an error
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_token_mode() {
        let c = HaConfig::default();
        // The auth mode is `Token` if `token` is set, otherwise
        // `Token` too (the client will fail with a clear error).
        assert_eq!(c.auth_mode(), HaAuthMode::Token);
    }
}
