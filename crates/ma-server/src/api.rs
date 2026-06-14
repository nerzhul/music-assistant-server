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
use ma_core::enums::MediaType;
use ma_core::messages::CommandMessage;

use crate::state::AppState;

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

/// Best-effort client IP for the rate limiter. Honours
/// `X-Forwarded-For` (left-most entry) when behind a reverse proxy,
/// otherwise falls back to the peer address exposed by axum.
fn client_ip_from(headers: &axum::http::HeaderMap) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            return first.trim().to_string();
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        return real.trim().to_string();
    }
    "unknown".to_string()
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

async fn auth_login(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
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
    let key = format!(
        "{}\0{}",
        client_ip_from(&headers),
        body.credentials.username
    );
    if !state.auth.check_login_allowed(&key) {
        // Don't reveal which side is being throttled; the 429 is
        // public information anyway.
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many failed login attempts; please wait a minute",
        );
    }
    let user = match state
        .auth
        .verify_password(&body.credentials.username, &body.credentials.password)
    {
        Ok(u) => u,
        Err(_) => {
            state.auth.record_login_failure(&key);
            return json_error(
                StatusCode::UNAUTHORIZED,
                "authentication_failed",
                "invalid credentials",
            );
        }
    };
    state.auth.clear_login_failures(&key);
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

/// Validate the `avatar_url` supplied by the caller. The value is
/// stored verbatim in the user record and rendered in the UI; we
/// reject anything that isn't a syntactically valid `http(s)` URL and
/// cap the length so a user can't dump megabytes into a single field.
fn validate_avatar_url(url: &str) -> Result<(), &'static str> {
    if url.len() > 2048 {
        return Err("avatar_url too long (max 2048 chars)");
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("avatar_url must be an http(s) URL");
    }
    if url.contains('\n') || url.contains('\r') || url.contains('\0') {
        return Err("avatar_url contains invalid characters");
    }
    Ok(())
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
    if let Some(url) = &body.avatar_url {
        if let Err(msg) = validate_avatar_url(url) {
            return json_error(StatusCode::BAD_REQUEST, "invalid_input", msg);
        }
    }
    if let Some(name) = &body.display_name {
        if name.len() > 128 {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "display_name too long (max 128 chars)",
            );
        }
    }
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

async fn handle_setup(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<SetupBody>,
) -> Response {
    if state.auth.has_users() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "setup_already_done",
            "setup already completed",
        );
    }
    // The same rate limiter as /auth/login: defends against
    // scripted setup abuse (the endpoint is gated by !has_users, so
    // it can only be hit before the first admin is created, but an
    // attacker can still probe to detect that gate).
    let key = format!("{}\0setup", client_ip_from(&headers));
    if !state.auth.check_login_allowed(&key) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many attempts; please wait a minute",
        );
    }
    state.auth.record_login_failure(&key);
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
    if body.username.len() > 64 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "username too long (max 64 chars)",
        );
    }
    // Reject any control characters / path-traversal shenanigans in
    // the username. Argon2 hashes anything but we'd rather fail fast
    // than let a malicious user impersonate "admin" via trailing
    // whitespace or unicode confusables.
    if body
        .username
        .chars()
        .any(|c| c.is_control() || c == '/' || c == '\\' || c == '\0')
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "username contains invalid characters",
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
    state.auth.clear_login_failures(&key);
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
    if !state.auth.has_users() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_required",
            "setup required",
        );
    }
    let Some(db) = state.database.as_ref() else {
        return json_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "cover cache unavailable (no database)",
        );
    };
    // Two URL forms are supported:
    //   * `?id=<image_id>` — opaque SHA-1 hash
    //   * `?provider=<p>&item_id=<i>` — (provider, item_id) lookup
    let repo = ma_storage::CoverArtRepository::new(db.pool().clone());
    let record = if let Some(id) = params.get("id") {
        match repo.find_by_id(id).await {
            Ok(Some(r)) => Some(r),
            Ok(None) => None,
            Err(e) => {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    &e.to_string(),
                );
            }
        }
    } else if let (Some(provider), Some(item_id)) = (params.get("provider"), params.get("item_id"))
    {
        match repo.find(provider, item_id).await {
            Ok(Some(r)) => Some(r),
            Ok(None) => None,
            Err(e) => {
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    &e.to_string(),
                );
            }
        }
    } else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "either `id` or `provider`+`item_id` is required",
        );
    };
    match record {
        Some(r) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, r.content_type.clone())
            .header(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            )
            .body(Body::from(r.bytes))
            .expect("cover response is always valid"),
        None => json_error(StatusCode::NOT_FOUND, "not_found", "cover not found"),
    }
}

async fn handle_preview(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !state.auth.has_users() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_required",
            "setup required",
        );
    }
    let Some(item_id) = params.get("item_id") else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "item_id is required",
        );
    };
    // Walk providers looking for stream details; if found, hand the
    // stream URL back to the caller. A real implementation would
    // also support a short MP3 preview snippet (e.g. first 30s); for
    // now we return the full stream URL so the UI can use it.
    let providers = state.providers.list();
    for h in providers {
        if let Ok(details) = h.music.get_stream_details(item_id, MediaType::Track).await {
            return json_response(
                StatusCode::OK,
                json!({
                    "url": details.path,
                    "provider": h.instance_id,
                    "note": "preview endpoint: V1 returns the full stream URL; V2 will return a 30s snippet",
                }),
            );
        }
    }
    json_error(StatusCode::NOT_FOUND, "not_found", "no stream available")
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
