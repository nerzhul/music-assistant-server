//! `ma-player-bridge` — bridge player.
//!
//! The bridge is a *proxy* `PlayerControl` that delegates every
//! command to an inner player (typically a Sendspin client). Its
//! job is to give the rest of the player system a single, stable
//! `player_id` to address — the underlying Sendspin client can come
//! and go (reconnects, name changes) without the sync-group /
//! universal-group / queue code having to track those events.
//!
//! Phase 4 surface:
//!
//! * `BridgePlayer` — wraps an `Arc<dyn PlayerControl>` and
//!   forwards every command, translating the stable bridge id to
//!   the inner player's id where needed.
//! * `BridgePlayer::inner_player_id` — accessor for the underlying
//!   id (handy for the streams controller to build `/bridge/...`
//!   URLs).
//! * `PlayerControl` trait impl — so the bridge can be passed to
//!   `SyncGroup::add_member` / `UniversalGroup::capture_members`.

#![forbid(unsafe_code)]

pub mod player;

pub use player::BridgePlayer;
