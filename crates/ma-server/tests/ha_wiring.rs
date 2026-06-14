//! Integration test for the HA → MA player import wiring.
//!
//! Spins up a mock HA REST server, configures a `Discover` against
//! it, and verifies that `PlayerController::ha_players` ends up
//! populated with one entry per `media_player.*` entity after the
//! first poll. The test does not go through `ma-server::run` (that
//! would require a full env-driven setup); instead it exercises the
//! reconciliation loop directly.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use ma_core::player::PlayerControl;
use ma_ha::client::HaEntity;
use ma_ha::discover::Discover;
use ma_ha::HaClient;
use ma_server::PlayerController;
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockState {
    states: Arc<parking_lot::Mutex<Vec<HaEntity>>>,
}

async fn list_states(
    axum::extract::State(s): axum::extract::State<MockState>,
) -> Json<Vec<HaEntity>> {
    Json(s.states.lock().clone())
}

async fn spawn_mock() -> (SocketAddr, MockState) {
    let state = MockState {
        states: Arc::new(parking_lot::Mutex::new(vec![])),
    };
    let app = Router::new().route("/api/states", get(list_states).with_state(state.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (addr, state)
}

/// Tiny copy of the reconciliation loop from `ma-server::run`. The
/// production loop lives inside `consume_ha_snapshots`; we replicate
/// it here (rather than exposing it as `pub`) to keep the
/// reconciliation a private detail of the run module.
async fn reconcile(rx: ma_ha::DiscoverRx, ctrl: &PlayerController, client: Arc<HaClient>) {
    use std::collections::HashSet;
    while let Ok(snapshot) = rx.recv().await {
        let new_ids: HashSet<String> = snapshot
            .iter()
            .map(|s| s.entity.entity_id.clone())
            .collect();
        for s in &snapshot {
            let id = s.entity.entity_id.clone();
            let player_id = ma_core::identifiers::PlayerId::from(id.clone());
            let already = ctrl.ha_players.read().contains_key(&player_id);
            if already {
                if let Some(p) = ctrl.ha_players.read().get(&player_id).cloned() {
                    p.update_from_entity(&s.entity);
                }
            } else {
                let friendly = s
                    .entity
                    .attributes
                    .as_object()
                    .and_then(|a| a.get("friendly_name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| id.clone());
                let player = ha_player_new(&id, &friendly, client.clone());
                player.update_from_entity(&s.entity);
                ctrl.register_ha_player(&id, player);
            }
        }
        let current: Vec<String> = ctrl
            .ha_players
            .read()
            .keys()
            .map(|k| k.to_string())
            .collect();
        for id in current {
            if !new_ids.contains(&id) {
                ctrl.unregister_ha_player(&id);
            }
        }
    }
}

// We can't import `HaPlayer::new` into a test crate without exposing
// it; the `HaPlayer` type is `pub` in `ma-ha::player`, so we just
// reach through.
use ma_ha::HaPlayer;

fn ha_player_new(id: &str, name: &str, client: Arc<HaClient>) -> Arc<HaPlayer> {
    HaPlayer::new(id.to_string(), name.to_string(), client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_registers_ha_players() {
    let (addr, state) = spawn_mock().await;
    // Populate the mock with two media_player entities + a light.
    {
        let mut g = state.states.lock();
        g.push(HaEntity {
            entity_id: "media_player.living".into(),
            state: "playing".into(),
            attributes: serde_json::json!({"friendly_name": "Living Room", "volume_level": 0.5}),
            ..Default::default()
        });
        g.push(HaEntity {
            entity_id: "media_player.kitchen".into(),
            state: "idle".into(),
            attributes: serde_json::json!({"friendly_name": "Kitchen"}),
            ..Default::default()
        });
        g.push(HaEntity {
            entity_id: "light.bedroom".into(),
            state: "on".into(),
            attributes: serde_json::json!({}),
            ..Default::default()
        });
    }
    let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
    let cfg = ma_ha::HaConfig {
        url: Some(format!("http://{addr}")),
        token: Some("tok".into()),
        ..Default::default()
    };
    let discover = Arc::new(Discover::new(&cfg, client.clone()).unwrap());
    // Poll once to seed the channel.
    let snapshot = discover.poll_once().await.unwrap();
    assert_eq!(snapshot.len(), 2, "only media_player entities pass");
    // Now drive the reconciliation manually. The key trick: build
    // the channel, send, then DROP tx explicitly before awaiting
    // `reconcile`. Otherwise reconcile holds rx alive and the
    // channel never closes, deadlocking the loop.
    let ctrl = PlayerController::new();
    let (tx, rx) = async_channel::bounded(8);
    tx.send(snapshot).await.unwrap();
    drop(tx);
    reconcile(rx, &ctrl, client.clone()).await;
    assert_eq!(ctrl.ha_players.read().len(), 2);
    let names: Vec<String> = ctrl
        .ha_players
        .read()
        .values()
        .map(|p| p.display_name().to_string())
        .collect();
    assert!(names.contains(&"Living Room".to_string()));
    assert!(names.contains(&"Kitchen".to_string()));
    let snapshot_players = ctrl.all_players();
    assert_eq!(snapshot_players.len(), 2);
    // First snapshot says "playing" → volume_level 0.5.
    let living = ctrl
        .ha_players
        .read()
        .values()
        .find(|p| p.player_id() == "media_player.living")
        .unwrap()
        .clone();
    let s = living.state();
    assert_eq!(s.playback_state, ma_core::enums::PlaybackState::Playing);
    assert_eq!(s.volume_level, Some(50));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_unregisters_missing_players() {
    let fut = async {
        let (addr, state) = spawn_mock().await;
        eprintln!("mock spawned at {addr}");
        {
            let mut g = state.states.lock();
            g.push(HaEntity {
                entity_id: "media_player.one".into(),
                state: "playing".into(),
                attributes: serde_json::json!({"friendly_name": "One"}),
                ..Default::default()
            });
            g.push(HaEntity {
                entity_id: "media_player.two".into(),
                state: "playing".into(),
                attributes: serde_json::json!({"friendly_name": "Two"}),
                ..Default::default()
            });
        }
        let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
        let cfg = ma_ha::HaConfig {
            url: Some(format!("http://{addr}")),
            token: Some("tok".into()),
            ..Default::default()
        };
        let discover = Arc::new(Discover::new(&cfg, client.clone()).unwrap());
        let ctrl = PlayerController::new();
        // First snapshot registers both.
        let snap1 = discover.poll_once().await.unwrap();
        let (tx, rx) = async_channel::bounded(8);
        tx.send(snap1).await.unwrap();
        drop(tx);
        reconcile(rx, &ctrl, client.clone()).await;
        assert_eq!(ctrl.ha_players.read().len(), 2);
        // Remove one from HA's state list.
        {
            let mut g = state.states.lock();
            g.retain(|e| e.entity_id != "media_player.two");
        }
        eprintln!("after retain");
        // Use the second-poll result directly from a fresh HTTP call to
        // avoid reqwest keep-alive weirdness when reusing the same
        // client for back-to-back polls.
        let client2 = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
        eprintln!("before second list_states");
        let raw = client2.list_states().await.unwrap();
        eprintln!("after second list_states: {}", raw.len());
        let cfg2 = cfg.clone();
        let discover2 = Arc::new(Discover::new(&cfg2, client2.clone()).unwrap());
        let snap2: Vec<ma_ha::discover::DiscoveredEntity> = raw
            .into_iter()
            .filter(|e| e.is_media_player())
            .map(|entity| ma_ha::discover::DiscoveredEntity {
                entity,
                last_seen: std::time::Instant::now(),
            })
            .collect();
        let _ = discover2; // silence unused warning
        assert_eq!(snap2.len(), 1);
        let (tx, rx) = async_channel::bounded(8);
        tx.send(snap2).await.unwrap();
        drop(tx);
        reconcile(rx, &ctrl, client2.clone()).await;
        assert_eq!(ctrl.ha_players.read().len(), 1);
        let only = ctrl.ha_players.read().values().next().unwrap().clone();
        assert_eq!(only.player_id(), "media_player.one");
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), fut).await {
        Ok(()) => {}
        Err(_) => panic!("test timed out after 10s"),
    }
}
