# Raspberry Pi readiness: an audit

What actually stands between a fresh Pi and a working receiver, checked rather
than assumed. Each finding says how it was verified, what was done about it,
and — where nothing was done — why.

**Boundary of this audit.** Everything below was checked on the development
machine (macOS, aarch64) against this repository at the time of writing. **No
step was run on a Raspberry Pi, and no RTL-SDR was attached** — `rtl_test -t`
reported "No supported devices found" throughout. Findings are therefore of two
kinds, and they are labelled: *measured* (a command was run and its output
read) and *read from the source* (behaviour established from the code, and
still needing a Pi to confirm). Nothing here is labelled measured on the
strength of a code change alone.

---

## Summary

| # | Finding | Severity | Status |
|---|---|---|---|
| 1 | Deployment needed a second process (`rtl_tcp`) and a second unit | High | **Fixed** — direct USB, `--features usb` |
| 2 | A dropped source never reconnected; the retry loop only re-read | **Critical** | **Fixed** — `IqSource::reconnect` |
| 3 | Cross-compiling is *not* dependency-free, despite the claim | Medium | **Documented** — needs a C cross-toolchain |
| 4 | systemd units, udev rules and the DVB blacklist existed only inside prose | Medium | **Fixed** — `deploy/` |
| 5 | History retention was hardcoded at 24 h | Medium | **Fixed** — `retention_hours` |
| 6 | Undervoltage and thermal throttling are invisible and mimic bad reception | Medium | **Fixed** — `doctor` host checks |
| 7 | Docs claimed the server refuses to start without a receiver position | Medium | **Fixed** — it never did; and now it need not |
| 8 | The receiver position could only be changed by editing a file and restarting | Medium | **Fixed** — settable at runtime |
| 9 | A buffering source can drop samples with every counter looking healthy | Medium | **Fixed** — `source_overrun_bytes` |
| 10 | Building the client on the Pi needs Node and a lot of memory | Low | **Documented** — build it off-Pi |
| 11 | `.env` is resolved relative to the working directory | Low | **Documented** — units use `EnvironmentFile` |
| 12 | The API is unauthenticated and now has a write endpoint | Low | **Mitigated** — `station_writable` |

---

## 1. The deployment needed two processes

**Was:** `adsb-source` supported `file:` and `tcp:`. `usb:` parsed, and then
`open()` returned "not implemented yet". The only way to reach a dongle was to
run `rtl_tcp` beside skyward — a second unit to supervise, a second thing to
order at boot, and a localhost socket carrying 4.8 MB/s for no reason.

This sat oddly against the design's own first principle. `main.rs` opens with a
paragraph on why skyward is one binary rather than a decoder and an API: "two
processes means two ways to deploy the wrong version, two units to supervise,
two places for a config mismatch to hide." The radio was exactly that, one
layer down.

**Now:** `crates/adsb-source/src/usb.rs` drives the dongle through librtlsdr,
behind the off-by-default `usb` cargo feature.

```bash
cargo build --release --features usb
skyward devices          # what the bus offers
skyward run --source usb
```

The feature is off by default deliberately — see finding 3.

*Measured:* the feature compiles and links, and the FFI is exercised by
`cargo test -p adsb-source --features usb` (34 passing), which includes a test
that calls `rtlsdr_get_device_count` for real. `skyward devices` returns the
same answer as `rtl_test -t` on a machine with no dongle attached. *Read from
the source, not measured:* everything from `rtlsdr_open` onward. Tuning, gain,
the async transfer and the ring buffer have never run against hardware.

---

## 2. A dropped source never came back

**Critical, and the worst kind of bug this system can have** — the failure it
is most explicitly designed to prevent.

**Was:** the decode loop treated a transient error correctly at first glance —
count it, back off, retry:

```rust
Err(e) => {
    counters.reconnects.fetch_add(1, Ordering::Relaxed);
    std::thread::sleep(backoff);
    continue;          // ... and read again from the same dead socket
}
```

But it only ever retried the *read*. `TcpSource` had a `reconnect()` method
that re-dialled and replayed every setting; **nothing called it.** Once
`rtl_tcp` restarted, `read` returned `Ok(0)` → `Transient("closed the
connection")`, forever. The service stayed `active`, the log repeated one line
every thirty seconds, `reconnects` climbed impressively, and no sample ever
arrived again until someone restarted the process.

This is precisely the outcome `run.rs`'s own module docs call unacceptable: "a
receiver that quietly died six hours ago is the worst outcome."

**Now:** `reconnect` is on the `IqSource` trait, defaulting to a no-op success
(right for a file, which has nothing to re-establish), implemented by
`TcpSource` as the re-dial-and-replay it already had, and by `UsbSource` as
close-reopen-replay-restart. The decode loop asks for it after each backoff.

*Read from the source, not measured:* the fix is exercised by the existing
`TcpSource` reconnect tests, but a full rtl_tcp-restart-under-load cycle has
not been run.

---

## 3. Cross-compiling is not dependency-free

**The existing documentation is wrong about this**, in both `crates/adsb-source/src/tcp.rs`
("Keeping USB out of this binary also means zero C dependencies, which turns
cross-compiling for aarch64 from an afternoon into a non-event") and in
`docs/RASPBERRY_PI.md` ("Because `rtl_tcp` keeps the binary free of C
dependencies, an `aarch64` cross-build is clean").

*Measured:*

```
$ cargo tree -p adsb-server --edges normal | grep -i sys
        └── libsqlite3-sys v0.30.1
```

`adsb-store` depends on `rusqlite` with the `bundled` feature, which compiles
SQLite's C amalgamation through the `cc` crate at build time. Cross-compiling
has therefore always needed a C cross-toolchain — the `usb` feature does not
introduce the problem, it just adds a second C library to an existing one.

What *is* true, and worth keeping: the `usb` feature adds **zero Rust
dependencies**. `cargo tree` prints 264 lines with and without it; the binding
is hand-written FFI, not a crate.

**Now:** the claims are corrected, and `docs/RASPBERRY_PI.md` gives three real
options — build on the Pi, cross-compile with `cross` (which supplies the C
toolchain in a container), or cross-compile with an explicitly installed
`aarch64-linux-gnu-gcc`.

*Not verified:* no aarch64 build was performed. This machine has neither the
target (`rustup target list --installed` shows only `aarch64-apple-darwin`) nor
`cross`, `zig`, or `aarch64-linux-gnu-gcc`.

---

## 4. The deployment files existed only as prose

**Was:** `docs/RASPBERRY_PI.md` contained two systemd units and a modprobe
blacklist inside `tee` heredocs. Copy-pasting a unit file out of a code fence
in a browser is a way to get a service that starts and a way to get one that
does not, and there is no way to review a change to it.

**Now:** `deploy/` holds real files, versioned and diffable:

```
deploy/
├── install.sh                        # five explicit steps, refuses rather than guesses
├── blacklist-rtl.conf                # /etc/modprobe.d/
├── systemd/
│   ├── skyward.service               # the USB build: one unit, no rtl_tcp
│   └── rtl_tcp.service               # only for a build without the feature
└── udev/
    └── 99-skyward-rtlsdr.rules       # unprivileged access via plugdev
```

The unit fixes three things the prose version had wrong or missing:

- **`StartLimitIntervalSec=0`.** With systemd's default, five restarts inside
  ten seconds put the unit in a permanent `failed` state. A Pi that boots
  before its USB bus settles — after a power cut, the normal case — would go
  down and stay down.
- **`EnvironmentFile=` rather than a `.env`.** See finding 11.
- **`ProtectSystem=strict` and friends,** with `ReadWritePaths=/var/lib/skyward`.

*Read from the source, not measured:* `install.sh` passes `bash -n` and
`shellcheck` clean. Neither it nor the unit files have been run — there is no
Linux machine in this environment, and `systemd-analyze verify` does not exist
on macOS.

---

## 5. Retention was hardcoded

**Was:** `run.rs` built its `StoreConfig` as `{ path, ..Default::default() }`,
and the default carried `retention: Some(24h)`. There was no configuration key.
A station wanting a week of history had to be recompiled; one on a nearly-full
SD card had no way to ask for less.

**Now:** `retention_hours`, with the same defaults-file-environment layering as
everything else, `0` meaning keep everything, and a startup error above one
year — because that value is far more often a units mistake than an intention.

---

## 6. Undervoltage and throttling are invisible, and look like bad reception

An RTL-SDR draws roughly 300 mA on top of the board. That is enough to push a
marginal supply over the edge, and a Raspberry Pi's response to undervoltage is
not to crash — it is to drop its clock. A throttled Pi cannot consume 2.4 MS/s,
so samples are dropped, so the message count falls. Every visible symptom points
at the antenna. The firmware has known the whole time and nothing asked it.

The same is true of a Pi that has heat-soaked inside a closed case, which is
where an ADS-B receiver spends its life.

**Now:** `skyward doctor` reads `get_throttled` and
`/sys/class/thermal/thermal_zone0/temp` on Linux and reports both, including
the sticky since-boot bits that catch the brownout at 3am. It also reports
`MemAvailable`. On anything that is not Linux the checks are skipped rather
than faked.

*Read from the source, not measured:* these files do not exist on macOS, so the
Linux path has never executed. The `#[cfg(target_os = "linux")]` block compiles
only on Linux and has not been compiled here either — treat it as untested code
until a Pi runs `skyward doctor`.

---

## 7. "The server refuses to start without a position" was never true

Both `docs/RASPBERRY_PI.md` and `.env.example` stated that an unset receiver
position is a hard startup failure. `config.rs`'s own module documentation says
the same: "Better to refuse to start."

*Measured:*

```
$ skyward config --source file:/dev/null
  receiver.lat           (unset)                      default
  receiver.lon           (unset)                      default
$ echo $?
0
```

`Config::validate` requires latitude and longitude to arrive *together or not
at all*, and accepts `(None, None)`. `run` then logs a warning and decodes
happily with the range gate disabled. This is the better behaviour — a receiver
that will not start is worse than one that starts and says what it is missing —
but the documentation described something else, which is how someone ends up
debugging a startup failure that cannot happen.

**Now:** the docs describe what the code does, and finding 8 makes the missing
position fixable without touching a file at all.

---

## 8. The position could only be changed by editing a file and restarting

The receiver position is load-bearing: it is the reference for the 400 km range
gate, what local CPR resolves against, and the centre of the map's range rings.
It is also the one setting whose correct value is *discovered* rather than
configured — you put the Pi somewhere, you move the antenna, someone borrows
the whole thing for a weekend.

Making that an edit to a root-owned file on a machine you deliberately do not
log into, followed by a restart that drops every tracked aircraft, is the wrong
shape for the task.

**Now:** `PUT /api/v1/receiver` and a dialog in the web interface. The position
is persisted to its own small file so it survives a restart, and
`DELETE /api/v1/receiver` reverts to configuration. The tracker adopts the new
gate without rebuilding, so nothing currently tracked is lost.

The precedence inversion this creates — the runtime overlay outranks
`skyward.toml` and the environment — is a genuine hazard, and it is the "I
edited the config and nothing changed" failure this codebase treats as a
first-class bug. Three things make it visible: the origin is reported by
`skyward config` and in the API response, `doctor` has a `station.shadowed`
check that fires when the overlay disagrees with configuration, and `run` logs
it at startup.

*Measured:* full round trip against a running server and in a real browser —
unset → rejected bad latitude (`400`, with the server's sentence surfacing in
the dialog) → set → persisted to disk → adopted by the decoder 126 ms later →
survived a restart → shadow warning logged → reverted.

---

## 9. A buffering source can drop samples while every counter looks healthy

The one number that normally separates "dropping samples" from "hearing
nothing" is the effective sample rate — count samples, divide by elapsed wall
time. `doctor` and `/healthz` both lean on it.

It does not work for a source that buffers. `UsbSource` keeps a ring the dongle
fills asynchronously; when the decoder falls behind, the ring drops blocks and
`read` keeps returning a full 2.4 MS/s. The effective rate stays perfect while
a tenth of the sky goes missing.

**Now:** `IqSource::overruns()` reports what the source itself discarded, the
decode loop publishes it as `source_overrun_bytes`, and `/healthz` raises a
warning naming the cause: "the decoder is not keeping up with the radio. This
looks exactly like poor reception in the message count and is a software
problem."

---

## 10. Building the client on the Pi

`client/` is SvelteKit built with Vite, and Vite is memory-hungry. On a Pi Zero
2 W (512 MB) a cold `npm ci && npm run build` is a coin flip against the OOM
killer; on a 1 GB Pi 3 it works but takes a long time.

Nothing was changed here — the build is what it is — but the recommended path
in `docs/RASPBERRY_PI.md` is now to build the client on a laptop and either
cross-compile the whole binary or copy `client/build` across. `build.rs`
already handles the "client not built" case gracefully: it writes a placeholder
page naming the missing command, and `doctor`'s `web.client` check reports
whether a real client is embedded.

---

## 11. `.env` is resolved relative to the working directory

`dotenvy::dotenv()` walks up from the current directory. That is right for
development and a trap for a service: the file silently stops applying the
moment anything runs from somewhere else, and the only clue is a config dump
saying `default` next to a value you definitely wrote down.

`config.rs` already handles this better than most — it distinguishes
`Origin::DotEnv` from `Origin::Env` precisely so the dump can tell them apart,
and `doctor` prints which file was found. The remaining gap was that the
documented systemd unit did not use `EnvironmentFile=`, so an operator following
the guide got the `.env` behaviour by accident.

`deploy/systemd/skyward.service` uses `EnvironmentFile=/etc/skyward.env`.

---

## 12. The API is unauthenticated, and now has a write endpoint

It always was: `/api/v1/aircraft`, `/api/v1/stats` and the station position
have been readable by anyone who can reach the port. The CORS allowlist is not
a security boundary — it constrains browsers, not `curl`.

Finding 8 adds the first endpoint that *changes* something. On a home LAN
appliance whose whole purpose is to be opened from a phone, that is a
reasonable default, and the blast radius is one map pin. But it should be a
choice: `station_writable = false` (or `SKYWARD_STATION_WRITABLE=false`) pins
the position to configuration, and the dialog then renders disabled with the
reason shown rather than failing silently.

If the receiver is reachable from outside a trusted network, put it behind a
reverse proxy with authentication. Nothing in skyward provides that, and
nothing in skyward pretends to.

---

## Things that were already right

Worth recording, because an audit that lists only problems misrepresents the
system.

- **The decoder does not block on storage.** The write queue is bounded and
  full means drop-and-count. A dropped row costs one dot on a map; a stalled
  decoder loses samples permanently.
- **Health is freshness, not liveness.** `/healthz` returns 503 when samples
  have stopped arriving, so a bare `curl -f` is a valid probe. The obvious
  alternative — 200 whenever the process is up — stays true for hours after the
  decoder dies.
- **The client is compiled into the binary.** Deployment is one file, and a
  stale UI in front of a fresh server is structurally impossible.
- **Configuration carries provenance.** `skyward config` prints where every
  value came from, which is the only way to answer "did my edit take effect" on
  a machine you are not logged into.
- **Unknown config keys are a hard error.** `deny_unknown_fields` turns a typo
  from a silent no-op into a startup failure.
- **The offline self-test.** `doctor` decodes synthetic frames on the Pi's own
  CPU with no antenna attached, which separates "bad build" from "bad
  reception" before anyone goes climbing after the antenna.
- **CPU headroom is not a concern.** *Measured* on this machine
  (aarch64 macOS, release build): 1.0–1.7 ns/sample, 250–400× realtime across
  the three fixtures. *Extrapolated, not measured:* a Pi 4 core is roughly 5–8×
  slower at scalar work of this kind, which would leave ~40–70× headroom, and a
  Pi Zero 2 W perhaps 15–25×. Confirm on your own hardware with
  `skyward bench` — the `realtime` figure it prints is the number that matters.

---

## What a Pi still has to confirm

Everything in this audit that is labelled *read from the source*. Concretely,
on first deployment:

```bash
skyward devices                    # finding 1: the FFI against real hardware
skyward doctor                     # findings 6, 7: host checks, position handling
skyward bench                      # the realtime figure on this CPU
curl -s localhost:8080/healthz | jq '.source'   # finding 9: overrun_bytes stays 0
sudo systemctl restart rtl_tcp     # finding 2: skyward must recover by itself
```
