//! Multi-client broadcast stream.
//!
//! When a player requests a stream URL, the server creates one
//! `BroadcastStream` per active session and fans it out to N
//! `Subscriber`s. This is the same pattern the Python aiohttp
//! streamserver uses for visualizer data (peak/RMS) and for shared
//! playback sessions in the Sendspin protocol.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::debug;

/// Unique id for a subscriber. Phase 2 just uses a monotonic u64
/// — the Sendspin protocol uses an opaque 16-byte id but we don't
/// need to be wire-compatible for the broadcast primitive itself.
pub type SubscriberId = u64;

static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);
fn next_subscriber_id() -> SubscriberId {
    NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Error)]
pub enum BroadcastError {
    #[error("subscriber closed")]
    Closed,
    #[error("broadcast source exhausted")]
    Exhausted,
}

/// One downstream consumer of a `BroadcastStream`. Pulls chunks via
/// the `Stream` impl; each `Subscriber` is a separate task that
/// blocks on its `mpsc` channel.
pub struct Subscriber {
    id: SubscriberId,
    rx: mpsc::Receiver<Bytes>,
    /// EOF marker pushed when the source stream ends so subscribers
    /// can return cleanly.
    #[allow(dead_code)]
    eof: Arc<AtomicBool>,
    /// When the source has ended, future subscribers see this and
    /// return `None` immediately.
    finished: Arc<AtomicBool>,
}

impl std::fmt::Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscriber")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Subscriber {
    pub fn id(&self) -> SubscriberId {
        self.id
    }
}

impl Stream for Subscriber {
    type Item = Result<Bytes, BroadcastError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Drain any pending chunks from the mpsc channel FIRST. The
        // `finished` flag is only consulted once the channel is
        // empty (or closed), so chunks that were broadcast before
        // `finish()` are still delivered.
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => {
                return Poll::Ready(Some(Ok(bytes)));
            }
            Poll::Ready(None) => {
                // Channel closed → no more senders → end of stream.
                return Poll::Ready(None);
            }
            Poll::Pending => {}
        }
        // Channel is empty. If the broadcast was finished, return
        // None; otherwise stay pending.
        if self.finished.load(Ordering::Acquire) {
            return Poll::Ready(None);
        }
        Poll::Pending
    }
}

/// One slot in the `subscribers` list: id, sender, and EOF flag.
type SubscriberSlot = (SubscriberId, mpsc::Sender<Bytes>, Arc<AtomicBool>);

/// Multi-client fan-out. The producer side (`broadcast`) accepts
/// `Bytes` chunks; each chunk is copied to every active subscriber's
/// `mpsc::Sender` (with a small per-subscriber buffer).
pub struct BroadcastStream {
    subscribers: Mutex<Vec<SubscriberSlot>>,
    /// Set to true when the source stream ends; new subscribers
    /// will see this through their `Subscriber` and return `None`
    /// from the first poll.
    finished: Arc<AtomicBool>,
    /// Per-subscriber buffer size.
    buffer: usize,
}

impl Default for BroadcastStream {
    fn default() -> Self {
        Self::new(64)
    }
}

impl BroadcastStream {
    /// Create a new broadcast. `buffer` is the per-subscriber chunk
    /// backlog (oldest chunks are dropped when full — for live
    /// streams that's the right semantics).
    pub fn new(buffer: usize) -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            finished: Arc::new(AtomicBool::new(false)),
            buffer: buffer.max(1),
        }
    }

    /// Add a new subscriber. The returned `Subscriber` will receive
    /// every subsequent chunk.
    pub fn subscribe(&self) -> Subscriber {
        let id = next_subscriber_id();
        let (tx, rx) = mpsc::channel(self.buffer);
        let eof = Arc::new(AtomicBool::new(false));
        self.subscribers.lock().push((id, tx, Arc::clone(&eof)));
        Subscriber {
            id,
            rx,
            eof,
            finished: Arc::clone(&self.finished),
        }
    }

    /// Broadcast one chunk to every subscriber. Drops the chunk for
    /// any subscriber whose buffer is full (they will catch up on
    /// the next chunk).
    pub fn broadcast(&self, chunk: Bytes) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        let mut subs = self.subscribers.lock();
        for (id, tx, _eof) in subs.iter() {
            if tx.try_send(chunk.clone()).is_err() {
                // Buffer full; skip. (Subscriber is still alive,
                // just slow.) Don't drop the subscriber for this
                // reason — the next chunk may fit.
                debug!(subscriber = id, "subscriber buffer full, dropping chunk");
            }
        }
        // Detect disconnected subscribers and remove them.
        subs.retain(|(_, tx, _)| !tx.is_closed());
    }

    /// Mark the source as finished. Subscribers see the EOF marker
    /// on their next poll and return `None`.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        let subs = self.subscribers.lock();
        for (_, _tx, eof) in subs.iter() {
            eof.store(true, Ordering::Release);
        }
    }

    /// How many subscribers are currently attached.
    pub fn len(&self) -> usize {
        self.subscribers.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for BroadcastStream {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn broadcast_delivers_to_all_subscribers() {
        let b = BroadcastStream::new(8);
        let mut s1 = b.subscribe();
        let mut s2 = b.subscribe();
        b.broadcast(Bytes::from_static(b"hello"));
        b.broadcast(Bytes::from_static(b"world"));
        // Drain chunks first, then signal EOF.
        let mut got1 = Vec::new();
        let mut got2 = Vec::new();
        for _ in 0..2 {
            if let Some(c) = s1.next().await {
                got1.push(c.unwrap());
            }
            if let Some(c) = s2.next().await {
                got2.push(c.unwrap());
            }
        }
        b.finish();
        // After finish(), poll should return None (no more chunks).
        assert!(s1.next().await.is_none());
        assert!(s2.next().await.is_none());
        assert_eq!(
            got1,
            vec![Bytes::from_static(b"hello"), Bytes::from_static(b"world")]
        );
        assert_eq!(
            got2,
            vec![Bytes::from_static(b"hello"), Bytes::from_static(b"world")]
        );
    }

    #[tokio::test]
    async fn subscriber_added_after_finish_sees_none_immediately() {
        let b = BroadcastStream::new(8);
        b.finish();
        let mut s = b.subscribe();
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn dropped_subscriber_is_cleaned_up() {
        let b = BroadcastStream::new(8);
        {
            let s = b.subscribe();
            drop(s);
        }
        b.broadcast(Bytes::from_static(b"hi"));
        assert_eq!(b.len(), 0);
    }

    #[tokio::test]
    async fn broadcast_with_no_subscribers_is_a_no_op() {
        let b = BroadcastStream::new(8);
        b.broadcast(Bytes::from_static(b"hi"));
        b.finish();
        assert_eq!(b.len(), 0);
    }
}
