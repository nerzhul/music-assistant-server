//! `Provider` trait and a `ProviderRegistry` for runtime lookup.
//!
//! Each provider crate implements `MusicProvider` (or a subset like
//! `MetadataProvider`) and registers itself with a global
//! `ProviderRegistry` during startup. The webserver / player controllers
//! look providers up by domain + instance id.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use thiserror::Error;

use ma_core::enums::MediaType;

use crate::manifest::ProviderManifest;
use crate::media::{MediaItem, SearchResults};
use crate::stream::StreamDetails;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider not found: {0}")]
    NotFound(String),
    #[error("provider already registered: {0}")]
    AlreadyRegistered(String),
    #[error("provider is unavailable: {0}")]
    Unavailable(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("media not found: {0}")]
    MediaNotFound(String),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

/// A music provider: browse, search, resolve to stream details. This is
/// the minimum set the player controller needs; metadata providers,
/// plugins, and audio-analysis providers add their own narrower traits.
#[async_trait]
pub trait MusicProvider: Send + Sync {
    /// Stable domain (e.g. `"spotify"`).
    fn domain(&self) -> &str;

    /// Instance id (e.g. `"spotify"`, or `"spotify_2"` for the second).
    fn instance_id(&self) -> &str;

    /// Static manifest.
    fn manifest(&self) -> &ProviderManifest;

    /// `search` across the provider's catalogue.
    async fn search(
        &self,
        query: &str,
        media_types: &[MediaType],
        limit: u32,
    ) -> Result<SearchResults>;

    /// Look up a single item by provider-specific id.
    async fn get_item(&self, item_id: &str, media_type: MediaType) -> Result<MediaItem>;

    /// Resolve a playable item into stream details.
    async fn get_stream_details(
        &self,
        item_id: &str,
        media_type: MediaType,
    ) -> Result<StreamDetails>;

    /// Browsable root path. Implementations return sub-folders.
    async fn browse(&self, path: &str) -> Result<Vec<MediaItem>> {
        let _ = path;
        Err(ProviderError::Unsupported("browse"))
    }
}

/// Trait for providers that can play a stream directly (rather than just
/// resolving a URL that the stream controller will ffmpeg-process).
/// Filesystem, radio, and Spotify all implement this via concrete
/// `get_stream_bytes` methods.
///
/// This is a *standalone* trait: it does not require `MusicProvider`
/// because some pieces (e.g. `LibrespotStreamer` in the spotify crate)
/// only contribute the streaming side, not the catalogue side.
#[async_trait]
pub trait StreamProvider: Send + Sync {
    /// Stream a chunk of bytes from the underlying source. The default
    /// implementation returns `Unsupported`; concrete providers override
    /// this for direct file / pipe / HLS reads.
    async fn get_stream_bytes(
        &self,
        details: &StreamDetails,
        seek_position: u32,
    ) -> Result<bytes::Bytes> {
        let _ = (details, seek_position);
        Err(ProviderError::Unsupported("get_stream_bytes"))
    }
}

/// Handle to a registered provider.
#[derive(Clone)]
pub struct ProviderHandle {
    pub instance_id: String,
    pub manifest: ProviderManifest,
    pub music: Arc<dyn MusicProvider>,
    pub stream: Option<Arc<dyn StreamProvider>>,
}

impl std::fmt::Debug for ProviderHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderHandle")
            .field("instance_id", &self.instance_id)
            .field("domain", &self.manifest.domain)
            .finish_non_exhaustive()
    }
}

impl ProviderHandle {
    pub fn new<P: MusicProvider + 'static>(provider: P) -> Self {
        let manifest = provider.manifest().clone();
        let instance_id = provider.instance_id().to_string();
        Self {
            instance_id,
            manifest,
            music: Arc::new(provider),
            stream: None,
        }
    }

    /// Attach a separate streaming impl (Spotify's `LibrespotStreamer`
    /// is the canonical example: it doesn't expose a `MusicProvider`
    /// but the player controller needs it to be addressable by domain).
    pub fn with_stream<P: StreamProvider + 'static>(mut self, stream: P) -> Self {
        self.stream = Some(Arc::new(stream));
        self
    }
}

/// Process-wide registry of providers.
#[derive(Default)]
pub struct ProviderRegistry {
    by_id: RwLock<HashMap<String, ProviderHandle>>,
    by_domain: RwLock<HashMap<String, Vec<String>>>,
}

impl ProviderRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn register(&self, handle: ProviderHandle) -> Result<()> {
        let id = handle.instance_id.clone();
        let domain = handle.manifest.domain.clone();
        if self.by_id.read().contains_key(&id) {
            return Err(ProviderError::AlreadyRegistered(id));
        }
        self.by_id.write().insert(id.clone(), handle);
        self.by_domain.write().entry(domain).or_default().push(id);
        Ok(())
    }

    pub fn unregister(&self, instance_id: &str) -> Option<ProviderHandle> {
        let handle = self.by_id.write().remove(instance_id)?;
        let mut by_domain = self.by_domain.write();
        if let Some(list) = by_domain.get_mut(&handle.manifest.domain) {
            list.retain(|id| id != instance_id);
        }
        let _ = handle;
        Some(handle)
    }

    pub fn get(&self, instance_id: &str) -> Option<ProviderHandle> {
        self.by_id.read().get(instance_id).cloned()
    }

    pub fn list(&self) -> Vec<ProviderHandle> {
        self.by_id.read().values().cloned().collect()
    }

    pub fn list_by_domain(&self, domain: &str) -> Vec<ProviderHandle> {
        let ids = self
            .by_domain
            .read()
            .get(domain)
            .cloned()
            .unwrap_or_default();
        let by_id = self.by_id.read();
        ids.into_iter()
            .filter_map(|id| by_id.get(&id).cloned())
            .collect()
    }

    /// Load providers from a TOML / JSON config file. The config file
    /// has the shape `{"providers": { "<domain>": [ProviderConfig, ...]}}`.
    pub fn load_config(&self, _config: serde_json::Value) -> Result<()> {
        // Real config wiring happens in `ma-server` once a stable
        // schema is in place; for Phase 1 we expose a no-op that lets
        // test code call into the registry without a real config file.
        Ok(())
    }
}

/// Helper to look up a provider by `domain/instance_id` (the wire format
/// the UI uses, e.g. `"spotify/spotify_2"`).
pub fn split_provider_ref(s: &str) -> Option<(&str, &str)> {
    s.split_once('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ProviderManifest, ProviderStage, ProviderType};

    fn make_provider(domain: &str, instance: &str) -> ProviderHandle {
        struct Stub {
            domain: String,
            instance: String,
            manifest: ProviderManifest,
        }
        #[async_trait]
        impl MusicProvider for Stub {
            fn domain(&self) -> &str {
                &self.domain
            }
            fn instance_id(&self) -> &str {
                &self.instance
            }
            fn manifest(&self) -> &ProviderManifest {
                &self.manifest
            }
            async fn search(&self, _q: &str, _m: &[MediaType], _l: u32) -> Result<SearchResults> {
                Ok(SearchResults::default())
            }
            async fn get_item(&self, _id: &str, _m: MediaType) -> Result<MediaItem> {
                Err(ProviderError::MediaNotFound("n/a".into()))
            }
            async fn get_stream_details(&self, _id: &str, _m: MediaType) -> Result<StreamDetails> {
                Err(ProviderError::MediaNotFound("n/a".into()))
            }
        }
        let manifest = ProviderManifest {
            provider_type: ProviderType::Music,
            domain: domain.to_string(),
            stage: ProviderStage::Stable,
            name: "Test".into(),
            description: String::new(),
            codeowners: vec![],
            credits: vec![],
            requirements: vec![],
            documentation: None,
            multi_instance: true,
            builtin: false,
            allow_disable: true,
            icon: None,
        };
        let mut handle = ProviderHandle::new(Stub {
            domain: domain.into(),
            instance: instance.into(),
            manifest,
        });
        handle.instance_id = instance.into();
        handle
    }

    #[test]
    fn register_and_lookup() {
        let reg = ProviderRegistry::new();
        reg.register(make_provider("spotify", "spotify_1")).unwrap();
        reg.register(make_provider("spotify", "spotify_2")).unwrap();
        reg.register(make_provider("radio", "radio_1")).unwrap();
        assert!(reg.get("spotify_1").is_some());
        assert_eq!(reg.list_by_domain("spotify").len(), 2);
        assert_eq!(reg.list_by_domain("radio").len(), 1);
    }

    #[test]
    fn duplicate_registration_rejected() {
        let reg = ProviderRegistry::new();
        reg.register(make_provider("a", "x")).unwrap();
        let r = reg.register(make_provider("a", "x"));
        assert!(matches!(r, Err(ProviderError::AlreadyRegistered(_))));
    }

    #[test]
    fn unregister_drops_handle() {
        let reg = ProviderRegistry::new();
        reg.register(make_provider("a", "x")).unwrap();
        assert!(reg.unregister("x").is_some());
        assert!(reg.get("x").is_none());
        assert!(reg.unregister("x").is_none());
    }

    #[test]
    fn split_provider_ref_returns_domain_and_instance() {
        assert_eq!(
            split_provider_ref("spotify/spotify_2"),
            Some(("spotify", "spotify_2"))
        );
        assert_eq!(split_provider_ref("no_slash"), None);
    }

    #[test]
    fn provider_config_serializes() {
        use crate::ProviderConfig;
        let cfg = ProviderConfig {
            instance_id: "x".into(),
            domain: "spotify".into(),
            enabled: true,
            values: serde_json::json!({"client_id": "abc"}),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("\"client_id\":\"abc\""));
    }
}
