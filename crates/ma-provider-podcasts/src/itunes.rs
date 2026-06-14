//! iTunes Podcast Directory client.
//!
//! Mirrors the subset of `music_assistant/providers/itunes_podcasts/`
//! needed for the search + top-podcasts endpoints:
//!
//! * `GET https://itunes.apple.com/search?media=podcast&entity=podcast`
//!   – search by title
//! * `GET https://itunes.apple.com/us/rss/toppodcasts/limit=NN/genre=ID/json`
//!   – top charts (used for the recommendations folder)
//!
//! Response shape (trimmed to what we need) is the on-the-wire JSON
//! from the public search API. Field aliases use camelCase
//! (`collectionId`, `feedUrl`, `artworkUrl600`, …).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_USER_AGENT: &str = concat!(
    "MusicAssistantRust/",
    env!("CARGO_PKG_VERSION"),
    " (https://music-assistant.io)"
);

pub const SEARCH_URL: &str = "https://itunes.apple.com/search";
pub const TOP_PODCASTS_URL: &str =
    "https://itunes.apple.com/us/rss/toppodcasts/limit/{limit}/genre={genre}/json";

#[derive(Debug, Error)]
pub enum ITunesError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("iTunes api error: {0}")]
    Api(String),
}

pub type Result<T> = std::result::Result<T, ITunesError>;

/// Build a configured `reqwest::Client` with a sensible timeout and
/// the MA user-agent.
pub fn build_client() -> std::result::Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ITunesSearchResults {
    #[serde(rename = "resultCount")]
    pub result_count: u32,
    pub results: Vec<PodcastSearchResult>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PodcastSearchResult {
    #[serde(rename = "collectionId")]
    pub collection_id: Option<i64>,
    pub kind: Option<String>,
    #[serde(rename = "artistName")]
    pub artist_name: Option<String>,
    #[serde(rename = "collectionName")]
    pub collection_name: Option<String>,
    #[serde(rename = "collectionCensoredName")]
    pub collection_censored_name: Option<String>,
    #[serde(rename = "trackName")]
    pub track_name: Option<String>,
    #[serde(rename = "trackCensoredName")]
    pub track_censored_name: Option<String>,
    #[serde(rename = "feedUrl")]
    pub feed_url: Option<String>,
    #[serde(rename = "artworkUrl30")]
    pub artwork_url_30: Option<String>,
    #[serde(rename = "artworkUrl60")]
    pub artwork_url_60: Option<String>,
    #[serde(rename = "artworkUrl100")]
    pub artwork_url_100: Option<String>,
    #[serde(rename = "artworkUrl600")]
    pub artwork_url_600: Option<String>,
    #[serde(rename = "releaseDate")]
    pub release_date: Option<String>,
    #[serde(rename = "trackCount")]
    pub track_count: Option<u32>,
    #[serde(rename = "primaryGenreName")]
    pub primary_genre_name: Option<String>,
    pub genres: Vec<String>,
}

impl PodcastSearchResult {
    /// The best-quality artwork we have, in priority order.
    pub fn best_artwork(&self) -> Option<&str> {
        self.artwork_url_600
            .as_deref()
            .or(self.artwork_url_100.as_deref())
            .or(self.artwork_url_60.as_deref())
            .or(self.artwork_url_30.as_deref())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchParams {
    pub term: String,
    pub country: String,
    pub explicit: bool,
    pub limit: u32,
}

impl SearchParams {
    pub fn new(term: impl Into<String>, country: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            country: country.into(),
            explicit: true,
            limit: 25,
        }
    }
}

/// Run a search. `params.country` is a 2-letter ISO country code (e.g.
/// `"US"`, `"FR"`); the directory uses it to scope results to a store
/// front.
pub async fn search(
    client: &reqwest::Client,
    params: &SearchParams,
) -> Result<Vec<PodcastSearchResult>> {
    let mut q = vec![
        ("media", "podcast".to_string()),
        ("entity", "podcast".to_string()),
        ("country", params.country.to_ascii_uppercase()),
        ("attribute", "titleTerm".to_string()),
        (
            "explicit",
            if params.explicit { "Yes" } else { "No" }.to_string(),
        ),
        ("limit", params.limit.clamp(1, 200).to_string()),
        ("term", params.term.clone()),
    ];
    q.retain(|(_, v)| !v.is_empty());

    let resp = client.get(SEARCH_URL).query(&q).send().await?;
    if !resp.status().is_success() {
        return Err(ITunesError::Api(format!(
            "itunes search returned status {}",
            resp.status()
        )));
    }
    let body: ITunesSearchResults = resp.json().await?;
    Ok(body.results)
}

/// The iTunes Top Podcasts RSS returns a different shape — we use the
/// JSON variant of the feed rather than the XML one (the Python
/// provider parses the same shape).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopPodcastsResponse {
    pub feed: Option<TopPodcastsFeed>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopPodcastsFeed {
    pub country: Option<String>,
    pub results: Vec<TopPodcastEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopPodcastEntry {
    #[serde(rename = "artistName")]
    pub artist_name: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "artworkUrl60")]
    pub artwork_url_60: Option<String>,
    #[serde(rename = "artworkUrl100")]
    pub artwork_url_100: Option<String>,
}

/// Fetch the iTunes top podcasts for a country. `limit` is clamped to
/// `[1, 200]`, `genre` is a numeric genre id (`"26"` for podcasts,
/// `"1301"` for education, …).
pub async fn top_podcasts(
    client: &reqwest::Client,
    country: &str,
    limit: u32,
    genre: &str,
) -> Result<Vec<TopPodcastEntry>> {
    let url = TOP_PODCASTS_URL
        .replace("{limit}", &limit.clamp(1, 200).to_string())
        .replace("{genre}", genre);
    let resp = client
        .get(url)
        .query(&[("country", country.to_ascii_uppercase())])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(ITunesError::Api(format!(
            "itunes top podcasts returned status {}",
            resp.status()
        )));
    }
    let body: TopPodcastsResponse = resp.json().await?;
    Ok(body.feed.map(|f| f.results).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_search_response() {
        let raw = json!({
            "resultCount": 1,
            "results": [
                {
                    "collectionId": 12345,
                    "kind": "podcast",
                    "artistName": "Studio X",
                    "trackName": "Daily Pod",
                    "feedUrl": "https://example.com/feed.xml",
                    "artworkUrl600": "https://example.com/600.jpg",
                    "artworkUrl100": "https://example.com/100.jpg",
                    "trackCount": 100,
                    "primaryGenreName": "Technology"
                }
            ]
        });
        let s: ITunesSearchResults = serde_json::from_value(raw).unwrap();
        assert_eq!(s.result_count, 1);
        assert_eq!(s.results.len(), 1);
        let r = &s.results[0];
        assert_eq!(r.collection_id, Some(12345));
        assert_eq!(r.feed_url.as_deref(), Some("https://example.com/feed.xml"));
        assert_eq!(r.best_artwork(), Some("https://example.com/600.jpg"));
    }

    #[test]
    fn best_artwork_falls_back() {
        let r = PodcastSearchResult {
            artwork_url_100: Some("https://x/100.jpg".into()),
            ..Default::default()
        };
        assert_eq!(r.best_artwork(), Some("https://x/100.jpg"));
        let r = PodcastSearchResult {
            artwork_url_30: Some("https://x/30.jpg".into()),
            ..Default::default()
        };
        assert_eq!(r.best_artwork(), Some("https://x/30.jpg"));
        assert_eq!(PodcastSearchResult::default().best_artwork(), None);
    }

    #[test]
    fn parses_top_podcasts_response() {
        let raw = json!({
            "feed": {
                "country": "US",
                "results": [
                    {"artistName": "X", "id": "1", "name": "Top", "url": "https://itunes/x"}
                ]
            }
        });
        let s: TopPodcastsResponse = serde_json::from_value(raw).unwrap();
        let feed = s.feed.unwrap();
        assert_eq!(feed.results.len(), 1);
        assert_eq!(feed.results[0].id.as_deref(), Some("1"));
    }
}
