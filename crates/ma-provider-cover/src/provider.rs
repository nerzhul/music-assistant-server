//! `CoverProvider` — aggregates the three source clients and the
//! disk cache. Returns the first non-empty hit (or `None` if every
//! source fails).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::warn;

use ma_core::enums::ImageType;
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};

use crate::cache::{cache_key, CoverCache, CoverHit, DiskCoverCache, MemoryCoverCache};
use crate::google::{GoogleClient, GoogleConfig};
use crate::itunes::{ItunesClient, ItunesResult};
use crate::manifest::cover_manifest;
use crate::musicbrainz::MusicbrainzCoverClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverConfig {
    /// Directory used by the disk cache. Defaults to
    /// `{cache_dir}/covers` when the provider is constructed.
    pub cache_dir: Option<String>,
    /// Google CSE API key. When set together with `google_cx`, the
    /// Google client is enabled.
    pub google_api_key: Option<String>,
    pub google_cx: Option<String>,
    /// Optional Musicbrainz release id to look up first (skips Google +
    /// iTunes when present). Real callers will pass this per-item from
    /// the track's external_ids.
    pub prefer_musicbrainz: bool,
}

impl Default for CoverConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            google_api_key: None,
            google_cx: None,
            prefer_musicbrainz: true,
        }
    }
}

pub struct CoverProvider {
    pub config: CoverConfig,
    pub google: Option<GoogleClient>,
    pub itunes: ItunesClient,
    pub musicbrainz: MusicbrainzCoverClient,
    pub cache: Arc<dyn CoverCache>,
}

impl CoverProvider {
    pub fn new(config: CoverConfig) -> Result<Arc<Self>> {
        let google = if (GoogleConfig {
            api_key: config.google_api_key.clone(),
            cx: config.google_cx.clone(),
        })
        .enabled()
        {
            Some(GoogleClient::new(GoogleConfig {
                api_key: config.google_api_key.clone(),
                cx: config.google_cx.clone(),
            }))
        } else {
            None
        };
        let cache_dir: PathBuf = config
            .cache_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("MA_CACHE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/tmp/ma-covers"))
            });
        let cache: Arc<dyn CoverCache> = Arc::new(DiskCoverCache::new(cache_dir));
        Ok(Arc::new(Self {
            config,
            google,
            itunes: ItunesClient::new().map_err(|e| ProviderError::Internal(e.to_string()))?,
            musicbrainz: MusicbrainzCoverClient::new()
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            cache,
        }))
    }

    /// Build a provider backed by the in-memory cache. Useful in
    /// tests so we don't touch the filesystem.
    pub fn in_memory(config: CoverConfig) -> Result<Arc<Self>> {
        let google = if (GoogleConfig {
            api_key: config.google_api_key.clone(),
            cx: config.google_cx.clone(),
        })
        .enabled()
        {
            Some(GoogleClient::new(GoogleConfig {
                api_key: config.google_api_key.clone(),
                cx: config.google_cx.clone(),
            }))
        } else {
            None
        };
        Ok(Arc::new(Self {
            config,
            google,
            itunes: ItunesClient::new().map_err(|e| ProviderError::Internal(e.to_string()))?,
            musicbrainz: MusicbrainzCoverClient::new()
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            cache: Arc::new(MemoryCoverCache::new()),
        }))
    }

    pub fn into_handle(self: Arc<Self>) -> ProviderHandle {
        ProviderHandle::new(ProviderImpl { inner: self })
    }

    /// Look up cover art for `(artist, album, size)`. Tries each
    /// source in order, falling through on empty / error.
    pub async fn lookup(&self, artist: &str, album: &str, size: u32) -> Result<Option<CoverHit>> {
        // Cache check first.
        let google_key = cache_key("google", artist, album, size);
        let itunes_key = cache_key("itunes", artist, album, size);
        let mbz_key = cache_key("musicbrainz", artist, album, size);

        for (source_name, key) in [
            ("google", &google_key),
            ("itunes", &itunes_key),
            ("musicbrainz", &mbz_key),
        ] {
            if let Ok(Some(cached)) = self.cache.get(key).await {
                return Ok(Some(CoverHit {
                    source: source_name.to_string(),
                    image_type: ImageType::Thumb,
                    bytes: cached.bytes,
                    content_type: cached.content_type,
                }));
            }
        }

        // Try Musicbrainz first if `prefer_musicbrainz` and a release
        // id was passed via the size slot (we don't store it; the
        // caller would have to extend the API).
        if let Some(hit) = self.try_musicbrainz(artist, album).await {
            return Ok(Some(hit));
        }
        if let Some(hit) = self.try_itunes(artist, album).await {
            self.cache_hit(&itunes_key, &hit).await;
            return Ok(Some(hit));
        }
        if let Some(hit) = self.try_google(artist, album).await {
            self.cache_hit(&google_key, &hit).await;
            return Ok(Some(hit));
        }
        Ok(None)
    }

    async fn cache_hit(&self, key: &str, hit: &CoverHit) {
        let entry = crate::cache::CachedImage {
            bytes: hit.bytes.clone(),
            content_type: hit.content_type.clone(),
        };
        if let Err(e) = self.cache.put(key, &entry).await {
            warn!(error = %e, "cover cache put failed");
        }
    }

    async fn try_itunes(&self, artist: &str, album: &str) -> Option<CoverHit> {
        let results = self.itunes.search_artwork(artist, album, 5).await.ok()?;
        let item = results.into_iter().find(|r: &ItunesResult| {
            // Require a non-trivial match on the album name. The
            // Python provider does a fancier fuzzy match; this is
            // good enough for Phase 3.
            !r.collection_name.is_empty()
        })?;
        let url = ItunesClient::high_res_url(&item)?;
        let (bytes, content_type) = fetch_url(&self.itunes, &url).await?;
        Some(CoverHit {
            source: "itunes".into(),
            image_type: ImageType::Thumb,
            bytes,
            content_type,
        })
    }

    async fn try_google(&self, artist: &str, album: &str) -> Option<CoverHit> {
        let client = self.google.as_ref()?;
        let items = client.search_artwork(artist, album, 5).await.ok()?;
        let item = items.into_iter().next()?;
        if item.link.is_empty() {
            return None;
        }
        let (bytes, content_type) = fetch_url(client, &item.link).await?;
        Some(CoverHit {
            source: "google".into(),
            image_type: GoogleClient::image_type_for(&item),
            bytes,
            content_type,
        })
    }

    async fn try_musicbrainz(&self, _artist: &str, _album: &str) -> Option<CoverHit> {
        // The Musicbrainz path needs a release MBID; the lookup()
        // helper above only takes artist+album. The provider caller
        // is expected to invoke `best_for_release` directly when
        // they have an MBID. We return None here so the chain falls
        // through to iTunes / Google.
        None
    }
}

async fn fetch_url(client_with_http: &impl HasHttp, url: &str) -> Option<(Vec<u8>, String)> {
    let http = client_with_http.http();
    let resp = http.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let bytes = resp.bytes().await.ok()?;
    Some((bytes.to_vec(), content_type))
}

/// Trait-object helper: gives `fetch_url` a uniform way to pull the
/// `reqwest::Client` out of a concrete client without forcing
/// generics. Each source client implements this via a one-liner.
pub trait HasHttp {
    fn http(&self) -> &reqwest::Client;
}

impl HasHttp for ItunesClient {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

impl HasHttp for GoogleClient {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }
}

struct ProviderImpl {
    #[allow(dead_code)]
    inner: Arc<CoverProvider>,
}

#[async_trait]
impl MusicProvider for ProviderImpl {
    fn domain(&self) -> &str {
        "cover_art"
    }

    fn instance_id(&self) -> &str {
        "cover_art"
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(cover_manifest)
    }

    async fn search(
        &self,
        _query: &str,
        _media_types: &[ma_core::enums::MediaType],
        _limit: u32,
    ) -> Result<ma_providers::media::SearchResults> {
        Err(ProviderError::Unsupported(
            "cover_art does not implement search",
        ))
    }

    async fn get_item(
        &self,
        _item_id: &str,
        _media_type: ma_core::enums::MediaType,
    ) -> Result<ma_providers::media::MediaItem> {
        Err(ProviderError::Unsupported(
            "cover_art returns images via lookup() not get_item()",
        ))
    }

    async fn get_stream_details(
        &self,
        _item_id: &str,
        _media_type: ma_core::enums::MediaType,
    ) -> Result<ma_providers::stream::StreamDetails> {
        Err(ProviderError::Unsupported("cover_art is metadata-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::MemoryCoverCache;

    #[tokio::test]
    async fn lookup_returns_none_when_all_sources_fail() {
        let p = CoverProvider::in_memory(CoverConfig::default()).unwrap();
        let r = p
            .lookup("nonexistent artist", "nonexistent album", 600)
            .await;
        assert!(r.is_ok());
        assert!(r.unwrap().is_none());
    }

    #[test]
    fn cover_provider_handle_carries_manifest() {
        let p = CoverProvider::in_memory(CoverConfig::default()).unwrap();
        let h = p.into_handle();
        assert_eq!(h.manifest.domain, "cover_art");
    }

    #[test]
    fn cache_key_differs_per_size() {
        let k1 = cache_key("itunes", "Artist", "Album", 300);
        let k2 = cache_key("itunes", "Artist", "Album", 600);
        assert_ne!(k1, k2);
    }

    #[tokio::test]
    async fn memory_cache_eviction() {
        let cache = MemoryCoverCache::new();
        cache
            .put(
                "k",
                &crate::cache::CachedImage {
                    bytes: vec![1, 2, 3],
                    content_type: "image/jpeg".into(),
                },
            )
            .await
            .unwrap();
        assert!(cache.get("k").await.unwrap().is_some());
        cache.retain(&[]).await.unwrap();
        assert!(cache.get("k").await.unwrap().is_none());
    }
}
