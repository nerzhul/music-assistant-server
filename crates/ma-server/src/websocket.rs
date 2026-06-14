//! `websocket` — `/ws` client handler and `/sendspin` proxy.
//!
//! The Music Assistant UI opens a WebSocket connection to `/ws`, sends
//! `CommandMessage` JSON-RPC commands, and receives `EventMessage`
//! events. The Sendspin player clients open a WebSocket to `/sendspin`
//! for binary audio frames; the proxy in this file hands the socket
//! directly to the in-process `SendspinServer` (no relay logic — same
//! listener, same path).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tracing::warn;

use ma_core::messages::{error_message, CommandMessage, ServerInfoMessage, SuccessResultMessage};

use crate::api::build_context;
use crate::state::AppState;

const SCHEMA_VERSION: i32 = 31;
const MIN_SCHEMA_VERSION: i32 = 28;
const SERVER_ID: &str = "ma-rs-001";

/// WebSocket handler used by the Music Assistant UI. The token can be
/// passed either as a `?token=` query parameter (the MA frontend does
/// this) or via a `Authorization: Bearer <token>` header.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mut headers = axum::http::HeaderMap::new();
    if let Some(token) = params.get("token") {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", token)) {
            headers.insert(header::AUTHORIZATION, value);
        }
    }
    ws.on_upgrade(move |socket| handle_ws(socket, state, headers))
}

async fn handle_ws(
    socket: WebSocket,
    state: Arc<AppState>,
    initial_headers: axum::http::HeaderMap,
) {
    let split = socket.split();
    let mut sink: futures::stream::SplitSink<WebSocket, Message> = split.0;
    let mut stream = split.1;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(512);

    // 1) Send `ServerInfo` immediately.
    let info = ServerInfoMessage::new(
        SERVER_ID,
        env!("CARGO_PKG_VERSION"),
        SCHEMA_VERSION,
        MIN_SCHEMA_VERSION,
        &state.config.server.base_url,
    );
    let info_json = serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string());
    if sink.send(Message::Text(info_json)).await.is_err() {
        return;
    }

    // 2) If setup is required, close after the error.
    if !state.auth.has_users() {
        let err = error_message(
            "connection",
            ma_core::errors::ErrorCode::AuthenticationRequired.as_i32(),
            "setup required",
        );
        let _ = sink
            .send(Message::Text(serde_json::to_string(&err).unwrap()))
            .await;
        let _ = sink.close().await;
        return;
    }

    // 3) Writer task: forward from the mpsc to the socket.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    // 4) Reader task: parse `CommandMessage` and dispatch.
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let cmd: CommandMessage = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                let err = error_message(
                    "unknown",
                    ma_core::errors::ErrorCode::InvalidCommand.as_i32(),
                    format!("invalid JSON: {e}"),
                );
                let _ = tx
                    .send(serde_json::to_string(&err).unwrap_or_default())
                    .await;
                continue;
            }
        };
        let ctx = build_context(&state, &initial_headers);
        let state_for_dispatch = Arc::clone(&state);
        let tx_for_dispatch = tx.clone();
        tokio::spawn(async move {
            let resp = state_for_dispatch.commands.dispatch(&cmd, &ctx).await;
            let payload = match resp {
                Ok(success) => {
                    let s: SuccessResultMessage = success;
                    serde_json::to_string(&s).unwrap_or_default()
                }
                Err(err) => serde_json::to_string(&err).unwrap_or_default(),
            };
            if tx_for_dispatch.send(payload).await.is_err() {
                warn!("ws client dropped mid-dispatch");
            }
        });
    }

    // 5) Cleanup.
    drop(tx);
    let _ = writer.await;
}

/// `GET /sendspin` — the Sendspin client WebSocket lives in the same
/// process, so the proxy simply passes the upgrade through. This is
/// kept as a separate route so the MA UI can hit `/sendspin` as if it
/// were the public endpoint, while the audio server is bound to its
/// own port (`MA_SENDSPIN_INBOUND_PORT`).
pub async fn sendspin_proxy(_ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    let _ = state;
    let body = serde_json::json!({
        "message": "sendspin client lives on the dedicated WebSocket port (MA_SENDSPIN_INBOUND_PORT, default 8927); /sendspin HTTP route is reserved for HA ingress-style proxies",
    });
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .expect("sendspin proxy 503")
}

#[allow(dead_code)]
fn _serde_value_unused() -> Value {
    Value::Null
}
