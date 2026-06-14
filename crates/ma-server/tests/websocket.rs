//! WebSocket integration test for the `/ws` endpoint.

use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use ma_config::MassConfig;
use ma_server::auth::AuthManager;
use ma_server::{build_router, AppState};

use tokio_tungstenite::tungstenite::Message;

async fn spawn_app_with_token() -> (SocketAddr, String) {
    let ctrl = Arc::new(ma_server::PlayerController::new());
    let auth = AuthManager::new();
    let registry = ma_providers::provider::ProviderRegistry::new();
    let placeholder = Arc::new(AppState::new(
        MassConfig::default(),
        ctrl.clone(),
        auth.clone(),
        Arc::new(ma_core::api::CommandRegistry::new()),
        registry.clone(),
    ));
    let commands = ma_server::commands::build_registry(placeholder);
    let state = Arc::new(AppState::new(
        MassConfig::default(),
        ctrl,
        auth.clone(),
        commands,
        registry,
    ));
    let (admin, _) = state
        .auth
        .bootstrap_admin("admin", Some("hunter2hunter2".into()))
        .unwrap();
    let login = state.auth.create_token(&admin, "test").unwrap();
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, login.token)
}

#[tokio::test]
async fn ws_handshake_then_info_command() {
    let (addr, token) = spawn_app_with_token().await;
    let url = format!("ws://{}/ws?token={}", addr, token);
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();

    // 1) First message from server is `ServerInfo`.
    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(body["server_id"].is_string());
    assert!(body["schema_version"].is_i64());

    // 2) Send an `info` command.
    let payload: String = serde_json::json!({
        "message_id": "1",
        "command": "info",
    })
    .to_string();
    ws.send(Message::Text(payload)).await.unwrap();

    // 3) Read the response.
    let mut found_info = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let Some(Ok(msg)) = ws.next().await {
            if let Ok(text) = msg.into_text() {
                let body: serde_json::Value = serde_json::from_str(&text).unwrap();
                if body["message_id"] == "1" {
                    assert!(
                        body["result"]["server_version"].is_string()
                            || body["result"]["result"]["server_version"].is_string()
                    );
                    found_info = true;
                    break;
                }
            }
        } else {
            break;
        }
    }
    assert!(found_info, "did not receive info response over WS");
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn ws_unknown_command_returns_error_message() {
    let (addr, token) = spawn_app_with_token().await;
    let url = format!("ws://{}/ws?token={}", addr, token);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let _ = ws.next().await; // server info

    ws.send(Message::Text(
        serde_json::json!({
            "message_id": "1",
            "command": "nope",
        })
        .to_string(),
    ))
    .await
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let Some(Ok(msg)) = ws.next().await {
            if let Ok(text) = msg.into_text() {
                let body: serde_json::Value = serde_json::from_str(&text).unwrap();
                if body["message_id"] == "1" {
                    assert_eq!(body["error_code"], 12);
                    let _ = ws.close(None).await;
                    return;
                }
            }
        } else {
            break;
        }
    }
    panic!("did not receive error response");
}
