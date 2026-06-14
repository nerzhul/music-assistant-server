//! Role identifiers, version constants, and the registry of which roles this
//! server supports.
//!
//! Application-specific roles (those not in the spec) MUST start with `_`,
//! per the spec. The MA-defined `_bridge` player role and `_sync` visualizer
//! role are application-specific roles used for non-WebSocket clients
//! (AirPlay / Sonos bridges, external sync visualization targets).

use serde::{Deserialize, Serialize};

/// Spec-defined role IDs (with version).
pub mod spec {
    pub const PLAYER_V1: &str = "player@v1";
    pub const CONTROLLER_V1: &str = "controller@v1";
    pub const METADATA_V1: &str = "metadata@v1";
    pub const ARTWORK_V1: &str = "artwork@v1";
    pub const VISUALIZER_V1: &str = "visualizer@v1";
    pub const COLOR_V1: &str = "color@v1";
}

/// MA-defined application-specific role IDs.
pub mod app {
    /// Player role that receives audio via a callback (no WebSocket audio).
    pub const BRIDGE_PLAYER: &str = "player@_bridge";
    /// Visualizer role that delivers extracted frames via a callback.
    pub const SYNC_VISUALIZER: &str = "visualizer@_sync";
}

/// All roles the Rust server implements.
pub const SUPPORTED_ROLES: &[&str] = &[
    spec::PLAYER_V1,
    spec::CONTROLLER_V1,
    spec::METADATA_V1,
    spec::ARTWORK_V1,
    spec::VISUALIZER_V1,
    spec::COLOR_V1,
    app::BRIDGE_PLAYER,
    app::SYNC_VISUALIZER,
];

/// Family of a role (the part before the `@`).
pub fn family_of(role_id: &str) -> &str {
    role_id.split_once('@').map(|(f, _)| f).unwrap_or(role_id)
}

/// Compare two role IDs as `family@version`, return true if they share the
/// same family.
pub fn same_family(a: &str, b: &str) -> bool {
    family_of(a) == family_of(b)
}

/// Pick the first role in `offered` whose family is implemented and whose
/// family has not yet been activated. Returns the activated role id, or None
/// if no acceptable role is offered.
pub fn activate_for_family(offered: &[String], activated: &mut Vec<String>) -> Option<String> {
    for role in offered {
        if !role.starts_with("player@_") // skip app-specific in default flow
            && !role.starts_with("visualizer@_")
            && !SUPPORTED_ROLES.contains(&role.as_str())
        {
            // Unknown / not implemented by us; skip (per spec, this signals
            // the server is outdated to the client, but we still continue).
            continue;
        }
        let family = family_of(role);
        if activated.iter().any(|a| family_of(a) == family) {
            continue;
        }
        activated.push(role.clone());
        return Some(role.clone());
    }
    None
}

/// Codec identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Opus,
    Flac,
    Pcm,
}

impl AudioCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Flac => "flac",
            Self::Pcm => "pcm",
        }
    }
}

impl std::str::FromStr for AudioCodec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "opus" => Ok(Self::Opus),
            "flac" => Ok(Self::Flac),
            "pcm" => Ok(Self::Pcm),
            other => Err(format!("unknown codec: {other}")),
        }
    }
}

/// Sample type identifier for PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PcmSampleType {
    Int,
    Float,
}

impl PcmSampleType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
        }
    }
}

/// Combined audio format (PCM or compressed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "codec", rename_all = "lowercase")]
pub enum AudioFormat {
    Opus {
        sample_rate: u32,
        channels: u8,
        bit_depth: u16,
    },
    Flac {
        sample_rate: u32,
        channels: u8,
        bit_depth: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        block_size: Option<u32>,
    },
    Pcm {
        sample_rate: u32,
        channels: u8,
        bit_depth: u16,
        sample_type: PcmSampleType,
    },
}

impl AudioFormat {
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Opus { sample_rate, .. }
            | Self::Flac { sample_rate, .. }
            | Self::Pcm { sample_rate, .. } => *sample_rate,
        }
    }
    pub fn channels(&self) -> u8 {
        match self {
            Self::Opus { channels, .. }
            | Self::Flac { channels, .. }
            | Self::Pcm { channels, .. } => *channels,
        }
    }
    pub fn bit_depth(&self) -> u16 {
        match self {
            Self::Opus { bit_depth, .. }
            | Self::Flac { bit_depth, .. }
            | Self::Pcm { bit_depth, .. } => *bit_depth,
        }
    }
    pub fn codec(&self) -> AudioCodec {
        match self {
            Self::Opus { .. } => AudioCodec::Opus,
            Self::Flac { .. } => AudioCodec::Flac,
            Self::Pcm { .. } => AudioCodec::Pcm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_skips_already_activated() {
        let offered = vec!["player@v1".to_string(), "controller@v1".to_string()];
        let mut activated = vec!["player@v1".to_string()];
        let next = activate_for_family(&offered, &mut activated);
        assert_eq!(next.as_deref(), Some("controller@v1"));
        assert_eq!(activated, vec!["player@v1", "controller@v1"]);
    }

    #[test]
    fn activate_skips_unknown() {
        let offered = vec!["player@v9".to_string(), "controller@v1".to_string()];
        let mut activated = vec![];
        let next = activate_for_family(&offered, &mut activated);
        assert_eq!(next.as_deref(), Some("controller@v1"));
    }

    #[test]
    fn activate_includes_bridge_role() {
        let offered = vec!["player@_bridge".to_string(), "controller@v1".to_string()];
        let mut activated = vec![];
        let next = activate_for_family(&offered, &mut activated);
        assert_eq!(next.as_deref(), Some("player@_bridge"));
        let next = activate_for_family(&offered, &mut activated);
        assert_eq!(next.as_deref(), Some("controller@v1"));
    }

    #[test]
    fn family_of_strips_version() {
        assert_eq!(family_of("player@v1"), "player");
        assert_eq!(family_of("player@_bridge"), "player");
    }

    #[test]
    fn audio_codec_round_trip() {
        for c in [AudioCodec::Opus, AudioCodec::Flac, AudioCodec::Pcm] {
            let s = serde_json::to_string(&c).unwrap();
            let back: AudioCodec = serde_json::from_str(&s).unwrap();
            assert_eq!(c, back);
        }
    }
}
