-- Library schema: tracks, albums, artists, playlists, radios, audiobooks,
-- podcasts, genres, and the supporting join tables.
--
-- All `*_id` columns are `BIGINT` so the same SQL works against both
-- SQLite (INTEGER is 8 bytes in modern SQLite) and PostgreSQL
-- (BIGINT). `metadata` and `external_ids` are `TEXT` holding JSON
-- because sqlx's `Any` driver doesn't support the native JSON type
-- in a portable way; callers serialize/deserialize via serde_json.
--
-- The `search_name` / `search_sort_name` columns are the
-- lowercased, diacritics-stripped variants of `name` / `sort_name`
-- used for case-insensitive search (the Python equivalent
-- `library.helpers.create_safe_string` produces the same value).

CREATE TABLE IF NOT EXISTS tracks (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    version              TEXT,
    duration             BIGINT,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS albums (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    version              TEXT,
    album_type           TEXT NOT NULL,
    year                 BIGINT,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artists (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS playlists (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    owner                TEXT NOT NULL,
    is_editable          BIGINT NOT NULL,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL,
    supported_mediatypes TEXT NOT NULL DEFAULT '["track"]',
    is_dynamic           BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS radios (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audiobooks (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    version              TEXT,
    favorite             BIGINT NOT NULL DEFAULT 0,
    publisher            TEXT,
    duration             BIGINT,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS podcasts (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    version              TEXT,
    favorite             BIGINT NOT NULL DEFAULT 0,
    publisher            TEXT,
    total_episodes       BIGINT NOT NULL,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS genres (
    item_id              BIGINT PRIMARY KEY,
    name                 TEXT NOT NULL,
    sort_name            TEXT NOT NULL,
    translation_key      TEXT,
    description          TEXT,
    favorite             BIGINT NOT NULL DEFAULT 0,
    metadata             TEXT NOT NULL DEFAULT '{}',
    external_ids         TEXT NOT NULL DEFAULT '[]',
    genre_aliases        TEXT NOT NULL DEFAULT '[]',
    play_count           BIGINT NOT NULL DEFAULT 0,
    last_played          BIGINT NOT NULL DEFAULT 0,
    timestamp_added      TEXT NOT NULL,
    timestamp_modified   TEXT NOT NULL,
    search_name          TEXT NOT NULL,
    search_sort_name     TEXT NOT NULL,
    is_excluded          BIGINT NOT NULL DEFAULT 0,
    is_default           BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS genre_media_item_mapping (
    genre_id     BIGINT NOT NULL,
    media_id     BIGINT NOT NULL,
    media_type   TEXT NOT NULL,
    alias        TEXT,
    is_derived   BIGINT NOT NULL DEFAULT 0,
    is_manual    BIGINT NOT NULL DEFAULT 0,
    UNIQUE(genre_id, media_id, media_type)
);

CREATE TABLE IF NOT EXISTS genre_media_item_exclusion (
    genre_id     BIGINT NOT NULL,
    media_id     BIGINT NOT NULL,
    media_type   TEXT NOT NULL,
    UNIQUE(genre_id, media_id, media_type)
);

CREATE TABLE IF NOT EXISTS album_tracks (
    id            BIGINT PRIMARY KEY,
    track_id      BIGINT NOT NULL,
    album_id      BIGINT NOT NULL,
    disc_number   BIGINT NOT NULL,
    track_number  BIGINT NOT NULL,
    UNIQUE(track_id, album_id)
);

CREATE TABLE IF NOT EXISTS track_artists (
    track_id     BIGINT NOT NULL,
    artist_id    BIGINT NOT NULL,
    UNIQUE(track_id, artist_id)
);

CREATE TABLE IF NOT EXISTS album_artists (
    album_id     BIGINT NOT NULL,
    artist_id    BIGINT NOT NULL,
    UNIQUE(album_id, artist_id)
);

CREATE TABLE IF NOT EXISTS provider_mappings (
    media_type         TEXT NOT NULL,
    item_id            BIGINT NOT NULL,
    provider_domain    TEXT NOT NULL,
    provider_instance  TEXT NOT NULL,
    provider_item_id   TEXT NOT NULL,
    available          BIGINT NOT NULL DEFAULT 1,
    in_library         BIGINT NOT NULL DEFAULT 0,
    is_unique          BIGINT,
    url                TEXT,
    audio_format       TEXT,
    details            TEXT,
    UNIQUE(media_type, provider_instance, provider_item_id)
);

CREATE TABLE IF NOT EXISTS playlog (
    id              BIGINT PRIMARY KEY,
    item_id         TEXT NOT NULL,
    provider        TEXT NOT NULL,
    media_type      TEXT NOT NULL,
    name            TEXT NOT NULL,
    image           TEXT,
    timestamp       BIGINT NOT NULL DEFAULT 0,
    fully_played    BIGINT,
    seconds_played  BIGINT,
    userid          TEXT NOT NULL,
    queue_id        TEXT,
    user_initiated  BIGINT NOT NULL DEFAULT 1,
    UNIQUE(item_id, provider, media_type, userid)
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT,
    type  TEXT
);

-- Indexes (mirror the Python `__create_database_indexes`).
CREATE INDEX IF NOT EXISTS tracks_favorite_idx          ON tracks(favorite);
CREATE INDEX IF NOT EXISTS tracks_name_idx              ON tracks(name);
CREATE INDEX IF NOT EXISTS tracks_search_name_idx       ON tracks(search_name);
CREATE INDEX IF NOT EXISTS tracks_sort_name_idx         ON tracks(sort_name);
CREATE INDEX IF NOT EXISTS tracks_search_sort_name_idx  ON tracks(search_sort_name);

CREATE INDEX IF NOT EXISTS albums_favorite_idx          ON albums(favorite);
CREATE INDEX IF NOT EXISTS albums_name_idx              ON albums(name);
CREATE INDEX IF NOT EXISTS albums_search_name_idx       ON albums(search_name);
CREATE INDEX IF NOT EXISTS albums_sort_name_idx         ON albums(sort_name);
CREATE INDEX IF NOT EXISTS albums_search_sort_name_idx  ON albums(search_sort_name);

CREATE INDEX IF NOT EXISTS artists_favorite_idx         ON artists(favorite);
CREATE INDEX IF NOT EXISTS artists_name_idx             ON artists(name);
CREATE INDEX IF NOT EXISTS artists_search_name_idx      ON artists(search_name);
CREATE INDEX IF NOT EXISTS artists_sort_name_idx        ON artists(sort_name);
CREATE INDEX IF NOT EXISTS artists_search_sort_name_idx ON artists(search_sort_name);

CREATE INDEX IF NOT EXISTS playlists_favorite_idx          ON playlists(favorite);
CREATE INDEX IF NOT EXISTS playlists_name_idx              ON playlists(name);
CREATE INDEX IF NOT EXISTS playlists_search_name_idx       ON playlists(search_name);
CREATE INDEX IF NOT EXISTS playlists_sort_name_idx         ON playlists(sort_name);
CREATE INDEX IF NOT EXISTS playlists_search_sort_name_idx  ON playlists(search_sort_name);

CREATE INDEX IF NOT EXISTS radios_favorite_idx          ON radios(favorite);
CREATE INDEX IF NOT EXISTS radios_name_idx              ON radios(name);
CREATE INDEX IF NOT EXISTS radios_search_name_idx       ON radios(search_name);
CREATE INDEX IF NOT EXISTS radios_sort_name_idx         ON radios(sort_name);
CREATE INDEX IF NOT EXISTS radios_search_sort_name_idx  ON radios(search_sort_name);

CREATE INDEX IF NOT EXISTS audiobooks_favorite_idx          ON audiobooks(favorite);
CREATE INDEX IF NOT EXISTS audiobooks_name_idx              ON audiobooks(name);
CREATE INDEX IF NOT EXISTS audiobooks_search_name_idx       ON audiobooks(search_name);
CREATE INDEX IF NOT EXISTS audiobooks_sort_name_idx         ON audiobooks(sort_name);
CREATE INDEX IF NOT EXISTS audiobooks_search_sort_name_idx  ON audiobooks(search_sort_name);

CREATE INDEX IF NOT EXISTS podcasts_favorite_idx          ON podcasts(favorite);
CREATE INDEX IF NOT EXISTS podcasts_name_idx              ON podcasts(name);
CREATE INDEX IF NOT EXISTS podcasts_search_name_idx       ON podcasts(search_name);
CREATE INDEX IF NOT EXISTS podcasts_sort_name_idx         ON podcasts(sort_name);
CREATE INDEX IF NOT EXISTS podcasts_search_sort_name_idx  ON podcasts(search_sort_name);

CREATE INDEX IF NOT EXISTS genres_favorite_idx          ON genres(favorite);
CREATE INDEX IF NOT EXISTS genres_name_idx              ON genres(name);
CREATE INDEX IF NOT EXISTS genres_search_name_idx       ON genres(search_name);
CREATE INDEX IF NOT EXISTS genres_sort_name_idx         ON genres(sort_name);
CREATE INDEX IF NOT EXISTS genres_search_sort_name_idx  ON genres(search_sort_name);

CREATE INDEX IF NOT EXISTS album_tracks_album_idx  ON album_tracks(album_id);
CREATE INDEX IF NOT EXISTS album_tracks_track_idx  ON album_tracks(track_id);

CREATE INDEX IF NOT EXISTS track_artists_track_idx  ON track_artists(track_id);
CREATE INDEX IF NOT EXISTS track_artists_artist_idx ON track_artists(artist_id);

CREATE INDEX IF NOT EXISTS album_artists_album_idx  ON album_artists(album_id);
CREATE INDEX IF NOT EXISTS album_artists_artist_idx ON album_artists(artist_id);

CREATE INDEX IF NOT EXISTS provider_mappings_lookup_idx
    ON provider_mappings(media_type, provider_instance, provider_item_id);
CREATE INDEX IF NOT EXISTS provider_mappings_local_idx
    ON provider_mappings(media_type, item_id);

CREATE INDEX IF NOT EXISTS genre_mapping_genre_idx ON genre_media_item_mapping(genre_id);
CREATE INDEX IF NOT EXISTS genre_mapping_media_idx ON genre_media_item_mapping(media_id);

CREATE INDEX IF NOT EXISTS playlog_user_idx    ON playlog(userid);
CREATE INDEX IF NOT EXISTS playlog_item_idx    ON playlog(item_id, provider, media_type);
CREATE INDEX IF NOT EXISTS playlog_timestamp_idx ON playlog(timestamp);
