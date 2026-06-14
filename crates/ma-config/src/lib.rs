//! `ma-config` — configuration loading.
//!
//! Phase 0 stub: deserialize a minimal struct from a TOML file with envvar
//! overrides. Real config schema is built up in subsequent phases.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_BIND_IP: &str = "0.0.0.0";
pub const DEFAULT_BIND_PORT: u16 = 8095;
pub const DEFAULT_STREAM_PORT: u16 = 8097;
pub const DEFAULT_SENDSPIN_INBOUND_PORT: u16 = 8927;
pub const DEFAULT_SENDSPIN_OUTBOUND_PORT: u16 = 8928;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind_ip: String,
    pub bind_port: u16,
    pub stream_port: u16,
    pub base_url: String,
    pub allowed_origins: Vec<String>,
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_ip: DEFAULT_BIND_IP.to_string(),
            bind_port: DEFAULT_BIND_PORT,
            stream_port: DEFAULT_STREAM_PORT,
            base_url: format!("http://localhost:{DEFAULT_BIND_PORT}"),
            allowed_origins: vec!["*".to_string()],
            log_level: "info".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SendspinConfig {
    pub enabled: bool,
    pub inbound_port: u16,
    pub outbound_port: u16,
    pub server_name: String,
    pub bind_ip: String,
}

impl Default for SendspinConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            inbound_port: DEFAULT_SENDSPIN_INBOUND_PORT,
            outbound_port: DEFAULT_SENDSPIN_OUTBOUND_PORT,
            server_name: "Music Assistant (Rust)".to_string(),
            bind_ip: DEFAULT_BIND_IP.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MassConfig {
    pub server: ServerConfig,
    pub sendspin: SendspinConfig,
    /// Path to the data directory (database, cache).
    pub data_dir: String,
    /// Path to the cache directory (covers, librespot binary).
    pub cache_dir: String,
}

impl MassConfig {
    pub fn load_from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let cfg: MassConfig = toml::from_str(&raw)?;
        Ok(cfg)
    }

    /// Apply `MA_*` environment variable overrides.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("MA_BIND_IP") {
            self.server.bind_ip = v;
        }
        if let Ok(v) = std::env::var("MA_BIND_PORT") {
            if let Ok(p) = v.parse() {
                self.server.bind_port = p;
            }
        }
        if let Ok(v) = std::env::var("MA_LOG_LEVEL") {
            self.server.log_level = v;
        }
        if let Ok(v) = std::env::var("MA_BASE_URL") {
            self.server.base_url = v;
        }
        if let Ok(v) = std::env::var("MA_DATA_DIR") {
            self.data_dir = v;
        }
        if let Ok(v) = std::env::var("MA_CACHE_DIR") {
            self.cache_dir = v;
        }
    }

    pub fn data_dir(&self) -> std::path::PathBuf {
        if self.data_dir.is_empty() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".musicassistant")
        } else {
            std::path::PathBuf::from(&self.data_dir)
        }
    }

    pub fn cache_dir(&self) -> std::path::PathBuf {
        if self.cache_dir.is_empty() {
            self.data_dir().join("cache")
        } else {
            std::path::PathBuf::from(&self.cache_dir)
        }
    }
}

pub mod toml {
    use serde::de::DeserializeOwned;
    use std::fmt;

    #[derive(Debug)]
    pub struct Error(pub String);

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Error {}

    pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T, Error> {
        ::toml::from_str(s).map_err(|e| Error(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = MassConfig::default();
        assert_eq!(c.server.bind_port, 8095);
        assert_eq!(c.sendspin.inbound_port, 8927);
        assert!(c.sendspin.enabled);
    }

    #[test]
    fn env_overrides() {
        std::env::set_var("MA_BIND_PORT", "9999");
        let mut c = MassConfig::default();
        c.apply_env_overrides();
        assert_eq!(c.server.bind_port, 9999);
        std::env::remove_var("MA_BIND_PORT");
    }
}
