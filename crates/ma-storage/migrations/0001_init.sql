-- Initial schema: users, auth tokens, cover art.
--
-- Both SQLite and PostgreSQL are supported. The queries that touch
-- these tables use `sqlx::Any` which rewrites type names (BLOB vs
-- BYTEA, INTEGER vs BIGINT) automatically. For schema that needs
-- backend-specific tweaks, gate the SQL behind a `-- sqlite: only`
-- or `-- postgres: only` comment; sqlx respects these.

CREATE TABLE IF NOT EXISTS users (
    user_id         TEXT PRIMARY KEY NOT NULL,
    username        TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL,
    enabled         BIGINT NOT NULL DEFAULT 1,
    display_name    TEXT,
    avatar_url      TEXT,
    password_hash   TEXT NOT NULL,
    preferences     TEXT,
    provider_filter TEXT,
    player_filter   TEXT,
    created_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS auth_tokens (
    token_id        TEXT PRIMARY KEY NOT NULL,
    user_id         TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    token_hash      TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    expires_at      TEXT,
    last_used_at    TEXT,
    is_long_lived   BIGINT NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_auth_tokens_user ON auth_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_auth_tokens_hash ON auth_tokens(token_hash);

-- Cover art cache. `provider` is the source (itunes, musicbrainz,
-- google_cse, fanart); `item_id` is the hash key the MA frontend uses
-- to construct a `/imageproxy?id=…&provider=…` URL.
CREATE TABLE IF NOT EXISTS cover_art (
    image_id        TEXT PRIMARY KEY NOT NULL,
    provider        TEXT NOT NULL,
    item_id         TEXT NOT NULL,
    url             TEXT,
    content_type    TEXT NOT NULL DEFAULT 'image/jpeg',
    width           BIGINT,
    height          BIGINT,
    bytes           BLOB,
    fetched_at      TEXT NOT NULL,
    last_used_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_cover_art_lookup ON cover_art(provider, item_id);
