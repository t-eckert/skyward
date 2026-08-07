//! An immutable view of the aircraft set.
//!
//! The API serves snapshots rather than reaching into live state. The decoder
//! thread publishes a new one periodically; readers take the current one and
//! are never blocked by, and never block, decoding.
//!
//! The old design had every HTTP request take an async mutex around a blocking
//! SQLite connection, which serialized requests *and* parked a tokio worker on
//! disk I/O. Live data does not need a database at all — it is already in
//! memory.
//!
//! # Wire shape
//!
//! These types are the API contract, so the naming is deliberate:
//!
//! - **`now_ms` travels in the envelope.** Clients compute ages against the
//!   *server's* clock. Otherwise a laptop 40 seconds out of sync shows every
//!   aircraft as stale.
//! - **Epoch milliseconds, never formatted strings.** The old schema stored
//!   `"2026-08-06 12:00:00"` — no `T`, no `Z` — which V8 parses as *local* time
//!   and older Safari refuses entirely.
//! - **Units in the field names**: `altitude_ft`, `ground_speed_kt`, `track_deg`.
//! - **`track_deg`, not `heading`.** ADS-B velocity reports ground track. They
//!   differ by the wind correction angle, sometimes by 15 degrees.

use crate::{Aircraft, Tick};

/// One aircraft, flattened for serving.
#[derive(Clone, Debug, PartialEq)]
pub struct AircraftView {
    pub icao: String,
    pub callsign: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// How long ago the position was resolved. Lets a client fade a stale dot
    /// rather than drawing it as current.
    pub position_age_ms: Option<i64>,
    pub position_source: Option<&'static str>,
    pub altitude_ft: Option<i32>,
    pub altitude_source: Option<&'static str>,
    pub ground_speed_kt: Option<u16>,
    pub track_deg: Option<f32>,
    pub vertical_rate_fpm: Option<i32>,
    pub on_ground: bool,
    pub category: Option<String>,
    pub messages: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub seen_ms: i64,
    pub rssi_dbfs: Option<f32>,
}

impl AircraftView {
    fn from(aircraft: &Aircraft, now_ms: i64) -> Self {
        use adsb_core::units::Altitude;

        let (altitude_ft, altitude_source) = match aircraft.altitude {
            Some(Altitude::Barometric(ft)) => (Some(ft), Some("baro")),
            Some(Altitude::Gnss(ft)) => (Some(ft), Some("gnss")),
            _ => (None, None),
        };

        AircraftView {
            icao: aircraft.icao.to_string(),
            callsign: aircraft.callsign.clone(),
            lat: aircraft.position.map(|p| p.lat),
            lon: aircraft.position.map(|p| p.lon),
            // Clamp at zero: a clock step must not produce a negative age.
            position_age_ms: aircraft.position.map(|p| (now_ms - p.at_ms).max(0)),
            position_source: aircraft.position.map(|p| p.source.as_str()),
            altitude_ft,
            altitude_source,
            ground_speed_kt: aircraft.ground_speed_kt,
            track_deg: aircraft.track.map(|t| t.0),
            vertical_rate_fpm: aircraft.vertical_rate_fpm,
            on_ground: aircraft.on_ground,
            category: aircraft.category.map(|(tc, ca)| format!("{tc}-{ca}")),
            messages: aircraft.messages,
            first_seen_ms: aircraft.first_seen_ms,
            last_seen_ms: aircraft.last_seen_ms,
            seen_ms: (now_ms - aircraft.last_seen_ms).max(0),
            rssi_dbfs: aircraft.rssi_dbfs,
        }
    }
}

/// Everything the API needs to answer `/api/v1/aircraft`.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// The server's clock at the moment this was built.
    pub now_ms: i64,
    pub aircraft: Vec<AircraftView>,
}

impl Snapshot {
    pub fn build<'a>(aircraft: impl Iterator<Item = &'a Aircraft>, tick: Tick) -> Snapshot {
        let mut views: Vec<AircraftView> = aircraft
            .map(|a| AircraftView::from(a, tick.wall_ms))
            .collect();

        // Locatable aircraft first, then most recently heard. A client that
        // renders only the first N gets the useful ones.
        views.sort_by(|a, b| {
            b.lat
                .is_some()
                .cmp(&a.lat.is_some())
                .then(a.seen_ms.cmp(&b.seen_ms))
                .then(a.icao.cmp(&b.icao))
        });

        Snapshot {
            now_ms: tick.wall_ms,
            aircraft: views,
        }
    }

    pub fn empty(tick: Tick) -> Snapshot {
        Snapshot {
            now_ms: tick.wall_ms,
            aircraft: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.aircraft.len()
    }

    pub fn is_empty(&self) -> bool {
        self.aircraft.is_empty()
    }

    pub fn with_position(&self) -> impl Iterator<Item = &AircraftView> {
        self.aircraft.iter().filter(|a| a.lat.is_some())
    }

    pub fn find(&self, icao: &str) -> Option<&AircraftView> {
        let wanted = icao.to_ascii_uppercase();
        self.aircraft.iter().find(|a| a.icao == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tracker, TrackerConfig};
    use adsb_core::Frame;
    use std::time::Duration;

    fn tracker_with_traffic() -> (Tracker, Tick) {
        let mut t = Tracker::new(TrackerConfig::default());
        let tick = Tick::now();
        for hex in [
            "8D4840D6202CC371C32CE0576098", // ident only, no position
            "8D40621D58C382D690C8AC2863A7", // position even
            "8D40621D58C386435CC412692AD6", // position odd -> a fix
        ] {
            let bytes = adsb_core::hex_to_bytes(hex).unwrap();
            let frame = Frame::new(&bytes).unwrap();
            t.observe(&frame, tick, Some(-20.0));
        }
        (t, tick)
    }

    #[test]
    fn locatable_aircraft_sort_first() {
        let (t, tick) = tracker_with_traffic();
        let snap = t.snapshot(tick);
        assert_eq!(snap.len(), 2);
        assert!(snap.aircraft[0].lat.is_some(), "positioned aircraft first");
        assert_eq!(snap.aircraft[0].icao, "40621D");
        assert!(snap.aircraft[1].lat.is_none());
    }

    #[test]
    fn the_envelope_carries_the_server_clock() {
        let (t, tick) = tracker_with_traffic();
        let snap = t.snapshot(tick);
        assert_eq!(snap.now_ms, tick.wall_ms);
    }

    #[test]
    fn ages_are_relative_to_the_snapshot_not_the_client() {
        let (t, tick) = tracker_with_traffic();
        let later = Tick {
            mono: tick.mono + Duration::from_secs(5),
            wall_ms: tick.wall_ms + 5_000,
        };
        let snap = t.snapshot(later);
        let positioned = snap.with_position().next().unwrap();
        assert_eq!(positioned.seen_ms, 5_000);
        assert_eq!(positioned.position_age_ms, Some(5_000));
    }

    /// A wall-clock step backwards must not produce negative ages, which a
    /// client would render as an aircraft seen in the future.
    #[test]
    fn ages_never_go_negative() {
        let (t, tick) = tracker_with_traffic();
        let earlier = Tick {
            mono: tick.mono,
            wall_ms: tick.wall_ms - 100_000,
        };
        let snap = t.snapshot(earlier);
        for a in &snap.aircraft {
            assert!(a.seen_ms >= 0, "{} had age {}", a.icao, a.seen_ms);
            assert!(a.position_age_ms.unwrap_or(0) >= 0);
        }
    }

    #[test]
    fn field_names_carry_their_units() {
        let (t, tick) = tracker_with_traffic();
        let snap = t.snapshot(tick);
        let a = snap
            .find("40621d")
            .expect("lookup should be case-insensitive");
        assert_eq!(a.altitude_ft, Some(38000));
        assert_eq!(a.altitude_source, Some("baro"));
        assert_eq!(a.position_source, Some("global_cpr"));
    }

    #[test]
    fn icao_is_rendered_as_six_uppercase_hex() {
        let (t, tick) = tracker_with_traffic();
        for a in &t.snapshot(tick).aircraft {
            assert_eq!(a.icao.len(), 6, "{}", a.icao);
            assert!(
                a.icao
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_lowercase())
            );
        }
    }

    #[test]
    fn an_empty_tracker_yields_an_empty_snapshot() {
        let t = Tracker::new(TrackerConfig::default());
        let snap = t.snapshot(Tick::now());
        assert!(snap.is_empty());
        assert_eq!(snap.with_position().count(), 0);
    }
}
