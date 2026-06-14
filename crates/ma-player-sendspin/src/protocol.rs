//! Encoding helpers for outbound JSON / binary frames.
//!
//! Sendspin text messages use the envelope shape `{"type": "...", "payload": ...}`
//! for all messages except `client/time` and `server/time` which are flat.
//! Binary frames start with a uint8 message-type byte.

use bytes::{BufMut, BytesMut};

use ma_protocol_sendspin::binary::{
    encode_audio_or_artwork, MSG_TYPE_ARTWORK_BASE, MSG_TYPE_AUDIO_CHUNK,
};
use ma_protocol_sendspin::messages::{
    GroupUpdate, ServerCommand, ServerHello, ServerState, StreamClear, StreamEnd, StreamStart,
};
use ma_protocol_sendspin::roles::AudioFormat;
use serde::Serialize;

/// Encode an envelope `{"type": "...", "payload": ...}` as a JSON string.
pub fn envelope_json<T: Serialize>(
    msg_type: &str,
    payload: &T,
) -> Result<String, serde_json::Error> {
    let v = serde_json::json!({ "type": msg_type, "payload": payload });
    serde_json::to_string(&v)
}

/// Encode a `server/time` flat message.
pub fn server_time_json(
    client_transmitted: i64,
    server_received: i64,
    server_transmitted: i64,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({
        "type": "server/time",
        "client_transmitted": client_transmitted,
        "server_received": server_received,
        "server_transmitted": server_transmitted,
    }))
}

pub fn server_hello_envelope(h: &ServerHello) -> Result<String, serde_json::Error> {
    envelope_json("server/hello", h)
}

pub fn server_state_envelope(s: &ServerState) -> Result<String, serde_json::Error> {
    envelope_json("server/state", s)
}

pub fn server_command_envelope(c: &ServerCommand) -> Result<String, serde_json::Error> {
    envelope_json("server/command", c)
}

pub fn group_update_envelope(g: &GroupUpdate) -> Result<String, serde_json::Error> {
    envelope_json("group/update", g)
}

pub fn stream_start_envelope(s: &StreamStart) -> Result<String, serde_json::Error> {
    envelope_json("stream/start", s)
}

pub fn stream_end_envelope(s: &StreamEnd) -> Result<String, serde_json::Error> {
    envelope_json("stream/end", s)
}

pub fn stream_clear_envelope(s: &StreamClear) -> Result<String, serde_json::Error> {
    envelope_json("stream/clear", s)
}

/// Wrap an `AudioFormat` and its (optional) base64 codec header into a
/// `stream/start` envelope for the player role.
pub fn stream_start_player(
    format: AudioFormat,
    codec_header_b64: Option<String>,
) -> Result<String, serde_json::Error> {
    stream_start_envelope(&StreamStart::player(format, codec_header_b64))
}

/// Build an audio binary frame: type 4 + 8-byte BE timestamp + encoded audio.
pub fn audio_frame(timestamp_us: i64, payload: &[u8]) -> Vec<u8> {
    encode_audio_or_artwork(timestamp_us, payload, MSG_TYPE_AUDIO_CHUNK).to_vec()
}

/// Build an artwork binary frame: type 8 + 8-byte BE timestamp + image bytes.
/// `channel` must be in 0..=3.
pub fn artwork_frame(channel: u8, timestamp_us: i64, image: &[u8]) -> Option<Vec<u8>> {
    if channel > 3 {
        return None;
    }
    Some(encode_audio_or_artwork(timestamp_us, image, MSG_TYPE_ARTWORK_BASE + channel).to_vec())
}

/// Build a visualizer binary frame.
pub fn visualizer_frame(slot: u8, timestamp_us: i64, payload: &[u8]) -> Option<Vec<u8>> {
    if slot > 7 {
        return None;
    }
    Some(encode_audio_or_artwork(timestamp_us, payload, 16 + slot).to_vec())
}

/// Build a 16-bit BE value (used for loudness / spectrum / f_peak scaling).
pub fn put_u16_be(value: u16) -> [u8; 2] {
    [(value >> 8) as u8, value as u8]
}

/// Encode a loudness value (dB) to a uint16 BE in the spec's scaling
/// (`-60 dB → 0`, `0 dB → 65535`).
pub fn encode_loudness(db: f32) -> [u8; 2] {
    let clamped = db.clamp(-60.0, 0.0);
    let v = ((clamped + 60.0) / 60.0 * 65535.0).round() as u16;
    put_u16_be(v)
}

/// Encode a peak frequency payload (`uint16 freq` + `uint16 amp`).
pub fn encode_f_peak(freq_hz: u16, amp_scaled: u16) -> [u8; 4] {
    let f = put_u16_be(freq_hz);
    let a = put_u16_be(amp_scaled);
    [f[0], f[1], a[0], a[1]]
}

/// Encode a spectrum payload: `n_disp_bins` uint16 BE values.
pub fn encode_spectrum(bins: &[u16]) -> Vec<u8> {
    let mut out = BytesMut::with_capacity(bins.len() * 2);
    for b in bins {
        out.put_u16(*b);
    }
    out.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shape() {
        let s = envelope_json("server/hello", &serde_json::json!({"a": 1})).unwrap();
        assert!(s.contains("\"type\":\"server/hello\""));
        assert!(s.contains("\"payload\":{\"a\":1}"));
    }

    #[test]
    fn server_time_flat_shape() {
        let s = server_time_json(10, 20, 30).unwrap();
        assert!(s.contains("\"type\":\"server/time\""));
        assert!(s.contains("\"client_transmitted\":10"));
        assert!(s.contains("\"server_received\":20"));
        assert!(s.contains("\"server_transmitted\":30"));
        assert!(!s.contains("\"payload\""));
    }

    #[test]
    fn audio_frame_layout() {
        let frame = audio_frame(0x0102_0304_0506_0708, &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(frame[0], MSG_TYPE_AUDIO_CHUNK);
        // BE i64 = 0x01 02 03 04 05 06 07 08
        assert_eq!(
            &frame[1..9],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
        assert_eq!(&frame[9..], &[0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn artwork_frame_channel_range() {
        assert!(artwork_frame(0, 0, &[1, 2]).is_some());
        assert!(artwork_frame(3, 0, &[1, 2]).is_some());
        assert!(artwork_frame(4, 0, &[1, 2]).is_none());
    }

    #[test]
    fn loudness_scaling() {
        // -60 dB → 0
        assert_eq!(encode_loudness(-60.0), [0, 0]);
        // 0 dB → 65535
        assert_eq!(encode_loudness(0.0), [0xff, 0xff]);
        // -30 dB → midpoint (32767 or 32768)
        let mid = u16::from_be_bytes(encode_loudness(-30.0));
        assert!(mid > 32_000 && mid < 33_000);
    }
}
