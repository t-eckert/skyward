//! `skyward` — one binary, several subcommands.
//!
//! Deliberately one binary rather than the decoder/API pair the previous
//! implementation used. Two processes means two ways to deploy the wrong
//! version, two units to supervise, two places for a config mismatch to hide,
//! and — worst — a health endpoint that cheerfully reports `ok` while the
//! decoder has been dead for hours.

mod api;
mod bench;
mod config;
mod doctor;
mod run;
mod web;

use clap::{Parser, Subcommand};
use config::{CliOverrides, Config};

#[derive(Parser)]
#[command(
    name = "skyward",
    version,
    about = "An ADS-B receiver you can run on a Raspberry Pi",
    long_about = None
)]
struct Cli {
    /// Configuration file. Values here are overridden by SKYWARD_* environment
    /// variables, which are overridden by command-line flags.
    #[arg(long, global = true)]
    config: Option<String>,

    /// Where samples come from: file:PATH, tcp:HOST:PORT, or usb[:INDEX].
    #[arg(long, global = true)]
    source: Option<String>,

    #[arg(long, global = true)]
    sample_rate_hz: Option<u32>,

    /// Tuner gain in dB, or "auto". Must be a value the tuner actually offers.
    #[arg(long, global = true)]
    gain_db: Option<String>,

    #[arg(long, global = true)]
    db_path: Option<String>,

    #[arg(long, global = true)]
    bind: Option<String>,

    /// `text` for a terminal, `json` for journald.
    #[arg(long, global = true)]
    log_format: Option<String>,

    /// Named pipeline preset. `--list-impls` shows what exists.
    #[arg(long, global = true)]
    impl_set: Option<String>,

    // Per-stage overrides, layered on top of `--impl-set`. The point of the
    // registry is that a new implementation lands beside the old one, and the
    // comparison you usually want is against your own previous attempt --
    // `--detect correlator-v3` against `--detect correlator-v2` -- which
    // should not require defining a preset for every experiment.
    /// Magnitude implementation. Overrides the preset's choice.
    #[arg(long = "mag", global = true, value_name = "NAME")]
    magnitude: Option<String>,

    /// Preamble detector. Overrides the preset's choice.
    #[arg(long = "detect", global = true, value_name = "NAME")]
    detector: Option<String>,

    /// Bit slicer. Overrides the preset's choice.
    #[arg(long = "slice", global = true, value_name = "NAME")]
    slicer: Option<String>,

    /// Frame validator. Overrides the preset's choice.
    #[arg(long = "validate", global = true, value_name = "NAME")]
    validator: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode continuously and serve the API.
    Run(run::RunArgs),

    /// Check everything that could be wrong, and say so in plain language.
    ///
    /// Exit code 0 means healthy, 1 means warnings, 2 means something is
    /// broken. Designed to be the first thing you run on the Pi.
    Doctor(doctor::DoctorArgs),

    /// Score one or more fixtures and write a run record.
    Bench(bench::BenchArgs),

    /// Print the resolved configuration and where each value came from.
    Config,

    /// List every registered pipeline implementation.
    ListImpls,

    /// Decode Mode S messages given as hex. Useful for poking at a capture.
    Decode {
        /// One or more messages in hex.
        messages: Vec<String>,
    },
}

fn main() -> std::process::ExitCode {
    // Before anything else, and specifically before any thread exists: this
    // mutates the process environment, which is not thread-safe on Unix.
    config::load_dotenv();

    let cli = Cli::parse();

    let overrides = CliOverrides {
        source: cli.source.clone(),
        sample_rate_hz: cli.sample_rate_hz,
        gain_db: cli.gain_db.clone(),
        bind: cli.bind.clone(),
        db_path: cli.db_path.clone(),
        log_format: cli.log_format.clone(),
        impl_set: cli.impl_set.clone(),
        magnitude: cli.magnitude.clone(),
        detector: cli.detector.clone(),
        slicer: cli.slicer.clone(),
        validator: cli.validator.clone(),
    };

    // Commands that need no configuration at all run before it is resolved, so
    // a broken config file cannot stop you from inspecting the build.
    match &cli.command {
        Command::ListImpls => {
            print!("{}", adsb_dsp::registry::describe_all());
            return std::process::ExitCode::SUCCESS;
        }
        Command::Decode { messages } => {
            return decode_command(messages);
        }
        _ => {}
    }

    let config = match Config::resolve(cli.config.as_deref(), &overrides) {
        Ok(c) => c,
        Err(e) => {
            // A configuration error is deliberate and unrecoverable: fail
            // loudly at startup rather than retrying into a confusing state.
            eprintln!("skyward: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    match cli.command {
        Command::Config => {
            println!("skyward configuration");
            if let Some(path) = &config.config_path {
                println!("  (file: {path})");
            } else {
                println!("  (no config file; defaults, environment and flags only)");
            }
            if let Some(path) = &config.env_file {
                println!("  (.env: {path})");
            }
            println!();
            print!("{}", config.print_resolved());
            std::process::ExitCode::SUCCESS
        }
        Command::Doctor(args) => doctor::run(&config, &args),
        Command::Bench(args) => bench::run(&config, &args),
        Command::Run(args) => run::run(config, args),
        Command::ListImpls | Command::Decode { .. } => unreachable!("handled above"),
    }
}

/// Decode hex messages from the command line.
fn decode_command(messages: &[String]) -> std::process::ExitCode {
    if messages.is_empty() {
        eprintln!("usage: skyward decode <HEX> [HEX...]");
        return std::process::ExitCode::from(2);
    }

    let mut failures = 0;
    for hex in messages {
        let Some(bytes) = adsb_core::hex_to_bytes(hex) else {
            println!("{hex}: not valid hex");
            failures += 1;
            continue;
        };
        let frame = match adsb_core::Frame::new(&bytes) {
            Ok(f) => f,
            Err(e) => {
                println!("{hex}: {e}");
                failures += 1;
                continue;
            }
        };

        println!("{hex}");
        println!("  DF{:<2}  {}", frame.df(), frame.format().describe());
        println!("  ICAO  {}", frame.icao());
        println!(
            "  CRC   {}",
            if frame.crc_ok() {
                "valid".to_string()
            } else if adsb_core::crc::remainder_is_address(frame.df()) {
                "cannot be checked (address is XORed into the parity)".to_string()
            } else {
                format!("FAILED (remainder {:#08X})", adsb_core::crc::crc24(&bytes))
            }
        );
        if let Some(tc) = frame.type_code() {
            println!("  TC{tc:<2}");
        }
        match frame.decode() {
            Some(message) => println!("  {message:?}"),
            None => println!("  (no ADS-B payload in this downlink format)"),
        }
        println!();
    }

    if failures > 0 {
        std::process::ExitCode::from(1)
    } else {
        std::process::ExitCode::SUCCESS
    }
}
