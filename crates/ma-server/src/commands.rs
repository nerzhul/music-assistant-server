//! `commands` — registers all the API commands the webserver exposes.
//!
//! Mirrors the `Mass` class in `music_assistant/helpers/api.py`. Every
//! command takes a `CommandContext` plus a JSON `args` object and
//! returns a JSON value. Errors are returned via
//! `MusicAssistantError` (the registry maps the error code to
//! `error_code` in the wire response).
//!
//! Only the subset needed for the Music Assistant UI to discover
//! players, browse the library and trigger playback is implemented;
//! `not implemented` errors are returned for the rest until a later
//! phase wires them up.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use ma_core::api::{parse_args, CommandContext, CommandHandler, CommandRegistry, RequiredRole};
use ma_core::enums::MediaType;
use ma_core::errors::MusicAssistantError;

use crate::state::AppState;

pub fn build_registry(state: Arc<AppState>) -> Arc<CommandRegistry> {
    let reg = CommandRegistry::new();

    // --- public / anonymous commands ---

    reg.register(
        "info",
        RequiredRole::Anonymous,
        vec![],
        Some("server info / capabilities".to_string()),
        info_handler(state.clone()),
    );

    reg.register(
        "players/all",
        RequiredRole::Authenticated,
        vec![],
        Some("list every registered player".to_string()),
        players_all(state.clone()),
    );

    reg.register(
        "players/get",
        RequiredRole::Authenticated,
        vec!["player_id".into()],
        Some("fetch a single player".to_string()),
        player_get(state.clone()),
    );

    reg.register(
        "players/cmd/play",
        RequiredRole::Authenticated,
        vec!["player_id".into()],
        Some("start playback on a player".to_string()),
        player_cmd(state.clone(), PlayerCmd::Play),
    );
    reg.register(
        "players/cmd/stop",
        RequiredRole::Authenticated,
        vec!["player_id".into()],
        Some("stop playback on a player".to_string()),
        player_cmd(state.clone(), PlayerCmd::Stop),
    );
    reg.register(
        "players/cmd/pause",
        RequiredRole::Authenticated,
        vec!["player_id".into()],
        Some("pause playback".to_string()),
        player_cmd(state.clone(), PlayerCmd::Pause),
    );
    reg.register(
        "players/cmd/volume_set",
        RequiredRole::Authenticated,
        vec!["player_id".into(), "volume_level".into()],
        Some("set player volume (0..100)".to_string()),
        player_cmd(state.clone(), PlayerCmd::SetVolume),
    );
    reg.register(
        "players/cmd/volume_mute",
        RequiredRole::Authenticated,
        vec!["player_id".into(), "muted".into()],
        Some("set player mute".to_string()),
        player_cmd(state.clone(), PlayerCmd::SetMute),
    );
    reg.register(
        "players/cmd/power",
        RequiredRole::Authenticated,
        vec!["player_id".into(), "powered".into()],
        Some("power a player on/off".to_string()),
        player_cmd(state.clone(), PlayerCmd::SetPower),
    );

    // --- music catalogue ---

    reg.register(
        "music/search",
        RequiredRole::Authenticated,
        vec!["search_query".into()],
        Some("search the music library".to_string()),
        music_search(state.clone()),
    );

    reg.register(
        "music/browse",
        RequiredRole::Authenticated,
        vec!["path".into()],
        Some("browse a provider's root path".to_string()),
        music_browse(state.clone()),
    );

    reg.register(
        "music/get",
        RequiredRole::Authenticated,
        vec!["item_id".into(), "media_type".into()],
        Some("fetch a single media item".to_string()),
        music_get(state.clone()),
    );

    reg.register(
        "music/track/stream_url",
        RequiredRole::Authenticated,
        vec!["item_id".into()],
        Some("return the stream URL for a track".to_string()),
        music_stream_url(state.clone()),
    );

    // --- providers (admin only) ---

    reg.register(
        "providers/all",
        RequiredRole::Authenticated,
        vec![],
        Some("list all registered providers".to_string()),
        providers_all(state.clone()),
    );

    // --- sendspin (Phase 1: stubs for UI compatibility) ---

    reg.register(
        "sendspin/ice_servers",
        RequiredRole::Authenticated,
        vec![],
        Some("WebRTC ICE servers (V2: real config; V1: empty)".to_string()),
        |_ctx, _args| async move { Ok(json!([])) },
    );

    reg.register(
        "sendspin/connect",
        RequiredRole::Authenticated,
        vec![],
        Some("WebRTC connect (V2: real impl; V1: 501)".to_string()),
        |_ctx, _args| async {
            Err(MusicAssistantError::NotImplemented(
                "sendspin/connect (WebRTC) is not yet wired in the Rust port",
            ))
        },
    );

    Arc::new(reg)
}

// === handlers ===

fn info_handler(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, _args| {
        let state = Arc::clone(&state);
        async move {
            Ok(json!({
                "server_id": "ma-rs-001",
                "server_version": env!("CARGO_PKG_VERSION"),
                "schema_version": ma_server_info::SCHEMA_VERSION,
                "min_supported_schema_version": ma_server_info::MIN_SCHEMA_VERSION,
                "base_url": state.config.server.base_url,
                "homeassistant_addon": false,
                "onboard_done": state.auth.has_users(),
                "name": "Music Assistant (Rust)",
                "status": "running",
            }))
        }
    }
}

#[derive(Copy, Clone)]
enum PlayerCmd {
    Play,
    Stop,
    Pause,
    SetVolume,
    SetMute,
    SetPower,
}

fn players_all(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, _args| {
        let state = Arc::clone(&state);
        async move { Ok(to_value(state.player_controller.all_players())) }
    }
}

#[derive(Deserialize)]
struct PlayerIdArgs {
    player_id: String,
}

fn player_get(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        async move {
            let a: PlayerIdArgs = parse_args(&args)?;
            let id = ma_core::identifiers::PlayerId::from(a.player_id.clone());
            if let Some(p) = state.player_controller.sync_groups.read().get(&id) {
                return Ok(to_value(p.snapshot()));
            }
            if let Some(p) = state.player_controller.universal_groups.read().get(&id) {
                return Ok(to_value(p.snapshot()));
            }
            if let Some(p) = state.player_controller.bridges.read().get(&id) {
                return Ok(to_value(p.snapshot()));
            }
            Err(MusicAssistantError::NotFound(format!(
                "player {} not found",
                a.player_id
            )))
        }
    }
}

#[derive(Deserialize)]
struct CmdArgs {
    player_id: String,
    #[serde(default)]
    volume_level: Option<u32>,
    #[serde(default)]
    muted: Option<bool>,
    #[serde(default)]
    powered: Option<bool>,
}

fn player_cmd(state: Arc<AppState>, cmd: PlayerCmd) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        let cmd = cmd;
        async move {
            let a: CmdArgs = parse_args(&args)?;
            let id = ma_core::identifiers::PlayerId::from(a.player_id.clone());
            let target: Option<Arc<dyn ma_core::player::PlayerControl>> =
                if let Some(p) = state.player_controller.sync_groups.read().get(&id) {
                    Some(p.clone() as Arc<dyn ma_core::player::PlayerControl>)
                } else if let Some(p) = state.player_controller.universal_groups.read().get(&id) {
                    Some(p.clone() as Arc<dyn ma_core::player::PlayerControl>)
                } else if let Some(p) = state.player_controller.bridges.read().get(&id) {
                    Some(p.clone() as Arc<dyn ma_core::player::PlayerControl>)
                } else {
                    None
                };
            let Some(target) = target else {
                return Err(MusicAssistantError::NotFound(format!(
                    "player {} not found",
                    a.player_id
                )));
            };
            match cmd {
                PlayerCmd::Play => target.play().await,
                PlayerCmd::Stop => target.stop().await,
                PlayerCmd::Pause => target.stop().await,
                PlayerCmd::SetVolume => target.set_volume(a.volume_level.unwrap_or(0)).await,
                PlayerCmd::SetMute => target.set_mute(a.muted.unwrap_or(false)).await,
                PlayerCmd::SetPower => target.set_power(a.powered.unwrap_or(false)).await,
            }?;
            Ok(Value::Null)
        }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    search_query: String,
    #[serde(default)]
    media_types: Vec<String>,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    provider_instance_id_or_domain: Option<String>,
}

fn default_limit() -> u32 {
    25
}

fn music_search(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        async move {
            let a: SearchArgs = parse_args(&args)?;
            let media_types: Vec<MediaType> = if a.media_types.is_empty() {
                vec![MediaType::Track]
            } else {
                a.media_types
                    .iter()
                    .filter_map(|s| match s.as_str() {
                        "track" | "tracks" => Some(MediaType::Track),
                        "album" | "albums" => Some(MediaType::Album),
                        "artist" | "artists" => Some(MediaType::Artist),
                        "playlist" | "playlists" => Some(MediaType::Playlist),
                        _ => None,
                    })
                    .collect()
            };
            let mut merged = ma_providers::media::SearchResults::default();
            let providers: Vec<_> = if let Some(p) = &a.provider_instance_id_or_domain {
                state.providers.list_by_domain(p)
            } else {
                state.providers.list()
            };
            for handle in providers {
                let res = handle
                    .music
                    .search(&a.search_query, &media_types, a.limit)
                    .await;
                if let Ok(r) = res {
                    merged.tracks.extend(r.tracks);
                    merged.albums.extend(r.albums);
                    merged.artists.extend(r.artists);
                    merged.playlists.extend(r.playlists);
                }
            }
            Ok(serde_json::to_value(merged)?)
        }
    }
}

#[derive(Deserialize)]
struct BrowseArgs {
    path: String,
    #[serde(default)]
    provider_instance_id_or_domain: Option<String>,
}

fn music_browse(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        async move {
            let a: BrowseArgs = parse_args(&args)?;
            let providers: Vec<_> = if let Some(p) = &a.provider_instance_id_or_domain {
                state.providers.list_by_domain(p)
            } else {
                state.providers.list()
            };
            let mut merged: Vec<ma_providers::media::MediaItem> = Vec::new();
            for handle in providers {
                if let Ok(items) = handle.music.browse(&a.path).await {
                    merged.extend(items);
                }
            }
            Ok(serde_json::to_value(merged)?)
        }
    }
}

#[derive(Deserialize)]
struct GetArgs {
    item_id: String,
    media_type: String,
    #[serde(default)]
    provider_instance_id_or_domain: Option<String>,
}

fn music_get(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        async move {
            let a: GetArgs = parse_args(&args)?;
            let media_type = match a.media_type.as_str() {
                "track" | "tracks" => MediaType::Track,
                "album" | "albums" => MediaType::Album,
                "artist" | "artists" => MediaType::Artist,
                "playlist" | "playlists" => MediaType::Playlist,
                other => {
                    return Err(MusicAssistantError::InvalidInput(format!(
                        "unknown media_type: {other}"
                    )))
                }
            };
            if let Some(p) = &a.provider_instance_id_or_domain {
                let handle = state
                    .providers
                    .list_by_domain(p)
                    .into_iter()
                    .next()
                    .ok_or_else(|| MusicAssistantError::NotFound(p.clone()))?;
                let item = handle
                    .music
                    .get_item(&a.item_id, media_type)
                    .await
                    .map_err(|e| MusicAssistantError::Other(e.to_string()))?;
                return Ok(serde_json::to_value(item)?);
            }
            let providers = state.providers.list();
            for h in providers {
                if let Ok(item) = h.music.get_item(&a.item_id, media_type).await {
                    return Ok(serde_json::to_value(item)?);
                }
            }
            Err(MusicAssistantError::NotFound(a.item_id.clone()))
        }
    }
}

fn music_stream_url(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, args| {
        let state = Arc::clone(&state);
        async move {
            #[derive(Deserialize)]
            struct A {
                item_id: String,
            }
            let a: A = parse_args(&args)?;
            let providers = state.providers.list();
            for h in providers {
                if let Ok(details) = h
                    .music
                    .get_stream_details(&a.item_id, MediaType::Track)
                    .await
                {
                    return Ok(json!({
                        "url": details.path,
                        "provider": h.instance_id,
                    }));
                }
            }
            Err(MusicAssistantError::NotFound(a.item_id))
        }
    }
}

#[derive(Serialize)]
struct ProviderSummary {
    instance_id: String,
    domain: String,
    name: String,
    is_stream: bool,
}

fn providers_all(state: Arc<AppState>) -> impl CommandHandler {
    move |_ctx, _args| {
        let state = Arc::clone(&state);
        async move {
            let out: Vec<ProviderSummary> = state
                .providers
                .list()
                .into_iter()
                .map(|h| ProviderSummary {
                    instance_id: h.instance_id,
                    domain: h.manifest.domain,
                    name: h.manifest.name,
                    is_stream: h.stream.is_some(),
                })
                .collect();
            Ok(serde_json::to_value(out)?)
        }
    }
}

fn to_value<T: Serialize>(t: T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

mod ma_server_info {
    pub const SCHEMA_VERSION: i32 = 31;
    pub const MIN_SCHEMA_VERSION: i32 = 28;
}

#[allow(dead_code)]
fn _ctx_user(_ctx: &CommandContext) -> Option<&str> {
    _ctx.user_id.as_deref()
}
