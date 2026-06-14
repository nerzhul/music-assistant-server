//! `SyncGroup` — the group state machine.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use ma_core::enums::PlaybackState;
use ma_core::player::{Player, PlayerControl};

use crate::state::{GroupMember, GroupState, LeaderInfo, SharedGroupState};

/// Domain names whose live sync session can survive removal of the
/// current leader. Mirrors the constant in
/// `music_assistant/providers/sync_group/constants.py`.
pub const PROVIDERS_WITH_DYNAMIC_LEADER_SWITCH: &[&str] = &["airplay", "snapcast", "sendspin"];

/// How long to wait for the leader to confirm playback after a
/// (re)form. Matches `PLAYBACK_START_TIMEOUT` in the Python helper.
pub const PLAYBACK_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Idle grace period before an empty group dissolves. Matches
/// `IDLE_GRACE_SECONDS` in the Python helper.
pub const IDLE_GRACE_SECONDS: f64 = 10.0;

#[derive(Debug, Error)]
pub enum GroupError {
    #[error("group is in invalid state for this operation: {0:?}")]
    InvalidState(GroupState),
    #[error("player is not in the group: {0}")]
    NotInGroup(String),
    #[error("leader is required for this operation but no leader is selected")]
    NoLeader,
    #[error("player control error: {0}")]
    PlayerControl(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("timeout waiting for leader to confirm playback")]
    StartTimeout,
}

pub type Result<T> = std::result::Result<T, GroupError>;

/// Configuration for a `SyncGroup`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncGroupConfig {
    pub player_id: String,
    pub display_name: String,
    pub is_dynamic: bool,
    pub static_members: Vec<String>,
}

impl Default for SyncGroupConfig {
    fn default() -> Self {
        Self {
            player_id: "syncgroup_default".into(),
            display_name: "Sync Group".into(),
            is_dynamic: true,
            static_members: Vec::new(),
        }
    }
}

pub struct SyncGroup {
    config: SyncGroupConfig,
    state: SharedGroupState,
    write_lock: Mutex<()>,
}

impl std::fmt::Debug for SyncGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncGroup")
            .field("config", &self.config)
            .field("state", &self.state())
            .field(
                "leader",
                &self
                    .state
                    .inner
                    .try_read()
                    .ok()
                    .and_then(|g| g.leader.clone()),
            )
            .finish()
    }
}

impl SyncGroup {
    pub fn new(config: SyncGroupConfig) -> Arc<Self> {
        let state = SharedGroupState::new();
        Arc::new(Self {
            config,
            state,
            write_lock: Mutex::new(()),
        })
    }

    pub fn player_id(&self) -> &str {
        &self.config.player_id
    }

    pub fn state(&self) -> GroupState {
        self.state
            .inner
            .try_read()
            .map(|g| g.state)
            .unwrap_or(GroupState::Idle)
    }

    pub fn snapshot(&self) -> Player {
        self.state
            .inner
            .try_read()
            .map(|g| g.player_snapshot(&self.config.player_id))
            .unwrap_or_default()
    }

    pub async fn snapshot_async(&self) -> Player {
        self.state.snapshot(&self.config.player_id).await
    }

    pub fn shared_state(&self) -> SharedGroupState {
        self.state.clone()
    }

    pub async fn add_member(&self, control: Arc<dyn PlayerControl>) -> Result<LeaderInfo> {
        let _guard = self.write_lock.lock().await;
        let member = GroupMember::from_control(control);
        let member_id = member.player_id().to_string();
        // Phase 1: take the inner lock, mutate, drop it before any
        // await point.
        let to_call: Option<(Arc<dyn PlayerControl>, String)> = {
            let mut inner = self.state.inner.write().await;
            if inner.members.iter().any(|m| m.player_id() == member_id) {
                return Ok(inner.leader.clone().unwrap_or_default());
            }
            let control_clone = Arc::clone(&member.control);
            let was_empty = inner.members.is_empty();
            inner.members.push(member);
            if was_empty {
                inner.leader = Some(leader_info_from_member(inner.members.last().unwrap()));
                None
            } else if let Some(leader) = inner.leader.as_ref() {
                let can_group = control_clone.can_group_with(&leader.provider_domain);
                if !can_group {
                    debug!(
                        "member {} cannot group with current leader {} (domain {})",
                        member_id, leader.player_id, leader.provider_domain
                    );
                    None
                } else if member_id != leader.player_id {
                    Some((control_clone, leader.player_id.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((c, leader_id)) = to_call {
            if let Err(e) = c.group_with(&leader_id).await {
                warn!(error = %e, "group_with failed for {member_id}");
            }
        }
        // Recompute the state machine under the write lock.
        {
            let mut inner = self.state.inner.write().await;
            inner.state = compute_group_state(&inner);
        }
        Ok(self
            .state
            .inner
            .read()
            .await
            .leader
            .clone()
            .unwrap_or_default())
    }

    pub async fn remove_member(&self, player_id: &str) -> Result<Option<LeaderInfo>> {
        let _guard = self.write_lock.lock().await;
        let new_leader: Option<LeaderInfo> = {
            let mut inner = self.state.inner.write().await;
            let Some(pos) = inner
                .members
                .iter()
                .position(|m| m.player_id() == player_id)
            else {
                return Err(GroupError::NotInGroup(player_id.into()));
            };
            let was_leader = inner
                .leader
                .as_ref()
                .map(|l| l.player_id == player_id)
                .unwrap_or(false);
            if was_leader && self.config.static_members.iter().any(|s| s == player_id) {
                return Err(GroupError::PlayerControl(format!(
                    "{player_id} is a static member and cannot be removed"
                )));
            }
            inner.members.remove(pos);
            if was_leader {
                inner.leader = pick_leader(&inner.members, None);
            }
            inner.state = compute_group_state(&inner);
            inner.leader.clone()
        };
        Ok(new_leader)
    }

    pub async fn set_members(
        &self,
        to_add: &[Arc<dyn PlayerControl>],
        to_remove: &[String],
    ) -> Result<Option<LeaderInfo>> {
        if !self.config.is_dynamic {
            return Err(GroupError::InvalidState(GroupState::Forming));
        }
        let _guard = self.write_lock.lock().await;
        for c in to_add {
            let m = GroupMember::from_control(Arc::clone(c));
            let _ = self.add_member_inner_locked(&m).await;
        }
        for id in to_remove {
            let _ = self.remove_member_inner_locked(id).await;
        }
        Ok(self.state.inner.read().await.leader.clone())
    }

    pub fn select_leader(&self) -> Option<LeaderInfo> {
        let inner = self.state.inner.try_read().ok()?;
        let preferred = inner.leader.as_ref();
        pick_leader(&inner.members, preferred)
    }

    pub async fn play(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (_leader, leader_control) = self.leader_with_control().await?;
        leader_control
            .play()
            .await
            .map_err(|e| GroupError::PlayerControl(e.to_string()))?;
        {
            let mut inner = self.state.inner.write().await;
            inner.playback_state = PlaybackState::Playing;
            inner.state = compute_group_state(&inner);
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (_leader, leader_control) = self.leader_with_control().await?;
        leader_control
            .stop()
            .await
            .map_err(|e| GroupError::PlayerControl(e.to_string()))?;
        {
            let mut inner = self.state.inner.write().await;
            inner.playback_state = PlaybackState::Idle;
            inner.state = compute_group_state(&inner);
        }
        Ok(())
    }

    pub async fn set_volume(&self, level: u32) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (_leader, leader_control) = self.leader_with_control().await?;
        leader_control
            .set_volume(level)
            .await
            .map_err(|e| GroupError::PlayerControl(e.to_string()))?;
        self.state.inner.write().await.volume_level = Some(level);
        Ok(())
    }

    pub async fn set_mute(&self, mute: bool) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (_leader, leader_control) = self.leader_with_control().await?;
        leader_control
            .set_mute(mute)
            .await
            .map_err(|e| GroupError::PlayerControl(e.to_string()))?;
        self.state.inner.write().await.volume_muted = Some(mute);
        Ok(())
    }

    pub async fn set_power(&self, on: bool) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.state.inner.write().await.powered = Some(on);
        Ok(())
    }

    // -- private helpers --

    async fn add_member_inner_locked(&self, member: &GroupMember) -> Result<()> {
        let member_id = member.player_id().to_string();
        let to_call: Option<(Arc<dyn PlayerControl>, String)> = {
            let mut inner = self.state.inner.write().await;
            if inner.members.iter().any(|m| m.player_id() == member_id) {
                return Ok(());
            }
            let was_empty = inner.members.is_empty();
            let control = Arc::clone(&member.control);
            inner.members.push(member.clone());
            if was_empty {
                inner.leader = Some(leader_info_from_member(member));
                None
            } else if let Some(leader) = inner.leader.as_ref() {
                if leader.player_id != member_id && control.can_group_with(&leader.provider_domain)
                {
                    Some((control, leader.player_id.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((c, leader_id)) = to_call {
            if let Err(e) = c.group_with(&leader_id).await {
                warn!(error = %e, "group_with failed for {member_id}");
            }
        }
        Ok(())
    }

    async fn remove_member_inner_locked(&self, player_id: &str) -> Result<()> {
        {
            let mut inner = self.state.inner.write().await;
            let Some(pos) = inner
                .members
                .iter()
                .position(|m| m.player_id() == player_id)
            else {
                return Ok(());
            };
            let was_leader = inner
                .leader
                .as_ref()
                .map(|l| l.player_id == player_id)
                .unwrap_or(false);
            if was_leader && self.config.static_members.iter().any(|s| s == player_id) {
                return Err(GroupError::PlayerControl(format!(
                    "{player_id} is a static member and cannot be removed"
                )));
            }
            inner.members.remove(pos);
            if was_leader {
                inner.leader = pick_leader(&inner.members, None);
            }
        }
        Ok(())
    }

    async fn leader_with_control(&self) -> Result<(LeaderInfo, Arc<dyn PlayerControl>)> {
        let inner = self.state.inner.read().await;
        let leader = inner.leader.clone().ok_or(GroupError::NoLeader)?;
        let control = inner
            .members
            .iter()
            .find(|m| m.player_id() == leader.player_id)
            .map(|m| Arc::clone(&m.control))
            .ok_or(GroupError::NoLeader)?;
        Ok((leader, control))
    }
}

fn leader_info_from_member(m: &GroupMember) -> LeaderInfo {
    LeaderInfo {
        player_id: m.player_id().to_string(),
        provider_domain: m.provider_domain.clone(),
        supports_dynamic_handoff: PROVIDERS_WITH_DYNAMIC_LEADER_SWITCH
            .iter()
            .any(|d| **d == m.provider_domain),
    }
}

/// Pick a leader, preferring the current one if it's still
/// available. Otherwise pick the first member whose provider
/// supports dynamic handoff, then any available member.
fn pick_leader(members: &[GroupMember], current: Option<&LeaderInfo>) -> Option<LeaderInfo> {
    if let Some(cur) = current {
        if let Some(m) = members.iter().find(|m| m.player_id() == cur.player_id) {
            return Some(leader_info_from_member(m));
        }
    }
    members
        .iter()
        .find(|m| {
            PROVIDERS_WITH_DYNAMIC_LEADER_SWITCH
                .iter()
                .any(|d| **d == m.provider_domain)
        })
        .or_else(|| members.first())
        .map(leader_info_from_member)
}

/// Compute the high-level group state from the inner state.
fn compute_group_state(inner: &crate::state::GroupInner) -> GroupState {
    if inner.members.is_empty() {
        GroupState::Idle
    } else if inner.playback_state == PlaybackState::Playing {
        GroupState::Active
    } else {
        GroupState::Forming
    }
}

#[async_trait]
impl PlayerControl for SyncGroup {
    fn player_id(&self) -> &str {
        &self.config.player_id
    }

    fn provider_domain(&self) -> &str {
        "syncgroup"
    }

    fn state(&self) -> Player {
        self.snapshot()
    }

    fn can_group_with(&self, _other_domain: &str) -> bool {
        false
    }

    fn requires_flow_mode(&self) -> bool {
        true
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    async fn group_with(&self, _leader_id: &str) -> ma_core::Result<()> {
        Err(ma_core::errors::Error::Unsupported(
            "sync groups cannot be nested",
        ))
    }

    async fn ungroup(&self) -> ma_core::Result<()> {
        let to_remove: Vec<String> = {
            let inner = self.state.inner.read().await;
            inner
                .members
                .iter()
                .map(|m| m.player_id().to_string())
                .collect()
        };
        for id in to_remove {
            self.remove_member(&id)
                .await
                .map_err(|e| ma_core::errors::Error::Internal(format!("remove_member: {e}")))?;
        }
        Ok(())
    }

    async fn play(&self) -> ma_core::Result<()> {
        SyncGroup::play(self)
            .await
            .map_err(|e| ma_core::errors::Error::Internal(e.to_string()))
    }

    async fn stop(&self) -> ma_core::Result<()> {
        SyncGroup::stop(self)
            .await
            .map_err(|e| ma_core::errors::Error::Internal(e.to_string()))
    }

    async fn set_volume(&self, level: u32) -> ma_core::Result<()> {
        SyncGroup::set_volume(self, level)
            .await
            .map_err(|e| ma_core::errors::Error::Internal(e.to_string()))
    }

    async fn set_mute(&self, mute: bool) -> ma_core::Result<()> {
        SyncGroup::set_mute(self, mute)
            .await
            .map_err(|e| ma_core::errors::Error::Internal(e.to_string()))
    }

    async fn set_power(&self, on: bool) -> ma_core::Result<()> {
        SyncGroup::set_power(self, on)
            .await
            .map_err(|e| ma_core::errors::Error::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ma_core::player::{Player, PlayerControl};

    #[derive(Debug)]
    struct Stub {
        id: String,
        domain: String,
        state: parking_lot::Mutex<Player>,
    }

    impl Stub {
        fn new(id: &str, domain: &str) -> Self {
            let p = Player {
                player_id: ma_core::identifiers::PlayerId::from(id),
                provider: domain.into(),
                type_: ma_core::enums::PlayerType::Player,
                name: id.into(),
                available: true,
                ..Default::default()
            };
            Self {
                id: id.into(),
                domain: domain.into(),
                state: parking_lot::Mutex::new(p),
            }
        }
    }

    #[async_trait]
    impl PlayerControl for Stub {
        fn player_id(&self) -> &str {
            &self.id
        }
        fn provider_domain(&self) -> &str {
            &self.domain
        }
        fn state(&self) -> Player {
            self.state.lock().clone()
        }
        fn can_group_with(&self, other: &str) -> bool {
            other == "sendspin" || other == "airplay"
        }
        fn requires_flow_mode(&self) -> bool {
            true
        }
        fn display_name(&self) -> &str {
            &self.id
        }
        async fn group_with(&self, _: &str) -> ma_core::Result<()> {
            Ok(())
        }
        async fn ungroup(&self) -> ma_core::Result<()> {
            Ok(())
        }
        async fn play(&self) -> ma_core::Result<()> {
            self.state.lock().playback_state = PlaybackState::Playing;
            Ok(())
        }
        async fn stop(&self) -> ma_core::Result<()> {
            self.state.lock().playback_state = PlaybackState::Idle;
            Ok(())
        }
        async fn set_volume(&self, l: u32) -> ma_core::Result<()> {
            self.state.lock().volume_level = Some(l);
            Ok(())
        }
        async fn set_mute(&self, m: bool) -> ma_core::Result<()> {
            self.state.lock().volume_muted = Some(m);
            Ok(())
        }
        async fn set_power(&self, on: bool) -> ma_core::Result<()> {
            self.state.lock().powered = on;
            Ok(())
        }
    }

    fn stub(id: &str, domain: &str) -> Arc<dyn PlayerControl> {
        Arc::new(Stub::new(id, domain))
    }

    #[tokio::test]
    async fn empty_group_is_idle() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        assert_eq!(g.state(), GroupState::Idle);
        assert!(g.select_leader().is_none());
    }

    #[tokio::test]
    async fn first_member_becomes_leader() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        g.add_member(stub("spk1", "sendspin")).await.unwrap();
        let leader = g.select_leader().unwrap();
        assert_eq!(leader.player_id, "spk1");
        assert!(leader.supports_dynamic_handoff);
    }

    #[tokio::test]
    async fn second_member_groups_with_first() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        g.add_member(stub("spk1", "sendspin")).await.unwrap();
        g.add_member(stub("spk2", "sendspin")).await.unwrap();
        let leader = g.select_leader().unwrap();
        assert_eq!(leader.player_id, "spk1");
    }

    #[tokio::test]
    async fn removing_leader_picks_new_one() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        g.add_member(stub("spk1", "sendspin")).await.unwrap();
        g.add_member(stub("spk2", "sendspin")).await.unwrap();
        g.remove_member("spk1").await.unwrap();
        let leader = g.select_leader().unwrap();
        assert_eq!(leader.player_id, "spk2");
    }

    #[tokio::test]
    async fn removing_last_member_idles_group() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        g.add_member(stub("spk1", "sendspin")).await.unwrap();
        g.remove_member("spk1").await.unwrap();
        assert_eq!(g.state(), GroupState::Idle);
        assert!(g.select_leader().is_none());
    }

    #[tokio::test]
    async fn play_propagates_to_leader() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        g.add_member(stub("spk1", "sendspin")).await.unwrap();
        g.play().await.unwrap();
        let s = g.snapshot();
        assert_eq!(s.playback_state, PlaybackState::Playing);
        assert_eq!(g.state(), GroupState::Active);
    }

    #[tokio::test]
    async fn group_cannot_be_nested() {
        let g = SyncGroup::new(SyncGroupConfig::default());
        let r = g.group_with("another_id").await;
        assert!(r.is_err());
    }

    #[test]
    fn leader_pick_prefers_first_dynamic_handoff_member() {
        let members = vec![
            GroupMember {
                control: stub("spk1", "airplay"),
                display_name: "AirPlay".into(),
                provider_domain: "airplay".into(),
                requires_flow_mode: true,
            },
            GroupMember {
                control: stub("spk2", "sendspin"),
                display_name: "Sendspin".into(),
                provider_domain: "sendspin".into(),
                requires_flow_mode: true,
            },
        ];
        let leader = pick_leader(&members, None).unwrap();
        assert_eq!(leader.player_id, "spk1");
    }

    #[test]
    fn leader_pick_falls_back_to_first_member() {
        let members = vec![
            GroupMember {
                control: stub("p1", "lms"),
                display_name: "LMS".into(),
                provider_domain: "lms".into(),
                requires_flow_mode: true,
            },
            GroupMember {
                control: stub("p2", "lms"),
                display_name: "LMS 2".into(),
                provider_domain: "lms".into(),
                requires_flow_mode: true,
            },
        ];
        let leader = pick_leader(&members, None).unwrap();
        assert_eq!(leader.player_id, "p1");
    }
}
