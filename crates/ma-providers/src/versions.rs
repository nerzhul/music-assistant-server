//! DB schema versioning. Each provider that maintains its own table set
//! bumps its local version constant when the schema changes.

/// DB schema version for the `ma_providers` core tables (tracks,
/// albums, artists, playlists, provider_mappings). Increment when any
/// `MA_TABLE_*` definition in `ma-server` changes.
pub const CORE_DB_SCHEMA_VERSION: u32 = 1;

/// DB schema version for the filesystem_local provider tables.
pub const FILESYSTEM_DB_SCHEMA_VERSION: u32 = 1;
