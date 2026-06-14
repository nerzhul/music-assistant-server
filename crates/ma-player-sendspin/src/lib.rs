//! `ma-player-sendspin` — Sendspin player provider for Music Assistant.
//!
//! This crate implements the server half of the Sendspin Audio Protocol:
//!
//! * **`SendspinServer`** owns a `tokio-tungstenite` WebSocket listener on
//!   the configured inbound port (default 8927, path `/sendspin`).
//! * For every connected client it spawns a **`SendspinConnection`** task
//!   that drives the handshake (`client/hello` → `server/hello`), then
//!   maintains the message loop (`client/time` ↔ `server/time`,
//!   `client/state` updates, `client/command` dispatch).
//! * Each connection owns a per-client **`TimeFilter`** so the client can
//!   translate server timestamps to its local clock.
//! * The server runs a per-`player@v1` **`PushStream`** analogue that
//!   timestamps every audio chunk and fans out to all connected players
//!   in the same group.
//!
//! This is a from-scratch Rust port of the Python `aiosendspin` server; it
//! keeps the same wire protocol and time-filter math but uses idiomatic
//! async Rust (no `aiortc` — WebRTC signaling is provided as a stub so the
//! UI can fall back to direct LAN WebSocket).
//!
//! Reference: <https://github.com/Sendspin/spec>, the Python provider at
//! `music_assistant/providers/sendspin/*.py`, and the `aiosendspin` library.

#![forbid(unsafe_code)]

pub mod codec;
pub mod playback;
pub mod protocol;
pub mod roles;
pub mod server;
pub mod state;

pub use server::{SendspinServer, SendspinServerConfig};
pub use state::{ClientId, ConnectionEvent, GroupId, GroupPlaybackState, SendspinState};
