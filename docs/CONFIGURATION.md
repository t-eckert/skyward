# Configuration

Every setting, where it can be set, and what happens if you get it wrong.

## Precedence

Lowest to highest:

```
defaults  <  skyward.toml  <  .env  <  environment  <  CLI flags  <  station overlay
```

Two of those need a word.

**`.env` sits below the real environment, not beside it.** `dotenvy` never
overrides a variable that is already set, so `Environment=` in a systemd unit
always beats a stale `.env` left in a working directory. The two are reported
differently — `$SKYWARD_GAIN_DB (.env)` versus `$SKYWARD_GAIN_DB` — because
they fail differently: a real environment variable was set by whoever launched
the process, while a `.env` value silently disappears the moment you run from
another directory.

**The station overlay outranks everything.** It is the file a position set
through the API is written to, and it has to win or a position set from the web
interface would silently revert at the next restart. See
[The station overlay](#the-station-overlay) for the hazard this creates and
what makes it visible.

## Provenance is the feature

```console
$ skyward config
skyward configuration
  (.env: /home/pi/skyward/.env)

  source                 usb                          $SKYWARD_SOURCE
  sample_rate_hz         2400000                      default
  gain_db                44.5                         command line
  receiver.lat           45.421                       skyward-station.toml
```

On a machine you cannot log into, the question is never "what is the config" —
it is "did my edit take effect". A dump that prints values but not their origin
cannot answer that. `skyward doctor` prints the same table inside its report.

## Two things that fail rather than default

**An unknown key.** `deny_unknown_fields` is set on the file format, so a
typo'd `recevier.lat` is a startup error rather than a silently ignored line.
The classic blind-box failure is editing a key, restarting, and getting no
signal at all that anything was wrong.

**A half-set position.** Latitude and longitude must arrive together or not at
all. One without the other is a startup error.

Note what is *not* in that list: an entirely unset position is fine. The server
starts, logs a warning, and runs with the range gate and local CPR disabled.
Defaulting to `0, 0` would be the unforgivable option — that is in the Gulf of
Guinea, and the range gate would reject every aircraft on earth while looking
like poor reception.

---

## The file

TOML, passed with `--config PATH`. A path that is named but does not exist is
an error; no path at all is fine.

```toml
schema = 1                        # optional; mismatches are a startup error

source = "usb"
sample_rate_hz = 2_400_000
frequency_hz = 1_090_000_000
gain_db = "49.6"

bind = "0.0.0.0:8080"
db_path = "/var/lib/skyward/skyward.db"
retention_hours = 24
cors_origins = ["http://localhost:5173"]
log_format = "text"

station_file = "/var/lib/skyward/skyward-station.toml"
station_writable = true

impl_set = "baseline"
# Per-stage overrides, layered on top of the preset.
# magnitude = "naive"
# detector  = "naive"
# slicer    = "naive"
# validator = "crc-only"

[receiver]
lat = 45.421
lon = -75.697
altitude_m = 70
```

---

## Every key

### Source

| Key | Env | Flag | Default |
|---|---|---|---|
| `source` | `SKYWARD_SOURCE` | `--source` | `tcp:127.0.0.1:1234` |

One of three forms:

- **`file:PATH`** — replay a `.cu8` capture. Paced to wall-clock time unless
  `--fast`; `--loop-file` restarts it at the end.
- **`tcp:HOST:PORT`** — an `rtl_tcp` server. Split on the *last* colon, so IPv6
  literals survive: `tcp:::1:1234` is host `::1`, port 1234.
- **`usb`** or **`usb:INDEX`** — the dongle directly. Needs a binary built with
  `--features usb`; without it this is a startup error naming both the flag and
  the `rtl_tcp` alternative. `skyward devices` lists the indices.

  Bus enumeration order is not stable across reboots, so `usb:0` is not
  reliably the same stick twice on a station with two dongles.

| Key | Env | Flag | Default |
|---|---|---|---|
| `sample_rate_hz` | `SKYWARD_SAMPLE_RATE_HZ` | `--sample-rate-hz` | `2400000` |

Below 2 MS/s is a startup error: a Mode S bit is one microsecond long, so under
two samples per microsecond the two halves of a bit cannot be told apart.

2.4 MS/s is exact on this hardware — the RTL2832U derives its rate from a
28.8 MHz clock through a fractional divider, and 28.8/12 lands precisely.
2.048 MS/s does not. On USB, skyward reports the rate the hardware says it
achieved rather than the one requested, because feeding the requested rate to
the demodulator while the hardware runs at another produces plenty of preambles
and zero valid CRCs.

| Key | Env | Flag | Default |
|---|---|---|---|
| `frequency_hz` | — | — | `1090000000` |
| `gain_db` | `SKYWARD_GAIN_DB` | `--gain-db` | `"49.6"` |

Gain is a decibel figure as a string, or `"auto"`. It must be one of the
tuner's discrete steps — the R820T has 29 — and an unlisted value is a hard
error that prints the real table, because silently getting a different gain
than you asked for invalidates any sweep.

`"0"` means 0 dB, not automatic. (`rtl_sdr -g 0` means automatic, which is a
genuinely confusing convention and the reason there is a test pinning ours.)

Maximum gain is often *not* optimal: it lifts the noise floor equally and can
drive the tuner into compression on a nearby aircraft. `doctor` reports the
fraction of samples at the rails and suggests a figure when it is too high.

### Receiver position

| Key | Env | Default |
|---|---|---|
| `receiver.lat` | `SKYWARD_RECEIVER_LAT` | unset |
| `receiver.lon` | `SKYWARD_RECEIVER_LON` | unset |
| `receiver.altitude_m` | `SKYWARD_RECEIVER_ALT_M` | `0` |

Used for the 400 km range plausibility gate, for local CPR resolution, and as
the centre of the map's range rings. Altitude is metres above sea level and
only feeds `doctor`'s radio-horizon estimate; a rough figure is fine.

**Three decimals is plenty.** Local CPR only needs the reference in the same
~670 km zone as the aircraft, and the range gate works at 400 km. More
precision buys nothing and publishing an exact home address costs something.
(Centimetre accuracy would matter only for MLAT, which this project does not
do.)

It can also be set at runtime — see below.

| Key | Env | Default |
|---|---|---|
| `station_file` | `SKYWARD_STATION_FILE` | `skyward-station.toml` |
| `station_writable` | `SKYWARD_STATION_WRITABLE` | `true` |

`station_writable = false` pins the position to configuration; `PUT` and
`DELETE` on `/api/v1/receiver` then return `403`, and the dialog in the web
interface renders disabled with the reason shown.

### Server

| Key | Env | Flag | Default |
|---|---|---|---|
| `bind` | `SKYWARD_BIND` | `--bind` | `0.0.0.0:8080` |
| `cors_origins` | `SKYWARD_CORS_ORIGINS` | — | `["http://localhost:5173"]` |

An allowlist, not a wildcard — `*` is accepted and is not the default.

CORS constrains browsers, not `curl`. It is not an authentication mechanism and
skyward has none; see [Security](#security).

### Storage

| Key | Env | Flag | Default |
|---|---|---|---|
| `db_path` | `SKYWARD_DB_PATH` | `--db-path` | `skyward.db` |
| `retention_hours` | `SKYWARD_RETENTION_HOURS` | — | `24` |

`0` keeps everything, which will eventually fill the disk. Anything above 8760
(a year) is a startup error, on the grounds that it is far more often a units
mistake than an intention.

### Logging

| Key | Env | Flag | Default |
|---|---|---|---|
| `log_format` | `SKYWARD_LOG_FORMAT` | `--log-format` | `text` |

`text` for a terminal, `json` for journald and machine parsing. Anything else
is a startup error.

Level comes from `RUST_LOG` in the usual `tracing` syntax, defaulting to
`info`. `RUST_LOG=debug,hyper=info` is a good first escalation.

### Pipeline

| Key | Env | Flag | Default |
|---|---|---|---|
| `impl_set` | `SKYWARD_IMPL_SET` | `--impl-set` | `baseline` |
| `magnitude` | `SKYWARD_MAG` | `--mag` | from the preset |
| `detector` | `SKYWARD_DETECT` | `--detect` | from the preset |
| `slicer` | `SKYWARD_SLICE` | `--slice` | from the preset |
| `validator` | `SKYWARD_VALIDATE` | `--validate` | from the preset |

`impl_set` names a preset; the four per-stage keys override individual stages
on top of it. That layering exists because the comparison you usually want is
against your own previous attempt — `--detect correlator-v3` against
`--detect correlator-v2` — which should not require defining a preset for every
experiment.

An unknown name at any of the five is a startup error listing what exists,
checked before the radio is opened. `skyward list-impls` prints the registry.

Provenance reports a stage taken from the preset as `impl_set 'baseline'`
rather than `default`, because "default" would be a lie: change the preset and
the value changes with it.

---

## The station overlay

A position set through `PUT /api/v1/receiver` is written to `station_file` as a
small TOML file shaped like the config file's `[receiver]` table:

```toml
[receiver]
lat = 45.421
lon = -75.697
altitude_m = 70
```

It is written through a temporary file and a rename, because the alternative is
a power cut halfway through a 90-byte write leaving a truncated file that fails
to parse at the next boot — on a Pi, where sudden power loss is the normal way
the machine turns off. A corrupt overlay is warned about and ignored rather
than fatal; the configured position is still right there.

**It outranks the config file and the environment.** That is the hazard: edit
`/etc/skyward.env`, restart, and nothing changes. Three things make that
visible rather than mysterious:

- `skyward config` and `doctor` report the origin as the overlay's path.
- `doctor` has a dedicated `station.shadowed` check that fires only when the
  overlay holds a *different* position from the one configuration asks for, and
  names both.
- `run` logs it at startup.

To get rid of it:

```bash
curl -X DELETE http://localhost:8080/api/v1/receiver   # reverts and deletes the file
# or
rm /var/lib/skyward/skyward-station.toml               # takes effect at the next restart
```

---

## Build-time settings

These are not runtime configuration; they change what the binary can do.

| Cargo feature | Effect |
|---|---|
| `usb` (off by default) | Links librtlsdr and enables `--source usb` and `skyward devices` |

Off by default so the ordinary build has no native dependency of its own. Note
that the build is not C-free either way — `rusqlite`'s `bundled` feature
compiles SQLite — so a cross-build needs a C toolchain regardless.

| Build variable | Effect |
|---|---|
| `RTLSDR_LIB_DIR` | Where to find librtlsdr, overriding the search |
| `RTLSDR_STATIC` | `1` or `0` to force static or dynamic linking |

The defaults: static on macOS when `librtlsdr.a` is present, dynamic
everywhere else. Homebrew's dylib has an install name of
`@rpath/librtlsdr.0.dylib`, so a dynamically linked binary fails at startup
with "Library not loaded" unless the caller exports `DYLD_LIBRARY_PATH` —
which is a poor thing to hand someone whose goal is to look at aeroplanes.
Linking the static archive Homebrew ships beside it avoids the problem
entirely.

---

## Security

skyward has no authentication of any kind. Everything on the API — the aircraft
list, the statistics, the station position — is readable by anyone who can
reach the port, and with `station_writable` at its default the position is
writable by them too.

That is a reasonable default for a home LAN appliance whose whole purpose is to
be opened from a phone, and the blast radius of the one write endpoint is a map
pin. It is not appropriate for anything reachable from outside a trusted
network. Put it behind a reverse proxy with authentication, or bind to
localhost and reach it over SSH. Nothing in skyward provides that, and nothing
in skyward pretends to.

The receiver position is a home address. Keep it out of any repository — `.env`
is gitignored for that reason — and note that it is served to anyone who can
reach `/api/v1/receiver`.
