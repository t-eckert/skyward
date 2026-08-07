//! Run a raw IQ capture through the pipeline and report what came out.
//!
//! A stepping stone to `skyward bench`, and useful on its own for a quick
//! answer to "did that change help?".
//!
//!     cargo run --release --example score -- fixtures/raw/golden.cu8 2400000
//!
//! Always `--release`. The naive magnitude stage in a debug build is roughly
//! forty times slower, which is enough to make you optimize the wrong thing.

use adsb_dsp::registry;
use std::collections::BTreeMap;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: score <file.cu8> [sample_rate_hz] [--detect NAME] [--slice NAME]");
        std::process::exit(2);
    }
    let path = &args[1];
    let rate: u32 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_400_000);

    let mut set = registry::ImplSet::baseline();
    let mut i = 3;
    while i + 1 < args.len() {
        match args[i].as_str() {
            "--mag" => set.magnitude = args[i + 1].clone(),
            "--detect" => set.detector = args[i + 1].clone(),
            "--slice" => set.slicer = args[i + 1].clone(),
            "--validate" => set.validator = args[i + 1].clone(),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }

    let iq = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1);
    });

    let mut pipe = registry::build(&set, rate).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    });

    let seconds = (iq.len() / 2) as f64 / f64::from(rate);
    println!("{path}");
    println!("  {:.1} s at {:.3} MS/s", seconds, f64::from(rate) / 1e6);
    println!("  {set}");

    // Feed in realistic chunks rather than one slab, so this exercises the
    // same carry-over path the live receiver uses.
    let started = Instant::now();
    let mut found = Vec::new();
    for chunk in iq.chunks(256 * 1024 * 2) {
        pipe.feed(chunk, &mut found);
    }
    pipe.finish(&mut found);
    let elapsed = started.elapsed().as_secs_f64();

    let stats = pipe.stats();
    println!("\ndemodulation");
    println!("  candidates        {}", stats.candidates);
    println!("  CRC valid         {}", stats.crc_ok);
    println!("  CRC failed        {}", stats.crc_fail);
    println!("  slices failed     {}", stats.slices_failed);
    println!("  suppressed        {}", stats.suppressed_overlapping);
    println!(
        "  yield             {:.1}%  (report, never optimize)",
        stats.crc_yield() * 100.0
    );
    println!("  candidates/msg    {:.1}", stats.candidates_per_message());
    println!(
        "  realtime factor   {:.1}x  ({:.2} s wall, {:.0} ns/sample)",
        seconds / elapsed,
        elapsed,
        elapsed * 1e9 / (iq.len() / 2) as f64
    );

    println!("\n  downlink formats at candidate stage:");
    let mut dfs: Vec<(usize, u64)> = stats
        .by_df
        .iter()
        .copied()
        .enumerate()
        .filter(|&(_, n)| n > 0)
        .collect();
    dfs.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (df, n) in dfs.iter().take(8) {
        println!(
            "    DF{:<2} {:>8}   {}",
            df,
            n,
            adsb_core::DownlinkFormat::from_raw(*df as u8).describe()
        );
    }

    // Per-aircraft tally, and a count of type codes actually exercised.
    let mut aircraft: BTreeMap<String, u64> = BTreeMap::new();
    let mut type_codes: BTreeMap<u8, u64> = BTreeMap::new();
    for v in &found {
        if let Some(frame) = v.frame() {
            *aircraft.entry(frame.icao().to_string()).or_default() += 1;
            if let Some(tc) = frame.type_code() {
                *type_codes.entry(tc).or_default() += 1;
            }
        }
    }

    println!("\naircraft seen: {}", aircraft.len());
    let mut by_count: Vec<(&String, &u64)> = aircraft.iter().collect();
    by_count.sort_by_key(|&(_, n)| std::cmp::Reverse(*n));
    for (icao, n) in by_count.iter().take(20) {
        println!("  {icao}  {n} messages");
    }

    println!("\ntype codes: {type_codes:?}");
    println!(
        "\nmessages/min: {:.1}",
        stats.crc_ok as f64 * 60.0 / seconds.max(1e-9)
    );
}
