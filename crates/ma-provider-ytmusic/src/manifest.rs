//! Embedded manifest for the YouTube Music provider. Mirrors
//! `music_assistant/providers/ytmusic/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub fn ytmusic_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "ytmusic".to_string(),
        stage: ProviderStage::Beta,
        name: "YouTube Music".to_string(),
        description:
            "Stream tracks from YouTube Music via yt-dlp (requires browser cookie export)."
                .to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec!["yt-dlp <https://github.com/yt-dlp/yt-dlp>".to_string()],
        requirements: vec![
            "yt-dlp binary on PATH (or MA_YTMUSIC_YT_DLP_PATH)".to_string(),
            "Browser cookie export (Get cookies.txt LOCALLY)".to_string(),
        ],
        documentation: Some(
            "https://music-assistant.io/music-providers/youtube-music/".to_string(),
        ),
        multi_instance: true,
        builtin: false,
        allow_disable: true,
        icon: Some("youtube".to_string()),
    }
}
