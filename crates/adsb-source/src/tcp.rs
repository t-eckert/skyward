//! An `rtl_tcp` client.
//!
//! The recommended deployment path. `rtl_tcp` is battle-tested C, is
//! apt-installable on the Pi, and — the part that matters — uses libusb
//! **asynchronous** transfers internally, so it keeps capturing while the
//! consumer is busy. A synchronous `read_sync` loop drops whatever arrives
//! between calls, invisibly, and the loss shows up as "my detector is bad".
//!
//! Keeping USB out of this binary also means zero C dependencies, which turns
//! cross-compiling for aarch64 from an afternoon into a non-event.
//!
//! # Protocol
//!
//! On connect the server sends a 12-byte header: the magic `RTL0`, then the
//! tuner type and the number of gain-table entries, both big-endian `u32`.
//! After that it is a raw stream of interleaved `u8` IQ.
//!
//! Commands are five bytes: one opcode, then a big-endian `u32`.

use crate::{Gain, IqSource, SourceError, SourceOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

// Command opcodes. The old implementation had only the first three, which is
// why a gain sweep over rtl_tcp was not possible.
const CMD_SET_FREQ: u8 = 0x01;
const CMD_SET_SAMPLE_RATE: u8 = 0x02;
const CMD_SET_GAIN_MODE: u8 = 0x03;
const CMD_SET_GAIN: u8 = 0x04;
const CMD_SET_FREQ_CORRECTION: u8 = 0x05;
const CMD_SET_AGC_MODE: u8 = 0x08;
const CMD_SET_DIRECT_SAMPLING: u8 = 0x09;
const CMD_SET_GAIN_INDEX: u8 = 0x0D;
const CMD_SET_BIAS_TEE: u8 = 0x0E;

/// What the server reports about itself on connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DongleInfo {
    pub tuner_type: u32,
    pub gain_count: u32,
}

impl DongleInfo {
    pub fn tuner_name(&self) -> &'static str {
        match self.tuner_type {
            1 => "E4000",
            2 => "FC0012",
            3 => "FC0013",
            4 => "FC2580",
            5 => "R820T",
            6 => "R828D",
            _ => "unknown",
        }
    }

    fn parse(header: &[u8; 12]) -> Result<DongleInfo, SourceError> {
        if &header[..4] != b"RTL0" {
            return Err(SourceError::Config(format!(
                "not an rtl_tcp server: expected magic 'RTL0', got {:?}. \
                 Is something else listening on that port?",
                String::from_utf8_lossy(&header[..4])
            )));
        }
        Ok(DongleInfo {
            tuner_type: u32::from_be_bytes([header[4], header[5], header[6], header[7]]),
            gain_count: u32::from_be_bytes([header[8], header[9], header[10], header[11]]),
        })
    }
}

pub struct TcpSource {
    stream: TcpStream,
    address: String,
    info: DongleInfo,
    sample_rate: u32,
    read_timeout: Duration,
    /// Replayed after a reconnect so the dongle comes back configured.
    applied: Vec<(u8, u32)>,
}

impl TcpSource {
    pub fn connect(host: &str, port: u16, options: &SourceOptions) -> Result<Self, SourceError> {
        let address = format!("{host}:{port}");
        let stream = Self::dial(&address, options.read_timeout)?;
        let info = Self::read_header(&stream, &address)?;

        let mut source = TcpSource {
            stream,
            address,
            info,
            sample_rate: options.sample_rate,
            read_timeout: options.read_timeout,
            applied: Vec::new(),
        };
        source.set_sample_rate(options.sample_rate)?;
        Ok(source)
    }

    fn dial(address: &str, timeout: Duration) -> Result<TcpStream, SourceError> {
        let addr = address
            .to_socket_addrs()
            .map_err(|e| SourceError::Config(format!("cannot resolve {address}: {e}")))?
            .next()
            .ok_or_else(|| SourceError::Config(format!("{address} resolved to nothing")))?;

        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| SourceError::Transient(format!("cannot connect to {address}: {e}")))?;

        // Without this a wedged server blocks the process forever with no log
        // line -- the single worst failure mode on a box you cannot log into.
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| SourceError::Transient(format!("cannot set read timeout: {e}")))?;
        // IQ is latency-sensitive and already in large blocks.
        let _ = stream.set_nodelay(true);
        Ok(stream)
    }

    fn read_header(stream: &TcpStream, address: &str) -> Result<DongleInfo, SourceError> {
        let mut header = [0u8; 12];
        let mut got = 0;
        let mut s = stream;
        while got < 12 {
            match s.read(&mut header[got..]) {
                Ok(0) => {
                    return Err(SourceError::Transient(format!(
                        "{address} closed before sending its header"
                    )));
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(SourceError::Transient(format!(
                        "reading header from {address}: {e}"
                    )));
                }
            }
        }
        DongleInfo::parse(&header)
    }

    pub fn info(&self) -> DongleInfo {
        self.info
    }

    /// Send a command, remembering it so a reconnect can restore the state.
    fn command(&mut self, opcode: u8, value: u32) -> Result<(), SourceError> {
        let mut packet = [0u8; 5];
        packet[0] = opcode;
        packet[1..].copy_from_slice(&value.to_be_bytes());
        self.stream
            .write_all(&packet)
            .map_err(|e| SourceError::Transient(format!("sending command {opcode:#04x}: {e}")))?;

        self.applied.retain(|(op, _)| *op != opcode);
        self.applied.push((opcode, value));
        Ok(())
    }

    /// Re-establish the connection and replay every setting.
    ///
    /// Reconnecting without replaying leaves the dongle on its defaults —
    /// wrong frequency, auto gain — and the receiver goes quiet for reasons
    /// that are invisible from the message count alone.
    pub fn reconnect(&mut self) -> Result<(), SourceError> {
        let stream = Self::dial(&self.address, self.read_timeout)?;
        self.info = Self::read_header(&stream, &self.address)?;
        self.stream = stream;

        let settings = std::mem::take(&mut self.applied);
        for (opcode, value) in settings {
            self.command(opcode, value)?;
        }
        Ok(())
    }

    pub fn set_frequency_correction(&mut self, ppm: i32) -> Result<(), SourceError> {
        self.command(CMD_SET_FREQ_CORRECTION, ppm as u32)
    }

    pub fn set_direct_sampling(&mut self, mode: u32) -> Result<(), SourceError> {
        self.command(CMD_SET_DIRECT_SAMPLING, mode)
    }
}

impl IqSource for TcpSource {
    fn describe(&self) -> String {
        format!(
            "tcp:{} ({} tuner, {} gain steps) at {:.3} MS/s",
            self.address,
            self.info.tuner_name(),
            self.info.gain_count,
            f64::from(self.sample_rate) / 1e6
        )
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SourceError> {
        if buf.len() < 2 {
            return Ok(0);
        }
        // Ask for an even count so a pair is never split at the buffer edge.
        let want = buf.len() & !1;

        match self.stream.read(&mut buf[..want]) {
            Ok(0) => Err(SourceError::Transient(format!(
                "{} closed the connection",
                self.address
            ))),
            Ok(n) => {
                if n.is_multiple_of(2) {
                    Ok(n)
                } else {
                    // Complete the pair rather than carrying a byte over; the
                    // socket has more data by definition.
                    let mut got = n;
                    while got < n + 1 {
                        match self.stream.read(&mut buf[got..n + 1]) {
                            Ok(0) => return Ok(n - 1),
                            Ok(m) => got += m,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => return Ok(n - 1),
                        }
                    }
                    Ok(got)
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(SourceError::Transient(format!(
                    "no data from {} for {:?} -- server may be wedged",
                    self.address, self.read_timeout
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(0),
            Err(e) => Err(SourceError::Transient(format!(
                "reading from {}: {e}",
                self.address
            ))),
        }
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn set_frequency(&mut self, hz: u32) -> Result<(), SourceError> {
        self.command(CMD_SET_FREQ, hz)
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), SourceError> {
        self.command(CMD_SET_SAMPLE_RATE, hz)?;
        self.sample_rate = hz;
        Ok(())
    }

    fn set_gain(&mut self, gain: Gain) -> Result<(), SourceError> {
        match gain {
            Gain::Auto => self.command(CMD_SET_GAIN_MODE, 0),
            Gain::Tenths(tenths) => {
                // Manual mode first: setting a value while the tuner is in
                // automatic mode is silently ignored.
                self.command(CMD_SET_GAIN_MODE, 1)?;
                self.command(CMD_SET_GAIN, tenths as u32)
            }
            Gain::Index(index) => {
                self.command(CMD_SET_GAIN_MODE, 1)?;
                self.command(CMD_SET_GAIN_INDEX, index)
            }
        }
    }

    fn set_agc(&mut self, enabled: bool) -> Result<(), SourceError> {
        self.command(CMD_SET_AGC_MODE, u32::from(enabled))
    }

    fn set_bias_tee(&mut self, enabled: bool) -> Result<(), SourceError> {
        self.command(CMD_SET_BIAS_TEE, u32::from(enabled))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// A minimal rtl_tcp stand-in. Returns the bound port and a channel that
    /// yields every command byte the client sent.
    fn fake_server(
        payload: Vec<u8>,
        send_header: bool,
        magic: &'static [u8; 4],
    ) -> (u16, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            if send_header {
                let mut header = Vec::new();
                header.extend_from_slice(magic);
                header.extend_from_slice(&5u32.to_be_bytes()); // R820T
                header.extend_from_slice(&29u32.to_be_bytes()); // 29 gain steps
                sock.write_all(&header).unwrap();
            }
            sock.write_all(&payload).unwrap();
            sock.flush().unwrap();

            // Echo back whatever commands arrive until the client hangs up.
            let mut reader = sock.try_clone().unwrap();
            let mut buf = [0u8; 5];
            while let Ok(5) = reader.read(&mut buf) {
                if tx.send(buf.to_vec()).is_err() {
                    break;
                }
            }
        });
        (port, rx)
    }

    fn options() -> SourceOptions {
        SourceOptions {
            read_timeout: Duration::from_millis(500),
            ..SourceOptions::for_benchmark(2_400_000)
        }
    }

    #[test]
    fn parses_the_dongle_header() {
        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let (port, _rx) = fake_server(payload, true, b"RTL0");
        let src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();
        assert_eq!(src.info().tuner_name(), "R820T");
        assert_eq!(src.info().gain_count, 29);
        assert!(src.describe().contains("R820T"));
    }

    #[test]
    fn rejects_a_server_that_is_not_rtl_tcp() {
        let (port, _rx) = fake_server(vec![0; 100], true, b"HTTP");
        let err = TcpSource::connect("127.0.0.1", port, &options())
            .err()
            .expect("should reject bad magic");
        assert!(!err.is_transient(), "wrong protocol is a config error");
        assert!(err.to_string().contains("RTL0"));
    }

    #[test]
    fn reads_the_payload_in_even_chunks() {
        let payload: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let (port, _rx) = fake_server(payload.clone(), true, b"RTL0");
        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();

        let mut got = Vec::new();
        let mut buf = [0u8; 333]; // deliberately odd
        while got.len() < payload.len() {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    assert!(n.is_multiple_of(2), "odd read of {n} bytes");
                    got.extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }
        assert_eq!(&got[..payload.len()], &payload[..]);
    }

    #[test]
    fn sends_the_gain_commands_the_old_client_lacked() {
        let (port, rx) = fake_server(vec![0u8; 64], true, b"RTL0");
        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();

        src.set_frequency(1_090_000_000).unwrap();
        src.set_gain(Gain::Tenths(496)).unwrap();
        src.set_agc(false).unwrap();
        src.set_bias_tee(false).unwrap();

        let mut seen = Vec::new();
        while let Ok(cmd) = rx.recv_timeout(Duration::from_millis(500)) {
            seen.push((cmd[0], u32::from_be_bytes([cmd[1], cmd[2], cmd[3], cmd[4]])));
            if seen.len() >= 6 {
                break;
            }
        }

        assert!(
            seen.contains(&(CMD_SET_SAMPLE_RATE, 2_400_000)),
            "sample rate not sent: {seen:?}"
        );
        assert!(
            seen.contains(&(CMD_SET_FREQ, 1_090_000_000)),
            "frequency not sent: {seen:?}"
        );
        // Manual mode must precede the value, or the gain is silently ignored.
        let mode_at = seen
            .iter()
            .position(|&(op, v)| op == CMD_SET_GAIN_MODE && v == 1);
        let gain_at = seen
            .iter()
            .position(|&(op, v)| op == CMD_SET_GAIN && v == 496);
        assert!(
            mode_at.is_some() && gain_at.is_some(),
            "gain not sent: {seen:?}"
        );
        assert!(
            mode_at < gain_at,
            "gain value sent before manual mode: {seen:?}"
        );
        assert!(
            seen.contains(&(CMD_SET_AGC_MODE, 0)),
            "AGC not disabled: {seen:?}"
        );
    }

    #[test]
    fn auto_gain_uses_mode_zero_not_a_zero_value() {
        let (port, rx) = fake_server(vec![0u8; 64], true, b"RTL0");
        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();
        src.set_gain(Gain::Auto).unwrap();

        let mut seen = Vec::new();
        while let Ok(cmd) = rx.recv_timeout(Duration::from_millis(500)) {
            seen.push((cmd[0], u32::from_be_bytes([cmd[1], cmd[2], cmd[3], cmd[4]])));
            if seen.len() >= 2 {
                break;
            }
        }
        assert!(seen.contains(&(CMD_SET_GAIN_MODE, 0)), "{seen:?}");
        assert!(
            !seen.iter().any(|&(op, _)| op == CMD_SET_GAIN),
            "auto gain should not send a gain value: {seen:?}"
        );
    }

    #[test]
    fn a_closed_connection_is_transient_not_fatal() {
        let (port, _rx) = fake_server(vec![0u8; 8], true, b"RTL0");
        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();
        let mut buf = [0u8; 4096];
        // Drain the payload, then the server hangs up.
        let mut err = None;
        for _ in 0..10 {
            match src.read(&mut buf) {
                Ok(_) => continue,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        let err = err.expect("should eventually error");
        assert!(
            err.is_transient(),
            "a dropped connection must trigger reconnect, not exit: {err}"
        );
    }

    /// A wedged server -- accepts, sends a header, then goes silent forever.
    /// Without a read timeout this test would hang the suite.
    #[test]
    fn a_silent_server_times_out_rather_than_hanging() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut header = Vec::new();
            header.extend_from_slice(b"RTL0");
            header.extend_from_slice(&5u32.to_be_bytes());
            header.extend_from_slice(&29u32.to_be_bytes());
            sock.write_all(&header).unwrap();
            // Hold the socket open and never send anything else.
            thread::sleep(Duration::from_secs(30));
        });

        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();
        let started = std::time::Instant::now();
        let err = src.read(&mut [0u8; 4096]).expect_err("should time out");
        assert!(err.is_transient());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}; the read timeout is not being applied",
            started.elapsed()
        );
        assert!(err.to_string().contains("wedged"), "{err}");
    }

    #[test]
    fn reconnect_replays_settings() {
        let (port, rx) = fake_server(vec![0u8; 64], true, b"RTL0");
        let mut src = TcpSource::connect("127.0.0.1", port, &options()).unwrap();
        src.set_frequency(1_090_000_000).unwrap();
        src.set_gain(Gain::Tenths(400)).unwrap();

        // The recorded settings are what a reconnect would replay.
        let ops: Vec<u8> = src.applied.iter().map(|&(op, _)| op).collect();
        assert!(ops.contains(&CMD_SET_FREQ));
        assert!(ops.contains(&CMD_SET_GAIN));
        assert!(ops.contains(&CMD_SET_SAMPLE_RATE));

        // Setting the same opcode twice must not accumulate duplicates.
        src.set_frequency(1_090_000_000).unwrap();
        assert_eq!(
            src.applied
                .iter()
                .filter(|&&(op, _)| op == CMD_SET_FREQ)
                .count(),
            1
        );
        drop(rx);
    }
}
