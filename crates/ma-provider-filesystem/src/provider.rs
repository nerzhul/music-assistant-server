//! FilesystemLocalProvider — the `MusicProvider` impl.
//!
//! Wraps the scanner + parser + stream reader into a single object that
//! can be registered in a `ProviderRegistry`. The actual end-to-end
//! coverage lives in `tests/integration.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use ma_core::enums::MediaType;
use ma_providers::media::{MediaItem, SearchResults};
use ma_providers::provider::{
    MusicProvider, ProviderError, ProviderHandle, Result, StreamProvider,
};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::manifest::filesystem_local_manifest;
use crate::parser::{parse_track_file, ParsedTrack};
use crate::scanner::{ScanConfig, ScanError, ScanResult};
use crate::stream::read_stream;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    pub path: PathBuf,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

fn default_content_type() -> String {
    "music".into()
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/media"),
            content_type: default_content_type(),
        }
    }
}

pub struct FilesystemProvider {
    pub config: FilesystemConfig,
    /// Cached track records keyed by their absolute path. The
    /// `parser::parse_track_file` call is expensive (lofty is sync) so
    /// we memoise the result.
    cache: RwLock<std::collections::HashMap<String, Arc<ParsedTrack>>>,
}

impl FilesystemProvider {
    pub fn new(_instance_id: impl Into<String>, config: FilesystemConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            cache: RwLock::new(std::collections::HashMap::new()),
        })
    }

    /// Build a `ProviderHandle` ready for registration. The instance id
    /// doubles as the `provider` field on every track returned.
    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        let handle = ProviderHandle::new(ProviderImpl {
            inner: Arc::clone(&self),
            instance_id: instance_id.clone(),
            domain: instance_id,
        });
        // Expose a `StreamProvider` that reads from the local disk.
        handle.with_stream(FilesystemStreamer { inner: self })
    }

    /// Run a scan on the configured base path. Convenience wrapper.
    pub async fn scan(&self) -> std::result::Result<ScanResult, ScanError> {
        crate::scanner::scan(ScanConfig::new(self.config.path.clone())).await
    }

    /// Resolve a single file's tags, caching the result.
    pub fn parse(&self, abs_path: &str) -> Option<Arc<ParsedTrack>> {
        if let Some(hit) = self.cache.read().get(abs_path).cloned() {
            return Some(hit);
        }
        let parsed = parse_track_file(abs_path, &self.config.path.to_string_lossy())?;
        let arc = Arc::new(parsed);
        self.cache
            .write()
            .insert(abs_path.to_string(), Arc::clone(&arc));
        Some(arc)
    }
}

/// Wrapper that holds the per-instance id alongside the shared provider
/// state. `MusicProvider` is object-safe; the `instance_id()` method
/// returns the per-instance id, not the shared provider's id.
struct ProviderImpl {
    inner: Arc<FilesystemProvider>,
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
        MANIFEST.get_or_init(filesystem_local_manifest)
    }

    async fn search(
        &self,
        query: &str,
        media_types: &[MediaType],
        limit: u32,
    ) -> Result<SearchResults> {
        let _ = media_types;
        let limit = limit as usize;
        let scan = self
            .inner
            .scan()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let mut results = SearchResults::default();
        for f in scan.tracks.iter().take(limit * 4) {
            let Some(parsed) = self.inner.parse(&f.absolute_path.to_string_lossy()) else {
                continue;
            };
            let lc = query.to_ascii_lowercase();
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
            results.tracks.push(parsed.track.clone());
            if results.tracks.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        if media_type != MediaType::Track {
            return Err(ProviderError::Unsupported("non-track lookup"));
        }
        let path = item_id.split_once('|').map(|(_, p)| p).unwrap_or(item_id);
        let parsed = self
            .inner
            .parse(path)
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        Ok(MediaItem::Track(parsed.track.clone()))
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        _media_type: MediaType,
    ) -> Result<StreamDetails> {
        let path = item_id.split_once('|').map(|(_, p)| p).unwrap_or(item_id);
        let parsed = self
            .inner
            .parse(path)
            .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
        Ok(StreamDetails {
            provider: self.domain.clone(),
            item_id: parsed.track.item_id.clone(),
            media_type: MediaType::Track,
            stream_type: ma_core::enums::StreamType::LocalFile,
            audio_format: Some(StreamAudioFormat {
                content_type: parsed.content_type,
                sample_rate: parsed.sample_rate,
                bit_depth: parsed.bit_depth,
                channels: parsed.channels,
                bit_rate: parsed.bit_rate,
            }),
            path: path.to_string(),
            parts: Vec::new(),
            duration: parsed.track.duration,
            can_seek: true,
            live: false,
            title: Some(parsed.track.name.clone()),
            artist: parsed.track.artists.first().map(|a| a.name.clone()),
            album: parsed.track.album.as_ref().map(|a| a.name.clone()),
        })
    }

    async fn browse(&self, _path: &str) -> Result<Vec<MediaItem>> {
        let scan = self
            .inner
            .scan()
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        use std::collections::BTreeMap;
        let mut by_dir: BTreeMap<PathBuf, Vec<ma_providers::media::Track>> = BTreeMap::new();
        for f in scan.tracks {
            let dir = f
                .absolute_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            if let Some(parsed) = self.inner.parse(&f.absolute_path.to_string_lossy()) {
                by_dir.entry(dir).or_default().push(parsed.track.clone());
            }
        }
        let mut out = Vec::new();
        for (dir, tracks) in by_dir {
            if tracks.is_empty() {
                continue;
            }
            let name = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled")
                .to_string();
            let album = ma_providers::media::Album {
                item_id: ma_core::identifiers::MediaItemId(dir.to_string_lossy().to_string()),
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
impl StreamProvider for ProviderImpl {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        _seek_position: u32,
    ) -> Result<Bytes> {
        let mut s = read_stream(details)
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        use futures::StreamExt;
        match s.next().await {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(ProviderError::Internal(e.to_string())),
            None => Ok(Bytes::new()),
        }
    }
}

/// Standalone `StreamProvider` for the filesystem provider that doesn't
/// carry the per-instance `ProviderImpl` wrapper. Useful when the player
/// controller only knows the instance id.
pub struct FilesystemStreamer {
    pub inner: Arc<FilesystemProvider>,
}

#[async_trait]
impl StreamProvider for FilesystemStreamer {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        _seek_position: u32,
    ) -> Result<Bytes> {
        let mut s = read_stream(details)
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        use futures::StreamExt;
        match s.next().await {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(ProviderError::Internal(e.to_string())),
            None => Ok(Bytes::new()),
        }
    }
}
