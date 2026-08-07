//! Mode S / ADS-B message decoding.
//!
//! This crate turns *bytes* into *meaning*. It knows nothing about radios,
//! samples, files, or sockets — that keeps it fast to test and impossible to
//! break with an I/O change.
//!
//! The pipeline upstream of here produces a [`Frame`]: 7 or 14 bytes that have
//! already passed a CRC check. Everything in this crate is spec implementation
//! with published test vectors, which is exactly why none of it sits behind a
//! swappable trait — there is one right answer.
//!
//! ```
//! use adsb_core::{Frame, Message};
//!
//! // A real DF17 identification message.
//! let bytes = adsb_core::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
//! let frame = Frame::new(&bytes).unwrap();
//! assert_eq!(frame.icao().to_string(), "4840D6");
//! match frame.decode() {
//!     Some(Message::Identification { callsign, .. }) => assert_eq!(callsign, "KLM1023"),
//!     other => panic!("expected identification, got {other:?}"),
//! }
//! ```

pub mod bits;
pub mod cpr;
pub mod crc;
pub mod decode;
pub mod frame;
pub mod units;

pub use decode::Message;
pub use frame::{DownlinkFormat, Frame, FrameError};
pub use units::Icao;

/// Parse a hex string into bytes. Convenience for tests and CLI input.
pub fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Render bytes as uppercase hex.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02X}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let hex = "8D4840D6202CC371C32CE0576098";
        let bytes = hex_to_bytes(hex).unwrap();
        assert_eq!(bytes.len(), 14);
        assert_eq!(bytes_to_hex(&bytes), hex);
    }

    #[test]
    fn hex_rejects_odd_length() {
        assert!(hex_to_bytes("8D4").is_none());
    }
}
