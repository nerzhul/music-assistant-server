//! Google Custom Search JSON API client (paid, requires an API key +
//! Custom Search Engine id). Endpoint:
//! `https://www.googleapis.com/customsearch/v1?key=...&cx=...&q=...&searchType=image`
//!
//! Phase 3 implements the wire shape and the image picker; the actual
//! network call is feature-gated behind `GoogleConfig::enabled` so
//! tests can run without API credentials.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use ma_core::enums::ImageType;

#[derive(Debug, Error)]
pub enum GoogleError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("google cse not configured (missing api_key or cx)")]
    NotConfigured,
}

pub type Result<T> = std::result::Result<T, GoogleError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GoogleImage {
    pub link: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GoogleResponse {
    pub items: Vec<GoogleImage>,
}

#[derive(Debug, Clone, Default)]
pub struct GoogleConfig {
    pub api_key: Option<String>,
    pub cx: Option<String>,
}

impl GoogleConfig {
    pub fn enabled(&self) -> bool {
        self.api_key.is_some() && self.cx.is_some()
    }
}

#[derive(Clone)]
pub struct GoogleClient {
    pub http: reqwest::Client,
    pub config: GoogleConfig,
}

impl GoogleClient {
    pub fn new(config: GoogleConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("MusicAssistantRust/0.1")
                .build()
                .expect("reqwest client"),
            config,
        }
    }

    pub async fn search_artwork(
        &self,
        artist: &str,
        album: &str,
        limit: u32,
    ) -> Result<Vec<GoogleImage>> {
        if !self.config.enabled() {
            return Err(GoogleError::NotConfigured);
        }
        let q = format!("{artist} {album} album cover art");
        let url = "https://www.googleapis.com/customsearch/v1";
        let resp = self
            .http
            .get(url)
            .query(&[
                ("key", self.config.api_key.as_deref().unwrap_or("")),
                ("cx", self.config.cx.as_deref().unwrap_or("")),
                ("q", q.as_str()),
                ("searchType", "image"),
                ("num", &limit.to_string()),
                ("safe", "active"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let r: GoogleResponse = resp.json().await?;
        Ok(r.items)
    }

    /// Map a Google result to the `ImageType` enum based on the
    /// `mime` field. Google always returns `image/jpeg` / `image/png`
    /// / etc.
    pub fn image_type_for(item: &GoogleImage) -> ImageType {
        match item.mime.as_deref().unwrap_or("") {
            "image/jpeg" | "image/jpg" => ImageType::Thumb,
            "image/png" => ImageType::Thumb,
            "image/webp" => ImageType::Thumb,
            _ => ImageType::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_config_enabled_requires_both_fields() {
        assert!(!GoogleConfig::default().enabled());
        let c = GoogleConfig {
            api_key: Some("k".into()),
            cx: None,
        };
        assert!(!c.enabled());
        let c = GoogleConfig {
            api_key: Some("k".into()),
            cx: Some("cx".into()),
        };
        assert!(c.enabled());
    }

    #[test]
    fn image_type_for_recognises_jpeg_png() {
        let j = GoogleImage {
            mime: Some("image/jpeg".into()),
            ..Default::default()
        };
        let p = GoogleImage {
            mime: Some("image/png".into()),
            ..Default::default()
        };
        assert_eq!(GoogleClient::image_type_for(&j), ImageType::Thumb);
        assert_eq!(GoogleClient::image_type_for(&p), ImageType::Thumb);
    }
}
