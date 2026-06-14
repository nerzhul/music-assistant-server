//! `ma-core` — shared types, enums, errors and identifiers for the Music Assistant Rust port.
//!
//! This crate mirrors the public surface of the Python `music_assistant_models` package
//! (Pydantic dataclasses + StrEnum). It is the foundation that every other crate builds on.

#![forbid(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod api;
pub mod auth;
pub mod enums;
pub mod errors;
pub mod identifiers;
pub mod media;
pub mod messages;
pub mod player;

pub use enums::*;
pub use errors::*;
pub use identifiers::*;
pub use media::*;
pub use messages::*;
pub use player::*;
