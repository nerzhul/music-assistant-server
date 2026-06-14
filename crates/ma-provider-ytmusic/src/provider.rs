//! `YTMusicProvider` — the `MusicProvider` impl that wraps `yt-dlp`.
//!
//! The provider holds a [`Ytdlp`] handle and a per-instance config.
//! The `MusicProvider` methods are best-effort: `search` returns an
//! empty `SearchResults` (V1 limitation), `get_item` accepts a
//! `video_id` or full YouTube Music URL, and `get_stream_details`
//! spawns a `yt-dlp -g` call to resolve the direct media URL.
//!
//! The provider does NOT implement `StreamProvider`: the stream
//! controller will fetch the URL itself (via ffmpeg) so we don't
//! pipe the audio through Rust.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use ma_core::enums::{ContentType, MediaType, StreamType};
use ma_providers::media::{MediaItem, SearchResults};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::manifest::ytmusic_manifest;
use crate::parser::{best_audio_format, best_thumbnail, playlist_from_info, track_from_video};
use crate::yt_dlp::{Ytdlp, YtdlpError, YtdlpOptions};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YTMusicConfig {
    /// Override path to the `yt-dlp` binary. If `None`, we look on
    /// `$PATH`. Mirrors `MA_SPOTIFY_LIBRESPOT_PATH`.
    #[serde(default)]
    pub ytdlp_path: Option<PathBuf>,
    /// Path to a Netscape-format `cookies.txt` exported from the
    /// user's browser. Required for age-restricted / member-only
    /// content. Mirrors `MA_YTMUSIC_COOKIE_FILE`.
    #[serde(default)]
    pub cookie_file: Option<PathBuf>,
    /// Optional PO token (visitor data blob).
    #[serde(default)]
    pub po_token: Option<String>,
}

impl Default for YTMusicConfig {
    fn default() -> Self {
        Self {
            ytdlp_path: None,
            cookie_file: std::env::var("MA_YTMUSIC_COOKIE_FILE")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            po_token: std::env::var("MA_YTMUSIC_PO_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }
}

pub struct YTMusicProvider {
    pub config: YTMusicConfig,
    /// `None` if `yt-dlp` is not installed (we still register the
    /// provider so the user gets a clear error from `get_item` /
    /// `get_stream_details` rather than a panic).
    pub ytdlp: RwLock<Option<Arc<Ytdlp>>>,
}

impl YTMusicProvider {
    pub async fn new(
        _instance_id: String,
        config: YTMusicConfig,
    ) -> std::result::Result<Arc<Self>, YtdlpError> {
        let mut opts = YtdlpOptions {
            binary: config.ytdlp_path.clone(),
            cookie_file: config.cookie_file.clone(),
            po_token: config.po_token.clone(),
            ..YtdlpOptions::from_env()
        };
        if opts.cookie_file.is_none() {
            opts.cookie_file = config.cookie_file.clone();
        }
        if opts.binary.is_none() {
            opts.binary = config.ytdlp_path.clone();
        }
        let ytdlp = Ytdlp::new(opts).await.ok().map(Arc::new);
        Ok(Arc::new(Self {
            config,
            ytdlp: RwLock::new(ytdlp),
        }))
    }

    pub fn with_ytdlp(_instance_id: String, config: YTMusicConfig, ytdlp: Ytdlp) -> Arc<Self> {
        Arc::new(Self {
            config,
            ytdlp: RwLock::new(Some(Arc::new(ytdlp))),
        })
    }

    /// Build a `ProviderHandle`. The `instance_id` is used as both
    /// `domain` and `instance_id` (matches the Python convention
    /// where the provider is single-instance by default).
    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        ProviderHandle::new(YTMusicProviderImpl {
            inner: self,
            instance_id: instance_id.clone(),
            domain: instance_id,
        })
    }

    fn require_ytdlp(&self) -> Result<Arc<Ytdlp>> {
        self.ytdlp.read().clone().ok_or_else(|| {
            ProviderError::Unavailable(
                "yt-dlp is not installed or MA_YTMUSIC_YT_DLP_PATH is not set".to_string(),
            )
        })
    }
}

struct YTMusicProviderImpl {
    inner: Arc<YTMusicProvider>,
    instance_id: String,
    domain: String,
}

fn url_from_id(item_id: &str) -> String {
    if item_id.starts_with("http://") || item_id.starts_with("https://") {
        item_id.to_string()
    } else {
        format!("https://music.youtube.com/watch?v={item_id}")
    }
}

fn mime_for_format(format_id: &str, ext: &str) -> ContentType {
    let ext_lc = ext.to_ascii_lowercase();
    match ext_lc.as_str() {
        "mp3" => ContentType::Mp3,
        "m4a" | "mp4" | "aac" => ContentType::Aac,
        "ogg" | "vorbis" => ContentType::Vorbis,
        "opus" | "webm" => ContentType::Opus,
        "flac" => ContentType::Flac,
        "wav" => ContentType::Wav,
        _ => {
            if format_id.contains("mp3") {
                ContentType::Mp3
            } else if format_id.contains("opus") || format_id.contains("251") {
                ContentType::Opus
            } else {
                ContentType::Unknown
            }
        }
    }
}

#[async_trait]
impl MusicProvider for YTMusicProviderImpl {
    fn domain(&self) -> &str {
        &self.domain
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(ytmusic_manifest)
    }

    async fn search(
        &self,
        _query: &str,
        _media_types: &[MediaType],
        _limit: u32,
    ) -> Result<SearchResults> {
        // V1 limitation: InnerTube search is not implemented. The
        // plan calls for a V2 InnerTube client. We log a warning and
        // return an empty result set so callers degrade gracefully.
        warn!("YT Music search is not implemented in V1; install ytmusicapi or wait for V2");
        Ok(SearchResults::default())
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        let ytdlp = self.inner.require_ytdlp()?;
        let url = url_from_id(item_id);
        let info = ytdlp
            .extract_info(&url)
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        if let Some(entries) = info.entries.as_ref() {
            if !entries.is_empty() {
                // Treat as a playlist: surface the entries via
                // `get_item(playlist_id, MediaType::Playlist)`.
                if media_type == MediaType::Playlist {
                    let p = playlist_from_info(&info, &self.instance_id, &self.domain)
                        .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
                    return Ok(MediaItem::Playlist(p));
                }
            }
        }
        match media_type {
            MediaType::Track | MediaType::PodcastEpisode => {
                let t = track_from_video(&info, &self.instance_id, &self.domain)
                    .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
                Ok(MediaItem::Track(t))
            }
            MediaType::Playlist => {
                let p = playlist_from_info(&info, &self.instance_id, &self.domain)
                    .ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))?;
                Ok(MediaItem::Playlist(p))
            }
            _ => Err(ProviderError::Unsupported("ytmusic: non-track lookup")),
        }
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        _media_type: MediaType,
    ) -> Result<StreamDetails> {
        let ytdlp = self.inner.require_ytdlp()?;
        let url = url_from_id(item_id);
        // `extract_info` is the slow path but gives us the best
        // format selection. We use the audio-only format picked by
        // the parser to fill `content_type`, then `extract_url` to
        // resolve the direct media URL.
        let info = ytdlp
            .extract_info(&url)
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let direct_url = ytdlp
            .extract_url(&url)
            .await
            .map_err(|e| ProviderError::Unavailable(e.to_string()))?;
        let formats = info.formats.as_deref().unwrap_or(&[]);
        let chosen = best_audio_format(formats);
        let (content_type, bit_rate) = match chosen {
            Some(f) => (
                mime_for_format(&f.format_id, &f.ext),
                f.abr.map(|x| x as u32),
            ),
            None => (ContentType::Unknown, None),
        };
        debug!(item_id, "ytmusic stream resolved");
        Ok(StreamDetails {
            provider: self.domain.clone(),
            item_id: ma_core::identifiers::MediaItemId(item_id.to_string()),
            media_type: MediaType::Track,
            stream_type: StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type,
                sample_rate: chosen.and_then(|f| f.asr).unwrap_or(0),
                bit_depth: 0,
                channels: 2,
                bit_rate,
            }),
            path: direct_url,
            parts: Vec::new(),
            duration: info.duration,
            can_seek: true,
            live: false,
            title: Some(info.title.clone()),
            artist: Some(info.uploader.clone()),
            album: None,
        })
    }

    async fn browse(&self, _path: &str) -> Result<Vec<MediaItem>> {
        Ok(Vec::new())
    }
}

/// Helper to retrieve the best thumbnail URL of a `VideoInfo`. We
/// expose this for callers that already have a `VideoInfo` (e.g. the
/// library controller walking a playlist) and don't want to re-run
/// the full parser.
pub fn video_thumbnail(info: &crate::yt_dlp::VideoInfo) -> Option<String> {
    best_thumbnail(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yt_dlp::{FormatInfo, VideoInfo, YtdlpThumbnail};

    #[test]
    fn url_from_id_passthrough() {
        assert_eq!(
            url_from_id("https://music.youtube.com/watch?v=abc"),
            "https://music.youtube.com/watch?v=abc"
        );
    }

    #[test]
    fn url_from_id_constructs_watch_url() {
        assert_eq!(
            url_from_id("dQw4w9WgXcQ"),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    #[test]
    fn mime_for_format_dispatch() {
        assert_eq!(mime_for_format("140", "m4a"), ContentType::Aac);
        assert_eq!(mime_for_format("251", "webm"), ContentType::Opus);
        assert_eq!(mime_for_format("xyz-mp3-128", "bin"), ContentType::Mp3);
    }

    #[tokio::test]
    async fn search_returns_empty() {
        // We need a handle; the provider is best tested via the
        // smoke-test binary. The `search` short-circuit is exercised
        // here without spinning up yt-dlp.
        let p = YTMusicProvider::with_ytdlp(
            "ytmusic".into(),
            YTMusicConfig::default(),
            // No real Ytdlp needed for search; the impl doesn't use
            // it. We just need a placeholder. Use a fake binary.
            Ytdlp::with_binary(
                std::path::PathBuf::from("/nonexistent"),
                YtdlpOptions::default(),
            ),
        );
        let handle = p.into_handle("ytmusic".into());
        let results = handle
            .music
            .search("test", &[MediaType::Track], 10)
            .await
            .unwrap();
        assert!(results.tracks.is_empty());
    }

    #[test]
    fn stream_details_construction() {
        // Sanity: build a StreamDetails via the same code path the
        // provider uses, and confirm the fields round-trip.
        let info = VideoInfo {
            id: "id1".into(),
            title: "Title".into(),
            uploader: "Artist".into(),
            duration: Some(120.0),
            formats: Some(vec![FormatInfo {
                format_id: "140".into(),
                ext: "m4a".into(),
                url: "https://x/audio".into(),
                acodec: "aac".into(),
                vcodec: "none".into(),
                abr: Some(128.0),
                asr: Some(44100),
                ..Default::default()
            }]),
            thumbnails: vec![YtdlpThumbnail {
                url: "https://x/120.jpg".into(),
                width: Some(120),
                height: Some(90),
            }],
            ..Default::default()
        };
        let chosen = best_audio_format(info.formats.as_deref().unwrap()).unwrap();
        let details = StreamDetails {
            provider: "ytmusic".into(),
            item_id: ma_core::identifiers::MediaItemId("id1".into()),
            media_type: MediaType::Track,
            stream_type: StreamType::Http,
            audio_format: Some(StreamAudioFormat {
                content_type: mime_for_format(&chosen.format_id, &chosen.ext),
                sample_rate: chosen.asr.unwrap_or(0),
                bit_depth: 0,
                channels: 2,
                bit_rate: chosen.abr.map(|x| x as u32),
            }),
            path: "https://x/audio".into(),
            parts: Vec::new(),
            duration: info.duration,
            can_seek: true,
            live: false,
            title: Some(info.title.clone()),
            artist: Some(info.uploader.clone()),
            album: None,
        };
        assert_eq!(details.path, "https://x/audio");
        assert_eq!(details.audio_format.unwrap().content_type, ContentType::Aac);
    }
}
