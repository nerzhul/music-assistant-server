//! `ma-streams` — audio stream server that turns a
//! `StreamDetails` (provider-resolved playable item) into a live
//! chunked audio stream that any number of HTTP clients can pull.
//!
//! Mirrors `music_assistant/controllers/streams/{controller,audio}.py`
//! but uses a much smaller surface for Phase 2:
//!
//! * **fFmpeg process** spawned by `ffmpeg::FFMpeg`, which exposes
//!   stdout as an `AsyncGenerator[bytes]` (`iter_chunked`) plus a
//!   stderr reader that parses the `Stream #` and `Duration:` lines
//!   to populate the actual input format / bitrate / duration.
//! * **StreamWorker** drives the ffmpeg invocation for one
//!   `StreamDetails` and yields chunks. It is reusable across
//!   providers: the input is whatever bytes the provider
//!   `StreamProvider::get_stream_bytes` returns, the output is a
//!   canonical PCM/FLAC/Opus stream addressed by a per-request
//!   output `AudioFormat`.
//! * **BroadcastStream** wraps a source stream and fans it out to
//!   multiple `Subscriber`s (matches the aiohttp equivalent used by
//!   the Sendspin fan-out for visualizer/peak data).
//! * **CrossfadeStream** mixes two PCM streams with a linear fade —
//!   the dumb version of the Python `SmartFades` mixer; smarter DSP
//!   lives behind the `smart_fades` feature.
//! * **SmartFadesMixer** stub applying a simple replaygain / fixed
//!   gain on chunk boundaries. The full torch-based analyzer belongs
//!   to a later phase.
//! * **HttpServer** runs axum on 8097 and serves
//!   `GET /single/.../{player_id}.{fmt}` and
//!   `GET /flow/.../{player_id}.{fmt}` routes (the same shape as the
//!   Python `Webserver.register_dynamic_route`).

#![forbid(unsafe_code)]

pub mod broadcast;
pub mod crossfade;
pub mod ffmpeg;
pub mod server;
pub mod smart_fades;
pub mod worker;

pub use broadcast::{BroadcastStream, Subscriber, SubscriberId};
pub use crossfade::{CrossfadeConfig, CrossfadeStream, FADE_SAMPLES_DEFAULT};
pub use ffmpeg::{FFMpeg, FFMpegError, FFMpegStreamInfo};
pub use server::{HttpServer, HttpServerConfig, StreamUrl};
pub use smart_fades::{SmartFadesMixer, SmartFadesMode};
pub use worker::{StreamWorker, StreamWorkerConfig, WorkerChunk};
