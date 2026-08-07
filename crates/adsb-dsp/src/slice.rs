//! Stage 3 — turning magnitude into bits.
//!
//! Mode S data is pulse-position modulated at 1 Mbit/s. Each bit occupies one
//! microsecond and always contains exactly one pulse:
//!
//! ```text
//!   bit = 1     ██__        energy in the first half
//!   bit = 0     __██        energy in the second half
//! ```
//!
//! Because every bit has a transition, the clock recovers itself — there is no
//! run of identical symbols to drift through. That is why this decoder needs
//! no PLL.
//!
//! # The awkward number
//!
//! At 2.4 MS/s a bit is **2.4 samples** wide. Not two, not three. Over 112 bits
//! the sampling point walks a full 48 samples if you approximate it with an
//! integer, which is why every position here is computed from an `f64`
//! microsecond time rather than accumulated.
//!
//! 2.0 MS/s gives exactly 2 samples per bit and a trivially correct slicer, at
//! the cost of coarser timing. Both are worth measuring — the answer is not
//! obvious, and dump1090 ships hand-derived paths for each.
//!
//! # Improving on the baseline
//!
//! [`NaiveSlicer`] takes one sample from each half-bit and compares them.
//! Better:
//!
//! - **Integrate over each half-bit** instead of point-sampling. More samples,
//!   less noise, same cost class.
//! - **Interpolate at fractional positions** rather than rounding to the
//!   nearest sample, and consume [`Candidate::frac`] from stage 2 so the whole
//!   frame is aligned to where the edge actually was.
//! - **Emit real confidence.** `|early − late| / (early + late)` is a decent
//!   first cut. This is what makes safe error correction possible in stage 4,
//!   and it costs almost nothing here.
//! - **Notice both halves being high.** That is two aircraft transmitting at
//!   once, not a bit. Marking those bits low-confidence is much better than
//!   guessing, and lets stage 4 recover the frame.
//! - **Retry at ±1 sample.** Cheap, and rescues frames the detector placed
//!   slightly off.

use crate::{Candidate, LONG_BITS, PREAMBLE_US, RawFrame, SHORT_BITS, samples_per_us};

/// Turns a candidate position into bits.
///
/// Takes `&self`: a slicer is a pure function of the magnitude window, so it
/// can be run over candidates in parallel and is trivially testable.
pub trait BitSlicer: Send + Sync {
    /// Registry name, as used by `--slice`.
    fn name(&self) -> &'static str;

    /// One line for `--list-impls`.
    fn describe(&self) -> &'static str;

    /// Fill `out` from the message starting at `cand.offset`.
    ///
    /// Returns false if the frame could not be sliced at all — usually because
    /// the buffer ends first. `out.offset` is set by the pipeline, not here.
    fn slice(&self, mag: &[u16], cand: &Candidate, out: &mut RawFrame) -> bool;
}

/// Precomputed sample positions for one bit's two halves.
#[derive(Clone, Copy)]
struct BitWindow {
    early: usize,
    late: usize,
}

/// One sample from each half-bit, compared directly.
///
/// Reports full confidence on every bit, because it has no basis for saying
/// anything else — which means stage 4 has nothing to work with until you
/// replace this. That coupling is deliberate.
pub struct NaiveSlicer {
    /// Sample offsets relative to the start of the preamble.
    windows: Vec<BitWindow>,
}

impl NaiveSlicer {
    pub fn new(sample_rate: u32) -> Self {
        let spus = samples_per_us(sample_rate);
        let windows = (0..LONG_BITS)
            .map(|bit| {
                // Bit k spans [8 + k, 9 + k) microseconds past the preamble start.
                let t = PREAMBLE_US + bit as f64;
                BitWindow {
                    early: (t * spus).round() as usize,
                    late: ((t + 0.5) * spus).round() as usize,
                }
            })
            .collect();
        NaiveSlicer { windows }
    }
}

impl BitSlicer for NaiveSlicer {
    fn name(&self) -> &'static str {
        "naive"
    }

    fn describe(&self) -> &'static str {
        "one sample per half-bit, compared directly; reports no confidence"
    }

    fn slice(&self, mag: &[u16], cand: &Candidate, out: &mut RawFrame) -> bool {
        let base = cand.offset;

        // Slice the first five bits to learn the downlink format, so a short
        // frame does not need 112 microseconds of buffer to exist.
        let read = |bit: usize| -> Option<(u16, u16)> {
            let w = self.windows[bit];
            let (a, b) = (base + w.early, base + w.late);
            (b < mag.len()).then(|| (mag[a], mag[b]))
        };

        for bit in 0..5 {
            let (early, late) = match read(bit) {
                Some(v) => v,
                None => return false,
            };
            out.bits[bit] = u8::from(early > late);
            out.conf[bit] = 255;
        }

        let df = {
            let mut v = 0u8;
            for i in 0..5 {
                v = (v << 1) | out.bits[i];
            }
            v
        };
        let len = if df >= 16 { LONG_BITS } else { SHORT_BITS };

        for bit in 5..len {
            let (early, late) = match read(bit) {
                Some(v) => v,
                None => return false,
            };
            out.bits[bit] = u8::from(early > late);
            // No confidence information. A better slicer replaces this, and
            // stage 4 gets dramatically more effective when it does.
            out.conf[bit] = 255;
        }

        // Leave the tail of a short frame zeroed rather than stale, so a
        // reused RawFrame cannot leak bits from a previous message.
        for bit in len..LONG_BITS {
            out.bits[bit] = 0;
            out.conf[bit] = 0;
        }

        out.len = len;
        out.signal = cand.signal;
        out.noise = cand.noise;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::magnitude::{NaiveMagnitude, compute_vec};
    use crate::synth;

    const RATE: u32 = 2_400_000;

    #[test]
    fn bit_positions_do_not_drift_across_the_frame() {
        let s = NaiveSlicer::new(RATE);
        // Bit 0 starts at 8 us = sample 19.2 -> 19.
        assert_eq!(s.windows[0].early, 19);
        // Bit 111 starts at 119 us = sample 285.6 -> 286. An integer
        // approximation of 2 samples/bit would have said 8*2 + 111*2 = 238.
        assert_eq!(s.windows[111].early, 286);
        // Each half-bit is 1.2 samples apart at this rate.
        assert_eq!(s.windows[111].late, 287);
    }

    #[test]
    fn recovers_a_known_message_exactly() {
        let msg = adsb_core::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let iq = synth::synthesize(
            std::slice::from_ref(&msg),
            &synth::SynthConfig {
                sample_rate: RATE,
                noise_amplitude: 0.0,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);

        // The generator places the first message at a known offset.
        let cand = Candidate {
            offset: synth::first_message_offset(RATE),
            frac: 0.0,
            score: 10.0,
            noise: 100,
            signal: 40_000,
        };
        let mut frame = RawFrame::default();
        assert!(NaiveSlicer::new(RATE).slice(&mag, &cand, &mut frame));

        assert_eq!(frame.len, 112, "DF17 must slice as a long frame");
        assert_eq!(adsb_core::bits::pack(&frame.bits[..112]), msg);
    }

    #[test]
    fn short_frames_are_sliced_as_56_bits() {
        let msg = adsb_core::hex_to_bytes("5DC04413BFAD35").unwrap();
        let iq = synth::synthesize(
            std::slice::from_ref(&msg),
            &synth::SynthConfig {
                sample_rate: RATE,
                noise_amplitude: 0.0,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let cand = Candidate {
            offset: synth::first_message_offset(RATE),
            frac: 0.0,
            score: 10.0,
            noise: 100,
            signal: 40_000,
        };
        let mut frame = RawFrame::default();
        assert!(NaiveSlicer::new(RATE).slice(&mag, &cand, &mut frame));

        assert_eq!(frame.df(), 11);
        assert_eq!(frame.len, 56);
        assert_eq!(adsb_core::bits::pack(&frame.bits[..56]), msg);
    }

    #[test]
    fn refuses_to_slice_past_the_end_of_the_buffer() {
        let mag = vec![1000u16; 50];
        let cand = Candidate {
            offset: 0,
            frac: 0.0,
            score: 1.0,
            noise: 100,
            signal: 1000,
        };
        let mut frame = RawFrame::default();
        assert!(!NaiveSlicer::new(RATE).slice(&mag, &cand, &mut frame));
    }
}
