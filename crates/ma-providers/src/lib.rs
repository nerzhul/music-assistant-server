//! `ma-providers` — common Provider trait, manifest model, and shared types
//! used by every music/metadata/player provider in the Rust port.
//!
//! Mirrors the shape of the Python `MusicProvider` interface but uses
//! async-trait and an enum `MediaItem` so providers can return
//! heterogeneous results without `Box<dyn>`.

#![forbid(unsafe_code)]

pub mod manifest;
pub mod media;
pub mod provider;
pub mod stream;
pub mod versions;

pub use manifest::{ProviderConfig, ProviderManifest, ProviderStage, ProviderType};
pub use media::*;
pub use provider::*;
pub use stream::*;
pub use versions::*;
