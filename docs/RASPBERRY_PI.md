# Running skyward on a Raspberry Pi

skyward is one binary with the web client compiled in. A deployment is
therefore short: prepare the OS, get a binary onto the Pi, point it at the
radio, let systemd keep it alive. This guide is written for a headless Pi
reached over SSH — the machine skyward was designed for.

Two shapes of deployment are supported, and the first one is simpler:

| | **Direct USB** (recommended) | **Via `rtl_tcp`** |
|---|---|---|
| Build | `cargo build --release --features usb` | `cargo build --release` |
| Needs | `librtlsdr-dev` at build time | nothing extra |
| Processes | one | two |
| Source | `SKYWARD_SOURCE=usb` | `SKYWARD_SOURCE=tcp:127.0.0.1:1234` |

Direct USB is the default recommendation because it removes a process, a unit,
and a localhost socket carrying 4.8 MB/s. `rtl_tcp` remains fully supported —
it is the right answer when the dongle is on a *different* machine from the
decoder, and it is the fallback if the `usb` feature will not build for you.

Everything skyward-specific below (flag names, environment variables, ports,
`doctor` checks) comes from the code in this repository. The OS steps (`apt`,
`udev`, `systemd`) are ordinary Debian.

## What you need

- A Raspberry Pi 3, 4, 5, or Zero 2 W running **64-bit Raspberry Pi OS**
  (Bookworm or newer). 64-bit matters: the toolchain and release profile target
  `aarch64`.
- An **RTL-SDR dongle** — RTL2832U with an R820T/R820T2 tuner is what the
  defaults assume — and a **1090 MHz antenna**.
- A power supply with headroom. The dongle draws about 300 mA on top of the
  board, and an undervolted Pi throttles rather than crashing, which looks
  exactly like bad reception. `skyward doctor` checks for this.
- **The antenna matters more than everything else on this page.** See
  [Antenna placement](#antenna-placement-is-the-whole-game).

---

## 1. Prepare the operating system

### Free the dongle from the DVB driver

Raspberry Pi OS ships `dvb_usb_rtl28xxu` and loads it automatically, because as
far as the kernel is concerned this *is* a television tuner. It claims the
device at boot, and every later attempt to open it fails with
`usb_claim_interface error -6` — which reads like a permissions problem and is
not one.

```bash
sudo cp deploy/blacklist-rtl.conf /etc/modprobe.d/
sudo modprobe -r dvb_usb_rtl28xxu     # take effect now, without a reboot
```

### Grant USB access without running as root

```bash
sudo cp deploy/udev/99-skyward-rtlsdr.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Installing the `rtl-sdr` package does the same thing and also gives you
`rtl_test` for cross-checking. It is worth having either way:

```bash
sudo apt update && sudo apt install -y rtl-sdr
```

### Keep the clock honest

A Pi has no real-time clock and boots in 1970. Every timestamp is then garbage,
`?max_age=` filters return nothing, and the whole system looks like a dead
radio. It is almost always the last thing anyone suspects, so `doctor` checks it
first.

```bash
sudo timedatectl set-ntp true
timedatectl status        # want "System clock synchronized: yes"
```

### Confirm the dongle is visible

```bash
rtl_test -t
```

`Found 1 device(s)` and a tuner line (`Rafael Micro R820T`) is what you want.
Nothing found, after the blacklist step, means the module is still loaded or
something else already holds the device — another `rtl_tcp`, `dump1090`, a
previous `skyward`.

---

## 2. Get a binary onto the Pi

The client is embedded at compile time, so it is built first and `cargo build`
picks it up. A checkout that never built the client still compiles — the binary
serves a placeholder page naming the command you skipped, and `doctor`'s
`web.client` check reports it.

Pick one of the three routes below.

### Route A — build on the Pi

Simplest, and slow. Fine on a Pi 4 or 5.

```bash
# Rust. rust-toolchain.toml pins the version; rustup honours it.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# librtlsdr, for the usb feature.
sudo apt install -y librtlsdr-dev

# Node, for the client. Debian's own package is often too old for Vite.
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs

git clone https://github.com/t-eckert/skyward.git && cd skyward
cd client && npm ci && npm run build && cd ..
cargo build --release --features usb
```

> **On a Pi Zero 2 W, do not build the client here.** Vite under 512 MB is a
> coin flip against the OOM killer. Build `client/build` on a laptop and copy
> the directory across before running `cargo build`, or use route B.

### Route B — cross-compile with `cross` (recommended)

`cross` runs the build in a container that already has the C cross-toolchain,
which is what makes this the least fragile route.

> **Note:** the build is *not* free of C dependencies, whatever earlier
> versions of this guide said. `adsb-store` uses `rusqlite` with the `bundled`
> feature, which compiles SQLite's C amalgamation, and the `usb` feature adds
> librtlsdr. A cross-build has always needed a C cross-toolchain; `cross`
> supplies one.

On your laptop:

```bash
cd client && npm ci && npm run build && cd ..

cargo install cross
cross build --release --features usb --target aarch64-unknown-linux-gnu

scp target/aarch64-unknown-linux-gnu/release/skyward pi@raspberrypi:
```

That single file is the whole deployment. The client is already inside it.

### Route C — cross-compile with a system toolchain

If you would rather not run Docker:

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt install -y gcc-aarch64-linux-gnu libsqlite3-dev:arm64 librtlsdr-dev:arm64

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export RTLSDR_LIB_DIR=/usr/lib/aarch64-linux-gnu

cargo build --release --features usb --target aarch64-unknown-linux-gnu
```

`RTLSDR_LIB_DIR` tells `crates/adsb-source/build.rs` where to look; without it
the build script searches the host's directories, finds nothing, and prints a
warning naming the variable.

---

## 3. Install

```bash
sudo ./deploy/install.sh
```

Five explicit steps, each printed: create the `skyward` system user and put it
in `plugdev`; install the binary to `/usr/local/bin`; create
`/var/lib/skyward`; install the udev rule and DVB blacklist; write
`/etc/skyward.env` **only if one does not already exist**; install and enable
the unit without starting it.

It refuses rather than guesses. It will not build anything, and it will not
overwrite a configuration file holding a receiver position someone typed.

If you would rather do it by hand, read the script — it is short, and every
step is a single command.

### The `rtl_tcp` variant

Only for a binary built **without** the `usb` feature. Two processes competing
for one USB device is a guaranteed `usb_claim_interface error -6`, so install
this unit *or* use direct USB, never both.

```bash
sudo cp deploy/systemd/rtl_tcp.service /etc/systemd/system/
sudo systemctl enable --now rtl_tcp
# and in /etc/skyward.env:
#   SKYWARD_SOURCE=tcp:127.0.0.1:1234
```

No `-f` or `-s` flags: skyward speaks the rtl_tcp control protocol and sets the
frequency, sample rate and gain itself over the socket.

---

## 4. Configure

Precedence, lowest to highest:

```
defaults  <  skyward.toml  <  .env / environment  <  CLI flags  <  the runtime station overlay
```

`skyward config` prints every value **with where it came from**, which is how
you confirm an edit took effect on a machine you are not logged into. The full
list of keys is in [CONFIGURATION.md](CONFIGURATION.md).

On a Pi the clean choice is `/etc/skyward.env`, read by systemd's
`EnvironmentFile=`. Do not rely on a `.env` file: `dotenvy` resolves it
relative to the *working directory*, so it silently stops applying the moment
anything runs from elsewhere.

The minimum that matters:

```bash
sudo tee /etc/skyward.env >/dev/null <<'EOF'
# Three decimals (~100 m) is plenty. Local CPR only needs the reference in the
# same ~670 km zone, and the range gate works at 400 km. Precision buys nothing
# here and publishing your exact address costs something.
SKYWARD_RECEIVER_LAT=45.421
SKYWARD_RECEIVER_LON=-75.697
SKYWARD_RECEIVER_ALT_M=70

SKYWARD_SOURCE=usb
SKYWARD_SAMPLE_RATE_HZ=2400000
# Must be a step this tuner actually offers, or startup fails and prints the
# real list. Maximum gain is often NOT optimal — it lifts the noise floor too.
SKYWARD_GAIN_DB=49.6

SKYWARD_BIND=0.0.0.0:8080
SKYWARD_DB_PATH=/var/lib/skyward/skyward.db
SKYWARD_STATION_FILE=/var/lib/skyward/skyward-station.toml
SKYWARD_RETENTION_HOURS=24

SKYWARD_LOG_FORMAT=json
RUST_LOG=info
EOF
```

**You can leave the position blank.** The server starts without one — it logs a
warning, disables the range gate and local CPR, and decodes everything else —
and you can set it from the web interface once it is running, without a
restart. That is usually easier than editing a root-owned file over SSH.

Note the precedence, though: a position set that way is persisted to
`SKYWARD_STATION_FILE` and **overrides** this file from then on. If you later
edit `/etc/skyward.env` and nothing changes, that is why. `skyward doctor` says
so explicitly (`station.shadowed`), and `curl -X DELETE .../api/v1/receiver`
reverts it.

---

## 5. Start it, and verify it is actually receiving

Do not trust "the service is active". That proves the process is up, which is
exactly the thing that stays true for hours after a decoder dies. Check the
real ends.

```bash
# Is the dongle visible and free? Works even while something else holds it.
skyward devices

# The full self-check. Read every [warn] and [FAIL]; each ends in something to do.
sudo -u skyward skyward doctor

sudo systemctl start skyward
journalctl -u skyward -f
```

Then, from anywhere on the network:

```bash
curl -s http://raspberrypi.local:8080/healthz | jq
```

`status: "ok"` with `decode.messages` climbing is the goal. What the fields
mean, and what to do when they are wrong, is in
[OPERATIONS.md](OPERATIONS.md).

Open the interface at `http://<pi>:8080/`.

---

## Antenna placement is the whole game

Measured on identical hardware, moving one antenna through a doorway changed
the message rate by more than 4×. No software change in this repository has
ever come close to that.

Candidate preambles with **zero CRC-valid messages** is the signature of a
signal that is present but too weak — almost always placement. Outdoors beats
every indoor spot; a window beats an interior room; never mount the dipole flat
against glass, where at 1090 MHz a coated pane detunes it and drops peak signal
about sevenfold.

If reception seems dead, tune an FM station around 98.5 MHz as a control. FM
fine and 1090 silent means the antenna or its geometry, not the cable and not
the software. The [README antenna
table](../README.md#antenna-because-it-dominates-everything) and [Part VI of
the study guide](GUIDE.md) cover why.

---

## Updating a deployed Pi

```bash
# On the build machine
cd client && npm ci && npm run build && cd ..
cross build --release --features usb --target aarch64-unknown-linux-gnu
scp target/aarch64-unknown-linux-gnu/release/skyward pi@raspberrypi:

# On the Pi
sudo install -m 0755 ~/skyward /usr/local/bin/skyward
sudo systemctl restart skyward
```

Because the client is embedded, this replaces server and interface together;
there is no way to leave a stale client in front of a fresh server.
`skyward --version` and `doctor`'s `web.client` line both describe what is
actually inside the running binary.

The database and the station overlay live in `/var/lib/skyward` and are
untouched by an upgrade.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `usb_claim_interface error -6`, or `librtlsdr returned -6` | The DVB driver or another process holds the dongle | §1 blacklist; `sudo systemctl stop skyward rtl_tcp`; check for `dump1090` |
| `skyward devices` lists it, but opening fails | Permissions, not presence — enumeration does not claim the device | Install the udev rule; confirm the service user is in `plugdev` |
| `skyward devices` finds nothing, `rtl_test` too | Blacklist not applied, or power | `modprobe -r dvb_usb_rtl28xxu`; try a powered hub |
| `usb:0 needs a binary built with the usb feature` | Built without `--features usb` | Rebuild with it, or use `rtl_tcp` |
| Build warns "librtlsdr was not found" | No `librtlsdr-dev` | `apt install librtlsdr-dev`, or set `RTLSDR_LIB_DIR` |
| Exits at startup: "gain … is not one of this tuner's steps" | `SKYWARD_GAIN_DB` is not a discrete value | Use one the error prints (49.6 is the R820T maximum) |
| Source streams, `messages` stays 0 | Weak signal, or a sample-rate mismatch | Antenna placement first; if there are candidates but no CRCs, check the configured rate against the hardware's |
| Message rate falls, `overrun_bytes` climbing | The decoder is not keeping up | A software problem, not reception: check `host.power` and `host.temperature` in `doctor` |
| `doctor` reports undervoltage or throttling | Marginal power supply | Supply rated for the board plus ~300 mA; short, thick cable |
| Config edit has no effect on the position | A runtime overlay is shadowing it | `doctor` → `station.shadowed`; `curl -X DELETE .../api/v1/receiver` |
| Service down after a power cut and stays down | systemd's default restart limit | Use `deploy/systemd/skyward.service`, which sets `StartLimitIntervalSec=0` |
| `doctor` flags the clock | No RTC, NTP not synced | `sudo timedatectl set-ntp true` |
| Interface unreachable from another machine | Bound to localhost, or a firewall | `SKYWARD_BIND=0.0.0.0:8080` |
| Browser shows "client not built into this binary" | `npm run build` was skipped before `cargo build` | Build the client, rebuild, reinstall |

A fuller account of what was checked, what was fixed, and what a Pi still has
to confirm is in [PI_AUDIT.md](PI_AUDIT.md).
