//! Embedded manifests for the podcast providers. Mirrors the shape of
//! `music_assistant/providers/podcastfeed/manifest.json` and
//! `music_assistant/providers/itunes_podcasts/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub fn podcastfeed_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "podcastfeed".to_string(),
        stage: ProviderStage::Stable,
        name: "Podcast RSS feed".to_string(),
        description: "Play podcasts from a single RSS feed URL.".to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec![],
        documentation: Some("https://music-assistant.io/music-providers/podcasts/".to_string()),
        multi_instance: true,
        builtin: false,
        allow_disable: true,
        icon: Some("podcast".to_string()),
    }
}

pub fn itunes_podcasts_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "itunes_podcasts".to_string(),
        stage: ProviderStage::Stable,
        name: "Apple iTunes Podcasts".to_string(),
        description: "Search and discover podcasts via the iTunes Podcast Directory.".to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec![],
        documentation: Some(
            "https://music-assistant.io/music-providers/itunes-podcasts/".to_string(),
        ),
        multi_instance: false,
        builtin: false,
        allow_disable: true,
        icon: Some("apple".to_string()),
    }
}
