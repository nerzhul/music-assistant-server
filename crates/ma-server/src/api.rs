//! `api` — JSON-RPC-like command dispatch over HTTP.
//!
//! Exposes the same `/api` and `/auth/*` surface as the Python
//! `WebserverController` so the existing Music Assistant UI can talk to
//! the Rust binary without modification.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use ma_core::api::{CommandContext, RequiredRole};
use ma_core::auth::UserRole;
use ma_core::errors::MusicAssistantError;
use ma_core::messages::{error_message, CommandMessage, ErrorResultMessage, SuccessResultMessage};

use crate::state::AppState;

const MAX_PENDING_MSG: usize = 512;

/// Build the HTTP sub-router that handles `/api`, `/auth/*`, and the
/// auxiliary endpoints. This is merged with the static / WebSocket
/// routers in `build_router`.
pub fn build_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api", post(handle_jsonrpc))
        .route("/auth/login", post(auth_login))
        .route("/auth/logout", post(auth_logout))
        .route("/auth/me", get(auth_me_get).patch(auth_me_patch))
        .route("/auth/providers", get(auth_providers))
        .route("/auth/authorize", get(auth_authorize))
        .route("/auth/callback", get(auth_callback))
        .route("/setup", post(handle_setup))
        .route("/imageproxy", get(handle_imageproxy))
        .route("/preview", get(handle_preview))
        .route("/api-docs/commands.json", get(api_commands_json))
        .route("/api-docs/schemas.json", get(api_schemas_json))
        .route("/api-docs/openapi.json", get(api_openapi_json))
        .route("/sendspin", get(crate::websocket::sendspin_proxy))
        .route("/ws", get(crate::websocket::ws_handler))
}

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default = "default_provider")]
    provider_id: String,
    #[serde(default)]
    credentials: LoginCredentials,
    #[serde(default)]
    return_url: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct LoginCredentials {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

fn default_provider() -> String {
    "builtin".to_string()
}

async fn auth_login(State(state): State<Arc<AppState>>, Json(body): Json<LoginBody>) -> Response {
    if !state.auth.has_users() {
        return json_error(StatusCode::FORBIDDEN, "setup_required", "setup required");
    }
    if body.provider_id != "builtin" {
        return json_error(
            StatusCode::BAD_REQUEST,
            "unsupported",
            &format!("unsupported provider: {}", body.provider_id),
        );
    }
    let user = match state
        .auth
        .verify_password(&body.credentials.username, &body.credentials.password)
    {
        Ok(u) => u,
        Err(_) => {
            return json_error(
                StatusCode::UNAUTHORIZED,
                "authentication_failed",
                "invalid credentials",
            );
        }
    };
    let device = body
        .device_name
        .unwrap_or_else(|| "rust-client".to_string());
    let login = state.auth.create_token(&user, &device).unwrap();
    let mut payload = json!({
        "success": true,
        "token": login.token,
        "user": user,
    });
    if let Some(redirect) = body.return_url {
        let separator = if redirect.contains('?') { "&" } else { "?" };
        let redirect = format!("{}{}code={}", redirect, separator, url_escape(&login.token));
        payload["redirect_to"] = Value::String(redirect);
    }
    json_response(StatusCode::OK, payload)
}

#[derive(Deserialize)]
struct LogoutBody {
    #[serde(default)]
    token: Option<String>,
}

async fn auth_logout(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Option<Json<LogoutBody>>,
) -> Response {
    let token =
        extract_bearer(&headers).or_else(|| body.as_ref().and_then(|Json(b)| b.token.clone()));
    if let Some(t) = token {
        let _ = state.auth.revoke_token(&t);
    }
    json_response(StatusCode::OK, json!({"success": true}))
}

async fn auth_me_get(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    match authenticated_user(&state, &headers) {
        Some(user) => json_response(StatusCode::OK, json!(user)),
        None => json_error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "authentication required",
        ),
    }
}

#[derive(Deserialize, Default)]
struct UpdateMeBody {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

async fn auth_me_patch(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<UpdateMeBody>,
) -> Response {
    let user = match authenticated_user(&state, &headers) {
        Some(u) => u,
        None => {
            return json_error(
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "authentication required",
            )
        }
    };
    let updated = match state
        .auth
        .update_profile(&user.user_id, body.display_name, body.avatar_url)
    {
        Ok(u) => u,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                &e.to_string(),
            )
        }
    };
    json_response(StatusCode::OK, json!({"success": true, "user": updated}))
}

async fn auth_providers(State(_state): State<Arc<AppState>>) -> Response {
    json_response(
        StatusCode::OK,
        json!([
            {
                "id": "builtin",
                "name": "Built-in",
                "type": "builtin",
                "username_label": "Username",
                "password_label": "Password",
            }
        ]),
    )
}

async fn auth_authorize(State(_state): State<Arc<AppState>>) -> Response {
    json_error(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "OAuth providers are not yet wired in the Rust port",
    )
}

async fn auth_callback(State(_state): State<Arc<AppState>>) -> Response {
    json_error(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "OAuth callback is not yet wired in the Rust port",
    )
}

#[derive(Deserialize)]
struct SetupBody {
    username: String,
    password: String,
    #[serde(default)]
    device_name: Option<String>,
}

async fn handle_setup(State(state): State<Arc<AppState>>, Json(body): Json<SetupBody>) -> Response {
    if state.auth.has_users() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "setup_already_done",
            "setup already completed",
        );
    }
    if body.username.len() < 2 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "username must be at least 2 characters",
        );
    }
    if body.password.len() < 8 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "password must be at least 8 characters",
        );
    }
    let (user, _printed) = match state
        .auth
        .bootstrap_admin(&body.username, Some(body.password))
    {
        Ok(p) => p,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                &e.to_string(),
            )
        }
    };
    let device = body.device_name.unwrap_or_else(|| "setup".to_string());
    let login = state.auth.create_token(&user, &device).unwrap();
    json_response(
        StatusCode::OK,
        json!({
            "success": true,
            "token": login.token,
            "user": user,
        }),
    )
}

async fn handle_jsonrpc(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !state.auth.has_users() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_required",
            "setup required",
        );
    }
    let cmd: CommandMessage = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                &format!("invalid JSON: {e}"),
            );
        }
    };
    let ctx = build_context(&state, &headers);
    match state.commands.dispatch(&cmd, &ctx).await {
        Ok(success) => json_message(StatusCode::OK, &success),
        Err(err) => json_message(StatusCode::OK, &err),
    }
}

async fn handle_imageproxy(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    // The legacy `/imageproxy?id=...&provider=...&size=...` form. Phase
    // 5 returns a 404 until the cover provider ships a stable lookup
    // table; this keeps the route registered so the UI doesn't crash.
    if !state.auth.has_users() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_required",
            "setup required",
        );
    }
    json_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "imageproxy not yet implemented in the Rust port",
    )
    .with_query_hint(params.get("id").cloned().unwrap_or_default())
}

async fn handle_preview(State(_state): State<Arc<AppState>>) -> Response {
    json_error(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "preview stream is not yet wired in the Rust port",
    )
}

async fn api_commands_json(State(state): State<Arc<AppState>>) -> Response {
    let list: Vec<_> = state
        .commands
        .list()
        .into_iter()
        .map(|c| {
            json!({
                "name": c.name,
                "required_role": match c.required_role {
                    RequiredRole::Anonymous => "anonymous",
                    RequiredRole::Authenticated => "authenticated",
                    RequiredRole::Admin => "admin",
                },
                "required_args": c.required_args,
                "description": c.description,
            })
        })
        .collect();
    json_response(StatusCode::OK, json!({"commands": list}))
}

async fn api_schemas_json() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "schemas": [
                {"name": "CommandMessage", "module": "music_assistant_models.api"},
                {"name": "SuccessResultMessage", "module": "music_assistant_models.api"},
                {"name": "ErrorResultMessage", "module": "music_assistant_models.api"},
                {"name": "EventMessage", "module": "music_assistant_models.api"},
                {"name": "ServerInfoMessage", "module": "music_assistant_models.api"},
            ]
        }),
    )
}

async fn api_openapi_json() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "openapi": "3.0.0",
            "info": {"title": "Music Assistant Rust", "version": env!("CARGO_PKG_VERSION")},
            "paths": {},
        }),
    )
}

pub(crate) fn build_context(state: &AppState, headers: &axum::http::HeaderMap) -> CommandContext {
    let user = authenticated_user(state, headers);
    CommandContext {
        user_id: user.as_ref().map(|u| u.user_id.clone()),
        role: user.as_ref().map(|u| u.role),
        sendspin_player_id: None,
    }
}

pub(crate) fn authenticated_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<ma_core::auth::User> {
    let token = extract_bearer(headers)?;
    state.auth.authenticate_with_token(&token)
}

pub(crate) fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if let Some(rest) = raw.strip_prefix("Bearer ") {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = raw.strip_prefix("bearer ") {
        return Some(rest.trim().to_string());
    }
    None
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let body = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(Body::from(body))
        .expect("json response")
}

fn json_error(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(
        status,
        json!({
            "success": false,
            "error_code": code,
            "error": message,
        }),
    )
}

fn json_message<T: serde::Serialize>(status: StatusCode, msg: &T) -> Response {
    let body = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(Body::from(body))
        .expect("message response")
}

fn url_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

trait ResponseExt {
    fn with_query_hint(self, _hint: String) -> Self;
}
impl ResponseExt for Response {
    fn with_query_hint(self, _hint: String) -> Self {
        self
    }
}

#[allow(dead_code)]
fn _error_message_id_helper(message_id: &str, e: MusicAssistantError) -> ErrorResultMessage {
    error_message(message_id, e.code().as_i32(), e.to_string())
}

#[allow(dead_code)]
fn _success_message_id_helper<T: serde::Serialize>(
    message_id: &str,
    value: T,
) -> serde_json::Result<SuccessResultMessage> {
    use serde_json::to_value;
    let v = to_value(value)?;
    Ok(SuccessResultMessage {
        message_id: message_id.to_string(),
        result: Some(v),
        partial: false,
    })
}

#[allow(dead_code)]
fn _user_role_helper() -> Option<UserRole> {
    Some(UserRole::User)
}

#[allow(dead_code)]
fn _ensure_max_pending() -> usize {
    MAX_PENDING_MSG
}
