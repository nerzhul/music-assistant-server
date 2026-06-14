//! Stream details returned by a provider when the player wants to play
//! an item. The shape mirrors the Python `StreamDetails` minus the
//! per-provider-specific cache categorisation.

use ma_core::enums::{ContentType, StreamType};
use serde::{Deserialize, Serialize};

use ma_core::identifiers::MediaItemId;

/// One part of a multi-part stream (e.g. HLS segments, or chaptered
/// audiobooks). Phase 1 only ever populates a single part; the
/// structure is here for the Phase 2 work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MultiPartPath {
    pub path: String,
    pub mime_type: Option<String>,
    pub duration: Option<f64>,
    pub byte_size: Option<u64>,
    /// Optional decryption key for `EncryptedHttp` streams.
    pub decryption_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamDetails {
    /// Provider instance id that produced these details.
    pub provider: String,
    pub item_id: MediaItemId,
    /// `music` / `radio` / `podcast_episode` / `audiobook` / ...
    pub media_type: ma_core::enums::MediaType,
    /// `http` / `hls` / `icy` / `local_file` / ...
    pub stream_type: StreamType,
    /// Output PCM format the server should normalise to. Providers may
    /// leave this empty when the source format is unknown; the stream
    /// controller will pick a sensible default.
    pub audio_format: Option<StreamAudioFormat>,
    /// The path / URL / file path the stream controller will read.
    pub path: String,
    /// If the path is a multi-part stream, each segment is here.
    #[serde(default)]
    pub parts: Vec<MultiPartPath>,
    pub duration: Option<f64>,
    pub can_seek: bool,
    /// True when the stream is real-time / live (e.g. radio).
    pub live: bool,
    /// Display title (radio station name, podcast episode, ...).
    pub title: Option<String>,
    /// Optional artist / station for display.
    pub artist: Option<String>,
    /// Optional album / show for display.
    pub album: Option<String>,
}

impl Default for StreamDetails {
    fn default() -> Self {
        Self {
            provider: String::new(),
            item_id: MediaItemId::default(),
            media_type: ma_core::enums::MediaType::Unknown,
            stream_type: StreamType::Unknown,
            audio_format: None,
            path: String::new(),
            parts: Vec::new(),
            duration: None,
            can_seek: true,
            live: false,
            title: None,
            artist: None,
            album: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StreamAudioFormat {
    pub content_type: ContentType,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u8,
    pub bit_rate: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_details_default_is_seekable() {
        let d = StreamDetails::default();
        assert!(d.can_seek);
        assert!(!d.live);
        assert!(d.parts.is_empty());
    }

    #[test]
    fn stream_details_serde_round_trip() {
        let d = StreamDetails {
            provider: "spotify".into(),
            item_id: ma_core::identifiers::MediaItemId("abc".to_string()),
            media_type: ma_core::enums::MediaType::Track,
            stream_type: StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type: ContentType::Mp3,
                sample_rate: 44_100,
                bit_depth: 16,
                channels: 2,
                bit_rate: Some(320),
            }),
            path: "https://example.com/track.mp3".into(),
            duration: Some(180.0),
            can_seek: true,
            live: false,
            title: Some("Track".into()),
            artist: Some("Artist".into()),
            album: None,
            ..Default::default()
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: StreamDetails = serde_json::from_str(&s).unwrap();
        assert_eq!(back.item_id, d.item_id);
        assert_eq!(back.audio_format, d.audio_format);
    }
}
