//! End-to-end integration tests for the HTTP surface.
//!
//! These tests stand up the full axum router in-process, point a
//! `reqwest::Client` at it, and exercise:
//!
//! * `GET  /info`           — `ServerInfoMessage` shape
//! * `GET  /health`         — 200 OK
//! * `GET  /api-docs/...`   — JSON docs
//! * `POST /setup`          — admin bootstrap
//! * `POST /auth/login`     — long-lived token
//! * `GET  /auth/me`        — round-trip the bearer
//! * `POST /api`            — JSON-RPC command dispatch (info, players/all, etc.)

use std::net::SocketAddr;
use std::sync::Arc;

use ma_config::MassConfig;
use ma_core::api::CommandRegistry;
use ma_core::identifiers::PlayerId;
use ma_player_sync_group::{SyncGroup, SyncGroupConfig};
use ma_server::auth::AuthManager;
use ma_server::{build_router, AppState, PlayerController};

async fn spawn_app() -> (SocketAddr, Arc<AppState>) {
    let ctrl = Arc::new(PlayerController::new());
    let auth = AuthManager::new();
    let registry = ma_providers::provider::ProviderRegistry::new();
    let placeholder = Arc::new(AppState::new(
        MassConfig::default(),
        ctrl.clone(),
        auth.clone(),
        Arc::new(CommandRegistry::new()),
        registry.clone(),
    ));
    let commands = ma_server::commands::build_registry(placeholder);
    let state = Arc::new(AppState::new(
        MassConfig::default(),
        ctrl.clone(),
        auth.clone(),
        commands,
        registry,
    ));
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(false)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn info_endpoint_returns_server_info_shape() {
    let (addr, _state) = spawn_app().await;
    let resp = http()
        .get(format!("http://{}/info", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["server_id"].is_string());
    assert!(body["schema_version"].is_i64());
    assert!(body["min_supported_schema_version"].is_i64());
    assert!(body["base_url"].is_string());
}

#[tokio::test]
async fn health_endpoint_ok() {
    let (addr, _state) = spawn_app().await;
    let resp = http()
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn commands_json_endpoint_lists_commands() {
    let (addr, _state) = spawn_app().await;
    let resp = http()
        .get(format!("http://{}/api-docs/commands.json", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let list = body["commands"].as_array().unwrap();
    assert!(!list.is_empty());
    let names: Vec<&str> = list.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"players/all"));
    assert!(names.contains(&"info"));
}

#[tokio::test]
async fn api_dispatch_requires_setup_first() {
    let (addr, _state) = spawn_app().await;
    let resp = http()
        .post(format!("http://{}/api", addr))
        .json(&serde_json::json!({
            "message_id": "1",
            "command": "players/all",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn setup_then_login_then_call() {
    let (addr, _state) = spawn_app().await;

    // 1) Bootstrap admin.
    let resp = http()
        .post(format!("http://{}/setup", addr))
        .json(&serde_json::json!({
            "username": "admin",
            "password": "hunter2hunter2",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    let token = body["token"].as_str().unwrap().to_string();

    // 2) Hit /auth/me with the token.
    let resp = http()
        .get(format!("http://{}/auth/me", addr))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");

    // 3) `POST /api` info command (anonymous on the wire, but the
    // server now has users so the gate opens).
    let resp = http()
        .post(format!("http://{}/api", addr))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "message_id": "abc",
            "command": "info",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["message_id"], "abc");
    assert!(body["result"]["server_version"].is_string());

    // 4) `players/all` with a player we add to the controller.
    let group = SyncGroup::new(SyncGroupConfig {
        player_id: "group1".into(),
        display_name: "Group 1".into(),
        is_dynamic: true,
        static_members: Vec::new(),
    });
    _state
        .player_controller
        .sync_groups
        .write()
        .insert(PlayerId::from("group1"), group);

    let resp = http()
        .post(format!("http://{}/api", addr))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "message_id": "1",
            "command": "players/all",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let players = body["result"].as_array().unwrap();
    assert!(players.iter().any(|p| p["player_id"] == "group1"));
}

#[tokio::test]
async fn login_with_wrong_password_returns_401() {
    let (addr, _state) = spawn_app().await;
    // Bootstrap first.
    http()
        .post(format!("http://{}/setup", addr))
        .json(&serde_json::json!({
            "username": "admin",
            "password": "hunter2hunter2",
        }))
        .send()
        .await
        .unwrap();
    let resp = http()
        .post(format!("http://{}/auth/login", addr))
        .json(&serde_json::json!({
            "provider_id": "builtin",
            "credentials": {
                "username": "admin",
                "password": "wrong",
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn api_command_dispatch_returns_error_for_unknown_command() {
    let (addr, _state) = spawn_app().await;
    // Bootstrap.
    http()
        .post(format!("http://{}/setup", addr))
        .json(&serde_json::json!({
            "username": "admin",
            "password": "hunter2hunter2",
        }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = http()
        .post(format!("http://{}/auth/login", addr))
        .json(&serde_json::json!({
            "provider_id": "builtin",
            "credentials": {"username": "admin", "password": "hunter2hunter2"},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = body["token"].as_str().unwrap();
    let resp = http()
        .post(format!("http://{}/api", addr))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "message_id": "1",
            "command": "nope/this/doesnt/exist",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error_code"], 12); // InvalidCommand
}

#[tokio::test]
async fn logo_endpoint_returns_png() {
    let (addr, _state) = spawn_app().await;
    let resp = http()
        .get(format!("http://{}/logo.png", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "image/png");
}
