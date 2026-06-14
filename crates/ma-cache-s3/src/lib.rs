//! `ma-cache-s3` — S3-compatible cache backend for Music Assistant.
//!
//! Provides a single trait [`ObjectStore`] that abstracts a blob store so the
//! rest of the codebase can cache covers, podcast downloads, and music files
//! either on the local disk or in an S3-compatible bucket (AWS S3, MinIO,
//! TrueNAS, Garage).
//!
//! Two implementations are provided:
//!
//! * [`LocalDiskStore`] — a simple local-filesystem backend used as the
//!   default and as the L1 tier of a layered cache.
//! * [`S3Store`] — an S3-compatible backend backed by `rust-s3` + `quick-xml`
//!   that talks to any S3 endpoint (AWS, MinIO, etc.).
//!
//! Configuration is driven by envvars (see [`S3Config::from_env`]) so the
//! binary can be reconfigured at runtime without recompilation.

#![forbid(unsafe_code)]
#![warn(rust_2018_idioms)]

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod s3;

pub use s3::{S3Config, S3Store};

/// Errors returned by any [`ObjectStore`].
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("s3 error: {0}")]
    S3(String),
    #[error("backend error: {0}")]
    Backend(String),
}

impl From<CacheError> for io::Error {
    fn from(value: CacheError) -> Self {
        match value {
            CacheError::Io(e) => e,
            other => io::Error::other(other.to_string()),
        }
    }
}

/// A future yielding a chunk of bytes.
pub type BytesStream =
    Pin<Box<dyn tokio_stream::Stream<Item = io::Result<Bytes>> + Send + 'static>>;

/// Abstraction over a blob store. Keys are slash-separated paths (e.g.
/// `"covers/<sha1>.jpg"`) that the backend prepends with its own prefix.
#[async_trait]
pub trait ObjectStore: std::fmt::Debug + Send + Sync {
    /// Store `data` at `key`, replacing any existing object.
    async fn put(&self, key: &str, data: Bytes) -> Result<(), CacheError>;

    /// Read the full object identified by `key` in a single buffer.
    async fn get(&self, key: &str) -> Result<Bytes, CacheError>;

    /// Stream the object identified by `key`. Returns
    /// [`CacheError::NotFound`] if the object is missing.
    async fn get_stream(&self, key: &str) -> Result<BytesStream, CacheError>;

    /// Return `true` if the object exists.
    async fn exists(&self, key: &str) -> Result<bool, CacheError>;

    /// Delete the object. Returns `Ok(())` even if the object is missing.
    async fn delete(&self, key: &str) -> Result<(), CacheError>;

    /// Return the public URL for the object, or `None` if the backend does
    /// not expose public URLs (e.g. private S3 buckets).
    fn public_url(&self, key: &str) -> Option<String>;
}

/// Local filesystem implementation of [`ObjectStore`].
#[derive(Debug, Clone)]
pub struct LocalDiskStore {
    root: PathBuf,
    public_base: Option<String>,
}

impl LocalDiskStore {
    /// Create a new disk store rooted at `root`. The directory is created on
    /// demand the first time [`put`](ObjectStore::put) is called.
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            root: root.into(),
            public_base: None,
        })
    }

    /// Configure a public base URL (e.g. `"https://cdn.example.com"`).
    /// `public_url()` will return `<base>/<key>` after this is set.
    pub fn with_public_base(mut self, base: impl Into<String>) -> Self {
        self.public_base = Some(base.into());
        self
    }

    fn full_path(&self, key: &str) -> Result<PathBuf, CacheError> {
        if key.is_empty() || key.contains("..") {
            return Err(CacheError::InvalidKey(key.to_string()));
        }
        Ok(self.root.join(key))
    }
}

#[async_trait]
impl ObjectStore for LocalDiskStore {
    async fn put(&self, key: &str, data: Bytes) -> Result<(), CacheError> {
        let path = self.full_path(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut f = fs::File::create(&path).await?;
        f.write_all(&data).await?;
        f.flush().await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, CacheError> {
        let path = self.full_path(key)?;
        let mut f = fs::File::open(&path).await.map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                CacheError::NotFound(key.to_string())
            } else {
                CacheError::Io(e)
            }
        })?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await?;
        Ok(Bytes::from(buf))
    }

    async fn get_stream(&self, key: &str) -> Result<BytesStream, CacheError> {
        let path = self.full_path(key)?;
        match fs::File::open(&path).await {
            Ok(f) => {
                let stream = tokio_util::io::ReaderStream::new(f);
                Ok(Box::pin(stream))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(CacheError::NotFound(key.to_string()))
            }
            Err(e) => Err(CacheError::Io(e)),
        }
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let path = self.full_path(key)?;
        Ok(fs::try_exists(&path).await.unwrap_or(false))
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let path = self.full_path(key)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CacheError::Io(e)),
        }
    }

    fn public_url(&self, key: &str) -> Option<String> {
        self.public_base
            .as_ref()
            .map(|b| format!("{}/{}", b.trim_end_matches('/'), key))
    }
}

/// Stack two [`ObjectStore`] implementations: try the top one first
/// (typically local disk), fall back to the bottom one (typically S3). Writes
/// propagate to both stores so the bottom tier is kept warm.
#[derive(Debug, Clone)]
pub struct LayeredStore {
    top: Arc<dyn ObjectStore>,
    bottom: Arc<dyn ObjectStore>,
}

impl LayeredStore {
    /// Build a layered store. `top` is consulted first for reads; writes
    /// populate both layers.
    pub fn new(top: Arc<dyn ObjectStore>, bottom: Arc<dyn ObjectStore>) -> Arc<Self> {
        Arc::new(Self { top, bottom })
    }
}

#[async_trait]
impl ObjectStore for LayeredStore {
    async fn put(&self, key: &str, data: Bytes) -> Result<(), CacheError> {
        if let Err(e) = self.top.put(key, data.clone()).await {
            tracing::warn!(error = %e, key, "top tier put failed, continuing");
        }
        if let Err(e) = self.bottom.put(key, data).await {
            tracing::warn!(error = %e, key, "bottom tier put failed");
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, CacheError> {
        match self.top.get(key).await {
            Ok(b) => Ok(b),
            Err(CacheError::NotFound(_)) => {
                let data = self.bottom.get(key).await?;
                if let Err(e) = self.top.put(key, data.clone()).await {
                    tracing::warn!(error = %e, key, "top tier fill-back failed");
                }
                Ok(data)
            }
            Err(e) => Err(e),
        }
    }

    async fn get_stream(&self, key: &str) -> Result<BytesStream, CacheError> {
        match self.top.get_stream(key).await {
            Ok(s) => Ok(s),
            Err(CacheError::NotFound(_)) => self.bottom.get_stream(key).await,
            Err(e) => Err(e),
        }
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        Ok(self.top.exists(key).await? || self.bottom.exists(key).await?)
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let _ = self.top.delete(key).await;
        self.bottom.delete(key).await
    }

    fn public_url(&self, key: &str) -> Option<String> {
        self.top
            .public_url(key)
            .or_else(|| self.bottom.public_url(key))
    }
}

/// Compute a stable S3-style key from a hash + extension.
pub fn key_for_hash(prefix: &str, hash_hex: &str, extension: &str) -> String {
    format!(
        "{}/{}.{}",
        prefix.trim_end_matches('/'),
        hash_hex,
        extension
    )
}

/// Ensure a `Path` exists by creating it (and parents) if missing.
pub async fn ensure_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ma-cache-s3-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn local_disk_put_get() {
        let dir = tmpdir();
        let store = LocalDiskStore::new(&dir);
        store
            .put("covers/abc.jpg", Bytes::from_static(b"hello"))
            .await
            .unwrap();
        let out = store.get("covers/abc.jpg").await.unwrap();
        assert_eq!(&out[..], b"hello");
        assert!(store.exists("covers/abc.jpg").await.unwrap());
        assert!(!store.exists("covers/missing.jpg").await.unwrap());
        assert_eq!(store.public_url("covers/abc.jpg"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn local_disk_public_base() {
        let dir = tmpdir();
        let store = LocalDiskStore::new(&dir);
        let store = LocalDiskStore {
            public_base: Some("https://cdn.example.com".into()),
            ..(*store).clone()
        };
        assert_eq!(
            store.public_url("covers/abc.jpg"),
            Some("https://cdn.example.com/covers/abc.jpg".to_string())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn local_disk_rejects_traversal() {
        let dir = tmpdir();
        let store = LocalDiskStore::new(&dir);
        let err = store.put("../escape", Bytes::from_static(b"x")).await;
        assert!(matches!(err, Err(CacheError::InvalidKey(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn local_disk_missing_returns_not_found() {
        let dir = tmpdir();
        let store = LocalDiskStore::new(&dir);
        let err = store.get("covers/missing.jpg").await.unwrap_err();
        assert!(matches!(err, CacheError::NotFound(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn layered_store_falls_back() {
        let top_dir = tmpdir();
        let bot_dir = tmpdir();
        let top = LocalDiskStore::new(&top_dir);
        let bot = LocalDiskStore::new(&bot_dir);
        let layered = LayeredStore::new(top.clone(), bot.clone());
        bot.put("x/y.bin", Bytes::from_static(b"data"))
            .await
            .unwrap();
        let out = layered.get("x/y.bin").await.unwrap();
        assert_eq!(&out[..], b"data");
        // Fill-back: top should now also have the object.
        assert!(top.exists("x/y.bin").await.unwrap());
        std::fs::remove_dir_all(&top_dir).ok();
        std::fs::remove_dir_all(&bot_dir).ok();
    }

    #[test]
    fn key_for_hash_formats() {
        let k = key_for_hash("covers", "abcdef", "jpg");
        assert_eq!(k, "covers/abcdef.jpg");
        let k2 = key_for_hash("covers/", "1234", "png");
        assert_eq!(k2, "covers/1234.png");
    }
}
