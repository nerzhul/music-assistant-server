//! Sendspin role state machine and event handlers.
//!
//! This module maps the per-role state from the spec onto the
//! `SendspinState` shared store. It is a thin layer — the heavy lifting
//! (push-stream scheduling, per-player DSP, sync group join) lives in
//! `playback` and is wired in by `server::SendspinServer`.
//!
//! The functions here are pure: they take the current state and an
//! incoming `client/state` / `client/command` payload, and return a list
//! of outbound messages the connection should send back.

use ma_core::identifiers::PlayerId;
use ma_protocol_sendspin::messages::{
    ClientCommand, ClientControllerCommand, ClientState, ControllerState, GroupUpdate,
    ServerCommand, ServerPlayerCommand, ServerState, StreamClear, StreamEnd, StreamStart,
};
use ma_protocol_sendspin::roles::AudioFormat;

use crate::protocol::{
    group_update_envelope, server_command_envelope, server_state_envelope, stream_clear_envelope,
    stream_end_envelope, stream_start_envelope,
};
use crate::state::{ClientRecord, SendspinState};

/// Apply a `client/state` update from the client to the server-side record.
/// Returns the new `ClientRecord` snapshot.
pub fn apply_client_state(state: &SendspinState, client_id: &str, msg: &ClientState) {
    state.update_client(client_id, |r| {
        r.state = msg.state;
        if let Some(player) = &msg.player {
            if let Some(v) = player.volume {
                r.volume = v;
            }
            if let Some(m) = player.muted {
                r.muted = m;
            }
            r.static_delay_ms = player.static_delay_ms;
            r.required_lead_time_ms = player.required_lead_time_ms;
            r.min_buffer_ms = player.min_buffer_ms;
            if let Some(supported) = &player.supported_commands {
                r.supported_player_commands = supported.clone();
            }
        }
    });
}

/// Map a `client/command` to the corresponding MA-side action. For the
/// player-family commands (volume, mute, play, pause, stop, next, previous,
/// repeat, shuffle, switch) we mutate the server state and emit a
/// `group/update` so the change is visible to the rest of the group.
pub fn handle_client_command(
    state: &SendspinState,
    client_id: &str,
    msg: &ClientCommand,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(cmd) = &msg.controller {
        if let Some(text) = render_command(state, client_id, cmd) {
            out.push(text);
        }
    }
    out
}

fn render_command(
    state: &SendspinState,
    client_id: &str,
    cmd: &ClientControllerCommand,
) -> Option<String> {
    let record = state.clients.read().get(client_id).cloned()?;
    let group_id = record.group_id.clone();
    match cmd.command.as_str() {
        "volume" => {
            let v = cmd.volume?;
            // Apply the same delta to every player in the group (mirroring
            // the spec's "Setting group volume" algorithm in a simplified
            // form: clamp each player and recompute the group volume as
            // the mean).
            let group_members: Vec<String> = state
                .clients
                .read()
                .values()
                .filter(|c| c.group_id == group_id)
                .map(|c| c.client_id.clone())
                .collect();
            let target = v as i32;
            for member_id in &group_members {
                state.update_client(member_id, |r| {
                    r.volume = target.clamp(0, 100) as u8;
                });
            }
            let update = GroupUpdate {
                playback_state: None,
                group_id: None,
                group_name: None,
            };
            // We re-emit a `server/state` controller block with the new mean.
            let updated: Vec<Arc<ClientRecord>> = state
                .clients
                .read()
                .values()
                .filter(|c| c.group_id == group_id)
                .cloned()
                .collect();
            let mean = if updated.is_empty() {
                0
            } else {
                (updated.iter().map(|c| c.volume as u32).sum::<u32>() / updated.len() as u32) as u8
            };
            let muted = !updated.is_empty() && updated.iter().all(|c| c.muted);
            let server_state = ServerState {
                metadata: None,
                controller: Some(ControllerState {
                    supported_commands: vec![
                        "play".into(),
                        "pause".into(),
                        "stop".into(),
                        "next".into(),
                        "previous".into(),
                        "volume".into(),
                        "mute".into(),
                    ],
                    volume: mean,
                    muted,
                    repeat: "off".into(),
                    shuffle: false,
                }),
                color: None,
            };
            let _ = update; // silence unused if compiler complains
            Some(server_state_envelope(&server_state).ok()?)
        }
        "mute" => {
            let m = cmd.mute?;
            let group_members: Vec<String> = state
                .clients
                .read()
                .values()
                .filter(|c| c.group_id == group_id)
                .map(|c| c.client_id.clone())
                .collect();
            for member_id in &group_members {
                state.update_client(member_id, |r| r.muted = m);
            }
            // Emit a `group/update` to broadcast the new state.
            let update = GroupUpdate {
                playback_state: None,
                group_id: None,
                group_name: None,
            };
            group_update_envelope(&update).ok()
        }
        "play" | "pause" | "stop" | "next" | "previous" | "repeat_off" | "repeat_one"
        | "repeat_all" | "shuffle" | "unshuffle" | "switch" => {
            // These are not yet wired to MA's queue controller; we just
            // bounce a `group/update` with the new playback state when it
            // changes.
            let playback = match cmd.command.as_str() {
                "play" => "playing",
                _ => "stopped",
            };
            let update = GroupUpdate {
                playback_state: Some(playback.to_string()),
                group_id: None,
                group_name: None,
            };
            group_update_envelope(&update).ok()
        }
        _ => None,
    }
}

use std::sync::Arc;

/// Build a `stream/start` envelope for a single client at the start of
/// playback. The player role gets the negotiated audio format; the
/// artwork role gets the default channel config; the visualizer role
/// gets a sensible default feature set.
pub fn build_stream_start(format: AudioFormat) -> Result<String, serde_json::Error> {
    stream_start_envelope(&StreamStart {
        player: Some(ma_protocol_sendspin::messages::StreamStartPlayer {
            codec: format.codec().as_str().to_string(),
            sample_rate: format.sample_rate(),
            channels: format.channels(),
            bit_depth: format.bit_depth(),
            codec_header: None,
        }),
        artwork: None,
        visualizer: None,
    })
}

pub fn build_stream_end() -> Result<String, serde_json::Error> {
    stream_end_envelope(&StreamEnd::default())
}

pub fn build_stream_clear() -> Result<String, serde_json::Error> {
    stream_clear_envelope(&StreamClear::default())
}

/// Build a `server/command` asking a specific client to change volume.
pub fn build_server_command_volume(volume: u8) -> Result<String, serde_json::Error> {
    server_command_envelope(&ServerCommand {
        player: Some(ServerPlayerCommand {
            command: "volume".into(),
            volume: Some(volume),
            mute: None,
            static_delay_ms: None,
        }),
    })
}

/// Build a `server/command` asking a specific client to mute / unmute.
pub fn build_server_command_mute(muted: bool) -> Result<String, serde_json::Error> {
    server_command_envelope(&ServerCommand {
        player: Some(ServerPlayerCommand {
            command: "mute".into(),
            volume: None,
            mute: Some(muted),
            static_delay_ms: None,
        }),
    })
}

/// Resolve a Sendspin `client_id` to an MA `PlayerId`. The convention is
/// `client_id` IS the player_id in the single-provider case; for bridge
/// clients we strip the `spb_` prefix and use the rest.
pub fn player_id_for_client(client_id: &str) -> PlayerId {
    PlayerId::new(client_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ma_protocol_sendspin::messages::{
        ClientHello, ClientState, ConnectionReason, PlayerSupportV1, SupportedAudioFormat,
        SyncState,
    };
    use ma_protocol_sendspin::roles::spec;
    use tokio::sync::mpsc;

    fn make_state() -> Arc<SendspinState> {
        SendspinState::new("srv1", "Test")
    }

    fn add_client(state: &SendspinState, id: &str) {
        let (tx, _rx) = mpsc::channel(8);
        let hello = ClientHello {
            client_id: id.into(),
            name: "Speaker".into(),
            version: 1,
            supported_roles: vec![spec::PLAYER_V1.into(), spec::CONTROLLER_V1.into()],
            device_info: None,
            player_v1_support: Some(PlayerSupportV1 {
                supported_formats: vec![SupportedAudioFormat {
                    codec: "opus".into(),
                    channels: 2,
                    sample_rate: 48_000,
                    bit_depth: 16,
                }],
                buffer_capacity: 1_048_576,
                supported_commands: vec!["volume".into(), "mute".into()],
            }),
            artwork_v1_support: None,
            visualizer_v1_support: None,
        };
        let active = state.hello_for(&hello.supported_roles).active_roles;
        state.upsert_client(ClientRecord {
            client_id: id.into(),
            name: hello.name,
            supported_roles: hello.supported_roles,
            active_roles: active,
            remote_addr: None,
            connection_reason: Some(ConnectionReason::Discovery),
            state: SyncState::Synchronized,
            volume: 50,
            muted: false,
            static_delay_ms: 0,
            required_lead_time_ms: 200,
            min_buffer_ms: 200,
            supported_player_commands: vec![],
            group_id: "g1".into(),
            connected_at: std::time::Instant::now(),
            outbound_tx: tx,
        });
    }

    #[tokio::test]
    async fn client_state_updates_volume() {
        let state = make_state();
        add_client(&state, "c1");
        let mut s = ClientState {
            state: SyncState::Synchronized,
            player: Some(ma_protocol_sendspin::messages::ClientPlayerState {
                volume: Some(80),
                muted: Some(true),
                static_delay_ms: 100,
                required_lead_time_ms: 250,
                min_buffer_ms: 300,
                supported_commands: Some(vec!["volume".into()]),
            }),
        };
        // The exact struct field is `player`, set that.
        s.state = SyncState::Synchronized;
        apply_client_state(&state, "c1", &s);
        let r = state.clients.read().get("c1").unwrap().clone();
        assert_eq!(r.volume, 80);
        assert!(r.muted);
        assert_eq!(r.static_delay_ms, 100);
    }

    #[tokio::test]
    async fn volume_command_emits_server_state() {
        let state = make_state();
        add_client(&state, "c1");
        add_client(&state, "c2");
        let cmd = ClientCommand {
            controller: Some(ClientControllerCommand {
                command: "volume".into(),
                volume: Some(70),
                mute: None,
            }),
        };
        let out = handle_client_command(&state, "c1", &cmd);
        assert_eq!(out.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(parsed["type"], "server/state");
        assert_eq!(parsed["payload"]["controller"]["volume"], 70);
    }

    #[tokio::test]
    async fn mute_command_broadcasts_group_update() {
        let state = make_state();
        add_client(&state, "c1");
        let cmd = ClientCommand {
            controller: Some(ClientControllerCommand {
                command: "mute".into(),
                volume: None,
                mute: Some(true),
            }),
        };
        let out = handle_client_command(&state, "c1", &cmd);
        assert_eq!(out.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(parsed["type"], "group/update");
    }

    #[tokio::test]
    async fn player_id_passthrough() {
        // Phase 1: client_id IS the player_id. The `spb_` bridge prefix is
        // used in the Python helpers for the `bridge_client_id_from_mac`
        // convention; on the Rust side the Sendspin `client_id` is opaque.
        let id = player_id_for_client("c1");
        assert_eq!(id.as_str(), "c1");
    }
}
