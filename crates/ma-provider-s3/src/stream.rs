//! `S3Streamer` — the `StreamProvider` impl for the S3 provider.
//!
//! Streams a track by signing a single S3 `GET` request and yielding the
//! resulting bytes. The stream controller (in `ma-streams`) is responsible
//! for the ffmpeg normalisation pipeline; this layer only serves bytes.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;

use ma_cache_s3::{CacheError, ObjectStore, S3Store};
use ma_providers::provider::{ProviderError, Result, StreamProvider};
use ma_providers::stream::StreamDetails;

pub struct S3Streamer {
    pub store: Arc<S3Store>,
    pub bucket: String,
    pub key_prefix: String,
}

impl std::fmt::Debug for S3Streamer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Streamer")
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

impl S3Streamer {
    /// Build a streamer from a pre-built S3 store and the bucket / prefix
    /// information needed to map a public URL back to a key.
    pub fn new(store: Arc<S3Store>, bucket: String, key_prefix: String) -> Arc<Self> {
        Arc::new(Self {
            store,
            bucket,
            key_prefix,
        })
    }

    /// Dummy streamer for unit tests. Returns errors on any streaming
    /// call; the only thing it does is implement `Debug`.
    pub fn dummy() -> Self {
        Self {
            store: dummy_store(),
            bucket: "dummy".into(),
            key_prefix: String::new(),
        }
    }
}

#[async_trait]
impl StreamProvider for S3Streamer {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        _seek_position: u32,
    ) -> Result<Bytes> {
        let key = extract_key_from_path(&details.path, &self.bucket, &self.key_prefix)
            .ok_or_else(|| ProviderError::InvalidInput("invalid s3 url".into()))?;
        let mut stream = self.store.get_stream(&key).await.map_err(map_err)?;
        match stream.next().await {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(ProviderError::Io(std::io::Error::other(e.to_string()))),
            None => Ok(Bytes::new()),
        }
    }
}

fn map_err(e: CacheError) -> ProviderError {
    match e {
        CacheError::NotFound(_) => ProviderError::MediaNotFound("s3 object".into()),
        other => ProviderError::Internal(other.to_string()),
    }
}

/// Strip the bucket + key prefix off a path to recover the object key.
fn extract_key_from_path(path: &str, bucket: &str, key_prefix: &str) -> Option<String> {
    let path = path
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let after_scheme = path.split_once('/')?.1;
    let after_bucket = after_bucket(after_scheme, bucket)?;
    let after_prefix = if key_prefix.is_empty() {
        after_bucket.to_string()
    } else {
        let prefix = format!("{}/", key_prefix.trim_end_matches('/'));
        after_bucket
            .strip_prefix(&prefix)
            .map(|s| s.to_string())
            .unwrap_or_else(|| after_bucket.to_string())
    };
    Some(after_prefix)
}

fn after_bucket<'a>(path: &'a str, bucket: &str) -> Option<&'a str> {
    if let Some(rest) = path.strip_prefix(&format!("{}/", bucket)) {
        return Some(rest);
    }
    if let Some(idx) = path.find('/') {
        return Some(&path[idx + 1..]);
    }
    None
}

fn dummy_store() -> Arc<S3Store> {
    let cfg = ma_cache_s3::S3Config {
        endpoint: "http://localhost:0".into(),
        region: "us-east-1".into(),
        bucket: "dummy".into(),
        key_prefix: String::new(),
        access_key_id: String::new(),
        secret_access_key: String::new(),
        session_token: None,
        path_style: true,
        publish_host: None,
        presign_ttl_secs: 60,
    };
    futures::executor::block_on(async { S3Store::new(cfg).expect("dummy store") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_key_path_style() {
        let key = extract_key_from_path("https://s3.example.com/ma/music/a.flac", "ma", "music/")
            .unwrap();
        assert_eq!(key, "a.flac");
    }

    #[test]
    fn extract_key_path_style_no_prefix() {
        let key = extract_key_from_path("https://s3.example.com/ma/a.flac", "ma", "").unwrap();
        assert_eq!(key, "a.flac");
    }

    #[test]
    fn extract_key_virtual_hosted() {
        let key = extract_key_from_path("https://ma.s3.example.com/music/a.flac", "ma", "music/")
            .unwrap();
        assert_eq!(key, "a.flac");
    }
}
