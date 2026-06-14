//! Stream reader for local files.
//!
//! The actual transcoding lives in `ma-streams` (Phase 2) — this
//! module just opens the file and reads raw PCM / encoded bytes for
//! the player controller to feed into ffmpeg. The Phase 1 implementation
//! supports plain `LocalFile` stream details and a tiny `StreamType::Custom`
//! adapter for tests.

use std::path::Path;
use std::pin::Pin;

use bytes::{Bytes, BytesMut};
use futures::Stream;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use ma_core::enums::StreamType;
use ma_providers::stream::StreamDetails;

use crate::parser::content_type_for_ext;

const CHUNK_SIZE: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid stream type for filesystem: {0:?}")]
    InvalidStreamType(StreamType),
    #[error("path is not a file: {0}")]
    NotAFile(String),
}

/// One chunk pulled from a local file.
pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes, StreamError>> + Send + Sync>>;

/// Open a local file for reading and yield chunks of up to 64 KiB.
pub async fn read_stream(details: &StreamDetails) -> Result<ChunkStream, StreamError> {
    if details.stream_type != StreamType::LocalFile && details.stream_type != StreamType::Custom {
        return Err(StreamError::InvalidStreamType(details.stream_type));
    }
    let path = std::path::PathBuf::from(&details.path);
    let meta = tokio::fs::metadata(&path).await?;
    if !meta.is_file() {
        return Err(StreamError::NotAFile(path.display().to_string()));
    }
    let file = File::open(&path).await?;
    let len = meta.len();
    let stream = async_stream(file, len, CHUNK_SIZE);
    Ok(Box::pin(stream))
}

fn async_stream(
    mut file: File,
    _len: u64,
    chunk_size: usize,
) -> impl Stream<Item = Result<Bytes, StreamError>> + Send + Sync {
    async_stream::stream! {
        let mut buf = BytesMut::with_capacity(chunk_size);
        loop {
            buf.resize(chunk_size, 0);
            let n = match file.read(&mut buf[..]).await {
                Ok(n) => n,
                Err(e) => {
                    yield Err(StreamError::Io(e));
                    break;
                }
            };
            if n == 0 {
                break;
            }
            buf.truncate(n);
            yield Ok(buf.split_to(n).freeze());
        }
    }
}

/// Read a single byte range. Used for HTTP Range requests on the
/// streams controller.
pub async fn read_range(
    details: &StreamDetails,
    start: u64,
    end_inclusive: u64,
) -> Result<Bytes, StreamError> {
    if details.stream_type != StreamType::LocalFile && details.stream_type != StreamType::Custom {
        return Err(StreamError::InvalidStreamType(details.stream_type));
    }
    let mut file = File::open(&details.path).await?;
    let len = (end_inclusive + 1 - start) as usize;
    let mut buf = BytesMut::zeroed(len);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    file.read_exact(&mut buf[..]).await?;
    Ok(buf.freeze())
}

/// Hint used by the player controller: when the path is a known audio
/// file, return its `ContentType` so the stream controller can pick a
/// matching demuxer.
pub fn content_type_for_path(path: &str) -> Option<ma_core::enums::ContentType> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    content_type_for_ext(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn read_stream_emits_full_file() {
        let tmp = std::env::temp_dir().join(format!(
            "ma-fs-stream-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("audio.mp3");
        let body: Vec<u8> = (0..200_000u32).map(|i| (i & 0xff) as u8).collect();
        std::fs::write(&p, &body).unwrap();

        let details = StreamDetails {
            provider: "filesystem_local".into(),
            item_id: ma_core::identifiers::MediaItemId("x".to_string()),
            media_type: ma_core::enums::MediaType::Track,
            stream_type: StreamType::LocalFile,
            path: p.to_string_lossy().to_string(),
            ..Default::default()
        };

        let mut s = read_stream(&details).await.unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(out.len(), body.len());
        assert_eq!(out, body);
    }

    #[tokio::test]
    async fn read_range_returns_requested_slice() {
        let tmp = std::env::temp_dir().join(format!(
            "ma-fs-range-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("audio.flac");
        let body: Vec<u8> = (0..1024u32).map(|i| (i & 0xff) as u8).collect();
        std::fs::write(&p, &body).unwrap();
        let details = StreamDetails {
            provider: "fs".into(),
            item_id: ma_core::identifiers::MediaItemId("x".to_string()),
            media_type: ma_core::enums::MediaType::Track,
            stream_type: StreamType::LocalFile,
            path: p.to_string_lossy().to_string(),
            ..Default::default()
        };
        let slice = read_range(&details, 100, 199).await.unwrap();
        assert_eq!(slice.len(), 100);
        assert_eq!(slice.as_ref(), &body[100..200]);
    }

    #[tokio::test]
    async fn read_stream_rejects_wrong_stream_type() {
        let details = StreamDetails {
            stream_type: StreamType::Http,
            path: "/tmp/x".into(),
            ..Default::default()
        };
        let r = read_stream(&details).await;
        assert!(matches!(r, Err(StreamError::InvalidStreamType(_))));
    }

    #[test]
    fn content_type_hints_match_extensions() {
        assert_eq!(
            content_type_for_path("/x/y.mp3"),
            Some(ma_core::enums::ContentType::Mp3)
        );
        assert_eq!(
            content_type_for_path("/x/y.flac"),
            Some(ma_core::enums::ContentType::Flac)
        );
        assert_eq!(content_type_for_path("/x/y.unknown"), None);
    }
}
