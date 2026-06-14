//! Sendspin wire-level JSON message types.
//!
//! Two shapes exist in the spec:
//!
//! 1. **Envelope messages** with `type` + `payload`: `server/hello`,
//!    `stream/start`, `stream/end`, `stream/clear`, `server/state`,
//!    `server/command`, `client/command`, `client/state`, `client/goodbye`,
//!    `stream/request-format`, `group/update`, `client/hello`.
//! 2. **Flat messages** without a `payload` wrapper: `client/time`,
//!    `server/time`.
//!
//! The Python server is loose about this in places (e.g. `server/hello` and
//! `client/hello` may appear either way depending on the library), so this
//! crate supports both shapes for maximum interop with `aiosendspin` 6.x and
//! the JS client library.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::roles::AudioFormat;

// -- Common payloads --------------------------------------------------------

/// `device_info` sub-object shared by both client and server hellos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct DeviceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}

/// One entry in `player@v1_support.supported_formats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupportedAudioFormat {
    pub codec: String,
    pub channels: u8,
    pub sample_rate: u32,
    pub bit_depth: u16,
}

/// `player@v1_support` payload in `client/hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct PlayerSupportV1 {
    pub supported_formats: Vec<SupportedAudioFormat>,
    pub buffer_capacity: u32,
    pub supported_commands: Vec<String>,
}

/// Artwork channel spec (in `artwork@v1_support.channels`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtworkChannelSpec {
    pub source: String,
    pub format: String,
    pub media_width: u32,
    pub media_height: u32,
}

/// `artwork@v1_support` payload in `client/hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ArtworkSupportV1 {
    pub channels: Vec<ArtworkChannelSpec>,
}

/// `visualizer@v1_support` payload in `client/hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct VisualizerSupportV1 {
    pub types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_max: Option<u32>,
}

// -- client/hello -----------------------------------------------------------

/// Payload of the `client/hello` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientHello {
    pub client_id: String,
    pub name: String,
    pub version: u32,
    pub supported_roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_info: Option<DeviceInfo>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "player@v1_support"
    )]
    pub player_v1_support: Option<PlayerSupportV1>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "artwork@v1_support"
    )]
    pub artwork_v1_support: Option<ArtworkSupportV1>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "visualizer@v1_support"
    )]
    pub visualizer_v1_support: Option<VisualizerSupportV1>,
}

// -- server/hello -----------------------------------------------------------

/// Payload of the `server/hello` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ServerHello {
    pub server_id: String,
    pub name: String,
    pub version: u32,
    pub active_roles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_reason: Option<ConnectionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionReason {
    Discovery,
    Playback,
}

// -- client/time / server/time (flat) ---------------------------------------

/// `client/time` — flat message (no `payload` wrapper).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientTime {
    #[serde(rename = "client_transmitted")]
    pub client_transmitted: i64,
}

/// `server/time` — flat message (no `payload` wrapper).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerTime {
    pub client_transmitted: i64,
    pub server_received: i64,
    pub server_transmitted: i64,
}

// -- client/state -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    Synchronized,
    Error,
    ExternalSource,
}

/// Player state sub-object in `client/state`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ClientPlayerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    pub static_delay_ms: u32,
    pub required_lead_time_ms: u32,
    pub min_buffer_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_commands: Option<Vec<String>>,
}

/// Payload of the `client/state` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientState {
    pub state: SyncState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<ClientPlayerState>,
}

// -- client/command ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientControllerCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
}

/// Payload of the `client/command` envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ClientCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<ClientControllerCommand>,
}

// -- client/goodbye --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoodbyeReason {
    AnotherServer,
    Shutdown,
    Restart,
    UserRequest,
}

/// Payload of the `client/goodbye` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientGoodbye {
    pub reason: GoodbyeReason,
}

// -- server/state -----------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct MetadataState {
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<MetadataProgress>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct MetadataProgress {
    pub track_progress: i64,
    pub track_duration: i64,
    pub playback_speed: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ControllerState {
    pub supported_commands: Vec<String>,
    pub volume: u8,
    pub muted: bool,
    #[serde(default)]
    pub repeat: String,
    #[serde(default)]
    pub shuffle: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ColorState {
    /// RGB triplet 0..=255 for each entry, e.g. `[255, 0, 64]`.
    pub rgb: [u8; 3],
}

/// Payload of the `server/state` envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ServerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<ControllerState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorState>,
}

// -- server/command ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ServerPlayerCommand {
    pub command: String, // "volume" | "mute" | "set_static_delay"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_delay_ms: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ServerCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<ServerPlayerCommand>,
}

// -- stream/start -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamStartPlayer {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamStartArtworkChannel {
    pub source: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StreamStartArtwork {
    pub channels: Vec<StreamStartArtworkChannel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamStartVisualizerSpectrum {
    pub n_disp_bins: u32,
    pub scale: String,
    pub f_min: u32,
    pub f_max: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StreamStartVisualizer {
    pub types: Vec<String>,
    pub rate_max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks_downbeats: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<StreamStartVisualizerSpectrum>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StreamStart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<StreamStartPlayer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<StreamStartArtwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visualizer: Option<StreamStartVisualizer>,
}

impl StreamStart {
    /// Convenience constructor for a `stream/start` that only configures the
    /// player role with the given audio format. `codec_header` (e.g. FLAC
    /// `fLaC` marker) is base64-encoded as required by the spec.
    pub fn player(format: AudioFormat, codec_header_b64: Option<String>) -> Self {
        let player = match format {
            AudioFormat::Opus {
                sample_rate,
                channels,
                bit_depth,
            } => StreamStartPlayer {
                codec: "opus".into(),
                sample_rate,
                channels,
                bit_depth,
                codec_header: None,
            },
            AudioFormat::Flac {
                sample_rate,
                channels,
                bit_depth,
                ..
            } => StreamStartPlayer {
                codec: "flac".into(),
                sample_rate,
                channels,
                bit_depth,
                codec_header: codec_header_b64,
            },
            AudioFormat::Pcm {
                sample_rate,
                channels,
                bit_depth,
                sample_type,
            } => StreamStartPlayer {
                codec: match sample_type {
                    super::roles::PcmSampleType::Int => "pcm".into(),
                    super::roles::PcmSampleType::Float => "pcm".into(),
                },
                sample_rate,
                channels,
                bit_depth,
                codec_header: None,
            },
        };
        Self {
            player: Some(player),
            artwork: None,
            visualizer: None,
        }
    }
}

// -- stream/clear / stream/end ---------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StreamClear {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StreamEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

// -- stream/request-format --------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct RequestFormat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<FormatRequestPlayer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<FormatRequestArtwork>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visualizer: Option<FormatRequestVisualizer>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct FormatRequestPlayer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct FormatRequestArtwork {
    pub channel: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_height: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct FormatRequestVisualizer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_max: Option<u32>,
}

// -- group/update -----------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct GroupUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

// -- Envelope helpers -------------------------------------------------------

/// Build a JSON envelope message `{"type": "...", "payload": ...}`.
pub fn envelope<T: Serialize>(msg_type: &str, payload: T) -> Result<Value, serde_json::Error> {
    Ok(serde_json::json!({
        "type": msg_type,
        "payload": payload,
    }))
}

/// Discriminate between known Sendspin message types based on the `type` field
/// of an inbound JSON object. Unknown types come back as `MessageKind::Other`.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageKind {
    ClientHello,
    ServerHello,
    ClientTime,
    ServerTime,
    ClientState,
    ClientCommand,
    ClientGoodbye,
    ServerState,
    ServerCommand,
    StreamStart,
    StreamEnd,
    StreamClear,
    StreamRequestFormat,
    GroupUpdate,
    Other(String),
}

impl MessageKind {
    pub fn from_type(s: &str) -> Self {
        match s {
            "client/hello" => Self::ClientHello,
            "server/hello" => Self::ServerHello,
            "client/time" => Self::ClientTime,
            "server/time" => Self::ServerTime,
            "client/state" => Self::ClientState,
            "client/command" => Self::ClientCommand,
            "client/goodbye" => Self::ClientGoodbye,
            "server/state" => Self::ServerState,
            "server/command" => Self::ServerCommand,
            "stream/start" => Self::StreamStart,
            "stream/end" => Self::StreamEnd,
            "stream/clear" => Self::StreamClear,
            "stream/request-format" => Self::StreamRequestFormat,
            "group/update" => Self::GroupUpdate,
            other => Self::Other(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_hello_round_trip() {
        let hello = ClientHello {
            client_id: "abc".into(),
            name: "Speaker".into(),
            version: 1,
            supported_roles: vec!["player@v1".into()],
            device_info: Some(DeviceInfo {
                product_name: Some("Pi".into()),
                ..Default::default()
            }),
            player_v1_support: Some(PlayerSupportV1 {
                supported_formats: vec![SupportedAudioFormat {
                    codec: "opus".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_depth: 16,
                }],
                buffer_capacity: 1_048_576,
                supported_commands: vec!["volume".into(), "mute".into()],
            }),
            artwork_v1_support: None,
            visualizer_v1_support: None,
        };
        let j = serde_json::to_value(&hello).unwrap();
        assert_eq!(j["client_id"], "abc");
        assert_eq!(j["supported_roles"][0], "player@v1");
        assert_eq!(j["player@v1_support"]["buffer_capacity"], 1_048_576);
        let back: ClientHello = serde_json::from_value(j).unwrap();
        assert_eq!(back, hello);
    }

    #[test]
    fn server_hello_with_connection_reason() {
        let hello = ServerHello {
            server_id: "srv1".into(),
            name: "MA Rust".into(),
            version: 1,
            active_roles: vec!["player@v1".into()],
            connection_reason: Some(ConnectionReason::Playback),
        };
        let j = serde_json::to_value(&hello).unwrap();
        assert_eq!(j["connection_reason"], "playback");
    }

    #[test]
    fn envelope_wraps_payload() {
        let j = envelope(
            "server/hello",
            &ServerHello {
                server_id: "s".into(),
                name: "n".into(),
                version: 1,
                active_roles: vec![],
                connection_reason: None,
            },
        )
        .unwrap();
        assert_eq!(j["type"], "server/hello");
        assert_eq!(j["payload"]["server_id"], "s");
    }

    #[test]
    fn client_time_flat() {
        let ct = ClientTime {
            client_transmitted: 1234,
        };
        let j = serde_json::to_value(&ct).unwrap();
        assert!(j.get("client_transmitted").is_some());
        assert!(j.get("payload").is_none());
    }

    #[test]
    fn message_kind_known_and_unknown() {
        assert_eq!(
            MessageKind::from_type("client/hello"),
            MessageKind::ClientHello
        );
        assert_eq!(
            MessageKind::from_type("weird/message"),
            MessageKind::Other("weird/message".into())
        );
    }
}
