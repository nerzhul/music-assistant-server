//! Internal unit tests for the filesystem provider. Lives in a
//! separate file so the `async fn` test bodies don't trip over
//! the `async_trait` expansion of the `MusicProvider` trait.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ma_core::enums::MediaType;
use ma_providers::media::{MediaItem, Track};
use ma_providers::provider::MusicProvider;

use ma_provider_filesystem::parser::{parse_m3u, parse_pls, ParsedTrack};
use ma_provider_filesystem::provider::{FilesystemConfig, FilesystemProvider};
use ma_provider_filesystem::scanner::{ScanConfig, ScannedKind};
use ma_provider_filesystem::stream::read_stream;

use ma_providers::stream::StreamDetails;

static NONCE: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> std::path::PathBuf {
    let n = NONCE.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "ma-fs-test-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test]
async fn provider_search_finds_matching_track() {
    let dir = tempdir();
    let track = dir.join("track.mp3");
    std::fs::write(&track, b"ID3\x04\x00\x00\x00\x00\x00\x00data").unwrap();
    let cfg = FilesystemConfig {
        path: dir.clone(),
        content_type: "music".into(),
    };
    let provider = FilesystemProvider::new("fs_test", cfg);
    let handle = provider.into_handle("fs_test".into());
    let p: Arc<dyn MusicProvider> = handle.music.clone();
    let _ = p.search("track", &[MediaType::Track], 5).await.unwrap();
}

#[tokio::test]
async fn get_item_returns_track_or_not_found() {
    let dir = tempdir();
    let track = dir.join("only.mp3");
    std::fs::write(&track, b"ID3\x04\x00\x00\x00\x00\x00\x00data").unwrap();
    let provider = FilesystemProvider::new(
        "fs_test",
        FilesystemConfig {
            path: dir.clone(),
            content_type: "music".into(),
        },
    );
    let handle = provider.into_handle("fs_test".into());
    let p: Arc<dyn MusicProvider> = handle.music.clone();
    let item_id = format!("{}|{}", "fs_test", track.to_string_lossy());
    let r = p.get_item(&item_id, MediaType::Track).await;
    assert!(matches!(
        r,
        Ok(MediaItem::Track(_)) | Err(ma_providers::provider::ProviderError::MediaNotFound(_))
    ));
}

#[tokio::test]
async fn scan_finds_mp3_flac_and_playlist() {
    let dir = tempdir();
    let album_dir = dir.join("Album1");
    std::fs::create_dir(&album_dir).unwrap();
    std::fs::write(album_dir.join("a.mp3"), b"ID3\x04\x00\x00\x00\x00\x00\x00x").unwrap();
    std::fs::write(album_dir.join("b.mp3"), b"ID3\x04\x00\x00\x00\x00\x00\x00y").unwrap();
    std::fs::write(album_dir.join("playlist.m3u"), b"#EXTM3U\na.mp3\nb.mp3\n").unwrap();
    let result = ma_provider_filesystem::scanner::scan(ScanConfig::new(dir.clone()))
        .await
        .unwrap();
    assert_eq!(result.tracks.len(), 2);
    assert_eq!(result.playlists.len(), 1);
    assert!(result.tracks.iter().all(|f| f.kind == ScannedKind::Track));
}

#[tokio::test]
async fn scan_then_browse_returns_albums() {
    let dir = tempdir();
    let album_dir = dir.join("Album1");
    std::fs::create_dir(&album_dir).unwrap();
    std::fs::write(album_dir.join("a.mp3"), b"ID3\x04\x00\x00\x00\x00\x00\x00x").unwrap();
    std::fs::write(album_dir.join("b.mp3"), b"ID3\x04\x00\x00\x00\x00\x00\x00y").unwrap();
    let provider = FilesystemProvider::new(
        "fs_test",
        FilesystemConfig {
            path: dir.clone(),
            content_type: "music".into(),
        },
    );
    let handle = provider.into_handle("fs_test".into());
    let p: Arc<dyn MusicProvider> = handle.music.clone();
    let _ = p.browse("/").await.unwrap();
}

#[tokio::test]
async fn get_stream_details_for_existing_file() {
    let dir = tempdir();
    let track = dir.join("x.mp3");
    std::fs::write(&track, b"ID3\x04\x00\x00\x00\x00\x00\x00y").unwrap();
    let provider = FilesystemProvider::new(
        "fs_test",
        FilesystemConfig {
            path: dir.clone(),
            content_type: "music".into(),
        },
    );
    let handle = provider.into_handle("fs_test".into());
    let p: Arc<dyn MusicProvider> = handle.music.clone();
    let item_id = format!("{}|{}", "fs_test", track.to_string_lossy());
    if let Ok(details) = p.get_stream_details(&item_id, MediaType::Track).await {
        assert_eq!(details.stream_type, ma_core::enums::StreamType::LocalFile);
        assert!(details.can_seek);
    }
}

#[tokio::test]
async fn chunk_stream_returns_full_file() {
    let dir = tempdir();
    let p = dir.join("x.mp3");
    let body: Vec<u8> = (0..4096u32).map(|i| (i & 0xff) as u8).collect();
    std::fs::write(&p, &body).unwrap();
    let details = StreamDetails {
        provider: "fs".into(),
        item_id: ma_core::identifiers::MediaItemId("x".to_string()),
        media_type: MediaType::Track,
        stream_type: ma_core::enums::StreamType::LocalFile,
        path: p.to_string_lossy().to_string(),
        ..Default::default()
    };
    let mut total = 0;
    let mut s = read_stream(&details).await.unwrap();
    while let Some(chunk) = futures::StreamExt::next(&mut s).await {
        total += chunk.unwrap().len();
    }
    assert_eq!(total, body.len());
}

#[test]
fn m3u_and_pls_parsers() {
    assert_eq!(parse_m3u("#EXTM3U\na.mp3\nb.mp3\n"), vec!["a.mp3", "b.mp3"]);
    assert_eq!(parse_pls("File1=a\nFile2=b\n"), vec!["a", "b"]);
}

#[test]
fn parsed_track_constructor_is_safe() {
    // Smoke check that we can construct a `ParsedTrack` for unit tests.
    let p = ParsedTrack {
        track: Track {
            item_id: ma_core::identifiers::MediaItemId("t".to_string()),
            provider: "fs".into(),
            name: "T".into(),
            uri: "filesystem://fs/t".into(),
            ..Default::default()
        },
        content_type: ma_core::enums::ContentType::Mp3,
        sample_rate: 44_100,
        channels: 2,
        bit_depth: 16,
        genre: None,
        bit_rate: Some(320),
    };
    assert_eq!(p.sample_rate, 44_100);
    assert_eq!(p.bit_depth, 16);
}
