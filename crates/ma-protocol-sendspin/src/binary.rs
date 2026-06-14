//! Binary message-type IDs and framing helpers.
//!
//! The first byte of a Sendspin binary WebSocket message is a uint8 that
//! identifies the role + slot. The remaining bytes are role-specific; the
//! audio and artwork roles always start with an 8-byte big-endian int64
//! server-clock timestamp (microseconds) followed by the encoded payload.

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Reserved message types 0..=3.
pub const MSG_TYPE_RESERVED: u8 = 0;
/// Player role: slot 0 → audio chunks (the only currently-defined slot).
pub const MSG_TYPE_AUDIO_CHUNK: u8 = 4;
/// Artwork role: slots 0..=3 → channels 0..=3.
pub const MSG_TYPE_ARTWORK_BASE: u8 = 8;
/// Visualizer role: 8 slots (16..=23). See [`visualizer`].
pub const MSG_TYPE_VISUALIZER_BASE: u8 = 16;
/// Lowest application-specific message type (server/clients may use 192..=255).
pub const MSG_TYPE_APP_BASE: u8 = 192;

/// Visualizer binary message-type IDs.
pub mod visualizer {
    use super::MSG_TYPE_VISUALIZER_BASE;
    /// A-weighted loudness in dB, uint16, scaled 0..=65535 = −60..=0 dB.
    pub const LOUDNESS: u8 = MSG_TYPE_VISUALIZER_BASE;
    /// Transient onset flag (uint8, bit 0 = downbeat).
    pub const BEAT: u8 = MSG_TYPE_VISUALIZER_BASE + 1;
    /// Dominant frequency peak: uint16 Hz + uint16 amplitude.
    pub const F_PEAK: u8 = MSG_TYPE_VISUALIZER_BASE + 2;
    /// Spectrum: n_disp_bins × uint16 (low → high freq).
    pub const SPECTRUM: u8 = MSG_TYPE_VISUALIZER_BASE + 3;
    /// Onset strength: uint8 (0..=255).
    pub const PEAK: u8 = MSG_TYPE_VISUALIZER_BASE + 4;
    // 21..=23 are reserved by the spec and MUST NOT be used.
}

/// Compose an artwork binary message (8-byte big-endian timestamp + image bytes).
pub fn encode_audio_or_artwork(timestamp_us: i64, payload: &[u8], msg_type: u8) -> Bytes {
    let mut buf = BytesMut::with_capacity(9 + payload.len());
    buf.put_u8(msg_type);
    buf.put_i64(timestamp_us);
    buf.extend_from_slice(payload);
    buf.freeze()
}

/// Parse the 8-byte big-endian timestamp from a binary audio/artwork message.
pub fn decode_timestamp(payload: &[u8]) -> Option<i64> {
    if payload.len() < 8 {
        return None;
    }
    let mut buf = payload;
    Some(buf.get_i64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let original = vec![0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03];
        let msg = encode_audio_or_artwork(123_456_789, &original, MSG_TYPE_AUDIO_CHUNK);
        assert_eq!(msg[0], MSG_TYPE_AUDIO_CHUNK);
        assert_eq!(decode_timestamp(&msg[1..]), Some(123_456_789));
        assert_eq!(&msg[9..], &original[..]);
    }

    #[test]
    fn visualizer_ids_in_range() {
        assert_eq!(visualizer::LOUDNESS, 16);
        assert_eq!(visualizer::BEAT, 17);
        assert_eq!(visualizer::F_PEAK, 18);
        assert_eq!(visualizer::SPECTRUM, 19);
        assert_eq!(visualizer::PEAK, 20);
    }

    #[test]
    fn artwork_id_for_channel() {
        assert_eq!(MSG_TYPE_ARTWORK_BASE, 8);
        for ch in 0..4u8 {
            let t = MSG_TYPE_ARTWORK_BASE + ch;
            assert!((8..=11).contains(&t));
        }
    }
}
