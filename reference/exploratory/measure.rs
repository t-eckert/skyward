// measure.rs — quick magnitude stats on a raw rtl_sdr u8 IQ file.
// rustc -O measure.rs -o measure ; ./measure file.iq [label]
use std::env;
use std::fs::File;
use std::io::Read;

fn main() {
    let a: Vec<String> = env::args().collect();
    let mut raw = Vec::new();
    File::open(&a[1]).unwrap().read_to_end(&mut raw).unwrap();
    let n = raw.len() / 2;
    let mut mag = Vec::with_capacity(n);
    let (mut si, mut sq) = (0f64, 0f64);
    let mut clip = 0usize;
    for k in 0..n {
        let i = raw[2 * k] as f32 - 127.5;
        let q = raw[2 * k + 1] as f32 - 127.5;
        si += i as f64; sq += q as f64;
        if raw[2*k] <= 1 || raw[2*k] >= 254 || raw[2*k+1] <= 1 || raw[2*k+1] >= 254 { clip += 1 }
        mag.push((i * i + q * q).sqrt());
    }
    mag.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let p = |f: f64| mag[((n - 1) as f64 * f) as usize];
    let label = a.get(2).cloned().unwrap_or_default();
    println!("{:<28} p50={:>6.1}  p99={:>6.1}  p99.99={:>6.1}  max={:>6.1}  dc=({:+.1},{:+.1})  clip={:.3}%",
             label, p(0.5), p(0.99), p(0.9999), mag[n - 1],
             si / n as f64, sq / n as f64, 100.0 * clip as f64 / n as f64);
}
