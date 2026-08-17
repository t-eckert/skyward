# Running skyward on a Raspberry Pi

skyward is one static binary with the web client compiled in, so a Pi
deployment is a short list: prepare the OS, build the binary, point it at the
radio, and let systemd keep it alive. This guide is written for a headless Pi
you reach over SSH — the box skyward was designed to run on.

Every skyward-specific value here (flag names, environment variables, ports,
`doctor` checks) is taken from the code in this repository. The OS steps
(`apt`, `udev`, `systemd`) are standard Debian/Raspberry Pi OS and are not
skyward-specific.

## What you need

- A Raspberry Pi 3, 4, 5, or Zero 2 W running **64-bit Raspberry Pi OS**
  (Bookworm or newer). 64-bit matters: the pinned Rust toolchain and the
  release profile target `aarch64`.
- An **RTL-SDR dongle** (RTL2832U + R820T/R820T2 is what the defaults assume)
  and a **1090 MHz antenna**. A tuned ADS-B antenna outdoors or at a window is
  worth far more than anything in software — see
  [Antennas](#antenna-placement-is-the-whole-game) below.
- Network access to the Pi, and a few GB free for the toolchain and build.

## 1. Prepare the operating system

### Free the dongle from the DVB driver

Raspberry Pi OS ships a kernel driver (`dvb_usb_rtl28xxu`) that claims the
dongle as a TV tuner the moment it is plugged in, so `rtl_tcp` gets
`usb_claim_interface error -3`. Blacklist it once:

```bash
sudo tee /etc/modprobe.d/blacklist-rtlsdr.conf >/dev/null <<'EOF'
blacklist dvb_usb_rtl28xxu
blacklist rtl2832
blacklist rtl2830
blacklist rtl8xxxu
EOF
sudo reboot
```

### Install the RTL-SDR tools

```bash
sudo apt update
sudo apt install -y rtl-sdr
```

This provides `rtl_tcp` (the recommended source — see the note at the end of
[§4.3 in the study guide](GUIDE.md)) and the udev rules that grant dongle
access to the `plugdev` group.

### Keep the clock honest

A Pi has no real-time clock. skyward stamps every snapshot and every stored
row with the wall clock, and `doctor` refuses a clock that looks implausible,
so make sure time is synced before you rely on timestamps:

```bash
sudo timedatectl set-ntp true
timedatectl status        # "System clock synchronized: yes"
```

### Verify the dongle is seen

```bash
rtl_test -t
```

You want `Found 1 device(s)` and a tuner line (e.g. `Rafael Micro R820T`). If
it reports no devices after the reboot above, the blacklist did not take —
recheck the file and that nothing else (another `rtl_tcp`, `dump1090`) already
holds the device.

## 2. Build skyward

skyward embeds the web client at compile time, so the client is built first
and `cargo build` picks it up. A checkout that never built the client still
compiles — the binary just serves a placeholder page telling you which command
you skipped.

### Toolchains

```bash
# Rust (the repo pins the exact version in rust-toolchain.toml; rustup honours it)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

# Node, to build the SvelteKit client. Debian's own package is often too old
# for Vite; NodeSource gives a current LTS.
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs
```

### Build, client first

```bash
git clone https://github.com/t-eckert/skyward.git
cd skyward

cd client
npm ci
npm run build          # emits client/build, which the server embeds
cd ..

cargo build --release -p adsb-server
```

The binary lands at `target/release/skyward`. On a Pi 4/5 the first build takes
several minutes; a Pi Zero 2 W is much slower but works.

> **Faster: cross-compile on a laptop.** Because `rtl_tcp` keeps the binary free
> of C dependencies, an `aarch64` cross-build is clean. Build the client
> (`cd client && npm run build`), then from the repo root
> `cargo install cross` and
> `cross build --release -p adsb-server --target aarch64-unknown-linux-gnu`,
> and copy `target/aarch64-unknown-linux-gnu/release/skyward` to the Pi. That
> is the whole deployment — one file — because the client is already inside it.

### Install the binary

```bash
sudo install -m 0755 target/release/skyward /usr/local/bin/skyward
skyward --version
```

## 3. Configure

skyward's precedence, lowest to highest, is:
`defaults < skyward.toml < .env / environment < CLI flags`. On a Pi the clean
choice is an environment file read by systemd, so configuration lives in one
root-owned place and `doctor` can show exactly where each value came from.

The receiver position is a home address; keep it out of any repo and readable
only by root.

```bash
sudo tee /etc/skyward.env >/dev/null <<'EOF'
# --- Receiver position (required) --------------------------------------------
# Three decimals (~100 m) is plenty; precision buys nothing and publishing your
# exact address costs something. The server refuses to start without this.
SKYWARD_RECEIVER_LAT=45.421
SKYWARD_RECEIVER_LON=-75.697
SKYWARD_RECEIVER_ALT_M=70

# --- Radio -------------------------------------------------------------------
SKYWARD_SOURCE=tcp:127.0.0.1:1234
SKYWARD_SAMPLE_RATE_HZ=2400000
# Must be a value your tuner actually offers, or startup fails and prints the
# real list. Max gain is often NOT optimal — it lifts noise too.
SKYWARD_GAIN_DB=49.6

# --- Server ------------------------------------------------------------------
# 0.0.0.0 so you can open the UI from another machine on your LAN.
SKYWARD_BIND=0.0.0.0:8080
SKYWARD_DB_PATH=/var/lib/skyward/skyward.db

SKYWARD_LOG_FORMAT=json
RUST_LOG=info
EOF
sudo chmod 600 /etc/skyward.env
```

Edit `SKYWARD_RECEIVER_LAT/LON/ALT_M` to **your** antenna location. The values
above are a placeholder (central Ottawa); the range ring and the range
plausibility gate are both drawn around this point, so a wrong position quietly
rejects real aircraft.

Create the state directory and a user to own it:

```bash
sudo useradd --system --home /var/lib/skyward --create-home --shell /usr/sbin/nologin skyward
sudo usermod -aG plugdev skyward        # USB access to the dongle
```

## 4. Run it under systemd

Two services: `rtl_tcp` holds the dongle and streams samples on localhost, and
`skyward` connects to it, tunes it (frequency, sample rate, and gain are sent
over the rtl_tcp control protocol — you do not pass them to `rtl_tcp`), decodes,
and serves the API and UI. skyward treats a dropped rtl_tcp connection as
transient and reconnects, so the ordering between them is soft.

### `rtl_tcp.service`

```bash
sudo tee /etc/systemd/system/rtl_tcp.service >/dev/null <<'EOF'
[Unit]
Description=rtl_tcp — RTL-SDR sample server for skyward
After=network.target

[Service]
# Bind to localhost only; skyward is the only client and reaches it at 127.0.0.1.
ExecStart=/usr/bin/rtl_tcp -a 127.0.0.1 -p 1234
User=skyward
Group=plugdev
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
```

### `skyward.service`

```bash
sudo tee /etc/systemd/system/skyward.service >/dev/null <<'EOF'
[Unit]
Description=skyward — ADS-B receiver, API, and web UI
After=rtl_tcp.service
Wants=rtl_tcp.service

[Service]
EnvironmentFile=/etc/skyward.env
ExecStart=/usr/local/bin/skyward run
WorkingDirectory=/var/lib/skyward
User=skyward
Group=skyward
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
```

### Enable and start

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now rtl_tcp.service skyward.service
```

## 5. Verify it is actually receiving

Do not trust "the service is active" — a live stream only proves the server is
up, not that anything is being decoded. Check the real ends:

```bash
# The full self-check, including tuning the radio. Read any [warn]/[fail] lines.
sudo -u skyward SKYWARD_RECEIVER_LAT=45.421 SKYWARD_RECEIVER_LON=-75.697 \
  skyward doctor

# What the running server sees. status "ok" with a rising message count is the goal.
curl -s localhost:8080/healthz | jq
```

`healthz` reports `decode.messages`, `decode.aircraft`, and `decode.positions`.
If `messages` climbs, you are receiving. If the source streams but `messages`
stays at 0, that is signal, not software — see below.

Then open the UI from any machine on the network:

```
http://<pi-ip-or-hostname>:8080
```

Follow the logs while you watch:

```bash
journalctl -u skyward -f
```

## Antenna placement is the whole game

Measured on identical hardware, moving the same antenna through a doorway
changed the message rate by more than 4×. Candidate preambles with **zero
CRC-valid messages** is the signature of a signal that is present but too weak
— almost always placement. Outdoors beats every indoor spot; a window beats an
interior room; never mount the dipole flat against glass (at 1090 MHz a coated
pane detunes it and drops peak signal ~7×). If reception is dead, tune an FM
station around 98.5 MHz as a control: if FM is fine but 1090 is silent, it is
the antenna or its geometry, not the cable. The
[README antenna table](../README.md#antenna-because-it-dominates-everything)
and [Part VI of the study guide](GUIDE.md) go into the why.

## Updating a deployed Pi

```bash
cd skyward && git pull
cd client && npm ci && npm run build && cd ..
cargo build --release -p adsb-server
sudo install -m 0755 target/release/skyward /usr/local/bin/skyward
sudo systemctl restart skyward
```

Because the client is embedded, this replaces server and UI together — there is
no way to leave a stale client in front of a fresh server. `skyward --version`
and `skyward doctor` (`web.client … files … embedded`) both describe what is
actually inside the running binary.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `usb_claim_interface error -3` | DVB driver or another process holds the dongle | Blacklist step in §1; `sudo systemctl stop skyward rtl_tcp` and check nothing else (`dump1090`) is running |
| `rtl_test`: no devices found | Blacklist not applied, or power/cable | Reboot after writing the blacklist file; try a powered hub — the Pi's USB can brown out a dongle |
| skyward exits: "gain … not offered" | `SKYWARD_GAIN_DB` is not a discrete tuner value | Use one from the list the error prints (49.6 is the R820T max) |
| skyward refuses to start, mentions position | No receiver lat/lon | Set `SKYWARD_RECEIVER_LAT/LON` in `/etc/skyward.env` |
| Source `streaming` but `messages` stays 0 | Weak signal | Antenna placement (above), not software |
| UI unreachable from another machine | Bound to localhost, or firewall | `SKYWARD_BIND=0.0.0.0:8080`; open the port if a firewall is active |
| `doctor` flags the clock | NTP not synced (no RTC on a Pi) | `sudo timedatectl set-ntp true` |
| Browser shows a "client not built" placeholder | `npm run build` was skipped before `cargo build` | Build the client, rebuild the server, reinstall |
```
