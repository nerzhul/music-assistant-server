//! Embedded manifest for the Spotify provider. Mirrors
//! `music_assistant/providers/spotify/manifest.json`.

use ma_providers::{ProviderManifest, ProviderStage, ProviderType};

pub const SPOTIFY_MANIFEST: ProviderManifest = ProviderManifest {
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

pub fn spotify_manifest() -> ProviderManifest {
    ProviderManifest {
        provider_type: ProviderType::Music,
        domain: "spotify".to_string(),
        stage: ProviderStage::Stable,
        name: "Spotify".to_string(),
        description:
            "Stream music, playlists, podcasts, and discover new songs via Spotify’s ecosystem."
                .to_string(),
        codeowners: vec!["@music-assistant".to_string()],
        credits: vec!["librespot".to_string()],
        requirements: vec!["pkce".to_string()],
        documentation: Some("https://music-assistant.io/music-providers/spotify/".to_string()),
        multi_instance: true,
        builtin: false,
        allow_disable: true,
        icon: None,
    }
}

/// Spotify Web API scopes. Mirrors `music_assistant/providers/spotify/constants.py`.
pub const SCOPES: &str = "playlist-read-private playlist-read-collaborative \
    playlist-modify-public playlist-modify-private user-follow-modify \
    user-follow-read user-library-read user-library-modify user-read-private \
    user-read-email user-top-read app-remote-control streaming \
    user-read-playback-state user-modify-playback-state user-read-currently-playing \
    user-read-playback-position user-read-recently-played";

/// The default redirect URI for PKCE. The Python provider hard-codes
/// the same string and the frontend redirect handler at
/// `https://music-assistant.io/callback` consumes it.
pub const CALLBACK_REDIRECT_URL: &str = "https://music-assistant.io/callback";

/// The client id used by the bundled MA OAuth app (the Python
/// provider calls this "app_var(2)" — we hardcode a sensible default
/// and let advanced users override it via config).
pub const DEFAULT_CLIENT_ID: &str = "ma_rs_default_client_id";
