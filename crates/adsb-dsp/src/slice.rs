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

/// Linear interpolation at fractional sample positions, with real confidence.
///
/// # Why this is the slicer that matters at 2.4 MS/s
///
/// A half-bit is **1.2 samples** here. [`NaiveSlicer`] takes the one sample
/// nearest each half-bit and rounds the position, so it can sit up to half a
/// sample off -- **42% of a half-bit** -- and it does so differently on every
/// one of 112 bits because the true positions are not on a sample grid.
///
/// That also explains why `slice.rs`'s first suggestion, integrating over each
/// half-bit, is worth little at this rate: there is barely more than one
/// sample per half-bit to integrate. The error is in *where* the sample is
/// taken, not in how many are averaged. So this interpolates instead.
///
/// # It consumes `Candidate::frac`
///
/// The detector may know the preamble began between samples. Nothing produced
/// that until now, but the field exists and honouring it costs one addition:
/// the whole frame shifts by the sub-sample amount rather than every bit
/// inheriting the detector's rounding error.
///
/// # And it reports confidence
///
/// `|early - late| / (early + late)`, as the module docs suggest. Nothing
/// consumes it yet -- the only validator is `crc-only` -- but reporting 255
/// for every bit, as the naive slicer does, is an assertion of certainty the
/// slicer has no basis for, and it is what stops stage 4 from ever being
/// written.
pub struct InterpSlicer {
    /// Fractional sample positions of each half-bit centre, relative to the
    /// start of the preamble.
    windows: Vec<(f64, f64)>,
}

impl InterpSlicer {
    pub fn new(sample_rate: u32) -> Self {
        let spus = samples_per_us(sample_rate);
        let windows = (0..LONG_BITS)
            .map(|bit| {
                // Bit k spans [8 + k, 9 + k) us, and the half-bits are
                // **start-anchored** -- sampled at their leading edge, not
                // their centre. This is the same convention the detector uses
                // for its slots, and for the same reason: a Mode S pulse
                // *begins* at its nominal time.
                //
                // Worth recording because it was got wrong here first.
                // Sampling half-bit centres instead is the obvious-looking
                // choice and it is much worse: 3262 messages -> 1308, and
                // positions 732 -> 269 on the golden fixture. detect.rs
                // records the identical mistake one stage up, where it "cost
                // an hour the first time".
                let t = PREAMBLE_US + bit as f64;
                ((t * spus), ((t + 0.5) * spus))
            })
            .collect();
        InterpSlicer { windows }
    }

    /// Magnitude at a fractional sample position, linearly interpolated.
    #[inline]
    fn at(mag: &[u16], pos: f64) -> Option<f32> {
        if pos < 0.0 {
            return None;
        }
        let i = pos.floor() as usize;
        if i + 1 >= mag.len() {
            return None;
        }
        let frac = (pos - i as f64) as f32;
        Some(f32::from(mag[i]) * (1.0 - frac) + f32::from(mag[i + 1]) * frac)
    }
}

impl BitSlicer for InterpSlicer {
    fn name(&self) -> &'static str {
        "interp"
    }

    fn describe(&self) -> &'static str {
        "linear interpolation at fractional half-bit centres; honours frac; real confidence"
    }

    fn slice(&self, mag: &[u16], cand: &Candidate, out: &mut RawFrame) -> bool {
        // The detector's sub-sample estimate shifts the entire frame, rather
        // than each bit inheriting a rounding error independently.
        let base = cand.offset as f64 + cand.frac as f64;

        let read = |bit: usize| -> Option<(f32, f32)> {
            let (e, l) = self.windows[bit];
            Some((Self::at(mag, base + e)?, Self::at(mag, base + l)?))
        };

        let decide = |early: f32, late: f32| -> (u8, u8) {
            let bit = u8::from(early > late);
            let sum = early + late;
            // Confidence is how decisively one half beat the other. Two halves
            // of equal energy is not a bit -- most often it is two aircraft
            // transmitting at once -- and saying so is much more useful than
            // guessing with full confidence.
            let conf = if sum > 0.0 {
                (((early - late).abs() / sum) * 255.0).clamp(0.0, 255.0) as u8
            } else {
                0
            };
            (bit, conf)
        };

        for bit in 0..5 {
            let Some((early, late)) = read(bit) else {
                return false;
            };
            let (b, c) = decide(early, late);
            out.bits[bit] = b;
            out.conf[bit] = c;
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
            let Some((early, late)) = read(bit) else {
                return false;
            };
            let (b, c) = decide(early, late);
            out.bits[bit] = b;
            out.conf[bit] = c;
        }

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

    /// A candidate at `offset` with no sub-sample hint.
    fn cand_at(offset: usize) -> Candidate {
        Candidate {
            offset,
            frac: 0.0,
            score: 1.0,
            noise: 100,
            signal: 1000,
        }
    }

    #[test]
    fn interp_half_bits_are_start_anchored_not_centred() {
        // The mistake this slicer was written wrong for the first time.
        // Centring costs 60% of the messages; see the constructor comment.
        let s = InterpSlicer::new(RATE);
        let spus = samples_per_us(RATE);
        let (early, late) = s.windows[0];
        assert!(
            (early - PREAMBLE_US * spus).abs() < 1e-9,
            "bit 0's early half must start at the preamble edge, got {early}"
        );
        assert!(
            (late - (PREAMBLE_US + 0.5) * spus).abs() < 1e-9,
            "bit 0's late half must start half a microsecond later, got {late}"
        );
    }

    #[test]
    fn interp_positions_are_fractional_and_do_not_drift() {
        // At 2.4 MS/s a bit is 2.4 samples. Rounding each position to an
        // integer walks 48 samples across 112 bits; keeping them fractional
        // is the whole point.
        let s = InterpSlicer::new(RATE);
        let spus = samples_per_us(RATE);
        let last = s.windows[LONG_BITS - 1].0;
        let want = (PREAMBLE_US + (LONG_BITS - 1) as f64) * spus;
        assert!(
            (last - want).abs() < 1e-9,
            "bit 111 drifted: {last} vs {want}"
        );
        assert!(
            last.fract() > 1e-6,
            "the last bit landed exactly on a sample; positions are not fractional"
        );
    }

    #[test]
    fn interp_reads_between_samples() {
        // Halfway between 1000 and 2000 must read 1500, not either endpoint.
        let mag = [1000u16, 2000, 1000, 2000];
        assert_eq!(InterpSlicer::at(&mag, 0.0), Some(1000.0));
        assert_eq!(InterpSlicer::at(&mag, 0.5), Some(1500.0));
        assert_eq!(InterpSlicer::at(&mag, 1.0), Some(2000.0));
        assert_eq!(
            InterpSlicer::at(&mag, 99.0),
            None,
            "past the end must not panic"
        );
    }

    #[test]
    fn interp_honours_the_candidates_sub_sample_offset() {
        // frac has been defined and plumbed through since stage 2 was written
        // and nothing has ever produced or consumed it. Shifting the frame by
        // a sub-sample amount must actually move where the bits are read.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let s = InterpSlicer::new(RATE);

        let mut a = RawFrame::default();
        let mut b = RawFrame::default();
        let mut c = cand_at(0);
        assert!(s.slice(&mag, &c, &mut a));
        c.frac = 0.4;
        assert!(s.slice(&mag, &c, &mut b));

        assert!(
            a.bits != b.bits || a.conf != b.conf,
            "frac had no effect; the sub-sample offset is being ignored"
        );
    }

    #[test]
    fn interp_reports_confidence_rather_than_asserting_certainty() {
        // The naive slicer returns 255 for every bit, which is a claim it has
        // no basis for and is what stops stage 4 from ever being written.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let s = InterpSlicer::new(RATE);
        let mut f = RawFrame::default();
        assert!(s.slice(&mag, &cand_at(0), &mut f));

        let conf = &f.conf[..f.len];
        assert!(
            conf.iter().any(|&c| c != 255),
            "every bit reported full confidence; that is the naive behaviour"
        );
        assert!(
            conf.iter().any(|&c| c > 0),
            "no bit had any confidence at all"
        );
    }

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
