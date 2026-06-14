//! Integration tests for the Home Assistant client + discovery loop.
//!
//! We spin up a tiny axum mock server that mimics the HA REST API
//! (`/api/`, `/api/states`, `/api/services/...`) and exercise both
//! the `HaClient` and the `Discover` loop against it.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use ma_core::player::PlayerControl;
use ma_ha::client::{HaClient, HaEntity};
use ma_ha::config::HaConfig;
use ma_ha::discover::Discover;
use ma_ha::player::HaPlayer;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Default, Clone)]
struct AppState {
    /// Recorded call_service calls. We assert against this in the
    /// command-forwarding tests.
    calls: Arc<Mutex<Vec<(String, String, serde_json::Value)>>>,
}

async fn api_root() -> &'static str {
    r#"{"message":"API running.","version":"2024.4"}"#
}

async fn list_states() -> Json<Vec<HaEntity>> {
    Json(vec![
        HaEntity {
            entity_id: "media_player.living_room".into(),
            state: "playing".into(),
            attributes: serde_json::json!({
                "volume_level": 0.4,
                "is_volume_muted": false,
                "media_title": "Song A",
                "media_artist": "Artist X",
                "media_content_id": "https://example.com/song",
            }),
            ..Default::default()
        },
        HaEntity {
            entity_id: "media_player.kitchen".into(),
            state: "off".into(),
            attributes: serde_json::json!({"volume_level": 0.2}),
            ..Default::default()
        },
        HaEntity {
            entity_id: "light.bedroom".into(),
            state: "on".into(),
            attributes: serde_json::json!({}),
            ..Default::default()
        },
    ])
}

async fn call_service(
    State(state): State<AppState>,
    Path((domain, service)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> &'static str {
    state.calls.lock().await.push((domain, service, body));
    "[]"
}

async fn spawn_mock() -> (SocketAddr, AppState) {
    let state = AppState::default();
    let app = Router::new()
        .route("/api/", get(api_root))
        .route("/api/states", get(list_states))
        .route(
            "/api/services/:domain/:service",
            post(call_service).with_state(state.clone()),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (addr, state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_ping_works() {
    let (addr, _state) = spawn_mock().await;
    let client = HaClient::new(format!("http://{addr}"), "tok", true).unwrap();
    let body = client.ping().await.unwrap();
    assert_eq!(body["message"], "API running.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_states_filters_media_players() {
    let (addr, _state) = spawn_mock().await;
    let client = HaClient::new(format!("http://{addr}"), "tok", true).unwrap();
    let states = client.list_states().await.unwrap();
    let mp: Vec<&HaEntity> = states.iter().filter(|e| e.is_media_player()).collect();
    assert_eq!(mp.len(), 2);
    assert!(mp.iter().any(|e| e.entity_id == "media_player.living_room"));
    assert!(mp.iter().any(|e| e.entity_id == "media_player.kitchen"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_loop_returns_two_players() {
    let (addr, _state) = spawn_mock().await;
    let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
    let cfg = HaConfig::default();
    let d = Arc::new(Discover::new(&cfg, client).unwrap());
    let snapshot = d.poll_once().await.unwrap();
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot
        .iter()
        .any(|s| s.entity.entity_id == "media_player.living_room"));
    assert!(snapshot
        .iter()
        .any(|s| s.entity.entity_id == "media_player.kitchen"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_with_glob_filter() {
    let (addr, _state) = spawn_mock().await;
    let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
    let cfg = HaConfig {
        include_entity_id_globs: vec![r"media_player\.living_.*".into()],
        ..Default::default()
    };
    let d = Discover::new(&cfg, client).unwrap();
    let snapshot = d.poll_once().await.unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].entity.entity_id, "media_player.living_room");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_forwards_command() {
    let (addr, state) = spawn_mock().await;
    let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
    let player = HaPlayer::new(
        "media_player.living_room".into(),
        "Living Room".into(),
        client,
    );
    // Update the snapshot to "playing" first.
    player.update_from_entity(&HaEntity {
        entity_id: "media_player.living_room".into(),
        state: "playing".into(),
        attributes: serde_json::json!({"volume_level": 0.5}),
        ..Default::default()
    });
    // Issue a play command (idempotent: it forwards media_play).
    player.play().await.unwrap();
    // Give the call a moment to be recorded.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let calls = state.calls.lock().await.clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "media_player");
    assert_eq!(calls[0].1, "media_play");
    assert_eq!(calls[0].2["entity_id"], "media_player.living_room");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn player_volume_set_clamps_and_forwards() {
    let (addr, state) = spawn_mock().await;
    let client = Arc::new(HaClient::new(format!("http://{addr}"), "tok", true).unwrap());
    let player = HaPlayer::new("media_player.living_room".into(), "Living".into(), client);
    player.set_volume(75).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let calls = state.calls.lock().await.clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "volume_set");
    let v = calls[0].2["volume_level"].as_f64().unwrap();
    assert!((v - 0.75).abs() < 1e-6);
}
