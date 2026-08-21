//! Stage 5 — turning position messages into coordinates.
//!
//! This is the last swappable stage, and it lives here rather than in
//! `adsb-dsp` because it is the only one that needs per-aircraft state: a
//! global fix requires an even *and* an odd frame from the same aircraft,
//! close together in time.
//!
//! # Why gates matter more than they look
//!
//! CPR fails silently. Pair two frames that straddle a zone boundary and the
//! arithmetic returns a confident, plausible, wrong answer — the old
//! implementation once put an Ottawa aircraft over Lake Huron, and the fix at
//! the time was a bounding box in the API to hide it.
//!
//! Hiding it is the wrong place. A bounding box filters *display*; it does not
//! stop a bad fix from being stored, from anchoring the next local decode, or
//! from being counted as a success. The gates below reject implausible fixes
//! where they are produced, which makes the bbox unnecessary rather than
//! load-bearing.
//!
//! # Improving on the baseline
//!
//! [`GlobalCprSolver`] only does global decoding, so an aircraft must send both
//! parities before it appears at all. Worth adding:
//!
//! - **Local CPR.** One frame plus a reference within ~180 NM resolves a
//!   position, so aircraft appear on the first message rather than the second,
//!   and aircraft that only ever send one parity appear at all.
//! - **Surface positions** (type codes 5–8), which use the 90-degree grid.
//! - **Smarter pairing** than "most recent of each": prefer pairs close in
//!   time, and reject a pair whose implied speed is impossible.

use adsb_core::{
    Icao,
    cpr::{self, CprFrame, CprKind},
};
use std::time::{Duration, Instant};

/// Where a position came from. Worth exposing to a UI: a local fix inherits
/// the error of its reference, and a stale one should be drawn differently
/// from a fresh one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionSource {
    /// An even/odd pair. Self-contained and needs no prior knowledge.
    GlobalCpr,
    /// A single frame plus a nearby reference.
    LocalCpr,
    /// A surface position, on the finer 90-degree grid.
    Surface,
}

impl PositionSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            PositionSource::GlobalCpr => "global_cpr",
            PositionSource::LocalCpr => "local_cpr",
            PositionSource::Surface => "surface",
        }
    }
}

/// A resolved position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub source: PositionSource,
    /// Wall-clock time of the fix, epoch milliseconds.
    pub at_ms: i64,
}

/// Why a position message did not produce a fix.
///
/// Counting rejections *by reason* is what lets the scoreboard distinguish
/// "my solver got stricter" from "my decoder got worse". Without it, positions
/// per minute drops and you cannot tell which.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// The other parity is too old to pair with.
    StalePair,
    /// The two frames fell in different longitude bands, so the aircraft
    /// crossed a zone boundary between them.
    NlMismatch,
    /// Latitude arithmetic produced something off the globe.
    LatOutOfBounds,
    /// Further away than the receiver could plausibly hear.
    OutOfRange,
    /// Getting there from the last fix would require an impossible speed.
    ImpliedSpeed,
    /// A range gate is enabled but the receiver position is unset.
    NoReceiverPosition,
}

impl RejectReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            RejectReason::StalePair => "stale_pair",
            RejectReason::NlMismatch => "nl_mismatch",
            RejectReason::LatOutOfBounds => "lat_out_of_bounds",
            RejectReason::OutOfRange => "out_of_range",
            RejectReason::ImpliedSpeed => "implied_speed",
            RejectReason::NoReceiverPosition => "no_receiver_position",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionOutcome {
    Fix(Fix),
    /// Not an error: the other parity has not arrived yet.
    NeedPair,
    Rejected(RejectReason),
}

/// Limits a fix must satisfy to be believed.
#[derive(Clone, Copy, Debug)]
pub struct Gates {
    /// Maximum age difference between the two frames of a pair.
    ///
    /// Ten seconds is the conventional figure: an airliner covers about 2.5 km
    /// in that time, comfortably inside a CPR zone.
    pub max_pair_age: Duration,
    /// Maximum plausible distance from the receiver, kilometres.
    ///
    /// The radio horizon for an aircraft at FL360 is roughly 400 km, so
    /// anything beyond that is arithmetic, not an aeroplane.
    pub max_range_km: f64,
    /// Maximum plausible ground speed between consecutive fixes, knots.
    pub max_speed_kt: f64,
    /// Receiver position, for the range gate and local decoding.
    pub receiver: Option<(f64, f64)>,
}

impl Default for Gates {
    fn default() -> Self {
        Gates {
            max_pair_age: Duration::from_secs(10),
            max_range_km: 400.0,
            max_speed_kt: 700.0,
            receiver: None,
        }
    }
}

/// Per-aircraft CPR state.
#[derive(Clone, Copy, Debug, Default)]
pub struct CprState {
    /// The frames are held with a **monotonic** timestamp on purpose. Using
    /// wall time would mean an NTP step could invalidate a good pair, or
    /// validate a stale one, at exactly the moment the Pi first syncs its
    /// clock after boot.
    even: Option<(CprFrame, Instant)>,
    odd: Option<(CprFrame, Instant)>,
    last_fix: Option<(Fix, Instant)>,
}

/// Resolves positions from CPR frames.
pub trait PositionSolver: Send {
    fn name(&self) -> &'static str;
    fn describe(&self) -> &'static str;

    /// Adopt new plausibility limits without rebuilding the solver.
    ///
    /// # Why a setter and not just a constructor argument
    ///
    /// The receiver position is a gate input, and it is the one input an
    /// operator changes *while the receiver is running* — moving the station,
    /// or setting it for the first time from the web interface. Rebuilding
    /// the solver to change it would throw away every aircraft's accumulated
    /// even/odd CPR state, so every aircraft in the air would vanish and
    /// re-appear one message later. Keeping the state and swapping the limits
    /// is the difference between a settings change and a visible outage.
    ///
    /// The default ignores it, which is right for a solver with no gates.
    fn set_gates(&mut self, _gates: Gates) {}

    /// Offer a position frame. `at` is monotonic; `now_ms` is wall clock.
    fn update(
        &self,
        state: &mut CprState,
        icao: Icao,
        frame: CprFrame,
        kind: CprKind,
        at: Instant,
        now_ms: i64,
    ) -> PositionOutcome;
}

/// Global CPR only: an aircraft must send both parities to be located.
pub struct GlobalCprSolver {
    pub gates: Gates,
}

impl GlobalCprSolver {
    pub fn new(gates: Gates) -> Self {
        GlobalCprSolver { gates }
    }

    /// Apply the plausibility gates to a candidate fix.
    fn admit(&self, state: &CprState, lat: f64, lon: f64, at: Instant) -> Result<(), RejectReason> {
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(RejectReason::LatOutOfBounds);
        }

        if let Some((rx_lat, rx_lon)) = self.gates.receiver {
            let range = cpr::haversine_km(rx_lat, rx_lon, lat, lon);
            if range > self.gates.max_range_km {
                return Err(RejectReason::OutOfRange);
            }
        }

        // A fix that would require the aircraft to have moved impossibly fast
        // since the last one is a mispaired frame, not a very quick aeroplane.
        if let Some((previous, when)) = state.last_fix {
            let seconds = at.saturating_duration_since(when).as_secs_f64();
            if seconds > 0.05 {
                let km = cpr::haversine_km(previous.lat, previous.lon, lat, lon);
                let knots = (km / 1.852) / (seconds / 3600.0);
                if knots > self.gates.max_speed_kt {
                    return Err(RejectReason::ImpliedSpeed);
                }
            }
        }
        Ok(())
    }
}

impl PositionSolver for GlobalCprSolver {
    fn name(&self) -> &'static str {
        "global-cpr"
    }

    fn describe(&self) -> &'static str {
        "even/odd pairs only, with range and implied-speed gates"
    }

    fn set_gates(&mut self, gates: Gates) {
        self.gates = gates;
    }

    fn update(
        &self,
        state: &mut CprState,
        _icao: Icao,
        frame: CprFrame,
        kind: CprKind,
        at: Instant,
        now_ms: i64,
    ) -> PositionOutcome {
        if frame.odd {
            state.odd = Some((frame, at));
        } else {
            state.even = Some((frame, at));
        }

        let (Some((even, even_at)), Some((odd, odd_at))) = (state.even, state.odd) else {
            return PositionOutcome::NeedPair;
        };

        // The pair must be close in time, or the aircraft may have crossed a
        // zone boundary and the arithmetic will lie without complaining.
        let gap = if even_at > odd_at {
            even_at - odd_at
        } else {
            odd_at - even_at
        };
        if gap > self.gates.max_pair_age {
            return PositionOutcome::Rejected(RejectReason::StalePair);
        }

        // Anchor to whichever frame arrived most recently: that is where the
        // aircraft actually is.
        let newest_odd = odd_at >= even_at;
        let Some((lat, lon)) = cpr::decode_global(even, odd, newest_odd, kind) else {
            return PositionOutcome::Rejected(RejectReason::NlMismatch);
        };

        if let Err(reason) = self.admit(state, lat, lon, at) {
            return PositionOutcome::Rejected(reason);
        }

        let fix = Fix {
            lat,
            lon,
            source: if kind == CprKind::Surface {
                PositionSource::Surface
            } else {
                PositionSource::GlobalCpr
            },
            at_ms: now_ms,
        };
        state.last_fix = Some((fix, at));
        PositionOutcome::Fix(fix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OTTAWA: (f64, f64) = (45.412, -75.679);

    fn solver(receiver: Option<(f64, f64)>) -> GlobalCprSolver {
        GlobalCprSolver::new(Gates {
            receiver,
            ..Default::default()
        })
    }

    fn frames_for(lat: f64, lon: f64) -> (CprFrame, CprFrame) {
        (
            cpr::encode(lat, lon, false, CprKind::Airborne),
            cpr::encode(lat, lon, true, CprKind::Airborne),
        )
    }

    #[test]
    fn one_parity_is_not_enough() {
        let s = solver(None);
        let mut state = CprState::default();
        let (even, _) = frames_for(45.4, -75.7);
        assert_eq!(
            s.update(
                &mut state,
                Icao::new(1),
                even,
                CprKind::Airborne,
                Instant::now(),
                0
            ),
            PositionOutcome::NeedPair
        );
    }

    #[test]
    fn a_fresh_pair_resolves() {
        let s = solver(Some(OTTAWA));
        let mut state = CprState::default();
        let (lat, lon) = (45.30, -75.60);
        let (even, odd) = frames_for(lat, lon);
        let t = Instant::now();

        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        match s.update(&mut state, Icao::new(1), odd, CprKind::Airborne, t, 1000) {
            PositionOutcome::Fix(fix) => {
                assert!(cpr::haversine_km(fix.lat, fix.lon, lat, lon) < 0.05);
                assert_eq!(fix.source, PositionSource::GlobalCpr);
                assert_eq!(fix.at_ms, 1000);
            }
            other => panic!("expected a fix, got {other:?}"),
        }
    }

    #[test]
    fn a_stale_pair_is_rejected() {
        let s = solver(None);
        let mut state = CprState::default();
        let (even, odd) = frames_for(45.3, -75.6);
        let t = Instant::now();

        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        let much_later = t + Duration::from_secs(30);
        assert_eq!(
            s.update(
                &mut state,
                Icao::new(1),
                odd,
                CprKind::Airborne,
                much_later,
                0
            ),
            PositionOutcome::Rejected(RejectReason::StalePair)
        );
    }

    /// The Lake Huron case: a mispaired frame yields a confident wrong answer
    /// hundreds of km away. The range gate is what catches it, at the point of
    /// production rather than in the API.
    #[test]
    fn an_impossibly_distant_fix_is_rejected() {
        let s = solver(Some(OTTAWA));
        let mut state = CprState::default();
        // Somewhere over Europe -- arithmetically fine, physically absurd.
        let (even, odd) = frames_for(52.26, 3.94);
        let t = Instant::now();

        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        assert_eq!(
            s.update(&mut state, Icao::new(1), odd, CprKind::Airborne, t, 0),
            PositionOutcome::Rejected(RejectReason::OutOfRange)
        );
    }

    #[test]
    fn without_a_receiver_position_the_range_gate_is_simply_off() {
        // Not silently defaulting to 0,0 -- that is in the Gulf of Guinea and
        // would reject every aircraft in Canada.
        let s = solver(None);
        let mut state = CprState::default();
        let (even, odd) = frames_for(52.26, 3.94);
        let t = Instant::now();
        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        assert!(matches!(
            s.update(&mut state, Icao::new(1), odd, CprKind::Airborne, t, 0),
            PositionOutcome::Fix(_)
        ));
    }

    #[test]
    fn a_teleporting_aircraft_is_rejected() {
        let s = solver(None);
        let mut state = CprState::default();
        let t = Instant::now();

        // Establish a fix near Ottawa.
        let (even, odd) = frames_for(45.3, -75.6);
        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        assert!(matches!(
            s.update(&mut state, Icao::new(1), odd, CprKind::Airborne, t, 0),
            PositionOutcome::Fix(_)
        ));

        // One second later, 300 km away. That is Mach 470.
        let later = t + Duration::from_secs(1);
        let (even2, odd2) = frames_for(48.0, -75.6);
        s.update(&mut state, Icao::new(1), even2, CprKind::Airborne, later, 0);
        assert_eq!(
            s.update(&mut state, Icao::new(1), odd2, CprKind::Airborne, later, 0),
            PositionOutcome::Rejected(RejectReason::ImpliedSpeed)
        );
    }

    #[test]
    fn a_plausible_movement_is_accepted() {
        let s = solver(None);
        let mut state = CprState::default();
        let t = Instant::now();

        let (even, odd) = frames_for(45.30, -75.60);
        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        s.update(&mut state, Icao::new(1), odd, CprKind::Airborne, t, 0);

        // 10 seconds later, 1.5 km on -- about 290 knots.
        let later = t + Duration::from_secs(10);
        let (even2, odd2) = frames_for(45.3135, -75.60);
        s.update(&mut state, Icao::new(1), even2, CprKind::Airborne, later, 0);
        match s.update(&mut state, Icao::new(1), odd2, CprKind::Airborne, later, 0) {
            PositionOutcome::Fix(_) => {}
            other => panic!("a normal airliner was rejected: {other:?}"),
        }
    }

    /// Tracking must not break when the wall clock jumps, which it does the
    /// first time a Pi with no RTC reaches an NTP server after boot.
    #[test]
    fn pairing_survives_a_wall_clock_step() {
        let s = solver(None);
        let mut state = CprState::default();
        let (even, odd) = frames_for(45.3, -75.6);
        let t = Instant::now();

        s.update(&mut state, Icao::new(1), even, CprKind::Airborne, t, 0);
        // Wall clock leaps 56 years forward; monotonic time barely moves.
        let outcome = s.update(
            &mut state,
            Icao::new(1),
            odd,
            CprKind::Airborne,
            t + Duration::from_millis(500),
            1_785_000_000_000,
        );
        assert!(
            matches!(outcome, PositionOutcome::Fix(_)),
            "an NTP step invalidated a good pair: {outcome:?}"
        );
    }

    #[test]
    fn reject_reasons_have_stable_names_for_the_api() {
        assert_eq!(RejectReason::StalePair.as_str(), "stale_pair");
        assert_eq!(RejectReason::OutOfRange.as_str(), "out_of_range");
        assert_eq!(PositionSource::GlobalCpr.as_str(), "global_cpr");
    }
}
