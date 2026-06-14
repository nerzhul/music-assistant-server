//! `ma-provider-ytmusic` — YouTube Music provider.
//!
//! **Scope of this scaffold (Phase 6, V1)**
//!
//! The Python reference (`music_assistant/providers/ytmusic/`) uses
//! `ytmusicapi` to talk to the Web API and `yt-dlp` to extract stream
//! URLs. Re-implementing the full `ytmusicapi` JSON surface in Rust
//! is a multi-week project; V1 of the Rust port is therefore
//! intentionally minimal:
//!
//! 1. **Streaming**: a [`yt_dlp::Ytdlp`] helper that shells out to
//!    `yt-dlp` (or its Python alias `yt-dlp` / the `youtube-dl` fork)
//!    to resolve a track `video_id` to a direct HLS / DASH manifest
//!    URL. The stream controller then runs the URL through ffmpeg,
//!    exactly as for radio streams. Cookie-based auth is passed
//!    through `--cookies <path>` (the user must have exported their
//!    browser cookie via a separate tool, e.g. "Get cookies.txt
//!    LOCALLY").
//!
//! 2. **Catalogue (best-effort)**: the same `yt-dlp` call can be run
//!    with `--flat-playlist -J` to dump playlist/album/track metadata
//!    as JSON. The `parsers` module maps that JSON to our flat
//!    [`Track`]/[`Album`]/[`Artist`]/[`Playlist`] structs. This covers
//!    the most common case (user pastes a YT Music album/playlist
//!    URL) without needing the full InnerTube reverse engineering.
//!
//! 3. **Search**: not implemented in V1; the provider returns an
//!    empty `SearchResults` and logs a `tracing::warn!`. The Python
//!    plan calls out InnerTube as the V2 work; we leave a clear
//!    `TODO(inner_tube)` marker.
//!
//! 4. **PO token**: optional, set `MA_YTMUSIC_PO_TOKEN` (and
//!    `MA_YTMUSIC_PO_TOKEN_SERVER_URL` for a remote token server). The
//!    token is passed to `yt-dlp` via the `po_token` JS argument
//!    (`--extractor-args "youtube:po_token=...;visitor_data=..."`).
//!
//! **Why `yt-dlp`?**
//!
//! The plan explicitly chooses `yt-dlp` as an external dependency to
//! avoid re-implementing the n-parameter signature decipher. `yt-dlp`
//! ships its own JS runtime (Deno / Node) so we don't need a second
//! dependency.

#![forbid(unsafe_code)]

pub mod manifest;
pub mod parser;
pub mod provider;
pub mod yt_dlp;

pub use manifest::ytmusic_manifest;
pub use provider::{YTMusicConfig, YTMusicProvider};
