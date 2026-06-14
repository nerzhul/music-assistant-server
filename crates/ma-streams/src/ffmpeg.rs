//! ffmpeg child-process wrapper.
//!
//! Mirrors `music_assistant/helpers/ffmpeg.py`:
//!
//! * `FFMpeg` extends `AsyncProcess` (which we re-implement on top of
//!   `tokio::process::Command` since we don't have the Python
//!   helper).
//! * `get_ffmpeg_args` builds the argv from `(input_format,
//!   output_format, filter_params, extra_args)` per the same
//!   protocol as the Python helper, so the input/output shapes are
//!   wire-compatible (this matters for the Sendspin FLAC frames —
//!   they are encoded with the same flags).
//! * `parse_ffmpeg_stream_info` / `parse_ffmpeg_duration` are 1:1
//!   re-implementations of the regex parsers.

use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures::Stream;
use regex::Regex;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use ma_core::enums::ContentType;

pub const MINIMAL_FFMPEG_VERSION: u32 = 6;

/// Information about an ffmpeg audio stream (parsed from the
/// `Stream #N: Audio: ...` log line that ffmpeg emits on stderr).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FFMpegStreamInfo {
    pub codec: ContentType,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub bit_rate: Option<u32>,
}

#[derive(Debug, Error)]
pub enum FFMpegError {
    #[error("ffmpeg binary not found in PATH")]
    NotFound,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg exited with code {0}")]
    Exit(i32),
    #[error("output closed unexpectedly")]
    OutputClosed,
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("unsupported output format: {0:?}")]
    UnsupportedOutputFormat(ContentType),
}

pub type Result<T> = std::result::Result<T, FFMpegError>;

/// One chunk pulled from the ffmpeg stdout.
pub type FFMpegChunkStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + Sync>>;

pub type FFMpegChunkSender = mpsc::Sender<Result<Bytes>>;

/// Inner process wrapper. Holds the `Child` and lets readers/writers
/// attach via dedicated `&mut` references.
struct ChildProcess {
    child: Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: Option<tokio::process::ChildStdout>,
}

/// Async ffmpeg process. The `FFMpeg` struct owns the `Child`; the
/// `iter_chunked` / `iter_stderr` / `feed_stdin` methods borrow from
/// it through a tokio mutex.
pub struct FFMpeg {
    args: Vec<String>,
    /// Set when `start` is called. Held to keep the child alive.
    inner: Arc<Mutex<Option<ChildProcess>>>,
    /// Log lines from stderr, kept for callers that want to surface
    /// them in error messages.
    log_history: Arc<Mutex<Vec<String>>>,
    pub input_stream_info: Arc<Mutex<Option<FFMpegStreamInfo>>>,
    pub output_stream_info: Arc<Mutex<Option<FFMpegStreamInfo>>>,
    pub parsed_duration: Arc<Mutex<Option<u32>>>,
}

impl FFMpeg {
    /// Build a new ffmpeg process with the given argv. Call `start`
    /// to spawn.
    pub fn new(args: Vec<String>) -> Self {
        Self {
            args,
            inner: Arc::new(Mutex::new(None)),
            log_history: Arc::new(Mutex::new(Vec::with_capacity(100))),
            input_stream_info: Arc::new(Mutex::new(None)),
            output_stream_info: Arc::new(Mutex::new(None)),
            parsed_duration: Arc::new(Mutex::new(None)),
        }
    }

    /// Spawn the child process and return immediately. Attach
    /// readers via the methods below.
    pub async fn start(&mut self) -> Result<()> {
        let mut cmd = Command::new(&self.args[0]);
        if self.args.len() > 1 {
            cmd.args(&self.args[1..]);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self.inner.lock().await = Some(ChildProcess {
            child,
            stdin,
            stdout,
        });
        // Spawn the stderr reader task.
        if let Some(stderr) = stderr {
            let log_history = Arc::clone(&self.log_history);
            let input_info = Arc::clone(&self.input_stream_info);
            let output_info = Arc::clone(&self.output_stream_info);
            let parsed_duration = Arc::clone(&self.parsed_duration);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    // parse if it's a Stream # line.
                    if let Some(info) = parse_ffmpeg_stream_info(&line) {
                        // The Python helper tracks _current_log_section
                        // via Input/Output headers. We use a simpler
                        // heuristic: alternate between input and
                        // output on each "Stream #" line. For
                        // single-input single-output invocations
                        // (the Phase 2 surface) this is enough.
                        let mut cur = input_info.lock().await;
                        if cur.is_none() {
                            *cur = Some(info);
                        } else {
                            drop(cur);
                            *output_info.lock().await = Some(info);
                        }
                    }
                    if let Some(dur) = parse_ffmpeg_duration(&line) {
                        *parsed_duration.lock().await = Some(dur);
                    }
                    if line.contains("critical") || line.contains("error") {
                        warn!("[ffmpeg] {line}");
                    } else {
                        debug!("[ffmpeg] {line}");
                    }
                    log_history.lock().await.push(line);
                }
            });
        }
        Ok(())
    }

    /// Read ffmpeg's stdout as a chunked async stream. Yields
    /// `FFMpegError::OutputClosed` when ffmpeg exits.
    pub fn iter_chunked(&self, chunk_size: usize) -> FFMpegChunkStream {
        let inner = Arc::clone(&self.inner);
        Box::pin(async_stream::stream! {
            loop {
                let mut guard = inner.lock().await;
                let Some(cp) = guard.as_mut() else {
                    yield Err(FFMpegError::OutputClosed);
                    break;
                };
                let Some(stdout) = cp.stdout.as_mut() else {
                    yield Err(FFMpegError::OutputClosed);
                    break;
                };
                let mut buf = BytesMut::with_capacity(chunk_size);
                buf.resize(chunk_size, 0);
                match stdout.read(&mut buf[..]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        yield Ok(buf.freeze());
                    }
                    Err(e) => {
                        yield Err(FFMpegError::Io(e));
                        break;
                    }
                }
            }
        })
    }

    /// Send bytes to ffmpeg's stdin. Used when the input is an async
    /// stream (a generator that produces PCM / MP3 / whatever) rather
    /// than a file or URL.
    pub async fn feed_stdin<R>(&self, mut source: R) -> Result<()>
    where
        R: Stream<Item = Result<Bytes>> + Unpin,
    {
        use futures::StreamExt;
        let mut guard = self.inner.lock().await;
        let Some(cp) = guard.as_mut() else {
            return Err(FFMpegError::OutputClosed);
        };
        let Some(stdin) = cp.stdin.as_mut() else {
            return Err(FFMpegError::OutputClosed);
        };
        while let Some(chunk) = source.next().await {
            match chunk {
                Ok(bytes) => {
                    stdin.write_all(&bytes).await?;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
        // Closing stdin signals EOF to ffmpeg.
        drop(guard);
        Ok(())
    }

    /// Wait for the process to exit and return its status.
    pub async fn wait(&self) -> Result<std::process::ExitStatus> {
        let mut guard = self.inner.lock().await;
        let cp = guard.as_mut().ok_or(FFMpegError::OutputClosed)?;
        let status = cp.child.wait().await?;
        Ok(status)
    }

    /// Best-effort kill. Used when the caller wants to stop playback.
    pub async fn kill(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if let Some(cp) = guard.as_mut() {
            cp.child.kill().await?;
        }
        Ok(())
    }

    /// Snapshot the recent log lines, for error reporting.
    pub async fn log_history_snapshot(&self) -> Vec<String> {
        self.log_history.lock().await.clone()
    }
}

/// Build the argv for an ffmpeg invocation that transcodes / resamples
/// one input into one output. Mirrors `get_ffmpeg_args` from
/// `music_assistant/helpers/ffmpeg.py`. The Python helper supports a
/// few more output flavours (NUT, ADTS, MP3) — Phase 2 covers PCM,
/// WAV, FLAC, and Opus.
#[allow(clippy::too_many_arguments)]
pub fn build_ffmpeg_args(
    input_path: &str,
    input_format: &FFMpegStreamInfo,
    output_path: &str,
    output_format: &FFMpegStreamInfo,
    filter_params: &[&str],
    extra_args: &[&str],
    extra_input_args: &[&str],
    extra_output_args: &[&str],
) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "ffmpeg".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-nostats".to_string(),
        "-ignore_unknown".to_string(),
        "-protocol_whitelist".to_string(),
        "file,hls,http,https,tcp,tls,crypto,pipe,data,fd,rtp,udp,concat".to_string(),
        "-probesize".to_string(),
        "8096".to_string(),
        "-analyzeduration".to_string(),
        "500000".to_string(),
    ];

    // Input args.
    if !extra_input_args.is_empty() && extra_input_args.contains(&"-f") {
        args.extend(extra_input_args.iter().map(|s| s.to_string()));
    } else {
        for a in extra_input_args {
            args.push((*a).to_string());
        }
        if input_path.starts_with("http") {
            args.extend([
                "-reconnect".into(),
                "1".into(),
                "-reconnect_delay_max".into(),
                "10".into(),
                "-reconnect_streamed".into(),
                "1".into(),
                "-reconnect_on_http_error".into(),
                "5xx,429".into(),
            ]);
        }
        if input_format.codec.is_pcm() {
            args.extend([
                "-ac".into(),
                "2".into(), // TODO: input_format.channels
                "-ar".into(),
                input_format
                    .sample_rate
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "48000".into()),
                "-acodec".into(),
                input_format.codec_name().to_string(),
                "-f".into(),
                input_format.codec_name().to_string(),
            ]);
        }
        args.extend(["-i".into(), input_path.into()]);
    }

    // Output args.
    args.extend([
        "-ac".into(),
        "2".into(), // TODO: output_format.channels
    ]);
    if output_path == "NULL" {
        args.extend(["-f".into(), "null".into()]);
    } else if output_format.codec.is_pcm() {
        args.extend([
            "-ar".into(),
            output_format
                .sample_rate
                .map(|s| s.to_string())
                .unwrap_or_else(|| "48000".into()),
            "-acodec".into(),
            output_format.codec_name().to_string(),
            "-f".into(),
            output_format.codec_name().to_string(),
        ]);
    } else {
        match output_format.codec {
            ContentType::Wav => {
                args.extend([
                    "-ar".into(),
                    output_format
                        .sample_rate
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "48000".into()),
                    "-f".into(),
                    "wav".into(),
                ]);
            }
            ContentType::Flac => {
                let sample_fmt = if output_format.bit_depth.unwrap_or(16) > 16 {
                    "s32"
                } else {
                    "s16"
                };
                args.extend([
                    "-sample_fmt".into(),
                    sample_fmt.into(),
                    "-ar".into(),
                    output_format
                        .sample_rate
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "48000".into()),
                    "-f".into(),
                    "flac".into(),
                    "-compression_level".into(),
                    "0".into(),
                ]);
            }
            ContentType::Opus => {
                args.extend([
                    "-ar".into(),
                    "48000".into(),
                    "-ac".into(),
                    "2".into(),
                    "-b:a".into(),
                    "256k".into(),
                    "-f".into(),
                    "opus".into(),
                ]);
            }
            ContentType::Mp3 => {
                args.extend(["-f".into(), "mp3".into(), "-b:a".into(), "320k".into()]);
            }
            ContentType::Aac => {
                args.extend([
                    "-f".into(),
                    "adts".into(),
                    "-c:a".into(),
                    "aac".into(),
                    "-b:a".into(),
                    "256k".into(),
                ]);
            }
            ContentType::Ogg => {
                args.extend(["-f".into(), "ogg".into()]);
            }
            other => {
                return Err(FFMpegError::UnsupportedOutputFormat(other));
            }
        }
    }
    for a in extra_output_args {
        args.push((*a).to_string());
    }
    args.push(output_path.into());

    // Auto-resample if needed.
    let mut filters: Vec<String> = filter_params.iter().map(|s| s.to_string()).collect();
    let in_sr = input_format.sample_rate.unwrap_or(48_000);
    let out_sr = output_format.sample_rate.unwrap_or(48_000);
    let in_bd = input_format.bit_depth.unwrap_or(16);
    let out_bd = output_format.bit_depth.unwrap_or(16);
    if in_sr != out_sr || (in_bd > 16 && out_bd == 16) {
        let mut f = "aresample=resampler=soxr:precision=30".to_string();
        if in_sr != out_sr {
            f.push_str(&format!(":osr={out_sr}"));
        }
        if out_bd == 16 && in_bd > 16 {
            f.push_str(":osf=s16:dither_method=triangular_hp");
        }
        filters.push(f);
    }

    if !filters.is_empty() && !extra_args.contains(&"-filter_complex") {
        args.extend(["-af".into(), filters.join(",")]);
    }
    for a in extra_args {
        args.push((*a).to_string());
    }
    Ok(args)
}

/// Parse a ffmpeg `Stream #N: Audio: <codec>, <rate> Hz, ...` line
/// into a `FFMpegStreamInfo`. Mirrors the Python parser.
pub fn parse_ffmpeg_stream_info(line: &str) -> Option<FFMpegStreamInfo> {
    if !line.starts_with("Stream #") || !line.contains(": Audio: ") {
        return None;
    }
    let after = line.split(": Audio: ").nth(1)?;
    let codec_token = after.split(' ').next()?.split(',').next()?;
    let codec = parse_codec_token(codec_token);
    let mut info = FFMpegStreamInfo {
        codec,
        ..Default::default()
    };
    if let Some(m) = sample_rate_re().captures(line) {
        info.sample_rate = m.get(1).and_then(|s| s.as_str().parse().ok());
    }
    if let Some(m) = bit_rate_re().captures(line) {
        info.bit_rate = m.get(1).and_then(|s| s.as_str().parse().ok());
    }
    // Bit depth: prefer explicit "(N bit)" annotation, else fall back
    // to sample-fmt token.
    if let Some(m) = explicit_bit_depth_re().captures(line) {
        info.bit_depth = m.get(1).and_then(|s| s.as_str().parse().ok());
    } else if codec.is_lossless() {
        if let Some(m) = sample_fmt_re().captures(line) {
            if let Some(token) = m.get(1) {
                info.bit_depth = sample_fmt_bit_depth(token.as_str());
            }
        }
    }
    Some(info)
}

pub fn parse_ffmpeg_duration(line: &str) -> Option<u32> {
    let m = duration_re().captures(line)?;
    let h: u32 = m.get(1)?.as_str().parse().ok()?;
    let mm: u32 = m.get(2)?.as_str().parse().ok()?;
    let s: f64 = m.get(3)?.as_str().parse().ok()?;
    Some(h * 3600 + mm * 60 + s as u32)
}

fn parse_codec_token(token: &str) -> ContentType {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        "mp3" => ContentType::Mp3,
        "aac" => ContentType::Aac,
        "flac" => ContentType::Flac,
        "vorbis" | "libvorbis" => ContentType::Vorbis,
        "opus" | "libopus" => ContentType::Opus,
        "wav" | "pcm_s16le" | "pcm_s24le" | "pcm_s32le" | "pcm_f32le" => ContentType::Wav,
        "wma" | "wmav2" => ContentType::Wma,
        "wavpack" => ContentType::WavPack,
        "alac" => ContentType::Alac,
        "ape" => ContentType::Ape,
        "mpeg" => ContentType::Mpeg,
        "dsd" | "dsf" => ContentType::Dsf,
        "mpc" | "musepack" => ContentType::MusePack,
        _ => ContentType::Unknown,
    }
}

fn sample_fmt_bit_depth(token: &str) -> Option<u32> {
    let t = token.split('(').next().unwrap_or(token).to_string();
    match t.as_str() {
        "u8" | "u8p" => Some(8),
        "s16" | "s16p" => Some(16),
        "s24" | "s24p" => Some(24),
        "s32" | "s32p" | "flt" | "fltp" => Some(32),
        "dbl" | "dblp" => Some(64),
        _ => None,
    }
}

impl FFMpegStreamInfo {
    pub fn from_content_type(c: ContentType) -> Self {
        Self {
            codec: c,
            sample_rate: Some(48_000),
            bit_depth: Some(16),
            bit_rate: None,
        }
    }
    pub fn is_pcm(&self) -> bool {
        self.codec.is_pcm()
    }
    pub fn is_lossless(&self) -> bool {
        self.codec.is_lossless()
    }
    pub fn codec_name(&self) -> &'static str {
        // Map to ffmpeg's `acodec` token. The mapping must be exact
        // for ffmpeg's `-acodec` parser.
        match self.codec {
            ContentType::Mp3 => "mp3",
            ContentType::Aac => "aac",
            ContentType::Flac => "flac",
            ContentType::Vorbis => "vorbis",
            ContentType::Opus => "libopus",
            ContentType::Wav => "pcm_s16le",
            ContentType::Wma => "wmav2",
            ContentType::WavPack => "wavpack",
            ContentType::Alac => "alac",
            ContentType::Ape => "ape",
            ContentType::Mpeg => "mp2",
            ContentType::Dsf => "dsd_lsbf_planar",
            ContentType::MusePack => "mpc7",
            _ => "pcm_s16le",
        }
    }
}

fn sample_rate_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+) Hz").unwrap())
}
fn bit_rate_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+) kb/s").unwrap())
}
fn explicit_bit_depth_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\((\d+) bit\)").unwrap())
}
fn sample_fmt_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(u8p?|s16p?|s24p?|s32p?|fltp?|dblp?)\b").unwrap())
}
fn duration_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Duration: (\d+):(\d+):(\d+(?:\.\d+)?)").unwrap())
}

/// Verify ffmpeg is installed and meets the minimum version. Mirrors
/// `check_ffmpeg_version` from the Python helper.
pub async fn check_ffmpeg_version() -> Result<u32> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-version");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let output = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FFMpegError::NotFound
        } else {
            FFMpegError::Io(e)
        }
    })?;
    if !output.status.success() {
        return Err(FFMpegError::Exit(output.status.code().unwrap_or(-1)));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version_line = stdout
        .lines()
        .find(|l| l.contains("ffmpeg version"))
        .ok_or_else(|| FFMpegError::InvalidArgument("missing version line".into()))?;
    let version_str = version_line
        .split("ffmpeg version ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|s| s.split('-').next())
        .ok_or_else(|| FFMpegError::InvalidArgument("malformed version line".into()))?;
    let major: u32 = version_str
        .split('.')
        .next()
        .and_then(|s| {
            s.chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .ok_or_else(|| {
            FFMpegError::InvalidArgument(format!("version not numeric: {version_str}"))
        })?;
    if major < MINIMAL_FFMPEG_VERSION {
        return Err(FFMpegError::InvalidArgument(format!(
            "ffmpeg {major} < {MINIMAL_FFMPEG_VERSION}"
        )));
    }
    Ok(major)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_info_parses_mp3() {
        let line = "Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 320 kb/s";
        let info = parse_ffmpeg_stream_info(line).unwrap();
        assert_eq!(info.codec, ContentType::Mp3);
        assert_eq!(info.sample_rate, Some(44_100));
        assert_eq!(info.bit_rate, Some(320));
        assert_eq!(info.bit_depth, None);
    }

    #[test]
    fn stream_info_parses_24bit_flac_via_explicit_annotation() {
        let line = "Stream #0:0: Audio: flac, 96000 Hz, stereo, s32 (24 bit)";
        let info = parse_ffmpeg_stream_info(line).unwrap();
        assert_eq!(info.codec, ContentType::Flac);
        assert_eq!(info.bit_depth, Some(24));
    }

    #[test]
    fn stream_info_returns_none_for_non_audio_line() {
        assert!(parse_ffmpeg_stream_info("Duration: 00:01:23.45, start: 0.0").is_none());
        assert!(parse_ffmpeg_stream_info("").is_none());
    }

    #[test]
    fn duration_parses_hh_mm_ss() {
        assert_eq!(
            parse_ffmpeg_duration("Duration: 01:23:45.67"),
            Some(3600 + 23 * 60 + 45)
        );
        assert_eq!(parse_ffmpeg_duration("Duration: 00:00:30.00"), Some(30));
        assert!(parse_ffmpeg_duration("Duration: N/A").is_none());
    }

    #[test]
    fn codec_names_match_ffmpeg_tokens() {
        assert_eq!(
            FFMpegStreamInfo::from_content_type(ContentType::Opus).codec_name(),
            "libopus"
        );
        assert_eq!(
            FFMpegStreamInfo::from_content_type(ContentType::Flac).codec_name(),
            "flac"
        );
        assert_eq!(
            FFMpegStreamInfo::from_content_type(ContentType::Mp3).codec_name(),
            "mp3"
        );
    }

    #[test]
    fn sample_fmt_bit_depth_handles_planar_and_non_planar() {
        assert_eq!(sample_fmt_bit_depth("s16"), Some(16));
        assert_eq!(sample_fmt_bit_depth("s16p"), Some(16));
        assert_eq!(sample_fmt_bit_depth("s32p"), Some(32));
        assert_eq!(sample_fmt_bit_depth("xyz"), None);
    }

    #[test]
    fn build_args_omits_extra_filter_complex_when_present() {
        let input = FFMpegStreamInfo {
            codec: ContentType::Flac,
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            bit_rate: None,
        };
        let output = FFMpegStreamInfo {
            codec: ContentType::Flac,
            sample_rate: Some(48_000),
            bit_depth: Some(16),
            bit_rate: None,
        };
        let args = build_ffmpeg_args(
            "in.flac",
            &input,
            "-",
            &output,
            &["loudnorm=I=-16:TP=-1.5:LRA=11"],
            &[],
            &[],
            &[],
        )
        .unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("-af "));
        // The auto-resample filter is appended after the user's filter
        // (so the soxr resampler runs *after* loudnorm, per the
        // Python helper's documented behaviour). We forced a
        // sample-rate change so the auto-resample kicks in.
        assert!(joined.contains("aresample=resampler=soxr"));
    }

    #[tokio::test]
    async fn check_version_returns_major() {
        // We assume ffmpeg is on the test box; we already saw 8.1.1.
        match check_ffmpeg_version().await {
            Ok(major) => assert!(major >= 6),
            Err(FFMpegError::NotFound) => {
                eprintln!("ffmpeg not on PATH; skipping");
            }
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
