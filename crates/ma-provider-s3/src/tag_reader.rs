//! Read audio tags (ID3, MP4, Vorbis, FLAC) from a remote object without
//! downloading the whole file.
//!
//! `lofty` requires seekable input, which S3 ranges provide. We fetch the
//! first ~256 KiB (which is enough for the tag headers of any common
//! container) and parse it.

use ma_cache_s3::CacheError;
use ma_provider_filesystem::parser::{parse_track_from_bytes, ParsedTrack};
use reqwest::Client;

const HEADER_BYTES: u64 = 256 * 1024;

/// Read a tag header from a remote object. Returns `Ok(None)` if the
/// object is too small to contain a tag, `Err` on I/O failures.
pub async fn read_remote_tags(
    client: &Client,
    url: &str,
    object_key: &str,
    content_length: Option<u64>,
) -> Result<Option<ParsedTrack>, TagError> {
    if let Some(len) = content_length {
        if len < 32 {
            return Ok(None);
        }
    }
    let range = format!("bytes=0-{}", HEADER_BYTES - 1);
    let resp = client
        .get(url)
        .header(reqwest::header::RANGE, range)
        .send()
        .await
        .map_err(|e| TagError::Backend(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(TagError::Backend(format!(
            "HEAD range failed: {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| TagError::Backend(e.to_string()))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let parsed = match parse_track_from_bytes(&bytes, object_key) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    Ok(Some(parsed))
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
