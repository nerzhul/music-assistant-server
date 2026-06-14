//! Embedded manifest for the Home Assistant integration. Mirrors
//! the shape of `music_assistant/providers/hass*/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub fn hass_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "hass".to_string(),
        stage: ProviderStage::Beta,
        name: "Home Assistant (media_player import)".to_string(),
        description: "Import Home Assistant media_player entities as MA players.".to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec!["Home Assistant 2024.4+ with REST API exposed.".to_string()],
        documentation: Some(
            "https://music-assistant.io/music-providers/home-assistant/".to_string(),
        ),
        multi_instance: false,
        builtin: false,
        allow_disable: true,
        icon: Some("home-assistant".to_string()),
    }
}
