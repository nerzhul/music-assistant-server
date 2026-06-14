//! Sendspin server: WebSocket listener, connection loop, and lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use ma_protocol_sendspin::messages::{
    ClientCommand, ClientGoodbye, ClientHello, ClientState, ClientTime, SyncState,
};
use ma_protocol_sendspin::roles::AudioFormat;
use ma_protocol_sendspin::SupportedAudioFormat;
use ma_time_filter::{TimeFilter, TimeUpdate};

use crate::codec::negotiate;
use crate::playback::PushStream;
use crate::protocol::{server_hello_envelope, server_time_json, stream_start_player};
use crate::roles::{
    apply_client_state, build_stream_clear, build_stream_end, handle_client_command,
};
use crate::state::{ClientRecord, OutboundMessage, SendspinState};

const SENDSPIN_INBOUND_PATH: &str = "/sendspin";

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("websocket error: {0}")]
    WebSocket(String),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SendspinServerConfig {
    pub bind_ip: String,
    pub inbound_port: u16,
    pub inbound_path: String,
    pub server_id: String,
    pub server_name: String,
}

impl Default for SendspinServerConfig {
    fn default() -> Self {
        Self {
            bind_ip: "0.0.0.0".into(),
            inbound_port: 8927,
            inbound_path: SENDSPIN_INBOUND_PATH.to_string(),
            server_id: Uuid::new_v4().to_string(),
            server_name: "Music Assistant (Rust)".into(),
        }
    }
}

/// The Sendspin server. Spawns a TcpListener on the configured port and
/// dispatches each accepted connection to a spawned task.
pub struct SendspinServer {
    pub config: SendspinServerConfig,
    pub state: Arc<SendspinState>,
    pub push_stream: PushStream,
}

impl SendspinServer {
    /// Create a new server.
    pub fn new(config: SendspinServerConfig) -> Arc<Self> {
        let state = SendspinState::new(config.server_id.clone(), config.server_name.clone());
        Arc::new(Self {
            config,
            state,
            push_stream: PushStream::new(),
        })
    }

    /// Run the listener until the future is cancelled or the listener
    /// fails.
    pub async fn run(self: Arc<Self>) -> Result<(), ServerError> {
        let bind_addr = format!("{}:{}", self.config.bind_ip, self.config.inbound_port);
        let addr: SocketAddr = bind_addr
            .parse()
            .map_err(|e: std::net::AddrParseError| ServerError::InvalidUrl(e.to_string()))?;
        let listener: TcpListener = TcpListener::bind(addr).await?;
        info!(addr = %addr, "sendspin server listening");

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    debug!(%peer, "sendspin inbound connection");
                    let me = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(e) = me.handle_connection(stream, peer).await {
                            warn!(%peer, error = %e, "sendspin connection error");
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "sendspin accept failed");
                }
            }
        }
    }

    async fn handle_connection(
        self: Arc<Self>,
        stream: TcpStream,
        peer: SocketAddr,
    ) -> Result<(), ServerError> {
        let ws = match tokio_tungstenite::accept_async(stream).await {
            Ok(ws) => ws,
            Err(e) => return Err(ServerError::WebSocket(e.to_string())),
        };
        let (mut ws_tx, mut ws_rx) = ws.split();

        // Wait for the very first frame: it MUST be `client/hello`.
        let first = match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Binary(_))) => {
                return Err(ServerError::WebSocket(
                    "expected text client/hello, got binary".into(),
                ))
            }
            Some(Ok(_)) => return Err(ServerError::WebSocket("expected text client/hello".into())),
            Some(Err(e)) => return Err(ServerError::WebSocket(e.to_string())),
            None => {
                debug!(%peer, "sendspin connection closed before hello");
                return Ok(());
            }
        };

        let hello: ClientHello = match parse_envelope(&first, "client/hello") {
            Ok(h) => h,
            Err(e) => {
                warn!(%peer, error = %e, "invalid client/hello");
                return Err(ServerError::WebSocket(format!("invalid hello: {e}")));
            }
        };
        debug!(client = %hello.client_id, name = %hello.name, "client/hello received");

        // Negotiate roles + audio format.
        let server_hello = self.state.hello_for(&hello.supported_roles);
        let active_roles = server_hello.active_roles.clone();

        // Pick the audio format (only meaningful for player roles).
        let audio_format: Option<AudioFormat> =
            if active_roles.iter().any(|r| r.starts_with("player@")) {
                hello.player_v1_support.as_ref().map(|p| {
                    let list: Vec<(String, u32, u8, u16)> = p
                        .supported_formats
                        .iter()
                        .map(|f: &SupportedAudioFormat| {
                            (f.codec.clone(), f.sample_rate, f.channels, f.bit_depth)
                        })
                        .collect();
                    negotiate(&list)
                })
            } else {
                None
            };

        // Compute timing parameters for the client to use.
        let required_lead_time_ms = 250;
        let min_buffer_ms = 250;

        // Outbound channel: connection writer task pulls from this and
        // sends to the WebSocket.
        let (out_tx, mut out_rx) = mpsc::channel::<OutboundMessage>(512);
        // The push stream will deliver binary frames to all subscribers.
        self.push_stream
            .add_subscriber(crate::playback::Subscriber {
                client_id: hello.client_id.clone(),
                tx: out_tx.clone(),
            });

        // Register the client in shared state.
        let record = ClientRecord {
            client_id: hello.client_id.clone(),
            name: hello.name.clone(),
            supported_roles: hello.supported_roles.clone(),
            active_roles: active_roles.clone(),
            remote_addr: Some(peer),
            connection_reason: None,
            state: SyncState::Synchronized,
            volume: 50,
            muted: false,
            static_delay_ms: 0,
            required_lead_time_ms,
            min_buffer_ms,
            supported_player_commands: hello
                .player_v1_support
                .as_ref()
                .map(|p| p.supported_commands.clone())
                .unwrap_or_default(),
            group_id: "g1".into(),
            connected_at: Instant::now(),
            outbound_tx: out_tx.clone(),
        };
        let record = self.state.upsert_client(record);

        // Send `server/hello` first.
        let hello_text = server_hello_envelope(&server_hello)?;
        if let Err(e) = ws_tx.send(Message::Text(hello_text)).await {
            warn!(error = %e, "failed to send server/hello");
            return Err(ServerError::WebSocket(e.to_string()));
        }
        // If the player role is active, send `stream/start` for the
        // negotiated audio format.
        if let Some(fmt) = audio_format {
            let start_text = stream_start_player(fmt, None)?;
            if let Err(e) = ws_tx.send(Message::Text(start_text)).await {
                warn!(error = %e, "failed to send stream/start");
                return Err(ServerError::WebSocket(e.to_string()));
            }
        }
        // Always send a `group/update` with the current group state.
        let group = ma_protocol_sendspin::messages::GroupUpdate {
            playback_state: Some("stopped".to_string()),
            group_id: Some("g1".to_string()),
            group_name: Some("Music Assistant".to_string()),
        };
        let group_text = crate::protocol::group_update_envelope(&group)?;
        let _ = ws_tx.send(Message::Text(group_text)).await;
        // Initial `server/state` with the controller block.
        let controller_state = ma_protocol_sendspin::messages::ServerState {
            metadata: None,
            controller: Some(ma_protocol_sendspin::messages::ControllerState {
                supported_commands: vec![
                    "play".into(),
                    "pause".into(),
                    "stop".into(),
                    "next".into(),
                    "previous".into(),
                    "volume".into(),
                    "mute".into(),
                ],
                volume: 50,
                muted: false,
                repeat: "off".into(),
                shuffle: false,
            }),
            color: None,
        };
        let state_text = crate::protocol::server_state_envelope(&controller_state)?;
        let _ = ws_tx.send(Message::Text(state_text)).await;

        // Per-connection time filter.
        let time_filter = Arc::new(Mutex::new(TimeFilter::new()));

        // Forward every outbound message to the WebSocket sink. We use
        // the `ws_tx` writer in a loop, reading from `out_rx`. The reader
        // loop also calls `ws_tx.send` directly for Pong frames.
        let (writer_tx, mut writer_rx) = mpsc::channel::<Message>(64);
        let writer_handle = {
            tokio::spawn(async move {
                while let Some(msg) = writer_rx.recv().await {
                    if let Err(e) = ws_tx.send(msg).await {
                        warn!(error = %e, "sendspin writer send failed");
                        break;
                    }
                }
            })
        };

        // The "outbound" channel is for fanout from server-wide events
        // (push_stream, group updates). All frames go to the writer task.
        let writer_tx_for_outbound = writer_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                let wire_msg = match msg {
                    OutboundMessage::Text(s) => Message::Text(s),
                    OutboundMessage::Binary(b) => Message::Binary(b),
                };
                if writer_tx_for_outbound.send(wire_msg).await.is_err() {
                    break;
                }
            }
        });

        // Reader loop.
        while let Some(frame) = ws_rx.next().await {
            let frame = match frame {
                Ok(f) => f,
                Err(e) => {
                    debug!(error = %e, "sendspin read error");
                    break;
                }
            };
            match frame {
                Message::Text(text) => {
                    self.handle_text_frame(&hello.client_id, &text, &time_filter)
                        .await;
                }
                Message::Binary(b) => {
                    debug!(client = %hello.client_id, len = b.len(), "binary frame");
                    // Binary frames are audio / artwork / visualizer data
                    // sent from the client. The spec doesn't define any
                    // such direction in V1 — log and ignore.
                }
                Message::Ping(p) => {
                    if writer_tx.send(Message::Pong(p)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => {
                    debug!(client = %hello.client_id, "sendspin close");
                    break;
                }
                _ => {}
            }
        }

        // Clean up.
        self.push_stream.remove_subscriber(&hello.client_id);
        self.state.remove_client(&hello.client_id);
        let _ = out_tx
            .send(OutboundMessage::Text(
                build_stream_end().unwrap_or_default(),
            ))
            .await;
        let _ = record; // keep the Arc alive until now
        writer_handle.abort();
        Ok(())
    }

    async fn handle_text_frame(
        self: &Arc<Self>,
        client_id: &str,
        text: &str,
        time_filter: &Arc<Mutex<TimeFilter>>,
    ) {
        let v: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!(client = %client_id, error = %e, "sendspin invalid JSON");
                return;
            }
        };
        let msg_type = match v.get("type").and_then(Value::as_str) {
            Some(t) => t,
            None => {
                warn!(client = %client_id, "sendspin message missing type");
                return;
            }
        };
        match msg_type {
            "client/time" => {
                if let Ok(ct) = serde_json::from_value::<ClientTime>(v) {
                    let now_local_us = monotonic_us();
                    let server_received_us = monotonic_us();
                    let server_transmitted_us = monotonic_us();
                    let resp = server_time_json(
                        ct.client_transmitted,
                        server_received_us,
                        server_transmitted_us,
                    );
                    if let Ok(text) = resp {
                        let tx_opt = self
                            .state
                            .clients
                            .read()
                            .get(client_id)
                            .map(|c| c.outbound_tx.clone());
                        if let Some(tx) = tx_opt {
                            let _ = tx.send(OutboundMessage::Text(text)).await;
                        }
                    }
                    // Feed the client's time filter with the same 4-tuple.
                    let mut tf = time_filter.lock();
                    tf.update(
                        now_local_us,
                        TimeUpdate {
                            client_transmitted_us: ct.client_transmitted,
                            server_received_us,
                            server_transmitted_us,
                            client_received_us: monotonic_us(),
                        },
                    );
                }
            }
            "client/state" => {
                if let Some(payload) = v.get("payload") {
                    if let Ok(cs) = serde_json::from_value::<ClientState>(payload.clone()) {
                        apply_client_state(&self.state, client_id, &cs);
                    }
                }
            }
            "client/command" => {
                if let Some(payload) = v.get("payload") {
                    if let Ok(cmd) = serde_json::from_value::<ClientCommand>(payload.clone()) {
                        let out = handle_client_command(&self.state, client_id, &cmd);
                        let tx_opt = self
                            .state
                            .clients
                            .read()
                            .get(client_id)
                            .map(|c| c.outbound_tx.clone());
                        if let Some(tx) = tx_opt {
                            for text in out {
                                let _ = tx.send(OutboundMessage::Text(text)).await;
                            }
                        }
                    }
                }
            }
            "client/goodbye" => {
                if let Some(payload) = v.get("payload") {
                    if let Ok(g) = serde_json::from_value::<ClientGoodbye>(payload.clone()) {
                        info!(client = %client_id, reason = ?g.reason, "client/goodbye");
                    }
                }
                let tx_opt = self
                    .state
                    .clients
                    .read()
                    .get(client_id)
                    .map(|c| c.outbound_tx.clone());
                if let Some(tx) = tx_opt {
                    let _ = tx
                        .send(OutboundMessage::Text(
                            build_stream_clear().unwrap_or_default(),
                        ))
                        .await;
                }
            }
            "stream/request-format" => {
                let tx_opt = self
                    .state
                    .clients
                    .read()
                    .get(client_id)
                    .map(|c| c.outbound_tx.clone());
                if let Some(tx) = tx_opt {
                    let _ = tx
                        .send(OutboundMessage::Text(
                            build_stream_end().unwrap_or_default(),
                        ))
                        .await;
                    let _ = tx
                        .send(OutboundMessage::Text(
                            stream_start_player(
                                AudioFormat::Pcm {
                                    sample_rate: 48_000,
                                    channels: 2,
                                    bit_depth: 16,
                                    sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
                                },
                                None,
                            )
                            .unwrap_or_default(),
                        ))
                        .await;
                }
            }
            _ => {
                debug!(client = %client_id, msg_type, "sendspin unhandled message type");
            }
        }
    }
}

/// Helper: parse a Sendspin envelope message into its payload, validating
/// the `type` field.
pub fn parse_envelope<T: serde::de::DeserializeOwned>(
    raw: &str,
    expected: &str,
) -> Result<T, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let t = v
        .get("type")
        .and_then(Value::as_str)
        .ok_or("missing type")?;
    if t != expected {
        return Err(format!("expected {expected}, got {t}"));
    }
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    serde_json::from_value(payload).map_err(|e| e.to_string())
}

/// Monotonic microsecond clock used for Sendspin timestamps.
pub fn monotonic_us() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_micros() as i64
}

// Re-export role ids for the server's public API.
pub use ma_protocol_sendspin::roles as protocol_roles;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_envelope_unwraps_payload() {
        let raw = json!({
            "type": "client/hello",
            "payload": {
                "client_id": "x",
                "name": "S",
                "version": 1,
                "supported_roles": ["player@v1"]
            }
        })
        .to_string();
        let h: ClientHello = parse_envelope(&raw, "client/hello").unwrap();
        assert_eq!(h.client_id, "x");
        assert_eq!(h.supported_roles, vec!["player@v1"]);
    }

    #[test]
    fn parse_envelope_rejects_wrong_type() {
        let raw = json!({"type": "client/state", "payload": {}}).to_string();
        let r: Result<ClientHello, _> = parse_envelope(&raw, "client/hello");
        assert!(r.is_err());
    }

    #[test]
    fn default_config_uses_port_8927() {
        let c = SendspinServerConfig::default();
        assert_eq!(c.inbound_port, 8927);
        assert_eq!(c.inbound_path, "/sendspin");
    }
}
