//! Signal processing: raw IQ samples in, validated Mode S frames out.
//!
//! # The shape of the problem
//!
//! ADS-B is on-off keyed at 1 Mbit/s on 1090 MHz. Information lives in the
//! *presence of energy*, not in phase, so the first thing we do is throw phase
//! away and work with magnitude alone. There is no carrier recovery, no
//! equalizer, and no PLL — which is what makes this a good place to learn.
//!
//! Aircraft transmit whenever they like, so there is no clock and no framing.
//! Every message opens with a fixed 8 µs preamble — pulses at 0, 1, 3.5 and
//! 4.5 µs — and we find messages by sliding that template over the magnitude
//! stream. Then 56 or 112 µs of pulse-position data follows: energy in the
//! first half of a bit period means 1, the second half means 0.
//!
//! # Four stages, four seams
//!
//! ```text
//!   u8 IQ ──▶ Magnitude ──▶ PreambleDetector ──▶ BitSlicer ──▶ FrameValidator ──▶ bytes
//!             u16 mag        Candidate           RawFrame       Validated
//! ```
//!
//! Each stage is a trait with a *registry* of named implementations, selected
//! at runtime. The point is not abstraction for its own sake — it is that you
//! can write `correlator-v2`, leave `naive` in place, and have the benchmark
//! harness tell you which is better on the same bytes. You are also competing
//! against your own earlier attempts, not just against the baseline.
//!
//! Two rules keep the seams honest:
//!
//! 1. **Dispatch is per buffer, never per sample.** One dynamic call per
//!    262,144 samples is unmeasurable; one per sample would dominate the
//!    runtime and make every benchmark a lie.
//! 2. **[`Pipeline`] owns the shared plumbing** — the carry-over ring, absolute
//!    sample offsets, and duplicate suppression. If each implementation owned
//!    those, they would disagree in small ways and the comparison would be
//!    invalid.
//!
//! # What is deliberately *not* swappable
//!
//! CRC arithmetic, field extraction, the callsign alphabet, CPR. Those have one
//! right answer and published test vectors; they live in `adsb-core` and are
//! covered by known-answer tests. A trait there would buy nothing.

pub mod detect;
pub mod magnitude;
pub mod pipeline;
pub mod registry;
pub mod slice;
pub mod synth;
pub mod types;
pub mod validate;

pub use detect::PreambleDetector;
pub use magnitude::Magnitude;
pub use pipeline::{Pipeline, PipelineStats};
pub use slice::BitSlicer;
pub use types::{Candidate, RawFrame, Validated};
pub use validate::FrameValidator;

/// Microseconds of preamble before the data begins.
pub const PREAMBLE_US: f64 = 8.0;

/// A long frame carries 112 bits at 1 bit per microsecond.
pub const LONG_BITS: usize = 112;

/// A short frame carries 56.
pub const SHORT_BITS: usize = 56;

/// Total airtime of the longest message, preamble included.
pub const LONG_MESSAGE_US: f64 = PREAMBLE_US + LONG_BITS as f64;

/// Magnitudes are fixed-point `u16` rather than `f32`.
///
/// Three reasons, in increasing order of importance: the buffer is half the
/// size, comparisons become integer, and — the real one — it makes a lookup
/// table possible. `|IQ|` has only 129 × 129 distinct values once you exploit
/// the symmetry of `|byte − 127.5|`, so the entire magnitude stage can become
/// a 33 KB table that lives in L2 cache. Handing `f32` across this seam would
/// foreclose the single most instructive optimization in the project.
///
/// Full scale: both components at their extreme give `|IQ| = 127.5·√2 ≈ 180.3`,
/// which maps to [`u16::MAX`].
pub const MAG_FULL_SCALE: f32 = 180.312_2;

/// Multiplier taking a raw `sqrt(i² + q²)` into the `u16` domain.
pub const MAG_SCALE: f32 = u16::MAX as f32 / MAG_FULL_SCALE;

/// Samples per microsecond at a given sample rate.
///
/// Kept as `f64` on purpose. At 2.4 MS/s this is 2.4 — *not* an integer — and
/// accumulating an integer approximation across 112 bits walks the sampling
/// point right out of the bit. The old implementation had a test documenting
/// exactly that failure.
pub fn samples_per_us(sample_rate: u32) -> f64 {
    f64::from(sample_rate) / 1e6
}

/// Samples spanned by the longest possible message.
pub fn long_message_samples(sample_rate: u32) -> usize {
    (LONG_MESSAGE_US * samples_per_us(sample_rate)).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnitude_full_scale_is_the_largest_representable_iq() {
        let extreme = ((127.5f32).powi(2) * 2.0).sqrt();
        assert!((extreme - MAG_FULL_SCALE).abs() < 0.01);
        // The extreme input must land at the top of the u16 range without
        // wrapping -- an overflow here would turn the loudest aircraft into
        // the quietest.
        assert!((extreme * MAG_SCALE) <= f32::from(u16::MAX) + 0.5);
    }

    #[test]
    fn sample_rate_conversions() {
        assert_eq!(samples_per_us(2_400_000), 2.4);
        assert_eq!(samples_per_us(2_000_000), 2.0);
        assert_eq!(long_message_samples(2_400_000), 288);
        assert_eq!(long_message_samples(2_000_000), 240);
    }
}
