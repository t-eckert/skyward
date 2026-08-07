// gen.rs — synthesize a raw rtl_sdr-format IQ file containing known ADS-B
// messages, so the demodulator can be tested against ground truth.
//
// rustc -O gen.rs -o gen ; ./gen out.iq <sample_rate> <amplitude> <noise>
use std::env;
use std::fs::File;
use std::io::Write;

// Known-good DF17 messages (these are the canonical examples from the
// ADS-B literature; each has a valid CRC-24).
const MSGS: &[&str] = &[
    "8D4840D6202CC371C32CE0576098", // ident, callsign KLM1023
    "8D40621D58C382D690C8AC2863A7", // airborne position, even
    "8D40621D58C386435CC412692AD6", // airborne position, odd
    "8D485020994409940838175B284F", // velocity
];

fn main() {
    let a: Vec<String> = env::args().collect();
    let path = &a[1];
    let fs: f64 = a[2].parse().unwrap();
    let amp: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(60.0);
    let noise: f64 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(2.0);
    let spus = fs / 1e6;

    // envelope[k] = desired amplitude at sample k
    let gap_us = 200.0;
    let total_us = MSGS.len() as f64 * (8.0 + 112.0 + gap_us) + gap_us;
    let n = (total_us * spus) as usize;
    let mut env = vec![0f64; n];

    let mut t = gap_us; // current time in us
    for hex in MSGS {
        let bits = hex_to_bits(hex);
        // preamble pulses at 0, 1, 3.5, 4.5 us, each 0.5 us wide
        for &p in &[0.0, 1.0, 3.5, 4.5] {
            pulse(&mut env, spus, t + p, 0.5);
        }
        // 112 data bits: energy in first half = 1, second half = 0
        for (k, &b) in bits.iter().enumerate() {
            let start = t + 8.0 + k as f64 + if b == 1 { 0.0 } else { 0.5 };
            pulse(&mut env, spus, start, 0.5);
        }
        t += 8.0 + 112.0 + gap_us;
    }

    // Turn the envelope into offset-binary u8 IQ with a random carrier phase
    // per sample plus Gaussian-ish noise.
    let mut seed = 0x243F6A8885A308D3u64;
    let mut rng = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let mut out = Vec::with_capacity(n * 2);
    let mut phase = 0f64;
    for k in 0..n {
        phase += 0.31; // arbitrary residual carrier offset
        let mut nz = || {
            // sum of 4 uniforms ~ approx normal
            let mut s = 0f64;
            for _ in 0..4 { s += (rng() >> 40) as f64 / 16777216.0 - 0.5 }
            s * noise
        };
        let i = env[k] * phase.cos() + nz();
        let q = env[k] * phase.sin() + nz();
        out.push(((i + 127.5).round().clamp(0.0, 255.0)) as u8);
        out.push(((q + 127.5).round().clamp(0.0, 255.0)) as u8);
    }
    File::create(path).unwrap().write_all(&out).unwrap();
    println!("wrote {path}: {} samples, {} messages, amp={amp} noise={noise}", n, MSGS.len());
    for m in MSGS { println!("  expect {m}") }

    fn pulse(env: &mut [f64], spus: f64, start_us: f64, width_us: f64) {
        let a = (start_us * spus).round() as usize;
        let b = ((start_us + width_us) * spus).round() as usize;
        for k in a..b.min(env.len()) { env[k] = 60.0 }
    }
}

fn hex_to_bits(hex: &str) -> Vec<u8> {
    let mut bits = Vec::with_capacity(112);
    for c in hex.chars() {
        let v = c.to_digit(16).unwrap() as u8;
        for s in (0..4).rev() { bits.push((v >> s) & 1) }
    }
    bits
}
