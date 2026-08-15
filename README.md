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

`.env` is loaded from the working directory at startup. A real environment
variable always wins over it, so systemd's `Environment=` on the Pi is never
overridden by a stale file — and `skyward config` marks which is which:

```
gain_db             44.5      $SKYWARD_GAIN_DB           # from the caller
receiver.lat        41.788    $SKYWARD_RECEIVER_LAT (.env)
sample_rate_hz      2400000   default
```

To develop the client with no radio:

```bash
skyward run --source file:fixtures/raw/golden.cu8 --api-only
curl localhost:8080/api/v1/aircraft | jq
```

## The client

A SvelteKit app in `client/`, built against the design in Paper. Aircraft are
drawn over an [OpenFreeMap](https://openfreemap.org) vector basemap with
MapLibre — no API key, no registration, no request limit, and OSM attribution
carried in the source TileJSON so MapLibre renders it unprompted.

**The range rings stayed.** Geography answers "where is that aircraft"; the
rings answer "how far out am I hearing, and in which direction", which is a
question about the antenna and the only one this project is really about. They
are drawn as true geodesic circles about the station, dashed so they read as
annotation rather than as another county line.

This does mean the map now needs the public internet, which the earlier
ring-only plot did not. If that becomes the wrong trade, the way back is
[PMTiles](https://docs.protomaps.com/pmtiles/): a measured `z0–10` extract of
the whole 400 km coverage circle is **67 MB** as a single file, and
`tower-http` already answers range requests, so the existing binary could serve
it with no tile server and no new crate. Fonts are bundled for that same
reason — a client that must reach the network to render its own labels fails
exactly when you are debugging the radio.

```bash
cd client
npm install
npm run dev        # localhost:5173, proxies /api to the receiver
```

The dev server proxies to `127.0.0.1:8080`, so the browser sees a single origin
and CORS never enters into it.

### The client is compiled into the binary

`skyward run` serves it at `/`, so there is nothing to deploy beside it:

```bash
cd client && npm run build     # emits client/build
cargo build --release -p adsb-server
scp target/release/skyward pi:  # that is the whole deployment
```

For the same reason there is one binary rather than a decoder and an API: two
artifacts means two ways to deploy the wrong version. A `--web-root` pointing
at a directory on the Pi would let a six-week-old client sit in front of a
freshly deployed server, reading fields the API no longer sends, with nothing
anywhere saying the two disagree. Embedding makes that impossible — `skyward
--version` now describes the interface as well as the decoder, and `skyward
doctor` reports what is actually inside:

```
[ok  ] web.client    21 files, 1642 KiB embedded
```

A checkout that has never run `npm run build` still compiles. The build script
writes a placeholder page saying which command is missing, so `cargo build`
fails for real reasons only, and `doctor` warns rather than the problem
surfacing as a blank browser tab.

Unmatched paths fall through to the app, which is what makes a reload of
`/aircraft/A0A41F` work. `/api/*`, `/healthz` and `/readyz` are excluded from
that fallback — without the exclusion a typo'd endpoint answered `200` with a
page of HTML, so `fetch` resolved happily and then died parsing `<!doctype`
as JSON. Hashed assets under `_app/immutable/` are cached for a year;
`index.html` never is, because it is what points at the current hashed names.

### Two themes, both descended from aviation practice

Switched in the header, remembered, and also selectable with `?theme=`.

- **Flight deck** (default) uses the cockpit colour convention literally:
  white is current status, cyan is background and reference, magenta is the
  target, amber is caution, green is normal. Instrument type is read at arm's
  length, so numerals are large against very small labels.
- **Chart** is the VFR sectional this screen replaces, on a neutral paper
  ground rather than sectional buff — the buff is a printing constraint and
  carrying it over tints every value on the screen. A sectional spends exactly
  two signal colours, so magenta carries the aircraft and blue stays
  structural.

Both are driven entirely by tokens in `app.css`, type scale and row heights
included. A theme that can only change hue cannot change how a screen feels.

Inter throughout, including the telemetry: columns line up because the figures
are `tabular-nums`, which is what kept them aligned before — a monospace
family was one way to get that, not the only one. The slashed zero is on,
because `0` and `O` sit together constantly in addresses like `A0A41F`. Only
the Latin subset ships, which is 48 kB rather than 224 kB.

### Two failures that do not announce themselves

**A dead stream does not necessarily raise an error.** The client declares the
connection dead after six seconds with no snapshot, because when the receiver
was stopped behind the dev proxy the socket stayed open, `onerror` never
fired, and the view showed `STREAMING` over a sixteen-second-old snapshot
reporting `heard 0.1 s` — the ages looked live because they are computed
against the server clock in the envelope, which had stopped advancing too.
`client/scripts/outage.mjs` stops and restarts the real receiver to prove the
watchdog still catches it.

**A live stream only proves the server is alive.** With the antenna unplugged,
snapshots kept arriving exactly on schedule — they were simply empty — so the
stream looked perfectly healthy. The server knew (`status: stalled`); nothing
asked it. The health endpoint now drives a third state, `NO SIGNAL`, which
says the problem is at the other end of the cable.

**And a map that renders nothing raises no error either.** MapLibre spawns its
tile-parsing worker from a URL relative to its own module, which neither Vite
mode resolves: in dev the pre-bundled copy 404s, and in the build the worker
chunk is never emitted. Both fail the same silent way — the style never
finishes loading, so no glyphs and no vector tiles are ever requested, and you
get an empty basemap with our overlay layers present and correct and nothing in
the console. `worker: { format: 'es' }` plus an explicit `setWorkerUrl` from a
`?worker&url` import fixes both. `client/scripts/mapdebug.mjs` prints the
network log and the map's own load state, which is where the signal actually
was. Dev passing says nothing about the build here — check both.

## Antenna, because it dominates everything

Measured on identical hardware and settings, one evening:

| placement | msg/min |
|---|---|
| dipole on a tripod, top-floor window, 40 cm from the glass | **261** |
| the same tripod on a desk in an interior room | 95 |
| **suction-cupped to that same window** | 11 |
| window facing a condo tower | 0.5 |

**Outdoors beats all of it.** Sitting on the front porch in Troy, PA —
2026-08-15, 38 minutes, same dipole, 49.6 dB, `baseline` impl set:

| placement | msg/min | max range |
|---|---|---|
| **front porch, outdoors, nothing overhead** | **430** | 92.3 nm |

Not directly comparable to the table above — a different site on a different
day, and 38 minutes against a full evening — so read it as a magnitude, not a
fourth row. The magnitude is the point: 1.6× the best indoor placement and 4.5×
the interior room, from moving the same antenna through a doorway. No amount of
work on the detector buys back what a wall costs.

Note what is *not* the limit at 92 nm. The radio horizon for an aircraft at
31,000 ft seen from 340 m is roughly 258 nm, so the ceiling here is terrain —
Troy sits in hilly country — and the baseline detector, which is already known
to be leaving about three quarters of the recoverable traffic on the table.
A better detector outdoors is the interesting experiment.

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
- **The ghost guard is bench-only.** `ghost_icao_ratio` is computed in
  `bench.rs` and not on the live path, so `/api/v1/stats` cannot report it and
  the client's scoreboard shows `—`. A zero there would read as "measured and
  clean", which is the wrong impression to give.
- **`bench` scores positions against the operator's current receiver
  position**, not the capture's. Moving the station from Ottawa to Troy took
  `golden` from 72 positions to 8 with an identical digest and an identical
  message count — the 400 km range gate correctly rejecting traffic 500 km
  away. Correct behaviour, wrong input: the fixture sidecars record
  `[receiver] unset = true`, so there is no capture-time position to use
  instead. The headline metric is unaffected.
