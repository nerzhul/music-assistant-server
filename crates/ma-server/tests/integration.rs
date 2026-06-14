//! End-to-end test: spin up the ma-server with all builtin providers
//! registered and verify a few HTTP endpoints work.

use std::time::Duration;

use ma_config::MassConfig;
use ma_server::run;

fn port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn boot() -> (String, tokio::task::JoinHandle<()>) {
    let port = port();
    let cfg = MassConfig {
        server: ma_config::ServerConfig {
            bind_ip: "127.0.0.1".into(),
            bind_port: port,
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        },
        sendspin: ma_config::SendspinConfig {
            enabled: false, // skip sendspin in this test
            ..Default::default()
        },
        ..Default::default()
    };
    // Set MA_FS_PATH so the filesystem provider registers.
    let dir = std::env::temp_dir().join(format!(
        "ma-fs-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("MA_FS_PATH", &dir);
    let handle = tokio::spawn(async move {
        let _ = run(cfg).await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    (format!("http://127.0.0.1:{port}"), handle)
}

#[tokio::test]
async fn info_endpoint_returns_server_info() {
    let (base, handle) = boot().await;
    let resp = reqwest::get(format!("{base}/info")).await.unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["server_id"], "ma-rs-001");
    assert_eq!(body["schema_version"], 31);
    handle.abort();
}

#[tokio::test]
async fn health_endpoint_says_ok() {
    let (base, handle) = boot().await;
    let body = reqwest::get(format!("{base}/health"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
    handle.abort();
}

#[tokio::test]
async fn logo_endpoint_returns_png_bytes() {
    let (base, handle) = boot().await;
    let resp = reqwest::get(format!("{base}/logo.png")).await.unwrap();
    assert!(resp.status().is_success());
    let bytes = resp.bytes().await.unwrap();
    // PNG magic number
    assert!(bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]));
    handle.abort();
}
