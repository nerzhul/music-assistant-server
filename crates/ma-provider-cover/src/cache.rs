//! Cover art cache trait + in-memory, on-disk, and database-backed
//! implementations. The memory cache is the L0 (per-process fast
//! path), the disk cache is the L1 (cold-restart survives), and the
//! database cache is the L2 (multi-instance shared).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use sha1_smol::Sha1;
use thiserror::Error;

use ma_core::enums::ImageType;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("backend error: {0}")]
    Backend(String),
}

/// One cached image with its `ContentType`.
#[derive(Debug, Clone)]
pub struct CachedImage {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// What the player / library controller needs from a hit: the bytes,
/// the mime type, and which source provided it.
#[derive(Debug, Clone)]
pub struct CoverHit {
    pub source: String,
    pub image_type: ImageType,
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[async_trait]
pub trait CoverCache: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<CachedImage>, CacheError>;
    async fn put(&self, key: &str, image: &CachedImage) -> Result<(), CacheError>;
    /// Invalidate all entries that aren't in the supplied set. Used by
    /// the GC pass to evict dead keys.
    async fn retain(&self, keep: &[String]) -> Result<(), CacheError>;
}

/// Stable cache key for a (artist, album, source) tuple. The MA cover
/// provider uses the format `{source}:{artist}|{album}|{size}` hashed
/// with SHA-1.
pub fn cache_key(source: &str, artist: &str, album: &str, size: u32) -> String {
    let mut h = Sha1::new();
    h.update(source.as_bytes());
    h.update(b"\0");
    h.update(artist.as_bytes());
    h.update(b"\0");
    h.update(album.as_bytes());
    h.update(b"\0");
    h.update(size.to_le_bytes().as_slice());
    let digest = h.digest();
    let mut out = String::with_capacity(40);
    for byte in digest.bytes() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn cache_filename(key: &str, ext: &str) -> String {
    format!("{key}.{ext}")
}

#[derive(Default)]
pub struct MemoryCoverCache {
    inner: Arc<RwLock<std::collections::HashMap<String, CachedImage>>>,
}

impl MemoryCoverCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CoverCache for MemoryCoverCache {
    async fn get(&self, key: &str) -> Result<Option<CachedImage>, CacheError> {
        Ok(self.inner.read().get(key).cloned())
    }

    async fn put(&self, key: &str, image: &CachedImage) -> Result<(), CacheError> {
        self.inner.write().insert(key.to_string(), image.clone());
        Ok(())
    }

    async fn retain(&self, keep: &[String]) -> Result<(), CacheError> {
        let keep_set: std::collections::HashSet<&str> = keep.iter().map(|s| s.as_str()).collect();
        self.inner
            .write()
            .retain(|k, _| keep_set.contains(k.as_str()));
        Ok(())
    }
}

/// Filesystem-backed cache. Stores each image at
/// `{base_dir}/{key}.{ext}` with sidecar metadata in
/// `{base_dir}/{key}.json`. The disk cache is the default for the
/// cover provider; the memory cache is mainly for tests.
pub struct DiskCoverCache {
    pub base_dir: PathBuf,
}

impl DiskCoverCache {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn ensure_dir(&self) -> Result<(), CacheError> {
        if !self.base_dir.exists() {
            std::fs::create_dir_all(&self.base_dir)?;
        }
        Ok(())
    }
}

#[async_trait]
impl CoverCache for DiskCoverCache {
    async fn get(&self, key: &str) -> Result<Option<CachedImage>, CacheError> {
        self.ensure_dir()?;
        let dir = &self.base_dir;
        let json_path = dir.join(format!("{key}.json"));
        if !json_path.exists() {
            return Ok(None);
        }
        let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json_path)?)?;
        let rel = meta
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CacheError::NotFound(key.into()))?;
        let bytes = std::fs::read(dir.join(rel))?;
        let content_type = meta
            .get("content_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("image/jpeg")
            .to_string();
        Ok(Some(CachedImage {
            bytes,
            content_type,
        }))
    }

    async fn put(&self, key: &str, image: &CachedImage) -> Result<(), CacheError> {
        self.ensure_dir()?;
        // Pick the extension from the content type. Fall back to `.bin`.
        let ext = match image.content_type.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            "image/gif" => "gif",
            other => {
                tracing::debug!(content_type = %other, "unknown cover content type, saving as bin");
                "bin"
            }
        };
        let path = format!("{key}.{ext}");
        let full = self.base_dir.join(&path);
        std::fs::write(&full, &image.bytes)?;
        let meta = serde_json::json!({
            "path": path,
            "content_type": image.content_type,
            "stored_at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        std::fs::write(
            self.base_dir.join(format!("{key}.json")),
            serde_json::to_string(&meta)?,
        )?;
        Ok(())
    }

    async fn retain(&self, keep: &[String]) -> Result<(), CacheError> {
        if !self.base_dir.exists() {
            return Ok(());
        }
        let keep_set: std::collections::HashSet<&str> = keep.iter().map(|s| s.as_str()).collect();
        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !keep_set.contains(stem) {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(())
    }
}

impl DiskCoverCache {
    /// Walk the cache directory and list every stored key.
    pub fn list_keys(&self, _base: &Path) -> Result<Vec<String>, CacheError> {
        let mut keys = Vec::new();
        if !self.base_dir.exists() {
            return Ok(keys);
        }
        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    keys.push(stem.to_string());
                }
            }
        }
        Ok(keys)
    }
}

/// Database-backed cache (L2). Backs onto `ma_storage::cover_art`. Used
/// when the server is configured with `MA_DATABASE_URL` so cover
/// hits survive restarts and are shared across instances.
pub struct DatabaseCoverCache {
    repo: ma_storage::CoverArtRepository,
    provider: String,
}

impl DatabaseCoverCache {
    /// Build a new database-backed cache. `provider` is the
    /// `cover_art.provider` column used when writing new rows
    /// (e.g. `"itunes"`, `"musicbrainz"`, `"google_cse"`).
    pub fn new(db: &ma_storage::Database, provider: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            repo: ma_storage::CoverArtRepository::new(db.pool().clone()),
            provider: provider.into(),
        })
    }
}

#[async_trait]
impl CoverCache for DatabaseCoverCache {
    async fn get(&self, key: &str) -> Result<Option<CachedImage>, CacheError> {
        let rec = self
            .repo
            .find(&self.provider, key)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        Ok(rec.map(|r| CachedImage {
            bytes: r.bytes,
            content_type: r.content_type,
        }))
    }

    async fn put(&self, key: &str, image: &CachedImage) -> Result<(), CacheError> {
        let now = chrono::Utc::now();
        let record = ma_storage::CoverArtRecord {
            image_id: format!("{}|{}", self.provider, key),
            provider: self.provider.clone(),
            item_id: key.to_string(),
            url: None,
            content_type: image.content_type.clone(),
            width: None,
            height: None,
            bytes: image.bytes.clone(),
            fetched_at: now,
            last_used_at: None,
        };
        self.repo
            .upsert(&record)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn retain(&self, _keep: &[String]) -> Result<(), CacheError> {
        // The DB cache uses a different retention policy: rows are
        // pruned by a background job, not on every write. This stub
        // is here so the trait can be implemented uniformly.
        Ok(())
    }
}

#[cfg(test)]
mod database_cache_tests {
    use super::*;
    use ma_storage::pool::{Database, DatabaseConfig};

    #[tokio::test]
    async fn database_cache_round_trip() {
        let db = Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap();
        let cache = DatabaseCoverCache::new(&db, "itunes");
        let key = "Aphex Twin / Selected Ambient Works 85-92 / 800";
        assert!(cache.get(key).await.unwrap().is_none());
        cache
            .put(
                key,
                &CachedImage {
                    bytes: b"\xFF\xD8\xFF\xE0fake".to_vec(),
                    content_type: "image/jpeg".into(),
                },
            )
            .await
            .unwrap();
        let hit = cache.get(key).await.unwrap().unwrap();
        assert_eq!(hit.bytes, b"\xFF\xD8\xFF\xE0fake");
        assert_eq!(hit.content_type, "image/jpeg");
    }

    #[tokio::test]
    async fn database_cache_different_providers_are_isolated() {
        let db = Database::connect(DatabaseConfig::in_memory_sqlite())
            .await
            .unwrap();
        let itunes = DatabaseCoverCache::new(&db, "itunes");
        let mb = DatabaseCoverCache::new(&db, "musicbrainz");
        let key = "shared-key";
        itunes
            .put(
                key,
                &CachedImage {
                    bytes: b"itunes-bytes".to_vec(),
                    content_type: "image/jpeg".into(),
                },
            )
            .await
            .unwrap();
        mb.put(
            key,
            &CachedImage {
                bytes: b"mb-bytes".to_vec(),
                content_type: "image/png".into(),
            },
        )
        .await
        .unwrap();
        let a = itunes.get(key).await.unwrap().unwrap();
        let b = mb.get(key).await.unwrap().unwrap();
        assert_eq!(a.bytes, b"itunes-bytes");
        assert_eq!(b.bytes, b"mb-bytes");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_unique() {
        let k1 = cache_key("google", "Aphex Twin", "Selected Ambient Works 85-92", 800);
        let k2 = cache_key("google", "Aphex Twin", "Selected Ambient Works 85-92", 800);
        let k3 = cache_key("itunes", "Aphex Twin", "Selected Ambient Works 85-92", 800);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[tokio::test]
    async fn memory_cache_round_trip() {
        let cache = MemoryCoverCache::new();
        let key = "abc";
        assert!(cache.get(key).await.unwrap().is_none());
        cache
            .put(
                key,
                &CachedImage {
                    bytes: vec![1, 2, 3],
                    content_type: "image/jpeg".into(),
                },
            )
            .await
            .unwrap();
        let got = cache.get(key).await.unwrap().unwrap();
        assert_eq!(got.bytes, vec![1, 2, 3]);
        cache.retain(&[]).await.unwrap();
        assert!(cache.get(key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn disk_cache_round_trip() {
        let tmp = std::env::temp_dir().join(format!(
            "ma-cover-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let cache = DiskCoverCache::new(tmp.clone());
        let key = "cover1";
        cache
            .put(
                key,
                &CachedImage {
                    bytes: vec![0xff, 0xd8, 0xff, 0xe0],
                    content_type: "image/jpeg".into(),
                },
            )
            .await
            .unwrap();
        let got = cache.get(key).await.unwrap().unwrap();
        assert_eq!(got.bytes, vec![0xff, 0xd8, 0xff, 0xe0]);
        assert_eq!(got.content_type, "image/jpeg");
        assert!(tmp.join("cover1.jpg").exists());
        assert!(tmp.join("cover1.json").exists());
        // retain
        cache.retain(&[]).await.unwrap();
        assert!(cache.get(key).await.unwrap().is_none());
    }
}
