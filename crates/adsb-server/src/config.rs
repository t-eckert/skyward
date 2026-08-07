//! Configuration, and where every value came from.
//!
//! # Provenance is the feature
//!
//! On a machine you cannot log into, the question is never "what is the
//! config" — it is "did my edit take effect". A settings dump that prints
//! values but not their origin cannot answer that, so every field here carries
//! the layer it was resolved from and `skyward config print` shows it:
//!
//! ```text
//! receiver.lat        45.412           /etc/skyward.toml:4
//! gain_db             44.5             SKYWARD_GAIN_DB
//! sample_rate_hz      2400000          default
//! ```
//!
//! # Two things that must fail rather than default
//!
//! **Unknown keys.** A typo'd key silently ignored is the classic blind-box
//! failure: you edit `recevier.lat`, restart, nothing changes, and there is no
//! signal at all. `deny_unknown_fields` turns that into a startup error.
//!
//! **A missing receiver position.** Defaulting to `0.0, 0.0` puts the station
//! in the Gulf of Guinea, which makes the range gate reject every aircraft on
//! earth — a total outage that looks like bad reception. Better to refuse to
//! start.

use serde::Deserialize;
use std::fmt;
use std::path::Path;

/// Which layer a value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Default,
    File(String),
    Env(&'static str),
    Cli,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Default => write!(f, "default"),
            Origin::File(path) => write!(f, "{path}"),
            Origin::Env(name) => write!(f, "${name}"),
            Origin::Cli => write!(f, "command line"),
        }
    }
}

/// A value together with where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Sourced<T> {
    pub value: T,
    pub origin: Origin,
}

impl<T> Sourced<T> {
    pub fn new(value: T, origin: Origin) -> Self {
        Sourced { value, origin }
    }

    /// Layer another value on top, if one was supplied.
    pub fn or_layer(self, candidate: Option<T>, origin: Origin) -> Self {
        match candidate {
            Some(value) => Sourced { value, origin },
            None => self,
        }
    }
}

impl<T> std::ops::Deref for Sourced<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

/// The on-disk file. Every field optional; unknown fields rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub schema: Option<i64>,
    pub source: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub frequency_hz: Option<u32>,
    pub gain_db: Option<String>,
    pub bind: Option<String>,
    pub db_path: Option<String>,
    pub cors_origins: Option<Vec<String>>,
    pub log_format: Option<String>,
    pub impl_set: Option<String>,
    #[serde(default)]
    pub receiver: FileReceiver,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileReceiver {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<f64>,
}

/// Values the CLI can override.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub source: Option<String>,
    pub sample_rate_hz: Option<u32>,
    pub gain_db: Option<String>,
    pub bind: Option<String>,
    pub db_path: Option<String>,
    pub log_format: Option<String>,
    pub impl_set: Option<String>,
}

/// The current schema version of the config file format.
pub const CONFIG_SCHEMA: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("configuration is invalid: {0}")]
    Invalid(String),
}

/// A fully resolved configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub source: Sourced<String>,
    pub sample_rate_hz: Sourced<u32>,
    pub frequency_hz: Sourced<u32>,
    pub gain_db: Sourced<String>,
    pub bind: Sourced<String>,
    pub db_path: Sourced<String>,
    pub cors_origins: Sourced<Vec<String>>,
    pub log_format: Sourced<String>,
    pub impl_set: Sourced<String>,
    pub receiver_lat: Sourced<Option<f64>>,
    pub receiver_lon: Sourced<Option<f64>>,
    pub receiver_alt_m: Sourced<f64>,
    /// Where the file layer came from, if any.
    pub config_path: Option<String>,
}

fn env_string(name: &'static str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

fn env_parsed<T: std::str::FromStr>(name: &'static str) -> Option<T> {
    env_string(name).and_then(|s| s.parse().ok())
}

impl Config {
    /// Resolve defaults, then file, then environment, then CLI.
    pub fn resolve(path: Option<&str>, cli: &CliOverrides) -> Result<Config, ConfigError> {
        let (file, file_origin) = match path {
            Some(p) if Path::new(p).exists() => {
                let text = std::fs::read_to_string(p).map_err(|e| ConfigError::Read {
                    path: p.to_string(),
                    source: e,
                })?;
                let parsed: FileConfig = toml::from_str(&text).map_err(|e| ConfigError::Parse {
                    path: p.to_string(),
                    message: describe_toml_error(&e),
                })?;
                if let Some(schema) = parsed.schema
                    && schema != CONFIG_SCHEMA
                {
                    return Err(ConfigError::Invalid(format!(
                        "{p} declares schema {schema}, this build expects {CONFIG_SCHEMA}"
                    )));
                }
                (parsed, Origin::File(p.to_string()))
            }
            // A path that was asked for but does not exist is an error; no
            // path at all is fine.
            Some(p) => {
                return Err(ConfigError::Invalid(format!(
                    "config file {p} does not exist"
                )));
            }
            None => (FileConfig::default(), Origin::Default),
        };

        let config = Config {
            source: Sourced::new("tcp:127.0.0.1:1234".to_string(), Origin::Default)
                .or_layer(file.source, file_origin.clone())
                .or_layer(env_string("SKYWARD_SOURCE"), Origin::Env("SKYWARD_SOURCE"))
                .or_layer(cli.source.clone(), Origin::Cli),

            sample_rate_hz: Sourced::new(2_400_000u32, Origin::Default)
                .or_layer(file.sample_rate_hz, file_origin.clone())
                .or_layer(
                    env_parsed("SKYWARD_SAMPLE_RATE_HZ"),
                    Origin::Env("SKYWARD_SAMPLE_RATE_HZ"),
                )
                .or_layer(cli.sample_rate_hz, Origin::Cli),

            frequency_hz: Sourced::new(1_090_000_000u32, Origin::Default)
                .or_layer(file.frequency_hz, file_origin.clone()),

            gain_db: Sourced::new("49.6".to_string(), Origin::Default)
                .or_layer(file.gain_db, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_GAIN_DB"),
                    Origin::Env("SKYWARD_GAIN_DB"),
                )
                .or_layer(cli.gain_db.clone(), Origin::Cli),

            bind: Sourced::new("0.0.0.0:8080".to_string(), Origin::Default)
                .or_layer(file.bind, file_origin.clone())
                .or_layer(env_string("SKYWARD_BIND"), Origin::Env("SKYWARD_BIND"))
                .or_layer(cli.bind.clone(), Origin::Cli),

            db_path: Sourced::new("skyward.db".to_string(), Origin::Default)
                .or_layer(file.db_path, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_DB_PATH"),
                    Origin::Env("SKYWARD_DB_PATH"),
                )
                .or_layer(cli.db_path.clone(), Origin::Cli),

            cors_origins: Sourced::new(vec!["http://localhost:5173".to_string()], Origin::Default)
                .or_layer(file.cors_origins, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_CORS_ORIGINS")
                        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect()),
                    Origin::Env("SKYWARD_CORS_ORIGINS"),
                ),

            log_format: Sourced::new("text".to_string(), Origin::Default)
                .or_layer(file.log_format, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_LOG_FORMAT"),
                    Origin::Env("SKYWARD_LOG_FORMAT"),
                )
                .or_layer(cli.log_format.clone(), Origin::Cli),

            impl_set: Sourced::new("baseline".to_string(), Origin::Default)
                .or_layer(file.impl_set, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_IMPL_SET"),
                    Origin::Env("SKYWARD_IMPL_SET"),
                )
                .or_layer(cli.impl_set.clone(), Origin::Cli),

            receiver_lat: Sourced::new(None, Origin::Default)
                .or_layer(file.receiver.lat.map(Some), file_origin.clone())
                .or_layer(
                    env_parsed::<f64>("SKYWARD_RECEIVER_LAT").map(Some),
                    Origin::Env("SKYWARD_RECEIVER_LAT"),
                ),

            receiver_lon: Sourced::new(None, Origin::Default)
                .or_layer(file.receiver.lon.map(Some), file_origin.clone())
                .or_layer(
                    env_parsed::<f64>("SKYWARD_RECEIVER_LON").map(Some),
                    Origin::Env("SKYWARD_RECEIVER_LON"),
                ),

            receiver_alt_m: Sourced::new(0.0, Origin::Default)
                .or_layer(file.receiver.altitude_m, file_origin.clone())
                .or_layer(
                    env_parsed("SKYWARD_RECEIVER_ALT_M"),
                    Origin::Env("SKYWARD_RECEIVER_ALT_M"),
                ),

            config_path: path.map(str::to_string),
        };

        config.validate()?;
        Ok(config)
    }

    /// Reject anything physically impossible before the radio is touched.
    pub fn validate(&self) -> Result<(), ConfigError> {
        adsb_source::SourceSpec::parse(&self.source)
            .map_err(|e| ConfigError::Invalid(e.to_string()))?;

        if *self.sample_rate_hz < 2_000_000 {
            return Err(ConfigError::Invalid(format!(
                "sample_rate_hz is {}, but Mode S needs at least 2 MS/s: a bit is one \
                 microsecond long, so below 2 samples per microsecond the two halves of \
                 a bit cannot be told apart",
                *self.sample_rate_hz
            )));
        }

        adsb_source::Gain::parse(&self.gain_db).map_err(|e| ConfigError::Invalid(e.to_string()))?;

        if !matches!(self.log_format.as_str(), "text" | "json") {
            return Err(ConfigError::Invalid(format!(
                "log_format '{}' must be 'text' or 'json'",
                *self.log_format
            )));
        }

        if adsb_dsp::registry::ImplSet::preset(&self.impl_set).is_none() {
            return Err(ConfigError::Invalid(format!(
                "unknown impl_set '{}'. Available: {}",
                *self.impl_set,
                adsb_dsp::registry::ImplSet::preset_names().join(", ")
            )));
        }

        // Latitude and longitude must arrive together or not at all.
        match (*self.receiver_lat, *self.receiver_lon) {
            (Some(lat), Some(lon)) => {
                if !(-90.0..=90.0).contains(&lat) {
                    return Err(ConfigError::Invalid(format!(
                        "receiver.lat {lat} is not a latitude"
                    )));
                }
                if !(-180.0..=180.0).contains(&lon) {
                    return Err(ConfigError::Invalid(format!(
                        "receiver.lon {lon} is not a longitude"
                    )));
                }
            }
            (None, None) => {}
            _ => {
                return Err(ConfigError::Invalid(
                    "receiver.lat and receiver.lon must both be set, or neither".into(),
                ));
            }
        }

        Ok(())
    }

    pub fn receiver(&self) -> Option<(f64, f64)> {
        match (*self.receiver_lat, *self.receiver_lon) {
            (Some(lat), Some(lon)) => Some((lat, lon)),
            _ => None,
        }
    }

    /// Render every value with its origin.
    pub fn print_resolved(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let mut row = |name: &str, value: String, origin: &Origin| {
            let _ = writeln!(out, "  {name:<22} {value:<28} {origin}");
        };

        row("source", self.source.value.clone(), &self.source.origin);
        row(
            "sample_rate_hz",
            self.sample_rate_hz.value.to_string(),
            &self.sample_rate_hz.origin,
        );
        row(
            "frequency_hz",
            self.frequency_hz.value.to_string(),
            &self.frequency_hz.origin,
        );
        row("gain_db", self.gain_db.value.clone(), &self.gain_db.origin);
        row("bind", self.bind.value.clone(), &self.bind.origin);
        row("db_path", self.db_path.value.clone(), &self.db_path.origin);
        row(
            "cors_origins",
            self.cors_origins.value.join(","),
            &self.cors_origins.origin,
        );
        row(
            "log_format",
            self.log_format.value.clone(),
            &self.log_format.origin,
        );
        row(
            "impl_set",
            self.impl_set.value.clone(),
            &self.impl_set.origin,
        );
        row(
            "receiver.lat",
            self.receiver_lat
                .value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "(unset)".into()),
            &self.receiver_lat.origin,
        );
        row(
            "receiver.lon",
            self.receiver_lon
                .value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "(unset)".into()),
            &self.receiver_lon.origin,
        );
        row(
            "receiver.altitude_m",
            self.receiver_alt_m.value.to_string(),
            &self.receiver_alt_m.origin,
        );
        out
    }
}

/// Turn a serde/toml error into something an operator can act on.
fn describe_toml_error(error: &toml::de::Error) -> String {
    let message = error.message().to_string();
    if message.contains("unknown field") {
        format!(
            "{message} (a typo here is silently ignored by most tools; this build refuses instead)"
        )
    } else {
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, contents: &str) -> String {
        let dir = std::env::temp_dir().join("skyward-config-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn defaults_resolve_without_a_file() {
        let c = Config::resolve(None, &CliOverrides::default()).unwrap();
        assert_eq!(*c.sample_rate_hz, 2_400_000);
        assert_eq!(c.sample_rate_hz.origin, Origin::Default);
        assert_eq!(*c.frequency_hz, 1_090_000_000);
        assert_eq!(c.receiver(), None);
    }

    #[test]
    fn a_file_overrides_defaults_and_records_its_path() {
        let path = write_temp(
            "basic.toml",
            r#"
                sample_rate_hz = 2000000
                [receiver]
                lat = 45.412
                lon = -75.679
            "#,
        );
        let c = Config::resolve(Some(&path), &CliOverrides::default()).unwrap();
        assert_eq!(*c.sample_rate_hz, 2_000_000);
        assert_eq!(c.sample_rate_hz.origin, Origin::File(path.clone()));
        assert_eq!(c.receiver(), Some((45.412, -75.679)));
        // Untouched fields still say so.
        assert_eq!(c.bind.origin, Origin::Default);
    }

    #[test]
    fn the_cli_wins_over_the_file() {
        let path = write_temp("precedence.toml", "sample_rate_hz = 2000000\n");
        let c = Config::resolve(
            Some(&path),
            &CliOverrides {
                sample_rate_hz: Some(2_560_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(*c.sample_rate_hz, 2_560_000);
        assert_eq!(c.sample_rate_hz.origin, Origin::Cli);
    }

    /// The classic blind-box failure: edit a key, restart, nothing happens.
    #[test]
    fn a_typo_in_a_key_is_a_hard_error() {
        let path = write_temp("typo.toml", "sample_rate_hertz = 2400000\n");
        let err = Config::resolve(Some(&path), &CliOverrides::default())
            .expect_err("an unknown key must not be ignored");
        let message = err.to_string();
        assert!(message.contains("sample_rate_hertz"), "{message}");
    }

    #[test]
    fn a_typo_in_a_nested_key_is_also_caught() {
        let path = write_temp("typo2.toml", "[receiver]\nlattitude = 45.0\n");
        assert!(Config::resolve(Some(&path), &CliOverrides::default()).is_err());
    }

    #[test]
    fn a_missing_config_file_is_an_error_not_a_silent_default() {
        let err = Config::resolve(Some("/nonexistent/skyward.toml"), &CliOverrides::default())
            .expect_err("should complain");
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn a_future_schema_version_is_refused() {
        let path = write_temp("schema.toml", "schema = 99\n");
        let err =
            Config::resolve(Some(&path), &CliOverrides::default()).expect_err("should refuse");
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn a_sample_rate_below_two_megasamples_is_refused_with_a_reason() {
        let path = write_temp("slow.toml", "sample_rate_hz = 1000000\n");
        let err = Config::resolve(Some(&path), &CliOverrides::default())
            .err()
            .unwrap();
        let message = err.to_string();
        assert!(message.contains("2 MS/s"), "{message}");
        // Explains *why*, so the operator does not just bump it blindly.
        assert!(message.contains("microsecond"), "{message}");
    }

    /// Half a coordinate is worse than none: it silently disables the gate or
    /// puts the station in the wrong hemisphere.
    #[test]
    fn half_a_receiver_position_is_refused() {
        let path = write_temp("halfpos.toml", "[receiver]\nlat = 45.412\n");
        let err = Config::resolve(Some(&path), &CliOverrides::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("both be set"), "{err}");
    }

    #[test]
    fn an_out_of_range_latitude_is_refused() {
        let path = write_temp("badlat.toml", "[receiver]\nlat = 145.0\nlon = -75.0\n");
        assert!(Config::resolve(Some(&path), &CliOverrides::default()).is_err());
    }

    #[test]
    fn an_unparseable_source_is_refused() {
        let path = write_temp("badsource.toml", "source = \"carrier-pigeon\"\n");
        let err = Config::resolve(Some(&path), &CliOverrides::default())
            .err()
            .unwrap();
        assert!(err.to_string().contains("carrier-pigeon"), "{err}");
    }

    #[test]
    fn an_unknown_impl_set_lists_the_alternatives() {
        let path = write_temp("badimpl.toml", "impl_set = \"turbo\"\n");
        let err = Config::resolve(Some(&path), &CliOverrides::default())
            .err()
            .unwrap();
        let message = err.to_string();
        assert!(
            message.contains("turbo") && message.contains("baseline"),
            "{message}"
        );
    }

    #[test]
    fn the_resolved_dump_shows_provenance() {
        let path = write_temp("dump.toml", "bind = \"127.0.0.1:9000\"\n");
        let c = Config::resolve(Some(&path), &CliOverrides::default()).unwrap();
        let dump = c.print_resolved();
        assert!(dump.contains("127.0.0.1:9000"));
        assert!(
            dump.contains(&path),
            "the file path should be shown: {dump}"
        );
        assert!(dump.contains("default"), "untouched fields say 'default'");
        assert!(dump.contains("(unset)"), "an unset receiver should say so");
    }
}
