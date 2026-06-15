//! `ma-library` — Library controller.
//!
//! Orchestrates the indexing of music sources into the persistent
//! `library` schema (`tracks`, `albums`, `artists`, `playlists`,
//! `provider_mappings`, etc.):
//!
//! 1. **Scan** a filesystem provider's base path (or any other
//!    `MusicProvider` that returns items via `search`).
//! 2. **Parse** each track's tags via `ma-provider-filesystem`'s
//!    lofty wrapper (synchronous, runs on a `spawn_blocking` worker).
//! 3. **Upsert** the track, its artists, its album, and the
//!    `(provider_domain, provider_instance, item_id)` mapping into
//!    the DB through `ma_storage::LibraryRepository`.
//!
//! The controller does NOT try to deduplicate across providers: each
//! `provider_mapping` is unique on
//! `(media_type, provider_instance, provider_item_id)`, and tracks
//! that exist on multiple providers share the same local `item_id`
//! via the mapping table. A track from Spotify (`spotify:track:abc`)
//! and one from filesystem (`/music/a.flac`) are two different
//! `provider_mappings` rows pointing at the same `tracks` row.
//!
//! V1 supports filesystem only. Spotify / Radio / Podcasts providers
//! are stubbed (they go through their own `get_item` / `search`
//! paths) — the controller exposes an `upsert_track_from_metadata`
//! helper for those.

#![forbid(unsafe_code)]

pub mod controller;
pub mod safe_string;

pub use controller::{IndexSummary, LibraryController, LibraryError, LibraryResult};
