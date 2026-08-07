//! Stage 1 — IQ to magnitude.
//!
//! ADS-B is on-off keyed, so phase carries nothing and `sqrt(i² + q²)` is the
//! whole of the demodulator. That makes this the cheapest stage conceptually
//! and, on a Raspberry Pi at 2.4 million samples per second, one of the most
//! expensive in wall-clock terms.
//!
//! # Where the learning is
//!
//! This stage should never change *what* you decode — only how fast. That
//! makes it the right place to learn to trust the pipeline digest: if a
//! magnitude change alters the message set, you have a bug, not an
//! optimization. Ideas, roughly in order of how much they teach:
//!
//! 1. **A lookup table.** `|byte − 127.5|` takes only 129 distinct values, so
//!    the whole function is a 129 × 129 table of `u16` — 33 KB, comfortably
//!    inside L2. This is what dump1090 does, and it is why this seam takes raw
//!    `u8` rather than pre-converted floats.
//! 2. **Alpha-max-plus-beta-min.** `max + 0.4·min` approximates the hypotenuse
//!    with no multiply and about 4% peak error. Try it, then measure: the
//!    preamble test is a *ratio* test, and 4% of error at low SNR costs you
//!    precisely the marginal aircraft you were trying to recover. Discovering
//!    that it is not good enough is the point of trying it.
//! 3. **SIMD.** `std::simd`, or NEON intrinsics on the Pi.

use crate::MAG_SCALE;

/// Converts interleaved unsigned 8-bit IQ into `u16` magnitudes.
pub trait Magnitude: Send {
    /// Registry name, as used by `--mag`.
    fn name(&self) -> &'static str;

    /// One line for `--list-impls`.
    fn describe(&self) -> &'static str;

    /// `iq` is interleaved I,Q bytes. `out.len()` must equal `iq.len() / 2`.
    fn compute(&self, iq: &[u8], out: &mut [u16]);
}

/// The obvious implementation: convert to float, square, add, square-root.
///
/// Correct, slow, and the reference every other implementation is compared
/// against. Do not optimize this one — it is the control.
#[derive(Default, Clone, Copy)]
pub struct NaiveMagnitude;

impl Magnitude for NaiveMagnitude {
    fn name(&self) -> &'static str {
        "naive"
    }

    fn describe(&self) -> &'static str {
        "sqrt(i^2 + q^2) in f32; the correctness reference"
    }

    fn compute(&self, iq: &[u8], out: &mut [u16]) {
        debug_assert_eq!(out.len(), iq.len() / 2);
        for (k, slot) in out.iter_mut().enumerate() {
            // rtl_sdr emits offset binary: 0..255 with 127.5 as zero.
            let i = f32::from(iq[2 * k]) - 127.5;
            let q = f32::from(iq[2 * k + 1]) - 127.5;
            let m = (i * i + q * q).sqrt() * MAG_SCALE;
            *slot = m.min(f32::from(u16::MAX)) as u16;
        }
    }
}

/// Convenience for tests and one-off analysis.
pub fn compute_vec(mag: &dyn Magnitude, iq: &[u8]) -> Vec<u16> {
    let mut out = vec![0u16; iq.len() / 2];
    mag.compute(iq, &mut out);
    out
}

/// Estimate a noise floor from a magnitude buffer.
///
/// Uses a low percentile rather than the mean: ADS-B bursts are rare and
/// bright, so the mean is dragged upward by exactly the signals we are trying
/// to distinguish from the floor. Sub-sampled because an exact percentile over
/// a 262,144-sample buffer is not worth the cache traffic.
pub fn noise_floor(mag: &[u16]) -> u16 {
    if mag.is_empty() {
        return 0;
    }
    let stride = (mag.len() / 4096).max(1);
    let mut sample: Vec<u16> = mag.iter().copied().step_by(stride).collect();
    sample.sort_unstable();
    sample[sample.len() / 2].max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_signal_is_zero_magnitude() {
        // 127/128 straddle the 127.5 zero point, so magnitude is the smallest
        // non-zero value rather than exactly 0.
        let out = compute_vec(&NaiveMagnitude, &[128, 128]);
        assert!(out[0] < 400, "near-zero IQ gave {}", out[0]);
    }

    #[test]
    fn extreme_input_reaches_full_scale_without_wrapping() {
        for pair in [[255u8, 255], [0, 0], [255, 0], [0, 255]] {
            let out = compute_vec(&NaiveMagnitude, &pair);
            assert!(out[0] > 40_000, "{pair:?} gave {}", out[0]);
        }
        // Both components extreme is the true maximum and must not overflow.
        let out = compute_vec(&NaiveMagnitude, &[255, 255]);
        assert_eq!(out[0], u16::MAX, "full scale should be exactly u16::MAX");
    }

    #[test]
    fn magnitude_is_rotation_invariant() {
        // The same amplitude at different phases must give the same magnitude;
        // that is the property that lets us discard phase entirely.
        let a = compute_vec(&NaiveMagnitude, &[227, 127])[0]; // +100 on I
        let b = compute_vec(&NaiveMagnitude, &[127, 227])[0]; // +100 on Q
        let c = compute_vec(&NaiveMagnitude, &[27, 127])[0]; //  -100 on I
        assert!(a.abs_diff(b) < 400, "{a} vs {b}");
        assert!(a.abs_diff(c) < 400, "{a} vs {c}");
    }

    #[test]
    fn noise_floor_ignores_rare_bright_bursts() {
        let mut mag = vec![600u16; 10_000];
        // 1% of samples are a loud burst; a mean would move, a median must not.
        for slot in mag.iter_mut().take(100) {
            *slot = 50_000;
        }
        let floor = noise_floor(&mag);
        assert!(floor < 700, "median was dragged to {floor}");
    }

    #[test]
    fn noise_floor_is_never_zero() {
        // Downstream code divides by this.
        assert_eq!(noise_floor(&[0, 0, 0, 0]), 1);
        assert_eq!(noise_floor(&[]), 0);
    }
}
