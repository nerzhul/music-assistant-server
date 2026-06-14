//! Wrapper around the `yt-dlp` binary.
//!
//! The wrapper is intentionally thin: we only call `yt-dlp` for two
//! things:
//!
//! 1. `extract_info` — dumps the metadata + format list for a single
//!    video (or a playlist) as JSON via `-J`. The JSON is then
//!    decoded by `parser` into our flat MA media types.
//! 2. `extract_url` — dumps only the resolved direct media URL via
//!    `-g` (one URL per format). Used by the stream controller
//!    when it wants a direct HTTP fetch instead of ffmpeg-ingest.
//!
//! The `yt-dlp` binary is auto-discovered on `PATH`, but the path
//! can be overridden via `MA_YTMUSIC_YT_DLP_PATH` (matching the
//! `MA_SPOTIFY_LIBRESPOT_PATH` convention used elsewhere).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum YtdlpError {
    #[error("yt-dlp binary not found (set MA_YTMUSIC_YT_DLP_PATH or install yt-dlp)")]
    NotFound,
    #[error("yt-dlp exited with code {0}: {1}")]
    NonZeroExit(i32, String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("yt-dlp returned no url for {0}")]
    NoUrl(String),
    #[error("yt-dlp timed out after {0:?}")]
    Timeout(Duration),
    #[error("cookie file is required for authenticated content (set MA_YTMUSIC_COOKIE_FILE)")]
    NoCookie,
}

pub type Result<T> = std::result::Result<T, YtdlpError>;

/// Subset of `yt-dlp -J` JSON we care about. Field names match the
/// `yt-dlp` output verbatim. Unused fields are ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    #[serde(rename = "ext")]
    pub ext: String,
    pub uploader: String,
    #[serde(rename = "uploader_id")]
    pub uploader_id: String,
    pub duration: Option<f64>,
    pub webpage_url: String,
    pub url: String,
    pub thumbnails: Vec<YtdlpThumbnail>,
    pub description: String,
    /// `playlist_count` and `entries` are populated for playlist / album URLs.
    pub playlist_count: Option<u32>,
    pub entries: Option<Vec<VideoInfo>>,
    pub formats: Option<Vec<FormatInfo>>,
    /// `yt-dlp` extractor key (e.g. `"Youtube"`, `"YoutubeTab"`). Used
    /// by `search()` to bucket hits into tracks / playlists.
    #[serde(rename = "ie_key")]
    pub ie_key: Option<String>,
    /// `yt-dlp` per-entry kind (`"video"`, `"playlist"`, `"url"`, …).
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct YtdlpThumbnail {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FormatInfo {
    pub format_id: String,
    pub ext: String,
    pub url: String,
    pub acodec: String,
    pub vcodec: String,
    pub abr: Option<f64>,
    pub vbr: Option<f64>,
    pub tbr: Option<f64>,
    pub asr: Option<u32>,
    pub filesize: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct YtdlpOptions {
    /// Path to the `yt-dlp` binary. If `None`, we look for it on
    /// `$PATH` via `which`.
    pub binary: Option<PathBuf>,
    /// Optional path to a `cookies.txt` file (Netscape format). If
    /// `None`, the provider refuses to serve authenticated content.
    pub cookie_file: Option<PathBuf>,
    /// PO token (visitor data blob) used for the "bot" checks. The
    /// token is passed through to `yt-dlp` via `--extractor-args`.
    pub po_token: Option<String>,
    /// Optional `visitor_data` companion string to the PO token.
    pub visitor_data: Option<String>,
    /// Optional player_client override (`web`, `android`, `ios`).
    pub player_client: Option<String>,
    /// Hard timeout for a single `yt-dlp` invocation.
    pub timeout: Duration,
}

impl Default for YtdlpOptions {
    fn default() -> Self {
        Self {
            binary: None,
            cookie_file: None,
            po_token: None,
            visitor_data: None,
            player_client: None,
            timeout: Duration::from_secs(60),
        }
    }
}

impl YtdlpOptions {
    /// Read options from the MA process environment. Unknown
    /// variables default to "not set".
    pub fn from_env() -> Self {
        let mut opts = Self::default();
        if let Ok(p) = std::env::var("MA_YTMUSIC_YT_DLP_PATH") {
            if !p.is_empty() {
                opts.binary = Some(PathBuf::from(p));
            }
        }
        if let Ok(p) = std::env::var("MA_YTMUSIC_COOKIE_FILE") {
            if !p.is_empty() {
                opts.cookie_file = Some(PathBuf::from(p));
            }
        }
        if let Ok(t) = std::env::var("MA_YTMUSIC_PO_TOKEN") {
            if !t.is_empty() {
                opts.po_token = Some(t);
            }
        }
        if let Ok(v) = std::env::var("MA_YTMUSIC_VISITOR_DATA") {
            if !v.is_empty() {
                opts.visitor_data = Some(v);
            }
        }
        if let Ok(c) = std::env::var("MA_YTMUSIC_PLAYER_CLIENT") {
            if !c.is_empty() {
                opts.player_client = Some(c);
            }
        }
        if let Ok(t) = std::env::var("MA_YTMUSIC_TIMEOUT_SECS") {
            if let Ok(s) = t.parse::<u64>() {
                opts.timeout = Duration::from_secs(s);
            }
        }
        opts
    }
}

pub struct Ytdlp {
    binary: PathBuf,
    opts: YtdlpOptions,
}

impl Ytdlp {
    /// Build a `Ytdlp` by locating the binary. Returns
    /// [`YtdlpError::NotFound`] if the binary is not on `PATH` and
    /// `MA_YTMUSIC_YT_DLP_PATH` is not set.
    pub async fn new(opts: YtdlpOptions) -> Result<Self> {
        let binary = match opts.binary.clone() {
            Some(p) => p,
            None => locate_ytdlp().await?,
        };
        Ok(Self { binary, opts })
    }

    pub fn with_binary(binary: PathBuf, opts: YtdlpOptions) -> Self {
        Self { binary, opts }
    }

    fn build_command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--no-warnings")
            .arg("--no-progress")
            .arg("--no-color")
            .arg("--no-playlist");
        if let Some(ref cookies) = self.opts.cookie_file {
            cmd.arg("--cookies").arg(cookies);
        }
        if self.opts.po_token.is_some() || self.opts.player_client.is_some() {
            let mut parts = Vec::new();
            if let Some(ref t) = self.opts.po_token {
                parts.push(format!("po_token=web.player+{t}"));
            }
            if let Some(ref v) = self.opts.visitor_data {
                parts.push(format!("visitor_data={v}"));
            }
            if let Some(ref c) = self.opts.player_client {
                parts.push(format!("player_client={c}"));
            }
            cmd.arg("--extractor-args")
                .arg(format!("youtube:{}", parts.join(";")));
        }
        cmd
    }

    /// Build a command that allows playlists/searches to return
    /// `entries`. Used by `search()` and the multi-track case in
    /// `extract_info` when the URL resolves to a playlist.
    fn build_command_multi(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--no-warnings")
            .arg("--no-progress")
            .arg("--no-color");
        // No `--no-playlist` here: playlist / search responses come
        // through as `entries` and we want them.
        if let Some(ref cookies) = self.opts.cookie_file {
            cmd.arg("--cookies").arg(cookies);
        }
        if self.opts.po_token.is_some() || self.opts.player_client.is_some() {
            let mut parts = Vec::new();
            if let Some(ref t) = self.opts.po_token {
                parts.push(format!("po_token=web.player+{t}"));
            }
            if let Some(ref v) = self.opts.visitor_data {
                parts.push(format!("visitor_data={v}"));
            }
            if let Some(ref c) = self.opts.player_client {
                parts.push(format!("player_client={c}"));
            }
            cmd.arg("--extractor-args")
                .arg(format!("youtube:{}", parts.join(";")));
        }
        cmd
    }

    /// Run `yt-dlp -J <url>` and decode the resulting JSON.
    pub async fn extract_info(&self, url: &str) -> Result<VideoInfo> {
        if self.opts.cookie_file.is_none() {
            return Err(YtdlpError::NoCookie);
        }
        let mut cmd = self.build_command();
        cmd.arg("-J").arg(url);
        let output = run_with_timeout(cmd, self.opts.timeout).await?;
        if !output.status.success() {
            return Err(YtdlpError::NonZeroExit(
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let info: VideoInfo = serde_json::from_slice(&output.stdout)?;
        Ok(info)
    }

    /// Run `yt-dlp -g <url>` and return the resolved direct media URL
    /// (the first non-empty line of stdout).
    #[allow(dead_code)]
    pub async fn extract_url(&self, url: &str) -> Result<String> {
        if self.opts.cookie_file.is_none() {
            return Err(YtdlpError::NoCookie);
        }
        let mut cmd = self.build_command();
        cmd.arg("-g").arg(url);
        let output = run_with_timeout(cmd, self.opts.timeout).await?;
        if !output.status.success() {
            return Err(YtdlpError::NonZeroExit(
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let mut s = String::from_utf8_lossy(&output.stdout).to_string();
        if let Some(idx) = s.find('\n') {
            s.truncate(idx);
        }
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            return Err(YtdlpError::NoUrl(url.to_string()));
        }
        Ok(trimmed)
    }

    /// Run `yt-dlp -J --flat-playlist "ytsearch<N>:<query>"` and
    /// return the resulting JSON. The top-level `entries` (when
    /// present) contains the search hits as flat `VideoInfo`s.
    ///
    /// We use the `--flat-playlist` form for speed: each entry only
    /// carries the metadata `yt-dlp` returns without a full
    /// per-video metadata fetch. The provider then calls
    /// `extract_info` on the picked `id` to resolve the actual
    /// stream.
    pub async fn search(&self, query: &str, limit: u32) -> Result<VideoInfo> {
        if query.trim().is_empty() {
            return Err(YtdlpError::NoUrl("empty query".to_string()));
        }
        let limit = limit.clamp(1, 50);
        let url = format!("ytsearch{limit}:{query}");
        let mut cmd = self.build_command_multi();
        cmd.arg("-J").arg("--flat-playlist").arg(&url);
        let output = run_with_timeout(cmd, self.opts.timeout).await?;
        if !output.status.success() {
            return Err(YtdlpError::NonZeroExit(
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let info: VideoInfo = serde_json::from_slice(&output.stdout)?;
        Ok(info)
    }

    /// Run `yt-dlp -J -f bestaudio/best --no-playlist` and pick the
    /// direct media URL from the first non-empty format URL. This is
    /// a single-call equivalent of `extract_info` + `extract_url`
    /// that we use from `get_stream_details`.
    pub async fn extract_best_audio(&self, url: &str) -> Result<BestAudioResult> {
        if self.opts.cookie_file.is_none() {
            return Err(YtdlpError::NoCookie);
        }
        let mut cmd = self.build_command();
        cmd.arg("-J").arg("-f").arg("bestaudio/best").arg(url);
        let output = run_with_timeout(cmd, self.opts.timeout).await?;
        if !output.status.success() {
            return Err(YtdlpError::NonZeroExit(
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let info: VideoInfo = serde_json::from_slice(&output.stdout)?;
        let chosen = info
            .formats
            .as_deref()
            .and_then(best_audio_format_field)
            .ok_or_else(|| YtdlpError::NoUrl(url.to_string()))?;
        Ok(BestAudioResult {
            direct_url: chosen.url.clone(),
            ext: chosen.ext.clone(),
            format_id: chosen.format_id.clone(),
            duration: info.duration,
            title: info.title.clone(),
            uploader: info.uploader.clone(),
            sample_rate: chosen.asr,
            bit_rate: chosen.abr.map(|x| x as u32),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct BestAudioResult {
    pub direct_url: String,
    pub ext: String,
    pub format_id: String,
    pub duration: Option<f64>,
    pub title: String,
    pub uploader: String,
    pub sample_rate: Option<u32>,
    pub bit_rate: Option<u32>,
}

/// Pick the best audio-only format. Re-exported from
/// [`crate::parser::best_audio_format`].
pub(crate) fn best_audio_format_field(formats: &[FormatInfo]) -> Option<&FormatInfo> {
    crate::parser::best_audio_format(formats)
}

#[derive(Default)]
struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<CommandOutput> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (mut stdout_r, mut stderr_r) = (stdout, stderr);

    let out_fut = async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stdout_r.take() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    };
    let err_fut = async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stderr_r.take() {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    };
    let (status_res, out_buf, err_buf) = match tokio::time::timeout(timeout, async {
        let (s, o, e) = tokio::join!(child.wait(), out_fut, err_fut);
        (s, o, e)
    })
    .await
    {
        Ok(t) => t,
        Err(_) => {
            // Best effort: kill the child before returning the error.
            let _ = child.start_kill();
            return Err(YtdlpError::Timeout(timeout));
        }
    };
    let status = status_res?;
    Ok(CommandOutput {
        status,
        stdout: out_buf,
        stderr: err_buf,
    })
}

async fn locate_ytdlp() -> Result<PathBuf> {
    // Try a few well-known names.
    for name in ["yt-dlp", "yt_dlp", "youtube-dl"] {
        let which = if cfg!(windows) { "where" } else { "which" };
        let out = Command::new(which).arg(name).output().await;
        if let Ok(out) = out {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout);
                let p = Path::new(path.trim()).to_path_buf();
                if p.exists() {
                    debug!(?p, "yt-dlp auto-located");
                    return Ok(p);
                }
            }
        }
    }
    warn!("yt-dlp not found in PATH; set MA_YTMUSIC_YT_DLP_PATH to override");
    Err(YtdlpError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_from_env_unset() {
        // No env vars set; all fields stay at defaults.
        let _ = YtdlpOptions::from_env();
    }

    #[test]
    fn extractor_args_string_is_well_formed() {
        let opts = YtdlpOptions {
            po_token: Some("abc".into()),
            visitor_data: Some("vd".into()),
            player_client: Some("web".into()),
            ..Default::default()
        };
        let ytdlp = Ytdlp {
            binary: PathBuf::from("/bin/yt-dlp"),
            opts,
        };
        let mut cmd = ytdlp.build_command();
        let args: Vec<String> = cmd
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--extractor-args".to_string()));
        let idx = args.iter().position(|a| a == "--extractor-args").unwrap();
        let value = &args[idx + 1];
        assert!(value.contains("po_token=web.player+abc"));
        assert!(value.contains("visitor_data=vd"));
        assert!(value.contains("player_client=web"));
    }

    #[test]
    fn deserializes_search_response_with_entries() {
        // Typical `yt-dlp -J --flat-playlist "ytsearch5:Nirvana"` output.
        let raw = serde_json::json!({
            "_type": "playlist",
            "id": "ytsearch5:Nirvana",
            "title": "ytsearch5:Nirvana",
            "entries": [
                {
                    "id": "dQw4w9WgXcQ",
                    "title": "Never Gonna Give You Up",
                    "uploader": "Rick Astley",
                    "uploader_id": "UCuAXFkgsw1L7xaCfnd5JJOw",
                    "duration": 213.0,
                    "ie_key": "Youtube",
                    "kind": "video",
                    "thumbnails": [{"url": "https://x/120.jpg", "width": 120, "height": 90}]
                },
                {
                    "id": "PLabc",
                    "title": "Nirvana Greatest Hits",
                    "uploader": "Nirvana",
                    "ie_key": "YoutubeTab",
                    "kind": "playlist",
                    "thumbnails": []
                }
            ]
        });
        let info: VideoInfo = serde_json::from_value(raw).unwrap();
        let entries = info.entries.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ie_key.as_deref(), Some("Youtube"));
        assert_eq!(entries[0].kind.as_deref(), Some("video"));
        assert_eq!(entries[1].ie_key.as_deref(), Some("YoutubeTab"));
        assert_eq!(entries[1].kind.as_deref(), Some("playlist"));
    }

    #[test]
    fn best_audio_format_field_picks_highest_abr() {
        let formats = vec![
            FormatInfo {
                format_id: "video".into(),
                ext: "mp4".into(),
                url: "https://x/video.mp4".into(),
                acodec: "aac".into(),
                vcodec: "h264".into(),
                abr: Some(128.0),
                ..Default::default()
            },
            FormatInfo {
                format_id: "audio_160".into(),
                ext: "m4a".into(),
                url: "https://x/audio_160.m4a".into(),
                acodec: "aac".into(),
                vcodec: "none".into(),
                abr: Some(160.0),
                asr: Some(48000),
                ..Default::default()
            },
            FormatInfo {
                format_id: "audio_128".into(),
                ext: "m4a".into(),
                url: "https://x/audio_128.m4a".into(),
                acodec: "aac".into(),
                vcodec: "none".into(),
                abr: Some(128.0),
                asr: Some(44100),
                ..Default::default()
            },
        ];
        let chosen = best_audio_format_field(&formats).unwrap();
        assert_eq!(chosen.format_id, "audio_160");
    }
}
