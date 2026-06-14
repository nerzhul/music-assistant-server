//! Typed representation of a row in the `cover_art` table.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct CoverArtRecord {
    pub image_id: String,
    pub provider: String,
    pub item_id: String,
    pub url: Option<String>,
    pub content_type: String,
    /// Image width in pixels. Stored as `i64` because `sqlx::Any` only
    /// has the `BigInt` codec; cast to `u32` when handing it to image
    /// crates.
    pub width: Option<i64>,
    /// Image height in pixels (see `width`).
    pub height: Option<i64>,
    pub bytes: Vec<u8>,
    pub fetched_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}
