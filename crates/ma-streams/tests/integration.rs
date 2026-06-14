//! End-to-end test: spin up the ma-streams HTTP server and verify
//! it serves a `/single/...` route from a `BroadcastStream`.
//!
//! The test boots axum on a random port, registers a fake session
//! that yields three chunks, and uses reqwest to verify the body
//! is what we expected.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ma_providers::stream::StreamDetails;
use ma_streams::server::{HttpServer, HttpServerConfig, HttpServerState, StreamSession};
use ma_streams::worker::StreamWorkerConfig;
use ma_streams::BroadcastStream;

fn port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

#[tokio::test]
async fn http_server_serves_single_route() {
    let p = port();
    let config = HttpServerConfig {
        bind_ip: "127.0.0.1".into(),
        port: p,
        base_url: format!("http://127.0.0.1:{p}"),
        fallback_data_dir: None,
    };
    let state = HttpServerState::new(config);

    // Build a fake session whose broadcast yields a few chunks.
    let bcast = Arc::new(BroadcastStream::new(4));
    let session = StreamSession {
        id: "test-session".into(),
        broadcast: Arc::clone(&bcast),
        streamdetails: Arc::new(StreamDetails::default()),
        config: StreamWorkerConfig::default(),
    };
    state.register_session(session);

    // Spawn a task that pushes chunks into the broadcast, then
    // finishes it. The HTTP client will pull them.
    let bcast_clone = Arc::clone(&bcast);
    let server_url = format!("http://127.0.0.1:{p}");
    // Use a Notify so the producer runs only after the test's
    // request is in flight. The producer broadcasts 3 chunks and
    // then finishes; the body stream drains them.
    let ready = Arc::new(tokio::sync::Notify::new());
    let ready_producer = Arc::clone(&ready);
    let producer = tokio::spawn(async move {
        ready_producer.notified().await;
        bcast_clone.broadcast(Bytes::from_static(b"chunk-1-"));
        bcast_clone.broadcast(Bytes::from_static(b"chunk-2-"));
        bcast_clone.broadcast(Bytes::from_static(b"chunk-3-"));
        bcast_clone.finish();
    });

    let url = format!("{server_url}/single/test-session/q1/i1/player1/flac");
    // Spawn the HTTP server in the background.
    let server = HttpServer::new(state.clone());
    let _handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    // Wait for the server to bind.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Spawn the GET in a task so we can signal the producer only
    // once the request is being sent.
    let ready_clone = Arc::clone(&ready);
    let get_task = tokio::spawn(async move {
        let resp = client.get(&url).send().await.expect("GET failed");
        assert!(resp.status().is_success());
        let body = resp.bytes().await.unwrap();
        // Signal the producer after the body has been received.
        ready_clone.notify_one();
        body
    });
    // Give the GET task a moment to send the request.
    tokio::time::sleep(Duration::from_millis(200)).await;
    ready.notify_one();
    let body = get_task.await.unwrap();
    assert!(body.starts_with(b"chunk-1-"), "got body {:?}", body);
    assert!(body.ends_with(b"chunk-3-"), "got body {:?}", body);
    let _ = producer.await;
}

#[tokio::test]
async fn http_server_returns_404_for_unknown_session() {
    let p = port();
    let config = HttpServerConfig {
        bind_ip: "127.0.0.1".into(),
        port: p,
        base_url: format!("http://127.0.0.1:{p}"),
        fallback_data_dir: None,
    };
    let state = HttpServerState::new(config);
    let server = HttpServer::new(state);
    let _handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let url = format!("http://127.0.0.1:{p}/single/no-such-session/q/i/p/flac");
    let resp = reqwest::get(&url).await.expect("GET failed");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn http_server_health_endpoint() {
    let p = port();
    let config = HttpServerConfig {
        bind_ip: "127.0.0.1".into(),
        port: p,
        base_url: format!("http://127.0.0.1:{p}"),
        fallback_data_dir: None,
    };
    let state = HttpServerState::new(config);
    let server = HttpServer::new(state);
    let _handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let url = format!("http://127.0.0.1:{p}/health");
    let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
    assert_eq!(body, "ok");
}
