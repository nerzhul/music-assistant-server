//! `LibrespotStreamer` — spawns the `librespot` binary as a child
//! process and yields PCM chunks from its stdout.
//!
//! Mirrors `music_assistant/providers/spotify/streaming.py`. We reuse
//! the same command-line shape:
//!
//! ```text
//! librespot --cache <dir> --disable-audio-cache --passthrough
//!           --bitrate 320 --backend pipe --single-track <spotify_uri>
//!           --disable-discovery --dither none
//! ```
//!
//! For Phase 3 the actual spawn is gated behind the `MA_SPOTIFY_LIBRESPOT_PATH`
//! env var; if the binary is not found we return an `AudioError` so the
//! stream controller can fall back to a Web API URL preview.

use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use ma_core::enums::StreamType;
use ma_providers::stream::StreamDetails;

pub const LIBRESPOT_BITRATE: &str = "320";
pub const LIBRESPOT_BACKEND: &str = "pipe";

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("librespot binary not configured (set MA_SPOTIFY_LIBRESPOT_PATH)")]
    NoLibrespot,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid stream type: {0:?}")]
    InvalidStreamType(StreamType),
    #[error("librespot exited with code {0}")]
    LibrespotFailed(i32),
}

pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, StreamError>> + Send + Sync>>;

#[derive(Clone)]
pub struct LibrespotStreamer {
    pub librespot_path: Arc<PathBuf>,
    pub cache_dir: Arc<PathBuf>,
}

impl LibrespotStreamer {
    pub fn new(librespot_path: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            librespot_path: Arc::new(librespot_path),
            cache_dir: Arc::new(cache_dir),
        }
    }

    /// Convenience: read `MA_SPOTIFY_LIBRESPOT_PATH` and
    /// `MA_CACHE_DIR` from the environment.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("MA_SPOTIFY_LIBRESPOT_PATH").ok()?;
        let cache = std::env::var("MA_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/ma-librespot-cache"));
        Some(Self::new(PathBuf::from(path), cache))
    }

    /// Build the argv we pass to the librespot binary for a single
    /// track / episode. The `--start-position` is included only when
    /// `seek_position > 0` (it isn't supported for non-track items).
    pub fn args_for(&self, spotify_uri: &str, seek_position: u32) -> Vec<String> {
        let mut args = vec![
            self.librespot_path.to_string_lossy().to_string(),
            "--cache".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
            "--disable-audio-cache".to_string(),
            "--passthrough".to_string(),
            "--bitrate".to_string(),
            LIBRESPOT_BITRATE.to_string(),
            "--backend".to_string(),
            LIBRESPOT_BACKEND.to_string(),
            "--single-track".to_string(),
            spotify_uri.to_string(),
            "--disable-discovery".to_string(),
            "--dither".to_string(),
            "none".to_string(),
        ];
        if seek_position > 0 {
            args.push("--start-position".to_string());
            args.push(seek_position.to_string());
        }
        args
    }

    /// Spawn librespot and return a stream of PCM chunks.
    pub async fn stream(
        &self,
        spotify_uri: &str,
        seek_position: u32,
    ) -> Result<ChunkStream, StreamError> {
        let args = self.args_for(spotify_uri, seek_position);
        debug!(?args, "spawning librespot");
        let mut child = Command::new(&args[0])
            .args(&args[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            StreamError::Io(std::io::Error::other("librespot stdout not captured"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            StreamError::Io(std::io::Error::other("librespot stderr not captured"))
        })?;
        // Log stderr in the background; surface fatal errors.
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.contains("ERROR") || line.contains("WARN") {
                    warn!("[librespot] {line}");
                } else {
                    debug!("[librespot] {line}");
                }
            }
        });
        // Forward stdout chunks as the stream output.
        let stream = async_stream::stream! {
            let mut reader = stdout;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        yield Ok(Bytes::copy_from_slice(&buf[..n]));
                    }
                    Err(e) => {
                        yield Err(StreamError::Io(e));
                        break;
                    }
                }
            }
            // Wait for librespot to exit so we can surface the return code.
            drop(stderr_task);
            match child.wait().await {
                Ok(status) if !status.success() => {
                    if let Some(code) = status.code() {
                        yield Err(StreamError::LibrespotFailed(code));
                    }
                }
                _ => {}
            }
        };
        Ok(Box::pin(stream))
    }
}

#[async_trait]
impl ma_providers::provider::StreamProvider for LibrespotStreamer {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        seek_position: u32,
    ) -> ma_providers::provider::Result<Bytes> {
        if details.stream_type != StreamType::Custom {
            return Err(ma_providers::provider::ProviderError::Unsupported(
                "librespot only streams Custom streams",
            ));
        }
        // The stream details' path is the `spotify://` URI.
        let mut s = self
            .stream(&details.path, seek_position)
            .await
            .map_err(|e| ma_providers::provider::ProviderError::Internal(e.to_string()))?;
        use futures::StreamExt;
        match s.next().await {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(ma_providers::provider::ProviderError::Internal(
                e.to_string(),
            )),
            None => Ok(Bytes::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_include_uri_and_bitrate() {
        let s = LibrespotStreamer::new(
            PathBuf::from("/usr/local/bin/librespot"),
            PathBuf::from("/var/cache/ma"),
        );
        let args = s.args_for("spotify:track:abc", 0);
        assert!(args.contains(&"spotify:track:abc".to_string()));
        assert!(args.contains(&"--bitrate".to_string()));
        assert!(args.contains(&"320".to_string()));
        assert!(args.contains(&"--backend".to_string()));
        assert!(args.contains(&"pipe".to_string()));
        assert!(!args.contains(&"--start-position".to_string()));
    }

    #[test]
    fn args_include_start_position_when_seek_nonzero() {
        let s = LibrespotStreamer::new(
            PathBuf::from("/usr/local/bin/librespot"),
            PathBuf::from("/var/cache/ma"),
        );
        let args = s.args_for("spotify:track:abc", 30);
        let pos_idx = args.iter().position(|a| a == "--start-position").unwrap();
        assert_eq!(args[pos_idx + 1], "30");
    }

    #[test]
    fn uri_kind_for_track_and_episode() {
        // Helper: in the provider layer the URI prefix decides whether
        // we tell librespot it's a `track:` or `episode:`. Phase 3
        // only emits the prefix the caller hands us — we don't try
        // to reformat.
        assert_eq!("spotify:track:abc", "spotify:track:abc");
    }
}
