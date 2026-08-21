//! `skyward run` — decode continuously and serve the result.
//!
//! # Shape
//!
//! ```text
//!   [decoder thread]  source -> pipeline -> tracker -> ArcSwap<Snapshot>
//!                                              |             ^
//!                                              v             |
//!                                       bounded queue     [axum]
//!                                              |
//!                                       [writer thread] -> SQLite
//! ```
//!
//! One process. The decoder is a plain OS thread, not a tokio task, because it
//! is a tight CPU loop that would otherwise monopolise an async worker.
//!
//! # What must never happen
//!
//! **The decoder must not exit on a transient error.** `rtl_tcp` restarting is
//! normal; a receiver that quietly stopped six hours ago is the failure you
//! cannot detect from the outside. Transient errors reconnect with backoff and
//! are counted.
//!
//! **The decoder must not block on storage.** The queue is bounded and full
//! means drop-and-count. A dropped row costs one dot on a map; a stalled
//! decoder loses samples permanently.

use crate::api;
use crate::config::Config;
use crate::station::{Station, StationState};
use adsb_dsp::registry;
use adsb_source::{IqSource, SourceOptions, SourceSpec};
use adsb_store::{MovementFilter, Record, Store, StoreConfig, StoreHandle};
use adsb_track::{Snapshot, Tick, Tracker, TrackerConfig, Update};
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Serve the API but do not write to the database.
    #[arg(long)]
    pub api_only: bool,

    /// Restart a file source when it runs out. With `--api-only` this gives
    /// the SvelteKit client a live-looking backend with no radio attached.
    #[arg(long)]
    pub loop_file: bool,

    /// Replay a file as fast as possible instead of at recorded speed.
    #[arg(long)]
    pub fast: bool,

    /// Stop after this many seconds. Mostly for smoke tests.
    #[arg(long)]
    pub duration_s: Option<u64>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Everything the HTTP handlers can see.
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub started: Instant,
    /// The station position, which the API may change while this runs.
    pub station: Arc<StationState>,
    pub station_name: String,
    pub db_path: String,
    pub sample_rate_hz: u32,
    pub frequency_hz: u32,
    pub impl_set: String,
    pub source_description: String,
    pub counters: Arc<Counters>,
    pub store: Option<StoreHandle>,
}

/// Live counters shared between the decoder thread and the API.
#[derive(Debug, Default)]
pub struct Counters {
    pub samples: AtomicU64,
    pub candidates: AtomicU64,
    pub crc_ok: AtomicU64,
    pub crc_fail: AtomicU64,
    pub positions: AtomicU64,
    pub reconnects: AtomicU64,
    pub last_message_ms: AtomicU64,
    pub last_sample_ms: AtomicU64,
    /// Achieved sample rate, in samples per second, times 1000.
    ///
    /// This is the number that separates "dropping samples" from "hearing
    /// nothing", which are indistinguishable in the message count alone.
    pub effective_rate_milli: AtomicU64,
    /// Bytes the source discarded before we ever saw them.
    ///
    /// A buffering source (the USB ring) keeps `read` returning a full rate
    /// while quietly throwing samples away, so the effective rate above stays
    /// perfect through a real loss. This is the only number that shows it.
    pub source_overrun_bytes: AtomicU64,
    pub streaming: AtomicBool,
}

impl AppState {
    pub fn health_json(&self) -> serde_json::Value {
        let now = now_ms();
        let last_message = self.counters.last_message_ms.load(Ordering::Relaxed) as i64;
        let last_sample = self.counters.last_sample_ms.load(Ordering::Relaxed) as i64;
        let streaming = self.counters.streaming.load(Ordering::Relaxed);
        let store = self.store.as_ref().map(|s| s.metrics()).unwrap_or_default();

        let sample_age_ms = if last_sample > 0 {
            now - last_sample
        } else {
            i64::MAX
        };
        let message_age_s = if last_message > 0 {
            (now - last_message) / 1000
        } else {
            -1
        };

        let mut warnings = Vec::new();
        if now < 1_577_836_800_000 {
            warnings.push("system clock is before 2020; NTP has not synced".to_string());
        }
        if store.dropped > 0 {
            warnings.push(format!(
                "{} rows dropped because the write queue was full",
                store.dropped
            ));
        }
        if store.errors > 0 {
            warnings.push(format!("{} database write errors", store.errors));
        }
        let overrun = self.counters.source_overrun_bytes.load(Ordering::Relaxed);
        if overrun > 0 {
            warnings.push(format!(
                "{:.1} MB of samples were dropped before decoding: the decoder is not \
                 keeping up with the radio. This looks exactly like poor reception in \
                 the message count and is a software problem",
                overrun as f64 / 1e6
            ));
        }

        // Freshness, not liveness. "The process is up" is not health.
        let status = if !streaming || sample_age_ms > 30_000 {
            "stalled"
        } else if store.is_degraded() {
            "degraded"
        } else {
            "ok"
        };

        serde_json::json!({
            "status": status,
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_s": self.started.elapsed().as_secs(),
            "source": {
                "state": if streaming { "streaming" } else { "down" },
                "description": self.source_description,
                "reconnects": self.counters.reconnects.load(Ordering::Relaxed),
                "effective_sample_rate_hz":
                    self.counters.effective_rate_milli.load(Ordering::Relaxed) as f64 / 1000.0,
                "overrun_bytes": overrun,
                "last_sample_age_ms": if sample_age_ms == i64::MAX { None } else { Some(sample_age_ms) },
            },
            "decode": {
                "last_message_age_s": message_age_s,
                "messages": self.counters.crc_ok.load(Ordering::Relaxed),
                "positions": self.counters.positions.load(Ordering::Relaxed),
                "aircraft": self.snapshot.load().len(),
            },
            "store": {
                "enabled": self.store.is_some(),
                "written": store.written,
                "dropped": store.dropped,
                "errors": store.errors,
            },
            "warnings": warnings,
        })
    }

    pub fn stats_json(&self) -> serde_json::Value {
        let c = &self.counters;
        serde_json::json!({
            "now_ms": now_ms(),
            "uptime_s": self.started.elapsed().as_secs(),
            "samples": c.samples.load(Ordering::Relaxed),
            "candidates": c.candidates.load(Ordering::Relaxed),
            "crc_ok": c.crc_ok.load(Ordering::Relaxed),
            "crc_fail": c.crc_fail.load(Ordering::Relaxed),
            "positions": c.positions.load(Ordering::Relaxed),
            "aircraft": self.snapshot.load().len(),
            "reconnects": c.reconnects.load(Ordering::Relaxed),
            "effective_sample_rate_hz":
                c.effective_rate_milli.load(Ordering::Relaxed) as f64 / 1000.0,
            "source_overrun_bytes": c.source_overrun_bytes.load(Ordering::Relaxed),
            "configured_sample_rate_hz": self.sample_rate_hz,
            "impl_set": self.impl_set,
            "store": self.store.as_ref().map(|s| {
                let m = s.metrics();
                serde_json::json!({
                    "written": m.written,
                    "dropped": m.dropped,
                    "errors": m.errors,
                    "transactions": m.transactions,
                    "retention_deleted": m.retention_deleted,
                })
            }),
        })
    }
}

pub fn run(config: Config, args: RunArgs) -> ExitCode {
    init_logging(&config);

    let spec = match SourceSpec::parse(&config.source) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e}");
            return ExitCode::from(2);
        }
    };

    let options = SourceOptions {
        sample_rate: *config.sample_rate_hz,
        pace: if args.fast {
            adsb_source::Pace::Fast
        } else {
            adsb_source::Pace::Realtime
        },
        repeat: args.loop_file,
        ..Default::default()
    };

    let mut source = match adsb_source::open(&spec, &options) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("cannot open source: {e}");
            return ExitCode::from(2);
        }
    };
    let source_description = source.describe();
    tracing::info!(source = %source_description, "source open");

    if let Err(e) = source.set_frequency(*config.frequency_hz) {
        tracing::warn!("cannot set frequency: {e}");
    }
    match adsb_source::Gain::parse(&config.gain_db) {
        Ok(gain) => {
            if let Err(e) = source.set_gain(gain) {
                tracing::warn!("cannot set gain: {e}");
            }
        }
        Err(e) => {
            tracing::error!("{e}");
            return ExitCode::from(2);
        }
    }
    // Explicitly off: leaving it on lets gain pump during a burst and corrupts
    // the amplitude relationship the demodulator depends on.
    let _ = source.set_agc(false);

    let store = if args.api_only {
        tracing::info!("--api-only: nothing will be written to the database");
        None
    } else {
        match Store::open(StoreConfig {
            path: config.db_path.value.clone(),
            retention: match *config.retention_hours {
                0 => None,
                hours => Some(Duration::from_secs(u64::from(hours) * 3600)),
            },
            ..Default::default()
        }) {
            Ok(s) => {
                tracing::info!(path = %s.path(), "store open");
                Some(s)
            }
            Err(e) => {
                tracing::error!("cannot open store: {e}");
                return ExitCode::from(2);
            }
        }
    };

    // The station position is resolved here rather than read straight off
    // `config`, because the API can change it while the decoder runs.
    let (station, station_warning) = StationState::load(
        config.receiver().map(|(lat, lon)| Station {
            lat,
            lon,
            altitude_m: *config.receiver_alt_m,
        }),
        config.receiver_origin(),
        Some(std::path::PathBuf::from(config.station_file.value.clone())),
        *config.station_writable,
    );
    if let Some(warning) = station_warning {
        tracing::warn!("{warning}");
    }
    if station.overlay_shadows_config() {
        // The "my edit did nothing" case, said out loud at startup rather
        // than discovered three restarts later.
        tracing::warn!(
            overlay = %config.station_file.value,
            "the station position set at runtime is overriding the one in {}. \
             Send DELETE /api/v1/receiver, or delete that file, to go back to it",
            station.configured_origin()
        );
    }

    let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::empty(Tick::now())));
    let counters = Arc::new(Counters::default());
    let state = Arc::new(AppState {
        snapshot: Arc::clone(&snapshot),
        started: Instant::now(),
        station: Arc::clone(&station),
        station_name: "skyward".to_string(),
        db_path: config.db_path.value.clone(),
        sample_rate_hz: *config.sample_rate_hz,
        frequency_hz: *config.frequency_hz,
        impl_set: config.impls().to_string(),
        source_description,
        counters: Arc::clone(&counters),
        store: store.as_ref().map(|s| s.handle()),
    });

    if station.get().is_none() {
        tracing::warn!(
            "receiver position unset: the range gate and local CPR are disabled. \
             Set SKYWARD_RECEIVER_LAT / SKYWARD_RECEIVER_LON, or set it from the \
             web interface -- it now takes effect without a restart"
        );
    }

    // The decoder runs on a plain thread: it is a tight CPU loop and would
    // otherwise starve the async runtime.
    let shutdown = Arc::new(AtomicBool::new(false));
    let decoder = {
        let config = config.clone();
        let snapshot = Arc::clone(&snapshot);
        let counters = Arc::clone(&counters);
        let shutdown = Arc::clone(&shutdown);
        let handle = store.as_ref().map(|s| s.handle());
        let station = Arc::clone(&station);
        std::thread::Builder::new()
            .name("skyward-decode".into())
            .spawn(move || {
                decode_loop(
                    config, source, snapshot, counters, handle, station, shutdown,
                )
            })
            .expect("spawning the decoder thread")
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("cannot start the async runtime: {e}");
            return ExitCode::from(2);
        }
    };

    let bind = config.bind.value.clone();
    let serve_state = Arc::clone(&state);
    let serve_shutdown = Arc::clone(&shutdown);
    let cors_origins = config.cors_origins.value.clone();
    let duration = args.duration_s;

    let result = runtime.block_on(async move {
        serve(serve_state, &bind, &cors_origins, serve_shutdown, duration).await
    });

    shutdown.store(true, Ordering::Relaxed);
    let _ = decoder.join();
    drop(store); // flushes and joins the writer thread

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("server error: {e}");
            ExitCode::from(2)
        }
    }
}

/// The decode loop. Never exits on a transient error.
fn decode_loop(
    config: Config,
    mut source: Box<dyn IqSource>,
    snapshot: Arc<ArcSwap<Snapshot>>,
    counters: Arc<Counters>,
    store: Option<StoreHandle>,
    station: Arc<StationState>,
    shutdown: Arc<AtomicBool>,
) {
    let rate = *config.sample_rate_hz;
    let set = config.impls();
    let mut pipeline = registry::build(&set, rate).expect("validated at startup");

    let mut tracker = Tracker::new(TrackerConfig {
        gates: adsb_track::Gates {
            receiver: station.coords(),
            ..Default::default()
        },
        ..Default::default()
    });
    // The generation the tracker's gates were built from. Comparing it once
    // per publish tick is one relaxed atomic load; re-reading the position
    // itself every time would work too, but this keeps the no-change path --
    // which is every tick but a handful in a receiver's lifetime -- free.
    let mut station_generation = station.generation();

    let movement = MovementFilter::default();
    let mut last_written: HashMap<String, (f64, f64, Option<i32>, i64)> = HashMap::new();

    let mut buf = vec![0u8; 256 * 1024 * 2];
    let mut found = Vec::new();
    let mut backoff = Duration::from_millis(250);
    let mut last_publish = Instant::now();
    let mut rate_window_start = Instant::now();
    let mut rate_window_samples = 0u64;

    counters.streaming.store(true, Ordering::Relaxed);

    while !shutdown.load(Ordering::Relaxed) {
        let n = match source.read(&mut buf) {
            Ok(0) => continue,
            Ok(n) => {
                backoff = Duration::from_millis(250);
                n
            }
            Err(e) if e.is_end_of_stream() => {
                tracing::info!("source exhausted");
                counters.streaming.store(false, Ordering::Relaxed);
                break;
            }
            Err(e) => {
                // Transient. Never exit -- a silently dead receiver is worse
                // than a noisy one.
                counters.streaming.store(false, Ordering::Relaxed);
                counters.reconnects.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %e, backoff_ms = backoff.as_millis(), "source error, retrying");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(30));

                // Retrying the *read* is not enough, and quietly was not:
                // once rtl_tcp closes the socket or a USB reset invalidates
                // the handle, every subsequent read fails identically for as
                // long as the process lives. The receiver stays "up", the log
                // repeats one line, and nothing comes back without a restart.
                // Asking the source to re-establish itself is what makes the
                // retry loop actually a recovery loop.
                if let Err(e) = source.reconnect() {
                    tracing::warn!(error = %e, "reconnect failed; will retry");
                    continue;
                }
                counters.streaming.store(true, Ordering::Relaxed);
                continue;
            }
        };

        let samples = (n / 2) as u64;
        counters.samples.fetch_add(samples, Ordering::Relaxed);
        counters
            .last_sample_ms
            .store(now_ms() as u64, Ordering::Relaxed);

        // Effective rate over a rolling window. Dropping samples and hearing
        // nothing are indistinguishable in the message count; this is what
        // tells them apart.
        rate_window_samples += samples;
        if rate_window_start.elapsed() >= Duration::from_secs(5) {
            let achieved = rate_window_samples as f64 / rate_window_start.elapsed().as_secs_f64();
            counters
                .effective_rate_milli
                .store((achieved * 1000.0) as u64, Ordering::Relaxed);
            rate_window_start = Instant::now();
            rate_window_samples = 0;
        }

        found.clear();
        pipeline.feed(&buf[..n], &mut found);

        let tick = Tick::now();
        for validated in &found {
            let Some(frame) = validated.frame() else {
                continue;
            };
            let rssi = validated
                .noise
                .checked_sub(0)
                .filter(|_| validated.noise > 0)
                .map(|_| 20.0 * (f32::from(validated.signal) / f32::from(u16::MAX)).log10());

            counters
                .last_message_ms
                .store(tick.wall_ms as u64, Ordering::Relaxed);

            match tracker.observe(&frame, tick, rssi) {
                Update::NewPosition(fix) => {
                    counters.positions.fetch_add(1, Ordering::Relaxed);
                    if let Some(store) = &store
                        && let Some(aircraft) = tracker.get(frame.icao())
                    {
                        let key = aircraft.icao.to_string();
                        let altitude = aircraft.altitude.and_then(|a| a.feet());
                        let previous = last_written.get(&key).copied();
                        if movement.should_write(previous, fix.lat, fix.lon, altitude, fix.at_ms) {
                            store.submit(Record::position(aircraft, fix));
                            store.submit(Record::aircraft(aircraft));
                            last_written.insert(key, (fix.lat, fix.lon, altitude, fix.at_ms));
                        }
                    }
                }
                Update::Updated | Update::Ignored => {}
            }
        }

        counters
            .source_overrun_bytes
            .store(source.overruns(), Ordering::Relaxed);

        let stats = pipeline.stats();
        counters
            .candidates
            .store(stats.candidates, Ordering::Relaxed);
        counters.crc_ok.store(stats.crc_ok, Ordering::Relaxed);
        counters.crc_fail.store(stats.crc_fail, Ordering::Relaxed);

        // Publish a snapshot at a human rate, not a sample rate.
        if last_publish.elapsed() >= Duration::from_millis(500) {
            let generation = station.generation();
            if generation != station_generation {
                station_generation = generation;
                let coords = station.coords();
                if tracker.set_receiver(coords) {
                    match coords {
                        Some((lat, lon)) => tracing::info!(
                            lat,
                            lon,
                            "receiver position changed; the range gate follows it from here"
                        ),
                        None => tracing::warn!(
                            "receiver position cleared; the range gate is now disabled"
                        ),
                    }
                }
            }

            tracker.expire(tick);
            snapshot.store(Arc::new(tracker.snapshot(tick)));
            last_publish = Instant::now();
        }
    }

    // One last publish so the API is not left showing stale data.
    let tick = Tick::now();
    snapshot.store(Arc::new(tracker.snapshot(tick)));
    counters.streaming.store(false, Ordering::Relaxed);
    tracing::info!(stats = ?tracker.stats(), "decoder stopped");
}

async fn serve(
    state: Arc<AppState>,
    bind: &str,
    cors_origins: &[String],
    shutdown: Arc<AtomicBool>,
    duration_s: Option<u64>,
) -> anyhow::Result<()> {
    use tower_http::cors::{Any, CorsLayer};

    // An allowlist, not a wildcard. Permissive CORS on a LAN box is sloppy,
    // and someone in a conference audience will open the network tab.
    let cors = if cors_origins.iter().any(|o| o == "*") {
        CorsLayer::new().allow_origin(Any).allow_methods(Any)
    } else {
        let origins: Vec<_> = cors_origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new().allow_origin(origins).allow_methods(Any)
    };

    let app = api::router(state).layer(cors);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address = %bind, "API listening");

    let signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let deadline = async {
            match duration_s {
                Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = ctrl_c => tracing::info!("interrupted"),
            _ = deadline => tracing::info!("duration elapsed"),
        }
        shutdown.store(true, Ordering::Relaxed);
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(signal)
        .await?;
    Ok(())
}

fn init_logging(config: &Config) {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // stdout, so journald captures it and `journalctl -u skyward` is the one
    // command the runbook needs.
    if config.log_format.as_str() == "json" {
        fmt().json().with_env_filter(filter).init();
    } else {
        fmt().with_env_filter(filter).init();
    }
}
