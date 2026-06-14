//! `ma-provider-filesystem` — Filesystem music provider.
//!
//! Phase 3 implements the minimum surface needed for the player
//! controller to play local files and seed the library:
//!
//! * **Scanner** — async walker that produces `Track` / `Album` /
//!   `Artist` items with metadata extracted via [`lofty`].
//! * **m3u / pls parser** — playlist files are decoded into a list of
//!   `Track` items; the lazy lookup of each track by path is left to
//!   the library controller.
//! * **Stream reader** — `get_stream_bytes` opens the file via
//!   `tokio::fs` and reads chunks; the stream controller wraps that in
//!   the ffmpeg pipeline.
//! * **Library insert** — straightforward SQLite insert via `sqlx`.
//!
//! Reference: `music_assistant/providers/filesystem_local/__init__.py`.

#![forbid(unsafe_code)]

pub mod manifest;
pub mod parser;
pub mod provider;
pub mod scanner;
pub mod stream;

pub use manifest::filesystem_local_manifest;
pub use provider::{FilesystemConfig, FilesystemProvider};
