//! Compact Position Reporting.
//!
//! A position message spends only 17 bits each on latitude and longitude,
//! which cannot address the globe. Instead each frame reports a coordinate
//! *within a zone*, and there are two frame types: **even** frames lay down 60
//! latitude zones, **odd** frames lay down 59. Because the two grids are
//! slightly out of step, the disagreement between an even and an odd reading
//! is unique to one zone — so a pair pins down an absolute position with no
//! prior knowledge at all. That is the whole trick.
//!
//! The pair must be close in time. If the aircraft crosses a zone boundary
//! between the two frames the arithmetic does not fail, it *silently returns a
//! wrong answer* — which is why the old implementation's longitude bug
//! survived so long: the output still looked like a plausible smooth track.
//!
//! # The bug this module exists to not have
//!
//! When the **odd** frame is the most recent, the longitude zone width is
//! `360/(NL-1)`, not `360/NL`. Getting that wrong produces a position error of
//! roughly `NL/(NL-1)` in the longitude offset — about 7.4 km at Ottawa's
//! latitude. It is invisible in a single fix and looks like GPS jitter in a
//! track. See [`tests::regression_odd_anchor_longitude_divisor`].
//!
//! The defence is [`encode`]: with an encoder, the decoder can be tested by
//! round-trip over a global grid, for **both** anchor frames. A single
//! canonical vector — which is all most tutorials give you — exercises only
//! the even anchor and would not have caught this.

/// Number of latitude zones in a quadrant. Fixed by the standard.
pub const NZ: f64 = 15.0;

/// 2^17 — the resolution of a CPR coordinate field.
const CPR_MAX: f64 = 131_072.0;

/// Airborne and surface messages use different latitude spans: surface
/// positions trade range for four times the resolution, since an aircraft on
/// the ground is necessarily near a known airport.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CprKind {
    Airborne,
    Surface,
}

impl CprKind {
    /// The full latitude span the zone system divides up.
    const fn span(self) -> f64 {
        match self {
            CprKind::Airborne => 360.0,
            CprKind::Surface => 90.0,
        }
    }
}

/// One encoded position report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CprFrame {
    /// 17-bit encoded latitude.
    pub lat: u32,
    /// 17-bit encoded longitude.
    pub lon: u32,
    /// Which grid this frame uses. Bit 54 of the message (spec bit 55).
    pub odd: bool,
}

/// Number of longitude zones at a given latitude: 59 at the equator, 1 at the
/// poles. Longitude zones must get wider as the meridians converge, or the
/// resolution near the poles would be absurd.
///
/// The closed form evaluates to exactly 60.0 at the equator, which is off by
/// one against the ICAO table, so the result is clamped.
pub fn nl(lat: f64) -> u32 {
    let lat = lat.abs();
    if lat >= 87.0 {
        return 1;
    }
    if lat == 0.0 {
        return 59;
    }
    let a = 1.0 - (std::f64::consts::PI / (2.0 * NZ)).cos();
    let b = lat.to_radians().cos().powi(2);
    let value = (2.0 * std::f64::consts::PI / (1.0 - a / b).acos()).floor();
    (value as u32).clamp(1, 59)
}

/// Encode a position into a CPR frame.
///
/// This exists primarily so [`decode_global`] and [`decode_local`] can be
/// tested by round-trip. It is also what the synthetic IQ generator uses to
/// build position messages with known-correct answers.
pub fn encode(lat: f64, lon: f64, odd: bool, kind: CprKind) -> CprFrame {
    let i = if odd { 1.0 } else { 0.0 };
    let span = kind.span();

    let d_lat = span / (4.0 * NZ - i);
    let yz = (CPR_MAX * (lat.rem_euclid(d_lat) / d_lat) + 0.5).floor();
    let r_lat = d_lat * ((yz / CPR_MAX) + (lat / d_lat).floor());

    let zones = nl(r_lat) as f64 - i;
    let d_lon = if zones > 0.0 { span / zones } else { span };
    let xz = (CPR_MAX * (lon.rem_euclid(d_lon) / d_lon) + 0.5).floor();

    CprFrame {
        lat: (yz as i64).rem_euclid(131_072) as u32,
        lon: (xz as i64).rem_euclid(131_072) as u32,
        odd,
    }
}

/// Recover an absolute position from an even/odd frame pair.
///
/// `newest_odd` selects which frame the answer is anchored to — it should be
/// whichever arrived most recently, because that is where the aircraft
/// actually is. Returns `None` when the pair is unusable.
pub fn decode_global(
    even: CprFrame,
    odd: CprFrame,
    newest_odd: bool,
    kind: CprKind,
) -> Option<(f64, f64)> {
    debug_assert!(!even.odd && odd.odd, "frames passed in the wrong order");

    let span = kind.span();
    let (elat, elon) = (f64::from(even.lat) / CPR_MAX, f64::from(even.lon) / CPR_MAX);
    let (olat, olon) = (f64::from(odd.lat) / CPR_MAX, f64::from(odd.lon) / CPR_MAX);

    // The latitude zone index. This single integer is what the two grids
    // agree on, and it is the whole reason the scheme works.
    let j = (59.0 * elat - 60.0 * olat + 0.5).floor();

    let mut lat_even = (span / (4.0 * NZ)) * (j.rem_euclid(60.0) + elat);
    let mut lat_odd = (span / (4.0 * NZ - 1.0)) * (j.rem_euclid(59.0) + olat);

    // The southern hemisphere arrives as 270..360; fold it to negative.
    if kind == CprKind::Airborne {
        if lat_even >= 270.0 {
            lat_even -= 360.0;
        }
        if lat_odd >= 270.0 {
            lat_odd -= 360.0;
        }
    }
    if !(-90.0..=90.0).contains(&lat_even) || !(-90.0..=90.0).contains(&lat_odd) {
        return None;
    }

    // If the two frames landed in different longitude bands, the aircraft
    // crossed a boundary between transmissions and the pair is unusable.
    // Silently proceeding here is a bad-position generator.
    let nl_even = nl(lat_even);
    if nl_even != nl(lat_odd) {
        return None;
    }
    let zones = f64::from(nl_even);

    let m = (elon * (zones - 1.0) - olon * zones + 0.5).floor();

    // THE divisor. For the odd anchor it is NL-1, not NL. See module docs.
    let (lat, ni, cpr_lon) = if newest_odd {
        (lat_odd, (zones - 1.0).max(1.0), olon)
    } else {
        (lat_even, zones.max(1.0), elon)
    };

    let mut lon = (span / ni) * (m.rem_euclid(ni) + cpr_lon);
    if lon >= 180.0 {
        lon -= 360.0;
    }
    Some((lat, lon))
}

/// Recover a position from a *single* frame, using a nearby reference.
///
/// Needs no pair, so it works on the first message from an aircraft and
/// survives an aircraft that only ever sends one parity. The reference must be
/// within about 180 NM (half a zone) or it resolves to the wrong zone —
/// use the receiver position, or the aircraft's own last known fix.
pub fn decode_local(frame: CprFrame, ref_lat: f64, ref_lon: f64, kind: CprKind) -> (f64, f64) {
    let i = if frame.odd { 1.0 } else { 0.0 };
    let span = kind.span();

    let d_lat = span / (4.0 * NZ - i);
    let cpr_lat = f64::from(frame.lat) / CPR_MAX;
    let j =
        (ref_lat / d_lat).floor() + (0.5 + (ref_lat.rem_euclid(d_lat)) / d_lat - cpr_lat).floor();
    let lat = d_lat * (j + cpr_lat);

    let zones = nl(lat) as f64 - i;
    let d_lon = if zones > 0.0 { span / zones } else { span };
    let cpr_lon = f64::from(frame.lon) / CPR_MAX;
    let m =
        (ref_lon / d_lon).floor() + (0.5 + (ref_lon.rem_euclid(d_lon)) / d_lon - cpr_lon).floor();
    let lon = d_lon * (m + cpr_lon);

    (lat, normalize_lon(lon))
}

fn normalize_lon(lon: f64) -> f64 {
    let mut lon = lon;
    while lon >= 180.0 {
        lon -= 360.0;
    }
    while lon < -180.0 {
        lon += 360.0;
    }
    lon
}

/// Great-circle distance in kilometres. Used by the plausibility gates and by
/// the round-trip tests to express error as a distance rather than degrees.
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371.0088;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical pair from the ADS-B literature.
    const EVEN: CprFrame = CprFrame {
        lat: 93_000,
        lon: 51_372,
        odd: false,
    };
    const ODD: CprFrame = CprFrame {
        lat: 74_158,
        lon: 50_194,
        odd: true,
    };

    #[test]
    fn nl_matches_the_icao_table_at_known_points() {
        assert_eq!(nl(0.0), 59);
        assert_eq!(nl(10.0), 59);
        assert_eq!(nl(11.0), 58);
        assert_eq!(nl(87.0), 1);
        assert_eq!(nl(-87.0), 1);
        assert_eq!(nl(90.0), 1);
    }

    #[test]
    fn nl_is_symmetric_about_the_equator() {
        for lat in [5.0, 23.4, 45.42, 60.0, 86.9] {
            assert_eq!(nl(lat), nl(-lat), "NL must not depend on hemisphere");
        }
    }

    #[test]
    fn canonical_vector_even_anchor() {
        let (lat, lon) = decode_global(EVEN, ODD, false, CprKind::Airborne).unwrap();
        assert!((lat - 52.257_202).abs() < 1e-5, "lat was {lat}");
        assert!((lon - 3.919_373).abs() < 1e-5, "lon was {lon}");
    }

    #[test]
    fn canonical_vector_odd_anchor() {
        let (lat, lon) = decode_global(EVEN, ODD, true, CprKind::Airborne).unwrap();
        assert!((lat - 52.265_780).abs() < 1e-5, "lat was {lat}");
        assert!((lon - 3.938_913).abs() < 1e-5, "lon was {lon}");
    }

    /// The bug found on 2026-08-06 in the previous implementation, kept as a
    /// named regression test with the actual numbers.
    ///
    /// It used `d_lon = 360/NL` for both anchors. With the odd frame most
    /// recent and NL = 36 that gives 360/36 = 10.0 degrees per zone instead of
    /// the correct 360/35 = 10.2857, yielding 3.8295 where the answer is
    /// 3.9389 — a 7.4 km error at this latitude, and roughly half of all fixes
    /// are odd-anchored.
    #[test]
    fn regression_odd_anchor_longitude_divisor() {
        let (lat, lon) = decode_global(EVEN, ODD, true, CprKind::Airborne).unwrap();
        assert_eq!(nl(lat), 36, "the test depends on landing in NL=36");

        let wrong = 3.829_498;
        assert!(
            (lon - wrong).abs() > 0.1,
            "regressed to the NL-instead-of-NL-1 divisor: got {lon}, the bug gives {wrong}"
        );
        assert!((lon - 3.938_913).abs() < 1e-5, "lon was {lon}");
    }

    /// CPR is a lossy encoding, so a round trip can never be exact. The
    /// latitude step is `360/60/2^17` = 5.1 m, and the longitude step grows as
    /// the meridians converge — about 5 m at the equator, roughly 21 m at 86
    /// degrees where NL collapses to 1. Half-step rounding on both axes puts
    /// the worst honest error near 12 m.
    ///
    /// 30 m therefore accepts every correct implementation while still being
    /// three orders of magnitude tighter than the bug class this guards
    /// against — the odd-anchor divisor error is 7.4 km.
    const ROUND_TRIP_TOLERANCE_KM: f64 = 0.030;

    /// The test that would have caught the bug in one run, and will catch the
    /// next four. Encode a position, decode it back, assert it survives — for
    /// both anchors, everywhere on the globe.
    #[test]
    fn global_round_trip_both_anchors() {
        let mut checked = 0;
        let mut lat = -86.0;
        while lat <= 86.0 {
            let mut lon = -179.0;
            while lon < 180.0 {
                let even = encode(lat, lon, false, CprKind::Airborne);
                let odd = encode(lat, lon, true, CprKind::Airborne);

                for newest_odd in [false, true] {
                    let (dlat, dlon) = decode_global(even, odd, newest_odd, CprKind::Airborne)
                        .unwrap_or_else(|| panic!("no fix at {lat},{lon} odd={newest_odd}"));
                    let err = haversine_km(lat, lon, dlat, dlon);
                    assert!(
                        err < ROUND_TRIP_TOLERANCE_KM,
                        "round trip at {lat},{lon} (newest_odd={newest_odd}) \
                         landed at {dlat},{dlon}, {:.1} m away",
                        err * 1000.0
                    );
                    checked += 1;
                }
                lon += 7.3;
            }
            lat += 3.7;
        }
        assert!(checked > 2000, "only checked {checked} points");
    }

    /// Zone boundaries are where off-by-one errors hide, so sample them densely.
    #[test]
    fn round_trip_near_longitude_zone_boundaries() {
        for lat in [0.0, 10.47, 14.82, 45.42, 52.26, 86.9, -45.42] {
            let zones = f64::from(nl(lat));
            for k in 0..(zones as i32).min(20) {
                // Sit just inside each edge of a longitude zone.
                let width = 360.0 / zones;
                for offset in [0.001, width / 2.0, width - 0.001] {
                    let lon = normalize_lon(-180.0 + f64::from(k) * width + offset);
                    let even = encode(lat, lon, false, CprKind::Airborne);
                    let odd = encode(lat, lon, true, CprKind::Airborne);
                    for newest_odd in [false, true] {
                        if let Some((dlat, dlon)) =
                            decode_global(even, odd, newest_odd, CprKind::Airborne)
                        {
                            let err = haversine_km(lat, lon, dlat, dlon);
                            assert!(
                                err < ROUND_TRIP_TOLERANCE_KM,
                                "near boundary at {lat},{lon}: {:.1} m error",
                                err * 1000.0
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn local_decode_round_trips_near_a_reference() {
        for (lat, lon) in [(45.4215, -75.6972), (52.2572, 3.9194), (-33.86, 151.21)] {
            for odd in [false, true] {
                let frame = encode(lat, lon, odd, CprKind::Airborne);
                // A reference 50 km away, comfortably inside the half-zone limit.
                let (dlat, dlon) = decode_local(frame, lat + 0.4, lon + 0.4, CprKind::Airborne);
                let err = haversine_km(lat, lon, dlat, dlon);
                assert!(
                    err < ROUND_TRIP_TOLERANCE_KM,
                    "local decode at {lat},{lon}: {err:.4} km"
                );
            }
        }
    }

    /// Surface CPR trades range for resolution: it divides a 90-degree span
    /// rather than 360, so it is four times finer but globally ambiguous —
    /// there are four candidate positions, one per quadrant. That is fine in
    /// practice because an aircraft on the ground is at an airport, and the
    /// receiver knows roughly where it is.
    ///
    /// So the meaningful test is the local one, which is how surface positions
    /// are actually resolved.
    #[test]
    fn surface_positions_round_trip_locally() {
        // Ottawa (CYOW) and Heathrow.
        for (lat, lon) in [(45.3225, -75.6692), (51.4700, -0.4543)] {
            for odd in [false, true] {
                let frame = encode(lat, lon, odd, CprKind::Surface);
                let (dlat, dlon) = decode_local(frame, lat + 0.05, lon + 0.05, CprKind::Surface);
                let err = haversine_km(lat, lon, dlat, dlon);
                // Four times the airborne resolution, so hold it to a tighter bound.
                assert!(
                    err < ROUND_TRIP_TOLERANCE_KM / 4.0,
                    "surface local decode at {lat},{lon}: {:.1} m",
                    err * 1000.0
                );
            }
        }
    }

    /// Global surface decode should still resolve the position modulo the
    /// 90-degree quadrant ambiguity.
    #[test]
    fn surface_global_resolves_within_its_quadrant() {
        let (lat, lon) = (45.3225, -75.6692);
        let even = encode(lat, lon, false, CprKind::Surface);
        let odd = encode(lat, lon, true, CprKind::Surface);

        for newest_odd in [false, true] {
            let (dlat, dlon) = decode_global(even, odd, newest_odd, CprKind::Surface)
                .expect("surface pair should resolve");
            // Latitude is unambiguous within the 90-degree span.
            assert!(
                (dlat - lat).abs() < 0.001,
                "surface latitude was {dlat}, expected {lat}"
            );
            // Longitude is correct modulo the quadrant width.
            // Distance to the nearest multiple of 90 degrees.
            let residual = (dlon - lon).rem_euclid(90.0);
            let off_by = residual.min(90.0 - residual);
            assert!(
                off_by < 0.01,
                "surface longitude {dlon} is not congruent to {lon} mod 90 \
                 (off by {off_by})"
            );
        }
    }

    #[test]
    fn mismatched_longitude_bands_are_rejected() {
        // Two frames from wildly different latitudes cannot form a valid pair.
        let even = encode(10.0, 0.0, false, CprKind::Airborne);
        let odd = encode(80.0, 0.0, true, CprKind::Airborne);
        assert!(decode_global(even, odd, true, CprKind::Airborne).is_none());
    }

    #[test]
    fn haversine_matches_a_known_distance() {
        // Ottawa to Toronto, about 350 km.
        let d = haversine_km(45.4215, -75.6972, 43.6532, -79.3832);
        assert!((d - 351.0).abs() < 5.0, "got {d} km");
    }
}
