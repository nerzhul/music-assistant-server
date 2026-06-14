//! `FeedProvider` — generic RSS/Atom podcast provider.
//!
//! Mirrors `music_assistant/providers/podcastfeed/__init__.py`. Each
//! instance is bound to a single feed URL; the user pastes a feed
//! link in the provider config and we expose the parsed feed as
//! one `Podcast` and one or more `PodcastEpisode`s.
//!
//! The provider does not implement `StreamProvider`: the episode
//! `audio_url` is an HTTP(S) URL that the stream controller will
//! hand to ffmpeg directly (`StreamType::Http`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use ma_core::enums::{ContentType, MediaType, StreamType};
use ma_providers::media::{MediaItem, Podcast, PodcastEpisode, SearchResults};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::feed_parser::{self, ParsedFeed};
use crate::manifest::podcastfeed_manifest;

pub const FEED_CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7); // 7 days
pub const FEED_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum FeedProviderError {
    #[error("no feed url configured")]
    NoFeedUrl,
    #[error("invalid feed url: {0}")]
    InvalidFeedUrl(String),
    #[error("fetch error: {0}")]
    Fetch(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodcastConfig {
    pub feed_url: String,
}

#[derive(Debug)]
pub struct FeedProvider {
    pub config: PodcastConfig,
    pub client: reqwest::Client,
    /// Cached parsed feed. Refreshed on demand with a 7-day TTL.
    pub feed: RwLock<Option<CachedFeed>>,
}

#[derive(Debug)]
pub struct CachedFeed {
    pub feed: ParsedFeed,
    pub fetched_at: std::time::Instant,
}

impl FeedProvider {
    /// Build a `FeedProvider` from a feed URL. Returns an error if the
    /// URL is missing or invalid.
    pub fn new(config: PodcastConfig) -> std::result::Result<Arc<Self>, FeedProviderError> {
        if config.feed_url.trim().is_empty() {
            return Err(FeedProviderError::NoFeedUrl);
        }
        feed_parser::normalize_feed_url(&config.feed_url)
            .ok_or_else(|| FeedProviderError::InvalidFeedUrl(config.feed_url.clone()))?;
        let client = reqwest::Client::builder()
            .user_agent(ua())
            .timeout(FEED_FETCH_TIMEOUT)
            .build()
            .map_err(|e| FeedProviderError::Fetch(e.to_string()))?;
        Ok(Arc::new(Self {
            config,
            client,
            feed: RwLock::new(None),
        }))
    }

    pub fn with_client(
        config: PodcastConfig,
        client: reqwest::Client,
    ) -> std::result::Result<Arc<Self>, FeedProviderError> {
        if config.feed_url.trim().is_empty() {
            return Err(FeedProviderError::NoFeedUrl);
        }
        feed_parser::normalize_feed_url(&config.feed_url)
            .ok_or_else(|| FeedProviderError::InvalidFeedUrl(config.feed_url.clone()))?;
        Ok(Arc::new(Self {
            config,
            client,
            feed: RwLock::new(None),
        }))
    }

    /// The stable per-instance id derived from the feed URL.
    pub fn podcast_id(&self) -> String {
        feed_parser::feed_id(&self.config.feed_url)
    }

    /// Return the parsed feed, fetching it on demand.
    pub async fn ensure_feed(&self) -> std::result::Result<ParsedFeed, FeedProviderError> {
        if let Some(cached) = self.feed.read().as_ref() {
            if cached.fetched_at.elapsed() < FEED_CACHE_TTL {
                return Ok(cached.feed.clone());
            }
        }
        let feed = feed_parser::fetch_and_parse(&self.config.feed_url, &self.client)
            .await
            .map_err(|e| FeedProviderError::Fetch(e.to_string()))?;
        *self.feed.write() = Some(CachedFeed {
            feed: feed.clone(),
            fetched_at: std::time::Instant::now(),
        });
        Ok(feed)
    }

    /// Force a refresh, ignoring the cache TTL.
    pub async fn refresh(&self) -> std::result::Result<ParsedFeed, FeedProviderError> {
        let feed = feed_parser::fetch_and_parse(&self.config.feed_url, &self.client)
            .await
            .map_err(|e| FeedProviderError::Fetch(e.to_string()))?;
        *self.feed.write() = Some(CachedFeed {
            feed: feed.clone(),
            fetched_at: std::time::Instant::now(),
        });
        Ok(feed)
    }

    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        ProviderHandle::new(FeedProviderImpl {
            inner: self,
            instance_id: instance_id.clone(),
            domain: instance_id,
        })
    }
}

struct FeedProviderImpl {
    inner: Arc<FeedProvider>,
    instance_id: String,
    domain: String,
}

fn ua() -> String {
    format!(
        "MusicAssistantRust/{} (https://music-assistant.io)",
        env!("CARGO_PKG_VERSION")
    )
}

fn mime_to_content_type(mime: Option<&str>, url: &str) -> ContentType {
    if let Some(m) = mime.map(str::to_ascii_lowercase) {
        if m.contains("mpeg") || m.contains("mp3") {
            return ContentType::Mp3;
        }
        if m.contains("aac") || m.contains("mp4") {
            return ContentType::Aac;
        }
        if m.contains("ogg") || m.contains("vorbis") {
            return ContentType::Vorbis;
        }
        if m.contains("opus") {
            return ContentType::Opus;
        }
        if m.contains("flac") {
            return ContentType::Flac;
        }
        if m.contains("wav") {
            return ContentType::Wav;
        }
    }
    // Fallback: inspect the URL extension.
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".mp3") {
        ContentType::Mp3
    } else if lower.ends_with(".m4a") || lower.ends_with(".aac") {
        ContentType::Aac
    } else if lower.ends_with(".ogg") {
        ContentType::Vorbis
    } else if lower.ends_with(".opus") {
        ContentType::Opus
    } else if lower.ends_with(".flac") {
        ContentType::Flac
    } else if lower.ends_with(".wav") {
        ContentType::Wav
    } else {
        ContentType::Unknown
    }
}

#[async_trait]
impl MusicProvider for FeedProviderImpl {
    fn domain(&self) -> &str {
        &self.domain
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(podcastfeed_manifest)
    }

    async fn search(
        &self,
        _query: &str,
        _media_types: &[MediaType],
        _limit: u32,
    ) -> Result<SearchResults> {
        Ok(SearchResults::default())
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        let feed = self
            .inner
            .ensure_feed()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        match media_type {
            MediaType::Podcast => {
                if item_id == self.inner.podcast_id() {
                    return Ok(MediaItem::Podcast(feed_to_podcast(
                        &feed,
                        &self.domain,
                        &self.instance_id,
                        &self.inner.podcast_id(),
                    )));
                }
                Err(ProviderError::MediaNotFound(item_id.into()))
            }
            MediaType::PodcastEpisode => {
                let pos = feed
                    .episodes
                    .iter()
                    .position(|e| e.guid == item_id)
                    .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
                let ep = feed_to_episode(
                    &feed.episodes[pos],
                    pos,
                    &self.domain,
                    &self.instance_id,
                    &self.inner.podcast_id(),
                );
                Ok(MediaItem::PodcastEpisode(ep))
            }
            _ => Err(ProviderError::Unsupported("non-podcast lookup")),
        }
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        _media_type: MediaType,
    ) -> Result<StreamDetails> {
        let feed = self
            .inner
            .ensure_feed()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let ep = feed
            .episodes
            .iter()
            .find(|e| e.guid == item_id)
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        let url = ep
            .audio_url
            .clone()
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        let content_type = mime_to_content_type(ep.audio_mime.as_deref(), &url);
        debug!(item_id, "podcast episode stream url resolved");
        Ok(StreamDetails {
            provider: self.domain.clone(),
            item_id: ma_core::identifiers::MediaItemId(item_id.to_string()),
            media_type: MediaType::PodcastEpisode,
            stream_type: StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type,
                sample_rate: 0,
                bit_depth: 0,
                channels: 0,
                bit_rate: None,
            }),
            path: url,
            parts: Vec::new(),
            duration: ep.duration.map(|d| d.as_secs_f64()),
            can_seek: true,
            live: false,
            title: Some(ep.title.clone()),
            artist: ep.author.clone(),
            album: Some(feed.title.clone()),
        })
    }

    async fn browse(&self, path: &str) -> Result<Vec<MediaItem>> {
        let feed = self
            .inner
            .ensure_feed()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let podcast = feed_to_podcast(
            &feed,
            &self.domain,
            &self.instance_id,
            &self.inner.podcast_id(),
        );
        let podcast_id = self.inner.podcast_id();
        let episodes_path = format!("episodes/{podcast_id}");
        if path == "episodes" || path == episodes_path {
            let mut out = Vec::with_capacity(feed.episodes.len());
            for (i, ep) in feed.episodes.iter().enumerate() {
                out.push(MediaItem::PodcastEpisode(feed_to_episode(
                    ep,
                    i,
                    &self.domain,
                    &self.instance_id,
                    &podcast_id,
                )));
            }
            return Ok(out);
        }
        if path.is_empty() || path == "/" {
            return Ok(vec![MediaItem::Podcast(podcast)]);
        }
        warn!(path, "unknown browse path for podcastfeed");
        Ok(Vec::new())
    }
}

fn feed_to_podcast(
    feed: &ParsedFeed,
    domain: &str,
    instance_id: &str,
    podcast_id: &str,
) -> Podcast {
    Podcast {
        item_id: ma_core::identifiers::MediaItemId(podcast_id.to_string()),
        provider: instance_id.to_string(),
        name: feed.title.clone(),
        publisher: feed.author.clone(),
        total_episodes: feed.episodes.len() as u32,
        image_url: feed.cover_url.clone(),
        uri: format!("{domain}://{instance_id}/{podcast_id}"),
    }
}

fn feed_to_episode(
    ep: &crate::feed_parser::ParsedEpisode,
    position: usize,
    domain: &str,
    instance_id: &str,
    podcast_id: &str,
) -> PodcastEpisode {
    PodcastEpisode {
        item_id: ma_core::identifiers::MediaItemId(ep.guid.clone()),
        provider: instance_id.to_string(),
        name: ep.title.clone(),
        duration: ep.duration.map(|d| d.as_secs_f64()),
        podcast: Some(Podcast {
            item_id: ma_core::identifiers::MediaItemId(podcast_id.to_string()),
            provider: instance_id.to_string(),
            name: String::new(),
            publisher: None,
            total_episodes: 0,
            image_url: ep.image_url.clone(),
            uri: format!("{domain}://{instance_id}/{podcast_id}"),
        }),
        position: Some(position as u32),
        publish_date: ep.published.map(|d| d.to_rfc3339()),
        audio_url: ep.audio_url.clone(),
        uri: format!("{domain}://{instance_id}/{podcast_id}#{}", ep.guid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_parser::parse;

    const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>Test</title>
    <description>D</description>
    <link>https://example.com</link>
    <itunes:image href="https://example.com/c.jpg"/>
    <item>
      <title>e1</title>
      <guid>g1</guid>
      <pubDate>2025-04-14T12:00:00Z</pubDate>
      <itunes:duration>00:30:00</itunes:duration>
      <enclosure url="https://example.com/e1.mp3" length="123" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn mime_to_content_type_dispatch() {
        assert_eq!(
            mime_to_content_type(Some("audio/mpeg"), "x"),
            ContentType::Mp3
        );
        assert_eq!(
            mime_to_content_type(Some("audio/mp4"), "x"),
            ContentType::Aac
        );
        assert_eq!(
            mime_to_content_type(Some("audio/ogg"), "x"),
            ContentType::Vorbis
        );
        assert_eq!(
            mime_to_content_type(Some("audio/opus"), "x"),
            ContentType::Opus
        );
        assert_eq!(
            mime_to_content_type(None, "https://x/track.flac"),
            ContentType::Flac
        );
        assert_eq!(mime_to_content_type(None, ""), ContentType::Unknown);
    }

    #[test]
    fn feed_id_stable() {
        let f1 = FeedProvider::new(PodcastConfig {
            feed_url: "https://example.com/feed.xml".into(),
        })
        .unwrap();
        let f2 = FeedProvider::new(PodcastConfig {
            feed_url: "https://example.com/feed.xml".into(),
        })
        .unwrap();
        assert_eq!(f1.podcast_id(), f2.podcast_id());
    }

    #[test]
    fn rejects_empty_url() {
        let err = FeedProvider::new(PodcastConfig {
            feed_url: String::new(),
        })
        .unwrap_err();
        assert!(matches!(err, FeedProviderError::NoFeedUrl));
    }

    #[test]
    fn rejects_ftp_url() {
        let err = FeedProvider::new(PodcastConfig {
            feed_url: "ftp://x/feed".into(),
        })
        .unwrap_err();
        assert!(matches!(err, FeedProviderError::InvalidFeedUrl(_)));
    }

    #[tokio::test]
    async fn parses_using_embedded_sample() {
        let feed = parse(FEED, "https://example.com/feed.xml").unwrap();
        assert_eq!(feed.episodes.len(), 1);
        assert_eq!(feed.episodes[0].guid, "g1");
    }
}
