//! Media item envelopes used by providers.
//!
//! We deliberately keep these flatter than the Python equivalents
//! (Pydantic dataclasses with rich metadata) — a Rust port that tries to
//! 1:1 mirror every field would be enormous and most of the metadata
//! fields are only used by the UI, not by the provider code.

use ma_core::enums::{AlbumType, ArtistType, MediaType};
use ma_core::identifiers::MediaItemId;
use serde::{Deserialize, Serialize};

/// A library item as returned by a provider. Each variant matches one
/// of the MA media types. Only fields the provider code actively uses
/// are populated; the rest can be filled in by the library controller
/// before being shown to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "media_type", rename_all = "snake_case")]
pub enum MediaItem {
    Track(Track),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
    Radio(Radio),
    Audiobook(Audiobook),
    Podcast(Podcast),
    PodcastEpisode(PodcastEpisode),
}

impl MediaItem {
    pub fn media_type(&self) -> MediaType {
        match self {
            Self::Track(_) => MediaType::Track,
            Self::Album(_) => MediaType::Album,
            Self::Artist(_) => MediaType::Artist,
            Self::Playlist(_) => MediaType::Playlist,
            Self::Radio(_) => MediaType::Radio,
            Self::Audiobook(_) => MediaType::Audiobook,
            Self::Podcast(_) => MediaType::Podcast,
            Self::PodcastEpisode(_) => MediaType::PodcastEpisode,
        }
    }
}

/// A collection of search results, one slot per supported media type.
/// Fields are `Vec` (not `Option<Vec>`) so providers can always fill the
/// `unknown` placeholder when they don't support a particular type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
    pub radio: Vec<Radio>,
    pub audiobooks: Vec<Audiobook>,
    pub podcasts: Vec<Podcast>,
    pub podcast_episodes: Vec<PodcastEpisode>,
}

impl SearchResults {
    pub fn merge(&mut self, other: SearchResults) {
        self.tracks.extend(other.tracks);
        self.albums.extend(other.albums);
        self.artists.extend(other.artists);
        self.playlists.extend(other.playlists);
        self.radio.extend(other.radio);
        self.audiobooks.extend(other.audiobooks);
        self.podcasts.extend(other.podcasts);
        self.podcast_episodes.extend(other.podcast_episodes);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Track {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub duration: Option<f64>,
    pub artists: Vec<Artist>,
    pub album: Option<Album>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub disc_number: Option<u32>,
    pub isrc: Option<String>,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Album {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub album_type: AlbumType,
    pub year: Option<u32>,
    pub artists: Vec<Artist>,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Artist {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub artist_type: ArtistType,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playlist {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub owner: Option<String>,
    pub is_editable: bool,
    pub track_count: u32,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Radio {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub click_count: Option<u64>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Audiobook {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub duration: Option<f64>,
    pub authors: Vec<Artist>,
    pub narrators: Vec<Artist>,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Podcast {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub publisher: Option<String>,
    pub total_episodes: u32,
    pub image_url: Option<String>,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PodcastEpisode {
    pub item_id: MediaItemId,
    pub provider: String,
    pub name: String,
    pub duration: Option<f64>,
    pub podcast: Option<Podcast>,
    pub position: Option<u32>,
    pub publish_date: Option<String>,
    pub audio_url: Option<String>,
    pub uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_merge_appends() {
        let mut a = SearchResults {
            tracks: vec![Track {
                item_id: ma_core::identifiers::MediaItemId("1".to_string()),
                provider: "p".into(),
                name: "T1".into(),
                uri: "u".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let b = SearchResults {
            tracks: vec![Track {
                item_id: ma_core::identifiers::MediaItemId("2".to_string()),
                provider: "p".into(),
                name: "T2".into(),
                uri: "u".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        a.merge(b);
        assert_eq!(a.tracks.len(), 2);
    }

    #[test]
    fn media_item_media_type_dispatch() {
        let track = MediaItem::Track(Track {
            item_id: ma_core::identifiers::MediaItemId("t".to_string()),
            provider: "p".into(),
            name: "T".into(),
            uri: "u".into(),
            ..Default::default()
        });
        assert_eq!(track.media_type(), MediaType::Track);
        let radio = MediaItem::Radio(Radio::default());
        assert_eq!(radio.media_type(), MediaType::Radio);
    }
}
