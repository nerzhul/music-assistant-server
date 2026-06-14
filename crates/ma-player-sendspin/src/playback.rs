//! Minimal `PushStream` analogue for Phase 1.
//!
//! The Python `aiosendspin.server.push_stream.PushStream` coordinates
//! per-channel audio + artwork + visualizer delivery. The full port
//! (fading, sliding-window queue, multi-channel DSP) belongs to a later
//! phase; this module provides the data structure the WebSocket
//! connection writer reads from.
//!
//! The contract for Phase 1:
//!
//! * `PushStream::push_audio` enqueues a (timestamp, bytes) pair.
//! * `PushStream::take_pending` drains pending frames for delivery.
//! * `PushStream::start` / `PushStream::end` publish `stream/start` and
//!   `stream/end` envelopes to subscribers.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::debug;

use ma_protocol_sendspin::roles::AudioFormat;

use crate::protocol::{stream_end_envelope, stream_start_envelope};
use crate::state::OutboundMessage;

/// A single chunk of PCM/encoded audio ready to be delivered.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Server-clock timestamp (microseconds) at which the first sample
    /// should be output. The client subtracts its own `static_delay_ms`
    /// from this and uses the time filter to translate to local time.
    pub timestamp_us: i64,
    /// Duration of the chunk in microseconds (computed from format).
    pub duration_us: i64,
    /// Encoded audio bytes (PCM pass-through in Phase 1).
    pub data: Arc<Vec<u8>>,
}

impl AudioChunk {
    pub fn new(timestamp_us: i64, duration_us: i64, data: Vec<u8>) -> Self {
        Self {
            timestamp_us,
            duration_us,
            data: Arc::new(data),
        }
    }
}

/// Coalesced stream state shared by all subscribers.
#[derive(Debug)]
struct StreamCore {
    /// Negotiated audio format for the current stream.
    format: Option<AudioFormat>,
    /// Optional base64 codec header (e.g. FLAC `fLaC` marker).
    codec_header: Option<String>,
    /// Queued audio chunks (timestamped) awaiting delivery to the
    /// subscriber's outbound channel.
    pending: Vec<AudioChunk>,
    /// True while a stream is active.
    active: bool,
}

impl StreamCore {
    fn new() -> Self {
        Self {
            format: None,
            codec_header: None,
            pending: Vec::new(),
            active: false,
        }
    }
}

/// Per-subscriber handle: the connection task clones the `mpsc::Sender`
/// into its outbound channel and the `PushStream` writes chunks to it.
#[derive(Clone)]
pub struct Subscriber {
    pub client_id: String,
    pub tx: mpsc::Sender<OutboundMessage>,
}

#[derive(Clone)]
pub struct PushStream {
    core: Arc<Mutex<StreamCore>>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

impl PushStream {
    pub fn new() -> Self {
        Self {
            core: Arc::new(Mutex::new(StreamCore::new())),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_subscriber(&self, sub: Subscriber) {
        self.subscribers.lock().push(sub);
    }

    pub fn remove_subscriber(&self, client_id: &str) {
        let mut subs = self.subscribers.lock();
        subs.retain(|s| s.client_id != client_id);
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().len()
    }

    /// Start a new stream with the given format. Emits `stream/start` to
    /// every subscriber and clears any pending chunks.
    pub async fn start(
        &self,
        format: AudioFormat,
        codec_header: Option<String>,
    ) -> Result<(), serde_json::Error> {
        {
            let mut core = self.core.lock();
            core.format = Some(format);
            core.codec_header = codec_header;
            core.pending.clear();
            core.active = true;
        }
        let envelope = stream_start_envelope(
            &ma_protocol_sendspin::messages::StreamStart::player(format, None),
        )?;
        self.broadcast_text(envelope).await;
        Ok(())
    }

    /// End the current stream. Emits `stream/end` to every subscriber.
    pub async fn end(&self) -> Result<(), serde_json::Error> {
        {
            let mut core = self.core.lock();
            core.active = false;
            core.pending.clear();
        }
        let envelope = stream_end_envelope(&ma_protocol_sendspin::messages::StreamEnd::default())?;
        self.broadcast_text(envelope).await;
        Ok(())
    }

    /// Push a chunk to the stream. In Phase 1, the chunk is fanned out
    /// immediately to every subscriber as a binary frame.
    pub async fn push_audio(&self, chunk: AudioChunk) {
        if !self.core.lock().active {
            debug!("push_audio on inactive stream — dropping");
            return;
        }
        let mut frame = Vec::with_capacity(9 + chunk.data.len());
        frame.push(ma_protocol_sendspin::binary::MSG_TYPE_AUDIO_CHUNK);
        frame.extend_from_slice(&chunk.timestamp_us.to_be_bytes());
        frame.extend_from_slice(&chunk.data);
        self.broadcast_binary(frame).await;
    }

    async fn broadcast_text<T: Serialize>(&self, msg: T) {
        let text = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(e) => {
                debug!(error = %e, "failed to encode push_stream text frame");
                return;
            }
        };
        let subs = self.subscribers.lock().clone();
        for s in subs {
            if s.tx
                .send(OutboundMessage::Text(text.clone()))
                .await
                .is_err()
            {
                debug!(client = %s.client_id, "subscriber channel closed");
            }
        }
    }

    async fn broadcast_binary(&self, frame: Vec<u8>) {
        let subs = self.subscribers.lock().clone();
        for s in subs {
            if s.tx
                .send(OutboundMessage::Binary(frame.clone()))
                .await
                .is_err()
            {
                debug!(client = %s.client_id, "subscriber channel closed");
            }
        }
    }
}

impl Default for PushStream {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_stream_round_trip() {
        let ps = PushStream::new();
        let (tx, mut rx) = mpsc::channel(8);
        ps.add_subscriber(Subscriber {
            client_id: "c1".into(),
            tx,
        });
        let format = AudioFormat::Pcm {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 16,
            sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
        };
        ps.start(format, None).await.unwrap();
        // First message should be the stream/start envelope.
        let first = rx.recv().await.unwrap();
        match first {
            OutboundMessage::Text(s) => assert!(s.contains("stream/start")),
            _ => panic!("expected text frame"),
        }
        // Push a chunk and expect a binary frame.
        let chunk = AudioChunk::new(123, 1000, vec![0u8; 32]);
        ps.push_audio(chunk).await;
        let second = rx.recv().await.unwrap();
        match second {
            OutboundMessage::Binary(b) => {
                assert_eq!(b[0], ma_protocol_sendspin::binary::MSG_TYPE_AUDIO_CHUNK);
                assert_eq!(&b[1..9], &123i64.to_be_bytes());
                assert_eq!(&b[9..], &[0u8; 32][..]);
            }
            _ => panic!("expected binary frame"),
        }
        // End the stream.
        ps.end().await.unwrap();
        let third = rx.recv().await.unwrap();
        match third {
            OutboundMessage::Text(s) => assert!(s.contains("stream/end")),
            _ => panic!("expected text frame"),
        }
    }

    #[tokio::test]
    async fn push_stream_ignores_inactive() {
        let ps = PushStream::new();
        let (tx, mut rx) = mpsc::channel(8);
        ps.add_subscriber(Subscriber {
            client_id: "c1".into(),
            tx,
        });
        // No start, just push — should be a no-op.
        let chunk = AudioChunk::new(0, 1000, vec![1, 2, 3]);
        ps.push_audio(chunk).await;
        // No message should have arrived.
        assert!(rx.try_recv().is_err());
    }
}
