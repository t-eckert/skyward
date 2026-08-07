//! Wiring the four stages together.
//!
//! [`Pipeline`] is deliberately **not** swappable. It owns the things that
//! every implementation would otherwise have to get right independently:
//!
//! - the **carry-over ring**, so a message straddling a read boundary is still
//!   found;
//! - **absolute sample offsets**, so a message has the same identity no matter
//!   which buffer it arrived in;
//! - **duplicate suppression**, so one transmission produces one result.
//!
//! If each detector owned its own carry-over they would disagree in small
//! ways, and an A/B comparison between them would be measuring the plumbing
//! rather than the algorithm.
//!
//! # The invariant that makes benchmarking honest
//!
//! Feeding the same bytes in 8 KiB chunks or 256 KiB chunks must produce
//! byte-identical output. The old implementation searched each read buffer
//! independently and silently lost every message spanning a boundary — about
//! 0.1% of traffic, which barely matters for reception but is fatal for a
//! scoreboard, because it makes the score depend on the buffer size. There is
//! a test for this in [`tests::output_is_independent_of_buffer_size`].

use crate::{
    Candidate, RawFrame, Validated,
    detect::PreambleDetector,
    magnitude::{self, Magnitude},
    slice::BitSlicer,
    validate::FrameValidator,
};

/// Counters describing what the pipeline did. The raw material of the
/// scoreboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub samples: u64,
    pub candidates: u64,
    pub slices_attempted: u64,
    pub slices_failed: u64,
    /// Candidates dropped because they landed inside a message already decoded.
    pub suppressed_overlapping: u64,
    pub crc_ok: u64,
    pub crc_ok_corrected: u64,
    pub crc_fail: u64,
    /// Indexed by downlink format, counted before validation.
    pub by_df: [u64; 32],
}

impl PipelineStats {
    /// Fraction of candidates that became messages.
    ///
    /// Report it, never optimize it. A better detector proposes more marginal
    /// candidates, so yield *falls* while absolute message count rises —
    /// measured on our golden fixture, going from 607 to 2,403 messages took
    /// yield from 69% down to 5%. Chasing yield tunes the detector backwards.
    pub fn crc_yield(&self) -> f64 {
        if self.candidates == 0 {
            return 0.0;
        }
        self.crc_ok as f64 / self.candidates as f64
    }

    /// Candidates examined per message recovered — the cost side of the
    /// detector's sensitivity curve.
    pub fn candidates_per_message(&self) -> f64 {
        if self.crc_ok == 0 {
            return f64::INFINITY;
        }
        self.candidates as f64 / self.crc_ok as f64
    }
}

/// A configured four-stage decoder.
pub struct Pipeline {
    mag_impl: Box<dyn Magnitude>,
    detector: Box<dyn PreambleDetector>,
    slicer: Box<dyn BitSlicer>,
    validator: Box<dyn FrameValidator>,

    sample_rate: u32,
    long_message: usize,

    /// Magnitude samples retained across calls. `mag[0]` is absolute sample
    /// `abs_base`.
    mag: Vec<u16>,
    abs_base: u64,
    /// Absolute index up to which candidate search has already completed.
    searched_to: u64,
    /// No candidate before this absolute offset is accepted; set after a
    /// successful decode so one transmission yields one message.
    accept_from: u64,

    candidates: Vec<Candidate>,
    scratch: RawFrame,
    stats: PipelineStats,
}

impl Pipeline {
    pub fn new(
        mag_impl: Box<dyn Magnitude>,
        detector: Box<dyn PreambleDetector>,
        slicer: Box<dyn BitSlicer>,
        validator: Box<dyn FrameValidator>,
        sample_rate: u32,
    ) -> Self {
        Pipeline {
            mag_impl,
            detector,
            slicer,
            validator,
            sample_rate,
            long_message: crate::long_message_samples(sample_rate),
            mag: Vec::new(),
            abs_base: 0,
            searched_to: 0,
            accept_from: 0,
            candidates: Vec::new(),
            scratch: RawFrame::default(),
            stats: PipelineStats::default(),
        }
    }

    pub fn stats(&self) -> PipelineStats {
        self.stats
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Names of the four active implementations, for provenance in results.
    pub fn impl_names(&self) -> [&'static str; 4] {
        [
            self.mag_impl.name(),
            self.detector.name(),
            self.slicer.name(),
            self.validator.name(),
        ]
    }

    /// Return to the state of a fresh pipeline. Called between fixtures so a
    /// run cannot be influenced by whatever ran before it.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.mag.clear();
        self.abs_base = 0;
        self.searched_to = 0;
        self.accept_from = 0;
        self.candidates.clear();
        self.stats = PipelineStats::default();
    }

    /// Feed interleaved `u8` IQ. Appends any messages found to `out`.
    ///
    /// Chunk size affects only latency and memory, never results.
    pub fn feed(&mut self, iq: &[u8], out: &mut Vec<Validated>) {
        let new_samples = iq.len() / 2;
        if new_samples == 0 {
            return;
        }
        self.stats.samples += new_samples as u64;

        // Append the new magnitudes after whatever was carried over.
        let start = self.mag.len();
        self.mag.resize(start + new_samples, 0);
        self.mag_impl.compute(iq, &mut self.mag[start..]);

        self.search(out);
        self.trim();
    }

    /// Flush the tail once the stream has ended.
    ///
    /// A message in the final ~120 µs cannot be decoded while more input might
    /// still arrive, because the slicer would run off the end of the buffer.
    /// At end of stream we know no more is coming, so the remainder can be
    /// searched with a shrinking horizon.
    pub fn finish(&mut self, out: &mut Vec<Validated>) {
        // Everything still buffered is now final; allow the search right up to
        // the point where a long message no longer fits, then let the slicer
        // reject what it cannot complete.
        let available = self.mag.len();
        let from = (self.searched_to - self.abs_base) as usize;
        if from < available {
            self.run_detector(from, available);
            self.searched_to = self.abs_base + available as u64;
            self.drain_candidates(out);
        }
    }

    fn search(&mut self, out: &mut Vec<Validated>) {
        // Only search offsets where a full long message still fits, so the
        // slicer never runs off the end mid-frame.
        let available = self.mag.len();
        let horizon = available.saturating_sub(self.long_message);
        let from = (self.searched_to - self.abs_base) as usize;
        if horizon <= from {
            return;
        }

        self.run_detector(from, horizon);
        self.searched_to = self.abs_base + horizon as u64;
        self.drain_candidates(out);
    }

    fn run_detector(&mut self, from: usize, to: usize) {
        self.candidates.clear();
        self.detector
            .detect(&self.mag, from, to, &mut self.candidates);

        // A detector that emits outside its range would duplicate or drop
        // messages depending on chunking. Catch it in development rather than
        // via a mysteriously unstable benchmark.
        debug_assert!(
            self.candidates
                .iter()
                .all(|c| c.offset >= from && c.offset < to),
            "{} emitted a candidate outside [{from}, {to})",
            self.detector.name()
        );

        self.stats.candidates += self.candidates.len() as u64;
    }

    fn drain_candidates(&mut self, out: &mut Vec<Validated>) {
        // Take the candidate list so the borrow checker lets us call through
        // to the slicer, which borrows `self.mag`.
        let candidates = std::mem::take(&mut self.candidates);

        for cand in &candidates {
            let abs = self.abs_base + cand.offset as u64;

            // One transmission, one message: skip anything landing inside a
            // frame we already accepted.
            if abs < self.accept_from {
                self.stats.suppressed_overlapping += 1;
                continue;
            }

            self.stats.slices_attempted += 1;
            if !self.slicer.slice(&self.mag, cand, &mut self.scratch) {
                self.stats.slices_failed += 1;
                continue;
            }
            self.scratch.offset = abs;

            let df = self.scratch.df();
            self.stats.by_df[usize::from(df) & 31] += 1;

            match self.validator.validate(&self.scratch) {
                Some(v) => {
                    self.stats.crc_ok += 1;
                    if v.corrected_bits > 0 {
                        self.stats.crc_ok_corrected += 1;
                    }
                    // Suppress candidates inside the frame we just took.
                    let span = self.frame_span_samples(self.scratch.len);
                    self.accept_from = abs + span as u64;
                    out.push(v);
                }
                None => self.stats.crc_fail += 1,
            }
        }

        self.candidates = candidates;
        self.candidates.clear();
    }

    /// Samples occupied by a preamble plus `bits` of data.
    fn frame_span_samples(&self, bits: usize) -> usize {
        let spus = crate::samples_per_us(self.sample_rate);
        ((crate::PREAMBLE_US + bits as f64) * spus).ceil() as usize
    }

    /// Drop magnitude samples that can no longer be needed.
    fn trim(&mut self) {
        // Future searches begin at `searched_to` and may look back
        // `lookback()` samples, so anything earlier is dead.
        let keep_from_abs = self
            .searched_to
            .saturating_sub(self.detector.lookback() as u64);
        let drop = keep_from_abs.saturating_sub(self.abs_base) as usize;
        if drop == 0 {
            return;
        }
        let drop = drop.min(self.mag.len());
        self.mag.drain(..drop);
        self.abs_base += drop as u64;
    }

    /// Noise floor of the most recent buffer, for reporting.
    pub fn noise_floor(&self) -> u16 {
        magnitude::noise_floor(&self.mag)
    }
}

/// Run a whole buffer through a pipeline in one go. Convenience for tests.
pub fn decode_all(pipeline: &mut Pipeline, iq: &[u8]) -> Vec<Validated> {
    let mut out = Vec::new();
    pipeline.feed(iq, &mut out);
    pipeline.finish(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use crate::synth;

    const RATE: u32 = 2_400_000;

    fn baseline() -> Pipeline {
        registry::build(&registry::ImplSet::baseline(), RATE).expect("baseline must build")
    }

    #[test]
    fn recovers_every_synthetic_message() {
        let messages = synth::canonical_messages();
        let iq = synth::synthesize(&messages, &synth::SynthConfig::default());

        let mut p = baseline();
        let found = decode_all(&mut p, &iq);

        let want: Vec<String> = messages
            .iter()
            .map(|m| adsb_core::bytes_to_hex(m))
            .collect();
        let got: Vec<String> = found.iter().map(|v| v.hex()).collect();
        assert_eq!(got, want, "stats: {:?}", p.stats());
    }

    #[test]
    fn reports_absolute_offsets_matching_the_generator() {
        let messages = synth::canonical_messages();
        let cfg = synth::SynthConfig::default();
        let iq = synth::synthesize(&messages, &cfg);

        let mut p = baseline();
        let found = decode_all(&mut p, &iq);

        for (n, v) in found.iter().enumerate() {
            let expected = synth::message_offset(&cfg, n) as u64;
            let delta = v.offset.abs_diff(expected);
            assert!(
                delta <= 2,
                "message {n} at {} not near {expected}",
                v.offset
            );
        }
    }

    /// The property that makes the scoreboard trustworthy.
    #[test]
    fn output_is_independent_of_buffer_size() {
        let iq = synth::synthesize(
            &[synth::canonical_messages(), synth::ottawa_messages()].concat(),
            &synth::SynthConfig::default(),
        );

        let mut reference: Option<Vec<(u64, String)>> = None;
        for chunk_samples in [512usize, 1024, 4096, 65_536, iq.len()] {
            let mut p = baseline();
            let mut out = Vec::new();
            for chunk in iq.chunks(chunk_samples * 2) {
                p.feed(chunk, &mut out);
            }
            p.finish(&mut out);

            let digest: Vec<(u64, String)> = out.iter().map(|v| (v.offset, v.hex())).collect();
            match &reference {
                None => reference = Some(digest),
                Some(want) => assert_eq!(
                    &digest, want,
                    "chunk size {chunk_samples} changed the result"
                ),
            }
        }
        assert!(!reference.unwrap().is_empty(), "decoded nothing at all");
    }

    #[test]
    fn one_transmission_yields_one_message() {
        let msg = adsb_core::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let iq = synth::synthesize(&[msg], &synth::SynthConfig::default());
        let mut p = baseline();
        let found = decode_all(&mut p, &iq);
        assert_eq!(found.len(), 1, "duplicate suppression failed");
    }

    /// Suppression must not be so eager that it eats the *next* aircraft.
    /// Repeating the same payload is the strict case: identical bytes, so only
    /// the offsets distinguish them.
    #[test]
    fn repeated_transmissions_are_all_reported() {
        let msg = adsb_core::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let iq = synth::synthesize(
            &[msg.clone(), msg.clone(), msg],
            &synth::SynthConfig::default(),
        );
        let mut p = baseline();
        let found = decode_all(&mut p, &iq);
        assert_eq!(found.len(), 3, "over-suppressed: {:?}", p.stats());

        // Distinct absolute offsets are what make them distinguishable.
        let offsets: Vec<u64> = found.iter().map(|v| v.offset).collect();
        assert!(
            offsets.windows(2).all(|w| w[1] > w[0]),
            "offsets not strictly increasing: {offsets:?}"
        );
    }

    /// Suppression is only *needed* when a detector proposes several starts
    /// inside one frame. The naive detector rarely does at 2.4 MS/s, since a
    /// 0.5 us pulse is only ~1.2 samples wide, so this drives the mechanism
    /// directly rather than hoping it triggers.
    #[test]
    fn candidates_inside_an_accepted_frame_are_suppressed() {
        let msg = adsb_core::hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let cfg = synth::SynthConfig::default();
        let iq = synth::synthesize(&[msg], &cfg);
        let mut p = baseline();

        let mut out = Vec::new();
        p.feed(&iq, &mut out);
        assert_eq!(out.len(), 1, "setup: the message should decode once");

        // The decoded region has already been trimmed out of the buffer, so
        // drive the predicate directly: with `accept_from` in the far future,
        // every candidate counts as landing inside an accepted frame.
        p.accept_from = u64::MAX;
        let before = p.stats().suppressed_overlapping;
        p.candidates.push(Candidate {
            offset: 0,
            frac: 0.0,
            score: 5.0,
            noise: 300,
            signal: 40_000,
        });
        p.drain_candidates(&mut out);

        assert_eq!(
            out.len(),
            1,
            "a mid-frame candidate produced a second message"
        );
        assert_eq!(
            p.stats().suppressed_overlapping,
            before + 1,
            "the mid-frame candidate should have been suppressed"
        );
    }

    #[test]
    fn pure_noise_decodes_nothing() {
        let iq = synth::noise_only(RATE, 200_000, 3.0, 0xBEEF);
        let mut p = baseline();
        let found = decode_all(&mut p, &iq);
        assert!(found.is_empty(), "invented {} messages", found.len());
    }

    #[test]
    fn reset_clears_all_state() {
        let iq = synth::synthesize(&synth::canonical_messages(), &synth::SynthConfig::default());
        let mut p = baseline();
        let first = decode_all(&mut p, &iq);
        p.reset();
        assert_eq!(p.stats(), PipelineStats::default());
        let second = decode_all(&mut p, &iq);
        let hexes = |v: &Vec<Validated>| v.iter().map(|x| x.hex()).collect::<Vec<_>>();
        assert_eq!(hexes(&first), hexes(&second));
    }

    #[test]
    fn memory_does_not_grow_without_bound() {
        // Trimming must actually release samples, or a long-running receiver
        // on the Pi accumulates the whole stream in RAM.
        let iq = synth::noise_only(RATE, 100_000, 2.0, 1);
        let mut p = baseline();
        let mut out = Vec::new();
        for _ in 0..20 {
            p.feed(&iq, &mut out);
        }
        assert!(
            p.mag.len() < 400,
            "retained {} samples after 2M fed",
            p.mag.len()
        );
    }
}
