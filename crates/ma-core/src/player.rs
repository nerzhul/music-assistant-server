//! Player struct mirroring `music_assistant_models.player.Player`.
//!
//! This is the public, JSON-serialized form of a player that the UI
//! consumes. The `Player` trait below is the minimal in-process
//! surface that the sync-group / universal-group / bridge player
//! crates need: enough to read state, issue playback commands, and
//! query the protocols a player supports for grouping. The full
//! Python `Player` class is ~3000 lines; we expose only the
//! method shape the Phase 4 crates need.

use crate::enums::*;
use crate::identifiers::PlayerId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Player {
    pub player_id: PlayerId,
    pub provider: String,
    pub type_: PlayerType,
    pub name: String,
    pub available: bool,
    pub powered: bool,
    pub playback_state: PlaybackState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_time_last_updated: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_media: Option<serde_json::Value>,
    pub supported_features: Vec<PlayerFeature>,
    pub can_group_with: Vec<String>,
    pub synced_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_members: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_player_in_ui: Option<HidePlayerOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// What the sync-group / universal-group crates need to know about a
/// real (or synthetic) player. The full Player state lives on the
/// implementer; the trait just exposes the commands they need to
/// issue and the queries they need to make.
#[async_trait]
pub trait PlayerControl: std::fmt::Debug + Send + Sync {
    /// Stable player id.
    fn player_id(&self) -> &str;
    /// Provider domain (e.g. `"sendspin"`, `"syncgroup"`,
    /// `"universal_group"`, `"bridge"`).
    fn provider_domain(&self) -> &str;
    /// Snapshot of the player's state.
    fn state(&self) -> Player;
    /// Whether this player supports grouping with players of
    /// `other_domain`. The Python `can_group_with` set lives on the
    /// Player struct, so the trait method just delegates.
    fn can_group_with(&self, other_domain: &str) -> bool;
    /// Whether this player requires flow mode (i.e. can only consume
    /// `/flow/...` streams, not `/single/...`).
    fn requires_flow_mode(&self) -> bool;
    /// Display name (UI label).
    fn display_name(&self) -> &str;
    /// Group the player with `leader_id`. Implementation-specific:
    /// the Sendspin crate will tell the leader to add the follower
    /// via the protocol, the universal-group crate will record the
    /// player in its `current_members`, etc.
    async fn group_with(&self, leader_id: &str) -> crate::Result<()>;
    /// Remove the player from its current group. Pair of
    /// `group_with`.
    async fn ungroup(&self) -> crate::Result<()>;
    /// Start playback.
    async fn play(&self) -> crate::Result<()>;
    /// Stop playback.
    async fn stop(&self) -> crate::Result<()>;
    /// Set volume (0..100).
    async fn set_volume(&self, level: u32) -> crate::Result<()>;
    /// Set mute.
    async fn set_mute(&self, mute: bool) -> crate::Result<()>;
    /// Set power (on/off). Some players (groups) have no opinion and
    /// return Ok without doing anything.
    async fn set_power(&self, on: bool) -> crate::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_serde_skips_optionals() {
        let p = Player {
            player_id: PlayerId::from("sendspin_abc"),
            provider: "sendspin".into(),
            type_: PlayerType::Player,
            name: "Living Room".into(),
            available: true,
            powered: true,
            playback_state: PlaybackState::Playing,
            volume_level: Some(50),
            supported_features: vec![PlayerFeature::VolumeSet, PlayerFeature::Power],
            can_group_with: vec!["sendspin".into()],
            ..Default::default()
        };
        let j = serde_json::to_string(&p).unwrap();
        assert!(j.contains("\"player_id\":\"sendspin_abc\""));
        assert!(j.contains("\"volume_level\":50"));
        assert!(!j.contains("volume_muted"));
    }
}
