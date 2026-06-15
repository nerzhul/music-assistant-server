//! `ma-storage` — database layer for Music Assistant.
//!
//! Supports both **SQLite** (default, single-file embedded) and
//! **PostgreSQL** (clustered, multi-instance) via a `DatabaseUrl` that
//! resolves the right `sqlx` backend at runtime.
//!
//! ## Layout
//!
//! * [`DatabaseConfig`] — env-driven configuration
//! * [`Database`] — owns a `sqlx::AnyPool` and exposes the typed
//!   repositories
//! * [`migrations`] — embedded SQL files run at startup
//! * [`repos`] — one module per logical table (auth, cover_art, etc.)

#![forbid(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod cover_art;
pub mod error;
pub mod migrations;
pub mod pool;
pub mod repos;

pub use cover_art::CoverArtRecord;
pub use error::{StorageError, StorageResult};
pub use pool::{Database, DatabaseConfig, DatabaseKind};
pub use repos::{
    AlbumRow, ArtistRow, AuthRepository, CoverArtRepository, LibraryRepository, PlaylistRow,
    ProviderMappingRow, TrackRow,
};
