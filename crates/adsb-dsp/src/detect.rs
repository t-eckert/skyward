//! Stage 2 — finding where messages start.
//!
//! Aircraft transmit whenever they like. There is no clock, no framing, and no
//! channel access protocol; two aircraft transmitting at once simply collide.
//! The only thing you can rely on is that every message opens with the same
//! 8 µs preamble:
//!
//! ```text
//!   pulse   at 0.0 µs   ██
//!   pulse   at 1.0 µs       ██
//!   silence     2.0-3.0
//!   pulse   at 3.5 µs                 ██
//!   pulse   at 4.5 µs                     ██
//!   silence     5.0-8.0
//! ```
//!
//! Those spacings are deliberate: the pattern has poor autocorrelation with
//! shifted copies of itself, so noise rarely fakes it and a partially
//! overlapping second message is unlikely to look like a valid start.
//!
//! # Why this stage matters most
//!
//! It sets the ceiling. A message the detector never proposes cannot be
//! recovered by any amount of cleverness downstream. On our golden fixture the
//! naive detector below finds 607 CRC-valid messages while a merely
//! *mean-integrating* detector finds 2,403 — so roughly three quarters of the
//! recoverable traffic is being discarded right here.
//!
//! # Improving on the baseline
//!
//! [`NaiveDetector`] has five specific weaknesses, each worth a step:
//!
//! 1. **It samples half-microsecond slots and compares extremes.** Correlate
//!    the full 8 µs template instead. Integrating N samples buys roughly √N in
//!    SNR, and the preamble is about 19 samples at 2.4 MS/s.
//! 2. **`min(pulses) > 2 × max(silences)` is a veto.** One noisy sample in any
//!    of twelve silence slots kills an otherwise perfect match. Compare each
//!    pulse against its *adjacent* spaces and accumulate a score, so a single
//!    outlier costs a little instead of everything. Measured on our fixture,
//!    this structure — not the threshold — is what rejects weak signals.
//! 3. **The threshold is absolute.** [`MIN_PULSE`] is a constant, so the
//!    detector's sensitivity silently changes the moment you touch the gain.
//!    Track a running noise floor and threshold on SNR instead. This is the
//!    single highest-value fix.
//! 4. **No context.** Real messages are preceded by quiet. Requiring that, and
//!    rejecting candidates that overlap a stronger one nearby, cuts false
//!    positives cheaply.
//! 5. **No sub-sample alignment.** Fit a parabola to the correlation peak and
//!    hand the fractional offset to the slicer via [`Candidate::frac`].
//!
//! And one engineering lesson that does not show up as sensitivity: a 19-tap
//! correlation evaluated at every sample offset is roughly 46 million
//! multiply-accumulates per second, which will not fit on a Pi alongside
//! everything else. Gate cheaply first — is this sample even above the floor?
//! — and correlate only at the few offsets that pass. Around 100× less work
//! for identical output. Detector design is a cost/sensitivity curve, not a
//! point on it.

use crate::{Candidate, PREAMBLE_US, samples_per_us};

/// Proposes places where a message might begin.
pub trait PreambleDetector: Send {
    /// Registry name, as used by `--detect`.
    fn name(&self) -> &'static str;

    /// One line for `--list-impls`.
    fn describe(&self) -> &'static str;

    /// Discard any accumulated state.
    ///
    /// Called between fixtures. Without it a detector carrying a noise
    /// estimate would score differently depending on what ran before it, and
    /// the benchmark would stop being reproducible.
    fn reset(&mut self);

    /// How many samples of history before `search_from` this detector reads.
    ///
    /// The pipeline guarantees they are present, except at the very start of a
    /// stream. Return 0 if you only look forward.
    fn lookback(&self) -> usize {
        0
    }

    /// Append every candidate found with a start offset in `[from, to)`.
    ///
    /// Guarantees from the pipeline: a full long message fits after `to`, and
    /// `lookback()` samples exist before `from` unless the stream just started.
    ///
    /// The contract you must honour: **output depends only on the samples, not
    /// on how they were chunked.** Emitting a candidate outside `[from, to)`
    /// duplicates or drops messages depending on buffer size, and the
    /// buffer-invariance test will catch it.
    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>);
}

/// Absolute magnitude a preamble pulse must exceed.
///
/// Roughly three times the noise floor measured on our fixtures (~580 in `u16`
/// units, indoors at 49.6 dB gain). Being a constant is the *bug*: change the
/// gain, or move the antenna, and this silently becomes either deaf or
/// swamped. Replacing it with a tracked noise floor is improvement 3 above.
pub const MIN_PULSE: u16 = 1_750;

/// How far a pulse must exceed the loudest nearby silence.
pub const PULSE_TO_SILENCE_RATIO: f32 = 2.0;

/// Half-microsecond slot boundaries within the preamble, as sample offsets.
#[derive(Clone, Copy, Debug)]
struct Slot {
    start: usize,
    end: usize,
}

/// The obvious detector: average each half-microsecond slot of the preamble,
/// then require every pulse slot to beat every silence slot by a fixed ratio.
///
/// This is the control. It is deliberately simple and deliberately explainable
/// — a fair opponent you can describe on a slide, not a strawman.
pub struct NaiveDetector {
    pulses: Vec<Slot>,
    silences: Vec<Slot>,
}

impl NaiveDetector {
    pub fn new(sample_rate: u32) -> Self {
        let spus = samples_per_us(sample_rate);
        // Slots are start-anchored: a Mode S pulse *begins* at its nominal
        // time and runs 0.5 µs. Centring the window on the nominal time
        // instead straddles the pulse edge and finds nothing -- a mistake
        // worth recording, because it cost an hour the first time.
        let slot = |start_us: f64| Slot {
            start: (start_us * spus).round() as usize,
            end: ((start_us + 0.5) * spus).round() as usize,
        };

        NaiveDetector {
            pulses: [0.0, 1.0, 3.5, 4.5].iter().map(|&t| slot(t)).collect(),
            silences: [0.5, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5]
                .iter()
                .map(|&t| slot(t))
                .collect(),
        }
    }

    #[inline]
    fn slot_mean(mag: &[u16], base: usize, slot: Slot) -> u32 {
        let a = base + slot.start;
        let b = (base + slot.end).min(mag.len());
        if a >= b {
            return 0;
        }
        let sum: u32 = mag[a..b].iter().map(|&m| u32::from(m)).sum();
        sum / (b - a) as u32
    }
}

impl PreambleDetector for NaiveDetector {
    fn name(&self) -> &'static str {
        "naive"
    }

    fn describe(&self) -> &'static str {
        "half-microsecond slot means; min(pulse) > 2x max(silence), absolute threshold"
    }

    fn reset(&mut self) {}

    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>) {
        for base in from..to {
            // Cheap gate: the first pulse must at least clear the threshold.
            // Skips almost the entire buffer without touching the full
            // sixteen-slot template.
            if u32::from(mag[base]) < u32::from(MIN_PULSE) {
                continue;
            }

            let pulse_min = self
                .pulses
                .iter()
                .map(|&s| Self::slot_mean(mag, base, s))
                .min()
                .unwrap_or(0);
            if pulse_min < u32::from(MIN_PULSE) {
                continue;
            }

            let silence_max = self
                .silences
                .iter()
                .map(|&s| Self::slot_mean(mag, base, s))
                .max()
                .unwrap_or(0);

            if (pulse_min as f32) <= silence_max as f32 * PULSE_TO_SILENCE_RATIO {
                continue;
            }

            out.push(Candidate {
                offset: base,
                frac: 0.0, // no sub-sample estimate; see improvement 5
                score: pulse_min as f32 / (silence_max.max(1) as f32),
                noise: silence_max.min(u32::from(u16::MAX)) as u16,
                signal: pulse_min.min(u32::from(u16::MAX)) as u16,
            });
        }
    }
}

/// Number of samples a detector may need to look ahead from a candidate start.
pub fn preamble_samples(sample_rate: u32) -> usize {
    (PREAMBLE_US * samples_per_us(sample_rate)).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::magnitude::{NaiveMagnitude, compute_vec};
    use crate::synth;

    const RATE: u32 = 2_400_000;

    fn detect_all(mag: &[u16], span: usize) -> Vec<Candidate> {
        let mut det = NaiveDetector::new(RATE);
        let mut out = Vec::new();
        let to = mag.len().saturating_sub(span);
        det.detect(mag, 0, to, &mut out);
        out
    }

    #[test]
    fn slots_are_start_anchored_not_centred() {
        let det = NaiveDetector::new(RATE);
        // First pulse occupies [0.0, 0.5) us = samples 0..1 at 2.4 MS/s.
        assert_eq!(det.pulses[0].start, 0);
        assert_eq!(det.pulses[0].end, 1);
        // Pulse at 3.5 us starts at sample 8.4 -> 8.
        assert_eq!(det.pulses[2].start, 8);
        // Twelve silent half-microsecond slots in an 8 us preamble.
        assert_eq!(det.silences.len(), 12);
    }

    #[test]
    fn finds_a_clean_synthetic_preamble() {
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let cands = detect_all(&mag, crate::long_message_samples(RATE));
        assert!(
            cands.len() >= synth::canonical_messages().len(),
            "expected at least one candidate per message, got {}",
            cands.len()
        );
    }

    #[test]
    fn pure_noise_produces_almost_nothing() {
        let iq = synth::noise_only(RATE, 20_000, 2.0, 0x1234);
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let cands = detect_all(&mag, crate::long_message_samples(RATE));
        assert!(
            cands.len() < 20,
            "{} false positives in 20k noise samples",
            cands.len()
        );
    }

    #[test]
    fn candidates_stay_inside_the_requested_range() {
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let mut det = NaiveDetector::new(RATE);
        let mut out = Vec::new();
        let (from, to) = (500, 2000);
        det.detect(&mag, from, to, &mut out);
        for c in &out {
            assert!(
                c.offset >= from && c.offset < to,
                "candidate at {} escaped [{from}, {to})",
                c.offset
            );
        }
    }
}
