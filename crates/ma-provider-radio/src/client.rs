//! RadioBrowser REST API client.
//!
//! Implements the subset of the `radios` library the MA provider uses:
//! search, list by popularity / votes, list by country / language / tag,
//! plus single-station lookup. The on-the-wire JSON shape matches
//! `api.radio-browser.info` (see https://de1.api.radio-browser.info/).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

const DEFAULT_USER_AGENT: &str = concat!(
    "MusicAssistantRust/",
    env!("CARGO_PKG_VERSION"),
    " (https://music-assistant.io)"
);

const DEFAULT_BASE_URL: &str = "https://de1.api.radio-browser.info";

#[derive(Debug, Error)]
pub enum RadioBrowserError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("radio-browser api error: {0}")]
    Api(String),
    #[error("station not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, RadioBrowserError>;

/// One radio-browser station record. Field names mirror the JSON
/// (the API uses inconsistent naming — `stationuuid` / `clickcount`
/// / `lastcheckok` have no underscore, but `url_resolved` does).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    pub stationuuid: String,
    pub name: String,
    pub url: String,
    pub url_resolved: String,
    pub homepage: String,
    pub favicon: String,
    pub tags: String,
    pub country: String,
    pub countrycode: String,
    pub language: String,
    pub codec: String,
    pub bitrate: u32,
    pub votes: u32,
    pub clickcount: u32,
    pub hls: u8,
    pub lastcheckok: u8,
    pub lastchecktime: String,
}

/// Rust-side alias: `Station::click_count` → JSON `clickcount`.
/// Provided so the rest of the crate doesn't have to remember the API
/// quirk.
impl Station {
    pub fn click_count(&self) -> u32 {
        self.clickcount
    }
}

/// Sort order for the `stations` endpoint. Mirrors the `radios` library's
/// `Order` enum, with the underlying string the API expects.
#[derive(Debug, Clone, Copy)]
pub enum Order {
    Name,
    Tags,
    Country,
    Language,
    Votes,
    ClickCount,
    Bitrate,
    LastCheckOk,
}

impl Order {
    fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Tags => "tags",
            Self::Country => "country",
            Self::Language => "language",
            Self::Votes => "votes",
            Self::ClickCount => "clickcount",
            Self::Bitrate => "bitrate",
            Self::LastCheckOk => "lastcheckok",
        }
    }
}

/// Async RadioBrowser client. Cheap to clone (the inner `reqwest::Client`
/// is itself a thread-safe pool).
#[derive(Clone)]
pub struct RadioBrowserClient {
    pub http: reqwest::Client,
    pub base_url: Arc<String>,
}

impl RadioBrowserClient {
    pub fn new() -> Result<Self> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .build()?;
        Ok(Self {
            http,
            base_url: Arc::new(base_url.trim_end_matches('/').to_string()),
        })
    }

    /// List stations. Mirrors the `radios.stations(...)` call.
    pub async fn stations(
        &self,
        order: Order,
        reverse: bool,
        limit: u32,
        hide_broken: bool,
    ) -> Result<Vec<Station>> {
        let url = format!("{}/json/stations", self.base_url);
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("order", order.as_str()),
                ("reverse", if reverse { "true" } else { "false" }),
                ("limit", &limit.to_string()),
                ("hidebroken", if hide_broken { "true" } else { "false" }),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        let stations: Vec<Station> = resp.json().await?;
        Ok(stations)
    }

    /// Search by name. Mirrors the `radios.search(name=...)` call.
    pub async fn search(&self, name: &str, limit: u32) -> Result<Vec<Station>> {
        let url = format!("{}/json/stations/search", self.base_url);
        let resp = self
            .http
            .get(&url)
            .query(&[("name", name), ("limit", &limit.to_string())])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        let stations: Vec<Station> = resp.json().await?;
        Ok(stations)
    }

    /// Look up a single station by UUID.
    pub async fn station(&self, uuid: &str) -> Result<Station> {
        let url = format!("{}/json/stations/byuuid/{}", self.base_url, uuid);
        let resp = self.http.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RadioBrowserError::NotFound(uuid.into()));
        }
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        let station: Station = resp.json().await?;
        Ok(station)
    }

    /// Get a list of countries (cached in the Python provider for 7
    /// days). Returned shape: `[{name, iso_3166_1, stationcount}]`.
    pub async fn countries(&self) -> Result<Vec<Country>> {
        let url = format!("{}/json/countries", self.base_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        Ok(resp.json().await?)
    }

    /// Get a list of languages.
    pub async fn languages(&self) -> Result<Vec<Language>> {
        let url = format!("{}/json/languages", self.base_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        Ok(resp.json().await?)
    }

    /// Get a list of tags.
    pub async fn tags(&self) -> Result<Vec<Tag>> {
        let url = format!("{}/json/tags", self.base_url);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(RadioBrowserError::Api(resp.status().to_string()));
        }
        Ok(resp.json().await?)
    }

    /// Record a click (the Python provider calls this when a station is
    /// played, so the radio-browser popularity metric works).
    pub async fn station_click(&self, uuid: &str) -> Result<()> {
        let url = format!("{}/json/url/{}", self.base_url, uuid);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            warn!(uuid, status = %resp.status(), "station_click failed");
        }
        Ok(())
    }
}

impl Default for RadioBrowserClient {
    fn default() -> Self {
        Self::new().expect("default radio-browser client")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Country {
    pub name: String,
    #[serde(rename = "iso_3166_1")]
    pub iso_3166_1: String,
    pub stationcount: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Language {
    pub name: String,
    pub iso_639: String,
    pub stationcount: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Tag {
    pub name: String,
    pub stationcount: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_strings_match_api() {
        assert_eq!(Order::Name.as_str(), "name");
        assert_eq!(Order::ClickCount.as_str(), "clickcount");
        assert_eq!(Order::LastCheckOk.as_str(), "lastcheckok");
    }

    #[test]
    fn station_deserialises_snake_case() {
        // The radio-browser API uses `clickcount` (no underscore).
        let json = r#"{
            "stationuuid": "abc-123",
            "name": "Test FM",
            "url": "http://example.com/stream",
            "url_resolved": "http://example.com/stream",
            "codec": "MP3",
            "bitrate": 192,
            "clickcount": 1234,
            "tags": "rock,jazz"
        }"#;
        let station: Station = serde_json::from_str(json).unwrap();
        assert_eq!(station.stationuuid, "abc-123");
        assert_eq!(station.bitrate, 192);
        assert_eq!(station.click_count(), 1234);
    }

    #[test]
    fn station_round_trip() {
        let s = Station {
            stationuuid: "x".into(),
            name: "Test".into(),
            url: "http://x".into(),
            bitrate: 128,
            ..Default::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["stationuuid"], "x");
        let back: Station = serde_json::from_value(v).unwrap();
        assert_eq!(back.bitrate, 128);
    }
}
