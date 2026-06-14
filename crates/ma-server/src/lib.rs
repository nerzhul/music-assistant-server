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

#![forbid(unsafe_code)]
#![warn(rust_2018_idioms)]

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
    // SECURITY: `CorsLayer::very_permissive()` is the safest default
    // for a LAN-only deployment (the MA UI is typically served from
    // the same origin as the API). For internet-facing deployments
    // the operator MUST front the server with a reverse proxy that
    // enforces an allowlist of origins; the server is not designed
    // to be exposed directly.
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
    register_builtin_providers(&registry, &config).await;
    info!(providers = registry.list().len(), "providers registered");
    let registry_arc = registry;

    // Open the database (SQLite by default, PostgreSQL when
    // MA_DATABASE_URL points at a postgres:// URL). The connection
    // pool + migrations are managed by `ma_storage`. When the
    // database cannot be opened (e.g. in tests, or when the operator
    // disabled it), the server continues with the in-memory auth
    // backend only.
    let database: Option<Arc<ma_storage::Database>> =
        match ma_storage::pool::DatabaseConfig::from_env() {
            Ok(cfg) => match ma_storage::Database::connect(cfg).await {
                Ok(db) => {
                    info!(kind = db.kind().as_str(), "database ready");
                    Some(Arc::new(db))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "database connect failed; running without persistence");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "database config invalid; running without persistence");
                None
            }
        };

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
    let mut state = AppState::new(
        config.clone(),
        player_controller.clone(),
        auth.clone(),
        commands,
        registry_arc.clone(),
    );
    state = state.with_database_opt(database.clone());
    let state = Arc::new(state);

    // If we have a database, hydrate the in-memory auth state from
    // it. Otherwise (or on first run with no users), bootstrap an
    // admin from envvars / random password.
    if let Some(db) = database.as_ref() {
        state
            .auth
            .hydrate_from_db(db)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    }
    let (admin_user, printed) = state
        .auth
        .bootstrap_admin("admin", std::env::var("MA_AUTH_INITIAL_PASSWORD").ok())
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if printed.is_some() {
        // SECURITY: we never log the password itself. The operator
        // either set MA_AUTH_INITIAL_PASSWORD (and therefore already
        // knows it) or has to read it back from the bootstrap response
        // (when this code path is invoked via /setup, not here).
        tracing::warn!(
            username = %admin_user.username,
            "first admin user created; please change the password if you used the auto-generated bootstrap"
        );
    }
    if let Some(db) = database.as_ref() {
        // Persist the bootstrap admin to the DB so a restart preserves
        // the credentials.
        let user = state
            .auth
            .find_by_username(&admin_user.username)
            .ok_or_else(|| anyhow::anyhow!("admin user disappeared after bootstrap"))?;
        let hash = state
            .auth
            .password_hash_for(&user.user_id)
            .ok_or_else(|| anyhow::anyhow!("admin password hash missing"))?;
        ma_storage::AuthRepository::new(db.pool().clone())
            .upsert_user(&user, &hash)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
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
async fn register_builtin_providers(
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

    // iTunes Podcasts: single-instance, env-toggled by country code.
    if let Ok(country) = std::env::var("MA_PODCASTS_ITUNES_COUNTRY") {
        let cfg = ma_provider_podcasts::ITunesPodcastsConfig {
            country,
            explicit: std::env::var("MA_PODCASTS_ITUNES_EXPLICIT")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true),
            num_episodes: std::env::var("MA_PODCASTS_ITUNES_TOP_LIMIT")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(10),
        };
        match ma_provider_podcasts::ITunesPodcastsProvider::new(cfg) {
            Ok(provider) => {
                let handle = provider.into_handle();
                if let Err(e) = registry.register(handle) {
                    tracing::warn!(error = ?e, "itunes_podcasts register failed");
                } else {
                    tracing::info!(domain = "itunes_podcasts", "registered");
                }
            }
            Err(e) => tracing::warn!(error = %e, "itunes_podcasts init failed"),
        }
    }

    // RSS feeds: one per `MA_PODCASTS_FEEDS_<N>_URL` env entry. We
    // support up to 8 instances to keep the env simple.
    for n in 0..8 {
        let key = format!("MA_PODCASTS_FEEDS_{n}_URL");
        if let Ok(feed_url) = std::env::var(&key) {
            let name = std::env::var(format!("MA_PODCASTS_FEEDS_{n}_NAME"))
                .unwrap_or_else(|_| format!("podcast_{n}"));
            let cfg = ma_provider_podcasts::FeedConfig { feed_url };
            match ma_provider_podcasts::FeedProvider::new(cfg) {
                Ok(provider) => {
                    let handle = provider.clone().into_handle(name.clone());
                    if let Err(e) = registry.register(handle) {
                        tracing::warn!(error = ?e, "podcastfeed {} register failed", name);
                    } else {
                        tracing::info!(domain = "podcastfeed", instance = name, "registered");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "podcastfeed {n} init failed"),
            }
        }
    }

    // YouTube Music: one instance, env-toggled.
    if std::env::var("MA_YTMUSIC_ENABLED").is_ok() {
        let instance = std::env::var("MA_YTMUSIC_INSTANCE").unwrap_or_else(|_| "ytmusic".into());
        let cfg = ma_provider_ytmusic::YTMusicConfig::default();
        let provider = ma_provider_ytmusic::YTMusicProvider::new(instance.clone(), cfg.clone());
        match provider.await {
            Ok(p) => {
                let handle = p.into_handle(instance.clone());
                if let Err(e) = registry.register(handle) {
                    tracing::warn!(error = ?e, "ytmusic register failed");
                } else {
                    tracing::info!(domain = "ytmusic", instance, "registered");
                }
            }
            Err(e) => tracing::warn!(error = %e, "ytmusic init failed"),
        }
    }

    // Home Assistant: discover media_player.* entities and expose
    // them as MA players. The actual player registration happens
    // later in `state.ha_players` once the discovery loop returns a
    // snapshot; we just kick the loop here.
    let ha_cfg = ma_ha::HaConfig::default();
    if (ha_cfg.url.is_some() && ha_cfg.token.is_some()) || ha_cfg.supervisor_url.is_some() {
        match ma_ha::HaClient::from_config(&ha_cfg) {
            Ok(client) => {
                let client = Arc::new(client);
                match ma_ha::Discover::new(&ha_cfg, client.clone()) {
                    Ok(discover) => {
                        let discover = Arc::new(discover);
                        let poll_secs = std::env::var("MA_HA_POLL_SECS")
                            .ok()
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(30);
                        let _handle = discover
                            .clone()
                            .spawn(std::time::Duration::from_secs(poll_secs));
                        tracing::info!(
                            url = ha_cfg.url.as_deref().unwrap_or("(supervisor)"),
                            poll_secs,
                            "home assistant discovery started"
                        );
                    }
                    Err(e) => tracing::warn!(error = %e, "ha discover init failed"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "ha client init failed"),
        }
    }
}
