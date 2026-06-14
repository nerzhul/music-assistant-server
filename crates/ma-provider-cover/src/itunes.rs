//! iTunes Search API client (no API key, JSON). Endpoint:
//! `https://itunes.apple.com/search?term=...&entity=album&limit=...`

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ItunesError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, ItunesError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ItunesArtwork {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ItunesResult {
    #[serde(rename = "collectionName")]
    pub collection_name: String,
    #[serde(rename = "artistName")]
    pub artist_name: String,
    #[serde(rename = "artworkUrl100")]
    pub artwork_url_100: String,
}

#[derive(Clone)]
pub struct ItunesClient {
    pub http: reqwest::Client,
}

impl ItunesClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("MusicAssistantRust/0.1")
                .build()?,
        })
    }

    /// Search for album artwork. Returns up to `limit` candidates.
    pub async fn search_artwork(
        &self,
        artist: &str,
        album: &str,
        limit: u32,
    ) -> Result<Vec<ItunesResult>> {
        let term = format!("{artist} {album}");
        let url = "https://itunes.apple.com/search";
        let resp = self
            .http
            .get(url)
            .query(&[
                ("term", term.as_str()),
                ("entity", "album"),
                ("limit", &limit.to_string()),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        #[derive(Deserialize)]
        struct Resp {
            results: Vec<ItunesResult>,
        }
        let r: Resp = resp.json().await?;
        Ok(r.results)
    }

    /// Pick the highest-resolution artwork URL from an iTunes result.
    /// iTunes serves `100x100` thumbnails by default; the URL is
    /// templated so we can ask for any size by replacing the segment.
    pub fn high_res_url(result: &ItunesResult) -> Option<String> {
        if result.artwork_url_100.is_empty() {
            return None;
        }
        // Replace `100x100bb.jpg` with `600x600bb.jpg`. The standard
        // iTunes shape is `{base}/100x100bb.jpg`.
        let upsized = result.artwork_url_100.replace("100x100bb", "600x600bb");
        Some(upsized)
    }
}

impl Default for ItunesClient {
    fn default() -> Self {
        Self::new().expect("default iTunes client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_res_replaces_100x100() {
        let r = ItunesResult {
            collection_name: "Album".into(),
            artist_name: "Artist".into(),
            artwork_url_100: "https://is1.example.com/100x100bb.jpg".into(),
        };
        let url = ItunesClient::high_res_url(&r).unwrap();
        assert!(url.contains("600x600bb.jpg"));
    }

    #[test]
    fn high_res_returns_none_when_empty() {
        let r = ItunesResult::default();
        assert!(ItunesClient::high_res_url(&r).is_none());
    }
}
