//! The station's own position, changeable while the receiver is running.
//!
//! # Why this is not just a config value
//!
//! The receiver position is load-bearing: it is the reference for the 400 km
//! range gate, it is what local CPR resolves against, and it is the centre of
//! the ring the map draws. Getting it wrong does not produce an error, it
//! produces a receiver that appears to work and quietly rejects every aircraft
//! — the exact failure `config.rs` refuses to start rather than allow.
//!
//! It is also the one setting whose correct value is *discovered*, not
//! configured. You put the Pi somewhere, you move the antenna to the other
//! side of the house, a friend borrows the whole thing for a weekend. Making
//! that an edit to a `.env` on a machine you deliberately do not log into,
//! followed by a restart that drops every tracked aircraft, is the wrong shape
//! for the task.
//!
//! So the position lives here instead: resolved from config at startup,
//! overridable at runtime through `PUT /api/v1/receiver`, persisted to a small
//! file of its own so the change survives a restart, and revertible.
//!
//! # Precedence, and the trap in it
//!
//! The overlay file wins over `skyward.toml` and the environment. It has to —
//! otherwise a position set from the web interface would silently revert at
//! the next restart, which is worse than not offering the feature.
//!
//! But that inverts the usual mental model, and a stale overlay shadowing a
//! freshly edited `.env` is precisely the "my edit did nothing" failure this
//! codebase treats as a first-class bug. Two things make it visible rather
//! than mysterious: the origin travels with the value into `skyward config`
//! and `skyward doctor`, and `doctor` says so explicitly when the overlay
//! disagrees with what the rest of the configuration asked for.

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Where the station is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Station {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub altitude_m: f64,
}

impl Station {
    /// Reject anything that is not a point on the earth.
    ///
    /// Note what is *not* rejected: `0, 0`. It is a real coordinate in the
    /// Gulf of Guinea, and refusing it here would be guessing at intent. The
    /// difference from `config.rs` -- which refuses to default to it -- is
    /// that this value was typed by someone, and an unset position is
    /// expressed by clearing the station, not by sending zeroes.
    pub fn validate(&self) -> Result<(), String> {
        if !self.lat.is_finite() || !self.lon.is_finite() || !self.altitude_m.is_finite() {
            return Err("lat, lon and altitude_m must be finite numbers".into());
        }
        if !(-90.0..=90.0).contains(&self.lat) {
            return Err(format!("lat {} is not a latitude (-90 to 90)", self.lat));
        }
        if !(-180.0..=180.0).contains(&self.lon) {
            return Err(format!("lon {} is not a longitude (-180 to 180)", self.lon));
        }
        // The Dead Sea shore is -430 m; the highest permanent settlement is
        // about 5100 m. This is a generous sanity check on a field whose usual
        // error is metres-versus-feet.
        if !(-500.0..=9000.0).contains(&self.altitude_m) {
            return Err(format!(
                "altitude_m {} is implausible. This is metres above sea level, not feet",
                self.altitude_m
            ));
        }
        Ok(())
    }

    pub fn coords(&self) -> (f64, f64) {
        (self.lat, self.lon)
    }
}

/// The overlay file's contents. Shaped like `skyward.toml`'s `[receiver]`
/// table so that it reads as configuration rather than as a state blob, and
/// so its contents can be pasted straight into a config file.
#[derive(Debug, Serialize, Deserialize)]
struct Overlay {
    receiver: Station,
}

/// Where the position currently in force came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StationOrigin {
    /// Nothing anywhere set one.
    Unset,
    /// Resolved from defaults, the config file, or the environment.
    Configured(String),
    /// The runtime overlay file, written by a previous `PUT`.
    Overlay(String),
    /// Set through the API during this run, and persisted.
    Runtime,
}

impl StationOrigin {
    pub fn as_str(&self) -> &str {
        match self {
            StationOrigin::Unset => "unset",
            StationOrigin::Configured(origin) => origin,
            StationOrigin::Overlay(path) => path,
            StationOrigin::Runtime => "set at runtime",
        }
    }
}

/// The live position, shared between the API and the decoder thread.
pub struct StationState {
    current: ArcSwap<Option<Station>>,
    origin: ArcSwap<StationOrigin>,
    /// What configuration alone resolved to, so a `DELETE` has something to
    /// fall back to rather than simply unsetting the station.
    configured: Option<Station>,
    configured_origin: String,
    /// The overlay file. `None` disables persistence entirely, which is what
    /// tests and `--api-only` smoke runs want.
    path: Option<PathBuf>,
    /// Bumped on every accepted change.
    ///
    /// The decoder reads this once per publish tick — a single relaxed atomic
    /// load — and only touches the tracker when it moves. Polling the
    /// `ArcSwap` itself would also work; this makes "nothing changed" the
    /// cheapest possible path in a loop that runs at sample rate.
    generation: AtomicU64,
    /// Whether the API is allowed to change it.
    writable: bool,
}

impl StationState {
    /// Resolve the startup position: configuration, then the overlay on top.
    pub fn load(
        configured: Option<Station>,
        configured_origin: String,
        path: Option<PathBuf>,
        writable: bool,
    ) -> (Arc<StationState>, Option<String>) {
        let mut warning = None;
        let mut current = configured;
        let mut origin = match configured {
            Some(_) => StationOrigin::Configured(configured_origin.clone()),
            None => StationOrigin::Unset,
        };

        if let Some(path) = &path {
            match read_overlay(path) {
                Ok(Some(station)) => {
                    current = Some(station);
                    origin = StationOrigin::Overlay(path.display().to_string());
                }
                Ok(None) => {}
                Err(e) => {
                    // A corrupt overlay must not stop the receiver. Losing the
                    // position is bad; refusing to decode at all is worse, and
                    // the configured value is still right there.
                    warning = Some(format!(
                        "ignoring {}: {e}. Falling back to the configured position",
                        path.display()
                    ));
                }
            }
        }

        let state = Arc::new(StationState {
            current: ArcSwap::from_pointee(current),
            origin: ArcSwap::from_pointee(origin),
            configured,
            configured_origin,
            path,
            generation: AtomicU64::new(0),
            writable,
        });
        (state, warning)
    }

    pub fn get(&self) -> Option<Station> {
        **self.current.load()
    }

    pub fn coords(&self) -> Option<(f64, f64)> {
        self.get().map(|s| s.coords())
    }

    pub fn origin(&self) -> StationOrigin {
        (**self.origin.load()).clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    /// The position configuration alone would have produced.
    pub fn configured(&self) -> Option<Station> {
        self.configured
    }

    pub fn configured_origin(&self) -> &str {
        &self.configured_origin
    }

    /// Whether a persisted overlay is currently shadowing a *different*
    /// configured value. This is the "my edit did nothing" case, and `doctor`
    /// reports it.
    pub fn overlay_shadows_config(&self) -> bool {
        matches!(
            self.origin(),
            StationOrigin::Overlay(_) | StationOrigin::Runtime
        ) && self.configured.is_some()
            && self.configured != self.get()
    }

    /// Move the station, persisting the change.
    ///
    /// Persistence failure is reported rather than swallowed: a position that
    /// works now and silently reverts on the next reboot is worse than one
    /// that refuses to be set, because the reversion happens weeks later with
    /// no connection to the act that caused it.
    pub fn set(&self, station: Station) -> Result<(), String> {
        if !self.writable {
            return Err("this receiver was started with station writes disabled \
                 (station_writable = false)"
                .into());
        }
        station.validate()?;

        if let Some(path) = &self.path {
            write_overlay(path, station)?;
        }

        self.current.store(Arc::new(Some(station)));
        self.origin.store(Arc::new(StationOrigin::Runtime));
        self.generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Discard the overlay and go back to what configuration says.
    pub fn clear(&self) -> Result<(), String> {
        if !self.writable {
            return Err("this receiver was started with station writes disabled \
                 (station_writable = false)"
                .into());
        }

        if let Some(path) = &self.path
            && path.exists()
        {
            std::fs::remove_file(path)
                .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        }

        self.current.store(Arc::new(self.configured));
        self.origin.store(Arc::new(match self.configured {
            Some(_) => StationOrigin::Configured(self.configured_origin.clone()),
            None => StationOrigin::Unset,
        }));
        self.generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn read_overlay(path: &Path) -> Result<Option<Station>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read it: {e}"))?;
    let overlay: Overlay = toml::from_str(&text).map_err(|e| format!("cannot parse it: {e}"))?;
    overlay.receiver.validate()?;
    Ok(Some(overlay.receiver))
}

/// Write the overlay atomically.
///
/// Through a temporary file and a rename, because the alternative is a power
/// cut halfway through a 90-byte write leaving a truncated file that fails to
/// parse at the next boot — on a Raspberry Pi, where sudden power loss is the
/// normal way the machine turns off.
fn write_overlay(path: &Path, station: Station) -> Result<(), String> {
    let body = format!(
        "# Written by skyward when the station position was set through the API.\n\
         # Delete this file to fall back to skyward.toml and the environment,\n\
         # or send DELETE /api/v1/receiver, which does the same thing.\n\
         \n\
         [receiver]\n\
         lat = {}\n\
         lon = {}\n\
         altitude_m = {}\n",
        station.lat, station.lon, station.altitude_m
    );

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }

    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, body)
        .map_err(|e| format!("cannot write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("cannot replace {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skyward-station-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ottawa() -> Station {
        Station {
            lat: 45.421,
            lon: -75.697,
            altitude_m: 70.0,
        }
    }

    #[test]
    fn a_set_position_survives_a_restart() {
        let path = temp_dir("restart").join("station.toml");
        let (state, warning) = StationState::load(None, "default".into(), Some(path.clone()), true);
        assert!(warning.is_none());
        assert_eq!(state.get(), None);

        state.set(ottawa()).unwrap();
        assert_eq!(state.get(), Some(ottawa()));

        // A fresh process, same file: this is the whole reason the overlay
        // exists. Without it, a position set from the web interface would
        // silently revert at the next reboot.
        let (restarted, warning) = StationState::load(None, "default".into(), Some(path), true);
        assert!(warning.is_none());
        assert_eq!(restarted.get(), Some(ottawa()));
        assert!(matches!(restarted.origin(), StationOrigin::Overlay(_)));
    }

    #[test]
    fn the_overlay_wins_over_configuration_and_says_so() {
        let path = temp_dir("precedence").join("station.toml");
        let troy = Station {
            lat: 41.788,
            lon: -76.790,
            altitude_m: 340.0,
        };
        let (state, _) = StationState::load(
            Some(troy),
            "$SKYWARD_RECEIVER_LAT".into(),
            Some(path.clone()),
            true,
        );
        assert_eq!(state.get(), Some(troy));
        assert!(!state.overlay_shadows_config());

        state.set(ottawa()).unwrap();
        let (restarted, _) =
            StationState::load(Some(troy), "$SKYWARD_RECEIVER_LAT".into(), Some(path), true);
        assert_eq!(restarted.get(), Some(ottawa()));
        assert!(
            restarted.overlay_shadows_config(),
            "an overlay hiding a different configured value is the 'my edit did \
             nothing' failure, and has to be reportable"
        );
    }

    #[test]
    fn clearing_reverts_to_configuration_and_removes_the_file() {
        let path = temp_dir("clear").join("station.toml");
        let troy = Station {
            lat: 41.788,
            lon: -76.790,
            altitude_m: 340.0,
        };
        let (state, _) =
            StationState::load(Some(troy), "skyward.toml".into(), Some(path.clone()), true);
        state.set(ottawa()).unwrap();
        assert!(path.exists());

        state.clear().unwrap();
        assert_eq!(state.get(), Some(troy));
        assert!(
            !path.exists(),
            "clear must not leave the file behind to be \
                                 picked up again at the next restart"
        );
        assert!(matches!(state.origin(), StationOrigin::Configured(_)));
    }

    #[test]
    fn every_change_bumps_the_generation_so_the_decoder_notices() {
        let path = temp_dir("generation").join("station.toml");
        let (state, _) = StationState::load(None, "default".into(), Some(path), true);
        let before = state.generation();
        state.set(ottawa()).unwrap();
        assert!(state.generation() > before);
        let after_set = state.generation();
        state.clear().unwrap();
        assert!(state.generation() > after_set);
    }

    #[test]
    fn implausible_positions_are_refused() {
        for (station, why) in [
            (
                Station {
                    lat: 91.0,
                    lon: 0.0,
                    altitude_m: 0.0,
                },
                "latitude",
            ),
            (
                Station {
                    lat: 0.0,
                    lon: 181.0,
                    altitude_m: 0.0,
                },
                "longitude",
            ),
            (
                Station {
                    lat: 0.0,
                    lon: 0.0,
                    altitude_m: f64::NAN,
                },
                "NaN",
            ),
            // 300 feet entered as metres is fine; 300 metres entered as feet
            // is not detectable. 30000 is, and is the common paste error.
            (
                Station {
                    lat: 0.0,
                    lon: 0.0,
                    altitude_m: 30_000.0,
                },
                "altitude",
            ),
        ] {
            assert!(station.validate().is_err(), "{why} should be refused");
        }
    }

    /// A position at 0,0 is a real place, and the API must not second-guess
    /// someone who types it. Unsetting is what `clear` is for.
    #[test]
    fn null_island_is_accepted_because_it_is_a_coordinate() {
        assert!(
            Station {
                lat: 0.0,
                lon: 0.0,
                altitude_m: 0.0
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn a_corrupt_overlay_warns_and_falls_back_rather_than_refusing_to_start() {
        let path = temp_dir("corrupt").join("station.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        let troy = Station {
            lat: 41.788,
            lon: -76.790,
            altitude_m: 340.0,
        };
        let (state, warning) =
            StationState::load(Some(troy), "skyward.toml".into(), Some(path), true);
        assert_eq!(
            state.get(),
            Some(troy),
            "a bad overlay must not cost us a position we already have"
        );
        assert!(warning.expect("should warn").contains("Falling back"));
    }

    /// Half a write, then power loss. The Pi's normal shutdown method.
    #[test]
    fn a_truncated_overlay_is_never_left_behind() {
        let dir = temp_dir("atomic");
        let path = dir.join("station.toml");
        write_overlay(&path, ottawa()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            leftovers,
            vec!["station.toml".to_string()],
            "the temporary file must not survive a successful write"
        );
    }

    #[test]
    fn a_read_only_station_refuses_both_writes() {
        let path = temp_dir("readonly").join("station.toml");
        let (state, _) = StationState::load(None, "default".into(), Some(path.clone()), false);
        assert!(state.set(ottawa()).is_err());
        assert!(state.clear().is_err());
        assert!(!path.exists(), "a refused write must not touch the disk");
    }

    #[test]
    fn the_overlay_round_trips_through_toml() {
        let path = temp_dir("roundtrip").join("station.toml");
        write_overlay(&path, ottawa()).unwrap();
        assert_eq!(read_overlay(&path).unwrap(), Some(ottawa()));
    }
}
