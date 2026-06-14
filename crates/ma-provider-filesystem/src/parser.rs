//! Tag parsing for filesystem_local tracks using `lofty`.
//!
//! We support ID3v1/2 (MP3), MP4/M4A atoms, Vorbis comments (FLAC, OGG,
//! Opus) and basic WMA. For everything else we fall back to the
//! filename heuristic (parent directory = album, file stem = title).

use lofty::file::AudioFile;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::Accessor;

use ma_core::enums::ContentType;
use ma_providers::media::{Album, Artist, Track};

/// Detected format / content type for a given file extension.
pub fn content_type_for_ext(ext: &str) -> Option<ContentType> {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => Some(ContentType::Mp3),
        "m4a" | "m4b" | "mp4" | "aac" => Some(ContentType::M4A),
        "flac" => Some(ContentType::Flac),
        "ogg" => Some(ContentType::Ogg),
        "opus" => Some(ContentType::Opus),
        "wav" => Some(ContentType::Wav),
        "aiff" | "aif" => Some(ContentType::Aiff),
        "wma" | "wmav2" | "wmapro" => Some(ContentType::Wma),
        "dsf" | "dff" => Some(ContentType::Dsf),
        "wv" => Some(ContentType::WavPack),
        "ape" => Some(ContentType::Ape),
        "mpc" => Some(ContentType::MusePack),
        "ts" | "m2ts" | "mpeg" | "mpg" => Some(ContentType::Mpeg),
        _ => None,
    }
}

/// Parse tags from an audio file synchronously. This blocks the current
/// thread (lofty is sync); callers should run it inside
/// `tokio::task::spawn_blocking`.
pub fn parse_track_file(path: &str, provider: &str) -> Option<ParsedTrack> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let props = tagged.properties();

    let title = tag.title().map(|s| s.to_string()).unwrap_or_default();
    let artist_name = tag.artist().map(|s| s.to_string()).unwrap_or_default();
    let album_name = tag.album().map(|s| s.to_string()).unwrap_or_default();
    let album_artist = tag
        .get_string(&lofty::tag::ItemKey::AlbumArtist)
        .map(|s| s.to_string());
    let genre = tag.genre().map(|s| s.to_string());
    let track_num = tag.track();
    let disc_num = tag.disk();
    let year = tag.year();
    let isrc = tag
        .get_string(&lofty::tag::ItemKey::Isrc)
        .map(|s| s.to_string());
    let duration = props.duration().as_secs_f64();
    let sample_rate = props.sample_rate().unwrap_or(44_100);
    let channels = props.channels().unwrap_or(2);
    let bit_depth = props.bit_depth().unwrap_or(16);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let content_type = content_type_for_ext(ext).unwrap_or(ContentType::Unknown);

    let item_id = format!("{}|{}", provider, path);
    let uri = format!("filesystem://{}/{}", provider, path);

    let artist = Artist {
        item_id: ma_core::identifiers::MediaItemId(format!("artist:{artist_name}")),
        provider: provider.to_string(),
        name: artist_name,
        ..Default::default()
    };
    let album_artist_obj = album_artist.map(|name| Artist {
        item_id: ma_core::identifiers::MediaItemId(format!("artist:{name}")),
        provider: provider.to_string(),
        name,
        ..Default::default()
    });
    let album = if album_name.is_empty() {
        None
    } else {
        Some(Album {
            item_id: ma_core::identifiers::MediaItemId(format!("album:{album_name}")),
            provider: provider.to_string(),
            name: album_name,
            year,
            artists: album_artist_obj.into_iter().collect(),
            ..Default::default()
        })
    };

    let bit_depth_u16: u16 = bit_depth.into();

    let track = Track {
        item_id: ma_core::identifiers::MediaItemId(item_id),
        provider: provider.to_string(),
        name: if title.is_empty() {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path)
                .to_string()
        } else {
            title
        },
        duration: if duration > 0.0 { Some(duration) } else { None },
        artists: vec![artist],
        album,
        track_number: track_num,
        disc_number: disc_num,
        isrc,
        image_url: None,
        uri: uri.clone(),
    };

    Some(ParsedTrack {
        track,
        content_type,
        sample_rate,
        channels,
        bit_depth: bit_depth_u16,
        genre,
        bit_rate: props.audio_bitrate(),
    })
}

/// Output of `parse_track_file` — the parsed `Track` plus a few
/// extra fields the player controller needs.
#[derive(Debug, Clone)]
pub struct ParsedTrack {
    pub track: Track,
    pub content_type: ContentType,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u16,
    pub genre: Option<String>,
    pub bit_rate: Option<u32>,
}

/// Parse a path like `/music/Artist/Album/01 Track.mp3` into its
/// components. The first segment is the music root, then a hierarchy
/// of artist / album / disc / track. Returns the immediate parent
/// directory name (heuristic: that's the album name) and the filename
/// without extension (heuristic: that's the track title).
pub fn path_components(path: &str) -> PathComponents {
    let p = std::path::Path::new(path);
    let filename = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let parent = p
        .parent()
        .and_then(|s| s.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let grandparent = p
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    PathComponents {
        filename,
        stem,
        parent,
        grandparent,
    }
}

#[derive(Debug, Clone, Default)]
pub struct PathComponents {
    pub filename: String,
    pub stem: String,
    pub parent: String,
    pub grandparent: String,
}

/// Parse a `.m3u` / `.m3u8` playlist file. The parser is permissive:
/// lines starting with `#` are comments, `EXTINF` is dropped, blank
/// lines are skipped. Each remaining line is treated as a file path
/// (relative to the playlist's directory).
pub fn parse_m3u(playlist: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in playlist.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

/// Parse a `.pls` playlist file (INI-like). Returns the ordered list of
/// FileN= entries.
pub fn parse_pls(playlist: &str) -> Vec<String> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for line in playlist.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("File") {
            if let Some((num, path)) = rest.split_once('=') {
                if let Ok(n) = num.trim().parse::<u32>() {
                    out.push((n, path.trim().to_string()));
                }
            }
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_for_ext_recognises_common() {
        assert_eq!(content_type_for_ext("mp3"), Some(ContentType::Mp3));
        assert_eq!(content_type_for_ext("FLAC"), Some(ContentType::Flac));
        assert_eq!(content_type_for_ext("m4b"), Some(ContentType::M4A));
        assert_eq!(content_type_for_ext("ogg"), Some(ContentType::Ogg));
        assert_eq!(content_type_for_ext("opus"), Some(ContentType::Opus));
        assert_eq!(content_type_for_ext("xyz"), None);
    }

    #[test]
    fn path_components_extracts_parent_and_stem() {
        let c = path_components("/music/Artist/Album/01 Track.mp3");
        assert_eq!(c.parent, "Album");
        assert_eq!(c.grandparent, "Artist");
        assert_eq!(c.stem, "01 Track");
    }

    #[test]
    fn parse_m3u_skips_comments_and_blanks() {
        let raw = "#EXTM3U\n#EXTINF:180,Title\n\ntrack1.mp3\ntrack2.mp3\n";
        let v = parse_m3u(raw);
        assert_eq!(v, vec!["track1.mp3", "track2.mp3"]);
    }

    #[test]
    fn parse_m3u_empty() {
        assert!(parse_m3u("").is_empty());
        assert!(parse_m3u("# only comments\n").is_empty());
    }

    #[test]
    fn parse_pls_preserves_order() {
        let raw = "File2=second.mp3\nFile1=first.mp3\nFile3=third.mp3\n";
        let v = parse_pls(raw);
        assert_eq!(v, vec!["first.mp3", "second.mp3", "third.mp3"]);
    }

    #[test]
    fn parse_pls_skips_garbage() {
        let raw = "NumberOfEntries=2\nFile1=a.mp3\nrandom=stuff\nFile2=b.mp3\n";
        let v = parse_pls(raw);
        assert_eq!(v, vec!["a.mp3", "b.mp3"]);
    }
}
