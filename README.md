# skyward

An ADS-B receiver in Rust. Raw radio samples in, aircraft out, over HTTP.

Runs headless on a Raspberry Pi. Developed against captured IQ files, so most
of the work needs no hardware attached.

```
cargo run --release -p adsb-server -- decode 8D4840D6202CC371C32CE0576098
cargo run --release -p adsb-server -- doctor --offline
cargo run --release -p adsb-server -- bench --compare runs/baseline.json
cargo run --release -p adsb-server -- run --source file:fixtures/raw/golden.cu8
```

## What this is for

This is a learning project. The interesting radio code is meant to be
**rewritten by hand**, repeatedly, with a scoreboard saying whether each
attempt was better. Every DSP stage ships with a deliberately naive baseline
and a registry of alternatives, so a new implementation lands *beside* the old
one instead of replacing it.

The naive baseline is not a strawman — it is simple and explainable, and it
already recovers 517 messages from the golden fixture. It is also leaving
about three quarters of the recoverable traffic on the table, which is the
point.

## How it fits together

```
                                  ┌─ adsb-core ──────────────────────┐
                                  │ bits, CRC-24, frame, decode, CPR │
                                  │ pure logic, no I/O, 40 tests     │
                                  └──────────────┬───────────────────┘
                                                 │
  adsb-source          adsb-dsp                  │        adsb-track
  ┌──────────┐   ┌──────────────────────┐        │   ┌──────────────────┐
  │ file:    │   │ Magnitude            │◄───────┘   │ aircraft state   │
  │ tcp:     ├──►│ PreambleDetector     ├───────────►│ CPR pairing      │
  │ usb:     │   │ BitSlicer            │            │ plausibility     │
  │          │   │ FrameValidator       │            │ Snapshot         │
  │ + faults │   │ ── Pipeline (fixed) ─┘            └────────┬─────────┘
  └──────────┘   └──────────────────────┘                     │
       u8 IQ            u16 mag → bits → bytes                │
                                                    ┌─────────┴─────────┐
                                                    │                   │
                                              ArcSwap snapshot    bounded queue
                                                    │                   │
                                              ┌─────┴──────┐    ┌───────┴──────┐
                                              │ axum /api  │    │ adsb-store   │
                                              │ SSE stream │    │ batched SQL  │
                                              └────────────┘    └──────────────┘
```

One binary, `skyward`. Not the decoder/API pair the previous implementation
used: two processes means two ways to deploy the wrong version, and a health
endpoint that says `ok` while the decoder has been dead for hours.

## Crates

| crate | what it holds | tests |
|---|---|---|
| `adsb-core` | Bits, CRC-24, frame validation, field decode, CPR (encode *and* decode) | 40 |
| `adsb-dsp` | The four swappable stages, the pipeline, the synthetic generator | 45 |
| `adsb-source` | `file:` / `tcp:` / `usb:`, plus a fault-injecting decorator | 30 |
| `adsb-track` | Aircraft state, CPR pairing, plausibility gates, snapshots | 27 |
| `adsb-store` | Batched SQLite with retention and backpressure | 17 |
| `adsb-server` | CLI, config, doctor, bench, HTTP API | 21 |

## The scoreboard

```
$ skyward bench --compare runs/baseline.json

fixtures/raw/golden.cu8
  180.0 s at 2.400 MS/s
  messages     517   unique     517   aircraft    7   positions    72
  candidates   879   yield   58.8%   cand/msg   1.7   172.3 msg/min
  guards: ghosts 0.000   realtime 361.5x   1.2 ns/sample
```

**The headline is unique CRC-valid messages.** A CRC-clean DF17 is essentially
certainly real (false accepts run about 6 × 10⁻⁸), so ground truth is free and
more is unambiguously better.

**Yield is reported and never optimized.** A better detector proposes more
marginal candidates, so yield *falls* as messages rise — measured on this
fixture, going from 607 to 2,403 messages took yield from 69% down to 5%.
Optimizing yield tunes the detector backwards.

Two guards can veto an apparent win: `ghost_icao_ratio` catches error
correction inventing aircraft, and `realtime_factor` catches an implementation
that finds more but cannot keep up on a Pi.

## Fixtures

Captures live in `fixtures/raw/`. The `.cu8` files are gitignored — 1.3 GB of
IQ does not belong in git — but every capture has a **committed `.toml`
sidecar** recording sample rate, gain, antenna, position and time. An
unlabelled IQ file with an assumed sample rate is a silent multi-evening bug.

| fixture | length | baseline | headroom |
|---|---|---|---|
| `golden.cu8` | 180 s, 7 aircraft | 517 messages | 2,403 with a better detector |
| `desk.cu8` | 120 s, 3 aircraft | 167 messages | the hard tier — weak, obstructed |

## Getting started

```bash
cp .env.example .env          # set your receiver position; 3 decimals is plenty
cargo test --workspace        # ~1 second, no hardware
skyward doctor --offline      # proves the decode chain works on this CPU
skyward bench                 # score the fixtures
```

To develop the client with no radio:

```bash
skyward run --source file:fixtures/raw/golden.cu8 --api-only
curl localhost:8080/api/v1/aircraft | jq
```

## Antenna, because it dominates everything

Measured on identical hardware and settings, one evening:

| placement | msg/min |
|---|---|
| dipole on a tripod, top-floor window, 40 cm from the glass | **261** |
| the same tripod on a desk in an interior room | 95 |
| **suction-cupped to that same window** | 11 |
| window facing a condo tower | 0.5 |

Two things to remember. **Never mount the dipole against glass** — at 1090 MHz
the antenna's near field is only ~4.4 cm deep, so a coated pane detunes and
loads it; the noise floor is unchanged while peak signal drops 7×. And **FM at
98.5 MHz is a useful control**: if both bands drop, suspect the cable; if FM
rises while 1090 falls, it is geometry.

Half-wave dipole at 1090 MHz is 13.8 cm tip to tip, vertical.

## Known gaps

- **USB source.** Not implemented. `rtl_tcp` is the recommended deployment —
  it uses libusb async transfers, so it does not drop samples between reads,
  and it keeps this binary free of C dependencies.
- **Gillham altitude** (Q=0, above 50,175 ft) reports `Unavailable`.
- **Local CPR and surface positions** are designed for but not implemented.
- **`skyward bundle`** is specified in the plan but not built yet.
