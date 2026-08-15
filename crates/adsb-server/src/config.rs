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
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

/// Which layer a value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Default,
    File(String),
    /// A `SKYWARD_*` variable that a `.env` file supplied.
    ///
    /// Distinguished from [`Origin::Env`] because the two fail differently: a
    /// real environment variable is set by whoever launched the process, while
    /// a `.env` value silently disappears the moment you run from another
    /// directory. Reporting both as `$NAME` would hide that.
    DotEnv(&'static str),
    Env(&'static str),
    Cli,
    /// Expanded from a named `impl_set` preset.
    ///
    /// A stage you did not choose individually still came from *somewhere*,
    /// and "default" would be a lie — change the preset and the value changes
    /// with it. Printing the preset name is what makes `--detect` visibly
    /// different from the stage it replaced.
    Preset(String),
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Default => write!(f, "default"),
            Origin::File(path) => write!(f, "{path}"),
            Origin::DotEnv(name) => write!(f, "${name} (.env)"),
            Origin::Env(name) => write!(f, "${name}"),
            Origin::Cli => write!(f, "command line"),
            Origin::Preset(name) => write!(f, "impl_set '{name}'"),
        }
    }
}

/// What a `.env` file contributed to this process's environment.
#[derive(Debug, Default)]
pub struct DotEnv {
    /// The file that was loaded, if one was found.
    pub path: Option<String>,
    /// Keys the file actually supplied — that is, those the real environment
    /// had not already set.
    pub keys: HashSet<String>,
}

static DOTENV: OnceLock<DotEnv> = OnceLock::new();

/// Decide which of a `.env` file's keys will actually reach the process.
///
/// Split out from the loading so the precedence rule is testable without
/// mutating the environment of a running test binary.
fn keys_supplied_by_file(
    entries: impl Iterator<Item = (String, String)>,
    already_set: impl Fn(&str) -> bool,
) -> HashSet<String> {
    entries
        .map(|(key, _)| key)
        .filter(|key| !already_set(key))
        .collect()
}

/// Load `.env` into the environment, and remember what it contributed.
///
/// # Why this exists
///
/// `.env.example` tells you to `cp .env.example .env` and set your receiver
/// position. Without this, nothing read that file: the server started with no
/// position, refused the range gate, and the only clue was a config dump
/// saying `default` next to a value you had definitely written down.
///
/// A real environment variable always wins over the file, so systemd's
/// `Environment=` on the Pi is never quietly overridden by a stale `.env`
/// left in the working directory.
///
/// # Must be called before any thread starts
///
/// This mutates the process environment, which is not thread-safe on Unix.
/// `main` calls it as its first statement, before the tokio runtime exists.
pub fn load_dotenv() -> &'static DotEnv {
    DOTENV.get_or_init(|| {
        // Read the file first to see which keys the real environment has not
        // already claimed, *then* apply it. `dotenvy::dotenv` does not
        // override existing variables, so the two agree.
        let keys = match dotenvy::dotenv_iter() {
            Ok(iter) => keys_supplied_by_file(iter.flatten(), |key| {
                std::env::var_os(key).is_some()
            }),
            Err(_) => HashSet::new(),
        };

        let path = dotenvy::dotenv()
            .ok()
            .map(|p| p.display().to_string());

        DotEnv { path, keys }
    })
}

/// The `.env` contribution, or an empty record if loading never ran.
fn dotenv() -> &'static DotEnv {
    DOTENV.get_or_init(DotEnv::default)
}

/// Attribute an environment variable to the `.env` file or to the real
/// environment, so the config dump can tell them apart.
fn env_origin(name: &'static str) -> Origin {
    if dotenv().keys.contains(name) {
        Origin::DotEnv(name)
    } else {
        Origin::Env(name)
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

/// The four stage names the file supplied, lifted out before `file` is
/// consumed field by field.
#[derive(Debug, Default)]
struct FileStages {
    magnitude: Option<String>,
    detector: Option<String>,
    slicer: Option<String>,
    validator: Option<String>,
}

/// The four stages, each resolved with its own provenance.
#[derive(Debug, Clone)]
struct StageNames {
    magnitude: Sourced<String>,
    detector: Sourced<String>,
    slicer: Sourced<String>,
    validator: Sourced<String>,
}

impl StageNames {
    /// Expand the preset, then layer file, environment and flags over it.
    ///
    /// An unknown preset expands to the baseline here rather than erroring:
    /// `validate` reports that, and reporting it once with the list of valid
    /// names beats failing twice with two different messages.
    fn resolve(
        impl_set: &Sourced<String>,
        file: &FileStages,
        file_origin: &Origin,
        cli: &CliOverrides,
    ) -> Self {
        let base = adsb_dsp::registry::ImplSet::preset(impl_set)
            .unwrap_or_else(adsb_dsp::registry::ImplSet::baseline);
        let from_preset = Origin::Preset(impl_set.value.clone());

        let layer = |value: String,
                     file_value: &Option<String>,
                     env: &'static str,
                     cli_value: &Option<String>| {
            Sourced::new(value, from_preset.clone())
                .or_layer(file_value.clone(), file_origin.clone())
                .or_layer(env_string(env), env_origin(env))
                .or_layer(cli_value.clone(), Origin::Cli)
        };

        StageNames {
            magnitude: layer(base.magnitude, &file.magnitude, "SKYWARD_MAG", &cli.magnitude),
            detector: layer(base.detector, &file.detector, "SKYWARD_DETECT", &cli.detector),
            slicer: layer(base.slicer, &file.slicer, "SKYWARD_SLICE", &cli.slicer),
            validator: layer(
                base.validator,
                &file.validator,
                "SKYWARD_VALIDATE",
                &cli.validator,
            ),
        }
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
    /// Per-stage overrides, layered on top of whichever preset `impl_set`
    /// names. Set one to swap a single stage without defining a whole preset
    /// for every experiment.
    pub magnitude: Option<String>,
    pub detector: Option<String>,
    pub slicer: Option<String>,
    pub validator: Option<String>,
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
    pub magnitude: Option<String>,
    pub detector: Option<String>,
    pub slicer: Option<String>,
    pub validator: Option<String>,
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
    pub magnitude: Sourced<String>,
    pub detector: Sourced<String>,
    pub slicer: Sourced<String>,
    pub validator: Sourced<String>,
    pub receiver_lat: Sourced<Option<f64>>,
    pub receiver_lon: Sourced<Option<f64>>,
    pub receiver_alt_m: Sourced<f64>,
    /// Where the file layer came from, if any.
    pub config_path: Option<String>,
    /// The `.env` file that contributed to the environment layer, if any.
    pub env_file: Option<String>,
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
        let (mut file, file_origin) = match path {
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

        let file_stage = FileStages {
            magnitude: file.magnitude.take(),
            detector: file.detector.take(),
            slicer: file.slicer.take(),
            validator: file.validator.take(),
        };
        // The preset resolves first: it supplies the base value for each of the
        // four stages, which per-stage overrides then layer on top of. Doing it
        // in one struct literal is not possible -- the stages depend on it.
        let impl_set = Sourced::new("baseline".to_string(), Origin::Default)
            .or_layer(file.impl_set, file_origin.clone())
            .or_layer(
                env_string("SKYWARD_IMPL_SET"),
                env_origin("SKYWARD_IMPL_SET"),
            )
            .or_layer(cli.impl_set.clone(), Origin::Cli);

        let stage = StageNames::resolve(&impl_set, &file_stage, &file_origin, cli);


        let config = Config {
            source: Sourced::new("tcp:127.0.0.1:1234".to_string(), Origin::Default)
                .or_layer(file.source, file_origin.clone())
                .or_layer(env_string("SKYWARD_SOURCE"), env_origin("SKYWARD_SOURCE"))
                .or_layer(cli.source.clone(), Origin::Cli),

            sample_rate_hz: Sourced::new(2_400_000u32, Origin::Default)
                .or_layer(file.sample_rate_hz, file_origin.clone())
                .or_layer(
                    env_parsed("SKYWARD_SAMPLE_RATE_HZ"),
                    env_origin("SKYWARD_SAMPLE_RATE_HZ"),
                )
                .or_layer(cli.sample_rate_hz, Origin::Cli),

            frequency_hz: Sourced::new(1_090_000_000u32, Origin::Default)
                .or_layer(file.frequency_hz, file_origin.clone()),

            gain_db: Sourced::new("49.6".to_string(), Origin::Default)
                .or_layer(file.gain_db, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_GAIN_DB"),
                    env_origin("SKYWARD_GAIN_DB"),
                )
                .or_layer(cli.gain_db.clone(), Origin::Cli),

            bind: Sourced::new("0.0.0.0:8080".to_string(), Origin::Default)
                .or_layer(file.bind, file_origin.clone())
                .or_layer(env_string("SKYWARD_BIND"), env_origin("SKYWARD_BIND"))
                .or_layer(cli.bind.clone(), Origin::Cli),

            db_path: Sourced::new("skyward.db".to_string(), Origin::Default)
                .or_layer(file.db_path, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_DB_PATH"),
                    env_origin("SKYWARD_DB_PATH"),
                )
                .or_layer(cli.db_path.clone(), Origin::Cli),

            cors_origins: Sourced::new(vec!["http://localhost:5173".to_string()], Origin::Default)
                .or_layer(file.cors_origins, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_CORS_ORIGINS")
                        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect()),
                    env_origin("SKYWARD_CORS_ORIGINS"),
                ),

            log_format: Sourced::new("text".to_string(), Origin::Default)
                .or_layer(file.log_format, file_origin.clone())
                .or_layer(
                    env_string("SKYWARD_LOG_FORMAT"),
                    env_origin("SKYWARD_LOG_FORMAT"),
                )
                .or_layer(cli.log_format.clone(), Origin::Cli),

            impl_set: impl_set.clone(),

            magnitude: stage.clone().magnitude,
            detector: stage.clone().detector,
            slicer: stage.clone().slicer,
            validator: stage.validator,

            receiver_lat: Sourced::new(None, Origin::Default)
                .or_layer(file.receiver.lat.map(Some), file_origin.clone())
                .or_layer(
                    env_parsed::<f64>("SKYWARD_RECEIVER_LAT").map(Some),
                    env_origin("SKYWARD_RECEIVER_LAT"),
                ),

            receiver_lon: Sourced::new(None, Origin::Default)
                .or_layer(file.receiver.lon.map(Some), file_origin.clone())
                .or_layer(
                    env_parsed::<f64>("SKYWARD_RECEIVER_LON").map(Some),
                    env_origin("SKYWARD_RECEIVER_LON"),
                ),

            receiver_alt_m: Sourced::new(0.0, Origin::Default)
                .or_layer(file.receiver.altitude_m, file_origin.clone())
                .or_layer(
                    env_parsed("SKYWARD_RECEIVER_ALT_M"),
                    env_origin("SKYWARD_RECEIVER_ALT_M"),
                ),

            config_path: path.map(str::to_string),
            env_file: dotenv().path.clone(),
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

        // Every stage name, not just the preset. A typo in `--detect` has to
        // fail here, before the radio is opened, with the alternatives listed
        // -- "it ran but quietly used something else" is the failure that
        // wastes an evening on a machine you cannot debug interactively.
        adsb_dsp::registry::check(&self.impls())
            .map_err(|e| ConfigError::Invalid(e.to_string()))?;

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

    /// The fully expanded implementation selection.
    ///
    /// Every consumer takes this rather than re-expanding the preset itself,
    /// so a per-stage override cannot be honoured in one place and silently
    /// dropped in another.
    pub fn impls(&self) -> adsb_dsp::registry::ImplSet {
        adsb_dsp::registry::ImplSet {
            magnitude: self.magnitude.value.clone(),
            detector: self.detector.value.clone(),
            slicer: self.slicer.value.clone(),
            validator: self.validator.value.clone(),
        }
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
            "magnitude",
            self.magnitude.value.clone(),
            &self.magnitude.origin,
        );
        row("detector", self.detector.value.clone(), &self.detector.origin);
        row("slicer", self.slicer.value.clone(), &self.slicer.origin);
        row(
            "validator",
            self.validator.value.clone(),
            &self.validator.origin,
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

    /// A real environment variable beats the file, so a stale `.env` in the
    /// working directory cannot quietly override systemd's `Environment=`.
    #[test]
    fn the_real_environment_wins_over_the_dotenv_file() {
        let entries = vec![
            ("SKYWARD_GAIN_DB".to_string(), "44.5".to_string()),
            ("SKYWARD_BIND".to_string(), "0.0.0.0:9999".to_string()),
        ];
        let supplied = keys_supplied_by_file(entries.into_iter(), |key| key == "SKYWARD_GAIN_DB");

        assert!(
            !supplied.contains("SKYWARD_GAIN_DB"),
            "already set in the real environment, so the file does not supply it"
        );
        assert!(supplied.contains("SKYWARD_BIND"));
    }

    #[test]
    fn a_dotenv_value_is_not_reported_as_a_plain_environment_variable() {
        // The distinction is the point: `$NAME` came from the caller, and
        // `$NAME (.env)` disappears if you run from another directory.
        assert_eq!(Origin::Env("SKYWARD_BIND").to_string(), "$SKYWARD_BIND");
        assert_eq!(
            Origin::DotEnv("SKYWARD_BIND").to_string(),
            "$SKYWARD_BIND (.env)"
        );
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
    fn stages_expand_from_the_preset_and_say_so() {
        let c = Config::resolve(None, &CliOverrides::default()).unwrap();
        assert_eq!(*c.detector, "naive");
        assert_eq!(c.detector.origin, Origin::Preset("baseline".into()));
        // "default" would be a lie: change the preset and this changes with it.
        let dump = c.print_resolved();
        assert!(dump.contains("impl_set 'baseline'"), "{dump}");
    }

    /// The reason the flags exist: swap one stage without defining a preset.
    #[test]
    fn a_per_stage_flag_overrides_the_preset() {
        let c = Config::resolve(
            None,
            &CliOverrides {
                detector: Some("naive".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.detector.origin, Origin::Cli);
        // Untouched stages still come from the preset.
        assert_eq!(c.slicer.origin, Origin::Preset("baseline".into()));
        assert_eq!(c.impls().detector, "naive");
    }

    #[test]
    fn a_stage_can_be_set_from_the_file_too() {
        let path = write_temp("stage.toml", "detector = \"naive\"\n");
        let c = Config::resolve(path.as_str().into(), &CliOverrides::default()).unwrap();
        assert_eq!(c.detector.origin, Origin::File(path));
    }

    /// A typo in `--detect` must fail before the radio is opened, listing what
    /// is actually available -- not fall back to the preset's choice.
    #[test]
    fn an_unknown_stage_name_is_refused_and_lists_the_alternatives() {
        let err = Config::resolve(
            None,
            &CliOverrides {
                detector: Some("corelator".into()),
                ..Default::default()
            },
        )
        .expect_err("a misspelled detector must not be accepted");
        let message = err.to_string();
        assert!(message.contains("corelator"), "echo the typo: {message}");
        assert!(message.contains("naive"), "list alternatives: {message}");
        assert!(message.contains("detector"), "name the stage: {message}");
    }

    #[test]
    fn impls_matches_the_individually_resolved_stages() {
        let c = Config::resolve(None, &CliOverrides::default()).unwrap();
        let set = c.impls();
        assert_eq!(set.magnitude, *c.magnitude);
        assert_eq!(set.detector, *c.detector);
        assert_eq!(set.slicer, *c.slicer);
        assert_eq!(set.validator, *c.validator);
        // And it must be something the registry will actually build.
        assert!(adsb_dsp::registry::check(&set).is_ok());
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
