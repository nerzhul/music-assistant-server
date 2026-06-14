//! Embedded manifest for the filesystem_local provider. Mirrors the
//! shape of `music_assistant/providers/filesystem_local/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub const FILESYSTEM_LOCAL_MANIFEST: ProviderManifest = ProviderManifest {
    provider_type: ProviderType::Music,
    domain: String::new(),
    stage: ProviderStage::Stable,
    name: String::new(),
    description: String::new(),
    codeowners: Vec::new(),
    credits: Vec::new(),
    requirements: Vec::new(),
    documentation: None,
    multi_instance: true,
    builtin: false,
    allow_disable: true,
    icon: None,
};

pub fn filesystem_local_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "filesystem_local".to_string(),
        stage: ProviderStage::Stable,
        name: "Filesystem (local disk)".to_string(),
        description: "Play music and audiobooks stored on locally connected drives.".to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec![],
        documentation: Some("https://music-assistant.io/music-providers/filesystem/".to_string()),
        multi_instance: true,
        builtin: false,
        allow_disable: true,
        icon: Some("harddisk".to_string()),
    }
}
