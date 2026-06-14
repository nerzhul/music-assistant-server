//! End-to-end tests: boot the Sendspin server on a fixed port, connect
//! via tungstenite, verify the handshake and clock-sync exchange.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, WebSocketStream};

use ma_player_sendspin::server::{SendspinServer, SendspinServerConfig};

/// Bind a real `SendspinServer` to an ephemeral port, returning the bound
/// address and a handle that aborts the server task on drop.
async fn spawn_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    // Bind a TCP listener ourselves to get an OS-chosen port, then close
    // it and hand the port number to the server. There's a tiny race here
    // but it's acceptable for a test.
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let config = SendspinServerConfig {
        bind_ip: addr.ip().to_string(),
        inbound_port: addr.port(),
        ..Default::default()
    };
    let server = SendspinServer::new(config);
    let handle = tokio::spawn(async move {
        let _ = server.run().await;
    });
    // Give the server a moment to actually bind.
    tokio::time::sleep(Duration::from_millis(150)).await;
    (addr, handle)
}

async fn connect(addr: std::net::SocketAddr) -> WebSocketStream<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let url = format!("ws://{addr}/sendspin");
    let (ws, _resp) = client_async(&url, tcp).await.expect("ws upgrade");
    ws
}

#[tokio::test]
async fn hello_handshake_yields_welcome_messages() {
    let (addr, handle) = spawn_server().await;
    let mut ws = connect(addr).await;

    // Send a client/hello.
    let hello = serde_json::json!({
        "type": "client/hello",
        "payload": {
            "client_id": "test-client",
            "name": "Test",
            "version": 1,
            "supported_roles": ["player@v1", "controller@v1"],
            "player@v1_support": {
                "supported_formats": [{
                    "codec": "pcm",
                    "channels": 2,
                    "sample_rate": 48000,
                    "bit_depth": 16
                }],
                "buffer_capacity": 1048576,
                "supported_commands": ["volume", "mute"]
            }
        }
    })
    .to_string();
    ws.send(Message::Text(hello)).await.unwrap();

    let mut got_hello = false;
    let mut got_stream_start = false;
    let mut got_group_update = false;
    let mut got_state = false;
    let deadline = Duration::from_secs(3);
    while !(got_hello && got_stream_start && got_group_update && got_state) {
        let frame = match timeout(deadline, ws.next()).await {
            Ok(Some(Ok(f))) => f,
            _ => break,
        };
        if let Message::Text(t) = frame {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or(serde_json::Value::Null);
            match v.get("type").and_then(|x| x.as_str()) {
                Some("server/hello") => got_hello = true,
                Some("stream/start") => got_stream_start = true,
                Some("group/update") => got_group_update = true,
                Some("server/state") => got_state = true,
                _ => {}
            }
        }
    }
    assert!(got_hello, "missing server/hello");
    assert!(got_stream_start, "missing stream/start");
    assert!(got_group_update, "missing group/update");
    assert!(got_state, "missing server/state");

    let _ = ws.send(Message::Close(None)).await;
    handle.abort();
}

#[tokio::test]
async fn client_time_returns_server_time() {
    let (addr, handle) = spawn_server().await;
    let mut ws = connect(addr).await;

    // handshake
    let hello = serde_json::json!({
        "type": "client/hello",
        "payload": {
            "client_id": "test-time",
            "name": "T",
            "version": 1,
            "supported_roles": ["controller@v1"]
        }
    })
    .to_string();
    ws.send(Message::Text(hello)).await.unwrap();
    for _ in 0..4 {
        let _ = timeout(Duration::from_millis(500), ws.next()).await;
    }

    let now: i64 = 1_000_000;
    let ct = serde_json::json!({
        "type": "client/time",
        "client_transmitted": now
    })
    .to_string();
    ws.send(Message::Text(ct)).await.unwrap();

    let mut got_server_time = false;
    let deadline = Duration::from_secs(2);
    while !got_server_time {
        let frame = match timeout(deadline, ws.next()).await {
            Ok(Some(Ok(f))) => f,
            _ => break,
        };
        if let Message::Text(t) = frame {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or(serde_json::Value::Null);
            if v.get("type").and_then(|x| x.as_str()) == Some("server/time") {
                got_server_time = true;
                assert!(v.get("client_transmitted").is_some());
                assert!(v.get("server_received").is_some());
                assert!(v.get("server_transmitted").is_some());
                assert_eq!(v["client_transmitted"], now);
            }
        }
    }
    assert!(got_server_time, "missing server/time response");

    handle.abort();
}

#[tokio::test]
async fn controller_command_broadcasts_group_update() {
    let (addr, handle) = spawn_server().await;
    let mut ws = connect(addr).await;

    let hello = serde_json::json!({
        "type": "client/hello",
        "payload": {
            "client_id": "test-ctrl",
            "name": "C",
            "version": 1,
            "supported_roles": ["controller@v1", "player@v1"]
        }
    })
    .to_string();
    ws.send(Message::Text(hello)).await.unwrap();

    // Read until we have the full handshake.
    let mut got_state = false;
    let deadline = Duration::from_secs(3);
    while !got_state {
        let frame = match timeout(deadline, ws.next()).await {
            Ok(Some(Ok(f))) => f,
            _ => break,
        };
        if let Message::Text(t) = frame {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or(serde_json::Value::Null);
            if v.get("type").and_then(|x| x.as_str()) == Some("server/state") {
                got_state = true;
            }
        }
    }
    assert!(got_state, "handshake never completed");

    let cmd = serde_json::json!({
        "type": "client/command",
        "payload": {
            "controller": { "command": "mute", "mute": true }
        }
    })
    .to_string();
    ws.send(Message::Text(cmd)).await.unwrap();

    let mut got_group_update = false;
    let deadline = Duration::from_secs(3);
    while !got_group_update {
        let frame = match timeout(deadline, ws.next()).await {
            Ok(Some(Ok(f))) => f,
            _ => break,
        };
        if let Message::Text(t) = frame {
            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or(serde_json::Value::Null);
            eprintln!("post-mute frame: {}", v);
            if v.get("type").and_then(|x| x.as_str()) == Some("group/update") {
                got_group_update = true;
            }
        }
    }
    assert!(got_group_update, "missing group/update after mute command");

    handle.abort();
}
