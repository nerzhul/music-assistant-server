//! `ma-player-universal-group` — universal-group player.
//!
//! A universal group ("UG") is a *fictional* player that consumes
//! one stream and fans it out to a captured set of `PlayerControl`
//! members. It does *not* use the native sync protocol of its
//! members; instead it acts as a relay (the Python class hosts a
//! `/ugp/{player_id}.{flac,mp3}` route on the streams controller
//! and the members each pull from that route).
//!
//! Phase 4 surface:
//!
//! * `UniversalGroup::capture_members` — store a snapshot of
//!   `PlayerControl` references for this session.
//! * `UniversalGroup::play_media` — placeholder that records the
//!   `current_media` field and transitions the group to PLAYING.
//!   The actual stream fan-out is delegated to `ma-streams` once
//!   the streams server is wired in.
//! * `UniversalGroup::set_volume` / `set_mute` / `stop` — broadcast
//!   to all captured members.
//! * `UniversalGroup::release` — release the captured members so
//!   they can join other groups; called after the idle grace
//!   period or on explicit `ungroup`.
//!
//! `PlayerControl` trait impl lets the group be addressed as a
//! regular player from the webserver / API layer.

#![forbid(unsafe_code)]

pub mod player;

pub use player::{UniversalGroup, UniversalGroupConfig};
