//! Mode S CRC-24.
//!
//! The last 24 bits of every Mode S frame are a parity field. Running the
//! whole frame — payload *and* parity — through the generator leaves zero for
//! an undamaged DF17. There is no handshake and no retransmission on 1090 MHz,
//! so this check is the only thing standing between you and garbage.
//!
//! The generator is `0xFFF409`. Note that this is *not*
//! `x^24 + x^23 + x^10 + x^3 + 1` as the old implementation's comment claimed:
//! `0xFFF409` has bits 23..12 set, plus 10, 3 and 0 — twelve more terms than
//! that. The polynomial is chosen so that all burst errors up to 24 bits are
//! detected, which matters when interference clobbers a run of adjacent bits.

/// Mode S CRC-24 generator polynomial.
pub const POLY: u32 = 0x00FF_F409;

/// Compute the CRC-24 over `data`.
///
/// For a frame that includes its parity field, a clean message returns 0.
/// For DF11 the result is the interrogator identifier (usually 0), and for
/// DF0/4/5/16/20/21 it is the ICAO address XORed in by the transmitter —
/// see [`remainder_is_address`].
pub fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &byte in data {
        for shift in (0..8).rev() {
            let bit = u32::from((byte >> shift) & 1);
            let msb = (crc >> 23) & 1;
            crc = ((crc << 1) & 0x00FF_FFFF) | bit;
            if msb != 0 {
                crc ^= POLY;
            }
        }
    }
    crc & 0x00FF_FFFF
}

/// True for the downlink formats whose parity has the aircraft address XORed
/// into it, meaning the CRC remainder *is* the address and cannot be used to
/// validate the frame.
///
/// This is a genuine hazard: the remainder of pure noise is a perfectly
/// plausible-looking ICAO address, so these formats are a ghost-aircraft
/// generator. Accept them only for addresses already confirmed by a
/// CRC-validated DF17/18.
pub const fn remainder_is_address(df: u8) -> bool {
    matches!(df, 0 | 4 | 5 | 16 | 20 | 21)
}

/// The syndrome of a frame: what the CRC leaves behind.
///
/// Zero means clean. Non-zero means either damage or an address-overlaid
/// format. Because the CRC is linear, the syndrome of a damaged frame equals
/// the syndrome of the error pattern alone — which is what makes single- and
/// double-bit correction a table lookup rather than a search. Building that
/// table is left as an exercise; see `docs/LEARNING.md` stage 4.
#[inline]
pub fn syndrome(data: &[u8]) -> u32 {
    crc24(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex_to_bytes;

    /// The canonical example from the ADS-B literature.
    #[test]
    fn clean_df17_has_zero_remainder() {
        let frame = hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        assert_eq!(crc24(&frame), 0, "a valid DF17 must leave zero");
    }

    #[test]
    fn real_captured_frames_have_zero_remainder() {
        // Decoded off the air in Ottawa, 2026-08-06.
        for hex in [
            "8DC060B6587D8236085C837FDA27",
            "8DC060B6990D2985500834E20E9E",
            "8DC053F699093B19D030164300A1",
            "8D40621D58C382D690C8AC2863A7",
        ] {
            let frame = hex_to_bytes(hex).unwrap();
            assert_eq!(crc24(&frame), 0, "{hex} should be clean");
        }
    }

    #[test]
    fn a_single_flipped_bit_is_detected() {
        let mut frame = hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        for bit in 0..112 {
            frame[bit / 8] ^= 1 << (7 - (bit % 8));
            assert_ne!(crc24(&frame), 0, "flipping bit {bit} must be detected");
            frame[bit / 8] ^= 1 << (7 - (bit % 8));
        }
    }

    /// The property that makes error correction possible: the syndrome depends
    /// only on the error pattern, not on the message it damaged.
    #[test]
    fn syndrome_is_linear_in_the_error_pattern() {
        let a = hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let b = hex_to_bytes("8D40621D58C382D690C8AC2863A7").unwrap();

        for bit in [0usize, 7, 33, 64, 111] {
            let mut da = a.clone();
            let mut db = b.clone();
            da[bit / 8] ^= 1 << (7 - (bit % 8));
            db[bit / 8] ^= 1 << (7 - (bit % 8));
            assert_eq!(
                crc24(&da),
                crc24(&db),
                "the same flipped bit must give the same syndrome in any message"
            );
        }
    }

    #[test]
    fn address_overlaid_formats_are_identified() {
        assert!(remainder_is_address(4));
        assert!(remainder_is_address(20));
        assert!(!remainder_is_address(17));
        assert!(!remainder_is_address(11));
    }
}
