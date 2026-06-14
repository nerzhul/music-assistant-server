//! MA `Player` impl that wraps a Home Assistant `media_player.*`
//! entity.
//!
//! The wrapper mirrors HA's state into MA every time a new snapshot
//! arrives on the [`Discover`] channel. Commands issued through the
//! MA controller (play/pause/volume) are forwarded back to HA via
//! `call_service`.
//!
//! The `PlayerControl` trait has sync `state()` (so it can be
//! snapshotted from any thread) and async command methods. We use
//! `tokio::task::block_in_place` to call the async `call_service`
//! from the command methods; this requires a multi-thread tokio
//! runtime, which `ma-server` already runs on.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::warn;

use ma_core::enums::{PlaybackState, PlayerFeature, PlayerType};
use ma_core::player::{Player, PlayerControl};
use ma_core::Result as MaResult;

use crate::client::{HaClient, HaEntity};

/// Per-entity state held in the registry. The `last_known_state`
/// is updated each time a new snapshot arrives.
#[derive(Debug, Clone)]
pub struct HaPlayerSnapshot {
    pub state: String,
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub media_title: Option<String>,
    pub media_artist: Option<String>,
    pub media_album: Option<String>,
    pub media_content_id: Option<String>,
    pub media_content_type: Option<String>,
    pub entity_picture: Option<String>,
    pub available: bool,
    pub updated_at: std::time::Instant,
}

impl Default for HaPlayerSnapshot {
    fn default() -> Self {
        Self {
            state: "off".into(),
            volume: None,
            muted: None,
            media_title: None,
            media_artist: None,
            media_album: None,
            media_content_id: None,
            media_content_type: None,
            entity_picture: None,
            available: false,
            updated_at: std::time::Instant::now(),
        }
    }
}

impl HaPlayerSnapshot {
    /// Build a snapshot from a fresh HA entity. The mapping mirrors
    /// the Python `hass_players` provider.
    pub fn from_entity(e: &HaEntity) -> Self {
        let attrs = e.attributes.as_object();
        let get_str = |k: &str| -> Option<String> {
            attrs
                .and_then(|a| a.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let get_f32 = |k: &str| -> Option<f32> {
            attrs
                .and_then(|a| a.get(k))
                .and_then(|v| v.as_f64())
                .map(|x| x as f32)
        };
        let get_bool =
            |k: &str| -> Option<bool> { attrs.and_then(|a| a.get(k)).and_then(|v| v.as_bool()) };
        Self {
            state: e.state.clone(),
            volume: get_f32("volume_level"),
            muted: get_bool("is_volume_muted"),
            media_title: get_str("media_title"),
            media_artist: get_str("media_artist"),
            media_album: get_str("media_album_name"),
            media_content_id: get_str("media_content_id"),
            media_content_type: get_str("media_content_type"),
            entity_picture: get_str("entity_picture"),
            available: e.state != "unavailable" && e.state != "unknown",
            updated_at: std::time::Instant::now(),
        }
    }
}

pub struct HaPlayer {
    pub entity_id: String,
    pub name: String,
    pub client: Arc<HaClient>,
    pub snapshot: RwLock<HaPlayerSnapshot>,
}

impl std::fmt::Debug for HaPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HaPlayer")
            .field("entity_id", &self.entity_id)
            .field("name", &self.name)
            .finish()
    }
}

impl HaPlayer {
    pub fn new(entity_id: String, name: String, client: Arc<HaClient>) -> Arc<Self> {
        Arc::new(Self {
            entity_id,
            name,
            client,
            snapshot: RwLock::new(HaPlayerSnapshot::default()),
        })
    }

    pub fn update_from_entity(&self, e: &HaEntity) {
        *self.snapshot.write() = HaPlayerSnapshot::from_entity(e);
    }

    fn playback_state_from_ha(s: &str) -> PlaybackState {
        match s {
            "playing" => PlaybackState::Playing,
            "paused" => PlaybackState::Paused,
            "idle" => PlaybackState::Idle,
            "off" => PlaybackState::Idle,
            "buffering" => PlaybackState::Idle,
            _ => PlaybackState::Idle,
        }
    }

    /// Run a `call_service` from a sync trait method. We require a
    /// multi-thread tokio runtime.
    fn call_service_blocking(
        &self,
        domain: &str,
        service: &str,
        body: serde_json::Value,
    ) -> Result<(), crate::client::HaClientError> {
        let client = self.client.clone();
        let domain = domain.to_string();
        let service = service.to_string();
        let entity_id = self.entity_id.clone();
        tokio::task::block_in_place(|| {
            let mut body = body;
            if !body.is_object() {
                body = serde_json::json!({});
            }
            if let Some(obj) = body.as_object_mut() {
                obj.entry("entity_id".to_string())
                    .or_insert_with(|| serde_json::Value::String(entity_id));
            }
            tokio::runtime::Handle::current()
                .block_on(async move { client.call_service(&domain, &service, &body).await })
        })
    }
}

impl HaPlayer {
    fn snapshot_to_player(&self) -> Player {
        let snap = self.snapshot.read();
        let volume = snap.volume.unwrap_or(0.0).clamp(0.0, 1.0);
        let current_media = if let Some(title) = snap.media_title.clone() {
            let artist = snap.media_artist.clone();
            let album = snap.media_album.clone();
            let content_id = snap
                .media_content_id
                .clone()
                .unwrap_or_else(|| "unknown".into());
            let picture = snap.entity_picture.clone();
            let entity_id = self.entity_id.clone();
            Some(serde_json::json!({
                "title": title,
                "artist": artist,
                "album": album,
                "uri": content_id,
                "image_url": picture,
                "queue_id": entity_id,
            }))
        } else {
            None
        };
        let volume_level = (volume * 100.0) as u32;
        let mut supported = vec![PlayerFeature::VolumeSet, PlayerFeature::Power];
        match snap.state.as_str() {
            "playing" => supported.push(PlayerFeature::Pause),
            "paused" | "idle" => {}
            _ => {}
        }
        Player {
            player_id: ma_core::identifiers::PlayerId::from(self.entity_id.clone()),
            provider: "hass".into(),
            type_: PlayerType::Player,
            name: self.name.clone(),
            available: snap.available,
            powered: snap.state != "off",
            playback_state: Self::playback_state_from_ha(&snap.state),
            volume_level: Some(volume_level),
            volume_muted: snap.muted,
            current_media,
            supported_features: supported,
            can_group_with: Vec::new(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl PlayerControl for HaPlayer {
    fn player_id(&self) -> &str {
        &self.entity_id
    }

    fn provider_domain(&self) -> &str {
        "hass"
    }

    fn state(&self) -> Player {
        self.snapshot_to_player()
    }

    fn can_group_with(&self, _other_domain: &str) -> bool {
        // HA media_player entities don't natively support cross-bridge
        // grouping; we leave this off for V1.
        false
    }

    fn requires_flow_mode(&self) -> bool {
        false
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    async fn group_with(&self, _leader_id: &str) -> MaResult<()> {
        Err(ma_core::errors::Error::Unsupported(
            "hass players do not support grouping in V1",
        ))
    }

    async fn ungroup(&self) -> MaResult<()> {
        Ok(())
    }

    async fn play(&self) -> MaResult<()> {
        self.call_service_blocking("media_player", "media_play", serde_json::json!({}))
            .map_err(|e| {
                warn!(entity_id = %self.entity_id, error = %e, "ha play failed");
                ma_core::errors::Error::Unavailable(e.to_string())
            })
    }

    async fn stop(&self) -> MaResult<()> {
        self.call_service_blocking("media_player", "media_stop", serde_json::json!({}))
            .map_err(|e| {
                warn!(entity_id = %self.entity_id, error = %e, "ha stop failed");
                ma_core::errors::Error::Unavailable(e.to_string())
            })
    }

    async fn set_volume(&self, level: u32) -> MaResult<()> {
        let v = (level as f32 / 100.0).clamp(0.0, 1.0);
        let body = serde_json::json!({ "volume_level": v });
        self.call_service_blocking("media_player", "volume_set", body)
            .map_err(|e| {
                warn!(entity_id = %self.entity_id, error = %e, "ha set_volume failed");
                ma_core::errors::Error::Unavailable(e.to_string())
            })
    }

    async fn set_mute(&self, mute: bool) -> MaResult<()> {
        let body = serde_json::json!({ "is_volume_muted": mute });
        self.call_service_blocking("media_player", "volume_mute", body)
            .map_err(|e| {
                warn!(entity_id = %self.entity_id, error = %e, "ha set_mute failed");
                ma_core::errors::Error::Unavailable(e.to_string())
            })
    }

    async fn set_power(&self, on: bool) -> MaResult<()> {
        let svc = if on { "turn_on" } else { "turn_off" };
        self.call_service_blocking("media_player", svc, serde_json::json!({}))
            .map_err(|e| {
                warn!(entity_id = %self.entity_id, error = %e, "ha set_power failed");
                ma_core::errors::Error::Unavailable(e.to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::HaEntity;

    fn make_entity(state: &str, vol: Option<f32>) -> HaEntity {
        let attrs = serde_json::json!({
            "volume_level": vol,
            "is_volume_muted": false,
            "media_title": "Song A",
            "media_artist": "Artist X",
            "media_content_id": "https://example.com/song",
        });
        HaEntity {
            entity_id: "media_player.living".into(),
            state: state.into(),
            attributes: attrs,
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_from_entity_extracts_fields() {
        let e = make_entity("playing", Some(0.42));
        let s = HaPlayerSnapshot::from_entity(&e);
        assert_eq!(s.state, "playing");
        assert_eq!(s.volume, Some(0.42));
        assert_eq!(s.media_title.as_deref(), Some("Song A"));
        assert!(s.available);
    }

    #[test]
    fn snapshot_treats_unavailable_as_offline() {
        let e = make_entity("unavailable", Some(0.5));
        let s = HaPlayerSnapshot::from_entity(&e);
        assert!(!s.available);
    }

    #[test]
    fn player_state_maps_playback() {
        let p = HaPlayer::new(
            "media_player.living".into(),
            "Living".into(),
            Arc::new(HaClient::new("http://ha:8123", "tok", true).unwrap()),
        );
        *p.snapshot.write() = HaPlayerSnapshot::from_entity(&make_entity("playing", Some(0.5)));
        let s = p.state();
        assert_eq!(s.playback_state, PlaybackState::Playing);
        assert!(s.powered);
        assert_eq!(s.volume_level, Some(50));
        assert!(s.current_media.is_some());
    }

    #[test]
    fn player_state_idle_when_off() {
        let p = HaPlayer::new(
            "media_player.living".into(),
            "Living".into(),
            Arc::new(HaClient::new("http://ha:8123", "tok", true).unwrap()),
        );
        *p.snapshot.write() = HaPlayerSnapshot::from_entity(&make_entity("off", Some(0.0)));
        let s = p.state();
        assert_eq!(s.playback_state, PlaybackState::Idle);
        assert!(!s.powered);
    }
}
