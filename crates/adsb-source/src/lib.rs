//! Where IQ samples come from.
//!
//! One trait, three implementations, and a decorator that breaks things on
//! purpose. The point of the abstraction is that the same binary develops
//! against a captured file on a laptop, demos from `rtl_tcp`, and deploys
//! against a dongle — switched by one string.
//!
//! # Bytes, not complex floats
//!
//! [`IqSource::read`] hands back interleaved unsigned 8-bit I,Q — exactly what
//! the RTL2832U produces. The obvious alternative, converting to
//! `Complex<f32>` at the source, turns 2 bytes into 8 before anything has
//! looked at them, doubles memory bandwidth on a Pi, and forecloses the
//! magnitude lookup table entirely. Conversion belongs in the stage that
//! consumes it.
//!
//! # Errors are a taxonomy, not a bag
//!
//! On a machine you cannot log into, *how* something failed decides what
//! should happen next, so the distinction is in the type:
//!
//! - [`SourceError::Config`] — wrong on purpose. Fail loudly at startup and
//!   never retry; retrying a configuration error just hides it.
//! - [`SourceError::Transient`] — `rtl_tcp` restarted, the USB bus glitched,
//!   the network blinked. **Never exit.** Reconnect, count it, carry on. A
//!   receiver that quietly died six hours ago is the worst outcome.
//! - [`SourceError::EndOfStream`] — a file ran out. Normal.

use std::time::Duration;

pub mod file;
pub mod misbehaving;
pub mod tcp;

pub use file::{FileSource, Pace};
pub use misbehaving::{Fault, MisbehavingSource};
pub use tcp::TcpSource;

/// A source of raw IQ samples.
pub trait IqSource: Send {
    /// Human-readable description, for logs and `doctor`.
    fn describe(&self) -> String;

    /// Fill as much of `buf` as possible with interleaved I,Q bytes.
    ///
    /// Returns the number of bytes written, always **even** — a source must
    /// never split an I/Q pair across two reads. Getting this wrong swaps I
    /// and Q for the entire remainder of the stream, which decodes as pure
    /// noise and looks exactly like a broken demodulator.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SourceError>;

    /// The sample rate actually in effect.
    ///
    /// Not necessarily the one you asked for. The RTL2832U derives its rate
    /// from a 28.8 MHz clock through a fractional divider: 2.4 MS/s is exact
    /// (28.8/12), 2.048 MS/s is not. Feeding the requested rate to the
    /// demodulator while the hardware runs at a different one is a leading
    /// cause of "plenty of preambles, zero valid CRCs".
    fn sample_rate(&self) -> u32;

    fn set_frequency(&mut self, _hz: u32) -> Result<(), SourceError> {
        Ok(())
    }

    fn set_sample_rate(&mut self, _hz: u32) -> Result<(), SourceError> {
        Ok(())
    }

    fn set_gain(&mut self, _gain: Gain) -> Result<(), SourceError> {
        Ok(())
    }

    /// Enable or disable the RTL2832U's *digital* AGC.
    ///
    /// Worth being explicit about: leaving it on lets the gain pump during a
    /// burst, which corrupts the amplitude relationship the whole demodulator
    /// depends on. `rtl_sdr` never turns it off, so the captures we took today
    /// have it in an unknown state.
    fn set_agc(&mut self, _enabled: bool) -> Result<(), SourceError> {
        Ok(())
    }

    fn set_bias_tee(&mut self, _enabled: bool) -> Result<(), SourceError> {
        Ok(())
    }

    /// Tuner gains available, in tenths of a dB. Empty when unknown.
    ///
    /// The R820T has 29 discrete steps, not a continuous dial. Asking for a
    /// value that is not on the list silently gets you a different one, so the
    /// config validator rejects unlisted values and prints this table.
    fn gain_table(&self) -> Vec<i32> {
        Vec::new()
    }
}

/// Tuner gain setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gain {
    /// Let the tuner decide. Note that `rtl_sdr -g 0` means *this*, not 0 dB —
    /// a genuinely confusing default that invalidates naive gain sweeps.
    Auto,
    /// Tenths of a dB. Must be a value from [`IqSource::gain_table`].
    Tenths(i32),
    /// Index into the tuner's gain table.
    Index(u32),
}

impl Gain {
    /// Parse a decibel string, or the word "auto".
    pub fn parse(s: &str) -> Result<Gain, SourceError> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(Gain::Auto);
        }
        let db: f64 = s
            .parse()
            .map_err(|_| SourceError::Config(format!("gain '{s}' is not a number or 'auto'")))?;
        Ok(Gain::Tenths((db * 10.0).round() as i32))
    }

    pub fn describe(self) -> String {
        match self {
            Gain::Auto => "auto".to_string(),
            Gain::Tenths(t) => format!("{:.1} dB", f64::from(t) / 10.0),
            Gain::Index(i) => format!("table index {i}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Deliberately unrecoverable: fail fast and loudly.
    #[error("configuration error: {0}")]
    Config(String),

    /// Recoverable: reconnect and keep going.
    #[error("transient I/O error: {0}")]
    Transient(String),

    #[error("end of stream")]
    EndOfStream,
}

impl SourceError {
    /// Whether the caller should retry rather than give up.
    pub fn is_transient(&self) -> bool {
        matches!(self, SourceError::Transient(_))
    }

    pub fn is_end_of_stream(&self) -> bool {
        matches!(self, SourceError::EndOfStream)
    }
}

/// Which source to open, parsed from a single string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSpec {
    File { path: String },
    Tcp { host: String, port: u16 },
    Usb { index: u32 },
}

impl SourceSpec {
    /// Parse `file:PATH`, `tcp:HOST:PORT`, or `usb[:INDEX]`.
    pub fn parse(s: &str) -> Result<SourceSpec, SourceError> {
        if let Some(path) = s.strip_prefix("file:") {
            if path.is_empty() {
                return Err(SourceError::Config("file: needs a path".into()));
            }
            return Ok(SourceSpec::File {
                path: path.to_string(),
            });
        }

        if let Some(rest) = s.strip_prefix("tcp:") {
            // rsplit so IPv6 literals with colons survive.
            let (host, port) = rest.rsplit_once(':').ok_or_else(|| {
                SourceError::Config(format!("'{s}' should look like tcp:HOST:PORT"))
            })?;
            let port: u16 = port
                .parse()
                .map_err(|_| SourceError::Config(format!("'{port}' is not a port number")))?;
            if host.is_empty() {
                return Err(SourceError::Config("tcp: needs a host".into()));
            }
            return Ok(SourceSpec::Tcp {
                host: host.to_string(),
                port,
            });
        }

        if s == "usb" {
            return Ok(SourceSpec::Usb { index: 0 });
        }
        if let Some(index) = s.strip_prefix("usb:") {
            let index = index
                .parse()
                .map_err(|_| SourceError::Config(format!("'{index}' is not a device index")))?;
            return Ok(SourceSpec::Usb { index });
        }

        Err(SourceError::Config(format!(
            "unknown source '{s}'. Use file:PATH, tcp:HOST:PORT, or usb[:INDEX]"
        )))
    }
}

/// Knobs that apply regardless of which source is opened.
#[derive(Clone, Debug)]
pub struct SourceOptions {
    /// Rate to request, and the rate a file is assumed to have been recorded at.
    pub sample_rate: u32,
    /// Whether a file should be paced to wall-clock time.
    pub pace: Pace,
    /// Whether a file should restart when it runs out.
    pub repeat: bool,
    /// How long to wait on a socket before declaring it wedged.
    ///
    /// The old implementation used a blocking `read_exact` with no timeout, so
    /// a hung `rtl_tcp` froze the process forever with no log line at all.
    pub read_timeout: Duration,
}

impl Default for SourceOptions {
    fn default() -> Self {
        SourceOptions {
            sample_rate: 2_400_000,
            pace: Pace::Realtime,
            repeat: false,
            read_timeout: Duration::from_secs(5),
        }
    }
}

impl SourceOptions {
    /// Benchmarks must never sleep, and must never loop.
    pub fn for_benchmark(sample_rate: u32) -> Self {
        SourceOptions {
            sample_rate,
            pace: Pace::Fast,
            repeat: false,
            ..Default::default()
        }
    }
}

/// Open a source.
pub fn open(spec: &SourceSpec, options: &SourceOptions) -> Result<Box<dyn IqSource>, SourceError> {
    match spec {
        SourceSpec::File { path } => Ok(Box::new(FileSource::open(path, options)?)),
        SourceSpec::Tcp { host, port } => Ok(Box::new(TcpSource::connect(host, *port, options)?)),
        SourceSpec::Usb { index } => Err(SourceError::Config(format!(
            "USB source (index {index}) is not implemented yet. Run rtl_tcp and use \
             tcp:127.0.0.1:1234 -- that is the recommended deployment anyway, because \
             rtl_tcp uses libusb async transfers and does not drop samples between reads."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_source_form() {
        assert_eq!(
            SourceSpec::parse("file:a/b.cu8").unwrap(),
            SourceSpec::File {
                path: "a/b.cu8".into()
            }
        );
        assert_eq!(
            SourceSpec::parse("tcp:127.0.0.1:1234").unwrap(),
            SourceSpec::Tcp {
                host: "127.0.0.1".into(),
                port: 1234
            }
        );
        assert_eq!(
            SourceSpec::parse("usb").unwrap(),
            SourceSpec::Usb { index: 0 }
        );
        assert_eq!(
            SourceSpec::parse("usb:2").unwrap(),
            SourceSpec::Usb { index: 2 }
        );
    }

    #[test]
    fn ipv6_hosts_survive_the_colon_split() {
        assert_eq!(
            SourceSpec::parse("tcp:::1:1234").unwrap(),
            SourceSpec::Tcp {
                host: "::1".into(),
                port: 1234
            }
        );
    }

    #[test]
    fn bad_specs_explain_themselves() {
        for bad in [
            "",
            "file:",
            "tcp:nohost",
            "tcp:host:notaport",
            "usb:x",
            "wat",
        ] {
            let err = SourceSpec::parse(bad)
                .err()
                .unwrap_or_else(|| panic!("'{bad}' should not parse"));
            assert!(
                matches!(err, SourceError::Config(_)),
                "'{bad}' gave {err:?}, expected a Config error"
            );
        }
    }

    #[test]
    fn gain_parsing_handles_auto_and_decibels() {
        assert_eq!(Gain::parse("auto").unwrap(), Gain::Auto);
        assert_eq!(Gain::parse("AUTO").unwrap(), Gain::Auto);
        assert_eq!(Gain::parse("49.6").unwrap(), Gain::Tenths(496));
        assert_eq!(Gain::parse("0").unwrap(), Gain::Tenths(0));
        assert!(Gain::parse("loud").is_err());
    }

    /// `rtl_sdr -g 0` means auto gain. Ours must not, or every gain sweep
    /// silently starts with a duplicate of the auto data point.
    #[test]
    fn zero_gain_means_zero_not_auto() {
        assert_eq!(Gain::parse("0").unwrap(), Gain::Tenths(0));
        assert_ne!(Gain::parse("0").unwrap(), Gain::Auto);
    }

    #[test]
    fn error_taxonomy_drives_retry_behaviour() {
        assert!(SourceError::Transient("x".into()).is_transient());
        assert!(!SourceError::Config("x".into()).is_transient());
        assert!(!SourceError::EndOfStream.is_transient());
        assert!(SourceError::EndOfStream.is_end_of_stream());
    }

    #[test]
    fn usb_is_refused_with_a_useful_message() {
        let err = open(&SourceSpec::Usb { index: 0 }, &SourceOptions::default())
            .err()
            .expect("usb should not open yet");
        let msg = err.to_string();
        assert!(
            msg.contains("rtl_tcp"),
            "should point at the alternative: {msg}"
        );
    }
}
