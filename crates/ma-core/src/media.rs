//! Media item structs: Track, Album, Artist, Playlist, Radio, Audiobook, Podcast.
//!
//! These are kept minimal and serde-compatible with the Python models
//! (`music_assistant_models.media_items`).

use crate::enums::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderMapping {
    pub provider_domain: String,
    pub provider_instance: String,
    pub item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioFormat {
    pub content_type: ContentType,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format_string: Option<String>,
}

impl AudioFormat {
    pub fn pcm_48000_stereo() -> Self {
        Self {
            content_type: ContentType::PcmS16Le,
            sample_rate: 48_000,
            bit_depth: 16,
            channels: 2,
            bit_rate: None,
            codec: None,
            output_format_string: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Track {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub version: Option<String>,
    pub duration: Option<f64>,
    pub artists: Vec<Artist>,
    pub album: Option<Album>,
    #[serde(default)]
    pub media_type: MediaType,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Album {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub album_type: AlbumType,
    pub year: Option<u32>,
    pub artists: Vec<Artist>,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Artist {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub artist_type: ArtistType,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playlist {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub owner: Option<String>,
    pub is_editable: bool,
    pub track_count: u32,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Radio {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Audiobook {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub duration: Option<f64>,
    pub authors: Vec<Artist>,
    pub narrators: Vec<Artist>,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Podcast {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub publisher: Option<String>,
    pub total_episodes: u32,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PodcastEpisode {
    pub item_id: String,
    pub provider: String,
    pub name: String,
    pub duration: Option<f64>,
    pub podcast: Option<Podcast>,
    pub position: Option<u32>,
    pub provider_mappings: Vec<ProviderMapping>,
    pub image: Option<String>,
    pub uri: String,
}
