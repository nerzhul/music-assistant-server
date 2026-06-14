//! Convert Spotify JSON responses to `ma-providers` media items.
//!
//! Mirrors `music_assistant/providers/spotify/parsers.py`. Each helper
//! takes the raw `serde_json::Value` returned by `SpotifyApi` plus a
//! `provider_instance_id` and returns the typed media item.

use ma_core::enums::AlbumType;
use ma_core::identifiers::MediaItemId;
use ma_providers::media::{Album, Artist, Audiobook, Playlist, Podcast, PodcastEpisode, Track};
use serde_json::Value;

use crate::web::helpers;

/// Provider instance id used to populate `provider` on every item.
pub struct ProviderCtx<'a> {
    pub instance_id: &'a str,
    pub domain: &'a str,
}

pub fn parse_track(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Track> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let duration_ms = helpers::opt_u64(v, "duration_ms").unwrap_or(0);
    let isrc = v
        .get("external_ids")
        .and_then(|e| helpers::opt_str(e, "isrc"))
        .map(str::to_string);
    let is_local = helpers::opt_bool(v, "is_local").unwrap_or(false);
    let is_playable = helpers::opt_bool(v, "is_playable").unwrap_or(true);
    let preview_url = helpers::opt_str(v, "preview_url").map(str::to_string);
    let track_number = helpers::opt_u32(v, "track_number");
    let disc_number = helpers::opt_u32(v, "disc_number");
    let album = v.get("album").and_then(|a| parse_album(a, ctx));
    let mut artists = Vec::new();
    if let Some(arr) = v.get("artists").and_then(|a| a.as_array()) {
        for a in arr {
            if let Some(artist) = parse_artist(a, ctx) {
                artists.push(artist);
            }
        }
    }
    let image = v.get("album").and_then(largest_image_from);
    Some(Track {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        duration: Some(duration_ms as f64 / 1000.0),
        artists,
        album,
        track_number,
        disc_number,
        isrc,
        image_url: image,
        uri,
    })
    .inspect(|_t| {
        // The Python provider also flips `available = !is_local && is_playable`
        // via the provider_mappings; we don't have a per-mapping field on
        // `Track` so the caller can check `is_local` / `is_playable` on the
        // source `serde_json::Value` if it needs to.
        let _ = (is_local, is_playable, preview_url);
    })
}

pub fn parse_album(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Album> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let album_type = helpers::opt_str(v, "album_type")
        .and_then(|t| match t {
            "album" => Some(AlbumType::Album),
            "single" => Some(AlbumType::Single),
            "compilation" => Some(AlbumType::Compilation),
            _ => None,
        })
        .unwrap_or(AlbumType::Unknown);
    let year = v
        .get("release_date")
        .and_then(Value::as_str)
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<u32>().ok());
    let mut artists = Vec::new();
    if let Some(arr) = v.get("artists").and_then(|a| a.as_array()) {
        for a in arr {
            if let Some(artist) = parse_artist(a, ctx) {
                artists.push(artist);
            }
        }
    }
    let image = largest_image_from(v);
    Some(Album {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        album_type,
        year,
        artists,
        image_url: image,
        uri,
    })
}

pub fn parse_artist(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Artist> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let image = largest_image_from(v);
    Some(Artist {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        artist_type: ma_core::enums::ArtistType::Singer,
        image_url: image,
        uri,
    })
}

pub fn parse_playlist(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Playlist> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let owner = v
        .get("owner")
        .and_then(|o| helpers::opt_str(o, "id"))
        .map(str::to_string);
    let track_count = v
        .get("tracks")
        .and_then(|t| t.get("total"))
        .and_then(|n| n.as_u64())
        .map(|n| n as u32)
        .unwrap_or(0);
    let image = largest_image_from(v);
    Some(Playlist {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        owner,
        is_editable: v
            .get("collaborative")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        track_count,
        image_url: image,
        uri,
    })
}

pub fn parse_podcast(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Podcast> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let image = largest_image_from(v);
    let publisher = helpers::opt_str(v, "publisher").map(str::to_string);
    Some(Podcast {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        publisher,
        total_episodes: 0,
        image_url: image,
        uri,
    })
}

pub fn parse_podcast_episode(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<PodcastEpisode> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let _image = largest_image_from(v);
    let duration_ms = helpers::opt_u64(v, "duration_ms").unwrap_or(0);
    let release_date = helpers::opt_str(v, "release_date").map(str::to_string);
    let audio_url = v
        .get("external_urls")
        .and_then(|e| helpers::opt_str(e, "spotify"))
        .map(str::to_string);
    let show = v.get("show").and_then(|s| parse_podcast(s, ctx));
    Some(PodcastEpisode {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        duration: if duration_ms > 0 {
            Some(duration_ms as f64 / 1000.0)
        } else {
            None
        },
        podcast: show,
        position: None,
        publish_date: release_date,
        audio_url,
        uri,
    })
}

pub fn parse_audiobook(v: &serde_json::Value, ctx: &ProviderCtx) -> Option<Audiobook> {
    let id = helpers::opt_str(v, "id")?;
    let name = helpers::opt_str(v, "name").unwrap_or(id).to_string();
    let uri = helpers::opt_str(v, "uri").unwrap_or("").to_string();
    let image = largest_image_from(v);
    let duration_ms = helpers::opt_u64(v, "duration_ms").unwrap_or(0);
    let mut authors = Vec::new();
    if let Some(arr) = v.get("authors").and_then(|a| a.as_array()) {
        for a in arr {
            if let Some(name) = helpers::opt_str(a, "name") {
                authors.push(Artist {
                    item_id: MediaItemId(format!("spotify:author:{name}")),
                    provider: ctx.instance_id.to_string(),
                    name: name.to_string(),
                    artist_type: ma_core::enums::ArtistType::Author,
                    ..Default::default()
                });
            }
        }
    }
    let mut narrators = Vec::new();
    if let Some(arr) = v.get("narrators").and_then(|a| a.as_array()) {
        for a in arr {
            if let Some(name) = helpers::opt_str(a, "name") {
                narrators.push(Artist {
                    item_id: MediaItemId(format!("spotify:narrator:{name}")),
                    provider: ctx.instance_id.to_string(),
                    name: name.to_string(),
                    artist_type: ma_core::enums::ArtistType::Narrator,
                    ..Default::default()
                });
            }
        }
    }
    Some(Audiobook {
        item_id: MediaItemId(id.to_string()),
        provider: ctx.instance_id.to_string(),
        name,
        duration: if duration_ms > 0 {
            Some(duration_ms as f64 / 1000.0)
        } else {
            None
        },
        authors,
        narrators,
        image_url: image,
        uri,
    })
}

fn largest_image_from(v: &serde_json::Value) -> Option<String> {
    helpers::largest_image(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>() -> ProviderCtx<'a> {
        ProviderCtx {
            instance_id: "spotify",
            domain: "spotify",
        }
    }

    #[test]
    fn parses_minimal_track() {
        let v = serde_json::json!({
            "id": "track1",
            "name": "Song",
            "uri": "spotify:track:track1",
            "duration_ms": 1234,
            "is_local": false,
            "is_playable": true,
            "artists": [
                {"id": "a1", "name": "Artist", "uri": "spotify:artist:a1"}
            ]
        });
        let t = parse_track(&v, &ctx()).unwrap();
        assert_eq!(t.name, "Song");
        assert_eq!(t.duration, Some(1.234));
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "Artist");
    }

    #[test]
    fn parses_album_year() {
        let v = serde_json::json!({
            "id": "alb",
            "name": "Album",
            "uri": "spotify:album:alb",
            "album_type": "album",
            "release_date": "2024-05-01"
        });
        let a = parse_album(&v, &ctx()).unwrap();
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.album_type, AlbumType::Album);
    }

    #[test]
    fn parses_playlist_track_count() {
        let v = serde_json::json!({
            "id": "pl",
            "name": "My List",
            "uri": "spotify:playlist:pl",
            "collaborative": true,
            "tracks": {"total": 42}
        });
        let p = parse_playlist(&v, &ctx()).unwrap();
        assert_eq!(p.track_count, 42);
        assert!(p.is_editable);
    }

    #[test]
    fn parses_podcast_episode_release_date() {
        let v = serde_json::json!({
            "id": "ep",
            "name": "Ep 1",
            "uri": "spotify:episode:ep",
            "duration_ms": 600_000,
            "release_date": "2024-12-25",
            "show": {"id": "show", "name": "Show", "uri": "spotify:show:show"}
        });
        let ep = parse_podcast_episode(&v, &ctx()).unwrap();
        assert_eq!(ep.publish_date.as_deref(), Some("2024-12-25"));
        assert_eq!(ep.podcast.as_ref().unwrap().name, "Show");
    }
}
