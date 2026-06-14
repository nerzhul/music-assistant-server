//! `ma-provider-cover` — cover art provider.
//!
//! Aggregates three sources: Google Custom Search JSON API (paid, with
//! API key), iTunes Search API (free, no key) and the Musicbrainz
//! Cover Art Archive. The first match wins; results are cached via the
//! [`CoverCache`] trait so the same lookup never hits the network twice.
//!
//! Reference: `music_assistant/providers/cover_art/__init__.py`
//! (the Python provider keeps a single trait — we mirror that).

#![forbid(unsafe_code)]

pub mod cache;
pub mod google;
pub mod itunes;
pub mod manifest;
pub mod musicbrainz;
pub mod provider;

pub use cache::{CoverCache, DiskCoverCache, MemoryCoverCache};
pub use provider::{CoverConfig, CoverProvider};
