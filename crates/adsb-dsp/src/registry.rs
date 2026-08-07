//! Named implementations, selected at runtime.
//!
//! Each stage is a registry rather than a fixed pair of choices, because the
//! interesting comparison is usually against *your own previous attempt* —
//! `correlator-v3` against `correlator-v2` — not against the baseline you
//! passed three weeks ago.
//!
//! # Adding one
//!
//! 1. Write the type and implement the stage trait.
//! 2. Add one arm to the relevant `match` below and one row to its `*_NAMES`.
//!
//! That is the whole ceremony. Then:
//!
//! ```text
//! skyward bench --detect correlator-v2 --compare runs/baseline.json
//! ```
//!
//! # Unknown names are fatal
//!
//! An unrecognised implementation name is a hard error listing the valid ones,
//! never a silent fallback to the default. On a Raspberry Pi you cannot debug
//! interactively, and "it ran but quietly used something else" is precisely
//! the failure that wastes an evening.

use crate::{
    Pipeline,
    detect::{NaiveDetector, PreambleDetector},
    magnitude::{Magnitude, NaiveMagnitude},
    slice::{BitSlicer, NaiveSlicer},
    validate::{CrcOnlyValidator, FrameValidator},
};

/// A full selection of implementations, one per stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplSet {
    pub magnitude: String,
    pub detector: String,
    pub slicer: String,
    pub validator: String,
}

impl ImplSet {
    /// The control. Simple, explainable, and deliberately not tuned — a fair
    /// opponent rather than a strawman.
    pub fn baseline() -> Self {
        ImplSet {
            magnitude: "naive".into(),
            detector: "naive".into(),
            slicer: "naive".into(),
            validator: "crc-only".into(),
        }
    }

    /// Resolve a named preset. Presets exist mainly for the Pi: a config file
    /// names one word, and `doctor` prints the full expansion so there is
    /// never any doubt about what is actually running.
    pub fn preset(name: &str) -> Option<Self> {
        match name {
            "baseline" => Some(Self::baseline()),
            // Add "thomas" here once there is something to put in it.
            _ => None,
        }
    }

    pub fn preset_names() -> &'static [&'static str] {
        &["baseline"]
    }
}

impl std::fmt::Display for ImplSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mag={} detect={} slice={} validate={}",
            self.magnitude, self.detector, self.slicer, self.validator
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("unknown {stage} implementation '{name}'. Available: {available}")]
    Unknown {
        stage: &'static str,
        name: String,
        available: String,
    },
}

fn unknown(stage: &'static str, name: &str, available: &[(&str, &str)]) -> RegistryError {
    RegistryError::Unknown {
        stage,
        name: name.to_string(),
        available: available
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

pub const MAGNITUDE_NAMES: &[(&str, &str)] =
    &[("naive", "sqrt(i^2+q^2) in f32; the correctness reference")];

pub const DETECTOR_NAMES: &[(&str, &str)] = &[(
    "naive",
    "half-microsecond slot means; min(pulse) > 2x max(silence), absolute threshold",
)];

pub const SLICER_NAMES: &[(&str, &str)] = &[(
    "naive",
    "one sample per half-bit, compared directly; reports no confidence",
)];

pub const VALIDATOR_NAMES: &[(&str, &str)] = &[(
    "crc-only",
    "accept only an exact CRC match; cannot invent aircraft",
)];

pub fn magnitude(name: &str) -> Result<Box<dyn Magnitude>, RegistryError> {
    match name {
        "naive" => Ok(Box::new(NaiveMagnitude)),
        other => Err(unknown("magnitude", other, MAGNITUDE_NAMES)),
    }
}

pub fn detector(name: &str, sample_rate: u32) -> Result<Box<dyn PreambleDetector>, RegistryError> {
    match name {
        "naive" => Ok(Box::new(NaiveDetector::new(sample_rate))),
        other => Err(unknown("detector", other, DETECTOR_NAMES)),
    }
}

pub fn slicer(name: &str, sample_rate: u32) -> Result<Box<dyn BitSlicer>, RegistryError> {
    match name {
        "naive" => Ok(Box::new(NaiveSlicer::new(sample_rate))),
        other => Err(unknown("slicer", other, SLICER_NAMES)),
    }
}

pub fn validator(name: &str) -> Result<Box<dyn FrameValidator>, RegistryError> {
    match name {
        "crc-only" => Ok(Box::new(CrcOnlyValidator)),
        other => Err(unknown("validator", other, VALIDATOR_NAMES)),
    }
}

/// Build a pipeline from a named selection.
pub fn build(set: &ImplSet, sample_rate: u32) -> Result<Pipeline, RegistryError> {
    Ok(Pipeline::new(
        magnitude(&set.magnitude)?,
        detector(&set.detector, sample_rate)?,
        slicer(&set.slicer, sample_rate)?,
        validator(&set.validator)?,
        sample_rate,
    ))
}

/// Everything registered, for `--list-impls`.
pub fn describe_all() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for (stage, entries) in [
        ("magnitude (--mag)", MAGNITUDE_NAMES),
        ("detector  (--detect)", DETECTOR_NAMES),
        ("slicer    (--slice)", SLICER_NAMES),
        ("validator (--validate)", VALIDATOR_NAMES),
    ] {
        let _ = writeln!(s, "{stage}:");
        for (name, description) in entries {
            let _ = writeln!(s, "  {name:<16} {description}");
        }
    }
    let _ = writeln!(s, "presets   (--impl-set):");
    for name in ImplSet::preset_names() {
        let _ = writeln!(s, "  {name}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_builds() {
        assert!(build(&ImplSet::baseline(), 2_400_000).is_ok());
    }

    #[test]
    fn unknown_names_fail_loudly_and_list_the_alternatives() {
        // `.err()` rather than `.unwrap_err()`: the latter needs the Ok type to
        // implement Debug, and `Box<dyn PreambleDetector>` deliberately does not.
        let err = detector("corelator", 2_400_000)
            .err()
            .expect("an unknown name must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("corelator"), "should echo the typo: {msg}");
        assert!(
            msg.contains("naive"),
            "should list what is available: {msg}"
        );
    }

    #[test]
    fn every_registered_name_actually_builds() {
        // Guards against adding a row to the *_NAMES table and forgetting the
        // match arm -- which would only surface on the Pi.
        for (name, _) in MAGNITUDE_NAMES {
            assert!(magnitude(name).is_ok(), "magnitude '{name}' not wired up");
        }
        for (name, _) in DETECTOR_NAMES {
            assert!(
                detector(name, 2_400_000).is_ok(),
                "detector '{name}' not wired up"
            );
        }
        for (name, _) in SLICER_NAMES {
            assert!(
                slicer(name, 2_400_000).is_ok(),
                "slicer '{name}' not wired up"
            );
        }
        for (name, _) in VALIDATOR_NAMES {
            assert!(validator(name).is_ok(), "validator '{name}' not wired up");
        }
    }

    #[test]
    fn every_preset_resolves_and_builds() {
        for name in ImplSet::preset_names() {
            let set = ImplSet::preset(name).unwrap_or_else(|| panic!("preset '{name}' missing"));
            assert!(build(&set, 2_400_000).is_ok(), "preset '{name}' broken");
        }
    }

    #[test]
    fn names_reported_by_impls_match_their_registry_keys() {
        // A mismatch would make result provenance lie about what ran.
        assert_eq!(magnitude("naive").unwrap().name(), "naive");
        assert_eq!(detector("naive", 2_400_000).unwrap().name(), "naive");
        assert_eq!(slicer("naive", 2_400_000).unwrap().name(), "naive");
        assert_eq!(validator("crc-only").unwrap().name(), "crc-only");
    }
}
