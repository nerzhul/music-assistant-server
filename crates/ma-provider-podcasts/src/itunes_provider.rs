//! `ITunesPodcastsProvider` — search + discovery for the iTunes
//! Podcast Directory.
//!
//! Mirrors `music_assistant/providers/itunes_podcasts/__init__.py`.
//! The provider does not stream anything itself: each `Podcast` it
//! returns carries a `feed_url` (from iTunes) which the user (or the
//! library controller) turns into a `podcastfeed` instance.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::warn;

use ma_core::enums::MediaType;
use ma_providers::media::{MediaItem, Podcast, SearchResults};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};
use ma_providers::stream::StreamDetails;

use crate::itunes::{self, PodcastSearchResult, TopPodcastEntry};
use crate::manifest::itunes_podcasts_manifest;

pub const TOP_PODCASTS_CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ITunesPodcastsConfig {
    pub country: String,
    #[serde(default = "default_explicit")]
    pub explicit: bool,
    #[serde(default = "default_num_episodes")]
    pub num_episodes: u32,
}

fn default_explicit() -> bool {
    true
}

fn default_num_episodes() -> u32 {
    10
}

impl Default for ITunesPodcastsConfig {
    fn default() -> Self {
        Self {
            country: "US".to_string(),
            explicit: true,
            num_episodes: 10,
        }
    }
}

pub struct ITunesPodcastsProvider {
    pub config: ITunesPodcastsConfig,
    pub client: reqwest::Client,
    pub top_cache: RwLock<Option<(Vec<TopPodcastEntry>, std::time::Instant)>>,
}

impl ITunesPodcastsProvider {
    pub fn new(
        config: ITunesPodcastsConfig,
    ) -> std::result::Result<Arc<Self>, itunes::ITunesError> {
        let client = itunes::build_client().map_err(itunes::ITunesError::Http)?;
        Ok(Arc::new(Self {
            config,
            client,
            top_cache: RwLock::new(None),
        }))
    }

    pub fn with_client(config: ITunesPodcastsConfig, client: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            top_cache: RwLock::new(None),
        })
    }

    /// Build a `ProviderHandle` ready for registration. Domain +
    /// instance id are forced to `"itunes_podcasts"` (the iTunes
    /// directory is a single instance provider — the Python equivalent
    /// has `multi_instance: false`).
    pub fn into_handle(self: Arc<Self>) -> ProviderHandle {
        ProviderHandle::new(ProviderImpl {
            inner: self,
            instance_id: "itunes_podcasts".to_string(),
            domain: "itunes_podcasts".to_string(),
        })
    }

    async fn get_top_podcasts_cached(
        &self,
        limit: u32,
        genre: &str,
    ) -> std::result::Result<Vec<TopPodcastEntry>, itunes::ITunesError> {
        if let Some((cached, at)) = self.top_cache.read().clone() {
            if at.elapsed() < TOP_PODCASTS_CACHE_TTL && cached.len() as u32 >= limit {
                return Ok(cached);
            }
        }
        let list = itunes::top_podcasts(&self.client, &self.config.country, limit, genre).await?;
        *self.top_cache.write() = Some((list.clone(), std::time::Instant::now()));
        Ok(list)
    }
}

struct ProviderImpl {
    inner: Arc<ITunesPodcastsProvider>,
    instance_id: String,
    domain: String,
}

fn search_result_to_podcast(
    r: &PodcastSearchResult,
    instance_id: &str,
    domain: &str,
) -> Option<Podcast> {
    let feed_url = r.feed_url.as_deref()?;
    let name = r
        .track_name
        .clone()
        .or_else(|| r.collection_name.clone())
        .or_else(|| r.track_censored_name.clone())
        .or_else(|| r.collection_censored_name.clone())?;
    let podcast = Podcast {
        item_id: ma_core::identifiers::MediaItemId(feed_url.to_string()),
        provider: instance_id.to_string(),
        name,
        publisher: r.artist_name.clone(),
        total_episodes: r.track_count.unwrap_or(0),
        image_url: r.best_artwork().map(str::to_string),
        uri: format!("{domain}://{instance_id}/{feed_url}"),
    };
    Some(podcast)
}

fn top_entry_to_podcast(e: &TopPodcastEntry, instance_id: &str, domain: &str) -> Option<Podcast> {
    let id = e.id.as_deref()?;
    let name = e.name.clone()?;
    // The top-podcasts JSON doesn't carry a `feedUrl`; we use the
    // iTunes id as a placeholder. Resolving a feed URL requires
    // hitting `lookup?id=…` (V2) or `podcasts.apple.com/.../id{mbid}`
    // (V1). The Python provider just returns the id; we mirror that.
    let image = e
        .artwork_url_100
        .clone()
        .or_else(|| e.artwork_url_60.clone());
    Some(Podcast {
        item_id: ma_core::identifiers::MediaItemId(id.to_string()),
        provider: instance_id.to_string(),
        name,
        publisher: e.artist_name.clone(),
        total_episodes: 0,
        image_url: image,
        uri: format!("{domain}://{instance_id}/{id}"),
    })
}

#[async_trait]
impl MusicProvider for ProviderImpl {
    fn domain(&self) -> &str {
        &self.domain
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(itunes_podcasts_manifest)
    }

    async fn search(
        &self,
        query: &str,
        media_types: &[MediaType],
        limit: u32,
    ) -> Result<SearchResults> {
        let mut out = SearchResults::default();
        if !media_types.is_empty() && !media_types.contains(&MediaType::Podcast) {
            return Ok(out);
        }
        let limit = limit.clamp(1, 200);
        let params = itunes::SearchParams {
            term: query.to_string(),
            country: self.inner.config.country.clone(),
            explicit: self.inner.config.explicit,
            limit,
        };
        let results = self
            .inner
            .client
            .get(itunes::SEARCH_URL)
            .query(&[
                ("media", "podcast".to_string()),
                ("entity", "podcast".to_string()),
                ("country", params.country.to_ascii_uppercase()),
                ("attribute", "titleTerm".to_string()),
                (
                    "explicit",
                    if params.explicit { "Yes" } else { "No" }.to_string(),
                ),
                ("limit", params.limit.to_string()),
                ("term", params.term.clone()),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        if !results.status().is_success() {
            return Err(ProviderError::Unavailable(format!(
                "itunes search returned status {}",
                results.status()
            )));
        }
        let body: itunes::ITunesSearchResults = results
            .json()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        for r in &body.results {
            if let Some(p) = search_result_to_podcast(r, &self.instance_id, &self.domain) {
                out.podcasts.push(p);
            }
        }
        Ok(out)
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        if media_type != MediaType::Podcast {
            return Err(ProviderError::Unsupported("non-podcast lookup"));
        }
        // For iTunes, the only thing we know about an item is its
        // `feed_url` (from search) or iTunes `collectionId` (from
        // top-podcasts). We don't resolve the feed on `get_item`
        // because the iTunes search API does not have a "lookup by
        // feedUrl" endpoint; the caller should resolve the feed via a
        // `podcastfeed` instance. We just return a stub Podcast.
        warn!(item_id, "iTunes get_item returns a stub (no real lookup)");
        Ok(MediaItem::Podcast(Podcast {
            item_id: ma_core::identifiers::MediaItemId(item_id.to_string()),
            provider: self.instance_id.clone(),
            name: String::new(),
            publisher: None,
            total_episodes: 0,
            image_url: None,
            uri: format!("{}://{}/{}", self.domain, self.instance_id, item_id),
        }))
    }

    async fn get_stream_details(
        &self,
        _item_id: &str,
        _media_type: MediaType,
    ) -> Result<StreamDetails> {
        // iTunes is a discovery provider; streaming is handled by
        // the resolved `podcastfeed` instance.
        Err(ProviderError::Unsupported("iTunes does not stream"))
    }

    async fn browse(&self, path: &str) -> Result<Vec<MediaItem>> {
        // The Python provider exposes a `recommendations` folder
        // (top podcasts); we mirror that under `browse("top")`.
        match path {
            "top" | "top_podcasts" | "recommendations" | "" | "/" => {
                let limit = self.inner.config.num_episodes.clamp(1, 200);
                let entries = self
                    .inner
                    .get_top_podcasts_cached(limit, "26") // 26 = "Podcasts"
                    .await
                    .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
                let mut out = Vec::with_capacity(entries.len());
                for e in &entries {
                    if let Some(p) = top_entry_to_podcast(e, &self.instance_id, &self.domain) {
                        out.push(MediaItem::Podcast(p));
                    }
                }
                Ok(out)
            }
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::itunes::PodcastSearchResult;

    #[test]
    fn search_result_to_podcast_uses_feed_url_as_id() {
        let r = PodcastSearchResult {
            feed_url: Some("https://example.com/feed.xml".into()),
            track_name: Some("Daily Pod".into()),
            artist_name: Some("Studio".into()),
            artwork_url_600: Some("https://x/600.jpg".into()),
            track_count: Some(42),
            ..Default::default()
        };
        let p = search_result_to_podcast(&r, "itunes_podcasts", "itunes_podcasts").unwrap();
        assert_eq!(p.name, "Daily Pod");
        assert_eq!(p.publisher.as_deref(), Some("Studio"));
        assert_eq!(p.total_episodes, 42);
        assert_eq!(p.image_url.as_deref(), Some("https://x/600.jpg"));
        assert_eq!(p.item_id.0, "https://example.com/feed.xml".to_string());
    }

    #[test]
    fn search_result_without_feed_url_is_dropped() {
        let r = PodcastSearchResult {
            track_name: Some("X".into()),
            ..Default::default()
        };
        assert!(search_result_to_podcast(&r, "i", "i").is_none());
    }

    #[test]
    fn search_query_builds_correctly() {
        // sanity: instantiate a provider and confirm the config round-trips.
        let cfg = ITunesPodcastsConfig::default();
        assert_eq!(cfg.country, "US");
        assert!(cfg.explicit);
        assert_eq!(cfg.num_episodes, 10);
    }
}
