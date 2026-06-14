//! `ma-provider-s3` — S3-compatible music source provider.
//!
//! Reads a music library from an S3-compatible bucket (AWS S3, MinIO,
//! TrueNAS, Garage). The provider:
//!
//! 1. Lists objects under a configured prefix.
//! 2. Filters audio files (extensions: mp3, flac, m4a, ogg, opus, wav, aac).
//! 3. Parses tags with `lofty` after a small range-download of the head.
//! 4. Streams bytes back to the stream controller via the S3 `GET` API.
//!
//! Music items are keyed with the bucket-relative path so the same track
//! can be uniquely identified across multiple instances.
//!
//! Reference: `music_assistant/providers/filesystem_local/__init__.py` for
//! the file-handling pattern; the listing logic is custom for S3.

#![forbid(unsafe_code)]

pub mod manifest;
pub mod provider;
pub mod stream;
pub mod tag_reader;

pub use manifest::s3_manifest;
pub use provider::{S3Config, S3Provider};
pub use stream::S3Streamer;

use ma_cache_s3::S3Store;
use std::sync::Arc;

/// Build a `ma_cache_s3::S3Store` from a [`S3Config`]. Convenience used
/// by the server bootstrap and the tests.
pub fn build_store(cfg: &S3Config) -> Result<Arc<S3Store>, anyhow::Error> {
    let s3_cfg = ma_cache_s3::S3Config {
        endpoint: cfg.endpoint.clone(),
        region: cfg.region.clone(),
        bucket: cfg.bucket.clone(),
        key_prefix: cfg.key_prefix.clone(),
        access_key_id: cfg.access_key_id.clone(),
        secret_access_key: cfg.secret_access_key.clone(),
        session_token: cfg.session_token.clone(),
        path_style: cfg.path_style,
        publish_host: cfg.publish_host.clone(),
        presign_ttl_secs: cfg.presign_ttl_secs,
    };
    S3Store::new(s3_cfg).map_err(|e| anyhow::anyhow!(e.to_string()))
}
