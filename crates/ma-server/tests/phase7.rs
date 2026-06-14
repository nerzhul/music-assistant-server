//! Integration tests for the new Phase 5/7 endpoints:
//! * `/imageproxy` (cover art proxy backed by `ma_storage::cover_art`)
//! * `/preview` (returns the stream URL for a track)
//! * persistence — user data survives across `AuthManager` hydration

use std::net::SocketAddr;
use std::sync::Arc;

use ma_config::MassConfig;
use ma_server::auth::AuthManager;
use ma_server::{build_router, AppState, PlayerController};

use ma_storage::pool::{Database, DatabaseConfig};

async fn spawn_app_with_db() -> (SocketAddr, Arc<AppState>, Database) {
    let ctrl = Arc::new(PlayerController::new());
    let auth = AuthManager::new();
    let db = Arc::new(
        Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap(),
    );
    let registry = ma_providers::provider::ProviderRegistry::new();
    let placeholder = Arc::new(AppState::new(
        MassConfig::default(),
        ctrl.clone(),
        auth.clone(),
        Arc::new(ma_core::api::CommandRegistry::new()),
        registry.clone(),
    ));
    let commands = ma_server::commands::build_registry(placeholder);
    let state = AppState::new(MassConfig::default(), ctrl, auth, commands, registry)
        .with_database(Arc::clone(&db));
    let state = Arc::new(state);
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state, (*db).clone())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn imageproxy_returns_404_when_cover_missing() {
    let (addr, _state, _db) = spawn_app_with_db().await;
    // Bootstrap so the user gate passes.
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
        .get(format!("http://{}/imageproxy?id=does-not-exist", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn imageproxy_returns_404_when_no_database() {
    // The "no DB" path is the production fallback when MA_DATABASE_URL
    // is missing. It must still respond with 404 (not crash).
    let (addr, _state) = spawn_app_without_db().await;
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
        .get(format!("http://{}/imageproxy?id=x", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn imageproxy_streams_blob_from_db() {
    let (addr, _state, db) = spawn_app_with_db().await;
    // Bootstrap so the user gate passes.
    let token = {
        let resp = http()
            .post(format!("http://{}/setup", addr))
            .json(&serde_json::json!({
                "username": "admin",
                "password": "hunter2hunter2",
            }))
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = resp.json().await.unwrap();
        body["token"].as_str().unwrap().to_string()
    };
    // Insert a fake cover into the DB.
    let repo = ma_storage::CoverArtRepository::new(db.pool().clone());
    let record = ma_storage::CoverArtRecord {
        image_id: "fake-sha1".into(),
        provider: "itunes".into(),
        item_id: "track-1".into(),
        url: Some("https://example.com/x.jpg".into()),
        content_type: "image/jpeg".into(),
        width: Some(640),
        height: Some(640),
        bytes: b"\xFF\xD8\xFF\xE0fake-jpeg-bytes".to_vec(),
        fetched_at: chrono::Utc::now(),
        last_used_at: None,
    };
    repo.upsert(&record).await.unwrap();
    // Fetch via the proxy with `?id=`.
    let resp = http()
        .get(format!("http://{}/imageproxy?id=fake-sha1", addr))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "image/jpeg");
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(&bytes[..], b"\xFF\xD8\xFF\xE0fake-jpeg-bytes");
    // Now try the (provider, item_id) form.
    let resp = http()
        .get(format!(
            "http://{}/imageproxy?provider=itunes&item_id=track-1",
            addr
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], b"\xFF\xD8\xFF\xE0fake-jpeg-bytes");
}

#[tokio::test]
async fn imageproxy_requires_provider_or_id() {
    let (addr, _state, _db) = spawn_app_with_db().await;
    let _ = bootstrap(&addr).await;
    let resp = http()
        .get(format!("http://{}/imageproxy", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn preview_returns_not_found_for_unknown_track() {
    let (addr, _state, _db) = spawn_app_with_db().await;
    let _ = bootstrap(&addr).await;
    let resp = http()
        .get(format!("http://{}/preview?item_id=track-no-such", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn preview_requires_item_id() {
    let (addr, _state, _db) = spawn_app_with_db().await;
    let _ = bootstrap(&addr).await;
    let resp = http()
        .get(format!("http://{}/preview", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn auth_persists_to_database() {
    // Boot once, bootstrap, drop the server, boot a fresh auth
    // manager with the same DB, hydrate — the user should still
    // be there.
    let db = Database::connect(DatabaseConfig::in_memory_sqlite())
        .await
        .unwrap();
    let auth = AuthManager::new();
    auth.bootstrap_admin("alice", Some("hunter2hunter2".into()))
        .unwrap();
    // Persist the user to the DB.
    let user = auth.find_by_username("alice").unwrap();
    let hash = auth.password_hash_for(&user.user_id).unwrap();
    ma_storage::AuthRepository::new(db.pool().clone())
        .upsert_user(&user, &hash)
        .await
        .unwrap();
    // Build a fresh AuthManager and hydrate from the DB.
    let auth2 = AuthManager::new();
    assert!(!auth2.has_users());
    auth2.hydrate_from_db(&db).await.unwrap();
    assert!(auth2.has_users());
    let looked = auth2.verify_password("alice", "hunter2hunter2").unwrap();
    assert_eq!(looked.user_id, user.user_id);
}

async fn bootstrap(addr: &SocketAddr) -> String {
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
    body["token"].as_str().unwrap().to_string()
}

async fn spawn_app_without_db() -> (SocketAddr, Arc<AppState>) {
    let ctrl = Arc::new(PlayerController::new());
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
        auth,
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
