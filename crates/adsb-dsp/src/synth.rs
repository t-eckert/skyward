//! Synthetic IQ generation — ground truth you can dial.
//!
//! Real captures give you CRC-clean bytes, which tells you a message was
//! recovered but never that one was *missed*. Synthetic signals give you the
//! answer key: you know exactly how many messages are in the buffer, where
//! each one starts, and what it should decode to.
//!
//! Two things this makes possible that a capture cannot:
//!
//! - **SNR sweeps.** Generate the same messages at descending signal levels
//!   and plot recovery against SNR. A single number tells you an
//!   implementation is better; a curve tells you *where* it is better, which
//!   is far more useful when deciding what to work on next.
//! - **Coverage of message types the sky did not provide.** Our golden fixture
//!   has no surface positions, no Gillham altitudes and no airspeed subtypes,
//!   because nothing nearby was transmitting them. Here you can just ask.
//!
//! This module is also compiled into `doctor`, so the Pi can prove its decode
//! chain works with no antenna attached.

use crate::{PREAMBLE_US, samples_per_us};

/// How the synthetic signal should look.
#[derive(Clone, Copy, Debug)]
pub struct SynthConfig {
    pub sample_rate: u32,
    /// Peak IQ amplitude of a pulse, in the `u8` sample domain. Full scale is
    /// 127.5, so values above about 120 will clip — which is itself worth
    /// testing.
    pub signal_amplitude: f64,
    /// Standard deviation of the additive noise, same units.
    pub noise_amplitude: f64,
    /// Constant offset added to I and Q. Real dongles have a small one; a
    /// large one usually means direct sampling got left enabled.
    pub dc_offset: (f64, f64),
    /// Quiet time before the first message and between messages.
    pub gap_us: f64,
    /// Residual carrier offset, radians per sample. Magnitude demodulation
    /// should be completely indifferent to this — a good thing to verify.
    pub phase_step: f64,
    pub seed: u64,
}

impl Default for SynthConfig {
    fn default() -> Self {
        SynthConfig {
            sample_rate: 2_400_000,
            signal_amplitude: 60.0,
            noise_amplitude: 2.0,
            dc_offset: (0.0, 0.0),
            gap_us: 200.0,
            phase_step: 0.31,
            seed: 0x243F_6A88_85A3_08D3,
        }
    }
}

impl SynthConfig {
    /// Signal-to-noise ratio in dB.
    pub fn snr_db(&self) -> f64 {
        if self.noise_amplitude <= 0.0 {
            return f64::INFINITY;
        }
        20.0 * (self.signal_amplitude / self.noise_amplitude).log10()
    }

    /// A config with the signal level set to hit a target SNR, keeping the
    /// noise where it is. The knob for sweeps.
    pub fn with_snr_db(mut self, snr_db: f64) -> Self {
        self.signal_amplitude = self.noise_amplitude * 10f64.powf(snr_db / 20.0);
        self
    }
}

/// Sample offset at which the first message's preamble begins.
pub fn first_message_offset(sample_rate: u32) -> usize {
    (SynthConfig::default().gap_us * samples_per_us(sample_rate)).round() as usize
}

/// Sample offset of message `n`, given a config.
pub fn message_offset(cfg: &SynthConfig, n: usize) -> usize {
    let spus = samples_per_us(cfg.sample_rate);
    let stride = cfg.gap_us + PREAMBLE_US + 112.0;
    ((cfg.gap_us + stride * n as f64) * spus).round() as usize
}

/// Known-good frames with published decodings, useful as an answer key.
///
/// Includes the canonical identification, an even/odd position pair whose
/// resolved location is documented in `adsb_core::cpr`, a velocity message,
/// and a short DF11 so short-frame handling gets exercised too.
pub fn canonical_messages() -> Vec<Vec<u8>> {
    [
        "8D4840D6202CC371C32CE0576098", // ident, KLM1023
        "8D40621D58C382D690C8AC2863A7", // position, even
        "8D40621D58C386435CC412692AD6", // position, odd
        "8D485020994409940838175B284F", // velocity
        "5DC04413BFAD35",               // DF11 all-call reply (short)
    ]
    .iter()
    .map(|h| adsb_core::hex_to_bytes(h).unwrap())
    .collect()
}

/// Frames captured off the air in Ottawa on 2026-08-06.
pub fn ottawa_messages() -> Vec<Vec<u8>> {
    [
        "8DC060B6587D8236085C837FDA27", // position, FL240
        "8DC060B6990D2985500834E20E9E", // velocity, 298 kt
        "8DC053F699093B19D030164300A1", // velocity, climbing
        "8DC053F6F82300020049B8A00CD4", // operational status
    ]
    .iter()
    .map(|h| adsb_core::hex_to_bytes(h).unwrap())
    .collect()
}

/// Deterministic pseudo-random source. Not cryptographic; reproducibility is
/// the entire requirement, because a benchmark that moves between runs is not
/// a benchmark.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Roughly Gaussian: the sum of four uniforms, via the central limit
    /// theorem. Good enough for a receiver test and far cheaper than Box-Muller.
    fn gaussian(&mut self) -> f64 {
        let mut sum = 0.0;
        for _ in 0..4 {
            sum += (self.next_u64() >> 40) as f64 / 16_777_216.0 - 0.5;
        }
        sum
    }
}

/// Generate interleaved `u8` IQ containing the given frames.
pub fn synthesize(messages: &[Vec<u8>], cfg: &SynthConfig) -> Vec<u8> {
    let spus = samples_per_us(cfg.sample_rate);
    let stride_us = cfg.gap_us + PREAMBLE_US + 112.0;
    let total_us = cfg.gap_us + stride_us * messages.len() as f64;
    let n = (total_us * spus).ceil() as usize;

    // Build the amplitude envelope first, then modulate it onto a carrier.
    let mut env = vec![0f64; n];
    let mut pulse = |start_us: f64, width_us: f64| {
        let a = (start_us * spus).round() as usize;
        let b = ((start_us + width_us) * spus).round() as usize;
        for slot in env.iter_mut().take(b.min(n)).skip(a) {
            *slot = cfg.signal_amplitude;
        }
    };

    for (m, msg) in messages.iter().enumerate() {
        let t0 = cfg.gap_us + stride_us * m as f64;

        // Preamble: 0.5 us pulses beginning at 0, 1, 3.5 and 4.5 us.
        for offset in [0.0, 1.0, 3.5, 4.5] {
            pulse(t0 + offset, 0.5);
        }

        // Data: energy in the first half of the bit for 1, second half for 0.
        for (k, bit) in adsb_core::bits::unpack(msg).iter().enumerate() {
            let start = t0 + PREAMBLE_US + k as f64 + if *bit == 1 { 0.0 } else { 0.5 };
            pulse(start, 0.5);
        }
    }

    let mut rng = Rng(cfg.seed);
    let mut out = Vec::with_capacity(n * 2);
    let mut phase = 0f64;
    for &amplitude in env.iter() {
        phase += cfg.phase_step;
        let i = amplitude * phase.cos() + rng.gaussian() * cfg.noise_amplitude + cfg.dc_offset.0;
        let q = amplitude * phase.sin() + rng.gaussian() * cfg.noise_amplitude + cfg.dc_offset.1;
        out.push((i + 127.5).round().clamp(0.0, 255.0) as u8);
        out.push((q + 127.5).round().clamp(0.0, 255.0) as u8);
    }
    out
}

/// Pure noise, for measuring false-positive rates.
pub fn noise_only(sample_rate: u32, samples: usize, noise_amplitude: f64, seed: u64) -> Vec<u8> {
    let _ = sample_rate;
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(samples * 2);
    for _ in 0..samples {
        let i = rng.gaussian() * noise_amplitude;
        let q = rng.gaussian() * noise_amplitude;
        out.push((i + 127.5).round().clamp(0.0, 255.0) as u8);
        out.push((q + 127.5).round().clamp(0.0, 255.0) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_length_matches_the_configured_layout() {
        let cfg = SynthConfig::default();
        let iq = synthesize(&canonical_messages()[..1], &cfg);
        let spus = samples_per_us(cfg.sample_rate);
        let expected_us = cfg.gap_us + (cfg.gap_us + PREAMBLE_US + 112.0);
        assert_eq!(iq.len() / 2, (expected_us * spus).ceil() as usize);
    }

    #[test]
    fn generation_is_deterministic() {
        let cfg = SynthConfig::default();
        let a = synthesize(&canonical_messages(), &cfg);
        let b = synthesize(&canonical_messages(), &cfg);
        assert_eq!(a, b, "same seed must give identical bytes");
    }

    #[test]
    fn a_different_seed_gives_different_noise() {
        let a = synthesize(&canonical_messages(), &SynthConfig::default());
        let b = synthesize(
            &canonical_messages(),
            &SynthConfig {
                seed: 99,
                ..Default::default()
            },
        );
        assert_ne!(a, b);
    }

    #[test]
    fn snr_knob_is_consistent() {
        let cfg = SynthConfig::default().with_snr_db(20.0);
        assert!((cfg.snr_db() - 20.0).abs() < 1e-9);
        // 20 dB is a factor of ten in amplitude.
        assert!((cfg.signal_amplitude / cfg.noise_amplitude - 10.0).abs() < 1e-9);
    }

    #[test]
    fn message_offsets_are_where_they_claim() {
        let cfg = SynthConfig::default();
        assert_eq!(
            message_offset(&cfg, 0),
            first_message_offset(cfg.sample_rate)
        );
        // Each subsequent message is one full stride further along.
        let spus = samples_per_us(cfg.sample_rate);
        let stride = ((cfg.gap_us + PREAMBLE_US + 112.0) * spus).round() as usize;
        assert_eq!(message_offset(&cfg, 1) - message_offset(&cfg, 0), stride);
    }

    #[test]
    fn clean_signal_has_a_clear_dynamic_range() {
        let cfg = SynthConfig {
            noise_amplitude: 0.0,
            ..Default::default()
        };
        let iq = synthesize(&canonical_messages()[..1], &cfg);
        let mag = crate::magnitude::compute_vec(&crate::magnitude::NaiveMagnitude, &iq);
        let peak = *mag.iter().max().unwrap();
        let floor = *mag.iter().min().unwrap();
        assert!(peak > 20_000, "peak was only {peak}");

        // Offset-binary u8 cannot represent exact zero: 127.5 falls between
        // 127 and 128, so even a perfectly silent channel quantizes to half an
        // LSB on each axis. That puts the floor at |IQ| = 0.5*sqrt(2) -- about
        // 257 in u16 units -- and no amount of noise reduction goes below it.
        let quantization_floor = (0.5 * f32::sqrt(2.0) * crate::MAG_SCALE).round() as u16;
        assert_eq!(
            floor, quantization_floor,
            "silence should sit exactly at the ADC quantization floor"
        );
    }

    #[test]
    fn noise_only_produces_no_strong_bursts() {
        let iq = noise_only(2_400_000, 5_000, 2.0, 7);
        let mag = crate::magnitude::compute_vec(&crate::magnitude::NaiveMagnitude, &iq);
        let peak = *mag.iter().max().unwrap();
        assert!(peak < 8_000, "noise reached {peak}");
    }
}
