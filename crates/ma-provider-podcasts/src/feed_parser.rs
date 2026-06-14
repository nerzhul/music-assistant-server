//! RSS/Atom feed parser for podcast episodes.
//!
//! Mirrors the subset of `podcastparser` used by the Python
//! `podcastfeed` provider:
//! * `<channel><title>`, `<channel><description>`, `<channel><link>`
//! * `<channel><itunes:image href=…>`, `<channel><image><url>`
//! * `<item>` (RSS 2.0) or `<entry>` (Atom): each yields an episode
//!   with a `guid`, `title`, `description`, `pubDate` / `published`
//!   timestamp, and the first `<enclosure url=… length=… type=…>` or
//!   `<link rel="enclosure" …>` (Atom) audio URL.
//!
//! iTunes namespace tags (the `<itunes:*>` family) are read as
//! fallbacks but are not required.

use std::time::Duration;

use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use sha1_smol::Sha1;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum FeedError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("invalid xml attr: {0}")]
    XmlAttr(#[from] quick_xml::events::attributes::AttrError),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("missing feed url")]
    MissingFeedUrl,
    #[error("feed is empty")]
    EmptyFeed,
}

/// A parsed RSS/Atom podcast feed.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParsedFeed {
    pub title: String,
    pub description: String,
    pub link: String,
    pub cover_url: Option<String>,
    pub author: Option<String>,
    pub episodes: Vec<ParsedEpisode>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub description: String,
    pub published: Option<DateTime<Utc>>,
    pub duration: Option<Duration>,
    pub audio_url: Option<String>,
    pub audio_mime: Option<String>,
    pub audio_size: Option<u64>,
    pub image_url: Option<String>,
    pub author: Option<String>,
}

impl ParsedEpisode {
    pub fn is_playable(&self) -> bool {
        !self.guid.is_empty() && self.audio_url.is_some()
    }
}

pub async fn fetch_and_parse(
    feed_url: &str,
    client: &reqwest::Client,
) -> Result<ParsedFeed, FeedError> {
    if feed_url.trim().is_empty() {
        return Err(FeedError::MissingFeedUrl);
    }
    let normalized = normalize_feed_url(feed_url).ok_or(FeedError::MissingFeedUrl)?;
    let text = client
        .get(&normalized)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let feed = parse(&text, &normalized)?;
    Ok(feed)
}

pub fn normalize_feed_url(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect();
    let url = Url::parse(&s).ok()?;
    match url.scheme() {
        "http" | "https" => Some(url.into()),
        _ => None,
    }
}

pub fn feed_id(feed_url: &str) -> String {
    let mut h = Sha1::new();
    h.update(feed_url.as_bytes());
    let digest = h.digest();
    digest.to_string()[..16].to_string()
}

pub fn parse(input: &str, base_url: &str) -> Result<ParsedFeed, FeedError> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut feed = ParsedFeed::default();
    let mut current: Option<EpisodeBuilder> = None;
    let mut text_target: Option<TextTarget> = None;
    let mut in_channel_image = false;
    let mut in_channel = false;
    let mut in_image_link = false;
    let mut atom_link_rel: Option<String> = None;
    let mut atom_link_href: Option<String> = None;
    let mut atom_link_type: Option<String> = None;
    let mut atom_link_len: Option<u64> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let local = local_name(&e);
                match local.as_str() {
                    "channel" | "feed" => {
                        in_channel = true;
                    }
                    "item" | "entry" if in_channel => {
                        current = Some(EpisodeBuilder::default());
                    }
                    "image" if in_channel && current.is_none() => {
                        in_channel_image = true;
                    }
                    "enclosure" if current.is_some() => {
                        let mut url: Option<String> = None;
                        let mut mime: Option<String> = None;
                        let mut length: Option<u64> = None;
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "url" => url = Some(val),
                                "type" => mime = Some(val),
                                "length" => length = val.parse::<u64>().ok(),
                                _ => {}
                            }
                        }
                        if let Some(ep) = current.as_mut() {
                            if let Some(u) = url {
                                ep.audio_url.get_or_insert(u);
                            }
                            if let Some(m) = mime {
                                ep.audio_mime.get_or_insert(m);
                            }
                            if let Some(n) = length {
                                ep.audio_size.get_or_insert(n);
                            }
                        }
                    }
                    "link" if current.is_some() => {
                        atom_link_rel = None;
                        atom_link_href = None;
                        atom_link_type = None;
                        atom_link_len = None;
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "rel" => atom_link_rel = Some(val),
                                "href" => atom_link_href = Some(val),
                                "type" => atom_link_type = Some(val),
                                "length" => atom_link_len = val.parse::<u64>().ok(),
                                _ => {}
                            }
                        }
                    }
                    "itunes:image" | "image" if in_channel && current.is_none() => {
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if key == "href" {
                                let val = attr.unescape_value().unwrap_or_default().to_string();
                                feed.cover_url.get_or_insert(val);
                            } else if key == "url" && feed.cover_url.is_none() {
                                // RSS 2.0 <image><url>…</url></image> case handled
                                // below via the in_channel_image flag; this branch
                                // covers <image url="…"/> if any.
                                let val = attr.unescape_value().unwrap_or_default().to_string();
                                feed.cover_url.get_or_insert(val);
                            }
                        }
                    }
                    "itunes:image" | "image" if current.is_some() => {
                        if let Some(ep) = current.as_mut() {
                            for attr in e.attributes() {
                                let attr = attr?;
                                let key = std::str::from_utf8(attr.key.as_ref())
                                    .unwrap_or("")
                                    .to_ascii_lowercase();
                                if key == "href" || key == "url" {
                                    let val = attr.unescape_value().unwrap_or_default().to_string();
                                    ep.image_url.get_or_insert(val);
                                }
                            }
                        }
                    }
                    "url" if in_channel_image => {
                        in_image_link = true;
                        text_target = Some(TextTarget::ChannelImage);
                    }
                    "title" if in_channel_image => {
                        text_target = Some(TextTarget::Skip);
                    }
                    _ => {
                        if current.is_some() {
                            text_target = Some(match local.as_str() {
                                "title" => TextTarget::EpisodeTitle,
                                "description" | "summary" | "subtitle" | "content" => {
                                    TextTarget::EpisodeDescription
                                }
                                "guid" | "id" => TextTarget::EpisodeGuid,
                                "pubDate" | "published" | "date" | "updated" => {
                                    TextTarget::EpisodePubDate
                                }
                                "itunes:duration" | "duration" => TextTarget::EpisodeDuration,
                                "itunes:author" | "author" => TextTarget::EpisodeAuthor,
                                _ => TextTarget::Skip,
                            });
                        } else if in_channel {
                            text_target = Some(match local.as_str() {
                                "title" => TextTarget::ChannelTitle,
                                "description" | "subtitle" => TextTarget::ChannelDescription,
                                "link" => TextTarget::ChannelLink,
                                "itunes:author" | "author" | "managingEditor" => {
                                    TextTarget::ChannelAuthor
                                }
                                _ => TextTarget::Skip,
                            });
                        }
                    }
                }
            }
            Event::Empty(e) => {
                let local = local_name(&e);
                if local == "itunes:image" || local == "image" {
                    if current.is_some() {
                        if let Some(ep) = current.as_mut() {
                            for attr in e.attributes() {
                                let attr = attr?;
                                let key = std::str::from_utf8(attr.key.as_ref())
                                    .unwrap_or("")
                                    .to_ascii_lowercase();
                                if key == "href" {
                                    let val = attr.unescape_value().unwrap_or_default().to_string();
                                    ep.image_url.get_or_insert(val);
                                }
                            }
                        }
                    } else if in_channel {
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())
                                .unwrap_or("")
                                .to_ascii_lowercase();
                            if key == "href" {
                                let val = attr.unescape_value().unwrap_or_default().to_string();
                                feed.cover_url.get_or_insert(val);
                            }
                        }
                    }
                } else if local == "enclosure" {
                    let mut url: Option<String> = None;
                    let mut mime: Option<String> = None;
                    let mut length: Option<u64> = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        let key = std::str::from_utf8(attr.key.as_ref())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let val = attr.unescape_value().unwrap_or_default().to_string();
                        match key.as_str() {
                            "url" => url = Some(val),
                            "type" => mime = Some(val),
                            "length" => length = val.parse::<u64>().ok(),
                            _ => {}
                        }
                    }
                    if let Some(ep) = current.as_mut() {
                        if let Some(u) = url {
                            ep.audio_url.get_or_insert(u);
                        }
                        if let Some(m) = mime {
                            ep.audio_mime.get_or_insert(m);
                        }
                        if let Some(n) = length {
                            ep.audio_size.get_or_insert(n);
                        }
                    }
                } else if local == "link" {
                    // Atom enclosure can be self-closed: <link rel="enclosure" .../>
                    let mut rel: Option<String> = None;
                    let mut href: Option<String> = None;
                    let mut mime: Option<String> = None;
                    let mut length: Option<u64> = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        let key = std::str::from_utf8(attr.key.as_ref())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let val = attr.unescape_value().unwrap_or_default().to_string();
                        match key.as_str() {
                            "rel" => rel = Some(val),
                            "href" => href = Some(val),
                            "type" => mime = Some(val),
                            "length" => length = val.parse::<u64>().ok(),
                            _ => {}
                        }
                    }
                    let is_enclosure = rel.as_deref().unwrap_or("") == "enclosure"
                        || rel.is_none() && mime.is_some();
                    if is_enclosure {
                        if let Some(ep) = current.as_mut() {
                            if let Some(h) = href {
                                ep.audio_url.get_or_insert(h);
                            }
                            if let Some(m) = mime {
                                ep.audio_mime.get_or_insert(m);
                            }
                            if let Some(n) = length {
                                ep.audio_size.get_or_insert(n);
                            }
                        }
                    }
                }
            }
            Event::Text(t) => {
                if let Some(target) = text_target {
                    let s = t.unescape().unwrap_or_default().to_string();
                    apply_text(target, &s, &mut feed, &mut current);
                }
            }
            Event::CData(t) => {
                if let Some(target) = text_target {
                    let s = String::from_utf8_lossy(t.as_ref()).to_string();
                    apply_text(target, &s, &mut feed, &mut current);
                }
            }
            Event::End(e) => {
                let local = local_name_owned(&e);
                match local.as_str() {
                    "channel" | "feed" => in_channel = false,
                    "image" if in_channel_image => {
                        in_channel_image = false;
                    }
                    "url" if in_image_link => {
                        in_image_link = false;
                    }
                    "item" | "entry" => {
                        if let Some(ep) = current.take() {
                            let ep = ep.into_episode();
                            if ep.is_playable() {
                                feed.episodes.push(ep);
                            }
                        }
                    }
                    "link" if current.is_some() => {
                        // Atom enclosure: <link rel="enclosure" href="..." type="..." length="..."/>
                        let rel = atom_link_rel.as_deref().unwrap_or("");
                        if rel == "enclosure" || rel.is_empty() {
                            if let Some(ep) = current.as_mut() {
                                if let Some(href) = atom_link_href.take() {
                                    ep.audio_url.get_or_insert(href);
                                }
                                if let Some(t) = atom_link_type.take() {
                                    ep.audio_mime.get_or_insert(t);
                                }
                                if let Some(n) = atom_link_len.take() {
                                    ep.audio_size.get_or_insert(n);
                                }
                            }
                        } else {
                            atom_link_href = None;
                            atom_link_type = None;
                            atom_link_len = None;
                        }
                        atom_link_rel = None;
                    }
                    _ => {}
                }
                text_target = None;
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if let Some(ref cover) = feed.cover_url.clone() {
        if let Ok(absolute) = resolve_url(base_url, cover) {
            feed.cover_url = Some(absolute);
        }
    }
    for ep in &mut feed.episodes {
        if let Some(ref url) = ep.audio_url.clone() {
            if let Ok(absolute) = resolve_url(base_url, url) {
                ep.audio_url = Some(absolute);
            }
        }
        if let Some(ref img) = ep.image_url.clone() {
            if let Ok(absolute) = resolve_url(base_url, img) {
                ep.image_url = Some(absolute);
            }
        }
    }

    if feed.title.is_empty() && feed.episodes.is_empty() {
        return Err(FeedError::EmptyFeed);
    }
    Ok(feed)
}

fn apply_text(
    target: TextTarget,
    s: &str,
    feed: &mut ParsedFeed,
    current: &mut Option<EpisodeBuilder>,
) {
    match target {
        TextTarget::ChannelTitle => feed.title.push_str(s),
        TextTarget::ChannelDescription => feed.description.push_str(s),
        TextTarget::ChannelLink => feed.link.push_str(s),
        TextTarget::ChannelAuthor => {
            feed.author.get_or_insert_with(String::new).push_str(s);
        }
        TextTarget::ChannelImage => {
            feed.cover_url.get_or_insert(s.to_string());
        }
        TextTarget::EpisodeTitle => {
            if let Some(ep) = current.as_mut() {
                ep.title.push_str(s);
            }
        }
        TextTarget::EpisodeDescription => {
            if let Some(ep) = current.as_mut() {
                ep.description.push_str(s);
            }
        }
        TextTarget::EpisodeGuid => {
            if let Some(ep) = current.as_mut() {
                ep.guid.push_str(s);
            }
        }
        TextTarget::EpisodePubDate => {
            if let Some(ep) = current.as_mut() {
                ep.published = parse_published(s);
            }
        }
        TextTarget::EpisodeDuration => {
            if let Some(ep) = current.as_mut() {
                ep.duration = parse_duration(s);
            }
        }
        TextTarget::EpisodeAuthor => {
            if let Some(ep) = current.as_mut() {
                ep.author.get_or_insert_with(String::new).push_str(s);
            }
        }
        TextTarget::Skip => {}
    }
}

#[derive(Default)]
struct EpisodeBuilder {
    guid: String,
    title: String,
    description: String,
    published: Option<DateTime<Utc>>,
    duration: Option<Duration>,
    audio_url: Option<String>,
    audio_mime: Option<String>,
    audio_size: Option<u64>,
    image_url: Option<String>,
    author: Option<String>,
}

impl EpisodeBuilder {
    fn into_episode(self) -> ParsedEpisode {
        ParsedEpisode {
            guid: if self.guid.is_empty() {
                self.audio_url.clone().unwrap_or_default()
            } else {
                self.guid
            },
            title: self.title,
            description: self.description,
            published: self.published,
            duration: self.duration,
            audio_url: self.audio_url,
            audio_mime: self.audio_mime,
            audio_size: self.audio_size,
            image_url: self.image_url,
            author: self.author,
        }
    }
}

#[derive(Clone, Copy)]
enum TextTarget {
    ChannelTitle,
    ChannelDescription,
    ChannelLink,
    ChannelAuthor,
    ChannelImage,
    EpisodeTitle,
    EpisodeDescription,
    EpisodeGuid,
    EpisodePubDate,
    EpisodeDuration,
    EpisodeAuthor,
    Skip,
}

fn local_name(e: &quick_xml::events::BytesStart) -> String {
    let raw = e.name();
    let bytes = raw.as_ref();
    let s = std::str::from_utf8(bytes).unwrap_or("");
    strip_ns(s).to_string()
}

fn local_name_owned(e: &quick_xml::events::BytesEnd) -> String {
    let raw = e.name();
    let bytes = raw.as_ref();
    let s = std::str::from_utf8(bytes).unwrap_or("");
    strip_ns(s).to_string()
}

fn strip_ns(s: &str) -> &str {
    match s.find(':') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

fn parse_published(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(s) {
        return Some(d.with_timezone(&Utc));
    }
    None
}

fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let parts: Vec<&str> = s.split(':').collect();
    let nums: Vec<u64> = parts
        .iter()
        .filter_map(|p| p.trim().parse::<u64>().ok())
        .collect();
    if nums.is_empty() || nums.len() != parts.len() {
        return None;
    }
    let total = match nums.len() {
        3 => nums[0] * 3600 + nums[1] * 60 + nums[2],
        2 => nums[0] * 60 + nums[1],
        1 => nums[0],
        _ => return None,
    };
    Some(Duration::from_secs(total))
}

fn resolve_url(base: &str, relative: &str) -> Result<String, FeedError> {
    let base = Url::parse(base)?;
    let abs = base.join(relative)?;
    Ok(abs.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>Sample Podcast</title>
    <description>A description</description>
    <link>https://example.com</link>
    <itunes:author>Jane Doe</itunes:author>
    <itunes:image href="https://example.com/cover.jpg"/>
    <item>
      <title>Episode 1</title>
      <description>First ep</description>
      <guid>ep-1</guid>
      <pubDate>Mon, 14 Apr 2025 12:00:00 GMT</pubDate>
      <itunes:duration>00:30:00</itunes:duration>
      <enclosure url="https://example.com/ep1.mp3" length="12345" type="audio/mpeg"/>
    </item>
    <item>
      <title>Episode 2</title>
      <guid>ep-2</guid>
      <pubDate>2025-04-15T12:00:00Z</pubDate>
      <enclosure url="https://example.com/ep2.mp3" length="22222" type="audio/mpeg"/>
    </item>
    <item>
      <title>Broken</title>
      <guid>ep-3</guid>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_basic_rss() {
        let f = parse(SAMPLE, "https://example.com/feed.xml").unwrap();
        assert_eq!(f.title, "Sample Podcast");
        assert_eq!(f.description, "A description");
        assert_eq!(f.link, "https://example.com");
        assert_eq!(f.author.as_deref(), Some("Jane Doe"));
        assert_eq!(
            f.cover_url.as_deref(),
            Some("https://example.com/cover.jpg")
        );
        assert_eq!(f.episodes.len(), 2);
        let ep1 = &f.episodes[0];
        assert_eq!(ep1.guid, "ep-1");
        assert_eq!(ep1.title, "Episode 1");
        assert_eq!(
            ep1.audio_url.as_deref(),
            Some("https://example.com/ep1.mp3")
        );
        assert_eq!(ep1.audio_size, Some(12345));
        assert_eq!(ep1.duration, Some(Duration::from_secs(30 * 60)));
        assert!(ep1.published.is_some());
    }

    #[test]
    fn drops_episodes_without_audio() {
        let f = parse(SAMPLE, "https://example.com/feed.xml").unwrap();
        assert!(f.episodes.iter().all(|e| e.audio_url.is_some()));
    }

    #[test]
    fn parse_duration_formats() {
        assert_eq!(parse_duration("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("01:30"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("01:02:03"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("garbage"), None);
    }

    #[test]
    fn parse_published_formats() {
        assert!(parse_published("Mon, 14 Apr 2025 12:00:00 GMT").is_some());
        assert!(parse_published("2025-04-15T12:00:00Z").is_some());
        assert!(parse_published("garbage").is_none());
    }

    #[test]
    fn normalize_rejects_non_http() {
        assert!(normalize_feed_url("ftp://x").is_none());
        assert!(normalize_feed_url("file:///etc/passwd").is_none());
        assert!(normalize_feed_url("https://example.com/feed").is_some());
    }

    #[test]
    fn feed_id_stable() {
        assert_eq!(feed_id("https://a/feed"), feed_id("https://a/feed"));
        assert_ne!(feed_id("https://a/feed"), feed_id("https://b/feed"));
    }

    #[test]
    fn parses_atom_minimal() {
        let atom = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Pod</title>
  <link href="https://example.org/"/>
  <entry>
    <id>tag:example.org,2025:ep1</id>
    <title>Ep 1</title>
    <link rel="enclosure" type="audio/mpeg" href="https://example.org/ep1.mp3" length="4321"/>
    <published>2025-05-01T00:00:00Z</published>
  </entry>
</feed>"#;
        let f = parse(atom, "https://example.org/feed.atom").unwrap();
        assert_eq!(f.title, "Atom Pod");
        assert_eq!(f.episodes.len(), 1);
        assert_eq!(f.episodes[0].guid, "tag:example.org,2025:ep1");
        assert_eq!(
            f.episodes[0].audio_url.as_deref(),
            Some("https://example.org/ep1.mp3")
        );
    }
}
