//! `ma-server` — Music Assistant Rust server binary.
//!
//! Phase 5 exposes the full HTTP / WebSocket / auth surface the
//! existing Music Assistant UI needs to drive the server:
//!
//! * `GET    /info`              — `ServerInfoMessage` JSON
//! * `GET    /logo.png`          — the bundled Music Assistant logo
//! * `GET    /`                  — root placeholder
//! * `GET    /health`            — health check
//! * `POST   /api`               — JSON-RPC-like command dispatch
//! * `GET    /ws`                — WebSocket command + event channel
//! * `GET    /sendspin`          — Sendspin proxy (HTTP 503, see websocket.rs)
//! * `POST   /auth/login`        — login (returns a long-lived token)
//! * `POST   /auth/logout`       — revoke a token
//! * `GET    /auth/me`           — current user
//! * `PATCH  /auth/me`           — update profile
//! * `GET    /auth/providers`    — list login providers
//! * `POST   /setup`             — first-time admin bootstrap
//! * `GET    /api-docs/*.json`   — auto-generated command / schema docs
//! * `GET    /imageproxy?...`    — cover art proxy (404 until Phase 6)
//! * `GET    /preview`           — track preview stream (501 until Phase 6)

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use ma_config::MassConfig;
use ma_core::messages::ServerInfoMessage;

mod state;

pub mod api;
pub mod auth;
pub mod commands;
pub mod websocket;

pub use state::{AppState, PlayerController};

const SCHEMA_VERSION: i32 = 31;
const MIN_SCHEMA_VERSION: i32 = 28;
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const SERVER_ID: &str = "ma-rs-001";

/// Returns the bundled logo.png as a static `&'static [u8]`.
///
/// The file is embedded at compile time via `include_bytes!` so the binary
/// works without any on-disk resource directory.
fn logo_png() -> &'static [u8] {
    static LOGO: &[u8] = include_bytes!("../static/logo.png");
    LOGO
}

async fn get_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let info = ServerInfoMessage::new(
        SERVER_ID,
        SERVER_VERSION,
        SCHEMA_VERSION,
        MIN_SCHEMA_VERSION,
        &state.config.server.base_url,
    );
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_string()),
    )
}

async fn get_logo() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_static("image/png"))
        .header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        )
        .body(Body::from(logo_png()))
        .expect("static logo response is always valid")
}

async fn get_health() -> &'static str {
    "ok"
}

async fn get_root() -> &'static str {
    "Music Assistant (Rust) - phase 5"
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(get_root))
        .route("/health", get(get_health))
        .route("/info", get(get_info))
        .route("/logo.png", get(get_logo))
        .merge(crate::api::build_router())
        .layer(CorsLayer::very_permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(config: MassConfig) -> anyhow::Result<()> {
    let player_controller = Arc::new(PlayerController::new());
    let auth = crate::auth::AuthManager::new();
    let registry = ma_providers::provider::ProviderRegistry::new();
    register_builtin_providers(&registry, &config);
    info!(providers = registry.list().len(), "providers registered");
    let registry_arc = registry;

    // Build a placeholder AppState so the commands registry can close
    // over it, then swap in the real one.
    let placeholder = Arc::new(AppState::new(
        config.clone(),
        player_controller.clone(),
        auth.clone(),
        Arc::new(ma_core::api::CommandRegistry::new()),
        Arc::clone(&registry_arc),
    ));
    let commands = crate::commands::build_registry(placeholder.clone());
    let state = Arc::new(AppState::new(
        config.clone(),
        player_controller.clone(),
        auth.clone(),
        commands,
        registry_arc.clone(),
    ));

    // Bootstrap the initial admin user. Either the envvar
    // `MA_AUTH_INITIAL_PASSWORD` is honoured, or a random password is
    // generated and printed to the log.
    let (admin_user, printed) = state
        .auth
        .bootstrap_admin("admin", std::env::var("MA_AUTH_INITIAL_PASSWORD").ok())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if let Some(pw) = printed {
        tracing::warn!(
            username = %admin_user.username,
            password = %pw,
            "first admin user created; please change this password"
        );
    }

    let bind = SocketAddr::new(
        state.config.server.bind_ip.parse()?,
        state.config.server.bind_port,
    );
    let app = build_router(state.clone());

    info!(%bind, "starting ma-server (phase 5: full http + ws + auth + commands)");

    info!(
        sync_groups = player_controller.sync_groups.read().len(),
        universal_groups = player_controller.universal_groups.read().len(),
        bridges = player_controller.bridges.read().len(),
        "player controller ready"
    );

    // Spawn the Sendspin WebSocket server on its own port (default 8927).
    if config.sendspin.enabled {
        let sendspin =
            ma_player_sendspin::SendspinServer::new(ma_player_sendspin::SendspinServerConfig {
                bind_ip: config.sendspin.bind_ip.clone(),
                inbound_port: config.sendspin.inbound_port,
                inbound_path: "/sendspin".to_string(),
                server_id: uuid::Uuid::new_v4().to_string(),
                server_name: config.sendspin.server_name.clone(),
            });
        tokio::spawn(async move {
            if let Err(e) = sendspin.run().await {
                tracing::error!(error = %e, "sendspin server exited with error");
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Builtin / example provider set. Wires the provider crates that
/// always work without external credentials (filesystem, radiobrowser,
/// cover, spotify if a refresh token is present, s3 if MA_S3_BUCKET is
/// set).
fn register_builtin_providers(
    registry: &ma_providers::provider::ProviderRegistry,
    _config: &MassConfig,
) {
    if let Ok(path) = std::env::var("MA_FS_PATH") {
        let fs_cfg = ma_provider_filesystem::FilesystemConfig {
            path: std::path::PathBuf::from(path),
            content_type: "music".into(),
        };
        let provider = ma_provider_filesystem::FilesystemProvider::new("filesystem_local", fs_cfg);
        let handle = provider.into_handle("filesystem_local".into());
        if let Err(e) = registry.register(handle) {
            tracing::warn!(error = ?e, "filesystem_local register failed");
        } else {
            tracing::info!(domain = "filesystem_local", "registered");
        }
    }

    match ma_provider_radio::RadioBrowserProvider::new() {
        Ok(provider) => {
            let handle = provider.into_handle("radiobrowser".into());
            if let Err(e) = registry.register(handle) {
                tracing::warn!(error = ?e, "radiobrowser register failed");
            } else {
                tracing::info!(domain = "radiobrowser", "registered");
            }
        }
        Err(e) => tracing::warn!(error = ?e, "radiobrowser init failed"),
    }

    let cover_cfg = ma_provider_cover::CoverConfig {
        prefer_musicbrainz: true,
        ..Default::default()
    };
    match ma_provider_cover::CoverProvider::in_memory(cover_cfg) {
        Ok(provider) => {
            let handle = provider.into_handle();
            if let Err(e) = registry.register(handle) {
                tracing::warn!(error = ?e, "cover_art register failed");
            } else {
                tracing::info!(domain = "cover_art", "registered");
            }
        }
        Err(e) => tracing::warn!(error = ?e, "cover_art init failed"),
    }

    if let Ok(refresh) = std::env::var("MA_SPOTIFY_REFRESH_TOKEN") {
        let cfg = ma_provider_spotify::SpotifyConfig {
            refresh_token: Some(refresh),
            ..Default::default()
        };
        match ma_provider_spotify::SpotifyProvider::new("spotify".into(), cfg) {
            Ok(provider) => {
                let handle = provider.into_handle("spotify".into());
                if let Err(e) = registry.register(handle) {
                    tracing::warn!(error = ?e, "spotify register failed");
                } else {
                    tracing::info!(domain = "spotify", "registered");
                }
            }
            Err(e) => tracing::warn!(error = ?e, "spotify init failed"),
        }
    }

    // S3 music source — opt-in via envvars.
    if let Some(s3_cfg) = ma_provider_s3::S3Config::from_env() {
        let instance = std::env::var("MA_S3_INSTANCE").unwrap_or_else(|_| "s3".into());
        match ma_provider_s3::S3Provider::new(instance.clone(), s3_cfg) {
            Ok(provider) => {
                let handle = provider.into_handle(instance);
                if let Err(e) = registry.register(handle) {
                    tracing::warn!(error = ?e, "s3 register failed");
                } else {
                    tracing::info!(domain = "s3", "registered");
                }
            }
            Err(e) => tracing::warn!(error = %e, "s3 init failed"),
        }
    }
}
