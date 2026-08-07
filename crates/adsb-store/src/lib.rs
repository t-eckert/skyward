//! Durable history.
//!
//! Live state lives in memory ([`adsb_track::Snapshot`]); this crate is only
//! for the past — tracks you want to draw after the aircraft has gone.
//!
//! # Three things the old implementation got wrong
//!
//! **One SQLite write per message.** At 95 messages a minute that is fine. At
//! the thousands a minute a better decoder produces, on an SD card, it is not.
//! Writes are batched into transactions here.
//!
//! **The DSP thread could block on disk.** If storage is slow, the thing that
//! must not stop is sample processing — a stalled decoder loses samples
//! permanently, whereas a dropped database row loses one dot on a map. So the
//! queue is **bounded** and full means *drop and count*, never *wait*. That is
//! a deliberate trade, and it is measured rather than silent.
//!
//! **`positions` grew forever.** Two mechanisms now bound it: rows are only
//! written when the aircraft has actually moved, and a retention sweep deletes
//! old ones. With `auto_vacuum = INCREMENTAL` set at creation, the freed pages
//! are returned to the filesystem.
//!
//! **Errors were discarded.** The old code wrote `let _ = db.insert(...)`. On
//! a machine you cannot log into, a silently failing database is invisible
//! until you notice the map has no history. Failures are counted and surfaced
//! in `/healthz` as `degraded`.

pub mod schema;

use adsb_track::{Aircraft, Fix};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use rusqlite;
pub use schema::{SCHEMA_VERSION, open_readonly, open_writable, schema_version};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store configuration: {0}")]
    Config(String),
}

/// One row destined for the database.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Aircraft {
        icao: String,
        callsign: Option<String>,
        first_seen_ms: i64,
        last_seen_ms: i64,
        messages: u64,
    },
    Position {
        icao: String,
        ts_ms: i64,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        ground_speed_kt: Option<u16>,
        vertical_rate_fpm: Option<i32>,
        track_deg: Option<f32>,
        source: &'static str,
    },
}

impl Record {
    /// Build a position row from tracker state.
    pub fn position(aircraft: &Aircraft, fix: Fix) -> Record {
        Record::Position {
            icao: aircraft.icao.to_string(),
            ts_ms: fix.at_ms,
            lat: fix.lat,
            lon: fix.lon,
            altitude_ft: aircraft.altitude.and_then(|a| a.feet()),
            ground_speed_kt: aircraft.ground_speed_kt,
            vertical_rate_fpm: aircraft.vertical_rate_fpm,
            track_deg: aircraft.track.map(|t| t.0),
            source: fix.source.as_str(),
        }
    }

    /// Build an aircraft upsert from tracker state.
    pub fn aircraft(aircraft: &Aircraft) -> Record {
        Record::Aircraft {
            icao: aircraft.icao.to_string(),
            callsign: aircraft.callsign.clone(),
            first_seen_ms: aircraft.first_seen_ms,
            last_seen_ms: aircraft.last_seen_ms,
            messages: aircraft.messages,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub path: String,
    /// Rows per transaction.
    pub batch_size: usize,
    /// Longest a row waits before being committed anyway.
    pub flush_interval: Duration,
    /// Queue depth. Full means drop, never block.
    pub queue_capacity: usize,
    /// How long history is kept. `None` keeps everything, which will
    /// eventually fill the disk.
    pub retention: Option<Duration>,
    pub retention_sweep_interval: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        StoreConfig {
            path: "skyward.db".to_string(),
            batch_size: 500,
            flush_interval: Duration::from_secs(1),
            queue_capacity: 10_000,
            retention: Some(Duration::from_secs(24 * 3600)),
            retention_sweep_interval: Duration::from_secs(3600),
        }
    }
}

/// Counters the health endpoint reports.
#[derive(Debug, Default)]
pub struct StoreMetrics {
    pub queued: AtomicU64,
    /// Rows thrown away because the queue was full. Non-zero means storage
    /// cannot keep up; the decoder deliberately kept running.
    pub dropped: AtomicU64,
    pub written: AtomicU64,
    pub transactions: AtomicU64,
    /// Write failures. The old code discarded these entirely.
    pub errors: AtomicU64,
    pub retention_deleted: AtomicU64,
}

impl StoreMetrics {
    pub fn snapshot(&self) -> StoreMetricsSnapshot {
        StoreMetricsSnapshot {
            queued: self.queued.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            transactions: self.transactions.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            retention_deleted: self.retention_deleted.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreMetricsSnapshot {
    pub queued: u64,
    pub dropped: u64,
    pub written: u64,
    pub transactions: u64,
    pub errors: u64,
    pub retention_deleted: u64,
}

impl StoreMetricsSnapshot {
    /// Whether the health endpoint should report `degraded`.
    pub fn is_degraded(&self) -> bool {
        self.errors > 0 || self.dropped > 0
    }
}

/// A handle the decoder writes through. Cloneable and never blocks.
#[derive(Clone)]
pub struct StoreHandle {
    tx: SyncSender<Record>,
    metrics: Arc<StoreMetrics>,
}

impl StoreHandle {
    /// Queue a row. Returns false if it was dropped because the queue is full.
    ///
    /// Deliberately infallible from the caller's perspective: there is nothing
    /// useful a DSP thread can do about a slow disk except keep decoding.
    pub fn submit(&self, record: Record) -> bool {
        match self.tx.try_send(record) {
            Ok(()) => {
                self.metrics.queued.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) => {
                self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn metrics(&self) -> StoreMetricsSnapshot {
        self.metrics.snapshot()
    }
}

/// Owns the writer thread. Dropping it flushes and shuts down cleanly.
pub struct Store {
    handle: StoreHandle,
    worker: Option<JoinHandle<()>>,
    config: StoreConfig,
}

impl Store {
    pub fn open(config: StoreConfig) -> Result<Store, StoreError> {
        // Fail here, on the calling thread, so a bad path is a startup error
        // rather than a thread that dies quietly a moment later.
        let mut conn = open_writable(&config.path)?;
        if let Some(found) = schema_version(&conn)?
            && found != SCHEMA_VERSION
        {
            return Err(StoreError::Config(format!(
                "database {} has schema version {found}, this build expects {SCHEMA_VERSION}",
                config.path
            )));
        }

        let (tx, rx) = sync_channel::<Record>(config.queue_capacity);
        let metrics = Arc::new(StoreMetrics::default());
        let worker_metrics = Arc::clone(&metrics);
        let worker_config = config.clone();

        let worker = std::thread::Builder::new()
            .name("skyward-store".into())
            .spawn(move || {
                let mut pending: Vec<Record> = Vec::with_capacity(worker_config.batch_size);
                let mut last_flush = Instant::now();
                let mut last_sweep = Instant::now();

                loop {
                    match rx.recv_timeout(worker_config.flush_interval) {
                        Ok(record) => {
                            worker_metrics.queued.fetch_sub(1, Ordering::Relaxed);
                            pending.push(record);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        // Every handle dropped: flush and stop.
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            flush(&mut conn, &mut pending, &worker_metrics);
                            return;
                        }
                    }

                    let due = last_flush.elapsed() >= worker_config.flush_interval;
                    if pending.len() >= worker_config.batch_size || (due && !pending.is_empty()) {
                        flush(&mut conn, &mut pending, &worker_metrics);
                        last_flush = Instant::now();
                    }

                    if let Some(retention) = worker_config.retention
                        && last_sweep.elapsed() >= worker_config.retention_sweep_interval
                    {
                        sweep(&conn, retention, &worker_metrics);
                        last_sweep = Instant::now();
                    }
                }
            })
            .map_err(|e| StoreError::Config(format!("cannot start writer thread: {e}")))?;

        Ok(Store {
            handle: StoreHandle { tx, metrics },
            worker: Some(worker),
            config,
        })
    }

    pub fn handle(&self) -> StoreHandle {
        self.handle.clone()
    }

    pub fn metrics(&self) -> StoreMetricsSnapshot {
        self.handle.metrics()
    }

    pub fn path(&self) -> &str {
        &self.config.path
    }

    /// Bytes on disk, main file plus WAL.
    pub fn size_bytes(&self) -> u64 {
        let main = std::fs::metadata(&self.config.path)
            .map(|m| m.len())
            .unwrap_or(0);
        let wal = std::fs::metadata(format!("{}-wal", self.config.path))
            .map(|m| m.len())
            .unwrap_or(0);
        main + wal
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Replace the sender with a dead one so the worker sees Disconnected.
        let (dead, _) = sync_channel(1);
        self.handle.tx = dead;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Commit a batch in one transaction.
fn flush(conn: &mut rusqlite::Connection, pending: &mut Vec<Record>, metrics: &StoreMetrics) {
    if pending.is_empty() {
        return;
    }
    let count = pending.len() as u64;

    let result = (|| -> rusqlite::Result<()> {
        let tx = conn.transaction()?;
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO aircraft(icao, callsign, first_seen_ms, last_seen_ms, messages)
                 VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(icao) DO UPDATE SET
                   -- COALESCE so a later message without a callsign does not
                   -- erase one we already learned.
                   callsign     = COALESCE(excluded.callsign, aircraft.callsign),
                   last_seen_ms = MAX(excluded.last_seen_ms, aircraft.last_seen_ms),
                   messages     = MAX(excluded.messages, aircraft.messages)",
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO positions(icao, ts_ms, lat, lon, altitude_ft,
                                       ground_speed_kt, vertical_rate_fpm, track_deg, source)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;

            for record in pending.iter() {
                match record {
                    Record::Aircraft {
                        icao,
                        callsign,
                        first_seen_ms,
                        last_seen_ms,
                        messages,
                    } => {
                        upsert.execute(rusqlite::params![
                            icao,
                            callsign,
                            first_seen_ms,
                            last_seen_ms,
                            *messages as i64
                        ])?;
                    }
                    Record::Position {
                        icao,
                        ts_ms,
                        lat,
                        lon,
                        altitude_ft,
                        ground_speed_kt,
                        vertical_rate_fpm,
                        track_deg,
                        source,
                    } => {
                        insert.execute(rusqlite::params![
                            icao,
                            ts_ms,
                            lat,
                            lon,
                            altitude_ft,
                            ground_speed_kt.map(i64::from),
                            vertical_rate_fpm,
                            track_deg,
                            source
                        ])?;
                    }
                }
            }
        }
        tx.commit()
    })();

    match result {
        Ok(()) => {
            metrics.written.fetch_add(count, Ordering::Relaxed);
            metrics.transactions.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            // Counted, not discarded -- this is what surfaces as `degraded`.
            metrics.errors.fetch_add(count, Ordering::Relaxed);
        }
    }
    pending.clear();
}

/// Delete history older than the retention window and return the pages.
fn sweep(conn: &rusqlite::Connection, retention: Duration, metrics: &StoreMetrics) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let cutoff = now_ms - retention.as_millis() as i64;

    match conn.execute("DELETE FROM positions WHERE ts_ms < ?1", [cutoff]) {
        Ok(deleted) => {
            metrics
                .retention_deleted
                .fetch_add(deleted as u64, Ordering::Relaxed);
            // Hand pages back to the filesystem a little at a time, rather
            // than a full VACUUM that would lock the database for minutes.
            let _ = conn.pragma_update(None, "incremental_vacuum", 1000);
        }
        Err(_) => {
            metrics.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Decides whether a new fix is worth a row.
///
/// Writing every position would fill the disk with duplicates of a parked
/// aircraft. Writing only on movement keeps the track shape while cutting
/// volume by an order of magnitude on the ground.
#[derive(Clone, Copy, Debug)]
pub struct MovementFilter {
    pub min_move_km: f64,
    pub min_altitude_change_ft: i32,
    pub max_interval_ms: i64,
}

impl Default for MovementFilter {
    fn default() -> Self {
        MovementFilter {
            min_move_km: 0.05,
            min_altitude_change_ft: 100,
            max_interval_ms: 5_000,
        }
    }
}

impl MovementFilter {
    /// True when this fix should be persisted.
    pub fn should_write(
        &self,
        previous: Option<(f64, f64, Option<i32>, i64)>,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        ts_ms: i64,
    ) -> bool {
        let Some((plat, plon, palt, pts)) = previous else {
            return true; // always keep the first fix
        };
        if ts_ms - pts >= self.max_interval_ms {
            return true; // a heartbeat, so a parked aircraft still has a track
        }
        if adsb_core::cpr::haversine_km(plat, plon, lat, lon) >= self.min_move_km {
            return true;
        }
        match (palt, altitude_ft) {
            (Some(a), Some(b)) => (a - b).abs() >= self.min_altitude_change_ft,
            (None, Some(_)) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> String {
        let dir = std::env::temp_dir().join("skyward-store-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.db"));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        path.to_string_lossy().into_owned()
    }

    fn position(icao: &str, ts_ms: i64, lat: f64) -> Record {
        Record::Position {
            icao: icao.into(),
            ts_ms,
            lat,
            lon: -75.6,
            altitude_ft: Some(24_000),
            ground_speed_kt: Some(400),
            vertical_rate_fpm: Some(0),
            track_deg: Some(250.0),
            source: "global_cpr",
        }
    }

    fn count(path: &str, table: &str) -> i64 {
        let conn = open_readonly(path).unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn rows_are_written_and_readable() {
        let path = temp_path("write");
        {
            let store = Store::open(StoreConfig {
                path: path.clone(),
                flush_interval: Duration::from_millis(20),
                ..Default::default()
            })
            .unwrap();
            let h = store.handle();
            h.submit(Record::Aircraft {
                icao: "C060B6".into(),
                callsign: Some("ACA123".into()),
                first_seen_ms: 1000,
                last_seen_ms: 2000,
                messages: 42,
            });
            for i in 0..10 {
                h.submit(position(
                    "C060B6",
                    1000 + i * 100,
                    45.3 + f64::from(i as u32) * 0.001,
                ));
            }
            // Drop flushes and joins.
        }

        assert_eq!(count(&path, "aircraft"), 1);
        assert_eq!(count(&path, "positions"), 10);

        let conn = open_readonly(&path).unwrap();
        let callsign: String = conn
            .query_row(
                "SELECT callsign FROM aircraft WHERE icao='C060B6'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(callsign, "ACA123");
    }

    #[test]
    fn writes_are_batched_not_one_transaction_per_row() {
        let path = temp_path("batched");
        let store = Store::open(StoreConfig {
            path: path.clone(),
            batch_size: 100,
            flush_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let h = store.handle();
        for i in 0..500 {
            h.submit(position("C060B6", i, 45.3));
        }
        // Give the worker time to drain.
        for _ in 0..100 {
            if store.metrics().written >= 500 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let m = store.metrics();
        assert_eq!(m.written, 500);
        assert!(
            m.transactions <= 10,
            "500 rows took {} transactions; batching is not working",
            m.transactions
        );
    }

    /// The backpressure decision, made explicit: a full queue must never stall
    /// the caller, because the caller is the DSP thread.
    #[test]
    fn a_full_queue_drops_rather_than_blocking() {
        let path = temp_path("backpressure");
        let store = Store::open(StoreConfig {
            path: path.clone(),
            queue_capacity: 8,
            // Long flush interval so the worker stays asleep and the queue fills.
            flush_interval: Duration::from_secs(30),
            batch_size: 100_000,
            ..Default::default()
        })
        .unwrap();
        let h = store.handle();

        let started = Instant::now();
        for i in 0..5_000 {
            h.submit(position("C060B6", i, 45.3));
        }
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "submit blocked for {elapsed:?}; the DSP thread would have stalled"
        );
        let m = store.metrics();
        assert!(m.dropped > 0, "expected drops with a queue of 8");
        assert!(m.is_degraded(), "drops should show up as degraded");
    }

    #[test]
    fn callsign_is_not_erased_by_a_later_message_without_one() {
        let path = temp_path("callsign");
        {
            let store = Store::open(StoreConfig {
                path: path.clone(),
                flush_interval: Duration::from_millis(20),
                ..Default::default()
            })
            .unwrap();
            let h = store.handle();
            h.submit(Record::Aircraft {
                icao: "C060B6".into(),
                callsign: Some("ACA123".into()),
                first_seen_ms: 1,
                last_seen_ms: 2,
                messages: 1,
            });
            h.submit(Record::Aircraft {
                icao: "C060B6".into(),
                callsign: None,
                first_seen_ms: 1,
                last_seen_ms: 9,
                messages: 5,
            });
        }
        let conn = open_readonly(&path).unwrap();
        let callsign: Option<String> = conn
            .query_row(
                "SELECT callsign FROM aircraft WHERE icao='C060B6'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(callsign.as_deref(), Some("ACA123"));
    }

    #[test]
    fn retention_deletes_old_positions() {
        let path = temp_path("retention");
        let conn = open_writable(&path).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        for (i, age_hours) in [50i64, 30, 10, 1].iter().enumerate() {
            conn.execute(
                "INSERT INTO positions(icao, ts_ms, lat, lon, source)
                 VALUES(?1, ?2, 45.0, -75.0, 'global_cpr')",
                rusqlite::params![format!("AAA00{i}"), now_ms - age_hours * 3_600_000],
            )
            .unwrap();
        }
        assert_eq!(count(&path, "positions"), 4);

        let metrics = StoreMetrics::default();
        sweep(&conn, Duration::from_secs(24 * 3600), &metrics);
        drop(conn);

        // The 50h and 30h rows go; the 10h and 1h rows stay.
        assert_eq!(count(&path, "positions"), 2);
        assert_eq!(metrics.retention_deleted.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_mismatched_schema_version_is_refused() {
        let path = temp_path("schema-mismatch");
        {
            let conn = open_writable(&path).unwrap();
            conn.execute(
                "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        }
        let err = Store::open(StoreConfig {
            path: path.clone(),
            ..Default::default()
        })
        .err()
        .expect("should refuse an unknown schema");
        assert!(err.to_string().contains("99"), "{err}");
    }

    #[test]
    fn movement_filter_keeps_the_first_fix() {
        let f = MovementFilter::default();
        assert!(f.should_write(None, 45.0, -75.0, Some(1000), 0));
    }

    #[test]
    fn movement_filter_skips_a_stationary_aircraft() {
        let f = MovementFilter::default();
        // Same place, same altitude, one second later.
        assert!(!f.should_write(
            Some((45.0, -75.0, Some(1000), 0)),
            45.0,
            -75.0,
            Some(1000),
            1000
        ));
    }

    #[test]
    fn movement_filter_writes_when_the_aircraft_moves() {
        let f = MovementFilter::default();
        // ~110 m north.
        assert!(f.should_write(
            Some((45.0, -75.0, Some(1000), 0)),
            45.001,
            -75.0,
            Some(1000),
            500
        ));
    }

    #[test]
    fn movement_filter_writes_on_altitude_change() {
        let f = MovementFilter::default();
        assert!(f.should_write(
            Some((45.0, -75.0, Some(1000), 0)),
            45.0,
            -75.0,
            Some(1200),
            500
        ));
    }

    /// Even a parked aircraft should leave a heartbeat, or its track vanishes.
    #[test]
    fn movement_filter_writes_periodically_regardless() {
        let f = MovementFilter::default();
        assert!(f.should_write(
            Some((45.0, -75.0, Some(1000), 0)),
            45.0,
            -75.0,
            Some(1000),
            6_000
        ));
    }
}
