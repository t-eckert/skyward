//! The values that travel between pipeline stages.
//!
//! These types are the actual contract of the learning exercise: what one
//! stage can tell the next is what limits how good the next stage can be.

use crate::LONG_BITS;

/// A place the detector believes a preamble begins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Index of the first preamble sample, local to the magnitude slice the
    /// detector was given.
    pub offset: usize,

    /// Sub-sample refinement in `[-0.5, 0.5]`.
    ///
    /// At 2.4 MS/s a bit is 2.4 samples wide, so the true start of a message
    /// almost never lands on a sample boundary — being half a sample out costs
    /// real margin on every one of 112 bits. A detector that interpolates its
    /// correlation peak can say where the edge *actually* was; slicers that do
    /// not care may ignore this.
    pub frac: f32,

    /// Detector-defined confidence. Only comparable within one implementation,
    /// so never threshold on it across implementations.
    pub score: f32,

    /// Estimated noise floor here, in `u16` magnitude units.
    pub noise: u16,

    /// Estimated preamble peak here. With `noise`, gives an RSSI the API can
    /// report and the slicer can use to normalise.
    pub signal: u16,
}

/// A sliced but not yet validated frame.
#[derive(Clone, Debug)]
pub struct RawFrame {
    /// One byte per bit, 0 or 1. Only the first [`RawFrame::len`] are meaningful.
    pub bits: [u8; LONG_BITS],

    /// Per-bit confidence, 0..=255, where 255 means certain.
    ///
    /// This field is the reason the slicer and the validator are separate
    /// stages rather than one. Two-bit error correction over 112 bits is a
    /// 6,216-way search, and at a CRC false-accept rate of 6×10⁻⁸ per trial,
    /// run over millions of candidates, a blind search *will* manufacture
    /// aircraft that do not exist. Restricting flips to the bits the slicer
    /// already doubted makes correction both far cheaper and far safer.
    ///
    /// A slicer that has nothing useful to say should write 255 everywhere and
    /// let the validator fall back to brute force.
    pub conf: [u8; LONG_BITS],

    /// 56 or 112, decided by the downlink format in the first five bits.
    pub len: usize,

    /// Absolute sample index of the preamble start, counted from the beginning
    /// of the stream. Absolute rather than buffer-local so that results do not
    /// depend on how the input happened to be chunked.
    pub offset: u64,

    pub signal: u16,
    pub noise: u16,
}

impl Default for RawFrame {
    fn default() -> Self {
        RawFrame {
            bits: [0; LONG_BITS],
            conf: [0; LONG_BITS],
            len: LONG_BITS,
            offset: 0,
            signal: 0,
            noise: 0,
        }
    }
}

impl RawFrame {
    /// Downlink format: the first five bits.
    pub fn df(&self) -> u8 {
        let mut df = 0u8;
        for i in 0..5 {
            df = (df << 1) | self.bits[i];
        }
        df
    }

    /// How many bits this frame should contain, given its downlink format.
    pub fn expected_len(&self) -> usize {
        if self.df() >= 16 {
            LONG_BITS
        } else {
            crate::SHORT_BITS
        }
    }

    /// Signal-to-noise ratio in dB, or `None` when the noise estimate is zero.
    pub fn snr_db(&self) -> Option<f32> {
        (self.noise > 0).then(|| 20.0 * (f32::from(self.signal) / f32::from(self.noise)).log10())
    }

    /// Indices of the least trusted bits, weakest first. What a confidence-
    /// guided error corrector iterates over.
    pub fn weakest_bits(&self, count: usize) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.len).collect();
        idx.sort_by_key(|&i| self.conf[i]);
        idx.truncate(count);
        idx
    }
}

/// A frame that passed validation and is ready for `adsb-core` to decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validated {
    /// Packed frame bytes. Only the first [`Validated::len`] are meaningful.
    pub bytes: [u8; 14],
    /// 7 or 14.
    pub len: usize,
    /// How many bits error correction had to flip. Zero means the frame
    /// arrived clean; anything above zero deserves more suspicion, and the
    /// scoreboard tracks corrected and uncorrected separately for exactly
    /// that reason.
    pub corrected_bits: u8,
    /// Absolute sample index of the preamble start.
    pub offset: u64,
    pub signal: u16,
    pub noise: u16,
}

impl Validated {
    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn hex(&self) -> String {
        adsb_core::bytes_to_hex(self.payload())
    }

    /// Parse into a validated `adsb-core` frame.
    pub fn frame(&self) -> Option<adsb_core::Frame<'_>> {
        adsb_core::Frame::new(self.payload()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with_df(df: u8) -> RawFrame {
        let mut f = RawFrame::default();
        for i in 0..5 {
            f.bits[i] = (df >> (4 - i)) & 1;
        }
        f
    }

    #[test]
    fn df_is_read_from_the_first_five_bits() {
        assert_eq!(frame_with_df(17).df(), 17);
        assert_eq!(frame_with_df(11).df(), 11);
        assert_eq!(frame_with_df(0).df(), 0);
        assert_eq!(frame_with_df(31).df(), 31);
    }

    #[test]
    fn expected_length_follows_the_df_16_rule() {
        assert_eq!(frame_with_df(17).expected_len(), 112);
        assert_eq!(frame_with_df(11).expected_len(), 56);
        assert_eq!(frame_with_df(4).expected_len(), 56);
        assert_eq!(frame_with_df(20).expected_len(), 112);
    }

    #[test]
    fn weakest_bits_are_returned_weakest_first() {
        let mut f = RawFrame {
            conf: [255; LONG_BITS],
            ..Default::default()
        };
        f.conf[42] = 3;
        f.conf[7] = 1;
        f.conf[99] = 10;
        assert_eq!(f.weakest_bits(3), vec![7, 42, 99]);
    }

    #[test]
    fn snr_is_none_without_a_noise_estimate() {
        let mut f = RawFrame::default();
        assert_eq!(f.snr_db(), None);
        f.noise = 100;
        f.signal = 1000;
        let snr = f.snr_db().unwrap();
        assert!((snr - 20.0).abs() < 0.01, "10x amplitude should be 20 dB");
    }
}
