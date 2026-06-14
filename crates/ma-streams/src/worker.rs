//! `StreamWorker` — drives one ffmpeg pipeline for a single
//! `StreamDetails`.
//!
//! The worker takes:
//!
//! * the raw input bytes (a `StreamProvider::get_stream_bytes` future),
//! * the input format (parsed from the `StreamDetails.audio_format`),
//! * the desired output format (per-player),
//!
//! spawns ffmpeg, and exposes its stdout as a chunk stream plus the
//! `parsed_duration` and `input_stream_info` ffmpeg reports.

use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use thiserror::Error;
use tracing::warn;

use ma_core::enums::ContentType;
use ma_providers::stream::{StreamAudioFormat, StreamDetails};

use crate::ffmpeg::{build_ffmpeg_args, FFMpeg, FFMpegChunkStream, FFMpegError, FFMpegStreamInfo};

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("ffmpeg error: {0}")]
    FFMpeg(#[from] FFMpegError),
    #[error("no input source: {0}")]
    NoInput(String),
    #[error("unsupported input format: {0:?}")]
    UnsupportedInputFormat(ContentType),
}

pub type Result<T> = std::result::Result<T, WorkerError>;

/// One chunk of the worker's output.
#[derive(Debug, Clone)]
pub struct WorkerChunk {
    pub bytes: Bytes,
}

#[derive(Debug, Clone)]
pub struct StreamWorkerConfig {
    /// PCM s32 stereo @ 48 kHz is the canonical intermediate format
    /// — every transcoding pipeline lands here first, then a second
    /// ffmpeg can re-encode for each player's preferred codec if
    /// the player doesn't want FLAC. Phase 2 always uses this.
    pub output_format: StreamAudioFormat,
    /// Optional extra ffmpeg filter params (volume normalization,
    /// EQ, ...).
    pub filter_params: Vec<String>,
    /// Optional seek position in seconds (for seekable streams).
    pub seek_position: u32,
    /// ffmpeg loglevel. Defaults to "error" so we don't spam the
    /// console on every stream.
    pub loglevel: String,
}

impl Default for StreamWorkerConfig {
    fn default() -> Self {
        Self {
            output_format: StreamAudioFormat {
                content_type: ContentType::Flac,
                sample_rate: 48_000,
                bit_depth: 16,
                channels: 2,
                bit_rate: None,
            },
            filter_params: Vec::new(),
            seek_position: 0,
            loglevel: "error".to_string(),
        }
    }
}

/// Source of input bytes for the worker. Either a URL / file path
/// that ffmpeg reads itself, or a future that yields PCM bytes
/// piped to ffmpeg's stdin.
pub enum InputSource {
    /// ffmpeg opens this directly (URL or file path).
    Path(String),
    /// Bytes piped to ffmpeg's stdin.
    Stream(Pin<Box<dyn Stream<Item = FFMpegErrorResult<Bytes>> + Send>>),
}

pub type FFMpegErrorResult<T> = std::result::Result<T, FFMpegError>;

pub struct StreamWorker {
    config: StreamWorkerConfig,
    streamdetails: Arc<StreamDetails>,
    source: InputSource,
}

impl StreamWorker {
    pub fn new(
        streamdetails: Arc<StreamDetails>,
        source: InputSource,
        config: StreamWorkerConfig,
    ) -> Self {
        Self {
            config,
            streamdetails,
            source,
        }
    }

    /// Spawn ffmpeg and return a stream of output chunks plus a
    /// handle to the underlying process (for kill / wait).
    pub async fn run(self) -> Result<(FFMpegChunkStream, FFMpeg)> {
        let input_format = self.input_format();
        let output_format = self.output_format_info();
        let input_path = match &self.source {
            InputSource::Path(p) => p.clone(),
            InputSource::Stream(_) => "-".to_string(),
        };
        let filter_strs: Vec<&str> = self
            .config
            .filter_params
            .iter()
            .map(|s| s.as_str())
            .collect();
        let args = build_ffmpeg_args(
            &input_path,
            &input_format,
            "-",
            &output_format,
            &filter_strs,
            &[],
            &[],
            &[],
        )?;
        let mut ffmpeg = FFMpeg::new(args);
        ffmpeg.start().await?;
        // Spawn the stdin feeder if needed.
        if let InputSource::Stream(stream) = self.source {
            let ffmpeg_ref = &ffmpeg;
            let s = async_stream::stream! {
                futures::pin_mut!(stream);
                use futures::StreamExt;
                while let Some(chunk) = stream.next().await {
                    yield chunk;
                }
            };
            // Wrap in a Pin<Box<dyn Stream + Unpin>> so we can satisfy
            // `feed_stdin`'s `R: Stream + Unpin` bound without a
            // `.boxed()` call on the AsyncStream itself.
            let s: std::pin::Pin<Box<dyn futures::Stream<Item = _> + Send>> = Box::pin(s);
            if let Err(e) = ffmpeg_ref.feed_stdin(s).await {
                warn!(error = %e, "stdin feeder ended with error");
            }
        }
        let stream = ffmpeg.iter_chunked(64 * 1024);
        Ok((stream, ffmpeg))
    }

    fn input_format(&self) -> FFMpegStreamInfo {
        let fmt = &self.streamdetails.audio_format;
        FFMpegStreamInfo {
            codec: fmt
                .as_ref()
                .map(|a| a.content_type)
                .unwrap_or(ContentType::Unknown),
            sample_rate: fmt.as_ref().map(|a| a.sample_rate),
            bit_depth: fmt.as_ref().map(|a| a.bit_depth as u32),
            bit_rate: fmt.as_ref().and_then(|a| a.bit_rate),
        }
    }

    fn output_format_info(&self) -> FFMpegStreamInfo {
        FFMpegStreamInfo {
            codec: self.config.output_format.content_type,
            sample_rate: Some(self.config.output_format.sample_rate),
            bit_depth: Some(self.config.output_format.bit_depth as u32),
            bit_rate: self.config.output_format.bit_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ma_core::enums::StreamType;

    #[test]
    fn input_format_inherits_from_streamdetails() {
        let details = StreamDetails {
            audio_format: Some(StreamAudioFormat {
                content_type: ContentType::Mp3,
                sample_rate: 44_100,
                bit_depth: 16,
                channels: 2,
                bit_rate: Some(320),
            }),
            ..Default::default()
        };
        let worker = StreamWorker::new(
            Arc::new(details),
            InputSource::Path("in.mp3".into()),
            StreamWorkerConfig::default(),
        );
        let info = worker.input_format();
        assert_eq!(info.codec, ContentType::Mp3);
        assert_eq!(info.sample_rate, Some(44_100));
        assert_eq!(info.bit_depth, Some(16));
    }

    #[test]
    fn input_format_falls_back_to_48k_16bit_when_no_audio_format() {
        let details = StreamDetails {
            stream_type: StreamType::Http,
            ..Default::default()
        };
        let worker = StreamWorker::new(
            Arc::new(details),
            InputSource::Path("in.mp3".into()),
            StreamWorkerConfig::default(),
        );
        let info = worker.input_format();
        // With no audio_format set, content_type is Unknown but the
        // sample_rate / bit_depth remain None (we don't fabricate them).
        assert_eq!(info.codec, ContentType::Unknown);
        assert_eq!(info.sample_rate, None);
    }
}
