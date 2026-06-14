//! Error types for Music Assistant.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T, E = MusicAssistantError> = std::result::Result<T, E>;

/// Convenience alias for `MusicAssistantError` so call sites can
/// write `ma_core::errors::Error` instead of the long name.
pub type Error = MusicAssistantError;

/// Music Assistant error code. Wire-compatible with `MusicAssistantError` from
/// `music_assistant_models.errors` (https://github.com/music-assistant/server).
/// The numeric code is sent in `ErrorResultMessage.error_code` so the UI can
/// recognize it. Codes match the Python `error_code` class attribute values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    /// Catch-all (parent `MusicAssistantError` class in Python).
    Generic = 0,
    /// `ProviderUnavailableError`
    ProviderUnavailable = 1,
    /// `MediaNotFoundError`
    MediaNotFound = 2,
    /// `InvalidDataError`
    InvalidData = 3,
    /// `AlreadyRegisteredError`
    AlreadyRegistered = 4,
    /// `SetupFailedError`
    SetupFailed = 5,
    /// `LoginFailed`
    LoginFailed = 6,
    /// `AudioError`
    Audio = 7,
    /// `QueueEmpty`
    QueueEmpty = 8,
    /// `UnsupportedFeaturedException`
    UnsupportedFeature = 9,
    /// `PlayerUnavailableError`
    PlayerUnavailable = 10,
    /// `PlayerCommandFailed`
    PlayerCommandFailed = 11,
    /// `InvalidCommand`
    InvalidCommand = 12,
    /// `UnplayableMediaError`
    UnplayableMedia = 13,
    /// `InvalidProviderURI`
    InvalidProviderUri = 14,
    /// `InvalidProviderID`
    InvalidProviderId = 15,
    /// `RetriesExhausted`
    RetriesExhausted = 16,
    /// `ResourceTemporarilyUnavailable`
    ResourceTemporarilyUnavailable = 17,
    /// `ProviderPermissionDenied`
    ProviderPermissionDenied = 18,
    /// `ActionUnavailable`
    ActionUnavailable = 19,
    /// `AuthenticationRequired`
    AuthenticationRequired = 20,
    /// `AuthenticationFailed`
    AuthenticationFailed = 21,
    /// `InsufficientPermissions`
    InsufficientPermissions = 22,
    /// `InvalidToken`
    InvalidToken = 23,
    /// `ResourceBusyError`
    ResourceBusy = 24,
    /// `RateLimited`
    RateLimited = 25,
    /// Convenience alias: command failed (Python uses 0 for the parent class).
    CommandFailed = 999,
    /// Convenience alias: a feature the user requested is not implemented.
    NotImplemented = 998,
}

impl ErrorCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i32(*self as i32)
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = i32::deserialize(deserializer)?;
        Ok(match v {
            0 => Self::Generic,
            1 => Self::ProviderUnavailable,
            2 => Self::MediaNotFound,
            3 => Self::InvalidData,
            4 => Self::AlreadyRegistered,
            5 => Self::SetupFailed,
            6 => Self::LoginFailed,
            7 => Self::Audio,
            8 => Self::QueueEmpty,
            9 => Self::UnsupportedFeature,
            10 => Self::PlayerUnavailable,
            11 => Self::PlayerCommandFailed,
            12 => Self::InvalidCommand,
            13 => Self::UnplayableMedia,
            14 => Self::InvalidProviderUri,
            15 => Self::InvalidProviderId,
            16 => Self::RetriesExhausted,
            17 => Self::ResourceTemporarilyUnavailable,
            18 => Self::ProviderPermissionDenied,
            19 => Self::ActionUnavailable,
            20 => Self::AuthenticationRequired,
            21 => Self::AuthenticationFailed,
            22 => Self::InsufficientPermissions,
            23 => Self::InvalidToken,
            24 => Self::ResourceBusy,
            25 => Self::RateLimited,
            998 => Self::NotImplemented,
            999 => Self::CommandFailed,
            _ => Self::Generic,
        })
    }
}

/// Top-level error type returned to API callers and logged internally.
#[derive(Debug, Error)]
pub enum MusicAssistantError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("unsupported: {0}")]
    Unsupported(&'static str),

    #[error("unavailable: {0}")]
    Unavailable(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("authentication failed")]
    AuthenticationFailed,

    #[error("permission denied")]
    PermissionDenied,

    #[error("media not found: {0}")]
    MediaNotFound(String),

    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("player unavailable: {0}")]
    PlayerUnavailable(String),

    #[error("player command failed: {0}")]
    PlayerCommandFailed(String),

    #[error("stream unavailable: {0}")]
    StreamUnavailable(String),

    #[error("resource busy: {0}")]
    ResourceBusy(String),

    #[error("setup required: {0}")]
    SetupRequired(String),

    #[error("command failed: {0}")]
    CommandFailed(String),

    #[error("setup failed: {0}")]
    SetupFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Database(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("{0}")]
    Other(String),
}

impl MusicAssistantError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::MediaNotFound,
            Self::InvalidInput(_) => ErrorCode::InvalidData,
            Self::InvalidState(_) => ErrorCode::InvalidCommand,
            Self::NotImplemented(_) => ErrorCode::NotImplemented,
            Self::Unsupported(_) => ErrorCode::UnsupportedFeature,
            Self::Unavailable(_) => ErrorCode::ResourceTemporarilyUnavailable,
            Self::Timeout(_) => ErrorCode::ResourceTemporarilyUnavailable,
            Self::AuthenticationFailed => ErrorCode::AuthenticationFailed,
            Self::PermissionDenied => ErrorCode::InsufficientPermissions,
            Self::MediaNotFound(_) => ErrorCode::MediaNotFound,
            Self::ProviderUnavailable(_) => ErrorCode::ProviderUnavailable,
            Self::PlayerUnavailable(_) => ErrorCode::PlayerUnavailable,
            Self::PlayerCommandFailed(_) => ErrorCode::PlayerCommandFailed,
            Self::StreamUnavailable(_) => ErrorCode::ResourceTemporarilyUnavailable,
            Self::ResourceBusy(_) => ErrorCode::ResourceBusy,
            Self::SetupRequired(_) => ErrorCode::AuthenticationRequired,
            Self::CommandFailed(_) => ErrorCode::CommandFailed,
            Self::SetupFailed(_) => ErrorCode::SetupFailed,
            _ => ErrorCode::Generic,
        }
    }

    pub fn details(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl From<anyhow::Error> for MusicAssistantError {
    fn from(value: anyhow::Error) -> Self {
        Self::Internal(value.to_string())
    }
}

impl From<sqlx::Error> for MusicAssistantError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value.to_string())
    }
}

impl From<reqwest::Error> for MusicAssistantError {
    fn from(value: reqwest::Error) -> Self {
        Self::Other(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_round_trip() {
        let err = MusicAssistantError::NotFound("track".into());
        assert_eq!(err.code(), ErrorCode::MediaNotFound);
        assert_eq!(err.code().as_i32(), 2);
        // Make sure ErrorCode serializes as an integer (wire format).
        let v = serde_json::to_value(err.code()).unwrap();
        assert_eq!(v, serde_json::json!(2));
    }
}
