//! Shared application state for `ma-server`.

use std::collections::HashMap;
use std::sync::Arc;

use ma_config::MassConfig;
use ma_core::api::CommandRegistry;
use ma_core::identifiers::PlayerId;
use ma_core::player::{Player, PlayerControl};
use ma_ha::HaPlayer;
use ma_player_bridge::BridgePlayer;
use ma_player_sync_group::{SyncGroup, SyncGroupConfig};
use ma_player_universal_group::{UniversalGroup, UniversalGroupConfig};

use crate::auth::AuthManager;

pub struct AppState {
    pub config: MassConfig,
    pub player_controller: Arc<PlayerController>,
    pub auth: AuthManager,
    pub commands: Arc<CommandRegistry>,
    pub providers: Arc<ma_providers::provider::ProviderRegistry>,
    /// `None` when the in-memory auth backend is used (tests); `Some`
    /// when a `ma_storage::Database` is configured.
    pub database: Option<Arc<ma_storage::Database>>,
}

impl AppState {
    pub fn new(
        config: MassConfig,
        player_controller: Arc<PlayerController>,
        auth: AuthManager,
        commands: Arc<CommandRegistry>,
        providers: Arc<ma_providers::provider::ProviderRegistry>,
    ) -> Self {
        Self {
            config,
            player_controller,
            auth,
            commands,
            providers,
            database: None,
        }
    }

    pub fn with_database(mut self, db: Arc<ma_storage::Database>) -> Self {
        self.database = Some(db);
        self
    }

    /// Variant that accepts an `Option<Arc<Database>>` so the caller
    /// can pass through a result that may be `None` (when the DB
    /// failed to open).
    pub fn with_database_opt(mut self, db: Option<Arc<ma_storage::Database>>) -> Self {
        self.database = db;
        self
    }
}

/// Phase 4 player controller. Owns the live sync-group and
/// universal-group instances, plus a registry of bridge players.
#[derive(Default)]
pub struct PlayerController {
    /// Live sync groups keyed by their `player_id`.
    pub sync_groups: parking_lot::RwLock<HashMap<PlayerId, Arc<SyncGroup>>>,
    /// Live universal groups keyed by their `player_id`.
    pub universal_groups: parking_lot::RwLock<HashMap<PlayerId, Arc<UniversalGroup>>>,
    /// Live bridge players keyed by their `player_id`.
    pub bridges: parking_lot::RwLock<HashMap<PlayerId, Arc<BridgePlayer>>>,
    /// Live Home Assistant `media_player.*` imports, keyed by
    /// `entity_id`. Populated by the HA discovery loop in
    /// [`crate::lib::run`].
    pub ha_players: parking_lot::RwLock<HashMap<PlayerId, Arc<HaPlayer>>>,
}

impl PlayerController {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new sync group and return its id.
    pub fn create_sync_group(&self, config: SyncGroupConfig) -> Arc<SyncGroup> {
        let group = SyncGroup::new(config.clone());
        self.sync_groups
            .write()
            .insert(PlayerId::from(config.player_id.clone()), Arc::clone(&group));
        group
    }

    /// Create a new universal group and return its id.
    pub fn create_universal_group(&self, config: UniversalGroupConfig) -> Arc<UniversalGroup> {
        let group = UniversalGroup::new(config.clone());
        self.universal_groups
            .write()
            .insert(PlayerId::from(config.player_id.clone()), Arc::clone(&group));
        group
    }

    /// Wrap an existing player in a bridge and register it.
    pub fn create_bridge(
        &self,
        inner: Arc<dyn ma_core::player::PlayerControl>,
    ) -> Arc<BridgePlayer> {
        let bridge = BridgePlayer::wrap(inner, None);
        self.bridges.write().insert(
            PlayerId::from(bridge.player_id().to_string()),
            Arc::clone(&bridge),
        );
        bridge
    }

    /// Insert or replace a Home Assistant `media_player` import. The
    /// previous handle (if any) is dropped, which means any
    /// `BridgePlayer` wrapping it must be torn down separately by
    /// the caller.
    pub fn register_ha_player(&self, entity_id: &str, player: Arc<HaPlayer>) -> PlayerId {
        let id = PlayerId::from(entity_id.to_string());
        self.ha_players.write().insert(id.clone(), player);
        id
    }

    /// Remove a HA player from the controller. Returns the dropped
    /// handle (if any) so the caller can also drop any bridge that
    /// was wrapping it.
    pub fn unregister_ha_player(&self, entity_id: &str) -> Option<Arc<HaPlayer>> {
        self.ha_players
            .write()
            .remove(&PlayerId::from(entity_id.to_string()))
    }

    /// Get a snapshot of every registered player (synthetic or
    /// real). Phase 5 will list these via the webserver; for now
    /// we expose the method for tests.
    pub fn all_players(&self) -> Vec<Player> {
        let mut out: Vec<Player> = Vec::new();
        for g in self.sync_groups.read().values() {
            out.push(g.snapshot());
        }
        for g in self.universal_groups.read().values() {
            out.push(g.snapshot());
        }
        for b in self.bridges.read().values() {
            out.push(b.snapshot());
        }
        for p in self.ha_players.read().values() {
            out.push(p.state());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ma_core::enums::{PlaybackState, PlayerType};
    use ma_core::player::{Player, PlayerControl};

    #[tokio::test]
    async fn app_state_carries_controller() {
        let ctrl = Arc::new(PlayerController::new());
        let auth = AuthManager::new();
        let commands = Arc::new(CommandRegistry::new());
        let providers = ma_providers::provider::ProviderRegistry::new();
        let state = AppState::new(
            MassConfig::default(),
            ctrl.clone(),
            auth,
            commands,
            providers,
        );
        assert!(state.player_controller.sync_groups.read().is_empty());
    }

    #[test]
    fn controller_create_and_list() {
        let ctrl = PlayerController::new();
        let group = ctrl.create_sync_group(SyncGroupConfig {
            player_id: "syncgroup_test".into(),
            display_name: "Test".into(),
            is_dynamic: true,
            static_members: Vec::new(),
        });
        assert_eq!(group.player_id(), "syncgroup_test");
        assert_eq!(ctrl.sync_groups.read().len(), 1);
        let players = ctrl.all_players();
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].type_, PlayerType::Group);
        assert_eq!(players[0].provider, "syncgroup");
    }

    #[test]
    fn ha_players_register_and_unregister() {
        let ctrl = PlayerController::new();
        let client = Arc::new(ma_ha::HaClient::new("http://ha.local:8123", "tok", true).unwrap());
        let p = ma_ha::HaPlayer::new("media_player.living".into(), "Living".into(), client);
        let id = ctrl.register_ha_player("media_player.living", p.clone());
        assert_eq!(id.to_string(), "media_player.living");
        assert_eq!(ctrl.ha_players.read().len(), 1);
        assert_eq!(ctrl.all_players().len(), 1);
        let dropped = ctrl.unregister_ha_player("media_player.living");
        assert!(dropped.is_some());
        assert!(ctrl.ha_players.read().is_empty());
    }

    #[test]
    fn unregister_unknown_returns_none() {
        let ctrl = PlayerController::new();
        assert!(ctrl.unregister_ha_player("media_player.missing").is_none());
    }

    #[test]
    fn bridge_wraps_inner() {
        use async_trait::async_trait;
        use ma_core::identifiers::PlayerId;
        use std::sync::Arc;

        #[derive(Debug)]
        struct Stub {
            id: String,
            state: parking_lot::Mutex<Player>,
        }
        impl Stub {
            fn new(id: &str) -> Self {
                let p = Player {
                    player_id: PlayerId::from(id),
                    provider: "sendspin".into(),
                    type_: PlayerType::Player,
                    name: id.into(),
                    available: true,
                    ..Default::default()
                };
                Self {
                    id: id.into(),
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
            fn can_group_with(&self, _: &str) -> bool {
                true
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
            async fn set_volume(&self, _: u32) -> ma_core::Result<()> {
                Ok(())
            }
            async fn set_mute(&self, _: bool) -> ma_core::Result<()> {
                Ok(())
            }
            async fn set_power(&self, _: bool) -> ma_core::Result<()> {
                Ok(())
            }
        }
        let ctrl = PlayerController::new();
        let stub: Arc<dyn PlayerControl> = Arc::new(Stub::new("spk1"));
        let bridge = ctrl.create_bridge(stub);
        assert_eq!(bridge.player_id(), "bridge_spk1");
        assert_eq!(ctrl.bridges.read().len(), 1);
    }
}
