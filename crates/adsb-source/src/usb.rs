//! The dongle itself, over USB.
//!
//! Enabled by the `usb` cargo feature. Without it this crate has no C
//! dependency at all and cross-compiling is a non-event, which is why the
//! feature is off by default and [`tcp`](crate::tcp) remains a supported
//! deployment. With it, `skyward run --source usb` is the whole radio: no
//! `rtl_tcp`, no second unit to supervise, no localhost socket in the middle.
//!
//! # Why librtlsdr rather than a Rust reimplementation
//!
//! Everything hard about an RTL-SDR is below this line: the RTL2832U register
//! map, the R820T PLL and its VCO band search, the tuner's I2C gate, and the
//! fact that the whole thing must be driven with libusb *asynchronous*
//! transfers to keep capturing while the consumer is busy. Reimplementing that
//! in Rust would be a fine project and a terrible dependency — a subtle error
//! in the VCO search produces a radio that tunes to almost the right frequency
//! and decodes nothing, which is indistinguishable from a bad antenna.
//!
//! So this is a hand-written FFI binding to the same C library `rtl_tcp` uses.
//! Roughly forty lines of `extern` declarations, no bindgen, no build-time
//! codegen: the build stays a plain `cargo build` with one `-l` flag.
//!
//! # The async transfer is the entire point
//!
//! `rtlsdr_read_sync` drops whatever the dongle produces between calls, and it
//! does so silently — the samples are simply never delivered and the loss
//! reads as "my detector got worse". So the device is driven exactly the way
//! `rtl_tcp` drives it: a dedicated thread parked in `rtlsdr_read_async`,
//! which hands each completed URB to a callback, which appends it to a ring
//! buffer that [`IqSource::read`] drains.
//!
//! The ring is bounded, and an overrun **drops the oldest block and counts
//! it**. Dropping the newest would be simpler, but it lets the buffer stay
//! permanently full, so every sample the decoder sees is seconds stale while
//! the counters insist everything is fine. Dropping the oldest keeps the view
//! live and makes the loss visible in one number.
//!
//! Blocks only ever enter and leave the ring whole or in even-sized pieces.
//! That matters more than it looks: lose an odd number of bytes and I and Q
//! swap for the entire rest of the stream, which decodes as pure noise and
//! looks exactly like a broken demodulator.

use crate::{Gain, IqSource, SourceError, SourceOptions};
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_uchar, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// --------------------------------------------------------------- FFI -------

#[allow(non_camel_case_types)]
type rtlsdr_dev_t = c_void;

type ReadAsyncCb = extern "C" fn(buf: *mut c_uchar, len: u32, ctx: *mut c_void);

// Signatures transcribed from rtl-sdr.h. Every one returns 0 on success
// except where noted; `rtlsdr_get_device_count` and `rtlsdr_get_sample_rate`
// return the value itself.
unsafe extern "C" {
    fn rtlsdr_get_device_count() -> u32;
    fn rtlsdr_get_device_name(index: u32) -> *const c_char;
    fn rtlsdr_get_device_usb_strings(
        index: u32,
        manufact: *mut c_char,
        product: *mut c_char,
        serial: *mut c_char,
    ) -> c_int;
    fn rtlsdr_open(dev: *mut *mut rtlsdr_dev_t, index: u32) -> c_int;
    fn rtlsdr_close(dev: *mut rtlsdr_dev_t) -> c_int;
    fn rtlsdr_set_center_freq(dev: *mut rtlsdr_dev_t, freq: u32) -> c_int;
    fn rtlsdr_get_center_freq(dev: *mut rtlsdr_dev_t) -> u32;
    fn rtlsdr_set_freq_correction(dev: *mut rtlsdr_dev_t, ppm: c_int) -> c_int;
    fn rtlsdr_get_tuner_type(dev: *mut rtlsdr_dev_t) -> c_int;
    fn rtlsdr_get_tuner_gains(dev: *mut rtlsdr_dev_t, gains: *mut c_int) -> c_int;
    fn rtlsdr_set_tuner_gain(dev: *mut rtlsdr_dev_t, gain: c_int) -> c_int;
    fn rtlsdr_get_tuner_gain(dev: *mut rtlsdr_dev_t) -> c_int;
    fn rtlsdr_set_tuner_gain_mode(dev: *mut rtlsdr_dev_t, manual: c_int) -> c_int;
    fn rtlsdr_set_sample_rate(dev: *mut rtlsdr_dev_t, rate: u32) -> c_int;
    fn rtlsdr_get_sample_rate(dev: *mut rtlsdr_dev_t) -> u32;
    fn rtlsdr_set_agc_mode(dev: *mut rtlsdr_dev_t, on: c_int) -> c_int;
    fn rtlsdr_set_direct_sampling(dev: *mut rtlsdr_dev_t, on: c_int) -> c_int;
    fn rtlsdr_set_bias_tee(dev: *mut rtlsdr_dev_t, on: c_int) -> c_int;
    fn rtlsdr_reset_buffer(dev: *mut rtlsdr_dev_t) -> c_int;
    fn rtlsdr_read_async(
        dev: *mut rtlsdr_dev_t,
        cb: ReadAsyncCb,
        ctx: *mut c_void,
        buf_num: u32,
        buf_len: u32,
    ) -> c_int;
    fn rtlsdr_cancel_async(dev: *mut rtlsdr_dev_t) -> c_int;
}

/// How many URBs librtlsdr keeps in flight. `rtl_tcp`'s default.
const BUF_COUNT: u32 = 15;

/// Bytes per URB. Must be a multiple of 512, and should be a multiple of the
/// 16384-byte URB size. 256 KiB is ~55 ms at 2.4 MS/s.
const BUF_LEN: u32 = 256 * 1024;

/// How much undelivered IQ to hold before dropping the oldest.
///
/// 8 MiB is about 1.7 seconds at 2.4 MS/s — long enough to ride out a
/// scheduling hiccup or an SQLite checkpoint, short enough that a decoder
/// which genuinely cannot keep up shows it in the drop counter within seconds
/// rather than silently accumulating latency.
const RING_CAPACITY: usize = 8 * 1024 * 1024;

// ------------------------------------------------------- enumeration -------

/// One dongle as the USB bus describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub index: u32,
    /// librtlsdr's name for the (vendor, product) pair, e.g. "Generic RTL2832U OEM".
    pub name: String,
    pub manufacturer: String,
    pub product: String,
    /// The serial, which is what `usb:` should really be keyed on when a
    /// station has two dongles: bus enumeration order is not stable across
    /// reboots, so index 0 is not reliably the same stick twice.
    pub serial: String,
}

impl DeviceInfo {
    pub fn describe(&self) -> String {
        let serial = if self.serial.is_empty() {
            "no serial".to_string()
        } else {
            format!("SN {}", self.serial)
        };
        format!("{}: {} ({serial})", self.index, self.name)
    }
}

/// Every RTL-SDR the USB bus currently offers.
///
/// Reads the descriptor strings without opening the device, so this is safe to
/// call while another process holds the dongle — which is exactly when you
/// want it, because "in use by something else" is the most common USB failure
/// and an empty list would be the wrong answer.
pub fn devices() -> Vec<DeviceInfo> {
    let count = unsafe { rtlsdr_get_device_count() };
    (0..count)
        .map(|index| {
            let name = unsafe {
                let ptr = rtlsdr_get_device_name(index);
                if ptr.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                }
            };

            // 256 bytes each is what every caller of this API uses; librtlsdr
            // writes at most 256 including the terminator.
            let mut manufact: [c_char; 256] = [0; 256];
            let mut product: [c_char; 256] = [0; 256];
            let mut serial: [c_char; 256] = [0; 256];
            let ok = unsafe {
                rtlsdr_get_device_usb_strings(
                    index,
                    manufact.as_mut_ptr(),
                    product.as_mut_ptr(),
                    serial.as_mut_ptr(),
                ) == 0
            };

            DeviceInfo {
                index,
                name,
                manufacturer: if ok { from_c(&manufact) } else { String::new() },
                product: if ok { from_c(&product) } else { String::new() },
                serial: if ok { from_c(&serial) } else { String::new() },
            }
        })
        .collect()
}

/// `[c_char; 256]` written by C into an owned `String`.
fn from_c(buf: &[c_char; 256]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

// ------------------------------------------------------------- ring -------

/// Whole URBs, plus how far into the front one the consumer has read.
///
/// Holding blocks rather than one flat byte ring means the callback never
/// copies twice and an overrun is discarded a whole URB at a time — which is
/// what keeps every drop an even number of bytes.
#[derive(Default)]
struct Ring {
    blocks: VecDeque<Vec<u8>>,
    /// Bytes of `blocks[0]` already handed to the consumer. Always even.
    head: usize,
    bytes: usize,
    /// True once the reader thread has left `rtlsdr_read_async`.
    stopped: bool,
}

struct Shared {
    ring: Mutex<Ring>,
    wake: Condvar,
    /// Bytes discarded because the consumer was too slow. The single number
    /// that separates "the decoder cannot keep up" from "the sky is quiet".
    dropped: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Shared {
            ring: Mutex::new(Ring::default()),
            wake: Condvar::new(),
            dropped: AtomicU64::new(0),
        }
    }

    /// Called from librtlsdr's libusb thread. Must not panic across FFI, so
    /// a poisoned lock is recovered rather than unwrapped.
    ///
    /// **This is the only place a block enters the ring, and it is where the
    /// evenness invariant is established.** Everything downstream — the
    /// overrun policy, the drain — depends on every block holding whole I/Q
    /// pairs; enforcing it once here is what lets those be simple. A URB
    /// length is always even in practice, but "in practice" is not a
    /// guarantee, and one odd block swaps I and Q for the rest of the session.
    fn push(&self, block: Vec<u8>) {
        let mut block = block;
        if !block.len().is_multiple_of(2) {
            block.pop();
        }
        if block.is_empty() {
            return;
        }

        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = 0u64;

        while ring.bytes + block.len() > RING_CAPACITY {
            match ring.blocks.pop_front() {
                Some(front) => {
                    let unread = front.len() - ring.head;
                    ring.bytes -= unread;
                    ring.head = 0;
                    dropped += unread as u64;
                }
                None => break,
            }
        }

        ring.bytes += block.len();
        ring.blocks.push_back(block);
        drop(ring);

        if dropped > 0 {
            self.dropped.fetch_add(dropped, Ordering::Relaxed);
        }
        self.wake.notify_one();
    }

    fn stop(&self) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.stopped = true;
        drop(ring);
        self.wake.notify_all();
    }
}

/// The trampoline librtlsdr calls for each completed transfer.
extern "C" fn on_samples(buf: *mut c_uchar, len: u32, ctx: *mut c_void) {
    if buf.is_null() || ctx.is_null() || len == 0 {
        return;
    }
    // `ctx` is the `Arc<Shared>` this source handed to rtlsdr_read_async, and
    // the source outlives the reader thread by construction: `Drop` cancels
    // the transfer and joins before releasing anything.
    let shared = unsafe { &*(ctx as *const Shared) };
    let slice = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    shared.push(slice.to_vec());
}

// ----------------------------------------------------------- device -------

/// A `rtlsdr_dev_t*` that may cross threads.
///
/// librtlsdr is used this way by `rtl_tcp` itself: one thread parked in
/// `rtlsdr_read_async` while another issues control transfers. libusb is
/// thread-safe, and the control endpoint and the bulk endpoint are separate.
struct Device(*mut rtlsdr_dev_t);

unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    fn as_ptr(&self) -> *mut rtlsdr_dev_t {
        self.0
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { rtlsdr_close(self.0) };
        }
    }
}

/// Every setting applied so far, so a reconnect can restore them.
///
/// A dongle that comes back after a USB reset is on its power-on defaults:
/// wrong frequency, automatic gain, digital AGC on. Reconnecting without
/// replaying leaves a receiver that is technically streaming and hears
/// nothing, which is invisible in every counter except the message rate.
#[derive(Clone, Copy, Debug)]
struct Settings {
    sample_rate: u32,
    frequency_hz: Option<u32>,
    gain: Option<Gain>,
    agc: Option<bool>,
    bias_tee: Option<bool>,
    ppm: Option<i32>,
}

pub struct UsbSource {
    device: Arc<Device>,
    shared: Arc<Shared>,
    reader: Option<std::thread::JoinHandle<()>>,
    index: u32,
    info: Option<DeviceInfo>,
    tuner: &'static str,
    gains: Vec<i32>,
    /// The rate the hardware reports, which is not always the one requested.
    sample_rate: u32,
    read_timeout: Duration,
    settings: Settings,
}

impl UsbSource {
    /// Open device `index` and start streaming.
    pub fn open(index: u32, options: &SourceOptions) -> Result<Self, SourceError> {
        let available = unsafe { rtlsdr_get_device_count() };
        if available == 0 {
            return Err(SourceError::Config(
                "no RTL-SDR devices found on the USB bus. Is the dongle plugged in? \
                 On Linux, is the DVB-T kernel driver still holding it? \
                 (`lsmod | grep dvb_usb_rtl28xxu` -- see docs/RASPBERRY_PI.md)"
                    .to_string(),
            ));
        }
        if index >= available {
            return Err(SourceError::Config(format!(
                "usb:{index} requested but only {available} device(s) present. \
                 `skyward devices` lists them"
            )));
        }

        let info = devices().into_iter().find(|d| d.index == index);

        let mut raw: *mut rtlsdr_dev_t = std::ptr::null_mut();
        let rc = unsafe { rtlsdr_open(&mut raw, index) };
        if rc != 0 || raw.is_null() {
            // -6 is libusb's LIBUSB_ERROR_ACCESS by the time it reaches here on
            // Linux; naming both causes is worth more than the number.
            return Err(SourceError::Config(format!(
                "cannot open usb:{index} (librtlsdr returned {rc}). Either another \
                 process holds the dongle -- rtl_tcp, rtl_test, dump1090 -- or the \
                 user lacks permission. On Linux install the udev rule shipped with \
                 rtl-sdr, or run as a member of the `plugdev` group"
            )));
        }

        let device = Arc::new(Device(raw));
        let tuner = tuner_name(unsafe { rtlsdr_get_tuner_type(device.as_ptr()) });
        let gains = read_gain_table(device.as_ptr());

        let mut source = UsbSource {
            device,
            shared: Arc::new(Shared::new()),
            reader: None,
            index,
            info,
            tuner,
            gains,
            sample_rate: options.sample_rate,
            read_timeout: options.read_timeout,
            settings: Settings {
                sample_rate: options.sample_rate,
                frequency_hz: None,
                gain: None,
                agc: None,
                bias_tee: None,
                ppm: None,
            },
        };

        source.set_sample_rate(options.sample_rate)?;
        source.start_streaming()?;
        Ok(source)
    }

    /// Bytes discarded because the consumer could not keep up.
    pub fn dropped_bytes(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    pub fn tuner_name(&self) -> &'static str {
        self.tuner
    }

    pub fn info(&self) -> Option<&DeviceInfo> {
        self.info.as_ref()
    }

    /// Frequency correction in parts per million.
    ///
    /// Not needed for Mode S — the demodulator is non-coherent and a few ppm
    /// at 1090 MHz is tens of kilohertz inside a 2 MHz-wide channel — but the
    /// dongle's crystal error also scales the *sample clock*, and that does
    /// matter. Exposed for completeness.
    pub fn set_frequency_correction(&mut self, ppm: i32) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_freq_correction(self.device.as_ptr(), ppm) },
            "set frequency correction",
        )?;
        self.settings.ppm = Some(ppm);
        Ok(())
    }

    pub fn set_direct_sampling(&mut self, mode: i32) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_direct_sampling(self.device.as_ptr(), mode) },
            "set direct sampling",
        )
    }

    /// Park a thread in `rtlsdr_read_async` and let it fill the ring.
    fn start_streaming(&mut self) -> Result<(), SourceError> {
        // Discard whatever the RTL2832's FIFO accumulated while it was being
        // configured; those samples were taken at the wrong gain or frequency.
        check(
            unsafe { rtlsdr_reset_buffer(self.device.as_ptr()) },
            "reset the device buffer",
        )?;

        let device = Arc::clone(&self.device);
        let shared = Arc::clone(&self.shared);
        let index = self.index;

        let handle = std::thread::Builder::new()
            .name(format!("skyward-usb{index}"))
            .spawn(move || {
                // The Arc keeps `shared` alive for as long as this thread can
                // run; `Drop` cancels and joins before dropping the source.
                let ctx = Arc::as_ptr(&shared) as *mut c_void;
                let rc = unsafe {
                    rtlsdr_read_async(device.as_ptr(), on_samples, ctx, BUF_COUNT, BUF_LEN)
                };
                if rc != 0 {
                    // Not fatal by itself: `read` turns an empty stopped ring
                    // into a transient error, and the caller reconnects.
                    shared.dropped.fetch_add(0, Ordering::Relaxed);
                }
                shared.stop();
            })
            .map_err(|e| SourceError::Transient(format!("cannot spawn the USB reader: {e}")))?;

        self.reader = Some(handle);
        Ok(())
    }

    /// Cancel the transfer and join the reader thread.
    fn stop_streaming(&mut self) {
        if self.reader.is_none() {
            return;
        }
        unsafe { rtlsdr_cancel_async(self.device.as_ptr()) };
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }

    /// Re-apply every setting recorded so far.
    fn replay(&mut self) -> Result<(), SourceError> {
        let settings = self.settings;
        self.set_sample_rate(settings.sample_rate)?;
        if let Some(hz) = settings.frequency_hz {
            self.set_frequency(hz)?;
        }
        if let Some(gain) = settings.gain {
            self.set_gain(gain)?;
        }
        if let Some(agc) = settings.agc {
            self.set_agc(agc)?;
        }
        if let Some(on) = settings.bias_tee {
            self.set_bias_tee(on)?;
        }
        if let Some(ppm) = settings.ppm {
            self.set_frequency_correction(ppm)?;
        }
        Ok(())
    }
}

/// Copy up to `buf.len()` bytes out of the ring, oldest first.
///
/// Split out of [`IqSource::read`] so it can be tested without a dongle: this
/// is the one piece of the USB path whose bugs are silent. Lose a byte at a
/// block seam and I and Q swap for the entire rest of the session, which
/// decodes as noise and looks exactly like a broken demodulator.
///
/// Every take here is even, and that falls out rather than being imposed:
/// `want` is even, and every block is even because [`Shared::push`] makes it
/// so, therefore every remainder and every minimum of the two is even. The
/// assertion states that out loud instead of masking a violation of it with
/// another `& !1` — a silent correction here would turn a broken invariant
/// into lost samples, which is the harder of the two to notice.
fn drain(ring: &mut Ring, buf: &mut [u8]) -> usize {
    let want = buf.len() & !1;
    let mut written = 0;
    while written < want {
        let head = ring.head;
        let Some(front) = ring.blocks.front() else {
            break;
        };
        let block_len = front.len();
        let take = (block_len - head).min(want - written);
        debug_assert!(
            take.is_multiple_of(2),
            "a {take}-byte take from a {block_len}-byte block at offset {head} \
             would split an I/Q pair"
        );
        if take == 0 {
            break;
        }
        buf[written..written + take].copy_from_slice(&front[head..head + take]);
        written += take;
        ring.head = head + take;
        ring.bytes -= take;
        if ring.head == block_len {
            ring.blocks.pop_front();
            ring.head = 0;
        }
    }
    written
}

impl Drop for UsbSource {
    fn drop(&mut self) {
        // Order matters: cancel and join first, or the callback can fire
        // against a `Shared` that is on its way out.
        self.stop_streaming();
    }
}

impl IqSource for UsbSource {
    fn describe(&self) -> String {
        let identity = match &self.info {
            Some(info) if !info.serial.is_empty() => {
                format!("{} SN {}", info.name, info.serial)
            }
            Some(info) => info.name.clone(),
            None => "RTL-SDR".to_string(),
        };
        format!(
            "usb:{} ({identity}, {} tuner, {} gain steps) at {:.3} MS/s",
            self.index,
            self.tuner,
            self.gains.len(),
            f64::from(self.sample_rate) / 1e6
        )
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SourceError> {
        if buf.len() < 2 {
            return Ok(0);
        }

        let mut ring = self.shared.ring.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + self.read_timeout;

        while ring.bytes == 0 && !ring.stopped {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SourceError::Transient(format!(
                    "no samples from usb:{} for {:?}. The dongle has stopped delivering \
                     transfers -- a USB reset, a browned-out hub, or a device that was \
                     unplugged",
                    self.index, self.read_timeout
                )));
            }
            let (guard, _) = self
                .shared
                .wake
                .wait_timeout(ring, remaining)
                .unwrap_or_else(|e| e.into_inner());
            ring = guard;
        }

        if ring.bytes == 0 && ring.stopped {
            return Err(SourceError::Transient(format!(
                "usb:{} stopped streaming",
                self.index
            )));
        }

        Ok(drain(&mut ring, buf))
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn set_frequency(&mut self, hz: u32) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_center_freq(self.device.as_ptr(), hz) },
            "tune",
        )?;
        self.settings.frequency_hz = Some(hz);

        // The synthesiser lands on a multiple of its step, not on the number
        // you asked for. At 1090 MHz the error is a few hundred hertz and
        // harmless, but reporting the requested value would hide a real fault.
        let actual = unsafe { rtlsdr_get_center_freq(self.device.as_ptr()) };
        if actual != 0 && actual.abs_diff(hz) > 100_000 {
            return Err(SourceError::Config(format!(
                "asked for {:.3} MHz, tuner reports {:.3} MHz. That is far enough off \
                 that nothing will decode",
                f64::from(hz) / 1e6,
                f64::from(actual) / 1e6
            )));
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, hz: u32) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_sample_rate(self.device.as_ptr(), hz) },
            "set the sample rate",
        )?;
        self.settings.sample_rate = hz;

        // The RTL2832U derives its rate from a 28.8 MHz clock through a
        // fractional divider. 2.4 MS/s is exact (28.8/12); 2.048 MS/s is not.
        // Reporting the requested rate while the hardware runs at another is
        // the leading cause of "plenty of preambles, zero valid CRCs", so the
        // achieved rate is what everything downstream gets told.
        let actual = unsafe { rtlsdr_get_sample_rate(self.device.as_ptr()) };
        self.sample_rate = if actual == 0 { hz } else { actual };
        Ok(())
    }

    fn set_gain(&mut self, gain: Gain) -> Result<(), SourceError> {
        match gain {
            Gain::Auto => {
                check(
                    unsafe { rtlsdr_set_tuner_gain_mode(self.device.as_ptr(), 0) },
                    "enable automatic gain",
                )?;
            }
            Gain::Tenths(tenths) => {
                if !self.gains.is_empty() && !self.gains.contains(&tenths) {
                    // Silently getting a different gain than you asked for
                    // invalidates every gain sweep, so this is a hard error
                    // that prints the real table.
                    return Err(SourceError::Config(format!(
                        "gain {:.1} dB is not one of this tuner's steps. Available: {}",
                        f64::from(tenths) / 10.0,
                        describe_gains(&self.gains)
                    )));
                }
                // Manual mode first: a value set while the tuner is in
                // automatic mode is accepted and ignored.
                check(
                    unsafe { rtlsdr_set_tuner_gain_mode(self.device.as_ptr(), 1) },
                    "enable manual gain",
                )?;
                check(
                    unsafe { rtlsdr_set_tuner_gain(self.device.as_ptr(), tenths) },
                    "set gain",
                )?;
            }
            Gain::Index(index) => {
                let tenths = *self.gains.get(index as usize).ok_or_else(|| {
                    SourceError::Config(format!(
                        "gain index {index} is out of range; this tuner has {} steps",
                        self.gains.len()
                    ))
                })?;
                check(
                    unsafe { rtlsdr_set_tuner_gain_mode(self.device.as_ptr(), 1) },
                    "enable manual gain",
                )?;
                check(
                    unsafe { rtlsdr_set_tuner_gain(self.device.as_ptr(), tenths) },
                    "set gain",
                )?;
            }
        }
        self.settings.gain = Some(gain);
        Ok(())
    }

    fn set_agc(&mut self, enabled: bool) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_agc_mode(self.device.as_ptr(), c_int::from(enabled)) },
            "set the digital AGC",
        )?;
        self.settings.agc = Some(enabled);
        Ok(())
    }

    fn set_bias_tee(&mut self, enabled: bool) -> Result<(), SourceError> {
        check(
            unsafe { rtlsdr_set_bias_tee(self.device.as_ptr(), c_int::from(enabled)) },
            "set the bias tee",
        )?;
        self.settings.bias_tee = Some(enabled);
        Ok(())
    }

    fn gain_table(&self) -> Vec<i32> {
        self.gains.clone()
    }

    fn overruns(&self) -> u64 {
        self.dropped_bytes()
    }

    /// Close, reopen, replay, and restart the transfer.
    ///
    /// A USB device that resets comes back at the same index but as a fresh
    /// handle; the old one returns errors forever. Reopening is the only way
    /// back, and it is worth doing automatically — the whole point of the
    /// error taxonomy is that a receiver never dies quietly.
    fn reconnect(&mut self) -> Result<(), SourceError> {
        self.stop_streaming();

        let mut raw: *mut rtlsdr_dev_t = std::ptr::null_mut();
        let rc = unsafe { rtlsdr_open(&mut raw, self.index) };
        if rc != 0 || raw.is_null() {
            return Err(SourceError::Transient(format!(
                "cannot reopen usb:{}: librtlsdr returned {rc}",
                self.index
            )));
        }

        // Replace the handle before replaying, so every setter targets the new
        // device. Dropping the old `Device` closes the stale handle.
        self.device = Arc::new(Device(raw));
        self.shared = Arc::new(Shared::new());
        self.gains = read_gain_table(self.device.as_ptr());
        self.tuner = tuner_name(unsafe { rtlsdr_get_tuner_type(self.device.as_ptr()) });
        self.info = devices().into_iter().find(|d| d.index == self.index);

        self.replay()?;
        self.start_streaming()
    }
}

// ---------------------------------------------------------- helpers -------

fn check(rc: c_int, what: &str) -> Result<(), SourceError> {
    if rc == 0 {
        Ok(())
    } else {
        Err(SourceError::Transient(format!(
            "cannot {what}: librtlsdr returned {rc}"
        )))
    }
}

fn tuner_name(kind: c_int) -> &'static str {
    match kind {
        1 => "E4000",
        2 => "FC0012",
        3 => "FC0013",
        4 => "FC2580",
        5 => "R820T",
        6 => "R828D",
        _ => "unknown",
    }
}

fn read_gain_table(dev: *mut rtlsdr_dev_t) -> Vec<i32> {
    // Passing NULL asks for the count; the second call fills the array.
    let count = unsafe { rtlsdr_get_tuner_gains(dev, std::ptr::null_mut()) };
    if count <= 0 {
        return Vec::new();
    }
    let mut gains: Vec<c_int> = vec![0; count as usize];
    let written = unsafe { rtlsdr_get_tuner_gains(dev, gains.as_mut_ptr()) };
    if written <= 0 {
        return Vec::new();
    }
    gains.truncate(written as usize);
    gains
}

/// The gain table as decibels, for an error message an operator can act on.
pub fn describe_gains(gains: &[i32]) -> String {
    gains
        .iter()
        .map(|t| format!("{:.1}", f64::from(*t) / 10.0))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tuner gain currently in effect, in tenths of a dB.
pub fn current_gain(source: &UsbSource) -> i32 {
    unsafe { rtlsdr_get_tuner_gain(source.device.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the library is linked and the calling convention is right,
    /// with or without a dongle attached: a wrong `extern` signature or a
    /// missing symbol fails to link or crashes, it does not return a count.
    #[test]
    fn enumeration_answers_without_a_dongle() {
        let found = devices();
        assert_eq!(
            found.len(),
            unsafe { rtlsdr_get_device_count() } as usize,
            "devices() must report exactly what the bus offers"
        );
        for device in &found {
            assert!(!device.describe().is_empty());
        }
    }

    #[test]
    fn opening_a_device_that_is_not_there_is_a_config_error() {
        let available = unsafe { rtlsdr_get_device_count() };
        let err = UsbSource::open(available + 10, &SourceOptions::default())
            .err()
            .expect("an out-of-range index cannot open");
        assert!(
            !err.is_transient(),
            "asking for a device that does not exist is a configuration \
             mistake, and retrying it forever would hide the typo: {err}"
        );
    }

    #[test]
    fn gains_render_as_decibels() {
        assert_eq!(describe_gains(&[0, 496, 144]), "0.0, 49.6, 14.4");
    }

    /// The ring is the part that runs with no hardware, and the part where a
    /// mistake is invisible: an odd-sized drop swaps I and Q for the rest of
    /// the session and every downstream symptom points at the demodulator.
    #[test]
    fn the_ring_drops_whole_blocks_and_never_splits_a_pair() {
        let shared = Shared::new();
        let block = 64 * 1024;
        // Overfill by three blocks.
        let total = RING_CAPACITY / block + 3;
        for i in 0..total {
            shared.push(vec![(i % 251) as u8; block]);
        }

        let ring = shared.ring.lock().unwrap();
        assert!(ring.bytes <= RING_CAPACITY, "ring exceeded its capacity");
        assert!(
            ring.bytes.is_multiple_of(2),
            "an odd byte count means I/Q swapped"
        );
        assert!(
            shared.dropped.load(Ordering::Relaxed) > 0,
            "an overrun must be counted, not hidden"
        );
        assert!(
            shared.dropped.load(Ordering::Relaxed).is_multiple_of(2),
            "dropping an odd number of bytes swaps I and Q for the rest of the stream"
        );
    }

    /// The consumer's buffer size has nothing to do with the URB size, so
    /// almost every read spans a block boundary. Reassembling the stream out
    /// of order, or dropping a byte at the seam, would corrupt everything
    /// downstream while every counter still looked healthy.
    #[test]
    fn reads_reassemble_the_stream_across_block_boundaries() {
        let shared = Shared::new();
        // Three blocks of a known, continuous byte sequence.
        let mut expected = Vec::new();
        let mut next = 0u8;
        for _ in 0..3 {
            let block: Vec<u8> = (0..1000)
                .map(|_| {
                    let value = next;
                    next = next.wrapping_add(1);
                    value
                })
                .collect();
            expected.extend_from_slice(&block);
            shared.push(block);
        }

        let mut ring = shared.ring.lock().unwrap();
        let mut got = Vec::new();
        // Deliberately odd, and not a divisor of the block size.
        let mut buf = [0u8; 333];
        loop {
            let n = drain(&mut ring, &mut buf);
            if n == 0 {
                break;
            }
            assert!(n.is_multiple_of(2), "odd read of {n} bytes swaps I and Q");
            got.extend_from_slice(&buf[..n]);
        }

        assert_eq!(got, expected, "the stream was not reassembled in order");
        assert_eq!(ring.bytes, 0, "the ring should be empty");
        assert!(ring.blocks.is_empty(), "consumed blocks should be released");
    }

    /// A one-byte buffer cannot hold an I/Q pair, so it must return nothing
    /// rather than half of one.
    #[test]
    fn a_buffer_too_small_for_a_pair_reads_nothing() {
        let shared = Shared::new();
        shared.push(vec![1, 2, 3, 4]);
        let mut ring = shared.ring.lock().unwrap();
        assert_eq!(drain(&mut ring, &mut [0u8; 1]), 0);
        assert_eq!(ring.bytes, 4, "a refused read must not consume anything");
    }

    #[test]
    fn draining_an_empty_ring_returns_zero() {
        let shared = Shared::new();
        let mut ring = shared.ring.lock().unwrap();
        assert_eq!(drain(&mut ring, &mut [0u8; 64]), 0);
    }

    #[test]
    fn odd_callback_lengths_are_truncated_before_they_reach_the_ring() {
        let shared = Arc::new(Shared::new());
        let mut odd = vec![7u8; 1023];
        on_samples(
            odd.as_mut_ptr(),
            odd.len() as u32,
            Arc::as_ptr(&shared) as *mut c_void,
        );
        let ring = shared.ring.lock().unwrap();
        assert_eq!(ring.bytes, 1022, "an odd URB must be truncated, not stored");
    }

    /// The evenness invariant is established in exactly one place, so this is
    /// the test that has to hold it. If it ever fails, `drain`'s debug
    /// assertion fires next and the whole USB path is suspect.
    #[test]
    fn the_ring_never_holds_an_odd_block() {
        let shared = Shared::new();
        for length in [1, 2, 3, 511, 1023, 4097] {
            shared.push(vec![0u8; length]);
        }
        let ring = shared.ring.lock().unwrap();
        for block in &ring.blocks {
            assert!(
                block.len().is_multiple_of(2),
                "a {}-byte block reached the ring",
                block.len()
            );
        }
        assert!(ring.bytes.is_multiple_of(2));
    }
}
