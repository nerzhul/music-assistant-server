//! Provider manifest + config schema. Mirrors the shape of the Python
//! `music_assistant_models.provider.ProviderManifest`.

use serde::{Deserialize, Serialize};

/// Type of a provider. Mirrors `ProviderType` from `ma-core` but is kept
/// here so the registry can use the same shape as the JSON manifest files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Music,
    Player,
    Metadata,
    Plugin,
    Core,
    AudioAnalysis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStage {
    Alpha,
    Beta,
    Stable,
    Experimental,
    Unmaintained,
    Deprecated,
}

/// A static provider manifest, loaded from `manifest.json` next to the
/// provider's `__init__.py`. For the Rust port we embed the manifest in
/// the crate and expose it through a constant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderManifest {
    /// `"music" | "player" | "metadata" | "plugin" | "core" | "audio_analysis"`.
    #[serde(rename = "type")]
    pub provider_type: ProviderType,
    /// Stable domain identifier (e.g. `"filesystem_local"`, `"spotify"`).
    pub domain: String,
    /// Stability stage, surfaced in the UI.
    #[serde(default = "default_stage")]
    pub stage: ProviderStage,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub codeowners: Vec<String>,
    #[serde(default)]
    pub credits: Vec<String>,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub documentation: Option<String>,
    #[serde(default)]
    pub multi_instance: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub allow_disable: bool,
    #[serde(default)]
    pub icon: Option<String>,
}

fn default_stage() -> ProviderStage {
    ProviderStage::Stable
}

/// Per-instance configuration: the user-edited values used to bring up a
/// provider. In the Rust port we use a plain `serde_json::Value` blob so
/// different providers can have wildly different config shapes without
/// enumerating them all in one big enum.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub instance_id: String,
    pub domain: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub values: serde_json::Value,
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let m = ProviderManifest {
            provider_type: ProviderType::Music,
            domain: "filesystem_local".into(),
            stage: ProviderStage::Stable,
            name: "Filesystem".into(),
            description: "Local files".into(),
            codeowners: vec!["@me".into()],
            credits: vec![],
            requirements: vec![],
            documentation: None,
            multi_instance: true,
            builtin: false,
            allow_disable: true,
            icon: Some("harddisk".into()),
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"type\":\"music\""));
        let back: ProviderManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.domain, "filesystem_local");
        assert_eq!(back.provider_type, ProviderType::Music);
    }

    #[test]
    fn default_stage_is_stable() {
        assert_eq!(default_stage(), ProviderStage::Stable);
    }
}
