# Field runbook

For running skyward somewhere that is not home — a talk, a demo, someone else's house — where you have limited time and no second chance.

## The topology this is written for

**The receiver is remote and unattended.** It sits by a window on another floor, on the venue's Ethernet, with nobody next to it. You are somewhere else in the building — on stage — reaching it across the venue's network.

Everything below follows from one consequence of that: **once you walk away from the Pi, you cannot fix it.** Not with a keyboard, not with a cable, not by power-cycling it. Every decision on the day should be made in the order "can I still reach this thing if I am wrong", and the last thing you do before leaving it is verify from where you will actually be standing.

The good news, and it is real: a seventh-floor window is a genuinely good antenna position. Expect far better reception than anything measured at a desk indoors — the repo's own figure is that placement moves the message rate by more than 4x, and this is the largest placement change this receiver will have had.

Written from a real first deployment, in which nearly every wrong turn came from one of the traps below rather than from anything exotic.

---

## The one rule

**Never compare two measurements taken at different times.**

Air traffic varies by more than the thing you are trying to measure. Measured on this receiver in a single hour, in one fixed location with the antenna untouched:

| when | candidates/s | messages/min |
|---|---|---|
| quiet stretch | 0.023 | 0.40 |
| busy stretch, ~1 h later | 1.533 | 34.20 |
| same hardware, ~20 min after that | 0.293 | 9.39 |

The same receiver, the same antenna, the same spot, nothing touched: a **66× swing from nothing but the sky**, and then a 5× swing back. Any A/B run across a twenty-minute gap is measuring aircraft schedules, not your change.

During the first deployment this trap produced two confident, contradictory diagnoses in a single afternoon — first "the antenna is broken", then "the Pi is broken" — and neither survived a measurement taken at the right time. Both were argued from real numbers. The numbers were just answering a different question than the one being asked.

If you must compare, change one thing and measure **back to back**, minutes apart, ideally alternating A-B-A. If you cannot do that, do not draw a conclusion — say the measurement was inconclusive and move on. That is a real result; a confident wrong answer is not.

**One number that is not traffic-dependent:** `clip` from `skyward doctor --source usb`. It measures how much RF energy reaches the ADC on every sample, whether or not anything decodes, so it says something about the front end even in an empty sky. Low candidates *and* zero clip points at the signal path; low candidates with healthy clip usually just means nothing is up there.

**And one that is diagnostic in a different way:** yield. A more sensitive receiver proposes more marginal candidates, so yield *falls* as messages rise. Two setups where the better one shows the *lower* yield are probably genuinely different in sensitivity, not just sampling different traffic.

The corollary: **an empty map proves nothing.** Check a live flight tracker for what is actually overhead before deciding the receiver is broken.

---

## Before you leave

```bash
task deploy:drift          # installed config still matches the repo
task pi:power              # want throttled=0x0
task pi:health             # want status ok, overrun 0
```

Pack, and check off:

- [ ] **The good power supply.** 5.1 V / 3 A, and the short thick cable. Not a phone charger. This is the single most disruptive thing to get wrong and the hardest to diagnose in the room.
- [ ] The dongle, the antenna, **and its coax**.
- [ ] A USB extension cable if you have one — it buys distance between the dongle and the Pi's own radio noise.
- [ ] **Ethernet cables, at least two.** One Pi-to-network, one Pi-to-laptop for the direct fallback.
- [ ] **A small unmanaged switch.** Lets the Pi and the laptop share a single venue drop, and gives you a private LAN if that drop turns out to be dead.
- [ ] **The USB-C to Ethernet adapter.** The laptop has no built-in Ethernet, so without this the cables and the switch buy nothing. Easy to leave behind precisely because it lives in a bag rather than in the machine — and when it is not plugged in, macOS still lists stale `Ethernet Adapter` interfaces showing `status: inactive`, so the laptop *looks* like it has Ethernet when it does not. Check `ifconfig` shows `status: active` before you count on it.
- [ ] Know the console password (1Password: `skyward-pi-console`). You will need it if the network is the thing that is broken.

---

## Ahead of the day, with venue IT

The receiver has to get itself onto the network with **nobody standing next to it and no browser**. That rules out anything interactive, so ask before you arrive:

- **Does the wired network use a captive portal?** If yes, the Pi cannot clear it and will be unreachable from the moment you leave the floor. This is the single failure that ends the demo, and the only one you can prevent with an email.
- **Does it need MAC registration or 802.1X?** If registration is possible, give them these now:

  | | |
  |---|---|
  | hostname | `skyward` |
  | eth0 MAC | `e4:5f:01:f9:f3:44` |
  | wlan0 MAC | `e4:5f:01:f9:f3:46` |

- **Is outbound UDP allowed?** Tailscale prefers UDP 41641 and falls back to relaying over 443, so it usually survives a restrictive network — but "usually" is doing work in that sentence, and it is worth knowing which you are on.

---

## The setup meeting, the day before

This is where the work happens. Everything below is something you cannot learn at home, and the whole point of being there a day early is that a failure found now is a problem with a night to solve it.

Work through the arrival sequence below in full. Then do these four things, which only make sense at the venue:

**1. Prove it comes back unattended.**

The most valuable test available to you. Pull its power, put it back, and walk away for five minutes.

```bash
# from your laptop, after plugging it back in
curl -fsS https://skyward.feist-gondola.ts.net/healthz
```

If it rejoins the tailnet on its own, you know it survives the overnight power blip, the cleaner, and the breaker. If it does not, you have found the single failure that would end the demo — with a night to fix it. This has been tested at home and passed; it has not been tested on *their* network, which is the part that matters.

**2. Find out how the window position actually performs.**

Leave it decoding for as long as you are there and take a real measurement, not a glance.

```bash
task pi:stats     # note candidates and crc_ok, wait 10 min, note again
```

This is the number you have never been able to get: reception from a good position. Everything measured so far has been a desk indoors.

**3. Settle the power and the network drop physically.**

Which outlet, is it switched, is it on a timer, will anyone reclaim it. Same for the Ethernet port — confirm the drop is live and stays live, not just that it worked once.

**4. Decide whether it stays overnight.**

**If it can stay, leave it running.** That converts the night into a free unattended soak test on the real network, and you can check it from your hotel:

```bash
curl -fsS https://skyward.feist-gondola.ts.net/healthz
task pi:power PI_HOST=skyward.feist-gondola.ts.net
```

A receiver that has been up all night on their network, seen from off-site, is about as much confidence as you can buy. It also means the day-of work is a five-minute check rather than a setup.

If it comes back with you, plan on repeating the whole arrival sequence on the day, and budget the time.

---

## On the day, if it stayed overnight

```bash
curl -fsS https://skyward.feist-gondola.ts.net/healthz   # status ok, source streaming
task pi:power                                            # throttled=0x0
task pi:stats                                            # messages still climbing
```

Then check it once more **from the stage**, and start the local fixture playback in a second window before you go on. That is the whole day-of routine.

---

## Arrival sequence

Used in full at the setup meeting, and again on the day if the Pi did not stay.

The order matters because **each step can strand you**, and the whole sequence is designed so you discover that while you can still walk over and touch it.

**1. Place it, then power it.**

At the window. Antenna vertical, as high as the sill allows, near the glass but not flat against it.

Use an outlet that is not switched and not likely to be reclaimed. A receiver unplugged by cleaning staff looks exactly like a receiver that crashed, and you will not be able to tell which from the stage.

```bash
vcgencmd get_throttled        # 0x0, or fix the power before anything else
```

**2. Get it on the network — and prove it has internet, not just a link.**

A link light means nothing. The Pi can have an address, a route, and no way out.

```bash
ip -brief addr show eth0      # an address at all
ping -c1 1.1.1.1              # is there internet
curl -sI http://example.com | head -1    # 200, or a redirect = captive portal
```

A redirect here is the captive portal, and it is the end of the remote plan unless venue IT can exempt the Pi.

**3. Bring up the tailnet and confirm it from your own machine.**

```bash
tailscale status              # on the Pi
curl -fsS https://skyward.feist-gondola.ts.net/healthz   # from your laptop
```

**4. Set the station to Montreal.**

In the web interface. Takes effect in under a second, persists, no restart, nothing dropped. Three decimals.

Not `/etc/skyward.env` — the runtime overlay outranks it, so editing the file appears to do nothing.

**5. Watch it actually receive, from the window.**

```bash
task pi:stats
```

Give it a few minutes. A seventh-floor window should be dramatically better than anything this receiver has seen; if it is not, that is the moment to move the antenna, while you are still next to it.

**6. The step people skip: go to the stage and check again.**

```bash
curl -fsS https://skyward.feist-gondola.ts.net/healthz
```

**From where you will be standing, on the network you will actually be using.** A different floor may be a different VLAN, a different SSID, or a client-isolated guest network. Tailscale relays around isolation, but you want to have *seen* it work from there, not assumed it.

Only after this do you leave the Pi alone.

---

## Symptom → cause

| What you see | Check this first |
|---|---|
| No aircraft at all | A flight tracker. Is anything actually overhead? |
| Few candidates, few messages | Antenna placement, then power |
| **Candidates but no CRC-valid messages** | Signal present but too weak — placement |
| Good candidates, message rate falling | `vcgencmd get_throttled`, then `overrun_bytes` |
| `usb_claim_interface error -6` | The DVB driver has the dongle: `lsmod \| grep dvb` |
| Service `active`, nothing arriving | `/healthz` — it reports freshness, not liveness |
| Timestamps nonsense | No RTC. `timedatectl` — did NTP ever run? |
| Config edit did nothing | The runtime station overlay, or a drop-in outranking yours |
| Web UI unreachable | Tailscale, not skyward. `tailscale status` |

---

## The five failures that all look like a bad antenna

This is the heart of it. Every one of these presents as "the message count is low", and only one of them is actually the antenna.

1. **Undervoltage.** The firmware knows and nothing asks it. `vcgencmd get_throttled`.
2. **Thermal throttling.** Same signature. Soft-throttles at 80 °C.
3. **The DVB driver holding the dongle.** Fails at open, not at reception.
4. **The decoder not keeping up.** `overrun_bytes` climbing — a software problem wearing reception's clothes.
5. **Nothing in the sky.** The most common, and the one nobody checks.

Only after all five: the antenna. Vertical, as high as practical, near a window but not flat against the glass, elements ~6.9 cm each.

---

## Config written is not config in effect

Three settings on the first deployment were written correctly and silently did nothing. Assume nothing; ask the system.

| Setting | Why it failed | Ask instead |
|---|---|---|
| `StartLimitIntervalSec` | was in `[Service]`; systemd only reads it in `[Unit]` | `systemctl show skyward -p StartLimitIntervalUSec` |
| `RuntimeWatchdogSec` | `10-` drop-in outranked by Pi OS's `40-` | `systemctl show -p RuntimeWatchdogUSec` |
| `Storage=persistent` | `10-` drop-in outranked by `40-rpi-volatile-storage.conf` | `journalctl --header \| grep "File path"` |

Drop-ins merge in **lexical order across every directory systemd reads**, so a `10-` file loses to a vendor `40-` file. Use `99-`.

---

## If you lose it during the talk

**You cannot fix it.** It is on another floor, there is no keyboard on it, and you are on stage. Every option below is something you can do from where you are standing, in under a minute, or it is not an option.

Decide *now* which of these you would actually do, because the room will be watching.

**Is it the receiver, or the path to it?**

```bash
tailscale ping skyward        # does the tailnet still see it
```

If `tailscale ping` answers but the page does not, the Pi is alive and `tailscale serve` is the problem. If it does not answer, the Pi is off the network — and there is nothing you can do about that from the stage.

**The honest answer is that the recovery is a fallback, not a repair.** Keep the local playback below one keystroke away for the whole talk.

### Things that will not help, so do not try them mid-talk

- SSH to fix it. If Tailscale is down, SSH is down too — same path.
- Waiting for the healthcheck. It restarts skyward after ten minutes, which does not help if the problem is the network.
- Rebooting it. You have no way to.

---

## The fallback: run it locally

This is the one that saves the talk, and it needs to be set up **before you leave home**.

A recorded capture replays through the identical decode chain — same DSP, same tracker, same map, same API. It produces a real, populated, moving display. Nobody watching can tell the difference.

```bash
skyward run --source file:fixtures/raw/golden.cu8
```

Then open `http://localhost:8080`. Verified 2026-09-05 — 25 seconds into playback:

```
candidates 125   crc_ok 81   aircraft 6   positions 17

  C0583A  37,000 ft  544 kt
  C06590  23,450 ft  415 kt
  406A35  35,000 ft  542 kt
```

Six aircraft with real altitudes, speeds and plotted positions, from a file, with no radio attached. That is a better-populated map than the live receiver managed from a desk indoors all afternoon.

**Have this already running, in a second window, before you go on stage.** Switching to a browser tab is a recoverable moment; discovering you need to build something is not.

Also worth having ready, because they need nothing at all:

```bash
skyward doctor --offline      # decodes synthetic frames, no radio needed
skyward decode 8D4840D6202CC371C32CE0576098
```

The last one is a nice thing to be able to do live regardless — it takes a raw hex frame and prints the aircraft out of it, which is the whole project in one line.

### Before you leave home

- [ ] Confirm `fixtures/raw/golden.cu8` is on the **laptop** (it is gitignored, so it is not in any deployment).
- [ ] Run the playback once, end to end, and look at the map.
- [ ] Know the URL and the port.
