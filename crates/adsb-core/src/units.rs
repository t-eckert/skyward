//! Newtypes for the quantities that are easy to mix up.
//!
//! The old implementation passed bare `u32` for ICAO addresses and bare `f32`
//! for both heading and track. Track and heading are genuinely different
//! things — they differ by the wind correction angle, sometimes by 15 degrees
//! — and ADS-B velocity messages carry *track*. Naming it wrong is the kind of
//! error that survives all the way onto a conference slide.

use std::fmt;

/// A 24-bit ICAO aircraft address. Globally unique, assigned by the registry
/// of the aircraft's country. Canonically written as six uppercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Icao(pub u32);

impl Icao {
    pub const fn new(raw: u32) -> Self {
        Icao(raw & 0x00FF_FFFF)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    /// `0x000000` is not a legitimate assignment and is what you get from
    /// decoding noise, so it is worth being able to reject explicitly.
    pub const fn is_plausible(self) -> bool {
        self.0 != 0 && self.0 != 0x00FF_FFFF
    }
}

impl fmt::Display for Icao {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06X}", self.0)
    }
}

impl fmt::Debug for Icao {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Icao({self})")
    }
}

/// Barometric or GNSS altitude in feet above mean sea level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Altitude {
    /// Pressure altitude, referenced to the 1013.25 hPa standard datum.
    Barometric(i32),
    /// GNSS height, reported by some position type codes.
    Gnss(i32),
    /// The field was present but carried the "no data" encoding, or used an
    /// encoding we do not decode yet (see [`crate::decode`]).
    Unavailable,
}

impl Altitude {
    pub const fn feet(self) -> Option<i32> {
        match self {
            Altitude::Barometric(ft) | Altitude::Gnss(ft) => Some(ft),
            Altitude::Unavailable => None,
        }
    }
}

/// Ground track in degrees clockwise from true north, `0.0..360.0`.
///
/// Not heading. ADS-B airborne velocity messages (type code 19, subtypes 1
/// and 2) report the direction the aircraft is *moving over the ground*, which
/// differs from where its nose points by the wind correction angle.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TrackDeg(pub f32);

/// Speed over the ground in knots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Knots(pub u16);

/// Rate of climb (positive) or descent (negative) in feet per minute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FeetPerMinute(pub i32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icao_formats_as_six_hex_digits() {
        assert_eq!(Icao::new(0x4840D6).to_string(), "4840D6");
        assert_eq!(Icao::new(0xC053F6).to_string(), "C053F6");
        // Leading zeros must be preserved -- 0xA2 is not "A2".
        assert_eq!(Icao::new(0xA2).to_string(), "0000A2");
    }

    #[test]
    fn icao_masks_to_24_bits() {
        assert_eq!(Icao::new(0xFF_4840D6).raw(), 0x4840D6);
    }

    #[test]
    fn implausible_icaos_are_flagged() {
        assert!(!Icao::new(0).is_plausible());
        assert!(Icao::new(0x4840D6).is_plausible());
    }
}
