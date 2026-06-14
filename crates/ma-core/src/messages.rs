//! JSON-RPC-like message types used on `/api` and `/ws`.
//!
//! These mirror `music_assistant_models.api` and must remain wire-compatible
//! with the Python server. Field names, types, and casing are part of the API
//! contract used by the Music Assistant frontend.

use crate::enums::{CoreState, EventType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Command sent from client to server (or `server` to client for pings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandMessage {
    pub message_id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
}

/// Result message returned to a `CommandMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessResultMessage {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default)]
    pub partial: bool,
}

/// Error message returned to a `CommandMessage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResultMessage {
    pub message_id: String,
    pub error_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Server-emitted event message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMessage {
    pub event: EventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Welcome message pushed by the server right after a WebSocket connect.
///
/// The Python server uses `sdk_version` as the discriminator in
/// `parse_message`. We must use the exact same JSON shape:
/// `{"server_id": ..., "server_version": ..., "schema_version": ...,
///  "min_supported_schema_version": ..., "base_url": ..., ...}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfoMessage {
    pub server_id: String,
    pub server_version: String,
    pub schema_version: i32,
    pub min_supported_schema_version: i32,
    pub base_url: String,
    #[serde(default)]
    pub homeassistant_addon: bool,
    #[serde(default)]
    pub onboard_done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub status: CoreState,
}

impl ServerInfoMessage {
    pub fn new(
        server_id: impl Into<String>,
        server_version: impl Into<String>,
        schema_version: i32,
        min_supported_schema_version: i32,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            server_id: server_id.into(),
            server_version: server_version.into(),
            schema_version,
            min_supported_schema_version,
            base_url: base_url.into(),
            homeassistant_addon: false,
            onboard_done: true,
            name: Some("Music Assistant (Rust)".into()),
            status: CoreState::Running,
        }
    }
}

/// Discriminated union of all server→client message kinds, identified by JSON
/// fields (mirror of `parse_message` from the Python implementation).
#[derive(Debug, Clone)]
pub enum WireMessage {
    ServerInfo(ServerInfoMessage),
    Event(EventMessage),
    Success(SuccessResultMessage),
    Error(ErrorResultMessage),
    Command(CommandMessage),
}

impl WireMessage {
    pub fn parse(raw: &Value) -> Result<Self, serde_json::Error> {
        if raw.get("sdk_version").is_some() || raw.get("server_id").is_some() {
            return Ok(WireMessage::ServerInfo(serde_json::from_value(
                raw.clone(),
            )?));
        }
        if raw.get("event").is_some() {
            return Ok(WireMessage::Event(serde_json::from_value(raw.clone())?));
        }
        if raw.get("error_code").is_some() {
            return Ok(WireMessage::Error(serde_json::from_value(raw.clone())?));
        }
        if raw.get("result").is_some() {
            return Ok(WireMessage::Success(serde_json::from_value(raw.clone())?));
        }
        Ok(WireMessage::Command(serde_json::from_value(raw.clone())?))
    }
}

/// Helper to build an `ErrorResultMessage`.
pub fn error_message(
    message_id: impl Into<String>,
    code: i32,
    details: impl Into<String>,
) -> ErrorResultMessage {
    ErrorResultMessage {
        message_id: message_id.into(),
        error_code: code,
        details: Some(details.into()),
    }
}

/// Helper to build a `SuccessResultMessage` from any serializable value.
pub fn success_message<T: Serialize>(
    message_id: impl Into<String>,
    result: T,
) -> Result<SuccessResultMessage, serde_json::Error> {
    let value = serde_json::to_value(result)?;
    Ok(SuccessResultMessage {
        message_id: message_id.into(),
        result: Some(value),
        partial: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_shape() {
        let info = ServerInfoMessage::new("server", "0.1.0", 31, 28, "http://localhost:8095");
        let j = serde_json::to_value(&info).unwrap();
        assert_eq!(j["server_id"], "server");
        assert_eq!(j["schema_version"], 31);
        assert_eq!(j["min_supported_schema_version"], 28);
        assert_eq!(j["status"], "running");
    }

    #[test]
    fn parse_message_server_info() {
        let raw = serde_json::json!({
            "server_id": "x",
            "server_version": "0.1",
            "schema_version": 31,
            "min_supported_schema_version": 28,
            "base_url": "http://x",
        });
        let m = WireMessage::parse(&raw).unwrap();
        assert!(matches!(m, WireMessage::ServerInfo(_)));
    }

    #[test]
    fn parse_message_event() {
        let raw = serde_json::json!({"event": "player_added", "object_id": "p1", "data": null});
        let m = WireMessage::parse(&raw).unwrap();
        assert!(matches!(m, WireMessage::Event(_)));
    }

    #[test]
    fn parse_message_error() {
        let raw = serde_json::json!({"message_id": "1", "error_code": 3, "details": "nope"});
        let m = WireMessage::parse(&raw).unwrap();
        assert!(matches!(m, WireMessage::Error(_)));
    }

    #[test]
    fn parse_message_success() {
        let raw = serde_json::json!({"message_id": "1", "result": null, "partial": false});
        let m = WireMessage::parse(&raw).unwrap();
        assert!(matches!(m, WireMessage::Success(_)));
    }

    #[test]
    fn parse_message_command() {
        let raw = serde_json::json!({"message_id": "1", "command": "players/all"});
        let m = WireMessage::parse(&raw).unwrap();
        assert!(matches!(m, WireMessage::Command(_)));
    }
}
