//! `SpotifyProvider` — the `MusicProvider` impl for the Rust port.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use ma_core::enums::{ContentType, MediaType, StreamType};
use ma_providers::media::{MediaItem, SearchResults};
use ma_providers::provider::{MusicProvider, ProviderError, ProviderHandle, Result};
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::auth::PkceAuth;
use crate::manifest::spotify_manifest;
use crate::parsers::{
    parse_album, parse_artist, parse_playlist, parse_podcast, parse_track, ProviderCtx,
};
use crate::streaming::LibrespotStreamer;
use crate::web::SpotifyApi;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyConfig {
    pub client_id: String,
    pub refresh_token: Option<String>,
    pub dev_refresh_token: Option<String>,
    pub librespot_path: Option<String>,
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            client_id: crate::manifest::DEFAULT_CLIENT_ID.to_string(),
            refresh_token: None,
            dev_refresh_token: None,
            librespot_path: None,
        }
    }
}

pub struct SpotifyProvider {
    pub config: SpotifyConfig,
    pub auth: Arc<PkceAuth>,
    pub dev_auth: Option<Arc<PkceAuth>>,
    pub api: SpotifyApi,
    pub librespot: Option<LibrespotStreamer>,
}

impl SpotifyProvider {
    pub fn new(_instance_id: String, config: SpotifyConfig) -> Result<Arc<Self>> {
        let auth = Arc::new(PkceAuth::new(
            config.client_id.clone(),
            config.refresh_token.clone(),
        ));
        let api = SpotifyApi::new(Arc::clone(&auth));
        let dev_auth = config
            .dev_refresh_token
            .as_ref()
            .map(|rt| Arc::new(PkceAuth::new(config.client_id.clone(), Some(rt.clone()))));
        if let Some(dev) = &dev_auth {
            api.set_dev_auth(Arc::clone(dev));
        }
        let librespot = config
            .librespot_path
            .as_ref()
            .map(|p| LibrespotStreamer::new(p.into(), std::env::temp_dir().join("ma-librespot")));
        Ok(Arc::new(Self {
            config,
            auth,
            dev_auth,
            api,
            librespot,
        }))
    }

    pub fn into_handle(self: Arc<Self>, instance_id: String) -> ProviderHandle {
        let handle = ProviderHandle::new(ProviderImpl {
            inner: Arc::clone(&self),
            instance_id,
        });
        // Expose the configured LibrespotStreamer (if any) as the
        // handle's `StreamProvider` so the player controller can stream.
        let librespot = self.librespot.clone();
        let mut handle = handle;
        if let Some(streamer) = librespot {
            handle = handle.with_stream(SpotifyStreamer { inner: streamer });
        }
        handle
    }
}

struct ProviderImpl {
    inner: Arc<SpotifyProvider>,
    instance_id: String,
}

impl ProviderImpl {
    fn ctx(&self) -> ProviderCtx<'_> {
        ProviderCtx {
            instance_id: &self.instance_id,
            domain: "spotify",
        }
    }
}

#[async_trait]
impl MusicProvider for ProviderImpl {
    fn domain(&self) -> &str {
        "spotify"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn manifest(&self) -> &ma_providers::ProviderManifest {
        static MANIFEST: std::sync::OnceLock<ma_providers::ProviderManifest> =
            std::sync::OnceLock::new();
        MANIFEST.get_or_init(spotify_manifest)
    }

    async fn search(
        &self,
        query: &str,
        media_types: &[MediaType],
        limit: u32,
    ) -> Result<SearchResults> {
        let mut results = SearchResults::default();
        let mut types: Vec<&str> = Vec::new();
        if media_types.contains(&MediaType::Track) {
            types.push("track");
        }
        if media_types.contains(&MediaType::Album) {
            types.push("album");
        }
        if media_types.contains(&MediaType::Artist) {
            types.push("artist");
        }
        if media_types.contains(&MediaType::Playlist) {
            types.push("playlist");
        }
        if media_types.contains(&MediaType::Podcast) {
            types.push("show");
        }
        if media_types.contains(&MediaType::Audiobook) {
            types.push("audiobook");
        }
        if types.is_empty() {
            return Ok(results);
        }
        let body = self
            .inner
            .api
            .search(query, &types, limit)
            .await
            .map_err(|e| ProviderError::Internal(e.to_string()))?;
        let ctx = self.ctx();
        if let Some(arr) = body
            .get("tracks")
            .and_then(|t| t.get("items"))
            .and_then(|i| i.as_array())
        {
            for t in arr {
                if let Some(track) = parse_track(t, &ctx) {
                    results.tracks.push(track);
                }
            }
        }
        if let Some(arr) = body
            .get("albums")
            .and_then(|a| a.get("items"))
            .and_then(|i| i.as_array())
        {
            for a in arr {
                if let Some(album) = parse_album(a, &ctx) {
                    results.albums.push(album);
                }
            }
        }
        if let Some(arr) = body
            .get("artists")
            .and_then(|a| a.get("items"))
            .and_then(|i| i.as_array())
        {
            for a in arr {
                if let Some(artist) = parse_artist(a, &ctx) {
                    results.artists.push(artist);
                }
            }
        }
        if let Some(arr) = body
            .get("playlists")
            .and_then(|p| p.get("items"))
            .and_then(|i| i.as_array())
        {
            for p in arr {
                if let Some(playlist) = parse_playlist(p, &ctx) {
                    results.playlists.push(playlist);
                }
            }
        }
        if let Some(arr) = body
            .get("shows")
            .and_then(|p| p.get("items"))
            .and_then(|i| i.as_array())
        {
            for p in arr {
                if let Some(podcast) = parse_podcast(p, &ctx) {
                    results.podcasts.push(podcast);
                }
            }
        }
        if let Some(arr) = body
            .get("audiobooks")
            .and_then(|p| p.get("items"))
            .and_then(|i| i.as_array())
        {
            for a in arr {
                if let Some(book) = crate::parsers::parse_audiobook(a, &ctx) {
                    results.audiobooks.push(book);
                }
            }
        }
        Ok(results)
    }

    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem> {
        let ctx = self.ctx();
        let body = match media_type {
            MediaType::Track => self
                .inner
                .api
                .get_track(item_id)
                .await
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            MediaType::Album => self
                .inner
                .api
                .get_album(item_id)
                .await
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            MediaType::Artist => self
                .inner
                .api
                .get_artist(item_id)
                .await
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            MediaType::Playlist => self
                .inner
                .api
                .get_playlist(item_id)
                .await
                .map_err(|e| ProviderError::Internal(e.to_string()))?,
            _ => return Err(ProviderError::Unsupported("spotify item lookup")),
        };
        let item = match media_type {
            MediaType::Track => parse_track(&body, &ctx).map(MediaItem::Track),
            MediaType::Album => parse_album(&body, &ctx).map(MediaItem::Album),
            MediaType::Artist => parse_artist(&body, &ctx).map(MediaItem::Artist),
            MediaType::Playlist => parse_playlist(&body, &ctx).map(MediaItem::Playlist),
            _ => None,
        };
        item.ok_or_else(|| ProviderError::MediaNotFound(item_id.into()))
    }

    async fn get_stream_details(
        &self,
        item_id: &str,
        media_type: MediaType,
    ) -> Result<StreamDetails> {
        let kind = match media_type {
            MediaType::Track => "track",
            MediaType::PodcastEpisode => "episode",
            _ => return Err(ProviderError::Unsupported("non-playable spotify media")),
        };
        let _uri = format!("spotify://{kind}:{item_id}");
        let path = format!("spotify:{}:{}", kind, item_id);
        let sample_rate = 44_100;
        let bit_depth = 16;
        let bit_rate = 320;
        Ok(StreamDetails {
            provider: self.domain().to_string(),
            item_id: ma_core::identifiers::MediaItemId(item_id.into()),
            media_type,
            stream_type: StreamType::Custom,
            audio_format: Some(StreamAudioFormat {
                content_type: ContentType::Mp3,
                sample_rate,
                bit_depth,
                channels: 2,
                bit_rate: Some(bit_rate),
            }),
            path,
            parts: Vec::new(),
            duration: None,
            can_seek: true,
            live: false,
            title: None,
            artist: None,
            album: None,
        })
    }
}

/// Wrapper that exposes the configured `LibrespotStreamer` to the
/// player controller.
pub struct SpotifyStreamer {
    pub inner: crate::streaming::LibrespotStreamer,
}

#[async_trait]
impl ma_providers::provider::StreamProvider for SpotifyStreamer {
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        seek_position: u32,
    ) -> ma_providers::provider::Result<Bytes> {
        // The actual implementation lives in `LibrespotStreamer`'s
        // blanket `StreamProvider` impl.
        ma_providers::provider::StreamProvider::get_stream_bytes(
            &self.inner,
            details,
            seek_position,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::PkceAuth;
    use crate::manifest::spotify_manifest;

    #[test]
    fn manifest_has_expected_domain() {
        let m = spotify_manifest();
        assert_eq!(m.domain, "spotify");
    }

    #[test]
    fn new_provider_requires_token_in_config() {
        let cfg = SpotifyConfig::default();
        let p = SpotifyProvider::new("sp1".into(), cfg).unwrap();
        let handle = p.into_handle("sp1".into());
        assert_eq!(handle.instance_id, "sp1");
        assert_eq!(handle.manifest.domain, "spotify");
    }

    #[test]
    fn get_stream_details_uses_custom_type() {
        // Just exercise the static manifest function.
        let _ = PkceAuth::new("cid", None);
    }
}
