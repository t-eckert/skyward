# The `skyward` command

One binary, seven subcommands. Deliberately one rather than the decoder/API pair an earlier design used: two processes means two ways to deploy the wrong version, two units to supervise, two places for a config mismatch to hide, and — worst — a health endpoint that cheerfully reports `ok` while the decoder has been dead for hours.

```
skyward [GLOBAL OPTIONS] <COMMAND>

  run          Decode continuously and serve the API
  doctor       Check everything that could be wrong, and say so in plain language
  bench        Score one or more fixtures and write a run record
  config       Print the resolved configuration and where each value came from
  devices      List the RTL-SDR dongles on the USB bus
  list-impls   List every registered pipeline implementation
  decode       Decode Mode S messages given as hex
```

`list-impls`, `decode` and `devices` run **before** configuration is resolved, so a broken config file cannot stop you inspecting the build.

## Global options

Available on every subcommand. Each overrides the config file and the environment; see [CONFIGURATION.md](CONFIGURATION.md) for the full precedence.

| Flag | Meaning |
|---|---|
| `--config PATH` | Configuration file. A named path that does not exist is an error |
| `--source SPEC` | `file:PATH`, `tcp:HOST:PORT`, or `usb[:INDEX]` |
| `--sample-rate-hz N` | Samples per second. Below 2 MS/s is an error |
| `--gain-db V` | Decibels, or `auto`. Must be a step the tuner offers |
| `--bind HOST:PORT` | Where the API listens |
| `--db-path PATH` | SQLite file for history |
| `--log-format text\|json` | `json` for journald |
| `--impl-set NAME` | Pipeline preset |
| `--mag`, `--detect`, `--slice`, `--validate` | Override one stage on top of the preset |

`--version` prints the version; the same string appears in `/healthz` and in `doctor`'s `build.version` line, so one command describes the interface as well as the decoder.

---

## `skyward run`

Decode continuously and serve the API and web interface. This is what the systemd unit runs, with no arguments.

| Flag | Meaning |
|---|---|
| `--api-only` | Serve the API but write nothing to the database |
| `--loop-file` | Restart a file source when it runs out |
| `--fast` | Replay a file as fast as possible instead of at recorded speed |
| `--duration-s N` | Stop after N seconds. Mostly for smoke tests |

`--api-only --loop-file` against a fixture gives the SvelteKit client a live-looking backend with no radio attached, which is how the interface is developed:

```bash
skyward run --source file:fixtures/raw/golden.cu8 --loop-file --api-only
```

Exit codes: `0` on a clean shutdown, `2` for a configuration error or a source that could not be opened.

It will not exit on a transient error. `rtl_tcp` restarting, a USB bus glitch, a network blink — all of those are counted, backed off, and **reconnected**, because a receiver that quietly died six hours ago is the worst outcome available.

## `skyward doctor`

The command for a machine you cannot log into. Every check ends in a sentence someone can act on: `clip_pct = 3.2` is a measurement, "gain too high, reduce to about 44 dB and re-run" is a diagnosis, and only the second is useful at arm's length.

| Flag | Meaning |
|---|---|
| `--offline` | Skip anything that touches the radio |
| `--capture-seconds N` | Seconds of RF for the signal checks (default 2) |
| `--json` | Machine-readable, for pasting into an issue |

| Exit | Meaning |
|---|---|
| `0` | Healthy |
| `1` | Warnings — it will run, but read them |
| `2` | Something is broken |

```console
$ skyward doctor
skyward doctor

  [ok  ] build.version                skyward 0.1.0, rustc target aarch64
  [ok  ] build.pipeline               baseline = mag=naive detect=naive slice=naive validate=crc-only

  [ok  ] config.file                  none; defaults, environment and flags only
  [ok  ] web.client                   21 files, 1642 KiB embedded
  [ok  ] config.source                usb  $SKYWARD_SOURCE
  ...
  [ok  ] station.position             45.4210, -75.6970 at 70 m  ($SKYWARD_RECEIVER_LAT)
  [ok  ] usb.devices                  0: Generic RTL2832U OEM (SN 00000001)
  [ok  ] clock.wall                   1787272430 epoch seconds, plausible
  [ok  ] host.power                   no undervoltage or throttling since boot (0x0)
  [ok  ] host.temperature             48.3 C
  [ok  ] storage.writable             /var/lib/skyward is writable
  [ok  ] selftest.decode              5/5 synthetic frames recovered in 0.4 ms
  [ok  ] radio.rate                   2.400 MS/s effective, matching the request
  [ok  ] radio.decode                 879 candidates, 517 valid in 2.0 s (172 msg/min)
```

The checks that exist because of a specific failure:

- **`clock.wall`.** A Pi has no real-time clock and boots in 1970. Every timestamp is then garbage, `?max_age=` filters return nothing, and the whole system looks like a dead radio. It is almost always the last thing anyone suspects.
- **`host.power` / `host.temperature`** (Linux only). An undervolted or heat-soaked Pi throttles rather than crashing, cannot keep up with 2.4 MS/s, and drops samples. Every visible symptom points at the antenna. The firmware has known all along.
- **`selftest.decode`.** Decodes synthetic frames in memory with no antenna attached, which separates "bad build" from "bad reception" before anyone goes climbing after the antenna.
- **`radio.rate`.** Dropping samples and hearing nothing look *identical* in a message count, and one of them is software.
- **`radio.decode` with candidates but no CRCs.** That is the signature of a sample-rate mismatch, not a weak signal, and the message says so.
- **`station.shadowed`.** A position set at runtime is persisted and outranks the config file. This fires when the two disagree — the "I edited the config and nothing happened" case.
- **`web.client`.** The interface is compiled in, so whether it is really there is a property of *this binary*. The alternative way to find out is to open a browser, which on a Pi you are deliberately not doing.

## `skyward devices`

Lists the RTL-SDR dongles on the USB bus. Needs a binary built with `--features usb`; without it, it says so rather than printing an empty list — "no devices" and "this binary cannot see devices" are different answers, and only one of them means go and unplug something.

```console
$ skyward devices
1 RTL-SDR device(s):

  usb:0
    name          Generic RTL2832U OEM
    manufacturer  Realtek
    product       RTL2838UHIDIR
    serial        00000001

Use one with `--source usb:INDEX`, or set SKYWARD_SOURCE=usb:INDEX.
```

It reads descriptors **without opening the device**, so it still answers while `rtl_tcp` or another skyward holds the dongle. That difference is diagnostic: listed but unopenable is a permissions problem, not a missing device.

Exit `0` with devices, `1` with none (and a checklist), `2` without the feature.

## `skyward config`

Print every resolved value with where it came from.

```console
$ skyward config
skyward configuration
  (.env: /home/pi/skyward/.env)

  source                 usb                  $SKYWARD_SOURCE
  sample_rate_hz         2400000              default
  gain_db                44.5                 command line
  receiver.lat           45.421               skyward-station.toml
```

This is the command that answers "did my edit take effect".

## `skyward bench`

Score one or more fixtures and write a run record. The scoreboard for pipeline work.

| Flag | Meaning |
|---|---|
| `--out PATH` | Write the run record as JSON |
| `--compare PATH` | Compare against a previous record and fail on regression |
| `--verbose` | Print each decoded message |

```console
$ skyward bench
fixtures/raw/golden.cu8
  180.0 s at 2.400 MS/s
  messages     517   unique     517   aircraft    7   positions    72
  candidates   879   yield   58.8%   cand/msg   1.7   172.3 msg/min
  guards: ghosts 0.000   realtime 401.3x   1.0 ns/sample
  digest fnv1a64:c3ed11b99d82a415
```

The **digest** is the point: a stable hash of the decoded messages. Two runs with the same digest decoded exactly the same thing, so a refactor that claims to be a refactor is checkable.

**`realtime`** is the number that tells you whether a machine can keep up. On a Pi, run this before believing anything else about throughput.

**`ghosts`** is the rate of aircraft that appeared and never confirmed — decoded noise. It must not climb.

**Yield is reported, never optimised.** A better detector proposes more marginal candidates, so yield falls as messages rise.

Comparing runs to catch a regression:

```bash
skyward bench --out runs/before.json
# ... change the detector ...
skyward bench --detect correlator-v2 --compare runs/before.json
```

Note that `bench` scores fixtures against **the operator's** configured receiver position, not the capture's sidecar. An Ottawa config against a Pennsylvania capture drops the positions count hard with an identical message count and digest — which looks like a decode regression and is not one.

## `skyward list-impls`

```console
$ skyward list-impls
magnitude (--mag):
  naive            sqrt(i^2+q^2) in f32; the correctness reference
detector  (--detect):
  naive            half-microsecond slot means; min(pulse) > 2x max(silence), absolute threshold
slicer    (--slice):
  naive            one sample per half-bit, compared directly; reports no confidence
validator (--validate):
  crc-only         accept only an exact CRC match; cannot invent aircraft
presets   (--impl-set):
  baseline
```

A new implementation lands beside the old one and is selected by name, so the comparison you usually want — your third attempt against your second — is a flag, not a branch. An unknown name at any stage is a startup error listing what exists, checked before the radio is opened: "it ran but quietly used something else" is the failure that wastes an evening on a machine you cannot debug interactively.

## `skyward decode`

Decode Mode S messages given as hex. Useful for poking at a capture, and for checking a decoder against a message from anywhere else.

```console
$ skyward decode 8D4840D6202CC371C32CE0576098
8D4840D6202CC371C32CE0576098
  DF17  Extended squitter (ADS-B)
  ICAO  4840D6
  CRC   valid
  TC4
  Identification { callsign: "KLM1023", category: ... }
```

Exit `1` if any message failed to parse, `0` otherwise.

For downlink formats where the address is XORed into the parity, the CRC line says so rather than reporting a failure — the remainder *is* the address, and calling that a failure would be wrong.
