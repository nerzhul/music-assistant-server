//! `RadioBrowserProvider` — the `MusicProvider` impl that wraps the
//! REST client and exposes the same `search` / `browse` /
//! `get_stream_details` API as the Python `radiobrowser` provider.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::warn;

use ma_core::enums::{ContentType, MediaType, StreamType};
use ma_providers::media::{MediaItem, Radio as MediaRadio, SearchResults};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::client::{Order, RadioBrowserClient, Station};

/// Map a radio-browser codec string to a `ContentType`. The Python
/// provider uses `ContentType.try_parse` which falls back to `Unknown`
/// for anything that isn't recognised.
fn parse_codec(codec: &str) -> ContentType {
    match codec.to_ascii_uppercase().as_str() {
        "MP3" => ContentType::Mp3,
        "AAC" | "AAC+" => ContentType::Aac,
        "OGG" | "VORBIS" => ContentType::Vorbis,
        "OPUS" => ContentType::Opus,
        "FLAC" => ContentType::Flac,
        _ => ContentType::Unknown,
    }
}

/// In-process TTL cache for the API responses (matches the
/// `@use_cache(3600 * 6)` / `(3600 * 24 * 7)` annotations in the
/// Python provider).
#[derive(Default)]
struct Cache<T: Clone> {
    value: RwLock<Option<(T, std::time::Instant)>>,
}

impl<T: Clone> Cache<T> {
    fn get(&self, ttl: std::time::Duration) -> Option<T> {
        let guard = self.value.read();
        let (v, at) = guard.as_ref()?.clone();
        if at.elapsed() < ttl {
            Some(v)
        } else {
            None
        }
    }
    fn put(&self, value: T) {
        *self.value.write() = Some((value, std::time::Instant::now()));
    }
}

pub struct RadioBrowserProvider {
    client: RadioBrowserClient,
    cached_popular: Cache<Vec<MediaRadio>>,
    cached_voted: Cache<Vec<MediaRadio>>,
    cached_countries: Cache<Vec<CountryFolder>>,
    cached_languages: Cache<Vec<LanguageFolder>>,
    cached_tags: Cache<Vec<TagFolder>>,
}

#[derive(Clone)]
pub struct CountryFolder {
    pub code: String,
    pub name: String,
    pub favicon: Option<String>,
}

#[derive(Clone)]
pub struct LanguageFolder(pub String);

#[derive(Clone)]
pub struct TagFolder(pub String);

impl RadioBrowserProvider {
    pub fn new() -> Result<Arc<Self>> {
        let client =
            RadioBrowserClient::new().map_err(|e| ProviderError::Internal(e.to_string()))?;
        Ok(Arc::new(Self {
            client,
            cached_popular: Cache::default(),
            cached_voted: Cache::default(),
            cached_countries: Cache::default(),
            cached_languages: Cache::default(),
            cached_tags: Cache::default(),
        }))
    }

    pub fn with_client(client: RadioBrowserClient) -> Arc<Self> {
        Arc::new(Self {
            client,
            cached_popular: Cache::default(),
            cached_voted: Cache::default(),
            cached_countries: Cache::default(),
            cached_languages: Cache::default(),
            cached_tags: Cache::default(),
        })
    }

    /// Register this provider under a stable instance id.
    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        ProviderHandle::new(ProviderImpl {
            inner: self,
            instance_id,
        })
    }

    fn parse_radio(&self, station: Station) -> MediaRadio {
        let provider = "radiobrowser".to_string();
        MediaRadio {
            item_id: ma_core::identifiers::MediaItemId(station.stationuuid.clone()),
            provider: provider.clone(),
            name: station.name.clone(),
            homepage: if station.homepage.is_empty() {
                None
            } else {
                Some(station.homepage.clone())
            },
            favicon_url: if station.favicon.is_empty() {
                None
            } else {
                Some(station.favicon.clone())
            },
            click_count: Some(station.click_count() as u64),
            uri: format!("radiobrowser://radio/{provider}/{}", station.stationuuid),
        }
    }
}

struct ProviderImpl {
    inner: Arc<RadioBrowserProvider>,
    instance_id: String,
}

#[async_trait]
impl MusicProvider for ProviderImpl {
    fn domain(&self) -> &str {
        "radiobrowser"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(crate::manifest::radiobrowser_manifest)
    }

    async fn search(
        &self,
        query: &str,
        media_types: &[MediaType],
        limit: u32,
    ) -> Result<SearchResults> {
        let mut results = SearchResults::default();
        if !media_types.contains(&MediaType::Radio) {
            return Ok(results);
        }
        match self.inner.client.search(query, limit).await {
            Ok(stations) => {
                results.radio = stations
                    .into_iter()
                    .take(limit as usize)
                    .map(|s| self.inner.parse_radio(s))
                    .collect();
            }
            Err(e) => warn!(error = %e, query, "radiobrowser search failed"),
        }
        Ok(results)
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        if media_type != MediaType::Radio {
            return Err(ProviderError::Unsupported("non-radio lookup"));
        }
        let station = self
            .inner
            .client
            .station(item_id)
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        Ok(MediaItem::Radio(self.inner.parse_radio(station)))
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        _media_type: MediaType,
    ) -> Result<StreamDetails> {
        let station = self
            .inner
            .client
            .station(item_id)
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let stream_url = if !station.url_resolved.is_empty() {
            station.url_resolved.clone()
        } else {
            station.url.clone()
        };
        if stream_url.is_empty() {
            return Err(ProviderError::MediaNotFound(format!(
                "radio {} has no stream url",
                item_id
            )));
        }
        // Best-effort click tracking (matches the Python provider).
        if let Err(e) = self.inner.client.station_click(item_id).await {
            warn!(error = %e, "station_click failed");
        }
        let content_type = if station.hls == 1 {
            // HLS: leave to ffmpeg.
            ContentType::Unknown
        } else {
            parse_codec(&station.codec)
        };
        Ok(StreamDetails {
            provider: self.domain().to_string(),
            item_id: ma_core::identifiers::MediaItemId(station.stationuuid.clone()),
            media_type: MediaType::Radio,
            stream_type: StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type,
                sample_rate: 44_100,
                bit_depth: 16,
                channels: 2,
                bit_rate: if station.bitrate > 0 {
                    Some(station.bitrate)
                } else {
                    None
                },
            }),
            path: stream_url,
            parts: Vec::new(),
            duration: None,
            can_seek: false,
            live: true,
            title: Some(station.name),
            artist: None,
            album: None,
        })
    }

    async fn browse(&self, path: &str) -> Result<Vec<MediaItem>> {
        // Path grammar: "popularity" | "popularity/popular" | "popularity/votes"
        //              | "category" | "category/country" | "category/country/<code>"
        //              | "category/language" | "category/language/<name>"
        //              | "category/tag"     | "category/tag/<name>"
        let parts: Vec<&str> = if path.is_empty() {
            vec![]
        } else {
            path.split('/').filter(|s| !s.is_empty()).collect()
        };
        match parts.as_slice() {
            [] => Ok(vec![folder("popularity"), folder("category")]),
            ["popularity"] => Ok(vec![folder("popular"), folder("votes")]),
            ["popularity", "popular"] => self.list_popular().await,
            ["popularity", "votes"] => self.list_voted().await,
            ["category"] => Ok(vec![folder("country"), folder("language"), folder("tag")]),
            ["category", "country"] => self.list_country_folders().await,
            ["category", "country", code] => self.list_by_country(code).await,
            ["category", "language"] => self.list_language_folders().await,
            ["category", "language", name] => self.list_by_language(name).await,
            ["category", "tag"] => self.list_tag_folders().await,
            ["category", "tag", name] => self.list_by_tag(name).await,
            _ => Ok(vec![]),
        }
    }
}

fn folder(item_id: &str) -> MediaItem {
    use ma_providers::media::Album as BrowseFolder;
    MediaItem::Album(BrowseFolder {
        item_id: ma_core::identifiers::MediaItemId(format!("folder:{item_id}")),
        provider: "radiobrowser".into(),
        name: item_id.to_string(),
        ..Default::default()
    })
}

impl ProviderImpl {
    async fn list_popular(&self) -> Result<Vec<MediaItem>> {
        if let Some(cached) = self
            .inner
            .cached_popular
            .get(std::time::Duration::from_secs(6 * 3600))
        {
            return Ok(cached.into_iter().map(MediaItem::Radio).collect());
        }
        let stations = self
            .inner
            .client
            .stations(Order::ClickCount, true, 1000, true)
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let radios: Vec<MediaRadio> = stations
            .into_iter()
            .map(|s| self.inner.parse_radio(s))
            .collect();
        self.inner.cached_popular.put(radios.clone());
        Ok(radios.into_iter().map(MediaItem::Radio).collect())
    }

    async fn list_voted(&self) -> Result<Vec<MediaItem>> {
        if let Some(cached) = self
            .inner
            .cached_voted
            .get(std::time::Duration::from_secs(6 * 3600))
        {
            return Ok(cached.into_iter().map(MediaItem::Radio).collect());
        }
        let stations = self
            .inner
            .client
            .stations(Order::Votes, true, 1000, true)
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let radios: Vec<MediaRadio> = stations
            .into_iter()
            .map(|s| self.inner.parse_radio(s))
            .collect();
        self.inner.cached_voted.put(radios.clone());
        Ok(radios.into_iter().map(MediaItem::Radio).collect())
    }

    async fn list_country_folders(&self) -> Result<Vec<MediaItem>> {
        if let Some(cached) = self
            .inner
            .cached_countries
            .get(std::time::Duration::from_secs(7 * 24 * 3600))
        {
            return Ok(cached
                .into_iter()
                .map(|c| {
                    let mut f = folder(&c.code);
                    if let MediaItem::Album(ref mut a) = f {
                        a.name = c.name;
                    }
                    f
                })
                .collect());
        }
        let countries = self
            .inner
            .client
            .countries()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let folders: Vec<CountryFolder> = countries
            .into_iter()
            .map(|c| CountryFolder {
                code: c.iso_3166_1.to_lowercase(),
                name: c.name,
                favicon: None,
            })
            .collect();
        self.inner.cached_countries.put(folders.clone());
        Ok(folders
            .into_iter()
            .map(|c| {
                let mut f = folder(&c.code);
                if let MediaItem::Album(ref mut a) = f {
                    a.name = c.name;
                }
                f
            })
            .collect())
    }

    async fn list_by_country(&self, code: &str) -> Result<Vec<MediaItem>> {
        let url = format!(
            "{}/json/stations/bycountrycodeexact/{}",
            self.inner.client.base_url_for_test(),
            code.to_uppercase()
        );
        let stations: Vec<Station> = self
            .inner
            .client
            .http
            .get(&url)
            .query(&[("hidebroken", "true"), ("limit", "1000")])
            .send()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?
            .json()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        Ok(stations
            .into_iter()
            .map(|s| self.inner.parse_radio(s))
            .map(MediaItem::Radio)
            .collect())
    }

    async fn list_language_folders(&self) -> Result<Vec<MediaItem>> {
        if let Some(cached) = self
            .inner
            .cached_languages
            .get(std::time::Duration::from_secs(7 * 24 * 3600))
        {
            return Ok(cached.into_iter().map(|l| folder(&l.0)).collect());
        }
        let langs = self
            .inner
            .client
            .languages()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let folders: Vec<LanguageFolder> =
            langs.into_iter().map(|l| LanguageFolder(l.name)).collect();
        self.inner.cached_languages.put(folders.clone());
        Ok(folders.into_iter().map(|l| folder(&l.0)).collect())
    }

    async fn list_by_language(&self, name: &str) -> Result<Vec<MediaItem>> {
        let url = format!(
            "{}/json/stations/bylanguageexact/{}",
            self.inner.client.base_url_for_test(),
            urlencoding::encode(name)
        );
        let stations: Vec<Station> = self
            .inner
            .client
            .http
            .get(&url)
            .query(&[("hidebroken", "true"), ("limit", "1000")])
            .send()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?
            .json()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        Ok(stations
            .into_iter()
            .map(|s| self.inner.parse_radio(s))
            .map(MediaItem::Radio)
            .collect())
    }

    async fn list_tag_folders(&self) -> Result<Vec<MediaItem>> {
        if let Some(cached) = self
            .inner
            .cached_tags
            .get(std::time::Duration::from_secs(7 * 24 * 3600))
        {
            return Ok(cached.into_iter().map(|t| folder(&t.0)).collect());
        }
        let tags = self
            .inner
            .client
            .tags()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let mut sorted: Vec<TagFolder> = tags.into_iter().map(|t| TagFolder(t.name)).collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        self.inner.cached_tags.put(sorted.clone());
        Ok(sorted.into_iter().map(|t| folder(&t.0)).collect())
    }

    async fn list_by_tag(&self, name: &str) -> Result<Vec<MediaItem>> {
        let url = format!(
            "{}/json/stations/bytagexact/{}",
            self.inner.client.base_url_for_test(),
            urlencoding::encode(name)
        );
        let stations: Vec<Station> = self
            .inner
            .client
            .http
            .get(&url)
            .query(&[("hidebroken", "true"), ("limit", "1000")])
            .send()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?
            .json()
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        Ok(stations
            .into_iter()
            .map(|s| self.inner.parse_radio(s))
            .map(MediaItem::Radio)
            .collect())
    }
}

/// Backwards-compatible helper on the client for tests (the
/// `browse` paths need to know the base URL).
impl RadioBrowserClient {
    pub fn base_url_for_test(&self) -> &str {
        &self.base_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Station;

    #[test]
    fn parse_radio_extracts_fields() {
        let provider = RadioBrowserProvider::with_client(RadioBrowserClient::new().unwrap());
        let station = Station {
            stationuuid: "uuid-1".into(),
            name: "Test FM".into(),
            url: "http://x".into(),
            url_resolved: "http://x/stream".into(),
            homepage: "http://x".into(),
            favicon: "http://x/fav".into(),
            clickcount: 42,
            ..Default::default()
        };
        let r = provider.parse_radio(station);
        assert_eq!(r.name, "Test FM");
        assert_eq!(r.click_count, Some(42));
        assert_eq!(r.homepage, Some("http://x".into()));
        assert_eq!(r.favicon_url, Some("http://x/fav".into()));
    }

    #[tokio::test]
    async fn into_handle_preserves_instance_id() {
        let provider = RadioBrowserProvider::new().unwrap();
        let h = Arc::clone(&provider).into_handle("rb1".into());
        assert_eq!(h.instance_id, "rb1");
        assert_eq!(h.manifest.domain, "radiobrowser");
    }
}
