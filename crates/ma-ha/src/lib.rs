//! `ma-ha` — Home Assistant integration.
//!
//! **Scope (Phase 6, V1)**
//!
//! V1 covers the *HA → MA* direction: discover `media_player.*`
//! entities from a Home Assistant instance and surface them as MA
//! players. The reverse direction (MA → HA) is left for a later
//! phase.
//!
//! The crate has three layers:
//!
//! 1. [`client`] — a thin async HA REST + WebSocket client
//!    (configurable base URL + auth token).
//! 2. [`discover`] — polls `/api/states` and filters the entities
//!    matching the `include_entity_id_globs` filter.
//! 3. [`player`] — translates each `media_player.*` entity into an
//!    `ma_core::player::Player` (the V1 player only reflects HA
//!    state; commands are forwarded to HA via `call_service`).
//!
//! Authentication modes supported by V1:
//! * **Token** (`MA_HA_TOKEN`) — long-lived access token, the
//!   recommended setup for self-hosted HA.
//! * **Supervisor** (`MA_SUPERVISOR_TOKEN` + `MA_SUPERVISOR_URL`) —
//!   used when the MA server runs as a Home Assistant add-on.
//!
//! The OAuth2 device-code flow from the Python reference is
//! deliberately out of scope for V1; the plan's "V2 OAuth" mention
//! is a non-goal here.

#![forbid(unsafe_code)]

pub mod client;
pub mod config;
pub mod discover;
pub mod manifest;
pub mod player;

pub use client::{HaClient, HaClientError};
pub use config::{HaAuthMode, HaConfig};
pub use discover::{Discover, DiscoverError, DiscoveredEntity};
pub use manifest::hass_manifest;
