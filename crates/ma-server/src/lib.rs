//! `ma-server` — Music Assistant Rust server binary.
//!
//! Phase 0 exposes the minimal HTTP surface the UI needs to confirm it can
//! talk to the Rust binary:
//!
//! * `GET  /info`        — `ServerInfoMessage` JSON (the same payload the
//!   Python server sends on WebSocket connect, used by the frontend for
//!   capability detection).
//! * `GET  /logo.png`    — the bundled Music Assistant logo (static).
//! * `GET  /`            — 200 OK placeholder for health checks.
//! * `GET  /health`      — same as `/`.
//!
//! Full auth, API command dispatch, and WebSocket UI are added in later
//! phases (see plan section "Compatibilité API HTTP/WS avec l'UI").

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

pub use state::AppState;

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
    "Music Assistant (Rust) - phase 0"
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(get_root))
        .route("/health", get(get_health))
        .route("/info", get(get_info))
        .route("/logo.png", get(get_logo))
        .layer(CorsLayer::very_permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn run(config: MassConfig) -> anyhow::Result<()> {
    let state = Arc::new(AppState::new(config.clone()));
    let bind = SocketAddr::new(
        state.config.server.bind_ip.parse()?,
        state.config.server.bind_port,
    );
    let app = build_router(state);

    info!(%bind, "starting ma-server (phase 3: http + sendspin + providers)");

    // Build the provider registry. Phase 3 only instantiates the
    // built-in / configured providers when their config blocks are
    // populated — advanced deployments will switch this for a
    // real config loader.
    let registry = ma_providers::provider::ProviderRegistry::new();
    register_builtin_providers(&registry, &config);
    let registry_arc = Arc::new(registry);
    info!(
        providers = registry_arc.list().len(),
        "providers registered"
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

/// Phase 3 builtin / example provider set. Wires the four provider
/// crates that always work without external credentials:
///
/// * `filesystem_local` (when `MA_FS_PATH` is set)
/// * `radiobrowser` (public API, no key)
/// * `cover_art` (works with just the iTunes + Musicbrainz clients)
/// * `spotify` (only if `MA_SPOTIFY_REFRESH_TOKEN` is set; otherwise
///   the provider isn't registered)
///
/// This is the Rust equivalent of the Python server's
/// `default_providers` list. It runs at startup; production
/// deployments will replace it with a config-driven loader.
fn register_builtin_providers(
    registry: &ma_providers::provider::ProviderRegistry,
    _config: &MassConfig,
) {
    // Filesystem local — when MA_FS_PATH is set, register a single
    // instance. The MA webserver config can carry multiple paths in
    // the future.
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

    // RadioBrowser — always available.
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

    // Cover art — always available, in-memory cache so we don't
    // touch the filesystem unless the user has set MA_CACHE_DIR.
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

    // Spotify — only if a refresh token is provided. Auth without
    // one would require an interactive PKCE flow, which Phase 3
    // doesn't surface in the UI yet.
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
}
