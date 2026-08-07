//! Payload decoding for ADS-B extended squitters.
//!
//! Bit offsets below are 0-based from the start of the 112-bit frame. The
//! ADS-B "ME" payload occupies bits 32..88; the type code is its first five
//! bits and selects everything that follows.

use crate::{
    bits,
    cpr::{CprFrame, CprKind},
    frame::Frame,
    units::{Altitude, FeetPerMinute, Knots, TrackDeg},
};

/// A decoded ADS-B payload.
#[derive(Clone, PartialEq, Debug)]
pub enum Message {
    /// Type codes 1..4. The callsign an aircraft files its flight plan under.
    Identification {
        callsign: String,
        /// Wake-turbulence / emitter category, as (type_code, category).
        category: (u8, u8),
    },
    /// Type codes 9..18 (barometric) and 20..22 (GNSS height).
    AirbornePosition { altitude: Altitude, cpr: CprFrame },
    /// Type codes 5..8. Uses the finer 90-degree CPR grid.
    SurfacePosition {
        cpr: CprFrame,
        ground_speed: Option<Knots>,
        track: Option<TrackDeg>,
    },
    /// Type code 19, subtypes 1 and 2.
    Velocity {
        ground_speed: Option<Knots>,
        track: Option<TrackDeg>,
        vertical_rate: Option<FeetPerMinute>,
        /// True when the vertical rate came from GNSS rather than the
        /// barometer. They disagree, sometimes substantially.
        gnss_vertical_rate: bool,
    },
    /// Type code 19, subtypes 3 and 4 — airspeed rather than ground velocity,
    /// transmitted when the aircraft has no valid ground-referenced solution.
    Airspeed {
        heading: Option<TrackDeg>,
        airspeed: Option<Knots>,
        vertical_rate: Option<FeetPerMinute>,
    },
    /// Recognised but not decoded. Kept so the scoreboard can show what is
    /// being left on the table rather than silently discarding it.
    Unsupported { type_code: u8 },
}

/// Decode an extended squitter payload. Returns `None` for downlink formats
/// that carry no ADS-B payload.
pub fn decode(frame: &Frame) -> Option<Message> {
    let tc = frame.type_code()?;
    let b = frame.bytes();

    Some(match tc {
        1..=4 => identification(b, tc),
        5..=8 => surface_position(b),
        9..=18 => airborne_position(b, false),
        19 => velocity(b),
        20..=22 => airborne_position(b, true),
        _ => Message::Unsupported { type_code: tc },
    })
}

/// The 6-bit character set used by callsigns. Index 0 and the `#` slots are
/// invalid encodings; index 32 is space, which pads short callsigns.
const CALLSIGN_CHARSET: &[u8; 64] =
    b"#ABCDEFGHIJKLMNOPQRSTUVWXYZ##### ###############0123456789######";

fn identification(b: &[u8], tc: u8) -> Message {
    let category = bits::get(b, 37, 3) as u8;
    let mut callsign = String::with_capacity(8);
    for i in 0..8 {
        let index = bits::get(b, 40 + i * 6, 6) as usize;
        let ch = CALLSIGN_CHARSET[index] as char;
        // '#' marks a reserved encoding. Emitting it would put junk on a map,
        // so drop it -- but keep spaces so interior padding is visible.
        if ch != '#' {
            callsign.push(ch);
        }
    }
    Message::Identification {
        callsign: callsign.trim().to_string(),
        category: (tc, category),
    }
}

fn airborne_position(b: &[u8], gnss: bool) -> Message {
    Message::AirbornePosition {
        altitude: altitude_12bit(b, gnss),
        cpr: CprFrame {
            odd: bits::flag(b, 53),
            lat: bits::get(b, 54, 17),
            lon: bits::get(b, 71, 17),
        },
    }
}

/// The 12-bit altitude field at bits 40..52.
///
/// Bit 47 is the Q bit. Q=1 means 25-foot steps, which is what essentially
/// every airliner uses. Q=0 selects a 100-foot Gillham (reflected Gray) code,
/// used only above 50175 ft; we report it as unavailable rather than ship a
/// decoder with no test vectors to verify it against. See `docs/LATER.md`.
fn altitude_12bit(b: &[u8], gnss: bool) -> Altitude {
    let raw = bits::get(b, 40, 12);
    if raw == 0 {
        return Altitude::Unavailable;
    }
    if !bits::flag(b, 47) {
        return Altitude::Unavailable;
    }
    // Drop the Q bit and close the gap: 7 bits above it, 4 below.
    let n = ((bits::get(b, 40, 7) << 4) | bits::get(b, 48, 4)) as i32;
    let feet = n * 25 - 1000;
    if gnss {
        Altitude::Gnss(feet)
    } else {
        Altitude::Barometric(feet)
    }
}

fn surface_position(b: &[u8]) -> Message {
    // Movement is a 7-bit non-linear encoding at bits 37..44.
    let movement = bits::get(b, 37, 7);
    let ground_speed = surface_movement_kt(movement).map(Knots);

    // Track is valid only when the status bit at 44 is set.
    let track = bits::flag(b, 44).then(|| {
        let raw = bits::get(b, 45, 7);
        TrackDeg(raw as f32 * 360.0 / 128.0)
    });

    Message::SurfacePosition {
        cpr: CprFrame {
            odd: bits::flag(b, 53),
            lat: bits::get(b, 54, 17),
            lon: bits::get(b, 71, 17),
        },
        ground_speed,
        track,
    }
}

/// Surface movement uses piecewise-linear buckets so that slow taxi speeds get
/// fine resolution and fast ones do not.
fn surface_movement_kt(raw: u32) -> Option<u16> {
    let kt = match raw {
        0 => return None, // not available
        1 => 0.0,         // stopped
        2..=8 => 0.125 + f64::from(raw - 2) * 0.125,
        9..=12 => 1.0 + f64::from(raw - 9) * 0.25,
        13..=38 => 2.0 + f64::from(raw - 13) * 0.5,
        39..=93 => 15.0 + f64::from(raw - 39) * 1.0,
        94..=108 => 70.0 + f64::from(raw - 94) * 2.0,
        109..=123 => 100.0 + f64::from(raw - 109) * 5.0,
        124 => 175.0,
        _ => return None, // 125..127 reserved
    };
    Some(kt.round() as u16)
}

fn velocity(b: &[u8]) -> Message {
    let subtype = bits::get(b, 37, 3);

    // Vertical rate is laid out the same way for every subtype.
    let vertical_rate = {
        let raw = bits::get(b, 69, 9) as i32;
        (raw != 0).then(|| {
            let magnitude = (raw - 1) * 64;
            FeetPerMinute(if bits::flag(b, 68) {
                -magnitude
            } else {
                magnitude
            })
        })
    };
    let gnss_vertical_rate = !bits::flag(b, 67);

    match subtype {
        1 | 2 => {
            // Supersonic subtype reports in 4-knot units.
            let scale = if subtype == 2 { 4 } else { 1 };

            let ew_raw = bits::get(b, 46, 10) as i32;
            let ns_raw = bits::get(b, 57, 10) as i32;

            // Zero means "no velocity information", not "stationary".
            if ew_raw == 0 || ns_raw == 0 {
                return Message::Velocity {
                    ground_speed: None,
                    track: None,
                    vertical_rate,
                    gnss_vertical_rate,
                };
            }

            let ew = (ew_raw - 1) * scale * if bits::flag(b, 45) { -1 } else { 1 };
            let ns = (ns_raw - 1) * scale * if bits::flag(b, 56) { -1 } else { 1 };

            let speed = f64::from(ew * ew + ns * ns).sqrt().round() as u16;
            let mut track = f64::from(ew).atan2(f64::from(ns)).to_degrees();
            if track < 0.0 {
                track += 360.0;
            }

            Message::Velocity {
                ground_speed: Some(Knots(speed)),
                track: Some(TrackDeg(track as f32)),
                vertical_rate,
                gnss_vertical_rate,
            }
        }
        3 | 4 => {
            let heading =
                bits::flag(b, 45).then(|| TrackDeg(bits::get(b, 46, 10) as f32 * 360.0 / 1024.0));
            let raw = bits::get(b, 57, 10) as i32;
            let airspeed =
                (raw != 0).then(|| Knots(((raw - 1) * if subtype == 4 { 4 } else { 1 }) as u16));
            Message::Airspeed {
                heading,
                airspeed,
                vertical_rate,
            }
        }
        _ => Message::Unsupported { type_code: 19 },
    }
}

/// Convenience: the CPR grid a message's position belongs to.
pub const fn cpr_kind(type_code: u8) -> CprKind {
    if type_code >= 5 && type_code <= 8 {
        CprKind::Surface
    } else {
        CprKind::Airborne
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, hex_to_bytes};

    fn decode_hex(hex: &str) -> Message {
        let bytes = hex_to_bytes(hex).unwrap();
        let frame = Frame::new(&bytes).unwrap();
        assert!(frame.crc_ok(), "{hex} failed CRC");
        frame.decode().unwrap()
    }

    #[test]
    fn canonical_identification() {
        match decode_hex("8D4840D6202CC371C32CE0576098") {
            Message::Identification { callsign, category } => {
                assert_eq!(callsign, "KLM1023");
                assert_eq!(category, (4, 0));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn canonical_airborne_position() {
        match decode_hex("8D40621D58C382D690C8AC2863A7") {
            Message::AirbornePosition { altitude, cpr } => {
                assert_eq!(altitude, Altitude::Barometric(38000));
                assert!(!cpr.odd);
                assert_eq!(cpr.lat, 93_000);
                assert_eq!(cpr.lon, 51_372);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn canonical_odd_position() {
        match decode_hex("8D40621D58C386435CC412692AD6") {
            Message::AirbornePosition { cpr, .. } => {
                assert!(cpr.odd);
                assert_eq!(cpr.lat, 74_158);
                assert_eq!(cpr.lon, 50_194);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn canonical_velocity() {
        match decode_hex("8D485020994409940838175B284F") {
            Message::Velocity {
                ground_speed,
                track,
                vertical_rate,
                ..
            } => {
                assert_eq!(ground_speed, Some(Knots(159)));
                let track = track.unwrap().0;
                assert!((track - 182.88).abs() < 0.1, "track was {track}");
                assert_eq!(vertical_rate, Some(FeetPerMinute(-832)));
            }
            other => panic!("got {other:?}"),
        }
    }

    /// Captured off the air in Ottawa on 2026-08-06. The aircraft was climbing
    /// out to the northeast; these are the numbers the lab tool reported.
    #[test]
    fn real_ottawa_velocity() {
        match decode_hex("8DC053F699093B19D030164300A1") {
            Message::Velocity {
                ground_speed,
                track,
                vertical_rate,
                ..
            } => {
                // The exploratory lab tool reported 374 because it truncated;
                // rounding to nearest is correct, and the true value is ~374.5.
                assert_eq!(ground_speed, Some(Knots(375)));
                assert!((track.unwrap().0 - 57.0).abs() < 1.0);
                assert_eq!(vertical_rate, Some(FeetPerMinute(704)));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn real_ottawa_position_altitude() {
        match decode_hex("8DC060B6587D8236085C837FDA27") {
            Message::AirbornePosition { altitude, cpr } => {
                assert_eq!(altitude, Altitude::Barometric(24000));
                assert!(!cpr.odd);
                assert_eq!(cpr.lat, 72_452);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn unsupported_type_codes_are_reported_not_dropped() {
        // TC31 operational status, captured in Ottawa.
        match decode_hex("8DC053F6F82300020049B8A00CD4") {
            Message::Unsupported { type_code } => assert_eq!(type_code, 31),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn surface_movement_buckets() {
        assert_eq!(surface_movement_kt(0), None);
        assert_eq!(surface_movement_kt(1), Some(0));
        assert_eq!(surface_movement_kt(124), Some(175));
        assert_eq!(surface_movement_kt(125), None);
        // Monotonic across bucket boundaries.
        let mut last = 0;
        for raw in 1..=124 {
            if let Some(kt) = surface_movement_kt(raw) {
                assert!(kt >= last, "movement went backwards at {raw}");
                last = kt;
            }
        }
    }

    #[test]
    fn callsign_charset_is_64_entries() {
        assert_eq!(CALLSIGN_CHARSET.len(), 64);
        assert_eq!(CALLSIGN_CHARSET[1], b'A');
        assert_eq!(CALLSIGN_CHARSET[26], b'Z');
        assert_eq!(CALLSIGN_CHARSET[32], b' ');
        assert_eq!(CALLSIGN_CHARSET[48], b'0');
        assert_eq!(CALLSIGN_CHARSET[57], b'9');
    }
}
