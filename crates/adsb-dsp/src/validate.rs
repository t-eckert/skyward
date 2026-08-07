//! Stage 4 — deciding whether to believe a frame.
//!
//! There is no handshake on 1090 MHz, no acknowledgement, and no
//! retransmission. The 24-bit parity field is the *only* thing between you and
//! garbage, and it is doing a great deal of work: a clean DF17 leaves a
//! remainder of exactly zero, and the odds of noise doing that by accident are
//! about 6 × 10⁻⁸.
//!
//! That number is also why this stage is dangerous. See below.
//!
//! # Where the learning is
//!
//! [`CrcOnlyValidator`] accepts a frame only if it is already perfect. But
//! Mode S CRC-24 is a genuine linear error-correcting code, and that opens up
//! the most satisfying stage in the project:
//!
//! 1. **Single-bit correction is a lookup.** Because the code is linear, the
//!    syndrome of a damaged frame equals the syndrome of the error pattern
//!    alone — independent of the message underneath. So compute the CRC of
//!    each of the 112 one-hot vectors once, put them in a map, and any
//!    single-bit error is one hash lookup and one XOR.
//!    `adsb_core::crc::syndrome` is the primitive; the linearity property has
//!    a test in that module if you want to convince yourself first.
//! 2. **Two-bit correction** needs C(112,2) = 6,216 entries, about 25 KB.
//! 3. **Then the important part.** 6,216 trials at 6 × 10⁻⁸ each is roughly
//!    4 × 10⁻⁴ per candidate, and you will run millions of candidates. You are
//!    now manufacturing aircraft that do not exist. Two defences, both worth
//!    implementing: only flip bits the slicer marked low-confidence (which
//!    requires stage 3 to actually report confidence), and refuse to publish
//!    an ICAO seen only once.
//!
//! Watch `ghost_icao_ratio` on the scoreboard while you do this. It is the
//! metric that turns "more messages" into an honest claim.
//!
//! # The formats you cannot validate at all
//!
//! DF0, 4, 5, 16, 20 and 21 have the aircraft address XORed into their parity,
//! so the remainder *is* the address and there is nothing left to check
//! against. The remainder of pure noise is a perfectly plausible-looking ICAO.
//! Treat these as unverified: accept them only for addresses already confirmed
//! by a CRC-clean DF17/18.

use crate::{RawFrame, Validated};
use adsb_core::crc;

/// Decides whether a sliced frame is real, optionally repairing it.
pub trait FrameValidator: Send + Sync {
    /// Registry name, as used by `--validate`.
    fn name(&self) -> &'static str;

    /// One line for `--list-impls`.
    fn describe(&self) -> &'static str;

    /// Return the frame's bytes if it should be believed.
    fn validate(&self, raw: &RawFrame) -> Option<Validated>;
}

/// Pack the meaningful bits of a raw frame into bytes.
fn pack(raw: &RawFrame) -> ([u8; 14], usize) {
    let mut bytes = [0u8; 14];
    let len = raw.len / 8;
    for (i, slot) in bytes.iter_mut().enumerate().take(len) {
        let mut b = 0u8;
        for k in 0..8 {
            b = (b << 1) | raw.bits[i * 8 + k];
        }
        *slot = b;
    }
    (bytes, len)
}

/// Accept only frames that are already perfect.
///
/// No error correction, so it can never invent an aircraft — the safest
/// possible validator and the right baseline to measure correction against.
#[derive(Default, Clone, Copy)]
pub struct CrcOnlyValidator;

impl FrameValidator for CrcOnlyValidator {
    fn name(&self) -> &'static str {
        "crc-only"
    }

    fn describe(&self) -> &'static str {
        "accept only an exact CRC match; no error correction, cannot invent aircraft"
    }

    fn validate(&self, raw: &RawFrame) -> Option<Validated> {
        let (bytes, len) = pack(raw);
        let payload = &bytes[..len];
        let df = raw.df();

        let ok = match df {
            // Extended squitter: the parity must cancel exactly.
            17 | 18 => crc::crc24(payload) == 0,
            // All-call reply: the remainder is the interrogator identifier,
            // which is conventionally small. Anything in the high bits means
            // the frame is damaged.
            11 => crc::crc24(payload) & 0x00FF_FF80 == 0,
            // Address-overlaid formats cannot be checked standalone. Rejecting
            // them here is a deliberate choice: the tracker re-admits them for
            // addresses a DF17 has already confirmed.
            _ => false,
        };

        ok.then_some(Validated {
            bytes,
            len,
            corrected_bits: 0,
            offset: raw.offset,
            signal: raw.signal,
            noise: raw.noise,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LONG_BITS;

    fn raw_from_hex(hex: &str) -> RawFrame {
        let bytes = adsb_core::hex_to_bytes(hex).unwrap();
        let bits = adsb_core::bits::unpack(&bytes);
        let mut f = RawFrame {
            len: bits.len(),
            ..Default::default()
        };
        f.bits[..bits.len()].copy_from_slice(&bits);
        f.conf = [255; LONG_BITS];
        f
    }

    #[test]
    fn accepts_a_clean_extended_squitter() {
        let raw = raw_from_hex("8D4840D6202CC371C32CE0576098");
        let v = CrcOnlyValidator.validate(&raw).expect("should validate");
        assert_eq!(v.hex(), "8D4840D6202CC371C32CE0576098");
        assert_eq!(v.corrected_bits, 0);
        assert_eq!(v.len, 14);
    }

    #[test]
    fn accepts_a_clean_all_call_reply() {
        let raw = raw_from_hex("5DC04413BFAD35");
        let v = CrcOnlyValidator
            .validate(&raw)
            .expect("DF11 should validate");
        assert_eq!(v.len, 7);
    }

    #[test]
    fn rejects_every_single_bit_error() {
        let clean = raw_from_hex("8D4840D6202CC371C32CE0576098");
        for bit in 0..112 {
            let mut damaged = clean.clone();
            damaged.bits[bit] ^= 1;
            assert!(
                CrcOnlyValidator.validate(&damaged).is_none(),
                "bit {bit} flipped but the frame was still accepted"
            );
        }
    }

    #[test]
    fn rejects_address_overlaid_formats() {
        // DF4 -- the remainder is the ICAO, so there is nothing to verify.
        let mut raw = raw_from_hex("5DC04413BFAD35");
        raw.bits[0] = 0;
        raw.bits[1] = 0;
        raw.bits[2] = 1;
        raw.bits[3] = 0;
        raw.bits[4] = 0;
        assert_eq!(raw.df(), 4);
        assert!(CrcOnlyValidator.validate(&raw).is_none());
    }

    #[test]
    fn a_validated_frame_parses_as_core_frame() {
        let raw = raw_from_hex("8DC060B6587D8236085C837FDA27");
        let v = CrcOnlyValidator.validate(&raw).unwrap();
        let frame = v.frame().expect("should parse");
        assert_eq!(frame.icao().to_string(), "C060B6");
        assert!(frame.crc_ok());
    }
}
