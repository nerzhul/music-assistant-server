//! Musicbrainz Cover Art Archive client. Endpoint:
//! `https://coverartarchive.org/release/{mbid}` (returns JSON listing
//! available images). We pick the first "front" image and fall back
//! to any image.
//!
//! Unlike Google / iTunes, CAA returns small URLs (250px, 500px,
//! 1200px) that we can pick from directly.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MbzError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, MbzError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaaImage {
    #[serde(rename = "type")]
    pub kind: String,
    pub front: bool,
    pub approved: bool,
    pub image: String,
    pub thumbnails: CaaThumbnails,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaaThumbnails {
    pub small: Option<String>,
    pub large: Option<String>,
    #[serde(rename = "1200")]
    pub large_1200: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CaaRelease {
    pub images: Vec<CaaImage>,
}

#[derive(Clone)]
pub struct MusicbrainzCoverClient {
    http: reqwest::Client,
}

impl MusicbrainzCoverClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("MusicAssistantRust/0.1 (https://music-assistant.io)")
                .build()?,
        })
    }

    /// Look up cover art for a Musicbrainz release id. Returns the
    /// best candidate (a "front" image if any, otherwise the first
    /// image).
    pub async fn best_for_release(&self, mbid: &str) -> Result<Option<String>> {
        let url = format!("https://coverartarchive.org/release/{mbid}");
        let resp = self.http.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Ok(None);
        }
        let release: CaaRelease = resp.json().await?;
        Ok(pick_best(&release.images))
    }
}

fn pick_best(images: &[CaaImage]) -> Option<String> {
    let front = images.iter().find(|i| i.front);
    let candidate = front.or(images.first())?;
    // Prefer 1200px thumbnail if present, else full image.
    candidate
        .thumbnails
        .large_1200
        .clone()
        .or_else(|| candidate.thumbnails.large.clone())
        .or_else(|| Some(candidate.image.clone()))
}

impl Default for MusicbrainzCoverClient {
    fn default() -> Self {
        Self::new().expect("default musicbrainz client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_best_prefers_front() {
        let images = vec![
            CaaImage {
                kind: "image".into(),
                front: false,
                image: "https://x/other.jpg".into(),
                ..Default::default()
            },
            CaaImage {
                kind: "image".into(),
                front: true,
                image: "https://x/front.jpg".into(),
                thumbnails: CaaThumbnails {
                    large_1200: Some("https://x/front1200.jpg".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        ];
        let best = pick_best(&images).unwrap();
        assert_eq!(best, "https://x/front1200.jpg");
    }

    #[test]
    fn pick_best_falls_back_to_first() {
        let images = vec![CaaImage {
            image: "https://x/only.jpg".into(),
            ..Default::default()
        }];
        assert_eq!(pick_best(&images).unwrap(), "https://x/only.jpg");
    }

    #[test]
    fn pick_best_returns_none_for_empty() {
        assert!(pick_best(&[]).is_none());
    }
}
