//! `ma-provider-radio` — RadioBrowser music provider.
//!
//! Implements the minimum surface the UI needs:
//!
//! * **search** — radio-browser.info `json/stations/search` endpoint.
//! * **browse** — top-level folders (popularity / category); a
//!   two-deep hierarchy (popularity/popular, country/<code>, etc.)
//!   backed by the same REST API.
//! * **get_radio** — single station lookup.
//! * **get_stream_details** — resolve the station's resolved URL and
//!   report the codec / bitrate as a `StreamDetails`.
//!
//! Reference: `music_assistant/providers/radiobrowser/__init__.py`.

#![forbid(unsafe_code)]

pub mod client;
pub mod manifest;
pub mod provider;

pub use client::{RadioBrowserClient, Station};
pub use manifest::RADIOBROWSER_MANIFEST;
pub use provider::RadioBrowserProvider;
