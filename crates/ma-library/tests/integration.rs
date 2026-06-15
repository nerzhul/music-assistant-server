//! Integration tests for `LibraryController`.
//!
//! Builds a tiny filesystem tree with a couple of audio files
//! (synthesized on the fly: a minimal FLAC magic + ID3 header is
//! not enough for lofty to parse, so we mostly exercise the
//! "scanned but unparseable" path; for the parseable path we use a
//! mock-style approach via `upsert_track_from_metadata`).

use std::path::PathBuf;

use ma_library::controller::{IndexOptions, LibraryController};
use ma_storage::LibraryRepository;

async fn build_pool() -> sqlx::Pool<sqlx::any::Any> {
    sqlx::any::install_default_drivers();
    let tmp = std::env::temp_dir().join(format!(
        "ma-library-test-{}.db",
        std::process::id() as u64
            ^ std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
    ));
    let url = format!("sqlite://{}?mode=rwc", tmp.display());
    let opts: sqlx::any::AnyConnectOptions = url.parse().expect("valid url");
    sqlx::any::AnyPoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect")
}

async fn migrate(p: &sqlx::Pool<sqlx::any::Any>) {
    let m = include_str!("../../ma-storage/migrations/0002_library.sql");
    let cleaned: String = m
        .lines()
        .map(|l| {
            let trimmed = l.trim_start();
            if trimmed.starts_with("--") {
                String::new()
            } else if let Some(idx) = l.find("--") {
                format!("{}\n", &l[..idx])
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let stmts: Vec<String> = cleaned
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for s in &stmts {
        sqlx::query::<sqlx::any::Any>(s)
            .execute(p)
            .await
            .expect("migration");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn index_empty_dir_succeeds() {
    let pool = build_pool().await;
    migrate(&pool).await;
    let repo = LibraryRepository::new(pool.clone());
    let ctrl = LibraryController::new(repo);

    let tmp = tempfile::tempdir().unwrap();
    let summary = ctrl
        .index_filesystem(tmp.path(), &IndexOptions::default())
        .await
        .unwrap();
    assert_eq!(summary.tracks_parsed, 0);
    assert_eq!(summary.tracks_persisted, 0);
    assert_eq!(summary.files_scanned, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn index_skips_unparseable_audio() {
    // A `.flac` file that lofty can't parse (just a few zero
    // bytes) should be scanned but counted as `tracks_failed`
    // (lofty returns None).
    let pool = build_pool().await;
    migrate(&pool).await;
    let repo = LibraryRepository::new(pool.clone());
    let ctrl = LibraryController::new(repo);

    let tmp = tempfile::tempdir().unwrap();
    let track_path: PathBuf = tmp.path().join("a.flac");
    tokio::fs::write(&track_path, b"not really flac")
        .await
        .unwrap();

    let summary = ctrl
        .index_filesystem(tmp.path(), &IndexOptions::default())
        .await
        .unwrap();
    assert_eq!(summary.files_scanned, 1);
    assert_eq!(summary.tracks_parsed, 0);
    assert_eq!(summary.tracks_persisted, 0);
    assert_eq!(summary.tracks_failed, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upsert_track_from_metadata_persists() {
    // Use the lower-level helper to write a track without going
    // through the filesystem scan.
    use ma_core::enums::AlbumType;
    use ma_provider_filesystem::parser::ParsedTrack;
    use ma_providers::media::{Album, Artist, Track};

    let pool = build_pool().await;
    migrate(&pool).await;
    let repo = LibraryRepository::new(pool.clone());
    let ctrl = LibraryController::new(repo.clone());

    let artist = Artist {
        item_id: ma_core::identifiers::MediaItemId("a1".into()),
        provider: "filesystem_local".into(),
        name: "Test Artist".into(),
        ..Default::default()
    };
    let album = Album {
        item_id: ma_core::identifiers::MediaItemId("al1".into()),
        provider: "filesystem_local".into(),
        name: "Test Album".into(),
        album_type: AlbumType::Album,
        year: Some(2024),
        artists: vec![artist.clone()],
        ..Default::default()
    };
    let track = Track {
        item_id: ma_core::identifiers::MediaItemId("t1".into()),
        provider: "filesystem_local".into(),
        name: "Test Song".into(),
        duration: Some(180.0),
        artists: vec![artist],
        album: Some(album),
        track_number: Some(1),
        disc_number: Some(1),
        uri: "x".into(),
        ..Default::default()
    };
    let parsed = ParsedTrack {
        track,
        content_type: ma_core::enums::ContentType::Flac,
        sample_rate: 44100,
        channels: 2,
        bit_depth: 16,
        genre: Some("Rock".into()),
        bit_rate: Some(900),
    };
    ctrl.upsert_track_from_metadata(
        &parsed,
        "filesystem_local",
        "filesystem_local",
        "/music/test.flac",
    )
    .await
    .unwrap();
    // Lookup via the provider mapping.
    let local_id = repo
        .lookup_provider_mapping("track", "filesystem_local", "/music/test.flac")
        .await
        .unwrap();
    assert!(local_id.is_some());
    // The track, album, and artist rows exist.
    let track_count = repo.count_tracks().await.unwrap();
    assert_eq!(track_count, 1);
    let mapping_count = repo.count_provider_mappings().await.unwrap();
    assert_eq!(mapping_count, 1);
    // Case-insensitive search finds "test song".
    let hits = repo.search_tracks("Test Song", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Test Song");
    // Album + artist exist.
    let album_hits: Vec<_> = {
        let row: Option<i64> =
            sqlx::query_scalar::<_, i64>("SELECT item_id FROM albums WHERE name = 'Test Album'")
                .fetch_optional(&pool)
                .await
                .unwrap();
        row.into_iter().collect()
    };
    assert_eq!(album_hits.len(), 1);
    let artist_hits: Vec<_> = {
        let row: Option<i64> =
            sqlx::query_scalar::<_, i64>("SELECT item_id FROM artists WHERE name = 'Test Artist'")
                .fetch_optional(&pool)
                .await
                .unwrap();
        row.into_iter().collect()
    };
    assert_eq!(artist_hits.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_index_does_not_duplicate_tracks() {
    // Upsert the same track twice — the unique index on
    // `(provider_instance, provider_item_id)` should keep the row
    // count at 1.
    use ma_core::enums::AlbumType;
    use ma_provider_filesystem::parser::ParsedTrack;
    use ma_providers::media::{Album, Artist, Track};

    let pool = build_pool().await;
    migrate(&pool).await;
    let repo = LibraryRepository::new(pool.clone());
    let ctrl = LibraryController::new(repo.clone());

    let artist = Artist {
        item_id: ma_core::identifiers::MediaItemId("a1".into()),
        provider: "filesystem_local".into(),
        name: "Artist".into(),
        ..Default::default()
    };
    let album = Album {
        item_id: ma_core::identifiers::MediaItemId("al1".into()),
        provider: "filesystem_local".into(),
        name: "Album".into(),
        album_type: AlbumType::Album,
        artists: vec![artist.clone()],
        ..Default::default()
    };
    let track = Track {
        item_id: ma_core::identifiers::MediaItemId("t1".into()),
        provider: "filesystem_local".into(),
        name: "Song".into(),
        artists: vec![artist],
        album: Some(album),
        uri: "x".into(),
        ..Default::default()
    };
    let parsed = ParsedTrack {
        track,
        content_type: ma_core::enums::ContentType::Mp3,
        sample_rate: 44100,
        channels: 2,
        bit_depth: 16,
        genre: None,
        bit_rate: Some(320),
    };
    for _ in 0..3 {
        ctrl.upsert_track_from_metadata(
            &parsed,
            "filesystem_local",
            "filesystem_local",
            "/music/dup.mp3",
        )
        .await
        .unwrap();
    }
    assert_eq!(repo.count_tracks().await.unwrap(), 1);
    assert_eq!(repo.count_provider_mappings().await.unwrap(), 1);
}
