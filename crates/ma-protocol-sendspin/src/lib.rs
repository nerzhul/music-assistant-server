//! `ma-protocol-sendspin` — Sendspin Audio Protocol message types and binary IDs.
//!
//! Wire format: JSON text messages with `{ "type": "...", "payload": {...} }` shape
//! (the `server/state` envelope; some messages like `client/time` use a flat
//! shape — see `FlatMessage`). Binary messages use a single message-type byte
//! (0..=255) followed by role-specific payloads.
//!
//! Reference: <https://github.com/Sendspin/spec>
//!
//! This crate does not own the WebSocket transport or the time filter; it only
//! defines the wire types so the rest of the stack can speak Sendspin.

#![forbid(unsafe_code)]

pub mod binary;
pub mod messages;
pub mod roles;

pub use binary::*;
pub use messages::*;
pub use roles::*;
