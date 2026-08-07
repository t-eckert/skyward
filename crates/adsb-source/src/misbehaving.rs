//! A source decorator that breaks things on purpose.
//!
//! File replay is deterministic and well-behaved, which is exactly why it
//! cannot test the code paths that matter most on a machine you cannot log
//! into. Disconnects, stalls, truncated reads, clipping, sample gaps — those
//! only happen on real hardware, at inconvenient times, and are the failures
//! most likely to leave the Pi quietly dead.
//!
//! Wrapping any source in [`MisbehavingSource`] lets you provoke them on the
//! Mac, where you can actually debug.
//!
//! ```no_run
//! # use adsb_source::{FileSource, MisbehavingSource, Fault, SourceOptions};
//! let inner = FileSource::open("fixtures/raw/golden.cu8", &SourceOptions::default()).unwrap();
//! // Drop the connection every 50 reads and clip 2% of samples.
//! let mut src = MisbehavingSource::new(Box::new(inner))
//!     .with(Fault::DisconnectEvery(50))
//!     .with(Fault::ClipPercent(2.0));
//! ```

use crate::{Gain, IqSource, SourceError};

/// A misbehaviour to inject.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fault {
    /// Return a transient error every N reads, as a dropped `rtl_tcp` would.
    DisconnectEvery(u64),
    /// Return only a fraction of the requested bytes.
    ShortReads { fraction: f64 },
    /// Occasionally return an *odd* byte count, the I/Q swap trap.
    ///
    /// A correct consumer must survive this. `FileSource` handles it; this
    /// exists to prove the rest of the stack does too.
    OddReadEvery(u64),
    /// Drive a percentage of samples to the rails, as tuner overload does.
    ClipPercent(f64),
    /// Silently discard a run of samples, as a USB buffer overflow does.
    /// This is what "dropping samples" looks like from the receiver's side,
    /// and it is indistinguishable from poor reception in the message count
    /// alone — which is why `doctor` measures the effective sample rate.
    GapEvery { reads: u64, samples: usize },
    /// Stall for a while, as a busy or throttled Pi would.
    StallEvery { reads: u64, duration_ms: u64 },
}

/// Wraps another source and injects faults.
pub struct MisbehavingSource {
    inner: Box<dyn IqSource>,
    faults: Vec<Fault>,
    reads: u64,
    rng: u64,
}

impl MisbehavingSource {
    pub fn new(inner: Box<dyn IqSource>) -> Self {
        MisbehavingSource {
            inner,
            faults: Vec::new(),
            reads: 0,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    #[must_use]
    pub fn with(mut self, fault: Fault) -> Self {
        self.faults.push(fault);
        self
    }

    pub fn reads(&self) -> u64 {
        self.reads
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn fires(&self, period: u64) -> bool {
        period > 0 && self.reads.is_multiple_of(period)
    }
}

impl IqSource for MisbehavingSource {
    fn describe(&self) -> String {
        format!("{} [faults: {:?}]", self.inner.describe(), self.faults)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SourceError> {
        self.reads += 1;

        // Faults that pre-empt the read entirely.
        for fault in self.faults.clone() {
            match fault {
                Fault::DisconnectEvery(n) if self.fires(n) => {
                    return Err(SourceError::Transient(
                        "injected fault: connection dropped".into(),
                    ));
                }
                Fault::StallEvery { reads, duration_ms } if self.fires(reads) => {
                    std::thread::sleep(std::time::Duration::from_millis(duration_ms));
                }
                Fault::GapEvery { reads, samples } if self.fires(reads) => {
                    // Consume and discard, so the samples really are gone.
                    let mut sink = vec![0u8; samples * 2];
                    let _ = self.inner.read(&mut sink);
                }
                _ => {}
            }
        }

        // Shrink the request before passing it down.
        let mut want = buf.len();
        for fault in &self.faults {
            if let Fault::ShortReads { fraction } = fault {
                want = ((want as f64) * fraction).max(2.0) as usize;
            }
        }
        want = want.min(buf.len()) & !1;
        if want < 2 {
            want = 2.min(buf.len());
        }

        let mut n = self.inner.read(&mut buf[..want])?;

        // Faults that corrupt what came back.
        for fault in self.faults.clone() {
            match fault {
                Fault::OddReadEvery(period) if self.fires(period) && n >= 2 => {
                    n -= 1;
                }
                Fault::ClipPercent(pct) if pct > 0.0 => {
                    let count = ((n as f64) * pct / 100.0) as usize;
                    for _ in 0..count {
                        let index = (self.next_rand() as usize) % n.max(1);
                        buf[index] = if index.is_multiple_of(2) { 255 } else { 0 };
                    }
                }
                _ => {}
            }
        }

        Ok(n)
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn set_frequency(&mut self, hz: u32) -> Result<(), SourceError> {
        self.inner.set_frequency(hz)
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), SourceError> {
        self.inner.set_sample_rate(hz)
    }

    fn set_gain(&mut self, gain: Gain) -> Result<(), SourceError> {
        self.inner.set_gain(gain)
    }

    fn set_agc(&mut self, enabled: bool) -> Result<(), SourceError> {
        self.inner.set_agc(enabled)
    }

    fn gain_table(&self) -> Vec<i32> {
        self.inner.gain_table()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{FileSource, Pace};
    use std::io::Cursor;

    fn source(bytes: usize) -> Box<dyn IqSource> {
        let data: Vec<u8> = (0..bytes).map(|i| (i % 251) as u8).collect();
        Box::new(FileSource::from_reader(
            Cursor::new(data),
            2_400_000,
            Pace::Fast,
        ))
    }

    #[test]
    fn disconnects_are_transient_so_the_server_retries() {
        let mut src = MisbehavingSource::new(source(10_000)).with(Fault::DisconnectEvery(1));
        let err = src.read(&mut [0u8; 512]).expect_err("should fail");
        assert!(
            err.is_transient(),
            "injected disconnect must be recoverable"
        );
    }

    #[test]
    fn short_reads_still_return_even_counts() {
        let mut src =
            MisbehavingSource::new(source(10_000)).with(Fault::ShortReads { fraction: 0.13 });
        let mut buf = [0u8; 1024];
        for _ in 0..10 {
            match src.read(&mut buf) {
                Ok(n) => assert!(n.is_multiple_of(2), "odd count {n} from a short read"),
                Err(e) if e.is_end_of_stream() => break,
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn odd_reads_can_be_injected_for_downstream_testing() {
        let mut src = MisbehavingSource::new(source(10_000)).with(Fault::OddReadEvery(1));
        let n = src.read(&mut [0u8; 512]).unwrap();
        assert!(
            !n.is_multiple_of(2),
            "expected an injected odd count, got {n}"
        );
    }

    #[test]
    fn clipping_drives_samples_to_the_rails() {
        let mut src = MisbehavingSource::new(source(10_000)).with(Fault::ClipPercent(50.0));
        let mut buf = [0u8; 1024];
        let n = src.read(&mut buf).unwrap();
        let railed = buf[..n].iter().filter(|&&b| b == 0 || b == 255).count();
        assert!(railed > 100, "only {railed} of {n} samples were clipped");
    }

    #[test]
    fn gaps_actually_consume_samples() {
        let total = 10_000;
        let mut plain = MisbehavingSource::new(source(total));
        let mut gappy = MisbehavingSource::new(source(total)).with(Fault::GapEvery {
            reads: 1,
            samples: 100,
        });

        let drain = |src: &mut MisbehavingSource| {
            let mut buf = [0u8; 512];
            let mut got = 0;
            loop {
                match src.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(_) => break,
                }
            }
            got
        };

        let with_gaps = drain(&mut gappy);
        let without = drain(&mut plain);
        assert!(
            with_gaps < without,
            "gaps delivered {with_gaps} bytes, same or more than the clean {without}"
        );
    }

    #[test]
    fn a_clean_wrapper_is_transparent() {
        let mut wrapped = MisbehavingSource::new(source(4096));
        let mut plain = source(4096);
        let mut a = [0u8; 512];
        let mut b = [0u8; 512];
        let na = wrapped.read(&mut a).unwrap();
        let nb = plain.read(&mut b).unwrap();
        assert_eq!(na, nb);
        assert_eq!(a[..na], b[..nb]);
    }
}
