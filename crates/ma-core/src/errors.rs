//! Error types for Music Assistant.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T, E = MusicAssistantError> = std::result::Result<T, E>;

/// Music Assistant error code. Wire-compatible with `MusicAssistantError` from
/// `music_assistant_models.errors`. The numeric code is sent in
/// `ErrorResultMessage.error_code` so the UI can recognize it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    Generic = 0,
    Unsupported = 1,
    InvalidInput = 2,
    NotFound = 3,
    AlreadyExists = 4,
    AuthenticationFailed = 5,
    PermissionDenied = 6,
    Unavailable = 7,
    Timeout = 8,
    InvalidState = 9,
    MediaNotFound = 10,
    ProviderUnavailable = 11,
    PlayerUnavailable = 12,
    PlayerCommandFailed = 13,
    StreamUnavailable = 14,
    InvalidMediaType = 15,
    CommandFailed = 16,
    LoginFailed = 17,
    SetupFailed = 18,
    SetupRequired = 19,
    NotImplemented = 20,
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
            1 => Self::Unsupported,
            2 => Self::InvalidInput,
            3 => Self::NotFound,
            4 => Self::AlreadyExists,
            5 => Self::AuthenticationFailed,
            6 => Self::PermissionDenied,
            7 => Self::Unavailable,
            8 => Self::Timeout,
            9 => Self::InvalidState,
            10 => Self::MediaNotFound,
            11 => Self::ProviderUnavailable,
            12 => Self::PlayerUnavailable,
            13 => Self::PlayerCommandFailed,
            14 => Self::StreamUnavailable,
            15 => Self::InvalidMediaType,
            16 => Self::CommandFailed,
            17 => Self::LoginFailed,
            18 => Self::SetupFailed,
            19 => Self::SetupRequired,
            20 => Self::NotImplemented,
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
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::InvalidInput(_) => ErrorCode::InvalidInput,
            Self::InvalidState(_) => ErrorCode::InvalidState,
            Self::NotImplemented(_) => ErrorCode::NotImplemented,
            Self::Unsupported(_) => ErrorCode::Unsupported,
            Self::Unavailable(_) => ErrorCode::Unavailable,
            Self::Timeout(_) => ErrorCode::Timeout,
            Self::AuthenticationFailed => ErrorCode::AuthenticationFailed,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::MediaNotFound(_) => ErrorCode::MediaNotFound,
            Self::ProviderUnavailable(_) => ErrorCode::ProviderUnavailable,
            Self::PlayerUnavailable(_) => ErrorCode::PlayerUnavailable,
            Self::PlayerCommandFailed(_) => ErrorCode::PlayerCommandFailed,
            Self::StreamUnavailable(_) => ErrorCode::StreamUnavailable,
            Self::SetupRequired(_) => ErrorCode::SetupRequired,
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
        assert_eq!(err.code(), ErrorCode::NotFound);
        assert_eq!(err.code().as_i32(), 3);
        // Make sure ErrorCode serializes as an integer (wire format).
        let v = serde_json::to_value(err.code()).unwrap();
        assert_eq!(v, serde_json::json!(3));
    }
}
