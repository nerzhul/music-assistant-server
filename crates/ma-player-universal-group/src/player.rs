//! `UniversalGroup` — multi-provider fan-out sink.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::warn;

use ma_core::enums::PlaybackState;
use ma_core::player::{Player, PlayerControl};

/// How long the group holds its members after the queue goes
/// idle before releasing them. Matches `IDLE_GRACE_SECONDS` in the
/// Python implementation.
pub const IDLE_GRACE_SECONDS: f64 = 10.0;

#[derive(Debug, Error)]
pub enum UgpError {
    #[error("no members captured")]
    NoMembers,
    #[error("player control error: {0}")]
    PlayerControl(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, UgpError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UniversalGroupConfig {
    pub player_id: String,
    pub display_name: String,
}

impl Default for UniversalGroupConfig {
    fn default() -> Self {
        Self {
            player_id: "ugp_default".into(),
            display_name: "Universal Group".into(),
        }
    }
}

#[derive(Debug, Default)]
struct UgpInner {
    members: Vec<Arc<dyn PlayerControl>>,
    /// Currently-playing media (URI + display title). Mirrors the
    /// `current_media` field of the Python class.
    current_media: Option<Value>,
    playback_state: PlaybackState,
    volume_level: Option<u32>,
    volume_muted: Option<bool>,
    powered: Option<bool>,
    /// True while a member is "captured" by this group. A captured
    /// member is shown as `active_group = <this group>` in the
    /// webserver output until `release` is called.
    active_session: bool,
}

pub struct UniversalGroup {
    config: UniversalGroupConfig,
    inner: RwLock<UgpInner>,
}

impl std::fmt::Debug for UniversalGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniversalGroup")
            .field("config", &self.config)
            .field(
                "member_count",
                &self.inner.try_read().map(|g| g.members.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl UniversalGroup {
    pub fn new(config: UniversalGroupConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: RwLock::new(UgpInner::default()),
        })
    }

    pub fn player_id(&self) -> &str {
        &self.config.player_id
    }

    pub fn snapshot(&self) -> Player {
        let inner = self.inner.try_read().ok();
        let members: Vec<Arc<dyn PlayerControl>> = inner
            .as_ref()
            .map(|g| g.members.clone())
            .unwrap_or_default();
        let mut p = Player {
            player_id: ma_core::identifiers::PlayerId::from(self.config.player_id.clone()),
            provider: "universal_group".to_string(),
            type_: ma_core::enums::PlayerType::Group,
            name: self.config.display_name.clone(),
            ..Default::default()
        };
        p.available = !members.is_empty();
        if let Some(inner) = inner.as_ref() {
            p.powered = inner.powered.unwrap_or(true);
            p.playback_state = inner.playback_state;
            p.volume_level = inner.volume_level;
            p.volume_muted = inner.volume_muted;
        }
        p.group_members = Some(members.iter().map(|m| m.player_id().to_string()).collect());
        p
    }

    /// True while a session is active (members captured or in the
    /// grace window).
    pub fn is_active_session(&self) -> bool {
        self.inner
            .try_read()
            .map(|g| g.active_session)
            .unwrap_or(false)
    }

    /// Capture a snapshot of `controls` for the next session. The
    /// members are released either by `release()` or by the idle
    /// grace timer (Phase 4 leaves the timer to the caller; the
    /// `IdleGraceRunner` in the Python class is a one-off `asyncio`
    /// task we don't replicate here).
    pub async fn capture_members(&self, controls: Vec<Arc<dyn PlayerControl>>) {
        let mut inner = self.inner.write().await;
        inner.members = controls;
        inner.active_session = !inner.members.is_empty();
    }

    /// Add one member to the captured set.
    pub async fn add_member(&self, control: Arc<dyn PlayerControl>) {
        let mut inner = self.inner.write().await;
        if !inner
            .members
            .iter()
            .any(|m| m.player_id() == control.player_id())
        {
            inner.members.push(control);
            inner.active_session = true;
        }
    }

    /// Release the captured members and stop playback. The caller
    /// decides when to call this (the Python class schedules it
    /// after the idle grace window).
    pub async fn release(&self) {
        let mut inner = self.inner.write().await;
        inner.members.clear();
        inner.active_session = false;
        inner.playback_state = PlaybackState::Idle;
        inner.current_media = None;
    }

    pub async fn current_media(&self) -> Option<Value> {
        self.inner.read().await.current_media.clone()
    }

    /// Helper: list the member ids.
    pub async fn members(&self) -> Vec<String> {
        self.inner
            .read()
            .await
            .members
            .iter()
            .map(|m| m.player_id().to_string())
            .collect()
    }
}

#[async_trait]
impl PlayerControl for UniversalGroup {
    fn player_id(&self) -> &str {
        &self.config.player_id
    }

    fn provider_domain(&self) -> &str {
        "universal_group"
    }

    fn state(&self) -> Player {
        self.snapshot()
    }

    fn can_group_with(&self, _other_domain: &str) -> bool {
        false
    }

    fn requires_flow_mode(&self) -> bool {
        // Each captured member pulls its own /ugp stream from the
        // streams server; that URL is a per-session `/single/...`
        // URL, not a `/flow/...` URL.
        false
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    async fn group_with(&self, _leader_id: &str) -> ma_core::Result<()> {
        Err(ma_core::errors::Error::Unsupported(
            "universal groups cannot be nested",
        ))
    }

    async fn ungroup(&self) -> ma_core::Result<()> {
        UniversalGroup::release(self).await;
        Ok(())
    }

    async fn play(&self) -> ma_core::Result<()> {
        let members = self.inner.read().await.members.clone();
        if members.is_empty() {
            return Err(ma_core::errors::Error::Internal("no members".into()));
        }
        for m in &members {
            if let Err(e) = m.play().await {
                warn!(error = %e, member = m.player_id(), "play failed");
            }
        }
        self.inner.write().await.playback_state = PlaybackState::Playing;
        Ok(())
    }

    async fn stop(&self) -> ma_core::Result<()> {
        let members = self.inner.read().await.members.clone();
        for m in &members {
            if let Err(e) = m.stop().await {
                warn!(error = %e, member = m.player_id(), "stop failed");
            }
        }
        self.inner.write().await.playback_state = PlaybackState::Idle;
        Ok(())
    }

    async fn set_volume(&self, level: u32) -> ma_core::Result<()> {
        let members = self.inner.read().await.members.clone();
        for m in &members {
            if let Err(e) = m.set_volume(level).await {
                warn!(error = %e, member = m.player_id(), "set_volume failed");
            }
        }
        self.inner.write().await.volume_level = Some(level);
        Ok(())
    }

    async fn set_mute(&self, mute: bool) -> ma_core::Result<()> {
        let members = self.inner.read().await.members.clone();
        for m in &members {
            if let Err(e) = m.set_mute(mute).await {
                warn!(error = %e, member = m.player_id(), "set_mute failed");
            }
        }
        self.inner.write().await.volume_muted = Some(mute);
        Ok(())
    }

    async fn set_power(&self, on: bool) -> ma_core::Result<()> {
        self.inner.write().await.powered = Some(on);
        Ok(())
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
        last_state: parking_lot::Mutex<Player>,
    }

    impl Stub {
        fn new(id: &str) -> Self {
            let p = Player {
                player_id: ma_core::identifiers::PlayerId::from(id),
                provider: "test".into(),
                type_: ma_core::enums::PlayerType::Player,
                name: id.to_string(),
                available: true,
                ..Default::default()
            };
            Self {
                id: id.to_string(),
                last_state: parking_lot::Mutex::new(p),
            }
        }
    }

    #[async_trait]
    impl PlayerControl for Stub {
        fn player_id(&self) -> &str {
            &self.id
        }
        fn provider_domain(&self) -> &str {
            "test"
        }
        fn state(&self) -> Player {
            self.last_state.lock().clone()
        }
        fn can_group_with(&self, _: &str) -> bool {
            true
        }
        fn requires_flow_mode(&self) -> bool {
            false
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
            self.last_state.lock().playback_state = PlaybackState::Playing;
            Ok(())
        }
        async fn stop(&self) -> ma_core::Result<()> {
            self.last_state.lock().playback_state = PlaybackState::Idle;
            Ok(())
        }
        async fn set_volume(&self, l: u32) -> ma_core::Result<()> {
            self.last_state.lock().volume_level = Some(l);
            Ok(())
        }
        async fn set_mute(&self, m: bool) -> ma_core::Result<()> {
            self.last_state.lock().volume_muted = Some(m);
            Ok(())
        }
        async fn set_power(&self, on: bool) -> ma_core::Result<()> {
            self.last_state.lock().powered = on;
            Ok(())
        }
    }

    fn stub(id: &str) -> Arc<dyn PlayerControl> {
        Arc::new(Stub::new(id))
    }

    #[test]
    fn new_group_is_empty_and_idle() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        assert!(!g.is_active_session());
        let s = g.snapshot();
        assert!(!s.available);
        assert!(s.group_members.as_ref().unwrap().is_empty());
    }

    #[tokio::test]
    async fn capture_marks_session_active() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        g.capture_members(vec![stub("p1"), stub("p2")]).await;
        assert!(g.is_active_session());
        assert_eq!(g.members().await, vec!["p1", "p2"]);
    }

    #[tokio::test]
    async fn release_clears_members() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        g.capture_members(vec![stub("p1")]).await;
        g.release().await;
        assert!(!g.is_active_session());
        assert!(g.members().await.is_empty());
    }

    #[tokio::test]
    async fn play_fans_out_to_members() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        g.capture_members(vec![stub("p1"), stub("p2")]).await;
        g.play().await.unwrap();
        let s = g.snapshot();
        assert_eq!(s.playback_state, PlaybackState::Playing);
    }

    #[tokio::test]
    async fn play_with_no_members_errors() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        let r = g.play().await;
        assert!(r.is_err());
    }

    #[test]
    fn snapshot_provider_is_universal_group() {
        let g = UniversalGroup::new(UniversalGroupConfig::default());
        let s = g.snapshot();
        assert_eq!(s.provider, "universal_group");
        assert_eq!(s.type_, ma_core::enums::PlayerType::Group);
    }

    #[test]
    fn duration_constant_matches_python() {
        // IDLE_GRACE_SECONDS should equal the Python constant.
        const PYTHON: f64 = 10.0;
        assert_eq!(IDLE_GRACE_SECONDS, PYTHON);
    }
}
