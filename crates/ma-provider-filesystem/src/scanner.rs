//! Async filesystem scanner.
//!
//! Walks a base directory, applies the same ignore rules as the Python
//! version (`recycle`, `.snapshot`, `System Volume Information`, ...)
//! and yields every supported file. Tag parsing happens lazily in
//! [`crate::parser::parse_track_file`] when the library controller
//! persists the file as a Track.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

/// File extensions the scanner accepts for tracks.
pub const TRACK_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "mp4", "flac", "ogg", "wav", "aiff", "wma", "dsf", "opus", "wv", "ape", "mpc",
    "mpeg", "mpg", "ts", "m2ts",
];

/// File extensions accepted for playlists.
pub const PLAYLIST_EXTENSIONS: &[&str] = &["m3u", "m3u8", "pls"];

/// File extensions accepted for embedded artwork / folder images.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];

/// Directories we always skip, regardless of what's in them.
pub const IGNORE_DIRS: &[&str] = &[
    "recycle",
    "Recently-Snaphot",
    "Recently-Snapshot",
    "#recycle",
    "System Volume Information",
    "lost+found",
    "@eaDir",
];

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("io error scanning {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("root path is not a directory: {0}")]
    NotADirectory(String),
}

/// One discovered file, pre-tag-parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
    pub filename: String,
    pub file_size: u64,
    pub modified_secs: i64,
    pub kind: ScannedKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScannedKind {
    Track,
    Playlist,
    Image,
}

impl ScannedFile {
    pub fn ext(&self) -> Option<String> {
        std::path::Path::new(&self.filename)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
    }
}

/// Output of a full scan.
#[derive(Debug, Default, Clone)]
pub struct ScanResult {
    pub base_path: PathBuf,
    pub tracks: Vec<ScannedFile>,
    pub playlists: Vec<ScannedFile>,
    pub images: Vec<ScannedFile>,
    pub errors: Vec<String>,
}

/// Configuration for the scanner.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub base_path: PathBuf,
}

impl ScanConfig {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

/// Scan a directory tree, returning every file whose extension matches
/// a known track / playlist / image set. Skips IGNORE_DIRS and hidden
/// directories.
pub async fn scan(config: ScanConfig) -> Result<ScanResult, ScanError> {
    let meta = tokio::fs::metadata(&config.base_path)
        .await
        .map_err(|e| ScanError::Io {
            path: config.base_path.display().to_string(),
            source: e,
        })?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory(
            config.base_path.display().to_string(),
        ));
    }

    let base_for_closure = config.base_path.clone();
    let base_for_err = base_for_closure.clone();
    let collected: Vec<ScannedFile> = tokio::task::spawn_blocking(move || {
        let base_path: std::path::PathBuf = base_for_closure;
        let mut out: Vec<ScannedFile> = Vec::new();
        let mut stack: Vec<std::path::PathBuf> = vec![base_path.clone()];
        while let Some(dir) = stack.pop() {
            let entries: std::fs::ReadDir = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    warn!(path = %dir.display(), error = %e, "scan: read_dir failed");
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = match entry.file_name().into_string() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if name.starts_with('.') || name.starts_with('_') {
                    continue;
                }
                if IGNORE_DIRS.contains(&name.as_str()) {
                    continue;
                }
                let ft = match entry.file_type() {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !ft.is_file() {
                    continue;
                }
                let ext_lower = std::path::Path::new(&name)
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                let kind = if TRACK_EXTENSIONS.contains(&ext_lower.as_str()) {
                    Some(ScannedKind::Track)
                } else if PLAYLIST_EXTENSIONS.contains(&ext_lower.as_str()) {
                    Some(ScannedKind::Playlist)
                } else if IMAGE_EXTENSIONS.contains(&ext_lower.as_str()) {
                    Some(ScannedKind::Image)
                } else {
                    None
                };
                let Some(kind) = kind else { continue };
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let rel = path
                    .strip_prefix(&base_path)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                out.push(ScannedFile {
                    absolute_path: path,
                    relative_path: rel,
                    filename: name,
                    file_size: metadata.len(),
                    modified_secs: metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                    kind,
                });
            }
        }
        out
    })
    .await
    .map_err(|e| ScanError::Io {
        path: base_for_err.display().to_string(),
        source: std::io::Error::other(e.to_string()),
    })?;

    let mut result = ScanResult {
        base_path: config.base_path.clone(),
        ..Default::default()
    };
    for f in collected {
        match f.kind {
            ScannedKind::Track => result.tracks.push(f),
            ScannedKind::Playlist => result.playlists.push(f),
            ScannedKind::Image => result.images.push(f),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    fn tempdir(label: &str) -> std::path::PathBuf {
        let n = NONCE.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ma-fs-scan-{}-{}-{}-{}",
            label,
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn scan_finds_tracks_playlists_images() {
        let tmp = tempdir("happy");
        std::fs::write(tmp.join("a.mp3"), b"fake-mp3").unwrap();
        std::fs::write(tmp.join("b.flac"), b"fake-flac").unwrap();
        std::fs::write(tmp.join("c.m3u"), b"#EXTM3U\nfoo.mp3\n").unwrap();
        std::fs::write(tmp.join("d.jpg"), b"\xff\xd8\xff").unwrap();
        std::fs::write(tmp.join("e.txt"), b"ignore").unwrap();
        let result = scan(ScanConfig::new(tmp.clone())).await.unwrap();
        assert_eq!(result.tracks.len(), 2);
        assert_eq!(result.playlists.len(), 1);
        assert_eq!(result.images.len(), 1);
    }

    #[tokio::test]
    async fn scan_skips_ignored_dirs() {
        let tmp = tempdir("ignored");
        std::fs::create_dir(tmp.join("recycle")).unwrap();
        std::fs::write(tmp.join("recycle/junk.mp3"), b"x").unwrap();
        std::fs::create_dir(tmp.join("@eaDir")).unwrap();
        std::fs::write(tmp.join("@eaDir/junk2.mp3"), b"x").unwrap();
        std::fs::create_dir(tmp.join(".hidden")).unwrap();
        std::fs::write(tmp.join(".hidden/junk3.mp3"), b"x").unwrap();
        let result = scan(ScanConfig::new(tmp.clone())).await.unwrap();
        let rels: Vec<_> = result
            .tracks
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert!(!rels.iter().any(|r| r.contains("recycle")));
        assert!(!rels.iter().any(|r| r.contains("@eaDir")));
        assert!(!rels.iter().any(|r| r.contains(".hidden")));
    }

    #[tokio::test]
    async fn scan_rejects_non_directory_root() {
        let tmp = tempdir("notdir");
        let file = tmp.join("not-a-dir.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            scan(ScanConfig::new(file)).await,
            Err(ScanError::NotADirectory(_))
        ));
    }

    #[test]
    fn track_extensions_cover_baseline() {
        for ext in [
            "mp3", "flac", "opus", "ogg", "m4a", "wav", "dsf", "aiff", "wma",
        ] {
            assert!(
                TRACK_EXTENSIONS.contains(&ext),
                "missing track extension {ext}"
            );
        }
    }

    #[test]
    fn content_type_filter_round_trip() {
        use crate::parser::content_type_for_ext;
        for ext in TRACK_EXTENSIONS {
            assert!(content_type_for_ext(ext).is_some(), "no CT for {ext}");
        }
    }
}
