# Working on skyward

## Layout

```
crates/
  adsb-core/     Mode S decoding. Pure logic, no I/O
  adsb-dsp/      IQ samples in, validated frames out — the five stages
  adsb-source/   Where samples come from: file, rtl_tcp, USB
  adsb-track/    Messages into aircraft, and position solving
  adsb-store/    Batched SQLite writes with retention
  adsb-server/   The `skyward` binary, the API, the embedded client
client/          SvelteKit interface, compiled into the binary
fixtures/raw/    Real captures with sidecar metadata
deploy/          systemd units, udev rules, install script
docs/            This
reference/       Exploratory code, kept for reading, not built
runs/            Benchmark records
```

[ARCHITECTURE.md](ARCHITECTURE.md) explains why the boundaries are where they
are. [Part IV of the study guide](GUIDE.md) walks the files.

## The loop

```bash
cargo test -p adsb-core     # under a second, deliberately
cargo test -p adsb-dsp      # also under a second
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt
```

`adsb-core` and `adsb-dsp` are kept nearly dependency-free so those two stay
sub-second. A DSP experiment you can run in a second is one you actually run.

The toolchain is pinned in `rust-toolchain.toml`. The Pi is operated blind, so
"works on the Mac, fails on the Pi" must never be a toolchain difference
diagnosed remotely.

Build the USB source too — it is off by default and therefore easy to break
without noticing:

```bash
cargo test -p adsb-source --features usb
cargo build --features adsb-server/usb
```

### Running against a fixture

```bash
skyward run --source file:fixtures/raw/golden.cu8 --loop-file --api-only
```

`--api-only` writes nothing to the database; `--loop-file` restarts the capture
so the interface has a live-looking backend with no radio attached. `--fast`
replays as fast as possible, which is what `bench` uses.

### Running against the radio

```bash
cargo build --release --features usb
./target/release/skyward devices
./target/release/skyward run --source usb
```

Without the feature, start `rtl_tcp -a 127.0.0.1 -p 1234` separately and use
`--source tcp:127.0.0.1:1234`.

## The client

```bash
cd client
npm ci
npm run dev          # localhost:5173, proxying /api to the receiver
npm run check        # svelte-check; must be 0 errors and 0 warnings
npm run storybook    # localhost:6006
npm run build        # emits client/build, which the binary embeds
```

Vite proxies `/api`, `/healthz` and `/readyz` to `127.0.0.1:8080` (override
with `SKYWARD_API`), so the browser only ever sees one origin — which is also
how it ships. CORS therefore never enters into development.

**Rebuild the client before the binary.** `build.rs` watches `client/build`, so
a `cargo build` afterwards picks it up; without one, the binary serves a
placeholder page naming the command you skipped, and `doctor`'s `web.client`
check reports it.

`client/scripts/` holds three verification scripts that drive the client into
states you cannot reach by refreshing — the quiet sky, an outage, a theme
sweep. `scripts/outage.mjs` stops the *real* receiver rather than mocking the
failure, because that is what found the bug the staleness watchdog now
prevents. Read `client/scripts/README.md` before writing another.

## Adding a pipeline implementation

Four stages live in `adsb-dsp` and one in `adsb-track`, each a trait with named
implementations in a registry. A new one lands **beside** the old one and is
selected by string, so the comparison you usually want — your third attempt
against your second — is a flag rather than a branch.

1. Write it in the stage's module, implementing the trait.
2. Register it in `adsb-dsp/src/registry.rs` with a one-line description. That
   description is what `skyward list-impls` prints and what someone reads six
   months later, so make it say what is *different*, not what it is.
3. Run the benchmark against the baseline.

```bash
skyward bench --out runs/baseline.json
skyward bench --detect my-detector --compare runs/baseline.json
```

[§4.7 of the study guide](GUIDE.md) does this concretely.

### Reading the scoreboard

```
messages     517   unique     517   aircraft    7   positions    72
candidates   879   yield   58.8%   cand/msg   1.7   172.3 msg/min
guards: ghosts 0.000   realtime 401.3x   1.0 ns/sample
digest fnv1a64:c3ed11b99d82a415
```

- **digest** — a stable hash of the decoded messages. Identical digests mean
  identical output, so a refactor that claims to be a refactor is checkable.
- **messages** — up is better. This is the headline.
- **yield** — *reported, never optimised.* A better detector proposes more
  marginal candidates, so yield falls as messages rise. Tuning for it tunes the
  detector backwards.
- **ghosts** — aircraft that appeared and never confirmed. Decoded noise. Must
  not climb.
- **realtime** — how much faster than real time the pipeline ran. The number
  that decides whether a Pi can keep up.

One trap: `bench` scores fixtures against **the operator's** configured
receiver position, not the capture's sidecar. An Ottawa config against a
Pennsylvania capture drops the positions count hard with an identical message
count and digest, which looks exactly like a decode regression. Check the
station before the DSP.

## Fixtures

`fixtures/raw/*.cu8` are raw interleaved unsigned 8-bit I/Q at 2.4 MS/s, each
with a `.toml` sidecar describing when and where it was captured, what the gain
was, and what it is *for*.

The sidecar exists because a capture with no provenance is not evidence. And it
carries the receiver position: `porch.toml` is a Pennsylvania capture, and
scoring it from an Ottawa station silently drops its positions to nothing.

To capture your own, see [OPERATIONS.md](OPERATIONS.md#capturing-a-fixture).

## Conventions

**Comments explain why, not what.** The codebase is unusually heavily
commented, and almost all of it is decisions, trade-offs, and failures that
have actually happened. `git log` and the type signature cover the rest.

**Tests are named as claims.** `zero_gain_means_zero_not_auto`,
`an_unknown_api_path_is_a_json_404_not_the_app`,
`a_truncated_overlay_is_never_left_behind`. A test whose name is a sentence
tells you what broke without opening it.

**Every metric has to be unambiguous.** Before adding a counter, ask what two
different situations could produce the same value. That question is why the
effective sample rate exists (dropping samples versus hearing nothing), why
`source_overrun_bytes` exists (a buffering source defeats the effective rate),
and why the client averages over a window rather than over uptime.

**Errors say what to do next.** `SourceError::Config` fails loudly and is never
retried; `Transient` reconnects. `doctor` prints diagnoses, not measurements.

## Before committing

```bash
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo test -p adsb-source --features usb
cd client && npm run check && npm run build && cd ..
cargo build --release            # embeds the client you just built
skyward bench --compare runs/baseline.json
```

A clean build says nothing about behaviour. If a change is supposed to alter
what is decoded, the digest should move and you should be able to say why; if
it is not, the digest should not move at all.
