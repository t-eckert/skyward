//! A validated Mode S frame.
//!
//! Construction enforces that the byte count matches what the downlink format
//! demands, so every field accessor below can index without bounds anxiety.

use crate::{bits, crc, decode, units::Icao};

/// Downlink format — the first five bits of every Mode S transmission.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DownlinkFormat {
    /// DF0 — short air-air surveillance (ACAS).
    ShortAirAir,
    /// DF4 — surveillance, altitude reply.
    SurveillanceAltitude,
    /// DF5 — surveillance, identity reply (squawk).
    SurveillanceIdentity,
    /// DF11 — all-call reply. Carries the address in the clear.
    AllCallReply,
    /// DF16 — long air-air surveillance (ACAS).
    LongAirAir,
    /// DF17 — ADS-B extended squitter. The one we care about.
    ExtendedSquitter,
    /// DF18 — extended squitter from a non-transponder emitter (TIS-B etc).
    ExtendedSquitterNonTransponder,
    /// DF19 — military extended squitter.
    MilitaryExtendedSquitter,
    /// DF20 — Comm-B altitude reply.
    CommBAltitude,
    /// DF21 — Comm-B identity reply.
    CommBIdentity,
    /// DF24 — Comm-D extended length message.
    CommD,
    /// Anything unassigned. Almost always noise that happened to pass a
    /// preamble check.
    Other(u8),
}

impl DownlinkFormat {
    pub const fn from_raw(df: u8) -> Self {
        match df {
            0 => Self::ShortAirAir,
            4 => Self::SurveillanceAltitude,
            5 => Self::SurveillanceIdentity,
            11 => Self::AllCallReply,
            16 => Self::LongAirAir,
            17 => Self::ExtendedSquitter,
            18 => Self::ExtendedSquitterNonTransponder,
            19 => Self::MilitaryExtendedSquitter,
            20 => Self::CommBAltitude,
            21 => Self::CommBIdentity,
            24..=31 => Self::CommD,
            other => Self::Other(other),
        }
    }

    pub const fn describe(self) -> &'static str {
        match self {
            Self::ShortAirAir => "short air-air surveillance (ACAS)",
            Self::SurveillanceAltitude => "surveillance, altitude reply",
            Self::SurveillanceIdentity => "surveillance, identity reply",
            Self::AllCallReply => "all-call reply",
            Self::LongAirAir => "long air-air surveillance (ACAS)",
            Self::ExtendedSquitter => "ADS-B extended squitter",
            Self::ExtendedSquitterNonTransponder => "extended squitter, non-transponder",
            Self::MilitaryExtendedSquitter => "military extended squitter",
            Self::CommBAltitude => "Comm-B, altitude reply",
            Self::CommBIdentity => "Comm-B, identity reply",
            Self::CommD => "Comm-D (extended length message)",
            Self::Other(_) => "unassigned (probably noise)",
        }
    }
}

/// How many bytes a frame with this downlink format must contain.
///
/// The rule is simply DF >= 16, which is why the format lives in the top bit
/// of the first five: a receiver knows how long the burst will be after 5 µs.
pub const fn expected_len(df: u8) -> usize {
    if df >= 16 { 14 } else { 7 }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame is {got} bytes but downlink format {df} requires {want}")]
    WrongLength { df: u8, got: usize, want: usize },
    #[error("frame is empty")]
    Empty,
}

/// A Mode S frame whose length has been validated against its downlink format.
///
/// Holding a `Frame` does *not* imply the CRC passed — address-overlaid
/// formats (DF0/4/5/16/20/21) cannot be validated standalone at all. See
/// [`crc::remainder_is_address`].
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    bytes: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, FrameError> {
        let first = *bytes.first().ok_or(FrameError::Empty)?;
        let df = first >> 3;
        let want = expected_len(df);
        if bytes.len() != want {
            return Err(FrameError::WrongLength {
                df,
                got: bytes.len(),
                want,
            });
        }
        Ok(Frame { bytes })
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The raw 5-bit downlink format.
    pub fn df(&self) -> u8 {
        self.bytes[0] >> 3
    }

    pub fn format(&self) -> DownlinkFormat {
        DownlinkFormat::from_raw(self.df())
    }

    /// The transmitting aircraft's address.
    ///
    /// For DF11/17/18 this is carried in the clear. For the address-overlaid
    /// formats it is recovered from the CRC remainder — which means noise
    /// decodes to a plausible-looking address, so treat those with suspicion.
    pub fn icao(&self) -> Icao {
        if crc::remainder_is_address(self.df()) {
            Icao::new(crc::crc24(self.bytes))
        } else {
            Icao::new(bits::get(self.bytes, 8, 24))
        }
    }

    /// True if the CRC validates this frame outright.
    ///
    /// Only meaningful for DF17/18 (remainder must be zero) and DF11 (the
    /// remainder is the interrogator identifier, conventionally small).
    pub fn crc_ok(&self) -> bool {
        let remainder = crc::crc24(self.bytes);
        match self.df() {
            17 | 18 => remainder == 0,
            11 => remainder & 0x00FF_FF80 == 0,
            _ => false,
        }
    }

    /// The ADS-B type code, present only in extended squitters.
    pub fn type_code(&self) -> Option<u8> {
        matches!(self.df(), 17 | 18).then(|| bits::get(self.bytes, 32, 5) as u8)
    }

    /// Decode the payload, if we understand this message.
    pub fn decode(&self) -> Option<decode::Message> {
        decode::decode(self)
    }
}

impl std::fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("hex", &crate::bytes_to_hex(self.bytes))
            .field("df", &self.df())
            .field("icao", &self.icao())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex_to_bytes;

    #[test]
    fn parses_a_long_frame() {
        let bytes = hex_to_bytes("8D4840D6202CC371C32CE0576098").unwrap();
        let frame = Frame::new(&bytes).unwrap();
        assert_eq!(frame.df(), 17);
        assert_eq!(frame.format(), DownlinkFormat::ExtendedSquitter);
        assert_eq!(frame.icao().to_string(), "4840D6");
        assert_eq!(frame.type_code(), Some(4));
        assert!(frame.crc_ok());
    }

    #[test]
    fn parses_a_short_frame() {
        // A real DF11 all-call reply captured in Ottawa.
        let bytes = hex_to_bytes("5DC04413BFAD35").unwrap();
        let frame = Frame::new(&bytes).unwrap();
        assert_eq!(frame.df(), 11);
        assert_eq!(frame.icao().to_string(), "C04413");
        assert_eq!(frame.type_code(), None);
    }

    #[test]
    fn rejects_a_long_frame_given_short_bytes() {
        let bytes = hex_to_bytes("8D4840D6202CC3").unwrap();
        assert_eq!(
            Frame::new(&bytes).unwrap_err(),
            FrameError::WrongLength {
                df: 17,
                got: 7,
                want: 14
            }
        );
    }

    #[test]
    fn length_rule_is_df_16() {
        assert_eq!(expected_len(0), 7);
        assert_eq!(expected_len(11), 7);
        assert_eq!(expected_len(15), 7);
        assert_eq!(expected_len(16), 14);
        assert_eq!(expected_len(17), 14);
        assert_eq!(expected_len(31), 14);
    }
}
