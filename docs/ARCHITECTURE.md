# Architecture

How the pieces fit, and why they are shaped the way they are. For the theory underneath — what a Mode S frame is, why CPR needs two parities, how the CRC works — read [the study guide](GUIDE.md). For the code itself, its Part IV is a file-by-file walkthrough.

## The shape of it

```
   RF ────► RTL-SDR ────► adsb-source ────► adsb-dsp ────► adsb-core
  1090 MHz    dongle       IQ bytes          frames        decoded
                                                           messages
                                                              │
                                                              ▼
                                                         adsb-track
                                                      aircraft + fixes
                                                          │      │
                                             ┌────────────┘      └──────────┐
                                             ▼                             ▼
                                     ArcSwap<Snapshot>              adsb-store
                                             │                     SQLite history
                                             ▼                             │
                                       adsb-server ◄──────────────────────┘
                                     axum + embedded client
```

Six crates, and the dependency direction never reverses.

| Crate | Holds | Depends on |
|---|---|---|
| `adsb-core` | Mode S decoding. Pure logic, no I/O | — |
| `adsb-dsp` | IQ samples in, validated frames out | core |
| `adsb-source` | Where samples come from | — |
| `adsb-track` | Messages into aircraft | core |
| `adsb-store` | Durable history | core, track |
| `adsb-server` | The `skyward` binary | all of them |

`adsb-core` is almost dependency-free on purpose: it must compile and test in under a second so the tests actually get run, and it must contain nothing that can behave differently on the Pi than on the Mac. `adsb-dsp` depends only on core for the same reason — `cargo test -p adsb-dsp` is a sub-second loop, which is the difference between a DSP experiment you run and one you think about.

`adsb-source` deliberately does *not* depend on `adsb-dsp`. It is used from dsp's examples, and the one-way edge keeps the DSP crate testable with no I/O at all.

## Inside the running process

```
  [decoder thread]  source ──► pipeline ──► tracker ──► ArcSwap<Snapshot>
                                              │                  ▲
                                              ▼                  │
                                       bounded queue         [axum tasks]
                                              │
                                       [writer thread] ──► SQLite
```

Three threads that matter, and the boundaries between them are the design.

**The decoder is a plain OS thread, not a tokio task.** It is a tight CPU loop that would otherwise monopolise an async worker.

**The API never touches the decoder's data structures.** The decoder publishes an immutable `Snapshot` into an `ArcSwap` twice a second; handlers load a pointer. No locks, no database, sub-microsecond. An earlier design took an async mutex around a blocking rusqlite connection for *every* request, which serialized them all and blocked the runtime — to serve live aircraft that were already in memory.

**The decoder must not block on storage.** The write queue is bounded, and full means drop-and-count. A dropped row costs one dot on a map; a stalled decoder loses samples permanently, and there is no getting them back.

**History goes through `spawn_blocking`.** rusqlite is blocking, so a slow SD card parks a blocking thread rather than a tokio worker.

## Two failures the structure is built around

Everything above has a shape that only makes sense once you have watched these two happen.

### A receiver that died quietly

The worst outcome is not a crash. It is a receiver that has not heard anything in six hours while every indicator says it is fine. Several decisions exist only to prevent it:

- **Errors are a taxonomy, not a bag.** `SourceError::Config` is deliberate and unrecoverable — fail loudly at startup, never retry, because retrying a configuration error just hides it. `SourceError::Transient` means reconnect, count it, and carry on. `EndOfStream` is normal.
- **The decoder never exits on a transient error**, and — since it retried the same dead socket forever otherwise — it asks the source to `reconnect()`, which re-establishes the link *and replays every setting*. Reconnecting without the replay leaves a dongle on its power-on defaults: wrong frequency, automatic gain, and a receiver that is technically streaming and hears nothing.
- **Health is freshness, not liveness.** `/healthz` returns 503 once samples have stopped, so `curl -f` is a real probe.
- **The client treats silence as failure.** A dead SSE stream does not necessarily raise an error; the socket can stay open with `onerror` never firing. Only an arriving snapshot proves the stream is alive.

### A number that lies

Several metrics exist because the obvious one is ambiguous:

- **Dropping samples and hearing nothing are identical in a message count.** The effective sample rate — samples consumed over elapsed wall time — separates them, and one of the two is a software problem you can fix.
- **A buffering source defeats that.** The USB ring keeps `read` returning a full 2.4 MS/s while discarding blocks behind it, so the effective rate stays perfect through a real loss. `source_overrun_bytes` is the only number that shows it.
- **Yield falls as a detector gets better.** A better detector proposes more marginal candidates. Yield is reported and must never be optimised for; tuning for it tunes the detector backwards.
- **A lifetime average tells you nothing about now.** The client computes messages-per-minute over a rolling window rather than `crc_ok / uptime`, because a receiver up for six days averages away the fact that it stopped working this afternoon.

## The five stages, and how they are swapped

Decoding is four stages in `adsb-dsp` plus one in `adsb-track`:

| Stage | Flag | Where |
|---|---|---|
| Magnitude | `--mag` | `adsb-dsp/src/magnitude.rs` |
| Preamble detection | `--detect` | `adsb-dsp/src/detect.rs` |
| Bit slicing | `--slice` | `adsb-dsp/src/slice.rs` |
| Validation | `--validate` | `adsb-dsp/src/validate.rs` |
| Position solving | — | `adsb-track/src/position.rs` |

Each is a trait with named implementations in a registry, selected by string at startup. Position solving lives in `adsb-track` rather than `adsb-dsp` because it is the only stage that needs per-aircraft state: a global fix requires an even *and* an odd frame from the same aircraft, close together in time.

Names rather than types, because the comparison you actually want is against your own previous attempt. `--detect correlator-v3` against `--detect correlator-v2` should be a flag, not a branch — and it should show up in `doctor` and in `/api/v1/stats`, so a run is self-describing. A preset is expanded once in `Config::impls()` and every consumer takes the expansion, so a per-stage override cannot be honoured in one place and silently dropped in another.

An unknown name is a startup error listing the alternatives, checked before the radio is opened. "It ran but quietly used something else" is the failure that wastes an evening.

## Where samples come from

One trait, three implementations, and a decorator that breaks things on purpose:

| Spec | Implementation | For |
|---|---|---|
| `file:PATH` | `FileSource` | Development against a capture; benchmarks |
| `tcp:HOST:PORT` | `TcpSource` | `rtl_tcp`, including on another machine |
| `usb[:INDEX]` | `UsbSource` (feature `usb`) | The dongle directly |
| — | `MisbehavingSource` | Injecting faults deliberately |

The abstraction earns its place by letting the same binary develop against a file on a laptop, demo from `rtl_tcp`, and deploy against a dongle — switched by one string.

`read` hands back interleaved unsigned 8-bit I,Q — exactly what the RTL2832U produces. Converting to `Complex<f32>` at the source would turn 2 bytes into 8 before anything looked at them, double memory bandwidth on a Pi, and foreclose the magnitude lookup table entirely.

The returned count is **always even**. Getting that wrong swaps I and Q for the entire remainder of the stream, which decodes as pure noise and looks exactly like a broken demodulator. It is the reason the USB ring discards whole blocks rather than trimming bytes.

### The USB source, specifically

Everything hard about an RTL-SDR is below the FFI line: the RTL2832U register map, the R820T PLL and its VCO band search, the tuner's I2C gate. skyward binds to librtlsdr — the same C library `rtl_tcp` uses — rather than reimplementing it. A subtle error in the VCO search produces a radio that tunes to almost the right frequency and decodes nothing, which is indistinguishable from a bad antenna.

The device is driven exactly the way `rtl_tcp` drives it: a dedicated thread parked in `rtlsdr_read_async`, handing each completed transfer to a callback that appends it to a bounded ring. `rtlsdr_read_sync` would drop whatever arrives between calls, silently, and the loss would read as "my detector is bad".

An overrun drops the **oldest** block and counts it. Dropping the newest is simpler and lets the buffer stay permanently full, so every sample the decoder sees is seconds stale while the counters insist everything is fine.

## The client

SvelteKit, built to static assets, **compiled into the binary** with `rust-embed`.

For the same reason there is one binary rather than a decoder and an API: two artifacts means two ways to deploy the wrong version. A `--web-root` pointing at a directory would let a six-week-old client sit in front of a freshly deployed server, showing fields the API no longer sends and silently omitting the ones it does — with nothing anywhere saying the two disagree.

It also means deployment is `scp skyward pi:` and nothing else.

The API router's fallback serves the client, which is what makes a deep link work — `/aircraft/A0A41F` has no file behind it. A reserved-prefix list (`api/`, `healthz`, `readyz`) stops that fallback from answering a misspelled endpoint with a page of HTML.

`build.rs` writes a placeholder page when `client/build` is missing, so a fresh clone still compiles and the browser says which command was skipped.

## Configuration

Layered, and every value carries where it came from. On a machine you cannot log into, the question is never "what is the config" but "did my edit take effect", and a dump without provenance cannot answer it. See [CONFIGURATION.md](CONFIGURATION.md).

The one value that moves at runtime is the station position, which lives in an `ArcSwap` beside a generation counter. The decoder checks the counter once per publish tick — a single relaxed atomic load — and adopts a change by swapping the tracker's gates rather than rebuilding it, so no aircraft's accumulated CPR state is lost.

## What is deliberately not here

- **MLAT.** It needs several receivers and sub-microsecond timestamps.
- **Uplink of any kind.** This is a receiver.
- **Authentication.** See [Security](CONFIGURATION.md#security).
- **A second position solver.** `GlobalCprSolver` requires both parities, so an aircraft appears on its second position message rather than its first. Local CPR and surface positions are the obvious next implementations, and the registry exists so they land beside it rather than replacing it.
