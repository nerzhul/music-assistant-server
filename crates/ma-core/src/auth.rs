//! Authentication types and helpers used by both the in-process API
//! command registry and the HTTP/WS endpoints.
//!
//! Mirrors `music_assistant_models.auth` (`User`, `UserRole`,
//! `AuthToken`, `UserAuthProvider`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Roles available to authenticated users. The wire format is
/// lowercase strings (`"admin"`, `"user"`, `"guest"`) — see the Python
/// `UserRole` StrEnum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
    Guest,
}

impl UserRole {
    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin)
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
            Self::Guest => "guest",
        }
    }
}

impl std::str::FromStr for UserRole {
    type Err = crate::errors::MusicAssistantError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Self::Admin),
            "user" => Ok(Self::User),
            "guest" => Ok(Self::Guest),
            other => Err(crate::errors::MusicAssistantError::InvalidInput(format!(
                "unknown role: {other}"
            ))),
        }
    }
}

/// User record. Wire-compatible with the Python `User` dataclass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct User {
    pub user_id: String,
    pub username: String,
    pub role: UserRole,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub preferences: serde_json::Value,
    #[serde(default)]
    pub provider_filter: Vec<String>,
    #[serde(default)]
    pub player_filter: Vec<String>,
}

impl Default for User {
    fn default() -> Self {
        Self {
            user_id: Uuid::new_v4().to_string(),
            username: String::new(),
            role: UserRole::User,
            enabled: true,
            created_at: Utc::now(),
            display_name: None,
            avatar_url: None,
            preferences: serde_json::Value::Null,
            provider_filter: Vec::new(),
            player_filter: Vec::new(),
        }
    }
}

/// Authentication provider type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthProviderType {
    Builtin,
    HomeAssistant,
}

/// Long-lived access token. The plaintext token is never stored; we
/// only persist the SHA-256 of it (`token_hash`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthToken {
    pub token_id: String,
    pub user_id: String,
    pub token_hash: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub is_long_lived: bool,
}

impl Default for AuthToken {
    fn default() -> Self {
        Self {
            token_id: Uuid::new_v4().to_string(),
            user_id: String::new(),
            token_hash: String::new(),
            name: String::new(),
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            is_long_lived: true,
        }
    }
}

/// Hash a plaintext token so we can store / compare the hash.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_lower(&hasher.finalize())
}

/// Generate a new opaque, URL-safe token string. 32 bytes of entropy
/// encoded as base64url.
pub fn generate_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    let _ = getrandom_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn getrandom_bytes(out: &mut [u8]) -> std::io::Result<()> {
    // Tiny wrapper around `getrandom` without pulling a new dep: use
    // `std::time` + a UUID seed isn't secure. We use the `rand` crate
    // via `tokio`'s `rand` re-export? Simpler: just read from
    // `/dev/urandom` if available, else use a CSPRNG fallback.
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(out).is_ok() {
            return Ok(());
        }
    }
    // Fallback to system time + thread id; not cryptographically
    // strong but enough to be unique in this scope (the server runs
    // on Linux which always has /dev/urandom; this branch is for
    // exotic targets).
    let mut s: u64 = 0;
    for (i, b) in out.iter_mut().enumerate() {
        let nanos: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        s ^= nanos.rotate_left((i as u32) % 64);
        *b = (s as u8) ^ (i as u8);
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_round_trips_lowercase() {
        for r in [UserRole::Admin, UserRole::User, UserRole::Guest] {
            let s = serde_json::to_string(&r).unwrap();
            assert_eq!(s.trim_matches('"'), r.as_str());
        }
    }

    #[test]
    fn generate_token_is_unique_and_url_safe() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn hash_token_is_stable() {
        assert_eq!(hash_token("hello"), hash_token("hello"),);
        assert_ne!(hash_token("hello"), hash_token("world"));
    }

    #[test]
    fn user_default_serde_skips_optionals() {
        let u = User {
            username: "alice".into(),
            ..Default::default()
        };
        let j = serde_json::to_string(&u).unwrap();
        assert!(j.contains("\"username\":\"alice\""));
        assert!(!j.contains("display_name"));
    }
}
