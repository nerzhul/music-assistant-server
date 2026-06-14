//! Manifest describing the S3 provider to the webserver.

use ma_providers::manifest::{ProviderManifest, ProviderStage, ProviderType};

pub fn s3_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "s3".to_string(),
        stage: ProviderStage::Beta,
        name: "S3-compatible storage".to_string(),
        description:
            "Read a music library from any S3-compatible bucket (AWS S3, MinIO, TrueNAS, Garage)."
                .to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec![],
        requirements: vec!["s3".to_string()],
        documentation: None,
        multi_instance: true,
        builtin: false,
        allow_disable: true,
        icon: Some("network-server".to_string()),
    }
}
