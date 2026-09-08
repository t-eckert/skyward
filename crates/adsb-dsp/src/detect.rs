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
//! [`NaiveDetector`] has five specific weaknesses, each worth a step. Two are
//! now implemented and measured on hardware — see [`SnrDetector`] (3),
//! [`ScoredDetector`] (2) and [`PeakDetector`] (4). Together, on a Pi against
//! the golden fixture:
//! **517 messages to 1792, and 72 positions to 372, for 20% more CPU**
//! (10.9 to 13.1 ns/sample, 31.8x realtime against a guard that fails below
//! 1.5x). Of the two, the veto was worth far more than the threshold, exactly
//! as item 2 predicts. Item 4 then removed 57% of the candidates without
//! costing a single message, though not any measurable time: the cost is in
//! scanning, not in the candidates that scanning proposes.
//!
//! Item 1, the full-template correlator, is implemented as
//! [`CorrelatorDetector`] and **did not pay off** — see its documentation.
//! Roughly 1% more messages across four fixtures for four to seven times the
//! CPU, and a worse ghost ratio. It is kept as a registered implementation
//! because a measured negative result is worth more than a deleted branch,
//! and because it answers the cost question below.
//!
//! It also corrects the claim in the next paragraph. An ungated 19-tap
//! correlation *does* fit on a Pi: measured at 50.9 ns/sample against the
//! 416 ns/sample that realtime allows, 8.2x realtime against a guard that
//! fails below 1.5x. The gate is unnecessary. It is simply not worth
//! correlating at all, which is a different conclusion and a more useful one.
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
pub(crate) struct Slot {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// The obvious detector: average each half-microsecond slot of the preamble,
/// then require every pulse slot to beat every silence slot by a fixed ratio.
///
/// This is the control. It is deliberately simple and deliberately explainable
/// — a fair opponent you can describe on a slide, not a strawman.
pub struct NaiveDetector {
    pub(crate) pulses: Vec<Slot>,
    pub(crate) silences: Vec<Slot>,
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
    pub(crate) fn slot_mean(mag: &[u16], base: usize, slot: Slot) -> u32 {
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

/// How far a pulse must exceed the tracked noise floor, for [`SnrDetector`].
///
/// Chosen to match what [`MIN_PULSE`] already implies rather than to be
/// better: 1750 against the ~580 floor measured on the fixtures is about 3x,
/// so on that capture this detector should behave close to `naive`. The gain
/// is everywhere else -- a quieter front end, a different gain setting, a
/// different antenna -- where an absolute threshold silently changes
/// sensitivity and this does not.
pub const PULSE_SNR: f32 = 3.0;

/// Update the floor every Nth sample rather than every sample.
///
/// Not an approximation worth apologising for: the floor's time constants are
/// thousands of samples, so sampling it 16x less often is physically the same
/// estimate. It is, however, worth a great deal of speed.
///
/// Updating every sample cost 7.5 ns/sample against naive's 1.2 -- and the
/// reason is not the arithmetic, it is that `floor += ...` makes every
/// iteration depend on the previous one. That serial float dependency cannot
/// be pipelined or vectorised, where naive's integer compare against a
/// constant can be. Decimating breaks the chain and lets the hot path go back
/// to an integer compare against a cached threshold.
const FLOOR_DECIMATION: u64 = 16;

/// How fast the floor follows the noise, per *update*.
///
/// Symmetric, which is only safe because pulses are excluded from the
/// estimate entirely -- see [`NOISE_EXCLUDE`].
///
/// The first version of this was asymmetric instead: fall fast, rise slowly,
/// so that bursts of traffic could not drag the estimate up. That reasoning
/// was wrong, and measurably so. An estimator that falls fast and rises
/// slowly converges on the *lower envelope* of the signal, not its noise
/// level -- it tracks the quietest sample it has seen recently. The
/// threshold then sits at 3x the minimum rather than 3x the noise, which is
/// far too low: more samples pass the cheap gate (measured: 3.4x naive's
/// cost for no extra messages) and marginal junk reaches the CRC (measured:
/// ghost ratio 0.000 -> 0.125).
const FLOOR_RATE: f32 = FLOOR_DECIMATION as f32 / 4096.0;

/// A sample this far above the floor is signal, not noise, and is left out of
/// the estimate.
///
/// This is what keeps the tracker measuring the channel rather than the
/// traffic, and it is what makes a symmetric rate safe. A preamble pulse is
/// [`PULSE_SNR`] above the floor by definition, so excluding at the same
/// ratio excludes exactly the thing being detected.
const NOISE_EXCLUDE: f32 = PULSE_SNR;

/// How fast the floor may climb when *everything* looks like signal.
///
/// Without this an estimate seeded too low would exclude every sample as a
/// pulse and stay stuck there forever, deaf.
///
/// Sized against the longest thing that is legitimately loud. A 112-bit
/// extended squitter is 120 us, about 288 samples, so any real burst is a few
/// tens of updates; at this rate that moves the floor by well under 1%. A bad
/// seed still recovers in roughly 100 ms, which happens once at startup and
/// never matters again.
const FLOOR_UNSTICK: f32 = FLOOR_DECIMATION as f32 / 262_144.0;

/// Advance a noise-floor estimate by one update.
///
/// A free function rather than a method so the hot loop and the tests share
/// exactly one implementation. The loop keeps `floor` in a local for speed,
/// and a second copy of this arithmetic living in the loop body would be free
/// to drift away from the one the tests exercise.
#[inline]
fn advance_floor(floor: &mut f32, m: u16) {
    let x = f32::from(m);
    // Noise contributes at the normal rate; anything loud enough to be a
    // pulse contributes almost nothing, so the estimate stays a property of
    // the channel rather than of the traffic.
    let rate = if x < *floor * NOISE_EXCLUDE {
        FLOOR_RATE
    } else {
        FLOOR_UNSTICK
    };
    *floor += (x - *floor) * rate;
}

/// `naive`, with the absolute threshold replaced by a tracked noise floor.
///
/// # Why this is the highest-value change
///
/// [`MIN_PULSE`] is a constant calibrated against one capture. Measured on
/// this project's own hardware, the same antenna on two machines produced
/// `clip 0.000%` and `clip 0.322%` -- a systematic difference in front-end
/// level. An absolute threshold turns that into a difference in *sensitivity*,
/// and nothing reports it; the receiver simply appears to hear less. An
/// afternoon went into chasing that as an antenna fault.
///
/// Everything else here is identical to [`NaiveDetector`] on purpose, so a
/// benchmark against it isolates exactly one variable.
///
/// # Buffer invariance
///
/// The trait requires output to depend only on the samples, never on how they
/// were chunked. That rules out the obvious implementation -- take a
/// percentile of the current buffer -- because a different buffer size would
/// give a different floor and therefore different candidates.
///
/// Instead the floor is an exponential tracker advanced **once per sample, in
/// order**, over `[from, to)`. The pipeline calls `detect` over contiguous
/// ranges in sequence, so the state evolves through an identical series of
/// updates whatever the chunk boundaries are. `reset()` clears it between
/// fixtures, which the trait documents as a reproducibility hazard.
pub struct SnrDetector {
    pulses: Vec<Slot>,
    silences: Vec<Slot>,
    /// Tracks the floor in u16 units. `seeded` is false until the first
    /// sample, so the tracker starts from the signal rather than climbing out
    /// of an arbitrary constant.
    floor: f32,
    seeded: bool,
    /// Position in the sample stream, so decimation is a function of the
    /// stream and not of where the buffer boundaries happen to fall.
    n: u64,
    /// `floor * PULSE_SNR`, cached so the hot path is an integer compare.
    threshold: u32,
}

impl SnrDetector {
    pub fn new(sample_rate: u32) -> Self {
        let naive = NaiveDetector::new(sample_rate);
        SnrDetector {
            pulses: naive.pulses,
            silences: naive.silences,
            floor: 0.0,
            seeded: false,
            n: 0,
            threshold: u32::MAX,
        }
    }
}

impl PreambleDetector for SnrDetector {
    fn name(&self) -> &'static str {
        "snr"
    }

    fn describe(&self) -> &'static str {
        "naive slots, but thresholded on a tracked noise floor instead of an absolute level"
    }

    fn reset(&mut self) {
        self.floor = 0.0;
        self.seeded = false;
        self.n = 0;
        self.threshold = u32::MAX;
    }

    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>) {
        if from >= to {
            return;
        }
        // Seed once, outside the loop. Starting from the first sample rather
        // than a constant means the tracker never has to climb out of an
        // arbitrary starting point on a loud channel, or fall into one on a
        // quiet channel where it would be briefly deaf.
        if !self.seeded {
            self.floor = f32::from(mag[from]);
            self.threshold = (self.floor * PULSE_SNR) as u32;
            self.seeded = true;
        }

        // Hoisted into locals for the duration of the loop, and written back
        // at the end. Touching `self.n` and `self.threshold` through the
        // struct on every sample keeps them in memory and stops the compiler
        // unrolling the gate, which is most of what separates this from
        // naive's integer compare against a constant.
        let mut n = self.n;
        let mut floor = self.floor;
        let mut threshold = self.threshold;

        for base in from..to {
            // Advance on a stream-position schedule, then decide. Both are
            // functions of the samples alone, so chunking cannot change the
            // outcome.
            n += 1;
            if n.is_multiple_of(FLOOR_DECIMATION) {
                advance_floor(&mut floor, mag[base]);
                threshold = (floor * PULSE_SNR) as u32;
            }

            if u32::from(mag[base]) < threshold {
                continue;
            }
            let threshold_f = floor * PULSE_SNR;

            let pulse_min = self
                .pulses
                .iter()
                .map(|&s| NaiveDetector::slot_mean(mag, base, s))
                .min()
                .unwrap_or(0);
            if (pulse_min as f32) < threshold_f {
                continue;
            }

            let silence_max = self
                .silences
                .iter()
                .map(|&s| NaiveDetector::slot_mean(mag, base, s))
                .max()
                .unwrap_or(0);

            if (pulse_min as f32) <= silence_max as f32 * PULSE_TO_SILENCE_RATIO {
                continue;
            }

            out.push(Candidate {
                offset: base,
                frac: 0.0,
                // Score is SNR here, not a pulse/silence ratio. The trait
                // documents scores as comparable only within one
                // implementation, which is exactly why that is allowed.
                score: pulse_min as f32 / floor.max(1.0),
                // The tracked floor, not the loudest silence slot. This is
                // what the field actually means, and it makes the RSSI the
                // API reports a real measurement.
                noise: floor.min(f32::from(u16::MAX)) as u16,
                signal: pulse_min.min(u32::from(u16::MAX)) as u16,
            });
        }

        self.n = n;
        self.floor = floor;
        self.threshold = threshold;
    }
}

/// Total contrast, in noise floors, that the four pulses must accumulate.
///
/// Tuned by measurement, not derivation. The unit is "how many noise floors
/// does this pulse stand above its immediate neighbours", summed over four
/// pulses, so 6.0 is an average of 1.5 floors per pulse -- comfortably less
/// than any one pulse needs under `naive`, which is the point.
pub const SCORE_MIN: f32 = 6.0;

/// Suppression window for [`PeakDetector`], as a multiple of the preamble.
///
/// One preamble. Two genuine messages cannot start closer together than that
/// without overlapping, so a real transmission is never suppressed by another
/// real one.
pub const REFRACTORY_US: f64 = PREAMBLE_US;

/// How much stronger a candidate must be to survive inside the refractory
/// window of an earlier one.
///
/// 1.0 means "strictly stronger", which lets the entire rising edge of a
/// cluster through. Raising it suppresses more of that edge at the risk of
/// discarding the true peak when the climb is gradual.
///
/// Chosen by sweeping it against the golden fixture rather than by argument.
/// 1.25 is the last value that costs nothing:
///
/// ```text
///   margin   messages   positions   candidates
///     1.0      1792        372        136,955
///     1.25     1792        372        120,876   <- here
///     1.6      1789        370        109,817
///     2.0      1777        369        103,281
/// ```
///
/// Past 1.25 the climb toward a real peak starts being cut off before it
/// arrives, and messages go with it. The remaining candidates are cheap; the
/// messages are the whole point.
pub const PEAK_MARGIN: f32 = 1.25;

/// `snr`, with the all-or-nothing veto replaced by accumulated evidence.
///
/// # The veto is the problem, not the threshold
///
/// `naive` and `snr` both require `min(pulses) > 2 x max(silences)` across the
/// whole 8 us window. That is sixteen slots joined by AND: **one noisy sample
/// in any of twelve silence slots rejects an otherwise perfect preamble**, and
/// `detect.rs` records that this structure, rather than the threshold, is what
/// rejects weak signals.
///
/// Two changes, both about locality:
///
/// 1. **Each pulse is compared to its immediate neighbours only.** A burst of
///    noise at 7.5 us has no bearing on whether the pulse at 0.0 us is real.
///    Mode S puts a silence exactly half a microsecond either side of every
///    preamble pulse, so the adjacent slots are the honest local reference.
/// 2. **The comparisons are summed, not ANDed.** A single weak pulse costs
///    proportionally instead of absolutely.
///
/// # What this gives up
///
/// Distant silence slots no longer have a veto, so a noise burst shaped
/// vaguely like a preamble can survive further into the pipeline. That is a
/// deliberate trade: the CRC is the real arbiter, false candidates show up as
/// falling yield rather than as wrong aircraft, and `ghost_icao_ratio` guards
/// the outcome that would actually matter.
pub struct ScoredDetector {
    /// Each pulse slot with the silence slots immediately before and after it.
    pulses: Vec<(Slot, Vec<Slot>)>,
    floor: f32,
    seeded: bool,
    n: u64,
    threshold: u32,
}

impl ScoredDetector {
    pub fn new(sample_rate: u32) -> Self {
        let spus = samples_per_us(sample_rate);
        let slot = |start_us: f64| Slot {
            start: (start_us * spus).round() as usize,
            end: ((start_us + 0.5) * spus).round() as usize,
        };

        // Mode S places a silent half-microsecond either side of each preamble
        // pulse, so adjacency is just t +/- 0.5 us, bounded by the preamble.
        let silences = [0.5, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5];
        let pulses = [0.0f64, 1.0, 3.5, 4.5]
            .iter()
            .map(|&t| {
                let neighbours = silences
                    .iter()
                    .filter(|&&sil| {
                        (sil - (t - 0.5)).abs() < 1e-9 || (sil - (t + 0.5)).abs() < 1e-9
                    })
                    .map(|&sil| slot(sil))
                    .collect();
                (slot(t), neighbours)
            })
            .collect();

        ScoredDetector {
            pulses,
            floor: 0.0,
            seeded: false,
            n: 0,
            threshold: u32::MAX,
        }
    }
}

impl PreambleDetector for ScoredDetector {
    fn name(&self) -> &'static str {
        "scored"
    }

    fn describe(&self) -> &'static str {
        "tracked noise floor; per-pulse contrast against adjacent silences, accumulated not vetoed"
    }

    fn reset(&mut self) {
        self.floor = 0.0;
        self.seeded = false;
        self.n = 0;
        self.threshold = u32::MAX;
    }

    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>) {
        if from >= to {
            return;
        }
        if !self.seeded {
            self.floor = f32::from(mag[from]);
            self.threshold = (self.floor * PULSE_SNR) as u32;
            self.seeded = true;
        }

        let mut n = self.n;
        let mut floor = self.floor;
        let mut threshold = self.threshold;

        for base in from..to {
            n += 1;
            if n.is_multiple_of(FLOOR_DECIMATION) {
                advance_floor(&mut floor, mag[base]);
                threshold = (floor * PULSE_SNR) as u32;
            }

            // Same cheap gate as `snr`: the first pulse has to clear the
            // floor before the full template is worth evaluating.
            if u32::from(mag[base]) < threshold {
                continue;
            }

            let mut score = 0.0f32;
            let mut weakest = u32::MAX;
            for (pulse, neighbours) in &self.pulses {
                let p = NaiveDetector::slot_mean(mag, base, *pulse);
                weakest = weakest.min(p);
                // The loudest immediate neighbour, so a pulse has to stand
                // above the noise *right next to it* rather than above the
                // quietest place in the window.
                let local = neighbours
                    .iter()
                    .map(|&s| NaiveDetector::slot_mean(mag, base, s))
                    .max()
                    .unwrap_or(0);
                score += (p as f32 - local as f32) / floor.max(1.0);
            }

            if score < SCORE_MIN {
                continue;
            }

            out.push(Candidate {
                offset: base,
                frac: 0.0,
                score,
                noise: floor.min(f32::from(u16::MAX)) as u16,
                signal: weakest.min(u32::from(u16::MAX)) as u16,
            });
        }

        self.n = n;
        self.floor = floor;
        self.threshold = threshold;
    }
}

/// `scored`, plus local non-maximum suppression.
///
/// # Why
///
/// `scored` proposes 156 candidates per recovered message on the golden
/// fixture. Most are not separate discoveries: a real preamble clears the
/// score threshold at many neighbouring offsets, and each one costs a full
/// 112-bit slice and a CRC downstream. `detect.rs` lists this as improvement
/// 4 -- "rejecting candidates that overlap a stronger one nearby, cuts false
/// positives cheaply".
///
/// # Causal, because the contract requires it
///
/// True non-maximum suppression needs to see the whole cluster, and a
/// candidate near the end of a buffer would have to wait for the next one --
/// which would emit it outside `[from, to)` and make the output depend on
/// chunking, the one thing the trait forbids.
///
/// So this suppresses *backwards* only: a candidate is dropped if a
/// **stronger or equal** one occurred within the last [`REFRACTORY_US`]. The
/// rising edge of a cluster still emits a few, and the peak always survives
/// because it is stronger than everything before it. That is weaker than
/// full NMS and it is buffer-invariant, which matters more.
///
/// The window is one preamble long. Two genuine messages cannot begin inside
/// 8 us of each other without overlapping, so nothing real is lost.
pub struct PeakDetector {
    inner: Box<dyn PreambleDetector>,
    label: &'static str,
    scratch: Vec<Candidate>,
    /// Stream position of the first sample of the next call, so suppression
    /// state is expressed in stream coordinates rather than buffer ones.
    stream_pos: u64,
    /// Stream position and score of the last candidate emitted.
    last: Option<(u64, f32)>,
    refractory: u64,
}

impl PeakDetector {
    /// Suppression over `scored`, the historical meaning of `--detect peak`.
    pub fn new(sample_rate: u32) -> Self {
        Self::wrapping(
            Box::new(ScoredDetector::new(sample_rate)),
            sample_rate,
            "peak",
        )
    }

    /// Suppression over any detector, so a new front end can be measured with
    /// and without it rather than having suppression baked in.
    pub fn wrapping(
        inner: Box<dyn PreambleDetector>,
        sample_rate: u32,
        label: &'static str,
    ) -> Self {
        PeakDetector {
            inner,
            label,
            scratch: Vec::new(),
            stream_pos: 0,
            last: None,
            refractory: preamble_samples(sample_rate) as u64,
        }
    }
}

impl PreambleDetector for PeakDetector {
    fn name(&self) -> &'static str {
        self.label
    }

    fn describe(&self) -> &'static str {
        "candidates suppressed when a stronger one occurred within a preamble"
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.scratch.clear();
        self.stream_pos = 0;
        self.last = None;
    }

    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>) {
        if from >= to {
            return;
        }
        self.scratch.clear();
        self.inner.detect(mag, from, to, &mut self.scratch);

        for c in self.scratch.drain(..) {
            // Buffer offset -> stream position. `scored` emits in increasing
            // offset order, so this is monotonic.
            let pos = self.stream_pos + (c.offset - from) as u64;
            let suppressed = match self.last {
                Some((last_pos, last_score)) => {
                    pos.saturating_sub(last_pos) < self.refractory
                        && c.score <= last_score * PEAK_MARGIN
                }
                None => false,
            };
            if suppressed {
                continue;
            }
            self.last = Some((pos, c.score));
            out.push(c);
        }

        self.stream_pos += (to - from) as u64;
    }
}

/// Correlation score a candidate must reach, in noise floors.
///
/// The template is normalised so the score reads directly as
/// `(mean pulse - mean silence) / noise floor`, which makes it comparable to
/// the per-pulse contrast [`ScoredDetector`] accumulates and to
/// [`PULSE_SNR`]. Swept against the fixtures rather than derived.
pub const CORR_MIN: f32 = 1.6;

/// Matched filter over the whole 8 us preamble.
///
/// # What it buys
///
/// `naive`, `snr` and `scored` all reduce each half-microsecond slot to its
/// mean and then compare a handful of numbers. Correlating the full template
/// integrates every sample of the preamble, which buys roughly sqrt(N) in
/// SNR -- about 4.4x, or 6 dB, over the ~19 samples an 8 us preamble occupies
/// at 2.4 MS/s.
///
/// # The template is fractional on purpose
///
/// At 2.4 MS/s a half-microsecond slot is **1.2 samples**, not 1 and not 2.
/// Rounding slot edges to sample boundaries throws away up to 40% of a slot
/// and smears pulse energy into silence weights. So each tap is built from
/// how much of that sample's time window actually falls inside a pulse:
/// `weight = pulse_fraction` positive, `silence_fraction` negative.
///
/// # Zero-mean, and normalised to read as SNR
///
/// The pulse contributions are normalised to sum to +1 and the silence
/// contributions to -1 *before* they are combined, so the finished template
/// sums to zero. Note that a tap straddling a slot edge carries both, and
/// nets out — so the positive taps of the final template do **not** sum to 1,
/// only the whole thing to 0.
///
/// Zero-mean is what makes the correlation blind to the DC level: a change of
/// gain, or a drifting noise floor, moves every sample together and cancels.
/// The normalisation is what makes the score read as roughly
/// `(mean pulse - mean silence) / floor`, comparable to the other detectors
/// rather than an arbitrary number.
///
/// # Ungated, deliberately
///
/// `detect.rs` warns that correlating at every offset is "roughly 46 million
/// multiply-accumulates per second, which will not fit on a Pi alongside
/// everything else", and recommends gating cheaply first. That was written
/// before anyone measured the budget. Realtime at 2.4 MS/s allows 416
/// ns/sample; `naive` uses 10.8 and the regression guard only fails below
/// 1.5x realtime. A gate is complexity that also costs sensitivity -- the
/// cheap test that decides what is worth correlating is exactly the weak
/// rejector this whole exercise is trying to remove -- so the ungated
/// version is written first and measured. If it fits, the gate should never
/// be written.
pub struct CorrelatorDetector {
    /// Zero-mean, positive taps summing to +1 and negative to -1.
    template: Vec<f32>,
    floor: f32,
    seeded: bool,
    n: u64,
}

impl CorrelatorDetector {
    pub fn new(sample_rate: u32) -> Self {
        let spus = samples_per_us(sample_rate);
        let taps = preamble_samples(sample_rate);

        // How much of sample i overlaps [a, b) microseconds.
        let overlap = |i: usize, a: f64, b: f64| -> f64 {
            let lo = i as f64 / spus;
            let hi = (i + 1) as f64 / spus;
            (hi.min(b) - lo.max(a)).max(0.0) * spus
        };

        let pulses = [0.0f64, 1.0, 3.5, 4.5];
        let mut pos = vec![0.0f64; taps];
        let mut neg = vec![0.0f64; taps];
        for (i, (p, n)) in pos.iter_mut().zip(neg.iter_mut()).enumerate() {
            let in_pulse: f64 = pulses.iter().map(|&t| overlap(i, t, t + 0.5)).sum();
            // Everything inside the preamble that is not a pulse is silence.
            let covered = overlap(i, 0.0, PREAMBLE_US);
            *p = in_pulse;
            *n = (covered - in_pulse).max(0.0);
        }

        // Normalise each polarity separately: positive taps to +1, negative to
        // -1. Zero-mean falls out, and the score reads as a contrast ratio.
        let pos_sum: f64 = pos.iter().sum();
        let neg_sum: f64 = neg.iter().sum();
        let template = pos
            .iter()
            .zip(neg.iter())
            .map(|(&p, &n)| (p / pos_sum.max(1e-9) - n / neg_sum.max(1e-9)) as f32)
            .collect();

        CorrelatorDetector {
            template,
            floor: 0.0,
            seeded: false,
            n: 0,
        }
    }
}

impl PreambleDetector for CorrelatorDetector {
    fn name(&self) -> &'static str {
        "corr"
    }

    fn describe(&self) -> &'static str {
        "matched filter over the whole 8us preamble, fractional taps, ungated"
    }

    fn reset(&mut self) {
        self.floor = 0.0;
        self.seeded = false;
        self.n = 0;
    }

    fn detect(&mut self, mag: &[u16], from: usize, to: usize, out: &mut Vec<Candidate>) {
        if from >= to {
            return;
        }
        if !self.seeded {
            self.floor = f32::from(mag[from]);
            self.seeded = true;
        }

        let mut n = self.n;
        let mut floor = self.floor;
        let taps = self.template.len();

        for base in from..to {
            n += 1;
            if n.is_multiple_of(FLOOR_DECIMATION) {
                advance_floor(&mut floor, mag[base]);
            }

            if base + taps > mag.len() {
                break;
            }

            let mut acc = 0.0f32;
            for (k, &w) in self.template.iter().enumerate() {
                acc += w * f32::from(mag[base + k]);
            }

            let score = acc / floor.max(1.0);
            if score < CORR_MIN {
                continue;
            }

            out.push(Candidate {
                offset: base,
                frac: 0.0,
                score,
                noise: floor.min(f32::from(u16::MAX)) as u16,
                signal: (acc.max(0.0)).min(f32::from(u16::MAX)) as u16,
            });
        }

        self.n = n;
        self.floor = floor;
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
    fn snr_finds_the_same_clean_preambles() {
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let mut det = SnrDetector::new(RATE);
        let mut out = Vec::new();
        det.detect(
            &mag,
            0,
            mag.len() - crate::long_message_samples(RATE),
            &mut out,
        );
        assert!(
            out.len() >= synth::canonical_messages().len(),
            "expected at least one candidate per message, got {}",
            out.len()
        );
    }

    #[test]
    fn snr_does_not_invent_preambles_in_noise() {
        // The same bar `naive` has to clear. A detector that trades false
        // positives for sensitivity is not an improvement -- it just moves the
        // work to the CRC and shows up as collapsing yield.
        let iq = synth::noise_only(RATE, 20_000, 2.0, 0x1234);
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let mut det = SnrDetector::new(RATE);
        let mut out = Vec::new();
        det.detect(
            &mag,
            0,
            mag.len() - crate::long_message_samples(RATE),
            &mut out,
        );
        assert!(
            out.len() < 20,
            "{} false positives in 20k noise samples",
            out.len()
        );
    }

    #[test]
    fn snr_output_does_not_depend_on_chunking() {
        // The contract that rules out the obvious implementation of a noise
        // floor (a percentile of the current buffer). The tracker is advanced
        // once per sample in order, so chunk boundaries must not matter.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut whole = Vec::new();
        SnrDetector::new(RATE).detect(&mag, 0, end, &mut whole);

        for chunk in [977usize, 4096, 65_536] {
            let mut det = SnrDetector::new(RATE);
            let mut pieces = Vec::new();
            let mut from = 0;
            while from < end {
                let to = (from + chunk).min(end);
                det.detect(&mag, from, to, &mut pieces);
                from = to;
            }
            assert_eq!(
                whole, pieces,
                "chunking at {chunk} changed the output; the floor is not buffer-invariant"
            );
        }
    }

    #[test]
    fn snr_floor_ignores_pulses_and_tracks_noise() {
        // The property that matters: a burst of loud samples must not drag
        // the estimate up, or the detector goes deaf immediately after
        // traffic -- which is exactly when the next message arrives.
        let run = |seed: f32, sample: u16, updates: usize| {
            let mut floor = seed;
            for _ in 0..updates {
                advance_floor(&mut floor, sample);
            }
            floor
        };

        // A realistic burst, not a pathological one. The longest legitimate
        // transmission is a 112-bit squitter at 120 us -- about 288 samples,
        // so 50 decimated updates is already several messages back to back.
        let loud = run(600.0, 30_000, 50);
        assert!(
            loud < 700.0,
            "a burst dragged the floor to {loud} -- it is tracking traffic, not noise"
        );

        // But it must still follow the noise itself, in both directions.
        let quieter = run(600.0, 300, 5_000);
        assert!(quieter < 400.0, "floor did not fall to meet quieter noise");

        let louder = run(300.0, 600, 5_000);
        assert!(louder > 500.0, "floor did not rise to meet louder noise");
    }

    #[test]
    fn scored_finds_more_than_it_vetoes() {
        // The whole claim: dropping the all-or-nothing veto should propose
        // strictly more places to look, never fewer.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut naive_out = Vec::new();
        NaiveDetector::new(RATE).detect(&mag, 0, end, &mut naive_out);
        let mut scored_out = Vec::new();
        ScoredDetector::new(RATE).detect(&mag, 0, end, &mut scored_out);

        assert!(
            scored_out.len() >= naive_out.len(),
            "scored proposed {} against naive's {}",
            scored_out.len(),
            naive_out.len()
        );
    }

    #[test]
    fn scored_does_not_invent_preambles_in_noise() {
        // The bar every detector has to clear. Accumulating evidence instead
        // of vetoing is only a fair trade if noise still fails -- otherwise
        // the extra messages are just the CRC working harder.
        let iq = synth::noise_only(RATE, 20_000, 2.0, 0x1234);
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let mut out = Vec::new();
        ScoredDetector::new(RATE).detect(
            &mag,
            0,
            mag.len() - crate::long_message_samples(RATE),
            &mut out,
        );
        assert!(
            out.len() < 20,
            "{} false positives in 20k noise samples",
            out.len()
        );
    }

    #[test]
    fn scored_output_does_not_depend_on_chunking() {
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut whole = Vec::new();
        ScoredDetector::new(RATE).detect(&mag, 0, end, &mut whole);

        for chunk in [977usize, 4096, 65_536] {
            let mut det = ScoredDetector::new(RATE);
            let mut pieces = Vec::new();
            let mut from = 0;
            while from < end {
                let to = (from + chunk).min(end);
                det.detect(&mag, from, to, &mut pieces);
                from = to;
            }
            assert_eq!(whole, pieces, "chunking at {chunk} changed the output");
        }
    }

    #[test]
    fn scored_pairs_each_pulse_with_its_immediate_neighbours() {
        // Locality is the mechanism. The pulse at 0.0 us opens the preamble so
        // it has only the silence after it; the others are flanked on both
        // sides. Getting this wrong silently turns the detector back into a
        // global comparison.
        let det = ScoredDetector::new(RATE);
        assert_eq!(det.pulses.len(), 4);
        assert_eq!(
            det.pulses[0].1.len(),
            1,
            "the opening pulse has no silence before it"
        );
        for (i, (_, neighbours)) in det.pulses.iter().enumerate().skip(1) {
            assert_eq!(
                neighbours.len(),
                2,
                "pulse {i} should be flanked on both sides"
            );
        }
    }

    #[test]
    fn peak_emits_a_subset_of_scored() {
        // Suppression may only ever remove candidates. If `peak` proposes
        // something `scored` did not, the filter has a bug rather than an
        // opinion.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut scored_out = Vec::new();
        ScoredDetector::new(RATE).detect(&mag, 0, end, &mut scored_out);
        let mut peak_out = Vec::new();
        PeakDetector::new(RATE).detect(&mag, 0, end, &mut peak_out);

        assert!(peak_out.len() <= scored_out.len());
        for c in &peak_out {
            assert!(
                scored_out.contains(c),
                "peak proposed a candidate at {} that scored did not",
                c.offset
            );
        }
    }

    #[test]
    fn peak_output_does_not_depend_on_chunking() {
        // The reason suppression is backwards-only. Looking ahead would mean
        // deferring a candidate past the end of its buffer, and the output
        // would start depending on where the boundaries fell.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut whole = Vec::new();
        PeakDetector::new(RATE).detect(&mag, 0, end, &mut whole);

        for chunk in [977usize, 4096, 65_536] {
            let mut det = PeakDetector::new(RATE);
            let mut pieces = Vec::new();
            let mut from = 0;
            while from < end {
                let to = (from + chunk).min(end);
                det.detect(&mag, from, to, &mut pieces);
                from = to;
            }
            assert_eq!(whole, pieces, "chunking at {chunk} changed the output");
        }
    }

    #[test]
    fn peak_keeps_the_strongest_of_a_cluster() {
        // The peak of a cluster is stronger than everything before it, so a
        // backwards-only rule still keeps it. That is the property that lets
        // this discard half the candidates without losing a message.
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut scored_out = Vec::new();
        ScoredDetector::new(RATE).detect(&mag, 0, end, &mut scored_out);
        let mut peak_out = Vec::new();
        PeakDetector::new(RATE).detect(&mag, 0, end, &mut peak_out);

        let best_scored = scored_out.iter().map(|c| c.score).fold(0.0f32, f32::max);
        let best_peak = peak_out.iter().map(|c| c.score).fold(0.0f32, f32::max);
        assert_eq!(
            best_scored, best_peak,
            "suppression discarded the strongest candidate"
        );
    }

    #[test]
    fn correlator_template_is_zero_mean_and_normalised() {
        // The property the whole design rests on. Zero-mean makes the score
        // blind to DC, so a gain change or a drifting floor does not move it,
        // and the normalisation makes the score read as roughly
        // (mean pulse - mean silence) / floor -- comparable to every other
        // detector instead of being an arbitrary number.
        let det = CorrelatorDetector::new(RATE);
        let sum: f32 = det.template.iter().sum();
        assert!(sum.abs() < 1e-4, "template is not zero-mean: {sum}");

        // Zero-mean is only worth having if it delivers DC blindness, so test
        // the consequence rather than the arithmetic: flat input at any level
        // must correlate to nothing. That is what makes the score independent
        // of gain and of a drifting noise floor.
        //
        // The positive taps deliberately do NOT sum to 1. A tap straddling a
        // slot edge carries both a pulse and a silence contribution and nets
        // out, so only the whole template sums to zero.
        for level in [200u16, 2_000, 20_000] {
            let acc: f32 = det.template.iter().map(|&w| w * f32::from(level)).sum();
            assert!(
                acc.abs() < f32::from(level) * 1e-3,
                "flat input at {level} correlated to {acc}; the template is not DC-blind"
            );
        }

        // The structure must survive normalisation: opening pulse positive,
        // middle of the long trailing silence negative.
        assert!(
            det.template[0] > 0.0,
            "the pulse at 0.0 us should weigh positive"
        );
        let mid = det.template.len() * 3 / 4;
        assert!(
            det.template[mid] < 0.0,
            "the trailing silence should weigh negative"
        );
    }

    #[test]
    fn correlator_taps_are_fractional() {
        // At 2.4 MS/s a half-microsecond slot is 1.2 samples, so slot edges
        // land inside samples. Taps built by rounding would be a handful of
        // identical values; taps built from actual overlap are not.
        let det = CorrelatorDetector::new(RATE);
        let distinct = det
            .template
            .iter()
            .map(|w| (w * 1000.0).round() as i32)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            distinct.len() > 3,
            "only {} distinct tap values -- edges were rounded to samples",
            distinct.len()
        );
    }

    #[test]
    fn correlator_does_not_invent_preambles_in_noise() {
        let iq = synth::noise_only(RATE, 20_000, 2.0, 0x1234);
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let mut out = Vec::new();
        CorrelatorDetector::new(RATE).detect(
            &mag,
            0,
            mag.len() - crate::long_message_samples(RATE),
            &mut out,
        );
        assert!(
            out.len() < 20,
            "{} false positives in 20k noise samples",
            out.len()
        );
    }

    #[test]
    fn correlator_output_does_not_depend_on_chunking() {
        let iq = synth::synthesize(
            &synth::canonical_messages(),
            &synth::SynthConfig {
                sample_rate: RATE,
                ..Default::default()
            },
        );
        let mag = compute_vec(&NaiveMagnitude, &iq);
        let end = mag.len() - crate::long_message_samples(RATE);

        let mut whole = Vec::new();
        CorrelatorDetector::new(RATE).detect(&mag, 0, end, &mut whole);

        for chunk in [977usize, 4096, 65_536] {
            let mut det = CorrelatorDetector::new(RATE);
            let mut pieces = Vec::new();
            let mut from = 0;
            while from < end {
                let to = (from + chunk).min(end);
                det.detect(&mag, from, to, &mut pieces);
                from = to;
            }
            assert_eq!(whole, pieces, "chunking at {chunk} changed the output");
        }
    }

    #[test]
    fn snr_reset_clears_the_floor() {
        let mut det = SnrDetector::new(RATE);
        det.floor = 5000.0;
        det.seeded = true;
        det.n = 99;
        det.reset();
        assert!(
            !det.seeded,
            "a floor surviving reset makes bench irreproducible"
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
