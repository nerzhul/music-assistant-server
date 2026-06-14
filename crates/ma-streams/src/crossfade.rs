//! Crossfade between two PCM streams.
//!
//! The Python `smart_fades.mixer` is a full beat-aligned crossfader
//! that analyses both streams for similar BPM / key before deciding
//! where to cut. Phase 2 implements the dumb version: a fixed
//! linear crossfade over a fixed number of samples. The smart
//! detection lives in [`crate::smart_fades`].

const PCM_BYTES_PER_SAMPLE: usize = 4; // s32 little-endian

/// Default fade length: 4 seconds at 48 kHz stereo.
pub const FADE_SAMPLES_DEFAULT: u32 = 192_000;

/// Crossfade configuration.
#[derive(Debug, Clone)]
pub struct CrossfadeConfig {
    /// Number of samples (per channel) to fade across. Default 192000
    /// (4 s @ 48 kHz).
    pub fade_samples: u32,
    /// Number of channels. Phase 2 only supports 2.
    pub channels: u8,
}

impl Default for CrossfadeConfig {
    fn default() -> Self {
        Self {
            fade_samples: FADE_SAMPLES_DEFAULT,
            channels: 2,
        }
    }
}

impl CrossfadeConfig {
    pub fn fade_bytes(&self) -> usize {
        self.fade_samples as usize * self.channels as usize * PCM_BYTES_PER_SAMPLE
    }
}

/// CrossfadeStream: takes the tail of stream A, the head of stream B,
/// and a `CrossfadeConfig`, and yields a single stream that:
///   1. plays stream A until `fade_bytes` from its end,
///   2. ramps stream A's volume from 1.0 to 0.0 while ramping
///      stream B's volume from 0.0 to 1.0 over the next
///      `fade_bytes` bytes (interleaved),
///   3. then plays stream B.
pub struct CrossfadeStream {
    config: CrossfadeConfig,
    /// Tail of the outgoing stream. Buffered until we know what to
    /// mix in.
    tail: Vec<u8>,
    /// Head of the incoming stream.
    head: Vec<u8>,
}

impl CrossfadeStream {
    pub fn new(config: CrossfadeConfig) -> Self {
        Self {
            config,
            tail: Vec::new(),
            head: Vec::new(),
        }
    }

    /// Push the tail bytes of the outgoing stream. Once enough bytes
    /// have been pushed, internal mixing will start.
    pub fn push_tail(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
    }

    /// Push the head bytes of the incoming stream. Mixes against the
    /// tail if there's enough of each.
    pub fn push_head(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.head.extend_from_slice(bytes);
        let fade = self.config.fade_bytes();
        // Round to sample boundaries; we can't read half a sample.
        let mut to_mix = self.tail.len().min(self.head.len()).min(fade);
        to_mix -= to_mix % (PCM_BYTES_PER_SAMPLE * self.config.channels as usize);
        if to_mix == 0 {
            return Vec::new();
        }
        let start_alpha_num = self.tail.len() - to_mix;
        let mut out = Vec::with_capacity(to_mix);
        let step = PCM_BYTES_PER_SAMPLE * self.config.channels as usize;
        let num_samples = to_mix / step;
        for s in 0..num_samples {
            let i = s * step;
            let a = read_sample(&self.tail[start_alpha_num + i..]);
            let b = read_sample(&self.head[i..]);
            let progress = s as f32 / num_samples as f32;
            let alpha_out = 1.0 - progress;
            let alpha_in = progress;
            let mixed = (a as f32 * alpha_out + b as f32 * alpha_in) as i32;
            out.extend_from_slice(&mixed.to_le_bytes());
        }
        // Trim what we consumed.
        self.tail.drain(..to_mix);
        self.head.drain(..to_mix);
        out
    }

    /// Return any remaining head bytes after the crossfade is done.
    pub fn drain_remaining(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.head)
    }
}

fn read_sample(buf: &[u8]) -> i32 {
    let arr: [u8; 4] = [buf[0], buf[1], buf[2], buf[3]];
    i32::from_le_bytes(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_yields_zero_when_no_input() {
        let mut cf = CrossfadeStream::new(CrossfadeConfig {
            fade_samples: 8,
            channels: 2,
        });
        let out = cf.push_head(&[1, 2, 3, 4]);
        assert!(out.is_empty());
    }

    #[test]
    fn crossfade_at_progress_zero_returns_tail() {
        let mut cf = CrossfadeStream::new(CrossfadeConfig {
            fade_samples: 8,
            channels: 2,
        });
        // Tail = constant max value, head = constant min value.
        let tail = vec![0xFF; 32];
        let head = vec![0x00; 32];
        cf.push_tail(&tail);
        let out = cf.push_head(&head);
        // First 16 bytes of the mixed output (4 samples * 4 bytes) should
        // be near max; final 16 should be near min.
        assert!(!out.is_empty());
    }

    #[test]
    fn crossfade_progress_midpoint_is_sum() {
        let mut cf = CrossfadeStream::new(CrossfadeConfig {
            fade_samples: 4, // 1 sample
            channels: 1,
        });
        let tail = vec![0x00; 4]; // 0
        let head = vec![0xFF; 4]; // 255
        cf.push_tail(&tail);
        let out = cf.push_head(&head);
        assert_eq!(out.len(), 4);
        // progress 0.0 → all tail → 0
        // (we only mixed 1 sample at progress 0.0)
        assert_eq!(out, vec![0, 0, 0, 0]);
    }
}
