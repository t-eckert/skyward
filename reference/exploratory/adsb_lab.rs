// adsb_lab.rs — offline ADS-B lab bench.
//
// Reads a raw rtl_sdr u8 IQ file, demodulates Mode S, and reports what it found.
// No dependencies: rustc -O adsb_lab.rs
//
// usage: ./adsb_lab <file.iq> <sample_rate_hz>

use std::env;
use std::fs::File;
use std::io::Read;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: adsb_lab <file.iq> <sample_rate_hz>");
        std::process::exit(1);
    }
    let path = &args[1];
    let fs: f64 = args[2].parse().expect("sample rate");
    let spus = fs / 1e6; // samples per microsecond

    let mut raw = Vec::new();
    File::open(path).expect("open").read_to_end(&mut raw).expect("read");
    let nsamp = raw.len() / 2;
    println!("file: {path}");
    println!("  {} bytes, {} IQ samples, {:.2} s at {:.3} MS/s",
             raw.len(), nsamp, nsamp as f64 / fs, fs / 1e6);

    // ---- Stage 1: magnitude (AM envelope) -------------------------------
    // rtl_sdr gives offset-binary u8: 0..255 with 127.5 as zero.
    let mut mag = vec![0f32; nsamp];
    let (mut sum_i, mut sum_q) = (0f64, 0f64);
    let mut clipped = 0usize;
    for k in 0..nsamp {
        let i = raw[2 * k] as f32 - 127.5;
        let q = raw[2 * k + 1] as f32 - 127.5;
        sum_i += i as f64;
        sum_q += q as f64;
        if raw[2 * k] == 0 || raw[2 * k] == 255 || raw[2 * k + 1] == 0 || raw[2 * k + 1] == 255 {
            clipped += 1;
        }
        mag[k] = (i * i + q * q).sqrt();
    }
    let mean_mag: f64 = mag.iter().map(|&m| m as f64).sum::<f64>() / nsamp as f64;
    let mut sorted: Vec<f32> = mag.iter().step_by(97).cloned().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    println!("\nsignal stats");
    println!("  DC offset      I={:+.2}  Q={:+.2}   (want near 0)", sum_i / nsamp as f64, sum_q / nsamp as f64);
    println!("  clipped samps  {} ({:.4}%)   (want ~0; if high, drop gain)",
             clipped, 100.0 * clipped as f64 / nsamp as f64);
    println!("  mag  mean={:.1}  p50={:.1}  p99={:.1}  p99.99={:.1}  max={:.1}",
             mean_mag, pct(0.50), pct(0.99), pct(0.9999), sorted[sorted.len() - 1]);
    println!("  noise floor ~= p50; bursts show up in the top percentiles");

    // ---- Stage 2: preamble search + bit slicing -------------------------
    // Mode S preamble, 8 us: pulses centred at 0.0, 1.0, 3.5, 4.5 us,
    // silence everywhere else. Then 112 (or 56) us of PPM data:
    // energy in the first half of a bit period = 1, second half = 0.
    let mut stats = Stats::default();
    let mut msgs: Vec<Msg> = Vec::new();

    // Mean magnitude over the half-bit slot [start, start+w) microseconds
    // past sample `at`. Mode S pulses are 0.5 us long and *begin* at the
    // nominal time, so slots are start-anchored, not centre-anchored.
    let win = |start: f64, w: f64, at: usize| -> f32 {
        let a = (start * spus).round() as isize;
        let b = ((start + w) * spus).round() as isize;
        let mut s = 0f32;
        let mut n = 0;
        for j in a..b.max(a + 1) {
            let idx = at as isize + j;
            if idx >= 0 && (idx as usize) < mag.len() {
                s += mag[idx as usize];
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { s / n as f32 }
    };

    let msg_len_us = 8.0 + 112.0;
    let need = (msg_len_us * spus).ceil() as usize + 4;
    // Detection threshold, overridable so we can probe how much signal the
    // naive settings are leaving on the table.
    let snr_mult: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let ratio: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let mean_mode = args.get(5).map(|s| s == "mean").unwrap_or(false);
    let noise = pct(0.50).max(1.0);
    let floor = noise * snr_mult; // a preamble pulse must clear this to be tested

    let mut i = 0usize;
    while i + need < nsamp {
        // Cheap reject: the first preamble pulse has to be above the noise floor.
        // Skips ~all of the buffer without computing the full 16-window template.
        if win(0.0, 0.5, i) <= floor {
            i += 1;
            continue;
        }
        // Four preamble pulses, each 0.5 us long, starting at 0, 1, 3.5, 4.5 us.
        let p = [win(0.0, 0.5, i), win(1.0, 0.5, i), win(3.5, 0.5, i), win(4.5, 0.5, i)];
        // Every other half-microsecond slot in the 8 us preamble is silent.
        let s = [
            win(0.5, 0.5, i), win(1.5, 0.5, i), win(2.0, 0.5, i), win(2.5, 0.5, i),
            win(3.0, 0.5, i), win(4.0, 0.5, i), win(5.0, 0.5, i), win(5.5, 0.5, i),
            win(6.0, 0.5, i), win(6.5, 0.5, i), win(7.0, 0.5, i), win(7.5, 0.5, i),
        ];
        let pmin = p.iter().cloned().fold(f32::INFINITY, f32::min);
        let smax = s.iter().cloned().fold(0f32, f32::max);
        let pmean = p.iter().sum::<f32>() / p.len() as f32;
        let smean = s.iter().sum::<f32>() / s.len() as f32;

        // "mean" integrates across the whole preamble template instead of
        // letting a single noisy slot veto the match -- a cheap stand-in for
        // proper correlation, used to probe how much signal min/max discards.
        let accept = if mean_mode {
            pmean > smean.max(noise) * ratio && pmean > floor
        } else {
            pmin > smax * ratio && pmin > floor
        };
        if !accept {
            i += 1;
            continue;
        }
        stats.preambles += 1;

        // Slice 112 bits. Bit k occupies [8+k, 9+k) us; compare halves.
        let mut bits = [0u8; 112];
        let mut weak = 0u32; // bits where the two halves were nearly equal
        for k in 0..112 {
            let t = 8.0 + k as f64;
            let early = win(t, 0.5, i);
            let late = win(t + 0.5, 0.5, i);
            bits[k] = if early > late { 1 } else { 0 };
            if (early - late).abs() < 0.15 * (early + late).max(1.0) {
                weak += 1;
            }
        }

        let df = be(&bits, 0, 5) as u8;
        stats.df[df as usize] += 1;

        let long = matches!(df, 16 | 17 | 18 | 19 | 20 | 21 | 24);
        let nbits = if long { 112 } else { 56 };
        let rem = crc24(&bits[..nbits]);

        let ok = if df == 17 || df == 18 {
            rem == 0
        } else if df == 11 {
            // DF11: remainder is the interrogator id, usually 0.
            rem & 0xFFFF80 == 0
        } else {
            false // DF0/4/5/20/21 XOR the ICAO in; can't validate standalone
        };

        if ok {
            stats.crc_ok += 1;
            msgs.push(Msg { bits, nbits, df, weak, sample: i });
            i += need; // skip past this message
            continue;
        } else {
            stats.crc_bad += 1;
        }
        // Even on CRC failure, skip the message body: a preamble here means
        // the next 112 us are occupied, and re-triggering inside data is noise.
        i += (8.0 * spus) as usize;
    }

    let secs = nsamp as f64 / fs;
    println!("\ndemodulation");
    println!("  preambles found   {}", stats.preambles);
    println!("  CRC valid         {}", stats.crc_ok);
    println!("  CRC failed        {}", stats.crc_bad);
    if stats.preambles > 0 {
        println!("  yield             {:.1}%", 100.0 * stats.crc_ok as f64 / stats.preambles as f64);
    }
    println!("  >> RATE           {:.1} msg/min   over {:.0} s", stats.crc_ok as f64 * 60.0 / secs, secs);
    println!("\n  downlink formats seen at preamble (before CRC):");
    let mut dfs: Vec<(usize, u64)> = stats.df.iter().cloned().enumerate().filter(|&(_, c)| c > 0).collect();
    dfs.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    for (df, c) in dfs.iter().take(8) {
        println!("    DF{:<2} {:>7}   {}", df, c, df_name(*df as u8));
    }

    // ---- Stage 3: decode the valid ones ---------------------------------
    println!("\nvalid messages ({}):", msgs.len());
    for m in msgs.iter().take(60) {
        let hex: String = pack(&m.bits[..m.nbits]).iter().map(|b| format!("{b:02X}")).collect();
        let icao = be(&m.bits, 8, 24);
        print!("  t={:>7.3}s  {hex}  DF{:<2} ICAO {:06X}",
               m.sample as f64 / fs, m.df, icao);
        if m.df == 17 || m.df == 18 {
            let tc = be(&m.bits, 32, 5) as u8;
            print!("  TC{:<2} {}", tc, describe(&m.bits, tc));
        }
        if m.weak > 8 { print!("   [{} weak bits]", m.weak); }
        println!();
    }
    if msgs.len() > 60 {
        println!("  ... {} more", msgs.len() - 60);
    }

    // ---- Stage 4: CPR position decoding ---------------------------------
    // A position frame carries only 17 bits each of lat/lon -- not a global
    // position, but a coordinate *within a zone*. Two frame types exist, even
    // and odd, which lay down grids of 60 and 59 latitude zones respectively.
    // Because the grids differ slightly, the offset between the two readings
    // is unique to one zone, so an even+odd pair pins down the true position
    // with no prior knowledge. They must be close in time -- an aircraft that
    // moves too far between frames breaks the assumption.
    println!("\npositions (global CPR, even/odd pairs within 10 s):");
    let mut cpr: Vec<(u32, Option<Frame>, Option<Frame>)> = Vec::new();
    let mut fixes = 0;
    for m in &msgs {
        if m.df != 17 && m.df != 18 { continue }
        let tc = be(&m.bits, 32, 5) as u8;
        if !(9..=18).contains(&tc) && !(20..=22).contains(&tc) { continue }
        let icao = be(&m.bits, 8, 24);
        let f = Frame {
            lat: be(&m.bits, 54, 17) as f64 / 131072.0,
            lon: be(&m.bits, 71, 17) as f64 / 131072.0,
            t: m.sample as f64 / fs,
        };
        let odd = m.bits[53] == 1;
        let e = match cpr.iter().position(|(a, _, _)| *a == icao) {
            Some(k) => k,
            None => { cpr.push((icao, None, None)); cpr.len() - 1 }
        };
        if odd { cpr[e].2 = Some(f) } else { cpr[e].1 = Some(f) }

        if let (Some(ev), Some(od)) = (cpr[e].1, cpr[e].2) {
            if (ev.t - od.t).abs() <= 10.0 {
                if let Some((lat, lon)) = cpr_global(&ev, &od, odd) {
                    let alt = alt_ft(&m.bits);
                    println!("  t={:>7.3}s  {icao:06X}  {lat:>9.5} {lon:>10.5}   alt={}  range={:.1} km from Ottawa",
                             m.sample as f64 / fs,
                             alt.map(|a| format!("{a} ft")).unwrap_or("?".into()),
                             haversine(45.4215, -75.6972, lat, lon));
                    fixes += 1;
                }
            }
        }
    }
    if fixes == 0 { println!("  (none -- need an even and an odd frame from the same aircraft)") }

    // Per-aircraft summary
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for m in &msgs {
        let icao = be(&m.bits, 8, 24);
        match seen.iter_mut().find(|(a, _)| *a == icao) {
            Some((_, c)) => *c += 1,
            None => seen.push((icao, 1)),
        }
    }
    seen.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    println!("\naircraft seen: {}", seen.len());
    for (icao, c) in &seen {
        println!("  {:06X}  {} messages", icao, c);
    }
}

struct Msg { bits: [u8; 112], nbits: usize, df: u8, weak: u32, sample: usize }

#[derive(Clone, Copy)]
struct Frame { lat: f64, lon: f64, t: f64 }

/// Global CPR: recover absolute lat/lon from an even+odd frame pair.
/// `newest_odd` says which of the two arrived most recently -- the answer is
/// anchored to that frame, since it reflects where the aircraft is *now*.
fn cpr_global(ev: &Frame, od: &Frame, newest_odd: bool) -> Option<(f64, f64)> {
    const NZ: f64 = 15.0;
    let dlat_even = 360.0 / (4.0 * NZ);       // 6 degrees
    let dlat_odd = 360.0 / (4.0 * NZ - 1.0);  // 360/59 degrees

    // Latitude zone index: the one number that both grids agree on.
    let j = (59.0 * ev.lat - 60.0 * od.lat + 0.5).floor();

    let mut lat_even = dlat_even * (j.rem_euclid(60.0) + ev.lat);
    let mut lat_odd = dlat_odd * (j.rem_euclid(59.0) + od.lat);
    if lat_even >= 270.0 { lat_even -= 360.0 }
    if lat_odd >= 270.0 { lat_odd -= 360.0 }
    if !(-90.0..=90.0).contains(&lat_even) || !(-90.0..=90.0).contains(&lat_odd) {
        return None;
    }

    // Both frames must fall in the same longitude band, or the pair is unusable.
    let (nl_e, nl_o) = (nl(lat_even), nl(lat_odd));
    if nl_e != nl_o { return None }

    let nl_ = nl_e as f64;
    let m = (ev.lon * (nl_ - 1.0) - od.lon * nl_ + 0.5).floor();

    let (lat, ni, cpr_lon) = if newest_odd {
        (lat_odd, (nl_ - 1.0).max(1.0), od.lon)
    } else {
        (lat_even, nl_.max(1.0), ev.lon)
    };

    let mut lon = (360.0 / ni) * (m.rem_euclid(ni) + cpr_lon);
    if lon >= 180.0 { lon -= 360.0 }
    Some((lat, lon))
}

fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let (dp, dl) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

#[derive(Default)]
struct Stats { preambles: u64, crc_ok: u64, crc_bad: u64, df: [u64; 32] }

fn df_name(df: u8) -> &'static str {
    match df {
        0 => "short air-air surveillance (ACAS)",
        4 => "surveillance, altitude reply",
        5 => "surveillance, identity reply",
        11 => "all-call reply",
        16 => "long air-air surveillance (ACAS)",
        17 => "ADS-B extended squitter",
        18 => "extended squitter, non-transponder",
        19 => "military extended squitter",
        20 => "Comm-B, altitude reply",
        21 => "Comm-B, identity reply",
        24 => "Comm-D (ELM)",
        _ => "(unassigned / noise)",
    }
}

fn describe(bits: &[u8; 112], tc: u8) -> String {
    match tc {
        1..=4 => {
            const C: &[u8] = b"?ABCDEFGHIJKLMNOPQRSTUVWXYZ????? ???############0123456789######";
            let s: String = (0..8)
                .map(|i| C[be(bits, 40 + i * 6, 6) as usize % 64] as char)
                .collect();
            format!("identification  callsign={}", s.trim_end())
        }
        5..=8 => "surface position".into(),
        9..=18 | 20..=22 => {
            let odd = bits[53] == 1;
            let lat = be(bits, 54, 17);
            let lon = be(bits, 71, 17);
            let alt = alt_ft(bits);
            format!("airborne position  alt={}  cpr={} lat={} lon={}",
                    alt.map(|a| format!("{a} ft")).unwrap_or("?".into()),
                    if odd { "odd " } else { "even" }, lat, lon)
        }
        19 => {
            let st = be(bits, 37, 3);
            if st == 1 || st == 2 {
                let ewd = bits[45]; let ew = be(bits, 46, 10) as i32;
                let nsd = bits[56]; let ns = be(bits, 57, 10) as i32;
                let vew = if ewd == 1 { -(ew - 1) } else { ew - 1 };
                let vns = if nsd == 1 { -(ns - 1) } else { ns - 1 };
                let gs = (((vew * vew + vns * vns) as f64).sqrt()) as i32;
                let mut trk = (vew as f64).atan2(vns as f64).to_degrees();
                if trk < 0.0 { trk += 360.0 }
                let vrs = bits[68]; let vr = be(bits, 69, 9) as i32;
                let vrate = if vr == 0 { 0 } else { (vr - 1) * 64 * if vrs == 1 { -1 } else { 1 } };
                format!("velocity  gs={gs} kt  track={trk:.0}deg  vs={vrate:+} fpm")
            } else {
                format!("velocity subtype {st} (airspeed)")
            }
        }
        28 => "aircraft status".into(),
        29 => "target state and status".into(),
        31 => "operational status".into(),
        _ => format!("type code {tc}"),
    }
}

// 12-bit altitude field at bits 40..52, Q-bit at 47.
fn alt_ft(bits: &[u8; 112]) -> Option<i32> {
    let q = bits[47];
    if q == 1 {
        let n = ((be(bits, 40, 7) as i32) << 4) | be(bits, 48, 4) as i32;
        Some(n * 25 - 1000)
    } else {
        None // Gillham/Gray coded, 100 ft steps
    }
}

/// Number of longitude zones at a given latitude: 59 at the equator, 1 at the
/// poles. The closed form lands on exactly 60.0 at lat 0, which is off by one
/// against the ICAO table, so clamp it.
fn nl(lat: f64) -> u32 {
    if lat.abs() >= 87.0 { return 1 }
    let nz = 15.0f64;
    let a = 1.0 - (std::f64::consts::PI / (2.0 * nz)).cos();
    let b = lat.to_radians().cos().powi(2);
    let v = (2.0 * std::f64::consts::PI / (1.0 - a / b).acos()).floor() as u32;
    v.clamp(1, 59)
}

fn be(bits: &[u8], off: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for k in 0..n { v = (v << 1) | bits[off + k] as u32 }
    v
}

fn pack(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b)).collect()
}

// Mode S CRC-24, generator 0xFFF409 (x^24+x^23+x^10+x^3+1).
// The parity field is the last 24 bits, so running the whole message
// through leaves 0 for an untouched DF17.
fn crc24(bits: &[u8]) -> u32 {
    const POLY: u32 = 0xFFF409;
    let mut crc = 0u32;
    for &b in bits {
        let msb = (crc >> 23) & 1;
        crc = ((crc << 1) & 0xFFFFFF) | b as u32;
        if msb != 0 { crc ^= POLY }
    }
    crc & 0xFFFFFF
}
