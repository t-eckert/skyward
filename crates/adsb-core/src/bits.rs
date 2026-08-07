//! Bit extraction from a Mode S frame.
//!
//! ICAO documents number message bits 1..112. This module is 0-based, because
//! mixing the two conventions in the same codebase is how off-by-one bugs get
//! in. Every call site that quotes a spec bit number subtracts one and says so.

/// Extract `count` bits (max 32) starting at 0-based bit `offset`, MSB-first.
///
/// # Panics
/// If the range runs past the end of `data`, or `count > 32`. These are
/// programming errors, not data errors: frame lengths are validated before
/// decoding, so a slice overrun means the field table is wrong.
#[inline]
pub fn get(data: &[u8], offset: usize, count: usize) -> u32 {
    assert!(count <= 32, "cannot extract {count} bits into a u32");
    assert!(
        offset + count <= data.len() * 8,
        "bits {offset}..{} run past the end of a {}-byte frame",
        offset + count,
        data.len()
    );

    let mut value: u32 = 0;
    for i in 0..count {
        let bit = offset + i;
        let byte = data[bit / 8];
        let shift = 7 - (bit % 8);
        value = (value << 1) | u32::from((byte >> shift) & 1);
    }
    value
}

/// Extract a single bit as a bool.
#[inline]
pub fn flag(data: &[u8], offset: usize) -> bool {
    get(data, offset, 1) == 1
}

/// Pack a slice of 0/1 bytes (as the bit slicer produces) into bytes, MSB-first.
///
/// `bits.len()` should be a multiple of 8; a trailing partial byte is
/// left-aligned, matching how a truncated frame would appear on the wire.
pub fn pack(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for i in 0..8 {
                byte = (byte << 1) | chunk.get(i).copied().unwrap_or(0);
            }
            byte
        })
        .collect()
}

/// Unpack bytes into a vector of 0/1 bytes, MSB-first. Inverse of [`pack`].
pub fn unpack(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for shift in (0..8).rev() {
            out.push((byte >> shift) & 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_across_byte_boundaries() {
        // 0xAC = 1010_1100, 0x35 = 0011_0101
        let data = [0xAC, 0x35];
        assert_eq!(get(&data, 0, 8), 0xAC);
        assert_eq!(get(&data, 0, 4), 0b1010);
        assert_eq!(get(&data, 4, 4), 0b1100);
        // Straddling the boundary: last 4 of byte 0 + first 4 of byte 1.
        assert_eq!(get(&data, 4, 8), 0b1100_0011);
        assert_eq!(get(&data, 0, 16), 0xAC35);
    }

    #[test]
    fn extracts_downlink_format_from_a_real_frame() {
        // 8D... -> 1000_1101; DF is the first 5 bits = 10001 = 17.
        let frame = crate::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        assert_eq!(get(&frame, 0, 5), 17);
        // ICAO is bits 8..32 (spec bits 9..32).
        assert_eq!(get(&frame, 8, 24), 0x4840D6);
    }

    #[test]
    fn pack_unpack_round_trip() {
        let bytes = crate::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        assert_eq!(pack(&unpack(&bytes)), bytes);
    }

    #[test]
    #[should_panic(expected = "run past the end")]
    fn overrun_panics_rather_than_silently_truncating() {
        get(&[0x00], 4, 8);
    }
}
