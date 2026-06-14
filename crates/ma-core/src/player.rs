//! Player struct mirroring `music_assistant_models.player.Player`.
//!
//! This is the public, JSON-serialized form of a player that the UI consumes.

use crate::enums::*;
use crate::identifiers::PlayerId;
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
