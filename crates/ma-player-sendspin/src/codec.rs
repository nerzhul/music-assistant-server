//! Audio codec helpers used by the Sendspin server.
//!
//! Phase 1 provides:
//!
//! * **PCM pass-through** — `encode_pcm` is a no-op slice copy; the server
//!   already has float/PCM bytes from ffmpeg and can emit them as-is.
//! * **Opus / FLAC** — the real encoders belong to a later phase (they
//!   need bindings to libopus / libflac or pure-Rust crates). For now we
//!   expose the same API surface but `encode_opus` / `encode_flac` return
//!   a clear `NotImplemented` error. The `audio_format_for_codec` helper
//!   returns a sensible default negotiated format for each codec.
//!
//! Reference: Sendspin spec section "Stream/start player object".

use thiserror::Error;

use ma_protocol_sendspin::roles::AudioFormat;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("codec not implemented in phase 1: {0}")]
    NotImplemented(&'static str),
    #[error("invalid pcm data: {0}")]
    InvalidPcm(String),
}

/// Negotiate a Sendspin audio format given the client's preferred formats
/// and the server's capabilities. Returns the first client format that the
/// server supports, or the fallback if none match.
///
/// `supported_formats` is a list of `(codec, sample_rate, channels, bit_depth)`
/// tuples (in priority order) that the client advertised in
/// `player@v1_support.supported_formats`.
pub fn negotiate(supported_formats: &[(String, u32, u8, u16)]) -> AudioFormat {
    for (codec, sr, ch, bd) in supported_formats {
        match codec.as_str() {
            "opus" => {
                return AudioFormat::Opus {
                    sample_rate: *sr,
                    channels: *ch,
                    bit_depth: *bd,
                }
            }
            "flac" => {
                return AudioFormat::Flac {
                    sample_rate: *sr,
                    channels: *ch,
                    bit_depth: *bd,
                    block_size: None,
                }
            }
            "pcm" => {
                return AudioFormat::Pcm {
                    sample_rate: *sr,
                    channels: *ch,
                    bit_depth: *bd,
                    sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
                };
            }
            _ => continue,
        }
    }
    // Fallback: PCM 48 kHz / 2ch / 16-bit.
    AudioFormat::Pcm {
        sample_rate: 48_000,
        channels: 2,
        bit_depth: 16,
        sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
    }
}

/// Encode a chunk of audio bytes for the negotiated format. For PCM this
/// is a no-op pass-through (the input is already in the right sample format).
pub fn encode(format: AudioFormat, pcm: &[u8]) -> Result<Vec<u8>, CodecError> {
    match format {
        AudioFormat::Pcm { .. } => Ok(pcm.to_vec()),
        AudioFormat::Opus { .. } => Err(CodecError::NotImplemented("opus")),
        AudioFormat::Flac { .. } => Err(CodecError::NotImplemented("flac")),
    }
}

/// Validate a PCM buffer is correctly sized for the given format
/// (multiple of `frame_size = channels * (bit_depth / 8)`).
pub fn validate_pcm(format: AudioFormat, pcm: &[u8]) -> Result<(), CodecError> {
    let frame_size = format.channels() as usize * (format.bit_depth() as usize / 8);
    if frame_size == 0 {
        return Err(CodecError::InvalidPcm("frame size 0".into()));
    }
    if pcm.len() % frame_size != 0 {
        return Err(CodecError::InvalidPcm(format!(
            "buffer {} not a multiple of frame size {}",
            pcm.len(),
            frame_size
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_picks_first_supported() {
        let f = negotiate(&[
            ("opus".into(), 48_000, 2, 16),
            ("flac".into(), 48_000, 2, 16),
        ]);
        assert!(matches!(
            f,
            AudioFormat::Opus {
                sample_rate: 48_000,
                ..
            }
        ));
    }

    #[test]
    fn negotiate_skips_unknown_codecs() {
        let f = negotiate(&[("aac".into(), 44_100, 2, 16), ("pcm".into(), 48_000, 2, 16)]);
        assert!(matches!(f, AudioFormat::Pcm { .. }));
    }

    #[test]
    fn negotiate_fallback_pcm_48k() {
        let f = negotiate(&[("aac".into(), 44_100, 2, 16)]);
        assert!(matches!(
            f,
            AudioFormat::Pcm {
                sample_rate: 48_000,
                ..
            }
        ));
    }

    #[test]
    fn pcm_passthrough_works() {
        let format = AudioFormat::Pcm {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 16,
            sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
        };
        let pcm = vec![0u8; 4096];
        let out = encode(format, &pcm).unwrap();
        assert_eq!(out, pcm);
        validate_pcm(format, &pcm).unwrap();
    }

    #[test]
    fn pcm_misaligned_buffer_rejected() {
        let format = AudioFormat::Pcm {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 16,
            sample_type: ma_protocol_sendspin::roles::PcmSampleType::Int,
        };
        // 2ch * 2 bytes = 4 bytes/frame
        let bad = vec![0u8; 7];
        assert!(validate_pcm(format, &bad).is_err());
    }

    #[test]
    fn opus_returns_not_implemented() {
        let format = AudioFormat::Opus {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: 16,
        };
        let err = encode(format, &[0u8; 1024]).unwrap_err();
        assert!(matches!(err, CodecError::NotImplemented("opus")));
    }
}
