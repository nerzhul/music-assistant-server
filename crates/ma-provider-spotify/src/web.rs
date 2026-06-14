//! Minimal Spotify Web API client.
//!
//! The Python provider talks to a long list of endpoints
//! (`/v1/tracks/{id}`, `/v1/albums/{id}`, `/v1/artists/{id}/albums`,
//! `/v1/search`, `/v1/me/library`, ...). Phase 3 implements the
//! minimum set: `track`, `album`, `artist`, `playlist`, `search`,
//! plus the `me/library` family. All responses are returned as
//! `serde_json::Value` so the parsers can pick the fields they need
//! without committing to a typed shape up-front.

use std::sync::Arc;

use parking_lot::RwLock;
use serde::Deserialize;
use thiserror::Error;

use crate::auth::PkceAuth;

const BASE_URL: &str = "https://api.spotify.com/v1";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth error: {0}")]
    Auth(#[from] crate::auth::AuthError),
    #[error("spotify api error: {0}")]
    Spotify(String),
    #[error("missing field: {0}")]
    MissingField(&'static str),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// Thin async wrapper around the Spotify Web API. Holds an `Arc` to
/// the `PkceAuth` helper so it can refresh access tokens transparently.
#[derive(Clone)]
pub struct SpotifyApi {
    auth: Arc<PkceAuth>,
    http: reqwest::Client,
    /// Optional dev-mode client id used to bypass the global rate limit.
    dev_auth: Arc<RwLock<Option<Arc<PkceAuth>>>>,
}

impl SpotifyApi {
    pub fn new(auth: Arc<PkceAuth>) -> Self {
        Self {
            auth,
            http: reqwest::Client::builder()
                .user_agent("MusicAssistantRust/0.1")
                .build()
                .expect("reqwest client"),
            dev_auth: Arc::new(RwLock::new(None)),
        }
    }

    /// Add a dev-mode auth (the second "session" the Python provider
    /// uses to avoid the global rate limit). Takes priority over the
    /// default auth for read-heavy endpoints.
    pub fn set_dev_auth(&self, auth: Arc<PkceAuth>) {
        *self.dev_auth.write() = Some(auth);
    }

    /// Pick the auth to use for a given call. We prefer the dev session
    /// when present; otherwise the global one.
    fn pick_auth(&self) -> Arc<PkceAuth> {
        self.dev_auth
            .read()
            .clone()
            .unwrap_or_else(|| Arc::clone(&self.auth))
    }

    /// GET a path under `/v1/...` and return the JSON body.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        self.get_with_auth(path, &self.pick_auth()).await
    }

    async fn get_with_auth(&self, path: &str, auth: &PkceAuth) -> Result<serde_json::Value> {
        let token = auth.valid_access_token().await?;
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{BASE_URL}{path}")
        };
        let resp = self.http.get(&url).bearer_auth(&token).send().await?;
        if !resp.status().is_success() {
            return Err(ApiError::Spotify(format!("GET {path} → {}", resp.status())));
        }
        Ok(resp.json().await?)
    }

    /// GET a path that may be paged (Spotify uses cursor-based paging on
    /// most list endpoints; the wrapper folds the first page only).
    pub async fn get_track(&self, track_id: &str) -> Result<serde_json::Value> {
        self.get(&format!("/tracks/{track_id}")).await
    }

    pub async fn get_album(&self, album_id: &str) -> Result<serde_json::Value> {
        self.get(&format!("/albums/{album_id}")).await
    }

    pub async fn get_artist(&self, artist_id: &str) -> Result<serde_json::Value> {
        self.get(&format!("/artists/{artist_id}")).await
    }

    pub async fn get_playlist(&self, playlist_id: &str) -> Result<serde_json::Value> {
        self.get(&format!("/playlists/{playlist_id}")).await
    }

    pub async fn get_playlist_tracks(
        &self,
        playlist_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<serde_json::Value> {
        self.get(&format!(
            "/playlists/{playlist_id}/tracks?limit={limit}&offset={offset}"
        ))
        .await
    }

    pub async fn artist_albums(&self, artist_id: &str, limit: u32) -> Result<serde_json::Value> {
        self.get(&format!("/artists/{artist_id}/albums?limit={limit}"))
            .await
    }

    pub async fn artist_top_tracks(&self, artist_id: &str) -> Result<serde_json::Value> {
        // Spotify wants a `market=` query string for top tracks.
        self.get(&format!("/artists/{artist_id}/top-tracks?market=US"))
            .await
    }

    pub async fn search(&self, q: &str, types: &[&str], limit: u32) -> Result<serde_json::Value> {
        let type_param = types.join(",");
        self.get(&format!(
            "/search?q={}&type={type_param}&limit={limit}",
            urlencoding::encode(q)
        ))
        .await
    }

    pub async fn me(&self) -> Result<serde_json::Value> {
        self.get("/me").await
    }

    pub async fn my_saved_tracks(&self, limit: u32, offset: u32) -> Result<serde_json::Value> {
        self.get(&format!("/me/tracks?limit={limit}&offset={offset}"))
            .await
    }

    pub async fn my_albums(&self, limit: u32, offset: u32) -> Result<serde_json::Value> {
        self.get(&format!("/me/albums?limit={limit}&offset={offset}"))
            .await
    }

    pub async fn my_playlists(&self, limit: u32, offset: u32) -> Result<serde_json::Value> {
        self.get(&format!("/me/playlists?limit={limit}&offset={offset}"))
            .await
    }

    /// PUT/DELETE helper for `me/library/*` toggles. The Python provider
    /// uses these for `library_*_edit` features.
    pub async fn set_my_saved_tracks(&self, track_ids: &[String], remove: bool) -> Result<()> {
        let token = self.pick_auth().valid_access_token().await?;
        let url = format!("{BASE_URL}/me/tracks");
        let mut req = self.http.request(
            if remove {
                reqwest::Method::DELETE
            } else {
                reqwest::Method::PUT
            },
            &url,
        );
        req = req
            .bearer_auth(&token)
            .json(&serde_json::json!({ "ids": track_ids }));
        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(ApiError::Spotify(format!(
                "set_my_saved_tracks → {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

/// A handful of helpers used by the parsers to pick typed values out of
/// the raw JSON shapes.
pub mod helpers {
    use serde_json::Value;

    pub fn opt_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
        v.get(key).and_then(Value::as_str)
    }

    pub fn opt_u32(v: &Value, key: &str) -> Option<u32> {
        v.get(key).and_then(Value::as_u64).map(|n| n as u32)
    }

    pub fn opt_u64(v: &Value, key: &str) -> Option<u64> {
        v.get(key).and_then(Value::as_u64)
    }

    pub fn opt_bool(v: &Value, key: &str) -> Option<bool> {
        v.get(key).and_then(Value::as_bool)
    }

    /// Pull the largest image from a Spotify `images: [...]` array.
    pub fn largest_image(v: &Value) -> Option<String> {
        let images = v.get("images")?.as_array()?;
        let best = images
            .iter()
            .filter_map(|i| {
                let url = i.get("url")?.as_str()?;
                let size = i.get("width").and_then(Value::as_u64).unwrap_or(0);
                Some((size, url))
            })
            .max_by_key(|(s, _)| *s);
        best.map(|(_, u)| u.to_string())
    }
}

/// Strongly-typed wrapper around the `Value` returned by `SpotifyApi`,
/// intended for unit tests of the parsers.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackSummary {
    pub id: String,
    pub name: String,
    pub uri: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub is_playable: bool,
    #[serde(default)]
    pub preview_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::helpers::*;

    #[test]
    fn largest_image_picks_widest() {
        let v = serde_json::json!({
            "images": [
                {"url": "http://a/small.jpg", "width": 64},
                {"url": "http://a/large.jpg", "width": 640},
                {"url": "http://a/med.jpg", "width": 300}
            ]
        });
        assert_eq!(largest_image(&v).as_deref(), Some("http://a/large.jpg"));
    }

    #[test]
    fn largest_image_falls_back_to_first_when_no_width() {
        let v = serde_json::json!({
            "images": [
                {"url": "http://a/x.jpg"}
            ]
        });
        assert_eq!(largest_image(&v).as_deref(), Some("http://a/x.jpg"));
    }

    #[test]
    fn opt_str_returns_some_when_present() {
        let v = serde_json::json!({"id": "abc"});
        assert_eq!(opt_str(&v, "id"), Some("abc"));
        assert_eq!(opt_str(&v, "missing"), None);
    }

    #[test]
    fn track_summary_deserialises_minimal_payload() {
        use super::TrackSummary;
        let v = serde_json::json!({
            "id": "track1",
            "name": "Test",
            "uri": "spotify:track:track1",
            "duration_ms": 1234
        });
        let s: TrackSummary = serde_json::from_value(v).unwrap();
        assert_eq!(s.id, "track1");
        assert_eq!(s.duration_ms, 1234);
        assert!(!s.is_local);
    }
}
