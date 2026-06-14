//! Smart-fades mixer — volume normalisation on chunk boundaries.
//!
//! The Python `smart_fades.mixer` is a full beat-aligned crossfader
//! (BPM detection, frequency analysis, similar-mood matching).
//! Phase 2 implements just the volume-levelling part: the
//! `SmartFadesMixer` applies either:
//!
//! * a fixed gain (e.g. -6 dB for tracks, configurable per user),
//! * or a "fallback dynamic" mode where the loudness is estimated
//!   per chunk and the gain is adjusted to land at -16 LUFS,
//!
//! before the chunk is forwarded to the player. The dynamic mode
//! uses a simple RMS estimator; a proper EBU R128 / ReplayGain
//! implementation lives in a later phase.

use bytes::Bytes;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SmartFadesMode {
    /// Off — pass chunks through unchanged.
    Disabled,
    /// Apply a fixed gain in dB.
    FixedGain,
    /// Estimate per-chunk RMS and normalise to a target.
    #[default]
    FallbackDynamic,
}

#[derive(Debug, Clone)]
pub struct SmartFadesMixer {
    mode: SmartFadesMode,
    /// Fixed gain in dB. Default -6 dB.
    fixed_gain_db: f32,
    /// Target RMS for fallback dynamic mode. Default 0.2 (-14 dBFS).
    target_rms: f32,
    /// Cached running gain so we don't bounce around on every chunk.
    last_gain_linear: f32,
}

impl SmartFadesMixer {
    pub fn new(mode: SmartFadesMode) -> Self {
        Self {
            mode,
            fixed_gain_db: -6.0,
            target_rms: 0.2,
            last_gain_linear: 1.0,
        }
    }

    pub fn with_fixed_gain_db(mut self, db: f32) -> Self {
        self.fixed_gain_db = db;
        self
    }

    pub fn with_target_rms(mut self, target: f32) -> Self {
        self.target_rms = target;
        self
    }

    /// Apply the configured normalisation to one PCM chunk. We
    /// assume the chunk is little-endian signed 16-bit PCM stereo
    /// (the standard intermediate format inside the streams
    /// pipeline). For a real production version we would inspect
    /// the actual format; Phase 2 just covers the common case.
    pub fn process(&mut self, chunk: &Bytes) -> Bytes {
        match self.mode {
            SmartFadesMode::Disabled => chunk.clone(),
            SmartFadesMode::FixedGain => {
                let linear = db_to_linear(self.fixed_gain_db);
                scale_pcm16(chunk, linear)
            }
            SmartFadesMode::FallbackDynamic => {
                let rms = rms_pcm16(chunk);
                let desired_gain = if rms > 1e-4 {
                    (self.target_rms / rms).clamp(0.1, 10.0)
                } else {
                    1.0
                };
                // Smooth the gain change to avoid pumping.
                self.last_gain_linear = 0.7 * self.last_gain_linear + 0.3 * desired_gain;
                scale_pcm16(chunk, self.last_gain_linear)
            }
        }
    }
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn rms_pcm16(samples: &[u8]) -> f32 {
    // Pairwise u16 LE → i16 → squared sum.
    let mut sum_sq: f64 = 0.0;
    let mut n: u64 = 0;
    let chunks = samples.chunks_exact(2);
    for pair in chunks {
        let v = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0;
        sum_sq += (v * v) as f64;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    ((sum_sq / n as f64) as f32).sqrt()
}

fn scale_pcm16(samples: &[u8], gain: f32) -> Bytes {
    let mut out = Vec::with_capacity(samples.len());
    for pair in samples.chunks_exact(2) {
        let v = i16::from_le_bytes([pair[0], pair[1]]) as f32;
        let scaled = (v * gain).clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    Bytes::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_linear_matches_formula() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-4);
        assert!((db_to_linear(-6.0) - 0.5012).abs() < 1e-2);
        assert!((db_to_linear(20.0) - 10.0).abs() < 1e-2);
    }

    #[test]
    fn rms_zero_for_silence() {
        let silence = vec![0u8; 64];
        assert_eq!(rms_pcm16(&silence), 0.0);
    }

    #[test]
    fn rms_near_one_for_max_sine() {
        // 64 alternating max samples.
        let mut body = Vec::new();
        for i in 0..32 {
            let v: i16 = if i % 2 == 0 { 32767 } else { -32768 };
            body.extend_from_slice(&v.to_le_bytes());
        }
        let rms = rms_pcm16(&body);
        assert!(rms > 0.95 && rms < 1.05, "rms={rms}");
    }

    #[test]
    fn scale_reduces_amplitude() {
        let v: i16 = 16384;
        let mut body = Vec::new();
        body.extend_from_slice(&v.to_le_bytes());
        body.extend_from_slice(&v.to_le_bytes());
        let out = scale_pcm16(&body, 0.5);
        let got = i16::from_le_bytes([out[0], out[1]]);
        assert_eq!(got, 8192);
    }

    #[test]
    fn fixed_gain_clamps_to_i16() {
        let v: i16 = 30000;
        let mut body = Vec::new();
        body.extend_from_slice(&v.to_le_bytes());
        let out = scale_pcm16(&body, 2.0);
        let got = i16::from_le_bytes([out[0], out[1]]);
        assert_eq!(got, 32767);
    }

    #[test]
    fn dynamic_mode_silences_silent_chunk() {
        let mut mixer = SmartFadesMixer::new(SmartFadesMode::FallbackDynamic);
        let silence = vec![0u8; 1024];
        let out = mixer.process(&Bytes::from(silence));
        // After the gain converges to 1.0, silence is still silence.
        let mut all_zero = true;
        for pair in out.chunks_exact(2) {
            if pair != [0, 0] {
                all_zero = false;
                break;
            }
        }
        assert!(all_zero);
    }
}
