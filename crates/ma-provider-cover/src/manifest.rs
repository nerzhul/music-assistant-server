//! Embedded manifest for the cover art provider. Aggregates Google CSE
//! / iTunes Search / Musicbrainz Cover Art Archive; caches via
//! `CoverCache`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub const COVER_MANIFEST: ProviderManifest = ProviderManifest {
    provider_type: ProviderType::Metadata,
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

pub fn cover_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Metadata,
        domain: "cover_art".to_string(),
        stage: ProviderStage::Stable,
        name: "Cover art".to_string(),
        description:
            "Aggregate cover art from Google CSE, iTunes Search, and Musicbrainz Cover Art Archive."
                .to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec![],
        documentation: None,
        multi_instance: false,
        builtin: false,
        allow_disable: true,
        icon: Some("image".to_string()),
    }
}
