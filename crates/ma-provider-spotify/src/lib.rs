//! `ma-provider-spotify` — Spotify music provider.
//!
//! Phase 3 implements the minimum surface the player controller needs:
//!
//! * **PKCE auth** — generate a code_verifier / code_challenge pair, a
//!   redirect URL, and exchange the resulting code for an access /
//!   refresh token via the Spotify Accounts service.
//! * **Web API** — `track`, `album`, `artist`, `playlist`, `search`,
//!   `me/library/*` endpoints with bearer-token auth and the
//!   refresh-token rotation used by the Python provider.
//! * **Streaming** — spawn the `librespot` binary as a child process
//!   (the same approach as the Python provider) and yield PCM
//!   chunks from its stdout.
//! * **Parsers** — map Spotify JSON shapes onto the `ma-providers`
//!   `Track` / `Album` / `Artist` / `Playlist` / `Podcast` /
//!   `PodcastEpisode` / `Audiobook` types.

#![forbid(unsafe_code)]

pub mod auth;
pub mod manifest;
pub mod parsers;
pub mod provider;
pub mod streaming;
pub mod web;

pub use auth::{PkceAuth, SpotifyToken};
pub use manifest::SPOTIFY_MANIFEST;
pub use provider::{SpotifyConfig, SpotifyProvider};
pub use streaming::LibrespotStreamer;
pub use web::SpotifyApi;
