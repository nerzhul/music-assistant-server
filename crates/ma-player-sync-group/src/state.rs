//! Sync group state machine.
//!
//! Mirrors the player state in `SyncGroupPlayer` but reduced to the
//! fields the Phase 4 port actually uses. The full Python player
//! carries ~80 attributes; we keep only the ones that drive group
//! formation, leader selection, and member fan-out.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use ma_core::enums::PlaybackState;
use ma_core::player::{Player, PlayerControl};

/// Per-member record. Holds an `Arc<dyn PlayerControl>` (cheap to
/// clone) and a small cache of fields the group consults often
/// (display name, protocol domain) so it doesn't re-query the
/// underlying player on every command.
#[derive(Debug, Clone)]
pub struct GroupMember {
    pub control: Arc<dyn PlayerControl>,
    pub display_name: String,
    pub provider_domain: String,
    pub requires_flow_mode: bool,
}

impl GroupMember {
    pub fn from_control(c: Arc<dyn PlayerControl>) -> Self {
        Self {
            display_name: c.display_name().to_string(),
            provider_domain: c.provider_domain().to_string(),
            requires_flow_mode: c.requires_flow_mode(),
            control: c,
        }
    }

    pub fn player_id(&self) -> &str {
        self.control.player_id()
    }
}

/// Snapshot of the current leader, used by `SyncGroup::play_media` /
/// `set_volume` etc. to find the right receiver.
#[derive(Debug, Clone, Default)]
pub struct LeaderInfo {
    pub player_id: String,
    pub provider_domain: String,
    /// Set to true when the current leader can be promoted to a new
    /// leader at the protocol level (AirPlay / Snapcast / Sendspin).
    /// Otherwise removing the leader forces a dissolve + reform.
    pub supports_dynamic_handoff: bool,
}

/// Top-level state machine for a sync group. The four states
/// correspond to the transitions in the Python `SyncGroupPlayer`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupState {
    /// No members.
    #[default]
    Idle,
    /// Members registered, leader chosen, but no playback has
    /// started yet.
    Forming,
    /// Leader has accepted a play command and members are synced.
    Active,
    /// Members have been removed and we're tearing down the
    /// protocol-level group.
    Dissolving,
}

/// Internal mutable state held by `SyncGroup` behind a lock.
#[derive(Debug, Default)]
pub(crate) struct GroupInner {
    pub state: GroupState,
    pub leader: Option<LeaderInfo>,
    pub members: Vec<GroupMember>,
    pub playback_state: PlaybackState,
    pub volume_level: Option<u32>,
    pub volume_muted: Option<bool>,
    pub powered: Option<bool>,
}

impl GroupInner {
    pub fn player_snapshot(&self, player_id: &str) -> Player {
        let mut p = Player {
            player_id: ma_core::identifiers::PlayerId::from(player_id),
            provider: "syncgroup".to_string(),
            type_: ma_core::enums::PlayerType::Group,
            name: format!("Sync Group ({})", player_id),
            ..Default::default()
        };
        p.available = !self.members.is_empty();
        p.powered = self.powered.unwrap_or(true);
        p.playback_state = self.playback_state;
        p.volume_level = self.volume_level;
        p.volume_muted = self.volume_muted;
        p.group_members = Some(
            self.members
                .iter()
                .map(|m| m.player_id().to_string())
                .collect(),
        );
        p.can_group_with = self
            .members
            .iter()
            .map(|m| m.provider_domain.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        p
    }
}

/// Shared state container exposed to the rest of the server. The
/// sync group holds one of these and clones the `Arc` to share it
/// with the player controller and the webserver.
#[derive(Debug, Default, Clone)]
pub struct SharedGroupState {
    pub(crate) inner: Arc<RwLock<GroupInner>>,
}

impl SharedGroupState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self, player_id: &str) -> Player {
        self.inner.read().await.player_snapshot(player_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_state_default_is_idle() {
        assert_eq!(GroupState::default(), GroupState::Idle);
    }

    #[tokio::test]
    async fn player_snapshot_carries_group_members() {
        let s = SharedGroupState::new();
        let inner = s.inner.read().await;
        let p = inner.player_snapshot("syncgroup_x");
        assert_eq!(p.provider, "syncgroup");
        assert_eq!(p.type_, ma_core::enums::PlayerType::Group);
        assert!(!p.available);
        assert!(p.group_members.as_ref().unwrap().is_empty());
    }
}
