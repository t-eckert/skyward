# skyward documentation

An ADS-B receiver: RF at 1090 MHz in, aircraft on a map out, on hardware you
can leave in a window.

## Start here

| If you want to… | Read |
|---|---|
| Run it on your laptop in five minutes | [Quick start](#quick-start), below |
| Deploy it on a Raspberry Pi | [RASPBERRY_PI.md](RASPBERRY_PI.md) |
| Fix something that is misbehaving | [OPERATIONS.md](OPERATIONS.md) |
| Understand how the pieces fit | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Understand the *radio* — why any of this works | [GUIDE.md](GUIDE.md) |
| Look up a setting | [CONFIGURATION.md](CONFIGURATION.md) |
| Look up a command | [CLI.md](CLI.md) |
| Look up an endpoint | [API.md](API.md) |
| Work on the code | [DEVELOPMENT.md](DEVELOPMENT.md) |
| Know what a Pi deployment has and has not been checked for | [PI_AUDIT.md](PI_AUDIT.md) |

[GUIDE.md](GUIDE.md) is the long one and the one worth your time: 1600 lines
from why aircraft transmit at all, through the RTL-SDR's front end, the Mode S
frame format, and CPR, to a file-by-file walkthrough of this repository and a
reading list. The reference documents above assume you have skimmed it or do
not need it.

---

## Quick start

No radio required — the repository ships three real captures.

```bash
git clone https://github.com/t-eckert/skyward.git && cd skyward

cd client && npm ci && npm run build && cd ..
cargo build --release

./target/release/skyward run \
  --source file:fixtures/raw/golden.cu8 --loop-file --api-only
```

Open <http://localhost:8080>. Aircraft over Ottawa, decoded from three minutes
of recorded RF on a loop. `--api-only` keeps it out of the database.

Set the receiver position from the header, or leave it unset and watch the
range gate stay disabled — either is a fine way to see what the setting does.

### With a real dongle

```bash
# macOS
brew install librtlsdr
# Debian / Raspberry Pi OS
sudo apt install librtlsdr-dev

cargo build --release --features usb
./target/release/skyward devices
./target/release/skyward run --source usb
```

Without the `usb` feature, run `rtl_tcp -a 127.0.0.1 -p 1234` separately and
use `--source tcp:127.0.0.1:1234`. skyward sets frequency, sample rate and gain
itself over the control protocol, so `rtl_tcp` needs no flags.

### If nothing decodes

Two minutes of traffic is normal even indoors, but reception is
antenna-dominated to a degree that is hard to believe until you have measured
it — the same antenna moved through a doorway changed the message rate more
than fourfold. Candidate preambles with zero valid CRCs means the signal is
present and too weak.

```bash
./target/release/skyward doctor
```

Every line ends in something to do. [OPERATIONS.md](OPERATIONS.md) covers the
rest.

---

## What this is, and is not

**Is:** a Mode S / ADS-B receiver written to be understood and modified. The
decode chain is five swappable stages selected by name, there is a benchmark
with a stable digest so a change is checkable, and there is a `doctor` command
written for a machine you cannot log into.

**Is not:** the fastest decoder available — `dump1090` is mature C and this is
not trying to beat it. It has no MLAT, no uplink, and no authentication.

The [study guide](GUIDE.md) explains the domain; the [architecture
notes](ARCHITECTURE.md) explain the shape; both spend most of their length on
*why*, because the decisions are the part worth keeping.

## Conventions

Two run through the whole codebase and are worth knowing before reading any of
it.

**A green light is not evidence.** Health is freshness, not liveness. A
configuration dump prints where every value came from, not just what it is. The
benchmark digests its output so a refactor can be proved to be one. Every one
of these exists because the obvious version reported success through a real
failure.

**Errors say what to do.** `SourceError` is a taxonomy — `Config` means fail
loudly and never retry, `Transient` means reconnect and carry on — because on a
machine you cannot log into, *how* something failed decides what should happen
next. `doctor` reports diagnoses, not measurements.
