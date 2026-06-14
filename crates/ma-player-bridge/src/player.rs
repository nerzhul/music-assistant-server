//! `BridgePlayer` — thin proxy over another `PlayerControl`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::warn;

use ma_core::player::{Player, PlayerControl};

/// The id the bridge exposes to the rest of the system. By
/// convention the Python class prefixes the underlying player's id
/// with `bridge_`.
pub fn bridge_player_id(inner_id: &str) -> String {
    format!("bridge_{inner_id}")
}

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("inner player error: {0}")]
    Inner(String),
}

pub type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeConfig {
    /// The id this bridge exposes to the outside world. The
    /// underlying player's id is preserved separately and can be
    /// queried via `BridgePlayer::inner_player_id`.
    pub bridge_player_id: String,
    pub display_name: String,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            bridge_player_id: "bridge_default".into(),
            display_name: "Bridge".into(),
        }
    }
}

pub struct BridgePlayer {
    config: BridgeConfig,
    /// The inner player. Replaced on reconnection via
    /// `replace_inner`. Held in a `RwLock` so the swap is atomic.
    inner: RwLock<Option<Arc<dyn PlayerControl>>>,
}

impl std::fmt::Debug for BridgePlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_inner = self.inner.try_read().map(|g| g.is_some()).unwrap_or(false);
        f.debug_struct("BridgePlayer")
            .field("config", &self.config)
            .field("has_inner", &has_inner)
            .finish()
    }
}

impl BridgePlayer {
    pub fn new(config: BridgeConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: RwLock::new(None),
        })
    }

    /// Wrap an existing `PlayerControl`. The bridge id defaults to
    /// `bridge_<inner_id>` if not specified.
    pub fn wrap(inner: Arc<dyn PlayerControl>, display_name: Option<String>) -> Arc<Self> {
        let bridge_id = bridge_player_id(inner.player_id());
        let bridge = Arc::new(Self {
            config: BridgeConfig {
                bridge_player_id: bridge_id,
                display_name: display_name
                    .unwrap_or_else(|| format!("{} (bridge)", inner.display_name())),
            },
            inner: RwLock::new(Some(inner)),
        });
        bridge
    }

    /// Id of the inner player. Returns `None` when the inner
    /// hasn't been attached yet.
    pub async fn inner_player_id(&self) -> Option<String> {
        self.inner
            .read()
            .await
            .as_ref()
            .map(|p| p.player_id().to_string())
    }

    /// Replace the inner player (used on reconnect). Old commands
    /// in flight will fail; the caller is expected to retry.
    pub async fn replace_inner(&self, new: Arc<dyn PlayerControl>) {
        let mut slot = self.inner.write().await;
        *slot = Some(new);
    }

    /// Borrow the inner for ad-hoc queries.
    pub async fn inner(&self) -> Option<Arc<dyn PlayerControl>> {
        self.inner.read().await.clone()
    }

    pub fn snapshot(&self) -> Player {
        let inner = self.inner.try_read().ok().and_then(|g| g.clone());
        let inner_s = inner.as_ref().map(|p| p.state());
        let mut p = Player {
            player_id: ma_core::identifiers::PlayerId::from(self.config.bridge_player_id.clone()),
            provider: "bridge".to_string(),
            type_: ma_core::enums::PlayerType::Player,
            name: self.config.display_name.clone(),
            ..Default::default()
        };
        if let Some(s) = inner_s {
            p.available = s.available;
            p.playback_state = s.playback_state;
            p.volume_level = s.volume_level;
            p.volume_muted = s.volume_muted;
            p.powered = s.powered;
            p.supported_features = s.supported_features;
        }
        p
    }
}

#[async_trait]
impl PlayerControl for BridgePlayer {
    fn player_id(&self) -> &str {
        &self.config.bridge_player_id
    }

    fn provider_domain(&self) -> &str {
        "bridge"
    }

    fn state(&self) -> Player {
        self.snapshot()
    }

    fn can_group_with(&self, other_domain: &str) -> bool {
        if let Ok(inner) = self.inner.try_read() {
            inner
                .as_ref()
                .map(|p| p.can_group_with(other_domain))
                .unwrap_or(false)
        } else {
            false
        }
    }

    fn requires_flow_mode(&self) -> bool {
        if let Ok(inner) = self.inner.try_read() {
            inner
                .as_ref()
                .map(|p| p.requires_flow_mode())
                .unwrap_or(false)
        } else {
            false
        }
    }

    fn display_name(&self) -> &str {
        &self.config.display_name
    }

    async fn group_with(&self, leader_id: &str) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.group_with(leader_id).await,
            None => Err(ma_core::errors::Error::Internal("no inner player".into())),
        }
    }

    async fn ungroup(&self) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.ungroup().await,
            None => Ok(()),
        }
    }

    async fn play(&self) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.play().await,
            None => Err(ma_core::errors::Error::Internal("no inner player".into())),
        }
    }

    async fn stop(&self) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.stop().await,
            None => Ok(()),
        }
    }

    async fn set_volume(&self, level: u32) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.set_volume(level).await,
            None => {
                warn!("set_volume called before inner attached");
                Ok(())
            }
        }
    }

    async fn set_mute(&self, mute: bool) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.set_mute(mute).await,
            None => Ok(()),
        }
    }

    async fn set_power(&self, on: bool) -> ma_core::Result<()> {
        match self.inner.read().await.as_ref() {
            Some(p) => p.set_power(on).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ma_core::enums::PlaybackState;
    use ma_core::player::{Player, PlayerControl};

    #[derive(Debug)]
    struct Stub {
        id: String,
        state: parking_lot::Mutex<Player>,
    }

    impl Stub {
        fn new(id: &str) -> Self {
            let p = Player {
                player_id: ma_core::identifiers::PlayerId::from(id),
                provider: "sendspin".into(),
                available: true,
                ..Default::default()
            };
            Self {
                id: id.to_string(),
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
            "sendspin"
        }
        fn state(&self) -> Player {
            self.state.lock().clone()
        }
        fn can_group_with(&self, other: &str) -> bool {
            other == "sendspin"
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

    fn stub(id: &str) -> Arc<dyn PlayerControl> {
        Arc::new(Stub::new(id))
    }

    #[test]
    fn bridge_id_includes_inner_id() {
        assert_eq!(bridge_player_id("spk1"), "bridge_spk1");
    }

    #[tokio::test]
    async fn wrap_exposes_bridge_id() {
        let b = BridgePlayer::wrap(stub("spk1"), None);
        assert_eq!(b.player_id(), "bridge_spk1");
        assert_eq!(b.provider_domain(), "bridge");
        assert_eq!(b.inner_player_id().await.as_deref(), Some("spk1"));
    }

    #[tokio::test]
    async fn snapshot_inherits_inner_state() {
        let b = BridgePlayer::wrap(stub("spk1"), None);
        let inner = b.inner().await.unwrap();
        inner.play().await.unwrap();
        let s = b.snapshot();
        assert_eq!(s.playback_state, PlaybackState::Playing);
    }

    #[tokio::test]
    async fn replace_inner_swaps_target() {
        let b = BridgePlayer::wrap(stub("spk1"), None);
        b.replace_inner(stub("spk2")).await;
        assert_eq!(b.inner_player_id().await.as_deref(), Some("spk2"));
    }

    #[tokio::test]
    async fn bridge_forwards_play_command() {
        let b = BridgePlayer::wrap(stub("spk1"), None);
        b.play().await.unwrap();
        let s = b.snapshot();
        assert_eq!(s.playback_state, PlaybackState::Playing);
    }

    #[tokio::test]
    async fn new_without_inner_returns_unavailable() {
        let b = BridgePlayer::new(BridgeConfig {
            bridge_player_id: "bridge_pending".into(),
            display_name: "Pending".into(),
        });
        let s = b.snapshot();
        assert!(!s.available);
    }
}
