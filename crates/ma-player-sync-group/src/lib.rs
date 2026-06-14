//! `ma-player-sync-group` — sync-group player.
//!
//! Mirrors `music_assistant/providers/sync_group/player.py`. The
//! sync-group is a *synthetic* player that holds a set of
//! `PlayerControl` members and picks one of them to be the "sync
//! leader". The leader's native sync protocol is what actually
//! keeps the audio in lockstep; this crate just manages the group
//! membership + state machine.
//!
//! Phase 4 surface (intentionally small for a from-scratch port):
//!
//! * `SyncGroup` — owns the group state machine
//!   (`idle` / `forming` / `active` / `dissolving`).
//! * `SyncGroup::set_members` — add / remove members; triggers a
//!   dissolve / reform when the current leader is removed from a
//!   group whose protocol doesn't support dynamic leader switch
//!   (see `PROVIDERS_WITH_DYNAMIC_LEADER_SWITCH`).
//! * `SyncGroup::select_leader` — pick the best candidate from the
//!   current members, preferring protocol continuity.
//! * `SyncGroup::play_media` / `stop` / `set_volume` / `set_mute` —
//!   fan the command out to the leader (which propagates to the
//!   followers at the protocol level).
//!
//! The `PlayerControl` trait (in `ma-core::player`) is the
//! abstraction over the real players. The sync-group crate doesn't
//! need to know about Sendspin / AirPlay / Snapcast details — it
//! just queries `provider_domain()` and `requires_flow_mode()`.

#![forbid(unsafe_code)]

pub mod group;
pub mod state;

pub use group::{SyncGroup, SyncGroupConfig};
pub use state::{GroupMember, GroupState, LeaderInfo};
