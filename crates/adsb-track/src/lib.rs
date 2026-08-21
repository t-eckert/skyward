//! Turning a stream of messages into a set of aircraft.
//!
//! A decoded message is a fact about one instant. An aircraft is an
//! accumulation: a callsign from one message, an altitude from another, a
//! position that needs two, all of it decaying as the aircraft flies out of
//! range. This crate holds that state.
//!
//! # Ghost aircraft
//!
//! The formats that carry the address XORed into their parity — DF0, 4, 5, 16,
//! 20, 21 — cannot be validated. The CRC remainder of *noise* is a perfectly
//! plausible six-hex-digit address, so accepting them blindly populates the
//! map with aircraft that do not exist.
//!
//! The defence is [`Tracker::verified`]: an address is only real once a
//! CRC-clean DF17 or DF18 has proved it. Everything else is admitted only for
//! addresses already on that list. It is a heuristic, and it is the same one
//! the old implementation used, but here it is explicit and counted.

pub mod position;
pub mod snapshot;

use adsb_core::{
    Frame, Icao, Message,
    cpr::CprKind,
    units::{Altitude, TrackDeg},
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub use position::{
    CprState, Fix, Gates, GlobalCprSolver, PositionOutcome, PositionSolver, PositionSource,
    RejectReason,
};
pub use snapshot::{AircraftView, Snapshot};

/// Wall-clock and monotonic time, together.
///
/// Both are needed and they are not interchangeable. Monotonic time drives
/// CPR pairing, because it cannot jump; wall time is what gets stored and
/// served, because it is what a human and a client understand.
#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub mono: Instant,
    pub wall_ms: i64,
}

impl Tick {
    pub fn now() -> Self {
        Tick {
            mono: Instant::now(),
            wall_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        }
    }
}

/// Everything known about one aircraft.
#[derive(Clone, Debug)]
pub struct Aircraft {
    pub icao: Icao,
    pub callsign: Option<String>,
    pub altitude: Option<Altitude>,
    pub position: Option<Fix>,
    pub ground_speed_kt: Option<u16>,
    /// Ground track, not heading. See `adsb_core::units::TrackDeg`.
    pub track: Option<TrackDeg>,
    pub vertical_rate_fpm: Option<i32>,
    pub on_ground: bool,
    pub category: Option<(u8, u8)>,
    pub messages: u64,
    pub position_messages: u64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    /// Most recent signal strength, dB relative to full scale.
    pub rssi_dbfs: Option<f32>,
    cpr: CprState,
    last_mono: Instant,
}

impl Aircraft {
    fn new(icao: Icao, tick: Tick) -> Self {
        Aircraft {
            icao,
            callsign: None,
            altitude: None,
            position: None,
            ground_speed_kt: None,
            track: None,
            vertical_rate_fpm: None,
            on_ground: false,
            category: None,
            messages: 0,
            position_messages: 0,
            first_seen_ms: tick.wall_ms,
            last_seen_ms: tick.wall_ms,
            rssi_dbfs: None,
            cpr: CprState::default(),
            last_mono: tick.mono,
        }
    }

    /// Whether this aircraft has anything worth putting on a map.
    pub fn is_locatable(&self) -> bool {
        self.position.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerStats {
    pub messages: u64,
    /// Messages dropped because their address was never confirmed by a
    /// CRC-validated extended squitter.
    pub unverified_dropped: u64,
    pub fixes: u64,
    pub need_pair: u64,
    pub rejected_stale_pair: u64,
    pub rejected_nl_mismatch: u64,
    pub rejected_out_of_range: u64,
    pub rejected_implied_speed: u64,
    pub rejected_other: u64,
    pub aircraft_expired: u64,
}

impl TrackerStats {
    pub fn rejected_total(&self) -> u64 {
        self.rejected_stale_pair
            + self.rejected_nl_mismatch
            + self.rejected_out_of_range
            + self.rejected_implied_speed
            + self.rejected_other
    }

    fn count_rejection(&mut self, reason: RejectReason) {
        match reason {
            RejectReason::StalePair => self.rejected_stale_pair += 1,
            RejectReason::NlMismatch => self.rejected_nl_mismatch += 1,
            RejectReason::OutOfRange => self.rejected_out_of_range += 1,
            RejectReason::ImpliedSpeed => self.rejected_implied_speed += 1,
            _ => self.rejected_other += 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TrackerConfig {
    pub gates: Gates,
    /// How long an aircraft stays in the snapshot after its last message.
    pub expire_after: Duration,
    /// Whether to require a CRC-clean DF17/18 before trusting an address.
    ///
    /// Leave this on. Turning it off fills the map with decoded noise.
    pub require_verified_icao: bool,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        TrackerConfig {
            gates: Gates::default(),
            expire_after: Duration::from_secs(60),
            require_verified_icao: true,
        }
    }
}

/// What changed as a result of a message. Lets the storage layer write only
/// when something is worth writing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Update {
    /// The message was dropped as unverifiable.
    Ignored,
    /// State changed but no new position.
    Updated,
    /// A new position was resolved.
    NewPosition(Fix),
}

pub struct Tracker {
    config: TrackerConfig,
    solver: Box<dyn PositionSolver>,
    aircraft: HashMap<Icao, Aircraft>,
    verified: HashSet<Icao>,
    stats: TrackerStats,
}

impl Tracker {
    pub fn new(config: TrackerConfig) -> Self {
        let solver = Box::new(GlobalCprSolver::new(config.gates));
        Tracker {
            config,
            solver,
            aircraft: HashMap::new(),
            verified: HashSet::new(),
            stats: TrackerStats::default(),
        }
    }

    pub fn with_solver(mut self, solver: Box<dyn PositionSolver>) -> Self {
        self.solver = solver;
        self
    }

    pub fn stats(&self) -> TrackerStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.aircraft.len()
    }

    pub fn is_empty(&self) -> bool {
        self.aircraft.is_empty()
    }

    pub fn get(&self, icao: Icao) -> Option<&Aircraft> {
        self.aircraft.get(&icao)
    }

    /// Addresses proved real by a CRC-validated extended squitter.
    pub fn verified(&self) -> &HashSet<Icao> {
        &self.verified
    }

    pub fn solver_name(&self) -> &'static str {
        self.solver.name()
    }

    /// The plausibility limits currently in force.
    pub fn gates(&self) -> Gates {
        self.config.gates
    }

    /// Move the station, keeping every aircraft's CPR state.
    ///
    /// Returns true if the position actually changed. The range gate reads it
    /// on the next fix, so a station set from the web interface takes effect
    /// within one message rather than at the next restart.
    ///
    /// Note what is deliberately *not* done here: previously-accepted fixes
    /// are not re-examined. They were admitted under the old gate and are
    /// already drawn; retroactively deleting them would make a position change
    /// look like an outage. The gate governs what is admitted next.
    pub fn set_receiver(&mut self, receiver: Option<(f64, f64)>) -> bool {
        if self.config.gates.receiver == receiver {
            return false;
        }
        self.config.gates.receiver = receiver;
        self.solver.set_gates(self.config.gates);
        true
    }

    /// Fold one validated frame into the aircraft set.
    pub fn observe(&mut self, frame: &Frame, tick: Tick, rssi_dbfs: Option<f32>) -> Update {
        let icao = frame.icao();
        let df = frame.df();

        // DF17/18 carry the address in the clear and are CRC-checkable, so
        // seeing one is what makes an address trustworthy.
        if matches!(df, 17 | 18) {
            self.verified.insert(icao);
        } else if self.config.require_verified_icao && !self.verified.contains(&icao) {
            self.stats.unverified_dropped += 1;
            return Update::Ignored;
        }

        self.stats.messages += 1;
        let expire_after = self.config.expire_after;
        let entry = self
            .aircraft
            .entry(icao)
            .or_insert_with(|| Aircraft::new(icao, tick));

        // An address reappearing after a long silence is almost certainly a
        // different flight; start its history fresh rather than drawing a line
        // across the map.
        if tick.mono.saturating_duration_since(entry.last_mono) > expire_after {
            *entry = Aircraft::new(icao, tick);
        }

        entry.messages += 1;
        entry.last_seen_ms = tick.wall_ms;
        entry.last_mono = tick.mono;
        if rssi_dbfs.is_some() {
            entry.rssi_dbfs = rssi_dbfs;
        }

        let Some(message) = frame.decode() else {
            return Update::Updated;
        };

        match message {
            Message::Identification { callsign, category } => {
                if !callsign.is_empty() {
                    entry.callsign = Some(callsign);
                }
                entry.category = Some(category);
                Update::Updated
            }

            Message::AirbornePosition { altitude, cpr } => {
                entry.on_ground = false;
                if altitude != Altitude::Unavailable {
                    entry.altitude = Some(altitude);
                }
                entry.position_messages += 1;
                Self::solve(
                    &*self.solver,
                    &mut self.stats,
                    entry,
                    cpr,
                    CprKind::Airborne,
                    tick,
                )
            }

            Message::SurfacePosition {
                cpr,
                ground_speed,
                track,
            } => {
                entry.on_ground = true;
                entry.altitude = Some(Altitude::Barometric(0));
                if ground_speed.is_some() {
                    entry.ground_speed_kt = ground_speed.map(|k| k.0);
                }
                if track.is_some() {
                    entry.track = track;
                }
                entry.position_messages += 1;
                Self::solve(
                    &*self.solver,
                    &mut self.stats,
                    entry,
                    cpr,
                    CprKind::Surface,
                    tick,
                )
            }

            Message::Velocity {
                ground_speed,
                track,
                vertical_rate,
                ..
            } => {
                if let Some(speed) = ground_speed {
                    entry.ground_speed_kt = Some(speed.0);
                }
                if track.is_some() {
                    entry.track = track;
                }
                if let Some(rate) = vertical_rate {
                    entry.vertical_rate_fpm = Some(rate.0);
                }
                Update::Updated
            }

            Message::Airspeed { vertical_rate, .. } => {
                if let Some(rate) = vertical_rate {
                    entry.vertical_rate_fpm = Some(rate.0);
                }
                Update::Updated
            }

            Message::Unsupported { .. } => Update::Updated,
        }
    }

    /// Free function so the borrow of `entry` and of `stats` can coexist.
    fn solve(
        solver: &dyn PositionSolver,
        stats: &mut TrackerStats,
        entry: &mut Aircraft,
        frame: adsb_core::cpr::CprFrame,
        kind: CprKind,
        tick: Tick,
    ) -> Update {
        match solver.update(
            &mut entry.cpr,
            entry.icao,
            frame,
            kind,
            tick.mono,
            tick.wall_ms,
        ) {
            PositionOutcome::Fix(fix) => {
                entry.position = Some(fix);
                stats.fixes += 1;
                Update::NewPosition(fix)
            }
            PositionOutcome::NeedPair => {
                stats.need_pair += 1;
                Update::Updated
            }
            PositionOutcome::Rejected(reason) => {
                stats.count_rejection(reason);
                Update::Updated
            }
        }
    }

    /// Drop aircraft that have gone quiet.
    pub fn expire(&mut self, tick: Tick) -> usize {
        let cutoff = self.config.expire_after;
        let before = self.aircraft.len();
        self.aircraft
            .retain(|_, a| tick.mono.saturating_duration_since(a.last_mono) <= cutoff);
        let removed = before - self.aircraft.len();
        self.stats.aircraft_expired += removed as u64;
        removed
    }

    /// An immutable view for the API to serve without holding a lock.
    pub fn snapshot(&self, tick: Tick) -> Snapshot {
        Snapshot::build(self.aircraft.values(), tick)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Aircraft> {
        self.aircraft.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_from(hex: &str) -> Vec<u8> {
        adsb_core::hex_to_bytes(hex).unwrap()
    }

    fn observe(tracker: &mut Tracker, hex: &str, tick: Tick) -> Update {
        let bytes = frame_from(hex);
        let frame = Frame::new(&bytes).unwrap();
        tracker.observe(&frame, tick, Some(-18.0))
    }

    fn ottawa_config() -> TrackerConfig {
        TrackerConfig {
            gates: Gates {
                receiver: Some((45.412, -75.679)),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn identification_sets_the_callsign() {
        let mut t = Tracker::new(TrackerConfig::default());
        observe(&mut t, "8D4840D6202CC371C32CE0576098", Tick::now());
        let a = t.get(Icao::new(0x4840D6)).unwrap();
        assert_eq!(a.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(a.messages, 1);
    }

    #[test]
    fn velocity_populates_speed_and_track() {
        let mut t = Tracker::new(TrackerConfig::default());
        observe(&mut t, "8DC053F699093B19D030164300A1", Tick::now());
        let a = t.get(Icao::new(0xC053F6)).unwrap();
        assert_eq!(a.ground_speed_kt, Some(375));
        assert!((a.track.unwrap().0 - 57.0).abs() < 1.0);
        assert_eq!(a.vertical_rate_fpm, Some(704));
    }

    #[test]
    fn a_position_pair_produces_a_fix() {
        // The canonical pair resolves near the Netherlands, so no range gate.
        let mut t = Tracker::new(TrackerConfig::default());
        let tick = Tick::now();
        assert_eq!(
            observe(&mut t, "8D40621D58C382D690C8AC2863A7", tick),
            Update::Updated
        );
        match observe(&mut t, "8D40621D58C386435CC412692AD6", tick) {
            Update::NewPosition(fix) => {
                assert!((fix.lat - 52.2658).abs() < 0.01, "lat {}", fix.lat);
                assert_eq!(fix.source, PositionSource::GlobalCpr);
            }
            other => panic!("expected a fix, got {other:?}"),
        }
        assert_eq!(t.stats().fixes, 1);
        assert_eq!(t.stats().need_pair, 1);
    }

    /// Moving the station mid-flight must change what the range gate admits,
    /// without discarding the CPR state that got us there.
    ///
    /// This is the whole point of `set_receiver`: the operator sets the
    /// position from the web interface on a receiver that has been up for an
    /// hour, and the next fix has to be judged against the new position. A
    /// rebuild would have worked too -- and would have emptied the map.
    #[test]
    fn moving_the_receiver_changes_the_range_gate_immediately() {
        // The canonical pair resolves near the Netherlands. Ottawa is 5500 km
        // away, well outside the 400 km gate.
        let mut t = Tracker::new(ottawa_config());
        let tick = Tick::now();
        observe(&mut t, "8D40621D58C382D690C8AC2863A7", tick);
        observe(&mut t, "8D40621D58C386435CC412692AD6", tick);
        assert_eq!(
            t.stats().rejected_out_of_range,
            1,
            "an Ottawa station must not believe a Dutch position"
        );
        assert_eq!(t.stats().fixes, 0);

        // Move the station to Amsterdam while the aircraft is still tracked.
        assert!(t.set_receiver(Some((52.3, 4.8))), "the position changed");
        assert!(
            !t.set_receiver(Some((52.3, 4.8))),
            "setting the same position again is not a change"
        );

        // The aircraft is still here: its even/odd state was not thrown away.
        assert!(t.get(Icao::new(0x40_62_1D)).is_some());

        // One more frame of either parity re-pairs against the retained one.
        match observe(&mut t, "8D40621D58C386435CC412692AD6", tick) {
            Update::NewPosition(fix) => {
                assert!((fix.lat - 52.2658).abs() < 0.01, "lat {}", fix.lat);
            }
            other => panic!("the new gate should admit this fix, got {other:?}"),
        }
    }

    #[test]
    fn altitude_comes_from_the_position_message() {
        let mut t = Tracker::new(TrackerConfig::default());
        observe(&mut t, "8DC060B6587D8236085C837FDA27", Tick::now());
        let a = t.get(Icao::new(0xC060B6)).unwrap();
        assert_eq!(a.altitude, Some(Altitude::Barometric(24000)));
    }

    /// The ghost-aircraft defence.
    #[test]
    fn unverified_addresses_are_dropped() {
        let mut t = Tracker::new(TrackerConfig::default());
        // DF11's address is in the clear but its parity carries an
        // interrogator id; we still require a DF17 to have vouched for it.
        assert_eq!(
            observe(&mut t, "5DC04413BFAD35", Tick::now()),
            Update::Ignored
        );
        assert_eq!(t.stats().unverified_dropped, 1);
        assert!(t.is_empty());
    }

    #[test]
    fn an_address_confirmed_by_df17_is_then_accepted() {
        let mut t = Tracker::new(TrackerConfig::default());
        let tick = Tick::now();
        // A DF17 from C04413 vouches for the address...
        observe(&mut t, "8DC0441358A59244C071F63ABDC8", tick);
        // ...after which its DF11 is admitted.
        assert_ne!(observe(&mut t, "5DC04413BFAD35", tick), Update::Ignored);
        assert_eq!(t.stats().unverified_dropped, 0);
        assert!(t.verified().contains(&Icao::new(0xC04413)));
    }

    #[test]
    fn verification_can_be_disabled_for_experiments() {
        let mut t = Tracker::new(TrackerConfig {
            require_verified_icao: false,
            ..Default::default()
        });
        assert_ne!(
            observe(&mut t, "5DC04413BFAD35", Tick::now()),
            Update::Ignored
        );
    }

    #[test]
    fn quiet_aircraft_expire() {
        let mut t = Tracker::new(TrackerConfig {
            expire_after: Duration::from_secs(30),
            ..Default::default()
        });
        let start = Tick::now();
        observe(&mut t, "8D4840D6202CC371C32CE0576098", start);
        assert_eq!(t.len(), 1);

        let later = Tick {
            mono: start.mono + Duration::from_secs(31),
            wall_ms: start.wall_ms + 31_000,
        };
        assert_eq!(t.expire(later), 1);
        assert!(t.is_empty());
        assert_eq!(t.stats().aircraft_expired, 1);
    }

    /// An address reappearing hours later is a different flight. Carrying the
    /// old position forward would draw a line across the map.
    #[test]
    fn a_returning_address_starts_a_fresh_track() {
        let mut t = Tracker::new(TrackerConfig {
            expire_after: Duration::from_secs(30),
            ..Default::default()
        });
        let start = Tick::now();
        observe(&mut t, "8D4840D6202CC371C32CE0576098", start);
        assert_eq!(
            t.get(Icao::new(0x4840D6)).unwrap().callsign.as_deref(),
            Some("KLM1023")
        );

        let much_later = Tick {
            mono: start.mono + Duration::from_secs(3600),
            wall_ms: start.wall_ms + 3_600_000,
        };
        // A velocity message from the same address, an hour on.
        observe(&mut t, "8D4840D6202CC371C32CE0576098", much_later);
        let a = t.get(Icao::new(0x4840D6)).unwrap();
        assert_eq!(a.messages, 1, "message count should have reset");
        assert_eq!(a.first_seen_ms, much_later.wall_ms);
    }

    #[test]
    fn rejections_are_counted_by_reason() {
        let mut t = Tracker::new(ottawa_config());
        let tick = Tick::now();
        // The canonical pair resolves over the North Sea, 5500 km away.
        observe(&mut t, "8D40621D58C382D690C8AC2863A7", tick);
        observe(&mut t, "8D40621D58C386435CC412692AD6", tick);

        assert_eq!(t.stats().rejected_out_of_range, 1);
        assert_eq!(t.stats().fixes, 0);
        assert_eq!(t.stats().rejected_total(), 1);
        assert!(!t.get(Icao::new(0x40621D)).unwrap().is_locatable());
    }

    #[test]
    fn rssi_is_recorded() {
        let mut t = Tracker::new(TrackerConfig::default());
        observe(&mut t, "8D4840D6202CC371C32CE0576098", Tick::now());
        assert_eq!(t.get(Icao::new(0x4840D6)).unwrap().rssi_dbfs, Some(-18.0));
    }
}
