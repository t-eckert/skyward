//! `skyward doctor` — the command for a machine you cannot log into.
//!
//! Every check ends in a sentence an operator can act on. "clip_pct = 3.2" is
//! a measurement; "gain too high, reduce to about 44 dB and re-run" is a
//! diagnosis, and only the second one is useful at arm's length.
//!
//! Exit codes: 0 healthy, 1 warnings, 2 something is broken.
//!
//! # Checks that exist because of a specific failure
//!
//! **The clock.** A Raspberry Pi has no real-time clock, so it boots in 1970.
//! Every timestamp is then garbage, `?max_age=` filters return nothing, and
//! the whole system looks like a dead radio. It is a three-line check and it
//! is almost always the last thing anyone suspects.
//!
//! **The effective sample rate.** Dropping samples and receiving badly look
//! *identical* in the message count, and one of them is a software problem you
//! can fix. Counting samples and dividing by elapsed wall time distinguishes
//! them immediately.
//!
//! **The offline self-test.** Decoding synthetic frames in memory proves the
//! chain works on the Pi's own CPU with no antenna attached, which separates
//! "bad build" from "bad reception" before you go climbing after the antenna.

use crate::config::Config;
use adsb_dsp::{registry, synth};
use adsb_source::{SourceOptions, SourceSpec};
use std::process::ExitCode;
use std::time::Instant;

#[derive(clap::Args, Debug)]
pub struct DoctorArgs {
    /// Emit JSON instead of prose, for `skyward bundle` and for pasting.
    #[arg(long)]
    pub json: bool,

    /// Seconds of RF to capture for the signal checks.
    #[arg(long, default_value_t = 2)]
    pub capture_seconds: u64,

    /// Skip anything that touches the radio.
    #[arg(long)]
    pub offline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Pass,
    Warn,
    Fail,
}

impl Level {
    fn marker(self) -> &'static str {
        match self {
            Level::Pass => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        }
    }
}

struct Report {
    checks: Vec<(Level, String, String)>,
}

impl Report {
    fn new() -> Self {
        Report { checks: Vec::new() }
    }

    fn add(&mut self, level: Level, name: &str, detail: impl Into<String>) {
        self.checks.push((level, name.to_string(), detail.into()));
    }

    fn worst(&self) -> Level {
        self.checks
            .iter()
            .map(|(l, _, _)| *l)
            .max()
            .unwrap_or(Level::Pass)
    }

    fn print(&self) {
        let mut section = "";
        for (level, name, detail) in &self.checks {
            let prefix = name.split('.').next().unwrap_or("");
            if prefix != section {
                println!();
                section = prefix;
            }
            println!("  [{}] {:<28} {}", level.marker(), name, detail);
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": match self.worst() {
                Level::Pass => "ok",
                Level::Warn => "warn",
                Level::Fail => "fail",
            },
            "checks": self.checks.iter().map(|(level, name, detail)| {
                serde_json::json!({
                    "level": level.marker().trim(),
                    "name": name,
                    "detail": detail,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

pub fn run(config: &Config, args: &DoctorArgs) -> ExitCode {
    let mut report = Report::new();

    check_build(&mut report, config);
    check_config(&mut report, config);
    check_clock(&mut report);
    check_filesystem(&mut report, config);
    check_self_test(&mut report, config);

    if !args.offline {
        check_source(&mut report, config, args.capture_seconds);
    } else {
        report.add(Level::Warn, "radio.skipped", "--offline, nothing was tuned");
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json()).unwrap()
        );
    } else {
        println!("skyward doctor");
        report.print();
        println!();
        match report.worst() {
            Level::Pass => println!("All checks passed."),
            Level::Warn => println!("Warnings above. The receiver will run, but read them."),
            Level::Fail => println!("Something is broken. Fix the FAIL lines first."),
        }
    }

    match report.worst() {
        Level::Pass => ExitCode::SUCCESS,
        Level::Warn => ExitCode::from(1),
        Level::Fail => ExitCode::from(2),
    }
}

fn check_build(report: &mut Report, config: &Config) {
    report.add(
        Level::Pass,
        "build.version",
        format!(
            "skyward {} ({}), rustc target {}",
            env!("CARGO_PKG_VERSION"),
            option_env!("SKYWARD_GIT_SHA").unwrap_or("unknown sha"),
            std::env::consts::ARCH,
        ),
    );

    // Print the *expanded* implementation set. A preset name alone tells the
    // operator nothing about what is actually running, and with per-stage
    // overrides in play the preset name can be actively misleading -- an
    // `--detect` flag makes `baseline` no longer the baseline.
    let set = config.impls();
    match registry::check(&set) {
        Ok(()) => report.add(
            Level::Pass,
            "build.pipeline",
            format!("{} = {set}", *config.impl_set),
        ),
        Err(e) => report.add(Level::Fail, "build.pipeline", e.to_string()),
    }
}

fn check_config(report: &mut Report, config: &Config) {
    match &config.config_path {
        Some(path) => report.add(Level::Pass, "config.file", path.clone()),
        None => report.add(
            Level::Pass,
            "config.file",
            "none; defaults, environment and flags only",
        ),
    }

    // The client is compiled in, so whether it is really there is a property
    // of this binary and belongs in the same report as the build. Otherwise
    // the only way to find out is to open a browser, which on a Pi you are
    // deliberately not doing.
    let (files, bytes) = crate::web::footprint();
    if crate::web::is_built() {
        report.add(
            Level::Pass,
            "web.client",
            format!("{files} files, {} KiB embedded", bytes / 1024),
        );
    } else {
        report.add(
            Level::Warn,
            "web.client",
            "not built into this binary; the API works but the web interface \
             is a placeholder. Run `cd client && npm run build`, then rebuild.",
        );
    }

    // A `.env` only applies when you run from the directory above it, so say
    // which one was found rather than leaving the operator to guess why a
    // value they set is missing.
    match &config.env_file {
        Some(path) => report.add(Level::Pass, "config.env_file", path.clone()),
        None => report.add(
            Level::Pass,
            "config.env_file",
            "none found; environment variables must be set by the caller",
        ),
    }

    // Provenance for every value: this is how an operator confirms an edit
    // actually took effect.
    for line in config.print_resolved().lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        report.add(Level::Pass, &format!("config.{name}"), rest.join(" "));
    }

    if config.receiver().is_none() {
        report.add(
            Level::Warn,
            "config.receiver",
            "position unset: the range gate and local CPR are disabled. Set \
             SKYWARD_RECEIVER_LAT and SKYWARD_RECEIVER_LON (three decimals is plenty)",
        );
    }
}

/// A Pi with no RTC boots at 1970 and everything downstream is wrong.
fn check_clock(report: &mut Report) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 2020-01-01. Anything earlier means the clock has never been set.
    const SANE: u64 = 1_577_836_800;
    if now < SANE {
        report.add(
            Level::Fail,
            "clock.wall",
            format!(
                "system time is {now} (before 2020). This Pi has no real-time clock and \
                 has not reached an NTP server. Every timestamp will be wrong and the API \
                 will look empty -- this is NOT a radio problem. Check networking, then \
                 `timedatectl status`"
            ),
        );
    } else {
        report.add(
            Level::Pass,
            "clock.wall",
            format!("{now} epoch seconds, plausible"),
        );
    }
}

fn check_filesystem(report: &mut Report, config: &Config) {
    let path = std::path::Path::new(config.db_path.as_str());
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    let probe_dir = dir.unwrap_or_else(|| std::path::Path::new("."));

    if !probe_dir.exists() {
        report.add(
            Level::Fail,
            "storage.path",
            format!("{} does not exist", probe_dir.display()),
        );
        return;
    }

    // Actually write, rather than inspecting permissions and hoping.
    let probe = probe_dir.join(".skyward-write-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            report.add(
                Level::Pass,
                "storage.writable",
                format!("{} is writable", probe_dir.display()),
            );
        }
        Err(e) => report.add(
            Level::Fail,
            "storage.writable",
            format!("cannot write to {}: {e}", probe_dir.display()),
        ),
    }

    if path.exists() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        report.add(
            Level::Pass,
            "storage.database",
            format!("{} is {:.1} MB", path.display(), size as f64 / 1e6),
        );
    } else {
        report.add(
            Level::Pass,
            "storage.database",
            format!("{} will be created on first run", path.display()),
        );
    }
}

/// Prove the decode chain works with no antenna attached.
fn check_self_test(report: &mut Report, config: &Config) {
    let set = config.impls();
    let rate = *config.sample_rate_hz;

    let messages = synth::canonical_messages();
    let iq = synth::synthesize(
        &messages,
        &synth::SynthConfig {
            sample_rate: rate,
            ..Default::default()
        },
    );

    let mut pipeline = match registry::build(&set, rate) {
        Ok(p) => p,
        Err(e) => {
            report.add(Level::Fail, "selftest.build", e.to_string());
            return;
        }
    };

    let started = Instant::now();
    let found = adsb_dsp::pipeline::decode_all(&mut pipeline, &iq);
    let elapsed = started.elapsed();

    let expected: Vec<String> = messages
        .iter()
        .map(|m| adsb_core::bytes_to_hex(m))
        .collect();
    let got: Vec<String> = found.iter().map(|v| v.hex()).collect();

    if got == expected {
        report.add(
            Level::Pass,
            "selftest.decode",
            format!(
                "{}/{} synthetic frames recovered in {:.1} ms -- the decode chain is correct \
                 on this CPU",
                got.len(),
                expected.len(),
                elapsed.as_secs_f64() * 1000.0
            ),
        );
    } else {
        report.add(
            Level::Fail,
            "selftest.decode",
            format!(
                "recovered {}/{} synthetic frames. The build itself is wrong -- a bad \
                 cross-compile or a corrupt binary, NOT an antenna problem. Expected {expected:?}, \
                 got {got:?}",
                got.len(),
                expected.len()
            ),
        );
    }
}

/// Tune the radio and look at what comes back.
fn check_source(report: &mut Report, config: &Config, seconds: u64) {
    let spec = match SourceSpec::parse(&config.source) {
        Ok(s) => s,
        Err(e) => {
            report.add(Level::Fail, "radio.source", e.to_string());
            return;
        }
    };

    let options = SourceOptions {
        sample_rate: *config.sample_rate_hz,
        pace: adsb_source::Pace::Fast,
        ..Default::default()
    };

    let mut source = match adsb_source::open(&spec, &options) {
        Ok(s) => s,
        Err(e) => {
            // Transient or not, doctor cannot measure a radio it cannot open,
            // so this is a failure either way.
            report.add(
                Level::Fail,
                "radio.open",
                format!("{e}. If this is tcp:, is rtl_tcp running? `systemctl status rtl_tcp`"),
            );
            return;
        }
    };
    report.add(Level::Pass, "radio.open", source.describe());

    if let Err(e) = source.set_frequency(*config.frequency_hz) {
        report.add(Level::Warn, "radio.tune", e.to_string());
    }
    match adsb_source::Gain::parse(&config.gain_db) {
        Ok(gain) => {
            if let Err(e) = source.set_gain(gain) {
                report.add(Level::Warn, "radio.gain", e.to_string());
            } else {
                report.add(Level::Pass, "radio.gain", gain.describe());
            }
        }
        Err(e) => report.add(Level::Fail, "radio.gain", e.to_string()),
    }
    // rtl_sdr never does this, so captures made with it have the RTL2832's
    // digital AGC in an unknown state.
    let _ = source.set_agc(false);

    // Capture and measure.
    let rate = *config.sample_rate_hz;
    let want_samples = rate as u64 * seconds;
    let mut buf = vec![0u8; 256 * 1024 * 2];
    let mut samples = 0u64;
    let mut clipped = 0u64;
    let mut sum_i = 0i64;
    let mut sum_q = 0i64;
    let mut captured = Vec::with_capacity((want_samples * 2) as usize);

    let started = Instant::now();
    while samples < want_samples {
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for pair in buf[..n].chunks_exact(2) {
                    if pair[0] == 0 || pair[0] == 255 || pair[1] == 0 || pair[1] == 255 {
                        clipped += 1;
                    }
                    sum_i += i64::from(pair[0]) - 128;
                    sum_q += i64::from(pair[1]) - 128;
                }
                samples += (n / 2) as u64;
                captured.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.is_end_of_stream() => break,
            Err(e) => {
                report.add(Level::Fail, "radio.read", e.to_string());
                return;
            }
        }
        if started.elapsed().as_secs() > seconds * 3 + 5 {
            break;
        }
    }

    if samples == 0 {
        report.add(Level::Fail, "radio.capture", "no samples arrived at all");
        return;
    }

    // Dropping samples and hearing nothing look identical in a message count.
    let elapsed = started.elapsed().as_secs_f64().max(1e-9);
    let effective = samples as f64 / elapsed;
    let ratio = effective / f64::from(rate);
    if matches!(spec, SourceSpec::File { .. }) {
        report.add(
            Level::Pass,
            "radio.rate",
            format!("{:.3} MS/s from a file (pacing disabled)", effective / 1e6),
        );
    } else if ratio < 0.99 {
        report.add(
            Level::Fail,
            "radio.rate",
            format!(
                "effective rate {:.3} MS/s against a configured {:.3} MS/s ({:.1}%). \
                 Samples are being DROPPED -- the CPU cannot keep up, or rtl_tcp's buffers \
                 are overflowing. This looks exactly like bad reception but is a software \
                 problem",
                effective / 1e6,
                f64::from(rate) / 1e6,
                ratio * 100.0
            ),
        );
    } else {
        report.add(
            Level::Pass,
            "radio.rate",
            format!(
                "{:.3} MS/s effective, matching the request",
                effective / 1e6
            ),
        );
    }

    let clip_pct = clipped as f64 / samples as f64 * 100.0;
    let dc_i = sum_i as f64 / samples as f64;
    let dc_q = sum_q as f64 / samples as f64;

    if clip_pct > 1.0 {
        report.add(
            Level::Warn,
            "radio.level",
            format!(
                "{clip_pct:.2}% of samples are at the rails. Gain is too high: the tuner is \
                 in compression and strong aircraft are being distorted. Try about 44 dB"
            ),
        );
    }
    if dc_i.abs() > 20.0 || dc_q.abs() > 20.0 {
        report.add(
            Level::Warn,
            "radio.dc",
            format!(
                "large DC offset (I {dc_i:+.1}, Q {dc_q:+.1}). Direct sampling may be enabled, \
                 which would mean the tuner is bypassed entirely"
            ),
        );
    }

    // Now decode what we captured.
    let set = config.impls();
    let Ok(mut pipeline) = registry::build(&set, rate) else {
        return;
    };
    let _found = adsb_dsp::pipeline::decode_all(&mut pipeline, &captured);
    let stats = pipeline.stats();

    let detail = format!(
        "{} candidates, {} valid in {:.1} s ({:.0} msg/min), clip {clip_pct:.3}%",
        stats.candidates,
        stats.crc_ok,
        samples as f64 / f64::from(rate),
        stats.crc_ok as f64 * 60.0 / (samples as f64 / f64::from(rate)).max(1e-9)
    );

    if stats.crc_ok > 0 {
        report.add(Level::Pass, "radio.decode", detail);
    } else if stats.candidates > 0 {
        report.add(
            Level::Fail,
            "radio.decode",
            format!(
                "{detail}. Preambles are being found but nothing passes CRC -- that is the \
                 signature of a SAMPLE RATE MISMATCH, not weak signal. Check that the \
                 configured rate matches what the dongle is actually running"
            ),
        );
    } else if clip_pct > 1.0 {
        report.add(
            Level::Warn,
            "radio.decode",
            format!("{detail}. Nothing decoded, but the front end is overloaded -- fix gain first"),
        );
    } else {
        report.add(
            Level::Warn,
            "radio.decode",
            format!(
                "{detail}. No preambles at all. In order of likelihood: the antenna is not \
                 connected; it is the wrong antenna (1090 MHz wants ~6.9 cm per element); it \
                 is touching glass or metal, which detunes it badly; or it has no view of the \
                 sky. Two minutes of traffic is normal even indoors"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CliOverrides;

    #[test]
    fn the_self_test_passes_with_the_baseline_pipeline() {
        let config = Config::resolve(None, &CliOverrides::default()).unwrap();
        let mut report = Report::new();
        check_self_test(&mut report, &config);
        let (level, name, detail) = &report.checks[0];
        assert_eq!(*level, Level::Pass, "{name}: {detail}");
        assert!(detail.contains("5/5"), "{detail}");
    }

    #[test]
    fn the_clock_check_accepts_the_present() {
        let mut report = Report::new();
        check_clock(&mut report);
        assert_eq!(report.checks[0].0, Level::Pass);
    }

    #[test]
    fn an_unset_receiver_position_warns_but_does_not_fail() {
        let config = Config::resolve(None, &CliOverrides::default()).unwrap();
        let mut report = Report::new();
        check_config(&mut report, &config);
        assert_eq!(report.worst(), Level::Warn);
        assert!(
            report
                .checks
                .iter()
                .any(|(_, name, _)| name == "config.receiver")
        );
    }

    #[test]
    fn provenance_appears_in_the_report() {
        let config = Config::resolve(None, &CliOverrides::default()).unwrap();
        let mut report = Report::new();
        check_config(&mut report, &config);
        let dump: String = report
            .checks
            .iter()
            .map(|(_, n, d)| format!("{n} {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(dump.contains("config.sample_rate_hz"), "{dump}");
        assert!(dump.contains("default"), "{dump}");
    }

    #[test]
    fn json_output_carries_a_status() {
        let mut report = Report::new();
        report.add(Level::Pass, "a.b", "fine");
        let json = report.to_json();
        assert_eq!(json["status"], "ok");
        report.add(Level::Fail, "c.d", "broken");
        assert_eq!(report.to_json()["status"], "fail");
    }

    #[test]
    fn levels_order_so_the_worst_wins() {
        assert!(Level::Fail > Level::Warn);
        assert!(Level::Warn > Level::Pass);
    }
}
