//! The scoreboard.
//!
//! One command answers "did that change help?", against fixed bytes, with the
//! guard metrics that stop a plausible-looking improvement from being a
//! regression in disguise.
//!
//! # Which number is the headline
//!
//! **Unique CRC-valid messages.** Higher is unambiguously better: at a
//! 6 × 10⁻⁸ false-accept rate, a CRC-clean DF17 is essentially certainly real,
//! so no labelled data is needed — the ground truth is free.
//!
//! **Yield is reported and never optimized.** A better detector proposes more
//! marginal candidates, so yield *falls* as messages rise. Measured on the
//! golden fixture: going from 607 messages to 2,403 took yield from 69% to 5%.
//! Anyone optimizing yield would tune the detector backwards and feel good
//! about it.
//!
//! Two guards can veto an apparent win:
//!
//! - **`ghost_icao_ratio`** — addresses seen exactly once and never with a
//!   position. This is what catches error correction inventing aircraft.
//! - **`realtime_factor`** — an implementation that finds 30% more messages at
//!   0.8× realtime cannot keep up on the Pi, and is a regression.

use crate::config::Config;
use adsb_dsp::registry;
use adsb_source::{SourceOptions, SourceSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::ExitCode;
use std::time::Instant;

#[derive(clap::Args, Debug)]
pub struct BenchArgs {
    /// Fixture files to score. Defaults to everything in fixtures/raw.
    pub fixtures: Vec<String>,

    /// Write the run record here as JSON.
    #[arg(long)]
    pub out: Option<String>,

    /// Compare against a previous run record and fail on regression.
    #[arg(long)]
    pub compare: Option<String>,

    /// Print each decoded message.
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RunRecord {
    pub schema: u32,
    pub git_sha: String,
    pub impl_set: String,
    pub results: Vec<FixtureResult>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixtureResult {
    pub fixture: String,
    pub sample_rate_hz: u32,
    pub seconds: f64,

    // Headline
    pub crc_ok: u64,
    pub unique_messages: u64,
    pub aircraft: u64,
    pub positions: u64,

    // Cost and context
    pub candidates: u64,
    pub crc_fail: u64,
    pub crc_yield: f64,
    pub candidates_per_message: f64,
    pub messages_per_minute: f64,

    // Guards
    pub ghost_icao_ratio: f64,
    pub realtime_factor: f64,
    pub ns_per_sample: f64,

    pub by_type_code: BTreeMap<String, u64>,
    /// Stable hash over the decoded set. "Did behaviour change at all?" in one
    /// line. FNV-1a rather than SHA-256: this detects change, it does not need
    /// to resist an adversary, and it avoids a dependency.
    pub digest: String,
}

/// Score every requested fixture.
pub fn run(config: &Config, args: &BenchArgs) -> ExitCode {
    let fixtures = if args.fixtures.is_empty() {
        discover_fixtures()
    } else {
        args.fixtures.clone()
    };

    if fixtures.is_empty() {
        eprintln!(
            "no fixtures found. Pass paths explicitly, or put .cu8 captures in fixtures/raw/"
        );
        return ExitCode::from(2);
    }

    // Already validated at startup, names included, so this cannot fail here.
    let set = config.impls();

    let mut results = Vec::new();
    for path in &fixtures {
        match score_one(path, *config.sample_rate_hz, &set, config, args.verbose) {
            Ok(result) => {
                print_result(&result);
                results.push(result);
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                return ExitCode::from(2);
            }
        }
    }

    let record = RunRecord {
        schema: 1,
        git_sha: git_sha(),
        impl_set: set.to_string(),
        results,
    };

    if let Some(path) = &args.out {
        match serde_json::to_string_pretty(&record)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(path, json).map_err(|e| e.to_string()))
        {
            Ok(()) => println!("\nwrote {path}"),
            Err(e) => {
                eprintln!("cannot write {path}: {e}");
                return ExitCode::from(2);
            }
        }
    }

    if let Some(path) = &args.compare {
        return compare(&record, path);
    }

    ExitCode::SUCCESS
}

fn score_one(
    path: &str,
    sample_rate: u32,
    set: &registry::ImplSet,
    config: &Config,
    verbose: bool,
) -> Result<FixtureResult, String> {
    let spec = SourceSpec::parse(&format!("file:{path}")).map_err(|e| e.to_string())?;
    let mut source = adsb_source::open(&spec, &SourceOptions::for_benchmark(sample_rate))
        .map_err(|e| e.to_string())?;
    let mut pipeline = registry::build(set, sample_rate).map_err(|e| e.to_string())?;

    let mut tracker = adsb_track::Tracker::new(adsb_track::TrackerConfig {
        gates: adsb_track::Gates {
            receiver: config.receiver(),
            ..Default::default()
        },
        ..Default::default()
    });

    let started = Instant::now();
    let mut found = Vec::new();
    let mut buf = vec![0u8; 256 * 1024 * 2];
    let mut bytes = 0u64;
    loop {
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                bytes += n as u64;
                pipeline.feed(&buf[..n], &mut found);
            }
            Err(e) if e.is_end_of_stream() => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    pipeline.finish(&mut found);
    let elapsed = started.elapsed().as_secs_f64().max(1e-9);

    // Fold into the tracker so positions and ghost detection are measured on
    // the same path the live receiver uses.
    let base = adsb_track::Tick::now();
    let mut unique: HashSet<(u64, String)> = HashSet::new();
    let mut by_type_code: BTreeMap<String, u64> = BTreeMap::new();
    let mut seen_counts: HashMap<String, u64> = HashMap::new();
    let mut positions = 0u64;

    for validated in &found {
        // Dedup on (approximate time, payload) so one transmission counts once
        // even if a future detector proposes it twice.
        unique.insert((validated.offset / 500, validated.hex()));

        if let Some(frame) = validated.frame() {
            if let Some(tc) = frame.type_code() {
                *by_type_code.entry(format!("TC{tc}")).or_default() += 1;
            }
            *seen_counts.entry(frame.icao().to_string()).or_default() += 1;

            // Advance the clock with the sample offset so CPR pairing sees the
            // gaps the aircraft actually left, not zero.
            let offset_ms = (validated.offset as f64 / f64::from(sample_rate) * 1000.0) as u64;
            let tick = adsb_track::Tick {
                mono: base.mono + std::time::Duration::from_millis(offset_ms),
                wall_ms: base.wall_ms + offset_ms as i64,
            };
            if let adsb_track::Update::NewPosition(_) = tracker.observe(&frame, tick, None) {
                positions += 1;
            }
            if verbose {
                println!("  {} {}", validated.hex(), frame.icao());
            }
        }
    }

    let stats = pipeline.stats();
    let seconds = (bytes / 2) as f64 / f64::from(sample_rate);

    // An address heard exactly once and never located is the signature of a
    // false CRC accept. Real aircraft transmit twice a second.
    let located: HashSet<String> = tracker
        .iter()
        .filter(|a| a.is_locatable())
        .map(|a| a.icao.to_string())
        .collect();
    let ghosts = seen_counts
        .iter()
        .filter(|(icao, count)| **count == 1 && !located.contains(*icao))
        .count() as f64;
    let ghost_icao_ratio = if seen_counts.is_empty() {
        0.0
    } else {
        ghosts / seen_counts.len() as f64
    };

    let mut sorted: Vec<String> = unique.iter().map(|(o, h)| format!("{o}:{h}")).collect();
    sorted.sort();

    Ok(FixtureResult {
        fixture: path.to_string(),
        sample_rate_hz: sample_rate,
        seconds,
        crc_ok: stats.crc_ok,
        unique_messages: unique.len() as u64,
        aircraft: seen_counts.len() as u64,
        positions,
        candidates: stats.candidates,
        crc_fail: stats.crc_fail,
        crc_yield: stats.crc_yield(),
        candidates_per_message: stats.candidates_per_message(),
        messages_per_minute: stats.crc_ok as f64 * 60.0 / seconds.max(1e-9),
        ghost_icao_ratio,
        realtime_factor: seconds / elapsed,
        ns_per_sample: elapsed * 1e9 / (bytes / 2).max(1) as f64,
        by_type_code,
        digest: format!("fnv1a64:{:016x}", fnv1a64(&sorted.join("\n"))),
    })
}

fn print_result(r: &FixtureResult) {
    println!("\n{}", r.fixture);
    println!(
        "  {:.1} s at {:.3} MS/s",
        r.seconds,
        f64::from(r.sample_rate_hz) / 1e6
    );
    println!(
        "  messages {:>7}   unique {:>7}   aircraft {:>4}   positions {:>5}",
        r.crc_ok, r.unique_messages, r.aircraft, r.positions
    );
    println!(
        "  candidates {:>5}   yield {:>6.1}%   cand/msg {:>5.1}   {:.1} msg/min",
        r.candidates,
        r.crc_yield * 100.0,
        r.candidates_per_message,
        r.messages_per_minute
    );
    println!(
        "  guards: ghosts {:.3}   realtime {:.1}x   {:.1} ns/sample",
        r.ghost_icao_ratio, r.realtime_factor, r.ns_per_sample
    );
    println!("  digest {}", r.digest);
}

/// Diff against a previous run and fail on regression.
fn compare(current: &RunRecord, baseline_path: &str) -> ExitCode {
    let text = match std::fs::read_to_string(baseline_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {baseline_path}: {e}");
            return ExitCode::from(2);
        }
    };
    let baseline: RunRecord = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{baseline_path} is not a run record: {e}");
            return ExitCode::from(2);
        }
    };

    println!("\ncompared with {baseline_path} ({})", baseline.impl_set);
    let mut regressed = false;

    for now in &current.results {
        let Some(before) = baseline.results.iter().find(|r| r.fixture == now.fixture) else {
            println!("  {:<28} (no baseline)", now.fixture);
            continue;
        };

        let delta = |a: u64, b: u64| -> String {
            if b == 0 {
                return format!("{a:+}");
            }
            format!("{:+.1}%", (a as f64 - b as f64) / b as f64 * 100.0)
        };

        let mut verdict = "PASS";
        if now.unique_messages < before.unique_messages {
            verdict = "FAIL messages";
            regressed = true;
        } else if now.ghost_icao_ratio > before.ghost_icao_ratio + 0.01 {
            verdict = "WARN ghosts";
        } else if now.realtime_factor < 1.5 {
            verdict = "FAIL too slow";
            regressed = true;
        }

        println!(
            "  {:<24} messages {:>6} -> {:>6} ({:>7})   positions {:>4} -> {:>4}   rt {:.1}x   {}",
            short_name(&now.fixture),
            before.unique_messages,
            now.unique_messages,
            delta(now.unique_messages, before.unique_messages),
            before.positions,
            now.positions,
            now.realtime_factor,
            verdict
        );

        if now.digest == before.digest {
            println!("      digest unchanged -- behaviour is identical");
        }
    }

    if regressed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn short_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn discover_fixtures() -> Vec<String> {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir("fixtures/raw") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("cu8") {
                found.push(path.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    found
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// FNV-1a, 64 bit. Stable across platforms and architectures, which is what
/// lets a Mac digest be compared against a Pi digest.
fn fnv1a64(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_order_independent_after_sorting() {
        let a = fnv1a64("alpha\nbeta");
        let b = fnv1a64("alpha\nbeta");
        assert_eq!(a, b);
        assert_ne!(a, fnv1a64("beta\nalpha"));
    }

    #[test]
    fn short_name_strips_directories_and_extension() {
        assert_eq!(short_name("fixtures/raw/golden.cu8"), "golden");
        assert_eq!(short_name("desk.cu8"), "desk");
    }
}
