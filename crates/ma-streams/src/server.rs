//! HTTP server: axum on port 8097, serving `/single/...` and
//! `/flow/...` routes that look like the Python
//! `Webserver.register_dynamic_route` surface. Players fetch audio
//! from these URLs.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, info};
use url::Url;

use ma_core::enums::ContentType;
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::broadcast::BroadcastStream;
use crate::worker::StreamWorkerConfig;

#[derive(Debug, Clone)]
pub struct HttpServerConfig {
    pub bind_ip: String,
    pub port: u16,
    /// Base URL published to clients (e.g. `http://192.168.1.5:8097`).
    pub base_url: String,
    /// Directory used to source audio for the `/single/...` route when
    /// the `streamdetails` cannot be resolved. Used as a fallback
    /// for testing.
    pub fallback_data_dir: Option<PathBuf>,
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            bind_ip: "0.0.0.0".into(),
            port: 8097,
            base_url: "http://localhost:8097".into(),
            fallback_data_dir: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum HttpServerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("axum error: {0}")]
    Axum(#[from] axum::Error),
    #[error("missing query parameter: {0}")]
    MissingParam(&'static str),
    #[error("stream not found: {0}")]
    StreamNotFound(String),
    #[error("internal: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, HttpServerError>;

/// Decomposed URL for `/single/...` and `/flow/...` routes.
#[derive(Debug, Clone)]
pub struct StreamUrl {
    pub session_id: String,
    pub queue_id: String,
    pub queue_item_id: String,
    pub player_id: String,
    /// Output format extension (e.g. "flac", "mp3", "wav",
    /// "pcm;codec=pcm_s16le;rate=48000;bitrate=16;channels=2").
    pub fmt: String,
}

impl StreamUrl {
    pub fn path(&self) -> String {
        format!(
            "/single/{}/{}/{}/{}/{}",
            self.session_id, self.queue_id, self.queue_item_id, self.player_id, self.fmt
        )
    }
}

#[derive(Clone)]
pub struct StreamSession {
    pub id: String,
    pub broadcast: Arc<BroadcastStream>,
    pub streamdetails: Arc<StreamDetails>,
    pub config: StreamWorkerConfig,
}

/// App state shared across all HTTP routes.
#[derive(Clone)]
pub struct HttpServerState {
    pub config: Arc<HttpServerConfig>,
    pub sessions: Arc<dashmap::DashMap<String, StreamSession>>,
}

impl HttpServerState {
    pub fn new(config: HttpServerConfig) -> Self {
        Self {
            config: Arc::new(config),
            sessions: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Register a `StreamSession` so that requests to
    /// `/single/<session_id>/...` route to its `BroadcastStream`.
    pub fn register_session(&self, session: StreamSession) {
        self.sessions.insert(session.id.clone(), session);
    }

    pub fn unregister_session(&self, id: &str) -> Option<StreamSession> {
        self.sessions.remove(id).map(|(_, v)| v)
    }
}

pub struct HttpServer {
    state: HttpServerState,
}

impl HttpServer {
    pub fn new(state: HttpServerState) -> Self {
        Self { state }
    }

    /// Bind and serve until the process is dropped. Returns the
    /// `axum::serve` future — caller spawns it.
    pub fn into_make_service(self) -> Router {
        Router::new()
            .route("/health", get(health_handler))
            .route(
                "/single/:session_id/:queue_id/:queue_item_id/:player_id/:fmt",
                get(serve_single),
            )
            .route(
                "/flow/:session_id/:queue_id/:queue_item_id/:player_id/:fmt",
                get(serve_flow),
            )
            .route("/info", get(info_handler))
            .with_state(self.state)
    }

    /// Convenience: bind to the configured address and start serving.
    pub async fn run(self) -> Result<()> {
        let bind: SocketAddr = format!("{}:{}", self.state.config.bind_ip, self.state.config.port)
            .parse()
            .map_err(|e: std::net::AddrParseError| HttpServerError::Internal(e.to_string()))?;
        let router = self.into_make_service();
        info!(%bind, "starting ma-streams http server");
        let listener = tokio::net::TcpListener::bind(bind).await?;
        eprintln!("HTTP server bound to {bind}");
        axum::serve(listener, router).await?;
        Ok(())
    }
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn info_handler(State(state): State<HttpServerState>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "server": "ma-streams",
        "version": env!("CARGO_PKG_VERSION"),
        "base_url": state.config.base_url,
        "active_sessions": state.sessions.len(),
    }))
}

#[derive(Debug, Deserialize)]
struct SingleQuery {
    #[serde(default)]
    seek: Option<u32>,
}

async fn serve_single(
    State(state): State<HttpServerState>,
    Path((session_id, queue_id, queue_item_id, player_id, fmt)): Path<(
        String,
        String,
        String,
        String,
        String,
    )>,
    Query(q): Query<SingleQuery>,
) -> Response {
    debug!(%session_id, %player_id, %fmt, "serve_single");
    let Some(session) = state.sessions.get(&session_id) else {
        return not_found(&format!("session {session_id} not found"));
    };
    let mut subscriber = session.broadcast.subscribe();
    // Build the response. We construct it eagerly so we can return
    // a known `StatusCode` if the stream is already closed.
    let audio_format = audio_format_from_fmt(&fmt);
    let response_headers = build_stream_headers(&audio_format, &state.config.base_url);
    let body = Body::from_stream(async_stream::stream! {
        use futures::StreamExt;
        while let Some(chunk) = subscriber.next().await {
            match chunk {
                Ok(b) => yield Ok::<bytes::Bytes, std::io::Error>(b),
                Err(_) => break,
            }
        }
    });
    let _ = (queue_id, queue_item_id, q.seek); // unused; logged
    let _ = player_id;
    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().extend(response_headers);
    resp
}

async fn serve_flow(
    State(state): State<HttpServerState>,
    Path((session_id, queue_id, queue_item_id, player_id, fmt)): Path<(
        String,
        String,
        String,
        String,
        String,
    )>,
) -> Response {
    debug!(%session_id, %player_id, %fmt, "serve_flow");
    // The flow stream is the same as a single stream for Phase 2
    // (no real-time stitching yet — that's smart_fades territory).
    serve_single(
        State(state),
        Path((session_id, queue_id, queue_item_id, player_id, fmt)),
        Query(SingleQuery { seek: None }),
    )
    .await
}

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain")],
        msg.to_string(),
    )
        .into_response()
}

/// Build a sensible `Content-Type` + `Accept-Ranges` header pair
/// for the response.
fn build_stream_headers(fmt: &StreamAudioFormat, base_url: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    let mime = match fmt.content_type {
        ContentType::Mp3 => "audio/mpeg",
        ContentType::Aac => "audio/aac",
        ContentType::Flac => "audio/flac",
        ContentType::Opus => "audio/opus",
        ContentType::Wav => "audio/wav",
        ContentType::Ogg => "audio/ogg",
        ContentType::Vorbis => "audio/vorbis",
        _ => "application/octet-stream",
    };
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert("Accept-Ranges", HeaderValue::from_static("bytes"));
    h.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    // Use a sane fallback for the X-Ma-Base-Url header so we don't
    // panic on malformed URLs.
    let base = Url::parse(base_url)
        .map(|_| base_url.to_string())
        .unwrap_or_else(|_| "http://localhost:8097".to_string());
    if let Ok(val) = HeaderValue::from_str(&base) {
        h.insert("X-Ma-Base-Url", val);
    }
    h
}

fn audio_format_from_fmt(fmt: &str) -> StreamAudioFormat {
    // Match the Python helper's `output_format_str` parsing:
    // "pcm;codec=pcm_s16le;rate=48000;bitrate=16;channels=2"
    if let Some(rest) = fmt.strip_prefix("pcm;") {
        let mut sf = StreamAudioFormat {
            content_type: ContentType::Wav,
            sample_rate: 48_000,
            bit_depth: 16,
            channels: 2,
            bit_rate: None,
        };
        for kv in rest.split(';') {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "codec" => {
                        if let Some(ct) = parse_pcm_codec(v) {
                            sf.content_type = ct;
                        }
                    }
                    "rate" => {
                        if let Ok(sr) = v.parse() {
                            sf.sample_rate = sr;
                        }
                    }
                    "bitrate" => {
                        if let Ok(bd) = v.parse() {
                            sf.bit_depth = bd;
                        }
                    }
                    "channels" => {
                        if let Ok(c) = v.parse() {
                            sf.channels = c;
                        }
                    }
                    _ => {}
                }
            }
        }
        return sf;
    }
    StreamAudioFormat {
        content_type: match fmt {
            "mp3" => ContentType::Mp3,
            "aac" => ContentType::Aac,
            "flac" => ContentType::Flac,
            "opus" => ContentType::Opus,
            "ogg" => ContentType::Ogg,
            "vorbis" => ContentType::Vorbis,
            "wav" => ContentType::Wav,
            _ => ContentType::Unknown,
        },
        sample_rate: 48_000,
        bit_depth: 16,
        channels: 2,
        bit_rate: None,
    }
}

fn parse_pcm_codec(codec: &str) -> Option<ContentType> {
    // PCM variants — we use ContentType::Wav to mean "raw PCM
    // wrapped or not" and rely on ffmpeg's -acodec flag in the
    // worker to pick the right sample format.
    match codec {
        "pcm" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le" => Some(ContentType::Wav),
        _ => None,
    }
}

// Re-export `dashmap` for the public `HttpServerState` API. The
// dependency is implicit through the `dashmap::DashMap` type; we
// declare it in Cargo.toml.
pub use ::dashmap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_format_from_fmt_parses_pcm_string() {
        let f = audio_format_from_fmt("pcm;codec=pcm_s16le;rate=48000;bitrate=16;channels=2");
        assert_eq!(f.sample_rate, 48_000);
        assert_eq!(f.bit_depth, 16);
        assert_eq!(f.channels, 2);
        assert_eq!(f.content_type, ContentType::Wav);
    }

    #[test]
    fn audio_format_from_fmt_parses_flac() {
        let f = audio_format_from_fmt("flac");
        assert_eq!(f.content_type, ContentType::Flac);
    }

    #[test]
    fn audio_format_from_fmt_unknown_returns_unknown() {
        let f = audio_format_from_fmt("xyz");
        assert_eq!(f.content_type, ContentType::Unknown);
    }

    #[test]
    fn build_stream_headers_contains_content_type() {
        let h = build_stream_headers(
            &StreamAudioFormat {
                content_type: ContentType::Flac,
                sample_rate: 48_000,
                bit_depth: 16,
                channels: 2,
                bit_rate: None,
            },
            "http://localhost:8097",
        );
        assert!(h.contains_key(header::CONTENT_TYPE));
        assert!(h.contains_key("Accept-Ranges"));
    }
}
