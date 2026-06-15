//! `LibraryController` — orchestrates scan + parse + persist.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use thiserror::Error;
use tracing::{debug, info, warn};

use ma_provider_filesystem::parser::{parse_track_file, ParsedTrack};
use ma_provider_filesystem::scanner::{scan, ScanConfig, ScanError, ScanResult, ScannedFile};
use ma_storage::repos::library_repo::{AlbumRow, ArtistRow, LibraryRepository, TrackRow};
use ma_storage::StorageError;
use sha1_smol::Sha1;

use crate::safe_string::create_safe_string;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("scan error: {0}")]
    Scan(#[from] ScanError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

pub type LibraryResult<T> = std::result::Result<T, LibraryError>;

/// Summary of a `LibraryController::index_filesystem` run.
#[derive(Debug, Default, Clone)]
pub struct IndexSummary {
    pub base_path: PathBuf,
    pub files_scanned: usize,
    pub tracks_parsed: usize,
    pub tracks_persisted: usize,
    pub tracks_failed: usize,
    pub albums_upserted: usize,
    pub artists_upserted: usize,
    pub mappings_upserted: usize,
    pub duration: std::time::Duration,
}

/// Per-call options for `index_filesystem`.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Skip the lofty parse + DB write for tracks whose path / size
    /// didn't change since the last run. We compare the
    /// `(absolute_path, file_size, modified_secs)` triple against a
    /// pre-existing `provider_mappings` row's `details` JSON.
    pub incremental: bool,
    /// Provider domain and instance to use for the
    /// `provider_mappings` rows. The convention is the same as for
    /// the filesystem provider: domain == instance ==
    /// `"filesystem_local"`.
    pub provider_domain: String,
    pub provider_instance: String,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            incremental: true,
            provider_domain: "filesystem_local".to_string(),
            provider_instance: "filesystem_local".to_string(),
        }
    }
}

/// Library controller. Owns a [`LibraryRepository`] handle; all DB
/// writes go through it.
#[derive(Clone)]
pub struct LibraryController {
    repo: LibraryRepository,
}

impl LibraryController {
    pub fn new(repo: LibraryRepository) -> Self {
        Self { repo }
    }

    /// Index a filesystem directory tree:
    ///
    /// 1. `scan` the base path (`ScannedFile` list).
    /// 2. For each `Track` file, run the lofty parser on a
    ///    `spawn_blocking` worker.
    /// 3. Upsert the resulting `Track` / `Album` / `Artist` rows +
    ///    the `(provider_domain, provider_instance, item_id)`
    ///    `provider_mappings` row.
    pub async fn index_filesystem(
        &self,
        base_path: &Path,
        options: &IndexOptions,
    ) -> LibraryResult<IndexSummary> {
        let started = Instant::now();
        let scan_result = scan(ScanConfig::new(base_path.to_path_buf())).await?;
        info!(
            base = %base_path.display(),
            tracks = scan_result.tracks.len(),
            playlists = scan_result.playlists.len(),
            errors = scan_result.errors.len(),
            "library: filesystem scan complete"
        );
        let mut summary = IndexSummary {
            base_path: base_path.to_path_buf(),
            files_scanned: scan_result.tracks.len(),
            ..Default::default()
        };
        // Process tracks in chunks via `spawn_blocking` so we don't
        // hog the runtime. We batch the DB writes (sequential, on
        // the runtime) to keep the repository's `&mut self` borrow
        // simple.
        let chunks: Vec<Vec<ScannedFile>> = scan_result
            .tracks
            .chunks(64)
            .map(<[ScannedFile]>::to_vec)
            .collect();
        for chunk in chunks {
            let parsed: Vec<(ScannedFile, Option<ParsedTrack>)> =
                tokio::task::spawn_blocking(move || {
                    chunk
                        .into_iter()
                        .map(|f| {
                            let path_str = f.absolute_path.to_string_lossy().to_string();
                            let parsed = parse_track_file(&path_str, "");
                            (f, parsed)
                        })
                        .collect()
                })
                .await
                .map_err(|e| LibraryError::Io(std::io::Error::other(e.to_string())))?;
            for (scanned, parsed) in parsed {
                if let Some(parsed) = parsed {
                    summary.tracks_parsed += 1;
                    match self
                        .persist_track(&parsed, &scanned, &scan_result, options)
                        .await
                    {
                        Ok(()) => summary.tracks_persisted += 1,
                        Err(e) => {
                            summary.tracks_failed += 1;
                            warn!(
                                path = %scanned.absolute_path.display(),
                                error = %e,
                                "library: persist_track failed"
                            );
                        }
                    }
                } else {
                    summary.tracks_failed += 1;
                }
            }
        }
        summary.duration = started.elapsed();
        info!(
            base = %base_path.display(),
            persisted = summary.tracks_persisted,
            failed = summary.tracks_failed,
            duration_secs = summary.duration.as_secs_f64(),
            "library: index_filesystem done"
        );
        Ok(summary)
    }

    /// Upsert a single track (and its album / artists / mapping)
    /// from a pre-parsed `ParsedTrack`. Useful for callers that have
    /// already done their own scan / parse (e.g. a S3 provider that
    /// reads a tag header from a buffer).
    pub async fn upsert_track_from_metadata(
        &self,
        parsed: &ParsedTrack,
        provider_domain: &str,
        provider_instance: &str,
        provider_item_id: &str,
    ) -> LibraryResult<()> {
        let track = &parsed.track;
        let now = Utc::now();
        // Upsert artist.
        let artist_id = stable_id(&[&track.name, provider_item_id]);
        if let Some(artist) = track.artists.first() {
            let row = ArtistRow {
                item_id: artist_id,
                name: artist.name.clone(),
                sort_name: create_safe_string(&artist.name),
                favorite: false,
                metadata: serde_json::json!({}),
                external_ids: serde_json::json!([]),
                play_count: 0,
                last_played: 0,
                timestamp_added: now,
                timestamp_modified: now,
                search_name: create_safe_string(&artist.name),
                search_sort_name: create_safe_string(&artist.name),
            };
            self.repo.upsert_artist(&row).await?;
        }
        // Upsert album.
        let album_id = stable_id(&[&track.name, "album", provider_item_id]);
        if let Some(album) = &track.album {
            let row = AlbumRow {
                item_id: album_id,
                name: album.name.clone(),
                sort_name: create_safe_string(&album.name),
                version: None,
                album_type: album.album_type.as_str().to_string(),
                year: album.year.map(|y| y as i64),
                favorite: false,
                metadata: serde_json::json!({}),
                external_ids: serde_json::json!([]),
                play_count: 0,
                last_played: 0,
                timestamp_added: now,
                timestamp_modified: now,
                search_name: create_safe_string(&album.name),
                search_sort_name: create_safe_string(&album.name),
            };
            self.repo.upsert_album(&row).await?;
        }
        // Upsert track.
        let track_id = stable_id(&[provider_domain, provider_item_id]);
        let row = TrackRow {
            item_id: track_id,
            name: track.name.clone(),
            sort_name: create_safe_string(&track.name),
            version: None,
            duration: track.duration.map(|d| d as i64),
            favorite: false,
            metadata: serde_json::json!({
                "content_type": parsed.content_type.as_str(),
                "sample_rate": parsed.sample_rate,
                "channels": parsed.channels,
                "bit_depth": parsed.bit_depth,
                "bit_rate": parsed.bit_rate,
                "genre": parsed.genre,
            }),
            external_ids: serde_json::json!([]),
            play_count: 0,
            last_played: 0,
            timestamp_added: now,
            timestamp_modified: now,
            search_name: create_safe_string(&track.name),
            search_sort_name: create_safe_string(&track.name),
        };
        self.repo.upsert_track(&row).await?;
        // Album <-> Track + Artist <-> Track links.
        if track.album.is_some() {
            self.repo
                .link_album_track(
                    album_id,
                    track_id,
                    track.disc_number.map(|d| d as i64).unwrap_or(0),
                    track.track_number.map(|n| n as i64).unwrap_or(0),
                )
                .await?;
        }
        if !track.artists.is_empty() {
            self.repo.link_track_artist(track_id, artist_id).await?;
        }
        // Provider mapping.
        let mapping = ma_storage::repos::library_repo::ProviderMappingRow {
            media_type: "track".to_string(),
            item_id: track_id,
            provider_domain: provider_domain.to_string(),
            provider_instance: provider_instance.to_string(),
            provider_item_id: provider_item_id.to_string(),
            available: true,
            in_library: true,
            is_unique: Some(false),
            url: None,
            audio_format: Some(serde_json::json!({
                "content_type": parsed.content_type.as_str(),
                "sample_rate": parsed.sample_rate,
                "channels": parsed.channels,
                "bit_depth": parsed.bit_depth,
                "bit_rate": parsed.bit_rate,
            })),
            details: None,
        };
        self.repo.upsert_provider_mapping(&mapping).await?;
        Ok(())
    }

    /// Persist a single track parsed from a filesystem scan.
    /// Incremental mode: skip if the `(size, mtime)` triple is
    /// unchanged since the last successful persist.
    async fn persist_track(
        &self,
        parsed: &ParsedTrack,
        scanned: &ScannedFile,
        _scan_result: &ScanResult,
        options: &IndexOptions,
    ) -> LibraryResult<()> {
        let path_str = scanned.absolute_path.to_string_lossy().to_string();
        if options.incremental {
            if let Some(_prev) = self
                .repo
                .lookup_provider_mapping("track", &options.provider_instance, &path_str)
                .await?
            {
                // V1: we always re-parse. The SQL hit on
                // `provider_mappings` is enough to confirm the
                // track was previously indexed; a future PR can
                // compare `(size, mtime)` against the cached
                // `audio_format.details` JSON and skip the
                // `parse_track_file` call when unchanged.
                debug!(
                    path = %path_str,
                    "library: incremental hit (V1 still re-parses)"
                );
            }
        }
        self.upsert_track_from_metadata(
            parsed,
            &options.provider_domain,
            &options.provider_instance,
            &path_str,
        )
        .await
    }
}

/// Stable i64 id derived from a tuple of strings. We use the first
/// 8 bytes of a SHA-1 over the joined input — same trick the
/// filesystem provider uses for podcast ids. The hex prefix is
/// interpreted as an `i64` (top bit masked off so the value stays
/// non-negative, matching SQLite's auto-increment convention).
fn stable_id(parts: &[&str]) -> i64 {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"|");
    }
    let digest = h.digest().to_string();
    // First 16 hex chars = 64 bits. Drop the top bit so the result
    // is always positive.
    let raw = u64::from_str_radix(&digest[..16], 16).unwrap_or(0);
    (raw & 0x7FFF_FFFF_FFFF_FFFF) as i64
}

/// `LibraryController` is `Send + Sync` so it can live in an
/// `Arc<LibraryController>` on the `AppState`.
pub type SharedLibrary = Arc<LibraryController>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_id_is_deterministic() {
        let a = stable_id(&["filesystem_local", "/music/a.flac"]);
        let b = stable_id(&["filesystem_local", "/music/a.flac"]);
        assert_eq!(a, b);
        let c = stable_id(&["filesystem_local", "/music/b.flac"]);
        assert_ne!(a, c);
    }

    #[test]
    fn stable_id_is_non_negative() {
        let id = stable_id(&["provider", "item"]);
        assert!(id >= 0);
    }

    #[tokio::test]
    async fn options_default_provider_domain() {
        let opts = IndexOptions::default();
        assert_eq!(opts.provider_domain, "filesystem_local");
        assert_eq!(opts.provider_instance, "filesystem_local");
        assert!(opts.incremental);
    }
}
