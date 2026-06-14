//! `ma-provider-podcasts` — Podcast provider.
//!
//! Two sub-modules, mirroring the Python split:
//!
//! * **`feed`** — generic RSS/Atom feed reader. Each instance is bound
//!   to a single feed URL (the user pastes a feed link in the
//!   provider config). Implements `MusicProvider` for
//!   `Podcast` + `PodcastEpisode` (transposition of
//!   `providers/podcastfeed/__init__.py`).
//! * **`itunes`** — search/discovery provider backed by the iTunes
//!   Podcast Directory (`itunes.apple.com/search` + top-podcasts).
//!   `search()` returns `Podcast` items by iTunes `feedUrl`.
//!
//! Reference: `music_assistant/providers/podcastfeed/__init__.py` and
//! `music_assistant/providers/itunes_podcasts/__init__.py`.

#![forbid(unsafe_code)]

pub mod feed_parser;
pub mod feed_provider;
pub mod itunes;
pub mod itunes_provider;
pub mod manifest;

// Re-export the `itunes` module under a more conventional name for
// downstream users (and tests).
pub use itunes as itunes_api;

pub use feed_provider::{FeedProvider, PodcastConfig as FeedConfig};
pub use itunes_provider::{ITunesPodcastsConfig, ITunesPodcastsProvider};
pub use manifest::{itunes_podcasts_manifest, podcastfeed_manifest};
