//! Embedded manifest for the radiobrowser provider. Mirrors
//! `music_assistant/providers/radiobrowser/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub const RADIOBROWSER_MANIFEST: ProviderManifest = ProviderManifest {
    provider_type: ProviderType::Music,
    domain: String::new(),
    stage: ProviderStage::Stable,
    name: String::new(),
    description: String::new(),
    codeowners: Vec::new(),
    credits: Vec::new(),
    requirements: Vec::new(),
    documentation: None,
    multi_instance: false,
    builtin: false,
    allow_disable: true,
    icon: None,
};

pub fn radiobrowser_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "radiobrowser".to_string(),
        stage: ProviderStage::Stable,
        name: "RadioBrowser".to_string(),
        description: "Listen to thousands of internet radio stations from across the world."
            .to_string(),
        codeowners: vec!["@gieljnssns".to_string()],
        credits: vec![],
        requirements: vec![],
        documentation: Some(
            "https://music-assistant.io/music-providers/radio-browser/".to_string(),
        ),
        multi_instance: false,
        builtin: false,
        allow_disable: true,
        icon: Some("radio".to_string()),
    }
}
