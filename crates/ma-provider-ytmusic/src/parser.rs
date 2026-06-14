//! Map `yt-dlp -J` JSON output to our flat MA media types.
//!
//! The mapping is intentionally lossy: yt-dlp dumps *everything*
//! YouTube knows about the resource (chapters, storyboards, automatic
//! captions, …). We only pick the fields that surface in the MA
//! library / player / search UI.

use ma_providers::media::{Album, Artist, Playlist, Track};

use crate::yt_dlp::{FormatInfo, VideoInfo};

/// Convert a single video `VideoInfo` to a MA `Track`. The
/// `provider` and `domain` arguments are filled in by the caller
/// (the `YTMusicProvider` instance) so we don't need to thread a
/// self-reference through.
pub fn track_from_video(info: &VideoInfo, provider: &str, domain: &str) -> Option<Track> {
    if info.id.is_empty() {
        return None;
    }
    let artist = Artist {
        item_id: ma_core::identifiers::MediaItemId(info.uploader_id.clone()),
        provider: provider.to_string(),
        name: info.uploader.clone(),
        ..Default::default()
    };
    let album = Album {
        item_id: ma_core::identifiers::MediaItemId(format!("album:{}", info.uploader_id)),
        provider: provider.to_string(),
        name: info.uploader.clone(),
        artists: vec![artist.clone()],
        image_url: best_thumbnail(info),
        ..Default::default()
    };
    let image = best_thumbnail(info);
    let track_id = ma_core::identifiers::MediaItemId(info.id.clone());
    Some(Track {
        item_id: track_id,
        provider: provider.to_string(),
        name: info.title.clone(),
        duration: info.duration,
        artists: vec![artist],
        album: Some(album),
        track_number: None,
        disc_number: None,
        isrc: None,
        image_url: image,
        uri: format!("{domain}://{provider}/track/{}", info.id),
    })
}

/// Best-resolution thumbnail, if any. We don't trust the
/// `preference` / `width` fields to be sorted.
pub fn best_thumbnail(info: &VideoInfo) -> Option<String> {
    info.thumbnails
        .iter()
        .filter(|t| t.url.starts_with("http"))
        .max_by_key(|t| t.width.unwrap_or(0) as u64 * t.height.unwrap_or(0) as u64)
        .map(|t| t.url.clone())
}

/// Pick a single audio-only format. Falls back to the first format
/// in the list if no audio-only one is found.
pub fn best_audio_format(formats: &[FormatInfo]) -> Option<&FormatInfo> {
    let audio_only = formats
        .iter()
        .filter(|f| !f.url.is_empty() && (f.vcodec.is_empty() || f.vcodec == "none"))
        .max_by(|a, b| {
            let a_score = a.abr.unwrap_or(0.0)
                + if a.asr.unwrap_or(0) >= 44100 {
                    1.0
                } else {
                    0.0
                };
            let b_score = b.abr.unwrap_or(0.0)
                + if b.asr.unwrap_or(0) >= 44100 {
                    1.0
                } else {
                    0.0
                };
            a_score
                .partial_cmp(&b_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    audio_only.or_else(|| formats.iter().find(|f| !f.url.is_empty()))
}

/// Convert a `VideoInfo` that turned out to be a playlist / album
/// (i.e. `entries.is_some()`) into a `Playlist`. We use the
/// `webpage_url` as the item id.
pub fn playlist_from_info(info: &VideoInfo, provider: &str, domain: &str) -> Option<Playlist> {
    let entries = info.entries.as_ref()?;
    let id = if !info.id.is_empty() {
        info.id.clone()
    } else {
        info.webpage_url.clone()
    };
    if id.is_empty() {
        return None;
    }
    let count = info.playlist_count.unwrap_or(entries.len() as u32);
    let image = best_thumbnail(info);
    Some(Playlist {
        item_id: ma_core::identifiers::MediaItemId(id),
        provider: provider.to_string(),
        name: info.title.clone(),
        owner: Some(info.uploader.clone()),
        is_editable: false,
        track_count: count,
        image_url: image,
        uri: format!("{domain}://{provider}/playlist/{}", info.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yt_dlp::{VideoInfo, YtdlpThumbnail};

    fn make_track_info() -> VideoInfo {
        VideoInfo {
            id: "dQw4w9WgXcQ".into(),
            title: "Never Gonna Give You Up".into(),
            ext: "m4a".into(),
            uploader: "Rick Astley".into(),
            uploader_id: "UCuAXFkgsw1L7xaCfnd5JJOw".into(),
            duration: Some(213.0),
            webpage_url: "https://music.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            url: "https://example.com/audio.m4a".into(),
            thumbnails: vec![YtdlpThumbnail {
                url: "https://example.com/120.jpg".into(),
                width: Some(120),
                height: Some(90),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn parses_track() {
        let info = make_track_info();
        let t = track_from_video(&info, "ytmusic", "ytmusic").unwrap();
        assert_eq!(t.name, "Never Gonna Give You Up");
        assert_eq!(t.duration, Some(213.0));
        assert_eq!(t.artists.len(), 1);
        assert_eq!(t.artists[0].name, "Rick Astley");
        assert_eq!(t.image_url.as_deref(), Some("https://example.com/120.jpg"));
    }

    #[test]
    fn rejects_blank_id() {
        let mut info = make_track_info();
        info.id.clear();
        assert!(track_from_video(&info, "ytmusic", "ytmusic").is_none());
    }

    #[test]
    fn best_audio_format_picks_audio_only() {
        let formats = vec![
            FormatInfo {
                format_id: "video".into(),
                ext: "mp4".into(),
                url: "https://x/video.mp4".into(),
                acodec: "aac".into(),
                vcodec: "h264".into(),
                abr: Some(128.0),
                ..Default::default()
            },
            FormatInfo {
                format_id: "audio".into(),
                ext: "m4a".into(),
                url: "https://x/audio.m4a".into(),
                acodec: "aac".into(),
                vcodec: "none".into(),
                abr: Some(160.0),
                asr: Some(48000),
                ..Default::default()
            },
        ];
        let chosen = best_audio_format(&formats).unwrap();
        assert_eq!(chosen.format_id, "audio");
    }

    #[test]
    fn playlist_from_info_uses_id() {
        let info = VideoInfo {
            id: "PLABC".into(),
            title: "My Mix".into(),
            entries: Some(vec![]),
            webpage_url: "https://music.youtube.com/playlist?list=PLABC".into(),
            ..make_track_info()
        };
        let p = playlist_from_info(&info, "ytmusic", "ytmusic").unwrap();
        assert_eq!(p.name, "My Mix");
        assert_eq!(p.track_count, 0);
    }
}
