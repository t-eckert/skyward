# Operations

What to do when something is wrong, organised by what you can see. The rule
underneath all of it: **a green indicator is not evidence.** Check the end you
actually care about.

## The first three commands

In this order, always.

```bash
skyward devices                       # is the radio even visible
skyward doctor                        # the full self-check, in plain language
curl -s localhost:8080/healthz | jq   # what the running server sees
```

`doctor` exits `0` healthy, `1` warnings, `2` broken, and every line ends in
something to do. If you are about to paste output somewhere, use
`doctor --json`.

## Reading `/healthz`

```bash
watch -n2 'curl -s localhost:8080/healthz | jq "{status, source, decode}"'
```

| What you see | What it means |
|---|---|
| `status: "ok"`, `decode.messages` climbing | Working. Stop here |
| `status: "stalled"` | No samples for 30 s, or the source is down. A radio problem |
| `status: "degraded"` | Decoding fine; storage is failing. Check `store.errors` and disk |
| `source.state: "streaming"`, `messages` stuck at 0 | Samples arriving, nothing decodes. Usually the antenna |
| `source.effective_sample_rate_hz` below the configured rate | Samples are being **dropped**. Software, not reception |
| `source.overrun_bytes` climbing | Same thing, on a buffering source. Software, not reception |
| `source.reconnects` climbing | The link keeps dropping. Cable, hub, power, or a restarting `rtl_tcp` |

`warnings` is prose meant for a person; read it before anything else.

---

## Symptoms

### Nothing decodes, but samples are arriving

`source.state: streaming` with `decode.messages` at 0.

Look at `doctor`'s `radio.decode` line, which distinguishes three cases that
look identical from outside:

**Candidates found, zero valid CRCs.** That is the signature of a **sample rate
mismatch**, not a weak signal. The demodulator is looking for bit edges at the
wrong spacing. Check that `sample_rate_hz` matches what the hardware is
actually running — on USB, `/api/v1/receiver` reports the achieved rate, which
is not always the requested one.

It is also what a genuinely weak signal looks like, so if the rate is right,
this is placement.

**No candidates at all.** In order of likelihood: the antenna is not connected;
it is the wrong antenna (1090 MHz wants roughly 6.9 cm per element); it is
touching glass or metal, which detunes it badly; or it has no view of the sky.
Two minutes of traffic is normal even indoors.

Control experiment: tune an FM station around 98.5 MHz. If FM is fine and 1090
is silent, it is the antenna or its geometry, not the cable and not the
software.

**Clipping above 1%.** The front end is overloaded before anything else
matters. Reduce gain — try about 44 dB — and re-check. Maximum gain is often
not optimal; it lifts the noise floor equally and drives the tuner into
compression on a nearby aircraft.

### The message rate fell, and nothing was changed

Check in this order:

1. **`effective_sample_rate_hz` and `overrun_bytes`.** If either says samples
   are being lost, this is a software or CPU problem wearing reception's
   clothes.
2. **`doctor`'s `host.power` and `host.temperature`** (Linux). An undervolted
   or heat-soaked Pi throttles rather than crashing, which drops samples. The
   sticky since-boot bits catch a brownout that happened overnight.
3. **The antenna.** Wind moves things. Water gets into connectors.
4. **The sky.** Traffic genuinely varies by hour and by day.

### Positions fell, but messages did not

Messages steady, `decode.positions` down, and the bench digest unchanged.

**Check the station position before touching the decoder.** The 400 km range
gate is measured from the configured position, so a station configured for one
city while receiving in another rejects nearly everything — with an identical
message count and an identical digest. It looks exactly like a decode
regression and is not one.

```bash
curl -s localhost:8080/api/v1/receiver | jq '{lat, lon, origin, configured}'
skyward doctor --offline | grep station
```

The same trap applies to `skyward bench`, which scores fixtures against *the
operator's* configured position rather than the capture's sidecar.

### The position will not change

You edited `/etc/skyward.env`, restarted, and it is still the old value.

A position set through the web interface or the API is persisted to
`station_file` and **outranks** the config file and the environment.

```bash
skyward doctor --offline | grep station.shadowed
curl -X DELETE http://localhost:8080/api/v1/receiver    # revert and delete the overlay
```

More on the precedence in [CONFIGURATION.md](CONFIGURATION.md#the-station-overlay).

### A configuration change had no effect

```bash
skyward config
```

Every value with its origin. Three things to look for:

- `default` next to something you set — it did not reach the process.
- `$NAME (.env)` where you expected `$NAME` — the value is coming from a `.env`
  resolved against the working directory, and will vanish if anything runs from
  elsewhere. Use systemd's `EnvironmentFile=`.
- `skyward-station.toml` next to `receiver.lat` — see above.

A typo'd key in the *file* is a startup error, not a silent no-op. A typo'd
**environment variable** is not — nothing knows `SKYWARD_RECIEVER_LAT` was
meant to be anything.

### The dongle cannot be opened

```console
$ skyward devices
1 RTL-SDR device(s):
  usb:0  ...
$ skyward run
error: cannot open usb:0 (librtlsdr returned -6)
```

Listed but unopenable is the diagnostic case: enumeration reads descriptors
without claiming the device, so the hardware is fine and something else is
wrong. Two candidates:

**Another process holds it.** `rtl_tcp`, `rtl_test`, `dump1090`, or a previous
skyward that has not exited. In particular, never run both `rtl_tcp.service`
and a USB-mode skyward.

```bash
sudo systemctl stop rtl_tcp
sudo fuser -v /dev/bus/usb/*/* 2>&1 | head
```

**Permissions.** Install the udev rule and confirm the service user is in
`plugdev`:

```bash
sudo cp deploy/udev/99-skyward-rtlsdr.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
id skyward
```

If `devices` finds nothing at all, the DVB-T driver has claimed it — see
[RASPBERRY_PI.md](RASPBERRY_PI.md#free-the-dongle-from-the-dvb-driver).

### The service is down and stays down

```bash
systemctl status skyward
journalctl -u skyward -n 100 --no-pager
```

`Start request repeated too quickly` means systemd's restart limit tripped.
`deploy/systemd/skyward.service` sets `StartLimitIntervalSec=0` precisely
because a Pi booting after a power cut can lose several attempts before its USB
bus settles.

An exit code of `2` is a configuration error or a source that could not be
opened, and the log line above it says which.

### The interface is unreachable

```bash
curl -s localhost:8080/healthz     # from the Pi itself
```

Working locally but not remotely means `bind` is on localhost, or a firewall.
Set `SKYWARD_BIND=0.0.0.0:8080`.

### The browser shows "client not built into this binary"

`npm run build` was skipped before `cargo build`. The API is fine; only the
interface is a placeholder. Confirm with `doctor`'s `web.client` line, then
rebuild the client and the binary together.

---

## Routine work

### Following what it is doing

```bash
journalctl -u skyward -f
journalctl -u skyward --since "1 hour ago" -p warning
```

With `SKYWARD_LOG_FORMAT=json`, journald gets structured fields rather than
lines. Raise detail with `RUST_LOG=debug` in the environment file; `RUST_LOG`
takes the usual `tracing` syntax, so `debug,hyper=info` avoids drowning in HTTP
noise.

### Disk

```bash
du -h /var/lib/skyward/skyward.db*
curl -s localhost:8080/api/v1/stats | jq .store
```

History is trimmed to `retention_hours` (24 by default) by a sweep that runs
hourly; `store.retention_deleted` counts what it removed. `retention_hours = 0`
keeps everything and will eventually fill the disk.

`store.dropped` above zero means the write queue filled and rows were
discarded — deliberately, because the decoder must never block on storage. It
usually means the disk is slow, not that the decoder is.

The `-wal` file growing large is normal; it is checkpointed.

### Upgrading

```bash
sudo install -m 0755 skyward /usr/local/bin/skyward
sudo systemctl restart skyward
```

Server and interface move together — the client is inside the binary — so a
stale UI in front of a fresh server is structurally impossible. The database
and the station overlay live in `/var/lib/skyward` and are untouched.

Confirm what is actually running:

```bash
skyward --version
curl -s localhost:8080/healthz | jq .version
```

### Moving the receiver

Open the interface and click the position in the header, or:

```bash
curl -X PUT http://localhost:8080/api/v1/receiver \
  -H 'content-type: application/json' \
  -d '{"lat": 45.421, "lon": -75.697, "altitude_m": 70}'
```

Takes effect within a second, survives a restart, and loses nothing currently
tracked. Three decimals is all the range gate and local CPR can use.

### Checking a pipeline change did not break decoding

```bash
skyward bench --out runs/before.json
# ... make the change ...
skyward bench --detect correlator-v2 --compare runs/before.json
```

The digest is a stable hash of what was decoded: identical digests mean
identical output, so a refactor that claims to be a refactor is checkable.
Watch `ghosts` — aircraft that appeared and never confirmed — and remember that
**yield falls as a detector improves**, so a drop there is not a regression.

### Capturing a fixture

`rtl_tcp` and skyward both hold the device; stop them first or `rtl_sdr` gets
`usb_claim_interface error -6`.

```bash
sudo systemctl stop skyward
# -n counts samples; each is 2 bytes. 180 s at 2.4 MS/s = 432 000 000 samples.
rtl_sdr -f 1090000000 -s 2400000 -g 49.6 -n 432000000 fixtures/raw/new.cu8
```

Write the sidecar `.toml` beside it, including the receiver position the
capture was taken at — `porch.toml` is the worked example. Without it, scoring
that fixture from another station silently drops the positions count.

---

## Reporting a problem

```bash
skyward doctor --json > doctor.json
curl -s localhost:8080/healthz > health.json
curl -s localhost:8080/api/v1/stats > stats.json
journalctl -u skyward -n 500 --no-pager > skyward.log
```

`doctor --json` carries the build, the expanded pipeline, and every
configuration value with its provenance, which is most of what any question
needs. **It also carries the receiver position** — a home address. Round it or
remove it before posting anywhere public.
