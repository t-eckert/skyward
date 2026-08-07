//! Replaying a captured IQ file.
//!
//! The most important source in the project, because it is the one that makes
//! results reproducible. A benchmark against live radio is measuring the
//! weather.
//!
//! # The odd-byte trap
//!
//! `Read::read` may return fewer bytes than asked for, and nothing stops it
//! returning an *odd* number. If you treat that as a complete result, every
//! subsequent sample has I and Q swapped for the rest of the file. Magnitude
//! is `sqrt(i² + q²)`, so the swap is invisible in the signal statistics — it
//! just quietly decodes nothing, and looks exactly like a bad demodulator.
//!
//! [`FileSource`] carries the stray byte forward instead. There is a test that
//! reads through a deliberately hostile reader returning 1, 3 and 7 bytes at a
//! time and asserts the output is byte-identical to a clean read.

use crate::{IqSource, SourceError, SourceOptions};
use std::fs::File;
use std::io::{BufReader, Read};
use std::time::{Duration, Instant};

/// Whether replay should track wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Deliver samples at the rate they were recorded, so a replay behaves
    /// like a live receiver. Use for demos and for exercising the server.
    Realtime,
    /// Deliver as fast as the disk allows.
    ///
    /// **Benchmarks must use this.** A paced benchmark measures `thread::sleep`.
    Fast,
}

/// Reads interleaved `u8` IQ from anything implementing [`Read`].
pub struct FileSource<R: Read + Send = BufReader<File>> {
    inner: R,
    label: String,
    sample_rate: u32,
    pace: Pace,
    repeat: bool,
    /// Rewind support for `repeat`; `None` when the reader cannot be reopened.
    reopen: Option<Box<dyn Fn() -> Result<R, SourceError> + Send>>,

    /// A byte left over from a read that ended mid-sample.
    partial: Option<u8>,
    samples_delivered: u64,
    started: Option<Instant>,
    exhausted: bool,
}

impl FileSource<BufReader<File>> {
    pub fn open(path: &str, options: &SourceOptions) -> Result<Self, SourceError> {
        let owned = path.to_string();
        let make = move || -> Result<BufReader<File>, SourceError> {
            File::open(&owned)
                .map(|f| BufReader::with_capacity(1 << 20, f))
                .map_err(|e| SourceError::Config(format!("cannot open {owned}: {e}")))
        };
        let inner = make()?;
        Ok(FileSource {
            inner,
            label: format!("file:{path}"),
            sample_rate: options.sample_rate,
            pace: options.pace,
            repeat: options.repeat,
            reopen: Some(Box::new(make)),
            partial: None,
            samples_delivered: 0,
            started: None,
            exhausted: false,
        })
    }
}

impl<R: Read + Send> FileSource<R> {
    /// Wrap an arbitrary reader. Used by tests to inject hostile read patterns.
    pub fn from_reader(inner: R, sample_rate: u32, pace: Pace) -> Self {
        FileSource {
            inner,
            label: "file:<reader>".to_string(),
            sample_rate,
            pace,
            repeat: false,
            reopen: None,
            partial: None,
            samples_delivered: 0,
            started: None,
            exhausted: false,
        }
    }

    /// Total IQ samples handed out so far.
    pub fn samples_delivered(&self) -> u64 {
        self.samples_delivered
    }

    /// Sleep so that replay tracks the rate the file was recorded at.
    fn pace_to_realtime(&mut self) {
        if self.pace != Pace::Realtime {
            return;
        }
        let started = *self.started.get_or_insert_with(Instant::now);
        let owed = Duration::from_secs_f64(
            self.samples_delivered as f64 / f64::from(self.sample_rate.max(1)),
        );
        let elapsed = started.elapsed();
        if owed > elapsed {
            std::thread::sleep(owed - elapsed);
        }
    }
}

impl<R: Read + Send> IqSource for FileSource<R> {
    fn describe(&self) -> String {
        format!(
            "{} at {:.3} MS/s ({:?}{})",
            self.label,
            f64::from(self.sample_rate) / 1e6,
            self.pace,
            if self.repeat { ", looping" } else { "" }
        )
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SourceError> {
        if self.exhausted {
            return Err(SourceError::EndOfStream);
        }
        if buf.len() < 2 {
            return Ok(0);
        }

        // Re-seat any byte carried over from a read that ended mid-sample.
        let mut filled = 0usize;
        if let Some(byte) = self.partial.take() {
            buf[0] = byte;
            filled = 1;
        }

        // Keep reading until at least one whole sample is in hand. Returning
        // `Ok(0)` because a single byte arrived would be indistinguishable
        // from end of stream, and a reader that hands out one byte at a time
        // is perfectly legal.
        loop {
            match self.inner.read(&mut buf[filled..]) {
                Ok(0) => {
                    // End of file. Loop if asked, otherwise finish.
                    if self.repeat
                        && let Some(make) = &self.reopen
                    {
                        self.inner = make()?;
                        continue;
                    }
                    if filled >= 2 {
                        break;
                    }
                    // A single trailing byte cannot form a sample; drop it
                    // rather than emitting a half pair.
                    self.exhausted = true;
                    return Err(SourceError::EndOfStream);
                }
                Ok(n) => {
                    filled += n;
                    if filled >= 2 {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(SourceError::Transient(format!("read failed: {e}"))),
            }
        }

        // Never hand back a split I/Q pair.
        if !filled.is_multiple_of(2) {
            self.partial = Some(buf[filled - 1]);
            filled -= 1;
        }

        self.samples_delivered += (filled / 2) as u64;
        self.pace_to_realtime();
        Ok(filled)
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), SourceError> {
        // A file was recorded at whatever it was recorded at; this only tells
        // us how to interpret it.
        self.sample_rate = hz;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that returns awkward, short, odd-sized chunks.
    struct Dribble {
        data: Vec<u8>,
        pos: usize,
        pattern: Vec<usize>,
        step: usize,
    }

    impl Read for Dribble {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let want = self.pattern[self.step % self.pattern.len()];
            self.step += 1;
            let n = want.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn drain(src: &mut dyn IqSource, chunk: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; chunk];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    assert!(n.is_multiple_of(2), "returned an odd byte count: {n}");
                    out.extend_from_slice(&buf[..n]);
                }
                Err(e) if e.is_end_of_stream() => break,
                Err(e) => panic!("unexpected {e}"),
            }
        }
        out
    }

    fn ramp(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// The bug this module exists to not have.
    #[test]
    fn odd_length_reads_do_not_swap_i_and_q() {
        let data = ramp(1000);
        for pattern in [vec![1], vec![3], vec![7], vec![1, 3, 7, 2], vec![5, 1]] {
            let src = Dribble {
                data: data.clone(),
                pos: 0,
                pattern: pattern.clone(),
                step: 0,
            };
            let mut fs = FileSource::from_reader(src, 2_400_000, Pace::Fast);
            let got = drain(&mut fs, 64);
            assert_eq!(
                got, data,
                "pattern {pattern:?} corrupted the stream (I/Q swap)"
            );
        }
    }

    #[test]
    fn a_trailing_odd_byte_is_dropped_not_emitted() {
        // 1001 bytes cannot form 500.5 samples.
        let data = ramp(1001);
        let src = Dribble {
            data: data.clone(),
            pos: 0,
            pattern: vec![1001],
            step: 0,
        };
        let mut fs = FileSource::from_reader(src, 2_400_000, Pace::Fast);
        let got = drain(&mut fs, 4096);
        assert_eq!(got.len(), 1000);
        assert_eq!(got, data[..1000]);
    }

    #[test]
    fn every_read_returns_an_even_count() {
        let data = ramp(777);
        for chunk in [2usize, 3, 5, 8, 101] {
            let src = Dribble {
                data: data.clone(),
                pos: 0,
                pattern: vec![3, 1, 9],
                step: 0,
            };
            let mut fs = FileSource::from_reader(src, 2_400_000, Pace::Fast);
            // drain() asserts evenness on every read.
            let got = drain(&mut fs, chunk);
            assert_eq!(got.len(), 776, "chunk {chunk}");
        }
    }

    #[test]
    fn end_of_stream_is_reported_once_and_stays_reported() {
        let mut fs = FileSource::from_reader(
            Dribble {
                data: ramp(4),
                pos: 0,
                pattern: vec![4],
                step: 0,
            },
            2_400_000,
            Pace::Fast,
        );
        let mut buf = [0u8; 16];
        assert_eq!(fs.read(&mut buf).unwrap(), 4);
        assert!(fs.read(&mut buf).unwrap_err().is_end_of_stream());
        assert!(fs.read(&mut buf).unwrap_err().is_end_of_stream());
    }

    #[test]
    fn sample_count_tracks_bytes_delivered() {
        let mut fs = FileSource::from_reader(
            Dribble {
                data: ramp(200),
                pos: 0,
                pattern: vec![200],
                step: 0,
            },
            2_400_000,
            Pace::Fast,
        );
        drain(&mut fs, 4096);
        assert_eq!(fs.samples_delivered(), 100);
    }

    #[test]
    fn fast_pace_does_not_sleep() {
        // 2.4 million samples at 2.4 MS/s is one second of audio; if pacing
        // leaked into Fast this test would take that long.
        let mut fs = FileSource::from_reader(
            Dribble {
                data: vec![7u8; 4_800_000],
                pos: 0,
                pattern: vec![1 << 16],
                step: 0,
            },
            2_400_000,
            Pace::Fast,
        );
        let started = Instant::now();
        drain(&mut fs, 1 << 16);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "Fast pace slept for {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn realtime_pace_actually_paces() {
        // 24,000 samples at 2.4 MS/s is 10 ms.
        let mut fs = FileSource::from_reader(
            Dribble {
                data: vec![7u8; 48_000],
                pos: 0,
                pattern: vec![48_000],
                step: 0,
            },
            2_400_000,
            Pace::Realtime,
        );
        let started = Instant::now();
        drain(&mut fs, 48_000);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(8),
            "expected ~10 ms of pacing, got {elapsed:?}"
        );
    }

    #[test]
    fn missing_files_are_a_config_error_not_a_transient_one() {
        let err = FileSource::open("/nonexistent/nope.cu8", &SourceOptions::default())
            .err()
            .expect("should fail");
        assert!(
            !err.is_transient(),
            "a missing file must not send the server into a retry loop"
        );
    }

    #[test]
    fn a_real_file_round_trips() {
        let dir = std::env::temp_dir().join("skyward-file-source-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.cu8");
        let data = ramp(2048);
        std::fs::write(&path, &data).unwrap();

        let mut fs = FileSource::open(
            path.to_str().unwrap(),
            &SourceOptions::for_benchmark(2_400_000),
        )
        .unwrap();
        let got = drain(&mut fs, 300);
        assert_eq!(got, data);
        std::fs::remove_file(&path).ok();
    }
}
