//! `S3Provider` — `MusicProvider` impl backed by an S3-compatible bucket.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use ma_core::enums::MediaType;
use ma_core::identifiers::MediaItemId;
use ma_provider_filesystem::parser::ParsedTrack;
use ma_providers::media::{Album, MediaItem, SearchResults, Track};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, StreamProvider};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use ma_providers::provider::Result as ProviderResult;
use std::result::Result as StdResult;

use crate::manifest::s3_manifest;
use crate::stream::S3Streamer;
use crate::tag_reader::read_remote_tags;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "ogg", "oga", "opus", "wav", "aac", "aif", "aiff",
];

fn is_audio_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    AUDIO_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

/// User-facing S3 provider configuration. Mirrors `ma_cache_s3::S3Config`
/// but adds a `library_prefix` (the prefix under which music files are
/// stored; the cache prefix from the S3 config can be a strict superset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub key_prefix: String,
    #[serde(default)]
    pub library_prefix: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default = "default_path_style")]
    pub path_style: bool,
    #[serde(default)]
    pub publish_host: Option<String>,
    #[serde(default = "default_presign_ttl")]
    pub presign_ttl_secs: u32,
}

fn default_path_style() -> bool {
    false
}

fn default_presign_ttl() -> u32 {
    86_400
}

impl S3Config {
    /// Build a config from the `MA_S3_*` envvars. Returns `None` if the
    /// `MA_S3_BUCKET` variable is not set.
    pub fn from_env() -> Option<Self> {
        let bucket = std::env::var("MA_S3_BUCKET").ok()?;
        Some(Self {
            endpoint: std::env::var("MA_S3_ENDPOINT")
                .unwrap_or_else(|_| "https://s3.amazonaws.com".into()),
            region: std::env::var("MA_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
            bucket,
            key_prefix: std::env::var("MA_S3_KEY_PREFIX").unwrap_or_default(),
            library_prefix: std::env::var("MA_S3_LIBRARY_PREFIX").unwrap_or_default(),
            access_key_id: std::env::var("MA_S3_ACCESS_KEY_ID").unwrap_or_default(),
            secret_access_key: std::env::var("MA_S3_SECRET_ACCESS_KEY").unwrap_or_default(),
            session_token: std::env::var("MA_S3_SESSION_TOKEN").ok(),
            path_style: std::env::var("MA_S3_PATH_STYLE")
                .ok()
                .and_then(|s| match s.as_str() {
                    "1" | "true" | "yes" => Some(true),
                    "0" | "false" | "no" => Some(false),
                    _ => None,
                })
                .unwrap_or(false),
            publish_host: std::env::var("MA_S3_PUBLISH_HOST").ok(),
            presign_ttl_secs: std::env::var("MA_S3_PRESIGN_TTL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(86_400),
        })
    }
}

/// In-memory list of music objects discovered in the bucket.
#[derive(Debug, Default, Clone)]
pub struct S3ScanResult {
    /// Absolute S3 keys for every audio file under `library_prefix`.
    /// Capped at [`S3_SCAN_MAX_KEYS`] entries to keep memory bounded
    /// for buckets with millions of objects.
    pub tracks: Vec<String>,
}

/// Hard cap on the number of tracks kept in the S3 scan result.
/// At ~80 bytes per key, 200k entries is ~16 MiB which is a sensible
/// upper bound for a home music library.
pub const S3_SCAN_MAX_KEYS: usize = 200_000;

pub struct S3Provider {
    pub config: S3Config,
    pub(crate) streamer: Arc<S3Streamer>,
    scan: RwLock<Option<S3ScanResult>>,
    parsed_cache: RwLock<std::collections::HashMap<String, Arc<ParsedTrack>>>,
    http: Client,
}

/// Maximum number of `ParsedTrack` entries held in the S3 tag cache.
/// Each entry is typically a few KiB (an `Arc<ParsedTrack>` plus the
/// embedded `Track` and optional `Album`); a 4096-entry cap keeps
/// the cache well under 50 MiB even in the worst case.
pub const S3_TAG_CACHE_MAX_ENTRIES: usize = 4_096;

impl std::fmt::Debug for S3Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Provider")
            .field("bucket", &self.config.bucket)
            .field("library_prefix", &self.config.library_prefix)
            .finish()
    }
}

impl S3Provider {
    pub fn new(_instance_id: impl Into<String>, config: S3Config) -> StdResult<Arc<Self>, String> {
        let http = Client::builder()
            .user_agent(concat!("MusicAssistantRust/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let store = crate::build_store(&config).map_err(|e| e.to_string())?;
        let streamer = S3Streamer::new(store, config.bucket.clone(), config.key_prefix.clone());
        Ok(Arc::new(Self {
            config,
            streamer,
            scan: RwLock::new(None),
            parsed_cache: RwLock::new(std::collections::HashMap::new()),
            http,
        }))
    }

    /// Wrap into a `ProviderHandle` ready for registration.
    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        let inner = Arc::clone(&self);
        let handle = ProviderHandle::new(ProviderImpl {
            inner,
            instance_id: instance_id.clone(),
            domain: instance_id,
        });
        // The S3Streamer carries the bucket + key prefix; we expose it
        // as a separate `StreamProvider` so the player controller can
        // stream bytes without re-issuing a listing request.
        let streamer: Arc<dyn StreamProvider> = self.streamer.clone();
        handle.with_stream_dyn(streamer)
    }

    /// Trigger a scan of the bucket. Walks the configured prefix using
    /// the S3 List Objects v2 API and caches the result in-process.
    pub async fn scan(&self) -> StdResult<S3ScanResult, S3Error> {
        if let Some(cached) = self.scan.read().clone() {
            return Ok(cached);
        }
        let prefix_full = self.full_library_prefix();
        let mut tracks: Vec<String> = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            if tracks.len() >= S3_SCAN_MAX_KEYS {
                // We've hit the cap; record it once so the operator
                // can re-shard the bucket if needed.
                tracing::warn!(
                    cap = S3_SCAN_MAX_KEYS,
                    "s3 scan truncated; consider sharding the library across multiple prefixes"
                );
                break;
            }
            let page = self
                .list_page(&prefix_full, continuation.as_deref())
                .await?;
            for key in page.keys {
                if is_audio_key(&key) && tracks.len() < S3_SCAN_MAX_KEYS {
                    tracks.push(key);
                }
            }
            match page.next_continuation_token {
                Some(t) => continuation = Some(t),
                None => break,
            }
        }
        tracks.sort();
        let result = S3ScanResult { tracks };
        *self.scan.write() = Some(result.clone());
        Ok(result)
    }

    fn full_library_prefix(&self) -> String {
        let base = self.config.key_prefix.trim_end_matches('/');
        let lib = self.config.library_prefix.trim_matches('/');
        match (base.is_empty(), lib.is_empty()) {
            (true, true) => String::new(),
            (true, false) => lib.to_string(),
            (false, true) => base.to_string(),
            (false, false) => format!("{}/{}", base, lib),
        }
    }

    pub fn object_url(&self, key: &str) -> String {
        let key_prefix = if self.config.key_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.config.key_prefix.trim_end_matches('/'))
        };
        if self.config.path_style {
            format!(
                "{}/{}/{}{}",
                self.config.endpoint.trim_end_matches('/'),
                self.config.bucket,
                key_prefix,
                key
            )
        } else {
            let parsed = url::Url::parse(&self.config.endpoint).ok();
            let (scheme, host) = parsed
                .map(|u| {
                    (
                        u.scheme().to_string(),
                        u.host_str().unwrap_or("").to_string(),
                    )
                })
                .unwrap_or_else(|| ("https".to_string(), self.config.endpoint.clone()));
            format!(
                "{}://{}.{}/{}{}",
                scheme, self.config.bucket, host, key_prefix, key
            )
        }
    }

    async fn list_page(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> StdResult<ListPage, S3Error> {
        let url = if self.config.path_style {
            format!(
                "{}/{}/?list-type=2&prefix={}&max-keys=1000{}",
                self.config.endpoint.trim_end_matches('/'),
                self.config.bucket,
                urlencoding::encode(prefix),
                continuation
                    .map(|t| format!("&continuation-token={}", urlencoding::encode(t)))
                    .unwrap_or_default()
            )
        } else {
            let parsed = url::Url::parse(&self.config.endpoint)?;
            let host = parsed.host_str().unwrap_or("");
            format!(
                "{}://{}.{}/?list-type=2&prefix={}&max-keys=1000{}",
                parsed.scheme(),
                self.config.bucket,
                host,
                urlencoding::encode(prefix),
                continuation
                    .map(|t| format!("&continuation-token={}", urlencoding::encode(t)))
                    .unwrap_or_default()
            )
        };
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(S3Error::ListFailed(format!(
                "list status {}",
                resp.status()
            )));
        }
        let body = resp.text().await?;
        let page = parse_list_objects(&body, prefix);
        Ok(page)
    }

    pub async fn parse(&self, key: &str) -> Option<Arc<ParsedTrack>> {
        if let Some(hit) = self.parsed_cache.read().get(key).cloned() {
            return Some(hit);
        }
        let url = self.object_url(key);
        let parsed = read_remote_tags(&self.http, &url, key, None)
            .await
            .ok()
            .flatten()?;
        let arc = Arc::new(parsed);
        let mut cache = self.parsed_cache.write();
        // Bound the cache: when the cap is hit, drop a random quarter
        // of the entries. Random eviction is good enough for a
        // tag-cache: hits aren't a hard requirement, and avoiding a
        // full LRU keeps the code path O(1).
        if cache.len() >= S3_TAG_CACHE_MAX_ENTRIES {
            let drop_keys: Vec<String> = cache
                .keys()
                .take(S3_TAG_CACHE_MAX_ENTRIES / 4)
                .cloned()
                .collect();
            for k in drop_keys {
                cache.remove(&k);
            }
        }
        cache.insert(key.to_string(), Arc::clone(&arc));
        Some(arc)
    }
}

#[derive(Debug, Default)]
struct ListPage {
    keys: Vec<String>,
    next_continuation_token: Option<String>,
}

fn parse_list_objects(xml: &str, prefix: &str) -> ListPage {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Reuse a single buffer for the outer loop and another one for
    // the nested text reads so we don't allocate per element. The XML
    // response can be tens of MB for a busy bucket, so saving the
    // per-element alloc is non-trivial.
    let mut buf = Vec::with_capacity(4 * 1024);
    let mut text_buf = Vec::with_capacity(256);
    let mut page = ListPage::default();
    let mut current_key: Option<String> = None;
    let mut current_token: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"Key" => {
                    text_buf.clear();
                    if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf) {
                        current_key = Some(
                            t.unescape()
                                .ok()
                                .map(|s| s.into_owned())
                                .unwrap_or_default(),
                        );
                    }
                }
                b"NextContinuationToken" => {
                    text_buf.clear();
                    if let Ok(Event::Text(t)) = reader.read_event_into(&mut text_buf) {
                        current_token = Some(
                            t.unescape()
                                .ok()
                                .map(|s| s.into_owned())
                                .unwrap_or_default(),
                        );
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"Key" => {
                    if let Some(k) = current_key.take() {
                        if k.starts_with(prefix) || prefix.is_empty() {
                            page.keys.push(k);
                        }
                    }
                }
                b"NextContinuationToken" => {
                    page.next_continuation_token = current_token.take();
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    page
}

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("url: {0}")]
    Url(#[from] url::ParseError),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("list failed: {0}")]
    ListFailed(String),
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("backend error: {0}")]
    Backend(String),
}

struct ProviderImpl {
    inner: Arc<S3Provider>,
    instance_id: String,
    domain: String,
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
        MANIFEST.get_or_init(s3_manifest)
    }

    async fn search(
        &self,
        query: &str,
        _media_types: &[MediaType],
        limit: u32,
    ) -> ProviderResult<SearchResults> {
        let scan = self
            .inner
            .scan()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let limit = limit as usize;
        let lc = query.to_ascii_lowercase();
        let mut out = SearchResults::default();
        for key in scan.tracks.iter() {
            let Some(parsed) = self.inner.parse(key).await else {
                continue;
            };
            if !lc.is_empty()
                && !parsed.track.name.to_ascii_lowercase().contains(&lc)
                && !parsed
                    .track
                    .artists
                    .iter()
                    .any(|a| a.name.to_ascii_lowercase().contains(&lc))
            {
                continue;
            }
            out.tracks.push(parsed.track.clone());
            if out.tracks.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> ProviderResult<MediaItem> {
        if media_type != MediaType::Track {
            return Err(ProviderError::Unsupported("non-track lookup"));
        }
        let key = item_id.split_once('|').map(|(_, k)| k).unwrap_or(item_id);
        let parsed = self
            .inner
            .parse(key)
            .await
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        Ok(MediaItem::Track(parsed.track.clone()))
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        _media_type: MediaType,
    ) -> ProviderResult<StreamDetails> {
        let key = item_id.split_once('|').map(|(_, k)| k).unwrap_or(item_id);
        let parsed = self
            .inner
            .parse(key)
            .await
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        Ok(StreamDetails {
            provider: self.domain.clone(),
            item_id: parsed.track.item_id.clone(),
            media_type: MediaType::Track,
            stream_type: ma_core::enums::StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type: parsed.content_type,
                sample_rate: parsed.sample_rate,
                bit_depth: parsed.bit_depth,
                channels: parsed.channels,
                bit_rate: parsed.bit_rate,
            }),
            path: self.inner.object_url(key),
            parts: Vec::new(),
            duration: parsed.track.duration,
            can_seek: true,
            live: false,
            title: Some(parsed.track.name.clone()),
            artist: parsed.track.artists.first().map(|a| a.name.clone()),
            album: parsed.track.album.as_ref().map(|a| a.name.clone()),
        })
    }

    async fn browse(&self, _path: &str) -> ProviderResult<Vec<MediaItem>> {
        let scan = self
            .inner
            .scan()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let mut by_dir: BTreeMap<String, Vec<Track>> = BTreeMap::new();
        for key in scan.tracks {
            let dir = key
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
            if let Some(parsed) = self.inner.parse(&key).await {
                by_dir.entry(dir).or_default().push(parsed.track.clone());
            }
        }
        let mut out = Vec::new();
        for (dir, tracks) in by_dir {
            if tracks.is_empty() {
                continue;
            }
            let name = dir
                .rsplit_once('/')
                .map(|(_, n)| n)
                .unwrap_or(&dir)
                .to_string();
            let album = Album {
                item_id: MediaItemId(format!("s3://{}", dir)),
                provider: self.domain.clone(),
                name,
                artists: tracks
                    .first()
                    .and_then(|t| t.artists.first().cloned())
                    .into_iter()
                    .collect(),
                image_url: None,
                ..Default::default()
            };
            out.push(MediaItem::Album(album));
        }
        Ok(out)
    }
}

#[async_trait]
impl StreamProvider for S3Provider {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        seek_position: u32,
    ) -> ProviderResult<bytes::Bytes> {
        self.streamer.get_stream_bytes(details, seek_position).await
    }
}

trait ProviderHandleStreamExt {
    fn with_stream_dyn(self, stream: Arc<dyn StreamProvider>) -> Self;
}

impl ProviderHandleStreamExt for ProviderHandle {
    fn with_stream_dyn(mut self, stream: Arc<dyn StreamProvider>) -> Self {
        self.stream = Some(stream);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_audio_key_matches_known_extensions() {
        assert!(is_audio_key("a/b/c/song.flac"));
        assert!(is_audio_key("track.MP3"));
        assert!(is_audio_key("a.opus"));
        assert!(!is_audio_key("cover.jpg"));
        assert!(!is_audio_key("notes.txt"));
    }

    #[test]
    fn full_library_prefix_handles_combinations() {
        let p = S3Provider {
            config: S3Config {
                endpoint: "x".into(),
                region: "x".into(),
                bucket: "x".into(),
                key_prefix: "music/".into(),
                library_prefix: "library/".into(),
                access_key_id: String::new(),
                secret_access_key: String::new(),
                session_token: None,
                path_style: true,
                publish_host: None,
                presign_ttl_secs: 60,
            },
            streamer: Arc::new(S3Streamer::dummy()),
            scan: RwLock::new(None),
            parsed_cache: RwLock::new(Default::default()),
            http: Client::new(),
        };
        assert_eq!(p.full_library_prefix(), "music/library");
    }

    #[test]
    fn list_page_parses_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>ma</Name>
  <Prefix>music/</Prefix>
  <IsTruncated>false</IsTruncated>
  <Contents><Key>music/a.flac</Key></Contents>
  <Contents><Key>music/b.mp3</Key></Contents>
</ListBucketResult>"#;
        let page = parse_list_objects(xml, "music/");
        assert_eq!(page.keys, vec!["music/a.flac", "music/b.mp3"]);
    }

    #[test]
    fn list_page_captures_continuation_token() {
        let xml = r#"<?xml version="1.0"?>
<ListBucketResult>
  <IsTruncated>true</IsTruncated>
  <Contents><Key>x.flac</Key></Contents>
  <NextContinuationToken>abc</NextContinuationToken>
</ListBucketResult>"#;
        let page = parse_list_objects(xml, "");
        assert_eq!(page.next_continuation_token.as_deref(), Some("abc"));
    }

    #[test]
    fn object_url_path_style() {
        let p = S3Provider {
            config: S3Config {
                endpoint: "https://s3.example.com".into(),
                region: "us-east-1".into(),
                bucket: "ma".into(),
                key_prefix: "music/".into(),
                library_prefix: String::new(),
                access_key_id: String::new(),
                secret_access_key: String::new(),
                session_token: None,
                path_style: true,
                publish_host: None,
                presign_ttl_secs: 60,
            },
            streamer: Arc::new(S3Streamer::dummy()),
            scan: RwLock::new(None),
            parsed_cache: RwLock::new(Default::default()),
            http: Client::new(),
        };
        assert_eq!(
            p.object_url("a.flac"),
            "https://s3.example.com/ma/music/a.flac"
        );
    }
}
