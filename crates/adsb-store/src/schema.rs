//! Database schema and connection setup.
//!
//! # Two decisions that cannot be changed later
//!
//! **`auto_vacuum = INCREMENTAL` must be set before the first table exists.**
//! Switching afterwards requires a full `VACUUM`, which rewrites the entire
//! database — on a Pi with a 40 GB history on an SD card, that is not a thing
//! you get to do casually. Without it, deleted rows leave free pages that the
//! file never gives back, so retention frees nothing on disk.
//!
//! **Timestamps are `INTEGER` epoch milliseconds, not text.** The old schema
//! used `datetime('now')`, which produces `"2026-08-06 12:00:00"` — no `T`, no
//! `Z`. V8 parses that as *local* time and older Safari returns `Invalid Date`,
//! so it is a guaranteed bug in the SvelteKit client. Second resolution also
//! loses ordering when positions arrive twice a second.

use rusqlite::Connection;

/// Bumped when the schema changes in a way that matters. The server refuses to
/// start against a database it does not understand rather than corrupting it.
pub const SCHEMA_VERSION: i64 = 1;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS aircraft (
    icao          TEXT PRIMARY KEY,
    callsign      TEXT,
    first_seen_ms INTEGER NOT NULL,
    last_seen_ms  INTEGER NOT NULL,
    messages      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS positions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    icao              TEXT    NOT NULL,
    ts_ms             INTEGER NOT NULL,
    lat               REAL    NOT NULL,
    lon               REAL    NOT NULL,
    altitude_ft       INTEGER,
    ground_speed_kt   INTEGER,
    vertical_rate_fpm INTEGER,
    track_deg         REAL,
    source            TEXT    NOT NULL
);

-- One composite index, not two single-column ones. Every history query is
-- "this aircraft, ordered by time", which a (icao, ts_ms) index serves
-- directly; separate indexes on icao and ts_ms each serve half of it and
-- SQLite can only use one.
CREATE INDEX IF NOT EXISTS idx_positions_icao_ts ON positions(icao, ts_ms);

-- Retention sweeps delete by age across all aircraft.
CREATE INDEX IF NOT EXISTS idx_positions_ts ON positions(ts_ms);
"#;

/// Open a writable connection and make sure the schema is present.
pub fn open_writable(path: &str) -> rusqlite::Result<Connection> {
    let fresh = path == ":memory:" || !std::path::Path::new(path).exists();
    let conn = Connection::open(path)?;

    // Order matters. auto_vacuum has to be decided while the database is still
    // empty, so it goes first, before anything can create a page.
    if fresh {
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    }

    // WAL is what lets readers work while the writer is busy.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL trades a theoretical loss of the last transaction on power cut
    // for far fewer fsyncs, which is the difference between an SD card
    // surviving months and surviving years.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    // Cap the WAL so it cannot grow without bound between checkpoints.
    conn.pragma_update(None, "journal_size_limit", 32 * 1024 * 1024)?;

    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO NOTHING",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(conn)
}

/// Open a read-only connection for serving history.
pub fn open_readonly(path: &str) -> rusqlite::Result<Connection> {
    use rusqlite::OpenFlags;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "query_only", true)?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    Ok(conn)
}

/// The schema version recorded in the file, if any.
pub fn schema_version(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = 'schema_version'")?;
    let mut rows = stmt.query([])?;
    match rows.next()? {
        Some(row) => {
            let text: String = row.get(0)?;
            Ok(text.parse().ok())
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_database_gets_incremental_auto_vacuum() {
        let dir = std::env::temp_dir().join("skyward-schema-test-vacuum");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);

        let conn = open_writable(path.to_str().unwrap()).unwrap();
        let mode: i64 = conn
            .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
            .unwrap();
        // 0 = NONE, 1 = FULL, 2 = INCREMENTAL. Getting this wrong on a fresh
        // database cannot be fixed later without a full VACUUM.
        assert_eq!(mode, 2, "auto_vacuum should be INCREMENTAL");

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wal_is_enabled_so_readers_do_not_block_the_writer() {
        let conn = open_writable(":memory:").unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        // An in-memory database reports "memory"; a file reports "wal".
        assert!(mode == "wal" || mode == "memory", "got {mode}");
    }

    #[test]
    fn schema_version_is_recorded() {
        let conn = open_writable(":memory:").unwrap();
        assert_eq!(schema_version(&conn).unwrap(), Some(SCHEMA_VERSION));
    }

    #[test]
    fn the_composite_index_exists() {
        let conn = open_writable(":memory:").unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_positions_icao_ts'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// The history query must actually use the composite index, or a Pi with
    /// a few million rows will grind.
    #[test]
    fn the_track_query_uses_the_index() {
        let conn = open_writable(":memory:").unwrap();
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN
                 SELECT ts_ms, lat, lon FROM positions
                 WHERE icao = 'ABC123' AND ts_ms >= 0 ORDER BY ts_ms",
                [],
                |r| r.get(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_positions_icao_ts"),
            "query plan did not use the composite index: {plan}"
        );
    }

    #[test]
    fn opening_twice_is_idempotent() {
        let dir = std::env::temp_dir().join("skyward-schema-test-idem");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let _ = std::fs::remove_file(&path);
        let p = path.to_str().unwrap();

        drop(open_writable(p).unwrap());
        let conn = open_writable(p).unwrap();
        assert_eq!(schema_version(&conn).unwrap(), Some(SCHEMA_VERSION));

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }
}
