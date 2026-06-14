//! Strongly-typed identifiers used throughout Music Assistant.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// A unique player identifier (e.g. "sendspin", "spotify_connect_xyz").
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlayerId(pub String);

impl PlayerId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for PlayerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for PlayerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A unique queue identifier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QueueId(pub String);

impl QueueId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QueueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A unique media-item identifier (uri, e.g. `library://track/12345`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaItemId(pub String);

impl MediaItemId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for MediaItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A unique provider instance identifier (`domain` or `domain/instance`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderInstanceId(pub String);

impl ProviderInstanceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Universally unique identifier used for ephemeral entities (events, server id, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MassId(pub Uuid);

impl MassId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MassId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MassId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
