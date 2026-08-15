# skyward: a study guide

A first-principles walk through everything in this repository — the physics in
the air, the hardware that turns it into numbers, the protocol those numbers
encode, and every line of code that gets from one to the other.

Written assuming you're comfortable with complex analysis, Fourier transforms,
statistics and finite fields, but have never had to care about the difference
between a superheterodyne and a direct-conversion receiver. Where the physics
is familiar I'll move fast and just name the correspondence; where the
*conventions* are unfamiliar — and radio is mostly conventions with a thin
layer of physics on top — I'll go slowly.

**Part I** is the physics and the hardware. **Part II** is the protocol.
**Part III** is a guided tour of the code with line references. **Part IV** is
the theory you'll need to actually improve the DSP. **Part V** is antennas,
which turned out to matter more than all the software combined. **Part VI** is
where to read more.

---

# Part I — What is actually in the air

## 1.1 Why aircraft are shouting at all

The system you're receiving is an accident of history worth understanding,
because it explains every strange design decision downstream.

During the Second World War, the British fitted aircraft with transponders
that replied to radar interrogations — Identification Friend or Foe. That
became **Secondary Surveillance Radar** (SSR): a ground station transmits an
interrogation on **1030 MHz**, and any aircraft that hears it replies on
**1090 MHz**. "Secondary" distinguishes it from primary radar, which just
bounces energy off metal. Secondary radar asks a question and gets an
*answer*, which is enormously more informative — the aircraft can tell you its
identity and altitude rather than merely existing.

The original modes (A and C) were extremely simple: mode A returns a 4-digit
squawk code, mode C returns pressure altitude. Both use pulse-position replies
with no addressing at all, so in dense airspace every aircraft answers every
interrogation and the replies collide — a pathology called **FRUIT** (False
Replies Unsynchronised In Time) and **garbling**.

**Mode S** ("Select", 1970s–80s, developed largely at MIT Lincoln Laboratory)
fixed this by giving every aircraft a unique 24-bit address so a ground station
can interrogate *one* aircraft. That address is the ICAO number you see all
over this codebase.

Then came the crucial addition. A Mode S transponder can be configured to
transmit **unsolicited** — nobody asked, it just broadcasts. That is a
**squitter**. Extend the message to 112 bits and put position and velocity in
it and you have **ADS-B**: Automatic Dependent Surveillance – Broadcast.

- *Automatic*: no interrogation needed.
- *Dependent*: it depends on the aircraft's own navigation solution (GPS),
  rather than measuring the aircraft independently. This is a genuine
  weakness — an aircraft can lie, and there is no authentication whatsoever.
- *Surveillance – Broadcast*: to anyone listening.

So the reason you can do this with a $30 dongle is that ADS-B was designed for
ground stations and other aircraft, in the clear, with no thought that
receivers would become disposable. It is unencrypted and unauthenticated by
design; the aviation community's position is that the information is not
sensitive and that mandating crypto would have made the rollout impossible.

> **Read more:** Michael Stevens, *Secondary Surveillance Radar* (Artech House,
> 1988) is the standard text. For the Mode S design rationale specifically, see
> V. A. Orlando, "The Mode S Beacon Radar System", *Lincoln Laboratory
> Journal* 2(3), 1989 — freely available as a PDF and remarkably readable.

## 1.2 The pulse

At 1090 MHz, a transponder emits pulses. Physically:

- **Carrier frequency** 1090 MHz ± 1 MHz (the tolerance matters; see §2.4)
- **Pulse width** 0.5 µs
- **Peak power** typically 125 W to 500 W depending on transponder class
  (21–27 dBW). A large airliner is at the top of that range.
- **Modulation** is on-off keying: the carrier is either on at full power or
  off. There is no information in the phase, the frequency, or the amplitude
  beyond "present/absent".

The wavelength is
$$\lambda = \frac{c}{f} = \frac{2.998\times10^8}{1.090\times10^9} = 0.2750\ \text{m}$$

so 27.5 cm. Quarter wave is 6.88 cm, half wave 13.75 cm. Those two numbers are
the entire antenna design, and they appear in `README.md` and in the fixture
sidecars.

## 1.3 The link budget, and why range is what it is

This is worth doing quantitatively, because it explains why your indoor
reception was 47 km and a rooftop would be several hundred.

**Thermal noise.** The receiver's noise floor is set by $kTB$:
$$N = kTB = (1.38\times10^{-23})(290)(2.4\times10^6) = 9.6\times10^{-15}\ \text{W}$$
which is $-110.2$ dBm. The RTL-SDR's noise figure is around 6 dB (it's a mass-
market TV tuner), so the effective noise floor is roughly $-104$ dBm.

**Free-space path loss.** In the far field,
$$\text{FSPL(dB)} = 20\log_{10}(d_\text{km}) + 20\log_{10}(f_\text{MHz}) + 32.44$$

At 1090 MHz and 100 km that's $40 + 60.75 + 32.44 = 133.2$ dB.

**Putting it together** for a 250 W transmitter (54 dBm) at 100 km, with 2 dBi
of antenna gain at each end:
$$P_r = 54 + 2 - 133.2 + 2 = -75.2\ \text{dBm}$$

That is ~29 dB above the noise floor. **100 km is not remotely difficult.**

**So why 47 km?** Because free space is the wrong model indoors. Two things
dominate:

1. **The radio horizon.** Line of sight over a curved earth, with the standard
   4/3-radius refraction correction:
   $$d_\text{km} \approx 4.12\left(\sqrt{h_\text{ant}} + \sqrt{h_\text{ac}}\right)$$
   with heights in metres. An aircraft at FL360 (11,000 m) gives
   $4.12 \times 104.9 = 432$ km before the antenna's own height is counted. So
   the horizon is not the limit either.

2. **Obstruction.** A building is 15–30 dB of attenuation at 1090 MHz, and a
   wall of masonry in the path is far worse. This is the real limit, and it is
   why the measurements in Part V matter more than anything in the code.

The key intuition, which is not obvious coming from optics: at 27.5 cm
wavelength, buildings cast **hard shadows**. Diffraction around an obstacle is
significant when the obstacle is comparable to a wavelength; a 30 m building at
27.5 cm is 100 wavelengths across, so it behaves much more like an opaque
screen than like a knife edge you can bend around. At 98.5 MHz (3 m
wavelength) the same building is only 10 wavelengths and the signal diffracts
around it easily. **That frequency dependence is the single most useful
diagnostic tool in the whole project** — see §5.3.

---

# Part II — From a radio wave to an array of bytes

## 2.1 The problem with real signals

Here is the piece where your physics background pays off immediately, because
the standard SDR explanation is usually worse than the underlying idea.

A narrowband radio signal is
$$s(t) = a(t)\cos\!\big(2\pi f_c t + \phi(t)\big)$$
where $a(t)$ and $\phi(t)$ vary slowly compared to $f_c$. Everything you care
about is in $a$ and $\phi$; the carrier is just a vehicle.

The natural object is the **analytic signal**
$$z(t) = a(t)e^{j\phi(t)}e^{j2\pi f_c t}$$
whose real part is $s(t)$ and whose spectrum has no negative-frequency
component (it's $s$ plus $j$ times its Hilbert transform). Strip the carrier:
$$\tilde{z}(t) = z(t)e^{-j2\pi f_c t} = a(t)e^{j\phi(t)}$$

This $\tilde z$ is the **complex baseband** or **complex envelope**
representation, and it is exactly what an SDR hands you. `I` is
$\Re\tilde z = a\cos\phi$, `Q` is $\Im\tilde z = a\sin\phi$.

**Why you need both.** If you sampled only the real signal after mixing with
$\cos(2\pi f_\text{LO}t)$, you'd get
$\tfrac12[a\cos(2\pi(f_c-f_\text{LO})t + \phi) + a\cos(2\pi(f_c+f_\text{LO})t+\phi)]$,
and after filtering away the sum term you cannot tell $f_c - f_\text{LO} = +\Delta$
from $-\Delta$: the cosine is even. Mixing with both $\cos$ and $\sin$ — i.e.
with $e^{-j2\pi f_\text{LO}t}$ — preserves the sign of the offset. **Negative
frequencies are physically meaningful in complex baseband**, and they mean "below
the local oscillator". Losing them would fold the spectrum in half.

> **Read more:** Richard Lyons, *Understanding Digital Signal Processing*,
> chapter 8 ("Quadrature Signals") is the clearest treatment I know and is
> written for exactly this gap. For the analytic-signal formalism, any
> communications text: Proakis & Salehi, *Digital Communications*, §2.1.

## 2.2 What the RTL-SDR actually does

The dongle is two chips, and the fact that it works at all is an accident.

**R820T / R820T2 (the tuner).** A silicon tuner covering 24–1766 MHz. It
contains an LNA, a mixer driven by a synthesised local oscillator, IF
filtering, and a variable-gain amplifier. The **29 discrete gain steps** you
see referenced in `crates/adsb-source/src/lib.rs:87-94` are the tuner's VGA
settings; it is not a continuous dial, and requesting an unlisted value
silently snaps to a different one. That is why the config validates gain
against the tuner's actual table rather than accepting any float.

**RTL2832U (the demodulator).** Designed to demodulate European DVB-T
television. It has an 8-bit ADC running at 28.8 MHz, and — critically — a
debug mode that dumps raw I/Q over USB instead of decoded television. That
mode is the entire reason this hobby exists. Someone found it in 2012.

Three consequences you will meet in the code:

**Sample rates are fractions of 28.8 MHz.** 2.4 MS/s is exactly 28.8/12.
2.048 MS/s is *not* an exact divisor, so asking for it gets you something
nearby. This is why `IqSource::sample_rate()` is documented at
`crates/adsb-source/src/lib.rs:52-59` as "the rate actually in effect, not
necessarily the one you asked for". Feeding the demodulator the requested rate
while the hardware runs at another is the classic cause of "plenty of
preambles, zero valid CRCs" — the bit clock walks off.

**8 bits is not much.** Dynamic range for an ideal $N$-bit converter is
$6.02N + 1.76 = 49.9$ dB. In practice you get less. This is why gain setting
matters so much: too low and weak aircraft fall below one LSB, too high and
strong ones clip and intermodulate. There is no AGC that can fix both
simultaneously when aircraft vary by 40 dB in received power.

**Offset binary, centred on 127.5.** Samples are `u8` where 127.5 is zero
signal. Note that 127.5 is *not representable* — the quantisation floor is
half an LSB on each axis, giving a minimum magnitude of $0.5\sqrt2$. That is
asserted directly in `crates/adsb-dsp/src/synth.rs:265-272`, and it is why a
perfectly silent channel still reads 257 in our `u16` magnitude units rather
than 0.

> **Read more:** the osmocom rtl-sdr wiki (`osmocom.org/projects/rtl-sdr/wiki`)
> is the practical reference. The RTL2832U has no public datasheet; the R820T
> one circulates unofficially. For the ADC theory, Walt Kester (ed.), *Data
> Conversion Handbook* (Analog Devices), chapter 2.

## 2.3 Sampling a complex signal

A real signal sampled at $f_s$ gives you $f_s/2$ of bandwidth. A *complex*
signal sampled at $f_s$ gives you the full $f_s$, from $-f_s/2$ to $+f_s/2$,
because each sample carries two real numbers. So 2.4 MS/s complex is 2.4 MHz of
spectrum centred on the tuned frequency.

Is that enough for a 0.5 µs pulse? A rectangular pulse of width $\tau$ has
spectrum $\tau\,\mathrm{sinc}(f\tau)$, first null at $1/\tau = 2$ MHz. So the
main lobe is 4 MHz wide (±2 MHz) and we are capturing rather less than all of
it — we keep ±1.2 MHz. This rounds the pulse edges but preserves the energy and
the timing, which is all OOK needs. Going to 8 MS/s would sharpen edges and
help with overlapping signals; it would also quadruple the CPU load on a Pi.

The other constraint is timing, not bandwidth: we need enough samples per
microsecond to tell which *half* of a bit period the pulse occupied. Two
samples per microsecond is the bare minimum. At 2.4 MS/s we get 2.4, which is
awkward in a way that has consequences all through `slice.rs` — see §3.3.5.

## 2.4 Frequency offset, and why we get away with ignoring it

The transponder is specified at 1090 MHz ±1 MHz. The dongle's crystal is
typically ±30 ppm, which at 1090 MHz is ±33 kHz. So there is always a residual
carrier offset $\Delta f$ after downconversion, and the complex baseband signal
is really $a(t)e^{j\phi(t)}e^{j2\pi\Delta f t}$.

For a phase-modulated scheme this would be fatal and you'd need a
carrier-recovery loop. For OOK it is completely irrelevant, because
$$\left|a e^{j\phi}e^{j2\pi\Delta ft}\right| = |a|$$

**Taking the magnitude annihilates every phase error at once**: carrier offset,
crystal drift, Doppler (±1.2 kHz for a 600 kt aircraft), and the unknown
initial phase. This is the deep reason ADS-B is a good first SDR project —
you get to skip the entire subject of synchronisation.

The synthetic generator deliberately injects a carrier offset to prove this:
`crates/adsb-dsp/src/synth.rs:38-40` documents `phase_step` as "magnitude
demodulation should be completely indifferent to this", and
`crates/adsb-dsp/src/magnitude.rs:115-124` tests rotation invariance directly.

The price is real and is discussed in §4.2: throwing away phase costs you
sensitivity, because non-coherent integration is less efficient than coherent.

---

# Part III — The protocol

## 3.1 Pulse-position modulation

Mode S downlink data is PPM at 1 Mbit/s. Each bit occupies exactly 1 µs and
contains exactly one 0.5 µs pulse:

```
   bit = 1     ██__        pulse in the first half
   bit = 0     __██        pulse in the second half
```

Three properties fall out, and each shapes the code:

**The clock recovers itself.** Every bit has a transition, so there is no run
of identical symbols to drift through. No PLL, no scrambler, no line coding.
Contrast NRZ, which needs a scrambler precisely to guarantee transitions.

**Every bit has constant energy.** A message is a constant-power burst
regardless of content, which makes AGC and threshold-setting behave.

**It is spectrally wasteful.** You spend 1 µs of channel time to send 1 bit,
where a 2-level scheme could send 1 bit per symbol at half the bandwidth. Mode
S trades that away for robustness — and note the *uplink* at 1030 MHz uses
DPSK at 4 Mbit/s, so the asymmetry is deliberate: ground stations can afford
sophisticated receivers, transponders must be cheap and reliable.

## 3.2 The preamble

Every transmission opens with the same 8 µs pattern: 0.5 µs pulses beginning
at 0, 1.0, 3.5 and 4.5 µs, silence everywhere else.

```
 µs  0    1    2    3    4    5    6    7    8
     ██   ██             ██   ██
     |    |              |    |
     0   1.0            3.5  4.5
```

Those spacings are not arbitrary. The pattern has **low autocorrelation with
shifted copies of itself**: slide it by any non-zero multiple of 0.5 µs and at
most one pulse lines up. Two consequences:

- Random noise rarely fakes it, because you need four coincidences at
  specified spacings.
- A *second* overlapping message is unlikely to look like a valid start,
  which matters because in dense airspace overlapping replies are the norm.

This is the same design principle as a Barker code in radar, and for the same
reason: you want a correlation function with a sharp peak and low sidelobes.

The code encodes these positions in `crates/adsb-dsp/src/detect.rs:125-143`,
where `NaiveDetector::new` precomputes the sample ranges of four pulse slots
and twelve silence slots. Note the comment at `detect.rs:127-131` about slots being
**start-anchored**: a Mode S pulse *begins* at its nominal time and runs
0.5 µs. Centring a window on the nominal time straddles the pulse edge and
finds nothing. I made exactly that mistake on the first evening and it cost an
hour; the note is there so you don't repeat it.

## 3.3 Frame structure

After the preamble comes 56 or 112 µs of data. Which one is determined by the
first five bits, the **downlink format** (DF):

| DF | Bits | Name | CRC checkable? |
|---|---|---|---|
| 0 | 56 | Short air-air surveillance (ACAS) | no — address overlaid |
| 4 | 56 | Surveillance, altitude reply | no |
| 5 | 56 | Surveillance, identity reply | no |
| 11 | 56 | All-call reply | partly |
| 16 | 112 | Long air-air surveillance | no |
| **17** | **112** | **ADS-B extended squitter** | **yes** |
| 18 | 112 | Extended squitter, non-transponder (TIS-B) | yes |
| 20/21 | 112 | Comm-B altitude / identity | no |

The length rule is simply `df >= 16`, implemented at
`crates/adsb-core/src/frame.rs:78-80`. That is not a coincidence: the format
field's top bit *is* the length bit, so a receiver knows how long the burst
will be after 5 µs of data.

For DF17, the 112 bits are:

```
 bits    0-4      5-7      8-31           32-87              88-111
       ┌──────┬────────┬──────────────┬──────────────────┬──────────────┐
       │  DF  │   CA   │ ICAO address │  ME payload (56) │ parity (24)  │
       └──────┴────────┴──────────────┴──────────────────┴──────────────┘
```

and the ME payload's first five bits are the **type code** which selects
everything else:

| TC | Meaning |
|---|---|
| 1–4 | Aircraft identification (callsign) |
| 5–8 | Surface position |
| 9–18 | Airborne position, barometric altitude |
| 19 | Airborne velocity |
| 20–22 | Airborne position, GNSS height |
| 28 | Aircraft status |
| 29 | Target state and status |
| 31 | Operational status |

The dispatch is `crates/adsb-core/src/decode.rs:54-66`.

> **Read more, and this is the authoritative list:**
> - **ICAO Annex 10, Volume IV** — *Aeronautical Telecommunications:
>   Surveillance and Collision Avoidance Systems*. The treaty-level spec. Not
>   free, but the definitive source for waveform and timing.
> - **ICAO Doc 9871** — *Technical Provisions for Mode S Services and Extended
>   Squitter*. This is the one you actually want: it contains the register
>   definitions, the CPR algorithm, and the type-code tables in full.
> - **RTCA DO-260B** / **EUROCAE ED-102A** — the ADS-B Minimum Operational
>   Performance Standards. What manufacturers certify against.
> - **Junzi Sun, *The 1090 Megahertz Riddle*** (2nd ed., free at
>   `mode-s.org/decode`) — an open-access book that covers essentially
>   everything in this document's Part III with worked examples. If you read
>   one thing, read this. The canonical test vectors in our tests come from it.

---

# Part IV — The code

## 4.0 Layout and dependency direction

```
adsb-core     pure logic, no I/O            ← everything depends on this
adsb-dsp      samples → validated frames    ← depends on core
adsb-source   where samples come from       ← depends on nothing but std
adsb-track    frames → aircraft             ← depends on core
adsb-store    aircraft → SQLite             ← depends on core, track
adsb-server   the binary                    ← depends on all
client/       the web interface             ← compiled *into* adsb-server
```

The dependency direction is deliberate: `adsb-core` and `adsb-dsp` have no I/O
at all, so `cargo test -p adsb-core -p adsb-dsp` runs in about 70 ms and you
will actually run it. Anything that can behave differently on the Pi than on
the Mac is pushed out to the edges.

## 4.1 `adsb-core` — bytes into meaning

### 4.1.1 `bits.rs` — extraction

103 lines, and the only interesting thing about it is a convention decision.

ICAO documents number message bits **1 through 112**. This module is
**0-based**, stated at `crates/adsb-core/src/bits.rs:3-6`. Mixing the two
conventions in one codebase is how off-by-one bugs get in, so every call site
that quotes a spec bit number subtracts one and says so — see the comment at
`decode.rs:3-5`.

`get()` at `bits.rs:14` extracts up to 32 bits MSB-first and **panics** on
overrun rather than truncating (`bits.rs:16-24`). That's deliberate: frame
lengths are validated before decoding, so a slice overrun means the field table
is wrong, which is a programming error and should be loud. The test at
`bits.rs:97-101` pins that behaviour.

### 4.1.2 `crc.rs` — the only thing standing between you and garbage

This is where a finite-fields background makes everything click at once.

**The code as an algebraic object.** Treat a 112-bit message as a polynomial
over $\mathrm{GF}(2)$:
$$M(x) = m_{111}x^{111} + m_{110}x^{110} + \cdots + m_0$$
The Mode S generator is
$$G(x) = x^{24} + x^{23} + x^{22} + \cdots + x^{12} + x^{10} + x^{3} + 1$$
which is `0xFFF409` in the compact 24-bit representation (the $x^{24}$ term is
implicit). You can read the terms straight off the hex: bits 23–12 set, plus
10, 3, and 0. Note at `crates/adsb-core/src/crc.rs:12-16` that this is **not**
$x^{24}+x^{23}+x^{10}+x^3+1$ as the previous implementation's comment claimed —
that's twelve missing terms, and while it made no difference to the code it
would have been wrong on a conference slide.

The transmitter computes the remainder of $M(x)\cdot x^{24}$ modulo $G(x)$ and
appends it. The receiver divides the whole received polynomial by $G(x)$; a
clean message leaves zero, because appending the remainder makes the total
divisible by construction.

`crc24()` at `crc.rs:23-44` is the standard bit-serial long division: shift the
next message bit into the register, and if the bit shifted *out* was 1, XOR in
the generator. That is polynomial division with the coefficients living in the
register.

**The property that matters most.** The code is **linear**. If $r = c + e$ for
a valid codeword $c$ and an error pattern $e$, then
$$s(r) = s(c + e) = s(c) + s(e) = s(e)$$
since $s(c) = 0$. **The syndrome depends only on the error, not on the message
underneath it.** This is what makes error correction a table lookup rather than
a search, and it is verified directly by
`crc.rs:101-117` (`syndrome_is_linear_in_the_error_pattern`), which flips the
same bit in two different messages and asserts the syndromes match.

Consequences you'll use in stage 4 (§4.2.4):

- Compute the syndrome of each of the 112 one-hot error vectors once. Any
  single-bit error is now one hash lookup and one XOR.
- Two-bit errors need $\binom{112}{2} = 6{,}216$ entries, about 25 KB.
- A degree-24 generator detects **all** burst errors of length ≤ 24. This is
  why the polynomial is so dense — it's optimised for burst detection, which is
  what interference actually produces.

**The false-accept rate.** With 24 parity bits, a random 112-bit pattern passes
with probability $2^{-24} = 6\times10^{-8}$. That number appears all over the
scoreboard reasoning: it is small enough that a CRC-clean DF17 is essentially
certainly real (so ground truth is *free*, §4.6.2), and large enough that
6,216 correction attempts per candidate over millions of candidates
manufactures aircraft that do not exist (§4.2.4).

**The formats you cannot check at all.** DF0/4/5/16/20/21 XOR the aircraft
address into the parity field, so the remainder *is* the address and there's
nothing left to verify against. `remainder_is_address()` at `crc.rs:46-48`
identifies them; `Frame::icao()` at `frame.rs:133-141` uses that to decide
whether to read the address from the AA field or recover it from the CRC. The
hazard is that **the remainder of pure noise is a plausible-looking ICAO**, so
these formats are a ghost-aircraft generator. The defence lives in the tracker
(§4.4.1).

> **Read more:** Lin & Costello, *Error Control Coding*, 2nd ed., chapters 3–4
> for cyclic codes and syndrome decoding. Peterson & Brown's 1961 paper
> "Cyclic Codes for Error Detection" is the original and still clear. For the
> specific Mode S polynomial and its correction properties, ICAO Doc 9871
> Appendix A.

### 4.1.3 `frame.rs` — validated bytes

`Frame::new()` at `frame.rs:101-113` enforces that the byte count matches what
the DF demands, so every accessor below can index without bounds anxiety.

Note carefully what a `Frame` does *not* promise: holding one does **not** mean
the CRC passed. `crc_ok()` at `frame.rs:145-153` is separate, and returns
`false` for the address-overlaid formats because there is nothing to check.
That separation is the honest modelling of a protocol where some messages are
simply unverifiable.

### 4.1.4 `decode.rs` — field extraction

Mechanical spec implementation, and deliberately *not* behind a trait (see
§4.2.0). A few pieces worth reading closely:

**Callsign** (`decode.rs:70-89`). Eight 6-bit characters from a 64-entry
alphabet. Index 0 and various slots are reserved encodings which we drop rather
than render, because putting `#` on a map is worse than a shorter callsign.

**Altitude** (`decode.rs:108-124`). The 12-bit field at bits 40–51, with a
**Q bit** at bit 47. Q=1 means 25-foot increments: remove the Q bit, close the
gap, and $\text{alt} = 25n - 1000$. Q=0 selects a 100-foot **Gillham** code —
a reflected Gray code inherited from mechanical altimeter encoders, used only
above 50,175 ft. We return `Unavailable` for it (`decode.rs:113-115`), because
I had no test vectors to verify a Gillham decoder against and shipping an
unverified one seemed worse than a documented gap.

**Velocity** (`decode.rs:166-232`). Subtypes 1 and 2 give east-west and
north-south *ground* velocity components, from which we compute
$$v = \sqrt{v_{EW}^2 + v_{NS}^2}, \qquad \theta = \operatorname{atan2}(v_{EW}, v_{NS})$$
Note the argument order — `atan2(east, north)` gives degrees clockwise from
north, which is the aviation convention, not the mathematical one.

Two subtleties: the encoded value 0 means "no information", not "stationary"
(`decode.rs:191-199`), and subtype 2 is supersonic and reports in 4-knot units
(`decode.rs:185-186`).

This is also where the naming matters. The field is **track**, not heading —
ADS-B reports the direction the aircraft is *moving over the ground*, which
differs from where its nose points by the wind correction angle, sometimes by
15°. The distinction is enforced in the type system at
`crates/adsb-core/src/units.rs:65-71`.

### 4.1.5 `cpr.rs` — the genuinely clever bit

443 lines, and the only part of the protocol I'd call elegant.

**The problem.** A position message spends 17 bits each on latitude and
longitude. $2^{17} = 131{,}072$. To cover 360° of latitude at that resolution
gives 2.7 millidegrees ≈ 300 m — too coarse. But you cannot afford more bits.

**The solution.** Don't send a global position. Send a position *within a zone*,
and use **two different zone systems** on alternating messages so that the
disagreement between them identifies the zone.

Concretely: even frames divide latitude into $4N_Z = 60$ zones, odd frames into
$4N_Z - 1 = 59$, where $N_Z = 15$ (`cpr.rs:30`). Zone widths are
$$\Delta_0 = \frac{360}{60} = 6°, \qquad \Delta_1 = \frac{360}{59} \approx 6.102°$$

Let the true latitude be $\lambda$, and write it in each system as
$$\lambda = \Delta_0(j_0 + y_0) = \Delta_1(j_1 + y_1)$$
where $j_i$ is the integer zone index and $y_i \in [0,1)$ is the fraction the
message actually carries. Now compute
$$59y_0 - 60y_1 = 59\!\left(\frac{\lambda}{\Delta_0} - j_0\right) - 60\!\left(\frac{\lambda}{\Delta_1} - j_1\right)$$
$$= 59\cdot\frac{60\lambda}{360} - 59j_0 - 60\cdot\frac{59\lambda}{360} + 60j_1 = 60j_1 - 59j_0$$

The $\lambda$ terms cancel exactly. And since the two zone indices differ by at
most one for the same latitude, $60j_1 - 59j_0 \approx j$ — **the zone index
falls out of the two fractions alone**. That is the line at `cpr.rs:128`:

```rust
let j = (59.0 * elat - 60.0 * olat + 0.5).floor();
```

The $+0.5$ is rounding to nearest. It is one of the most information-dense
lines in the repository.

Longitude works the same way, except the number of longitude zones must shrink
towards the poles or the resolution would become absurd as the meridians
converge. `nl()` at `cpr.rs:71-83` implements the standard
$$N_L(\lambda) = \left\lfloor \frac{2\pi}{\arccos\left(1 - \frac{1-\cos(\pi/2N_Z)}{\cos^2\lambda}\right)} \right\rfloor$$
which runs from 59 at the equator to 1 at the poles. Note the clamp at line 82:
the closed form evaluates to exactly 60.0 at $\lambda = 0$, which is off by one
against the ICAO table, so it's clamped rather than trusted.

**The bug this module exists to not have.** When the *odd* frame is the most
recent, the longitude zone width is $360/(N_L - 1)$, not $360/N_L$. The
previous implementation used $N_L$ for both. At Ottawa's latitude $N_L = 36$,
so it computed $360/36 = 10.0°$ per zone instead of $360/35 = 10.286°$ — a
0.109° error, about **7.4 km**, on roughly half of all position fixes. The
correct divisor is `cpr.rs:157-162`.

What makes this worth studying is *why it survived*. The output still looked
like a smooth, plausible track. CPR does not fail loudly; it returns a
confident wrong answer. And the canonical test vector that every tutorial
gives you exercises only the **even** anchor, so it passes with the bug live.

**The defence is the encoder.** `encode()` at `cpr.rs:90-107` exists primarily
so the decoder can be tested by round trip: encode a position, decode it back,
assert it survives — for *both* anchor frames, over a global grid
(`cpr.rs:302-330`). I verified this catches the bug by reintroducing it: four
tests failed, including one reporting a 446 km error, while the canonical
even-anchor vector still passed.

The tolerance constant at `cpr.rs:287-296` is worth reading: 30 m, chosen
because CPR's own quantisation is ~5 m at the equator rising to ~21 m near the
poles where $N_L$ collapses to 1, and the bug class being guarded against is
kilometres.

> **Read more:** ICAO Doc 9871 Appendix A has the normative CPR algorithm
> including the local decode and the surface variants. Junzi Sun's book has the
> clearest worked example. There is also a genuinely interesting failure
> literature — search for "CPR decoding errors ADS-B" for papers on how
> mispaired frames produce plausible ghosts.

## 4.2 `adsb-dsp` — samples into frames

### 4.2.0 Why four traits and not ten

The crate documentation at `crates/adsb-dsp/src/lib.rs:29-52` lays out the
design. Four stages are swappable; everything else is fixed. The rule for
deciding is: **a trait is worth it where there are genuinely different
approaches with different trade-offs, and worthless where there is one right
answer and a published test vector.**

CRC arithmetic, field extraction, the callsign alphabet and CPR all have one
right answer. Making them swappable would add maintenance and teach nothing.
Detection, slicing, magnitude computation and validation all have real design
space.

Two rules keep the seams honest (`adsb-dsp/src/lib.rs:39-47`):

1. **Dispatch is per buffer, never per sample.** One dynamic call per 262,144
   samples is unmeasurable. One per sample would dominate the runtime and make
   every benchmark a lie about the algorithm.
2. **`Pipeline` owns the shared plumbing** — carry-over, absolute offsets,
   duplicate suppression. If each implementation owned those, they'd differ
   subtly and an A/B would be comparing plumbing.

### 4.2.1 Constants and the magnitude scale

`adsb-dsp/src/lib.rs:62-101`. The interesting one is the fixed-point choice.

Magnitudes are `u16`, not `f32`. Three reasons in increasing order of
importance (`adsb-dsp/src/lib.rs:73-83`): half the buffer, integer comparisons, and — the
real one — **it makes a lookup table possible**. $|IQ|$ has only $129 \times
129$ distinct values once you exploit the symmetry of $|b - 127.5|$ over
$0..128$, so the entire magnitude stage can become a 33 KB table that lives in
L2 cache. Handing `f32` across this seam would foreclose the single most
instructive optimisation in the project.

Full scale is $127.5\sqrt2 = 180.312$, mapped to `u16::MAX`
(`adsb-dsp/src/lib.rs:84-87`). The test at `adsb-dsp/src/lib.rs:108-117` asserts the extreme input lands
exactly at the top without wrapping — an overflow there would turn the loudest
aircraft into the quietest.

`samples_per_us()` at `adsb-dsp/src/lib.rs:95-97` returns `f64` deliberately. At 2.4 MS/s
this is **2.4, not an integer**, and accumulating an integer approximation
across 112 bits walks the sampling point out of the bit entirely.

### 4.2.2 Stage 1: `magnitude.rs`

$|z| = \sqrt{I^2+Q^2}$. That is the whole demodulator for OOK, and
`NaiveMagnitude::compute` at `magnitude.rs:56-66` is exactly that in `f32`.

This stage should **never change what you decode**, only how fast. That makes
it the right place to learn to trust the pipeline digest: if a magnitude change
alters the message set, you have a bug, not an optimisation.

Improvements, in order of instructiveness (`magnitude.rs:19-24`):

1. **The 129×129 LUT.** What dump1090 does.
2. **Alpha-max-plus-beta-min**: $|z| \approx \max + 0.4\min$, no multiply,
   ~4% peak error. Try it, then measure — the preamble test is a *ratio* test,
   and 4% of error at low SNR costs precisely the marginal aircraft you were
   trying to recover. Discovering that it isn't good enough is the point of
   trying it.
3. **SIMD**, via `std::simd` or NEON on the Pi.

`noise_floor()` at `magnitude.rs:81-90` uses a **median**, not a mean. ADS-B
bursts are rare and bright, so the mean is dragged upward by exactly the
signals you're trying to distinguish from the floor. The test at
`magnitude.rs:126-135` makes 1% of samples a loud burst and asserts the median
doesn't move.

### 4.2.3 Stage 2: `detect.rs` — where the ceiling is set

**This is the most important stage.** A message the detector never proposes
cannot be recovered by any amount of cleverness downstream. Measured on our
golden fixture: the naive detector finds 517 CRC-valid messages where a merely
*mean-integrating* one finds 2,403. Roughly three quarters of the recoverable
traffic is discarded right here.

`NaiveDetector` (`detect.rs:119-208`) averages each half-microsecond slot and
requires $\min(\text{pulses}) > 2\times\max(\text{silences})$ plus an absolute
floor. It has five specific weaknesses, enumerated at `detect.rs:36-59`:

1. **It samples half-microsecond slots rather than correlating the whole
   template.** Integrating $N$ samples buys roughly $\sqrt N$ in SNR for
   non-coherent detection (§5.1), and the preamble is ~19 samples at 2.4 MS/s.
2. **`min` vs `max` is a veto.** One noisy sample in any of twelve silence
   slots kills an otherwise perfect match. Compare each pulse against its
   *adjacent* spaces and accumulate a score, so an outlier costs a little
   instead of everything. **This structural issue, not the threshold, is what
   rejects weak signals** — I verified that by sweeping the threshold from
   3× down to 1.2× the noise floor and getting *identical* results.
3. **The threshold is absolute** (`MIN_PULSE` at `detect.rs:102`), so
   sensitivity silently changes the moment you touch the gain. Tracking a
   running noise floor and thresholding on SNR is the single highest-value fix.
4. **No context checks.** Real messages are preceded by quiet.
5. **No sub-sample alignment.** Fit a parabola to the correlation peak and
   report the fractional offset via `Candidate::frac`.

And an engineering lesson that doesn't show up as sensitivity: a 19-tap
correlation at every sample offset is ~46 million MACs/s and will not fit on a
Pi alongside everything else. Gate cheaply first — is this sample even above
the floor? — and correlate only at the hits. ~100× less work for identical
output. The naive detector already does a crude version of this at
`detect.rs:169-176`.

### 4.2.4 Stages 3 and 4: `slice.rs` and `validate.rs`

**Slicing.** `NaiveSlicer::slice` at `slice.rs:108-159` takes one sample from
each half-bit and compares them. The precomputed windows at `slice.rs:83-96`
are the anti-drift mechanism: each bit's position is computed from an `f64`
microsecond time rather than accumulated. The test at `slice.rs:170-180` pins
this — bit 111 starts at sample 286, where a naive integer approximation of
"2 samples per bit" would say 238.

The awkward number again: 2.4 samples per bit at 2.4 MS/s. dump1090 ships a
hand-derived 5-samples-per-2-bits scheme for exactly this rate; deriving it
yourself is a good exercise. 2.0 MS/s gives exactly 2 samples per bit and a
trivially correct slicer at the cost of coarser timing. **Measure both.**

Improvements at `slice.rs:26-43`: integrate over each half-bit instead of
point-sampling (worth ~17%, already measured); interpolate at fractional
positions; consume `Candidate::frac`; emit real confidence; detect "both halves
high", which is two aircraft transmitting at once rather than a bit.

**Validation and the coupling that matters.** `CrcOnlyValidator` at
`validate.rs:88-114` accepts only frames that are already perfect. Building on
it is the most intellectually satisfying stage, and it depends entirely on
stage 3.

`RawFrame.conf` at `crates/adsb-dsp/src/types.rs:48-63` carries per-bit
confidence, and its documentation explains why the seam exists: two-bit
correction is a 6,216-way search, and at $6\times10^{-8}$ per trial over
millions of candidates you *will* manufacture aircraft. Restricting flips to
bits the slicer already doubted makes correction both far cheaper and far
safer. The naive slicer writes 255 everywhere (`slice.rs:125` and `slice.rs:145`), which means
**stage 4 has nothing to work with until you replace stage 3**. That coupling
is deliberate, and it's the one design choice in the repo I'd most like you to
push back on if it feels like a rigged game.

`weakest_bits()` at `types.rs:106-111` is the helper a confidence-guided
corrector would iterate over.

### 4.2.5 `pipeline.rs` — the plumbing that makes benchmarking honest

`Pipeline` (`pipeline.rs:75-297`) is deliberately not swappable. It owns three
things:

**The carry-over ring.** The previous implementation searched each read buffer
independently and silently lost every message straddling a boundary — about
0.1% of traffic. That barely matters for reception, but it makes the score
depend on **buffer size**, which is fatal for A/B comparison. `search()` at
`pipeline.rs:192-205` only searches offsets where a full long message still
fits, and `trim()` at `pipeline.rs:277-291` keeps exactly the history the
detector declared it needs via `lookback()`.

The invariant is tested at `pipeline.rs:356-382`: the same bytes fed in 512,
1k, 4k, 64k and whole-file chunks must produce byte-identical output.

**Absolute offsets.** A message has the same identity regardless of which
buffer it arrived in.

**Duplicate suppression.** `drain_candidates()` at `pipeline.rs:226-269` sets
an `accept_from` watermark after each successful decode so one transmission
yields one message.

`PipelineStats::crc_yield()` at `pipeline.rs:57-62` carries the warning in its
own doc comment: report it, never optimise it. Explained in §4.6.2.

### 4.2.6 `synth.rs` — ground truth you can dial

Real captures give you CRC-clean bytes, which tells you a message was
*recovered* but never that one was *missed*. Synthetic signals give you the
answer key.

`synthesize()` at `synth.rs:143-185` builds an amplitude envelope, then
modulates it onto a rotating carrier with additive noise. Two capabilities this
unlocks:

**SNR sweeps.** `with_snr_db()` at `synth.rs:69-72` sets the signal level for a
target SNR. Generate the same messages at descending levels and plot recovery
against SNR. A single number tells you an implementation is better; **a curve
tells you *where* it's better**, which is far more useful when deciding what to
work on next.

**Coverage the sky didn't provide.** Our golden fixture has no surface
positions, no Gillham altitudes and no airspeed subtypes, because nothing
nearby transmitted them. Here you can just ask.

The RNG at `synth.rs:121-140` is a deliberately simple xorshift with a
Gaussian built from summed uniforms (central limit theorem, four terms). Not
cryptographic — reproducibility is the entire requirement, because a benchmark
that moves between runs is not a benchmark.

This module is also compiled into `doctor` so the Pi can prove its decode chain
works with no antenna attached (§4.6.1).

## 4.3 `adsb-source` — where samples come from

### 4.3.1 The trait

`IqSource` at `crates/adsb-source/src/lib.rs:40-95`. Two decisions:

**Bytes, not complex floats.** `read()` hands back interleaved `u8` — exactly
what the RTL2832U produces. Converting at the source turns 2 bytes into 8
before anything has looked at them, doubles memory bandwidth on a Pi, and
forecloses the magnitude LUT.

**Errors are a taxonomy, not a bag** (`adsb-source/src/lib.rs:131-153`). On a machine you
cannot log into, *how* something failed decides what should happen:

- `Config` — wrong on purpose. Fail loudly at startup, never retry.
- `Transient` — rtl_tcp restarted, USB glitched. **Never exit.** Reconnect,
  count it. A receiver that quietly died six hours ago is the worst outcome.
- `EndOfStream` — a file ran out. Normal.

### 4.3.2 `file.rs` and the odd-byte trap

The most important source, because it's what makes results reproducible. A
benchmark against live radio is measuring the weather.

`Read::read` may legally return an **odd** byte count. Treat that as complete
and every subsequent sample has I and Q swapped for the rest of the file.
Because $|z| = \sqrt{I^2+Q^2}$ is symmetric in its arguments, **the swap is
invisible in the signal statistics** — it just quietly decodes nothing and looks
exactly like a bad demodulator.

`FileSource::read` at `file.rs:126-183` carries the stray byte forward. Worth
reading the loop at `file.rs:145-171` carefully: my first version stashed the
byte and returned `Ok(0)`, which the caller cannot distinguish from EOF, so a
reader dribbling one byte at a time ended the stream immediately. The fix is
the `if filled >= 2 { break }` in the `Ok(n)` arm at `file.rs:165-167` — keep reading until a
whole sample is in hand. The test at `file.rs:245-262` drives it with 1, 3, 7
and mixed patterns.

`Pace` at `file.rs:26-35`: `Realtime` sleeps so replay behaves like a live
receiver; `Fast` doesn't. **Benchmarks must use `Fast`**, or you are measuring
`thread::sleep`. `SourceOptions::for_benchmark` at `adsb-source/src/lib.rs:237-244` enforces it.

### 4.3.3 `tcp.rs` — the rtl_tcp protocol

`rtl_tcp` is a small C program that opens the dongle and streams samples over a
socket. On connect it sends a 12-byte header — magic `RTL0`, then tuner type
and gain-table length as big-endian `u32` — after which it's a raw byte stream.
Commands are five bytes: one opcode, then a big-endian `u32`
(`tcp.rs:27-35`).

**Why this is the recommended deployment.** `rtl_tcp` uses libusb
**asynchronous** transfers internally, so it keeps capturing while the consumer
is busy. A synchronous `read_sync` loop drops whatever arrives between calls,
invisibly, and the loss shows up as "my detector is bad". It also keeps this
binary free of C dependencies, which turns an aarch64 cross-build from an
afternoon into a non-event.

Two things the previous client lacked:

**Gain commands.** `0x03` gain mode, `0x04` gain in tenths of a dB, `0x0D` gain
by table index, `0x08` AGC, `0x0E` bias tee. Without these a gain sweep over
rtl_tcp is impossible. Note the ordering at `tcp.rs:261-274`: manual mode must
be sent **before** the value, or the gain is silently ignored.

**A read timeout.** `dial()` at `tcp.rs:110-114` sets one, with the comment
explaining why: without it a wedged server blocks the process forever with no
log line — the single worst failure mode on a box you cannot log into. The test
at `tcp.rs:470-497` stands up a server that sends a header then goes silent and
asserts we give up in under three seconds.

`reconnect()` at `tcp.rs:166-176` replays every recorded setting. Reconnecting
without replaying leaves the dongle on defaults — wrong frequency, auto gain —
and the receiver goes quiet for reasons invisible from the message count.

### 4.3.4 `misbehaving.rs` — breaking things on purpose

File replay is deterministic and well-behaved, which is exactly why it cannot
test the paths that matter on hardware. `Fault` at `misbehaving.rs:25-44`
injects disconnects, short reads, odd reads, clipping, sample gaps and stalls.

`GapEvery` deserves attention: it silently discards samples, which is what a
USB buffer overflow looks like from the receiver's side. That failure is
**indistinguishable from poor reception in the message count alone**, which is
why `doctor` measures the effective sample rate (§4.6.1).

## 4.4 `adsb-track` — messages into aircraft

### 4.4.1 The ghost defence

A decoded message is a fact about one instant. An aircraft is an accumulation.
`Tracker::observe` at `crates/adsb-track/src/lib.rs:231-346` folds messages
into per-aircraft state.

The first thing it does (`adsb-track/src/lib.rs:236-245`) is the ghost defence: DF17/18 carry
the address in the clear and are CRC-checkable, so seeing one makes an address
trustworthy. Everything else is admitted only for addresses already on the
verified list. Without this, the remainder of noise decoded as DF4 populates
your map with aircraft that do not exist.

There's a second, subtler decision at `adsb-track/src/lib.rs:255-260`: an address reappearing
after a long silence starts a **fresh** track. The same aircraft an hour later
is a different flight, and carrying the old position forward would draw a line
across the map.

### 4.4.2 `position.rs` — stage 5 and the plausibility gates

`GlobalCprSolver` at `position.rs:175-280`. Two things worth studying.

**Monotonic versus wall-clock time.** `CprState` at `position.rs:147-155` holds
frames with an `Instant`, not a `SystemTime`, and the comment explains why:
using wall time would mean an NTP step could invalidate a good pair or validate
a stale one — at exactly the moment a Pi first syncs its clock after boot. The
test at `position.rs:437-457` steps the wall clock 56 years forward and asserts
pairing survives.

**The gates** (`admit()` at `position.rs:185-211`). Range from the receiver
(400 km, the radio horizon at FL360) and implied speed between consecutive
fixes (700 kt). These reject implausible fixes **where they are produced**.

The previous implementation instead filtered by bounding box in the API, after
a mispaired frame put an Ottawa aircraft over Lake Huron. That's the wrong
place: a bbox filters *display*, but does not stop a bad fix from being stored,
from anchoring the next local decode, or from being counted as a success.

`RejectReason` at `position.rs:78-92` exists so rejections can be counted **by
reason**. Without that, positions-per-minute drops after a change and you
cannot tell whether your solver got stricter or your decoder got worse.

### 4.4.3 `snapshot.rs` — the wire contract

`Snapshot` at `snapshot.rs:88-140` is an immutable view the API serves without
locks. Naming choices that are contract decisions, documented at
`snapshot.rs:12-24`:

- **`now_ms` in the envelope**, so clients compute ages against the *server's*
  clock. Otherwise a laptop 40 seconds out of sync shows everything as stale.
- **Epoch milliseconds, never formatted strings.** The old schema stored
  `"2026-08-06 12:00:00"` — no `T`, no `Z` — which V8 parses as *local* time and
  older Safari rejects outright.
- **Units in field names.** `altitude_ft`, `ground_speed_kt`, `track_deg`.
- Ages clamped at zero (`snapshot.rs:68-69`), so a backwards clock step can't
  render an aircraft seen in the future.

## 4.5 `adsb-store` — durable history

Live state is in memory; this crate is only for the past. Three things the
previous implementation got wrong, all documented at
`crates/adsb-store/src/lib.rs:6-26`.

**Batching.** One SQLite write per message is fine at 95 msg/min and is not at
the thousands a better decoder produces. `flush()` at `adsb-store/src/lib.rs:311-394` commits
in transactions.

**Backpressure direction.** The queue is bounded and full means **drop and
count**, never block (`StoreHandle::submit` at `adsb-store/src/lib.rs:183-198`). A stalled
decoder loses samples permanently; a dropped row loses one dot on a map. That
is a deliberate trade and it is *measured* rather than silent — the test at
`adsb-store/src/lib.rs:576-600` asserts 5,000 submits into a queue of 8 complete in under a
second and that the drops show up as `degraded`.

**`auto_vacuum = INCREMENTAL` before the first table exists**
(`schema.rs:66-70`). This cannot be changed later without a full `VACUUM`,
which rewrites the entire database. Without it, retention deletes rows but
never returns pages to the filesystem. It's a one-shot decision that is easy to
miss and expensive to get wrong.

Also note the **composite index** `(icao, ts_ms)` at `schema.rs:50-54`. Every
history query is "this aircraft, ordered by time"; two separate single-column
indexes each serve half of it and SQLite can only use one. The test at
`schema.rs:171-187` runs `EXPLAIN QUERY PLAN` and asserts the index is actually
used — a cheap way to catch a schema regression that would otherwise only show
up as a slow Pi.

## 4.6 `adsb-server` — the binary

### 4.6.1 `doctor.rs` — the command for a machine you cannot log into

Every check ends in a sentence you can act on. `clip_pct = 3.2` is a
measurement; "gain too high, reduce to about 44 dB and re-run" is a diagnosis,
and only the second is useful at arm's length.

Three checks exist because of specific failure modes:

**The clock** (`doctor.rs:246-272`). A Raspberry Pi has no real-time clock and
boots in 1970. Every timestamp is then garbage, `?max_age=` filters return
nothing, and the whole system looks like a dead radio. Three lines of code, and
almost always the last thing anyone suspects.

**The effective sample rate** (`doctor.rs` in `check_source`, and permanently in
`/api/v1/stats`). Count samples, divide by elapsed wall time, compare to the
configured rate. **Dropping samples and hearing nothing are identical in the
message count**, and one of them is a software problem you can fix.

**The offline self-test** (`check_self_test`, `doctor.rs:323-382`). Decode the synthetic frames in
memory and assert the expected set comes back. This proves the decode chain is
correct on the Pi's own CPU with no antenna attached, separating "bad build" —
a bad cross-compile, a corrupt binary — from "bad reception", *before* you go
climbing after the antenna.

The RF sanity table in the module docs maps each measurement to a diagnosis:
clipping → gain too high; large DC offset → direct sampling may be enabled;
preambles but no CRC passes → **sample rate mismatch**, not weak signal.

### 4.6.2 `bench.rs` — the scoreboard

**The headline is unique CRC-valid messages.** Because the false-accept rate is
$6\times10^{-8}$, a CRC-clean DF17 is essentially certainly real, so **ground
truth is free**. You need no labelled data: the score is just how many distinct
real messages you pulled out of fixed bytes, and more is unambiguously better.
That property is what makes this a good learning project — the feedback loop is
honest and needs no oracle.

**Yield is an anti-metric.** A better detector proposes more marginal
candidates, so yield *falls* while messages rise. Measured on the golden
fixture: 607 → 2,403 messages took yield from 69% down to 5%. Anyone optimising
yield would tune the detector to be *less sensitive* and feel good about it.
It's printed with "(report, never optimize)" beside it.

**Two guards can veto an apparent win:**

- `ghost_icao_ratio` (`bench.rs:238-247`) — addresses seen exactly once and
  never located. Real aircraft transmit twice a second, so a singleton is the
  signature of a false CRC accept. This is what catches error correction
  inventing aircraft.

  **But it has a floor that depends on the recording, not on the decoder.**
  `golden` and `desk` both score 0.000; `porch` scores 0.133 — about one
  address in seven. Nothing is inventing aircraft there. A recording that
  hears further hears *more marginal traffic*, and an aircraft caught at the
  very edge of detection legitimately produces one CRC-clean message and then
  nothing. The guard is only meaningful against the same fixture's own
  baseline. Compare a porch run against golden's 0.000 and you will fail every
  honest improvement you ever make.
- `realtime_factor` — an implementation finding 30% more at 0.8× realtime
  cannot keep up on a Pi and is a regression.

`digest` is an FNV-1a hash over the sorted message set. "Did behaviour change
at all?" in one line — unchanged is what you *want* after a magnitude
optimisation and a red flag after a detector change. FNV rather than SHA-256
because it detects change, doesn't need to resist an adversary, and avoids a
dependency; it's also stable across architectures, so a Mac digest can be
compared to a Pi digest.

### 4.6.3 `config.rs` — provenance as a feature

On a machine you cannot log into, the question is never "what is the config",
it's **"did my edit take effect"**. A dump that prints values but not origins
cannot answer that. `Sourced<T>` at `config.rs:141-158` carries the layer each
value came from, and `print_resolved()` at `config.rs:437-495` shows it:

```
receiver.lat        45.412      $SKYWARD_RECEIVER_LAT
sample_rate_hz      2400000     default
```

Two things fail rather than default:

**Unknown keys.** `#[serde(deny_unknown_fields)]` at `config.rs:169` and
`config.rs:186`. A typo'd `recevier.lat` silently ignored is the classic
blind-box failure — you edit, restart, nothing changes, and there is no signal
at all.

**A missing receiver position.** Defaulting to `0.0, 0.0` puts the station in
the Gulf of Guinea, which makes the range gate reject every aircraft on earth —
a total outage that looks like bad reception. Half a coordinate is refused too
(`config.rs:418-424`).

Validation at `config.rs:374-427` explains *why* rather than just refusing:
the sample-rate error says "a bit is one microsecond long, so below 2 samples
per microsecond the two halves of a bit cannot be told apart".

### 4.6.4 `run.rs` and `api.rs`

The decoder runs on a **plain OS thread**, not a tokio task
(`run.rs:299-310`): it's a tight CPU loop that would otherwise monopolise an
async worker. It publishes into an `ArcSwap<Snapshot>` twice a second; the API
reads that with no locks and no database.

The old design took an async mutex around a blocking rusqlite connection for
*every* request, which serialised them all and blocked the runtime. Live
aircraft were never in the database to begin with — they were already in memory.

**Health is freshness, not liveness** (`AppState::health_json`, `run.rs:104-158`). The old API answered
`ok` whenever SQLite was readable, which stayed true for hours after the
decoder died. Here, `stalled` is returned when the last sample is more than 30
seconds old, and `/healthz` responds **503** for it so a bare `curl -f` is a
valid probe.

The SSE stream is at `api.rs` (`stream`). SSE rather than WebSocket: the
traffic is one-way, `EventSource` reconnects by itself, it survives proxies,
and it's about five lines in SvelteKit. There are no client-to-server messages,
so an upgrade handshake would buy nothing.


## 4.7 `registry.rs` — how a second implementation lands beside the first

This is the mechanism the whole project is built around, so it is worth
understanding before you write any DSP at all. The premise in the README —
"a new implementation lands *beside* the old one instead of replacing it" —
is implemented here, in 246 lines.

### 4.7.1 The shape

Each stage is a `Box<dyn Trait>` chosen by name at runtime, not a generic
parameter chosen at compile time. Four parallel structures per stage:

| | |
|---|---|
| a trait | `Magnitude`, `PreambleDetector`, `BitSlicer`, `FrameValidator` |
| a `*_NAMES` table | `(name, one-line description)`, drives `--list-impls` |
| a constructor `fn` | `detector(name, sample_rate) -> Result<Box<dyn …>>` |
| a `match` arm | maps the name to the type |

`ImplSet` is one name per stage; `build()` turns it into a `Pipeline`.

Dynamic dispatch costs a vtable lookup per *call*, and the calls are per-buffer
rather than per-sample — `compute()` is handed a whole slice. So the indirection
is amortised over ~65k samples and does not show up in `ns/sample`. That is why
the seam can be this cheap.

### 4.7.2 Why names rather than types

Because the comparison you actually want is against **your own previous
attempt**, not against the baseline. `correlator-v3` against `correlator-v2` is
the interesting question by week three, and a compile-time selection would mean
rebuilding to answer it — or worse, deleting v2.

Three invariants make that safe, each with a test:

- **Unknown names are fatal**, never a silent fallback. `unknown_names_fail_
  loudly_and_list_the_alternatives` asserts the error echoes your typo *and*
  lists the valid names. On a Pi, "it ran but quietly used something else" is
  exactly the failure that costs an evening.
- **Every registered name builds.** `every_registered_name_actually_builds`
  walks the `*_NAMES` tables and constructs each one, so adding a row and
  forgetting the `match` arm fails on your laptop rather than on the Pi.
- **Reported names match registry keys.** `names_reported_by_impls_match_their_
  registry_keys` — otherwise a run record would lie about what produced it,
  which quietly poisons every comparison you make afterwards.

### 4.7.3 Adding one, concretely

Say you are writing the correlator from §5.1.

1. **Write the type** in `detect.rs` and implement `PreambleDetector`
   (`detect.rs`, the trait). Five methods: `name`, `describe`, `reset`,
   `lookback`, `detect`.

2. **Honour the buffer-invariance contract.** `detect()` gets `(mag, from, to,
   out)` and must emit candidates whose offsets lie in `[from, to)`, depending
   only on the samples and not on how they were chunked. Emit outside the
   window and you duplicate or drop messages as a function of buffer size.
   `output_is_independent_of_buffer_size` (`pipeline.rs:356-382`) will catch it.

3. **Declare your lookback.** If you correlate over a 16-sample template you
   read samples before `from`; return that from `lookback()` and the pipeline
   guarantees they are there (except at stream start).

4. **Register it**: one row in `DETECTOR_NAMES`, one arm in `detector()`.

5. **Fill in `Candidate`.** `offset` and `score` at minimum. A correlator can
   also give you `frac` — sub-sample refinement in `[-0.5, 0.5]`. At 2.4 MS/s a
   bit is 2.4 samples wide, so half a sample of misalignment costs real margin
   on every one of 112 bits, and a correlator that interpolates its peak is the
   only thing that can tell you where the edge actually was. `score` is
   explicitly **only comparable within one implementation** — never threshold
   on it across two.

### 4.7.4 The gap you will hit immediately

`--list-impls` prints its stages as `magnitude (--mag)`, `detector (--detect)`
and so on, and the module docs give this example:

```text
skyward bench --detect correlator-v2 --compare runs/baseline.json
```

**Those flags do not exist.** `skyward bench --detect naive` fails with
`unexpected argument '--detect' found`; only `--impl-set` is wired up, and the
only preset is `baseline`. Verified against the shipped binary.

So today the actual route is to add a preset in `ImplSet::preset()` — there is
already a comment marking the spot (`// Add "thomas" here once there is
something to put in it`) — and select it with `--impl-set`:

```rust
"thomas" => Some(ImplSet { detector: "correlator-v2".into(), ..Self::baseline() }),
```

```bash
skyward bench --impl-set thomas --compare runs/baseline.json
```

That works, and it has the side benefit of naming the *combination* you ran,
which is what a run record should record anyway. But it means one preset per
experiment, which gets tedious fast. Adding four optional per-stage flags to
the CLI is a small change and would make the registry behave the way its own
documentation already claims.

---

## 4.8 `web.rs` and `client/` — the interface

### 4.8.1 Why the client is inside the binary

`web.rs` embeds `client/build` with `rust-embed` and serves it as the router's
fallback. Same reasoning as one binary rather than a decoder-plus-API pair: two
artifacts means two ways to deploy the wrong version. A `--web-root` on the Pi
would let a six-week-old client sit in front of a fresh server, reading fields
the API no longer sends, with nothing anywhere reporting the disagreement.

Consequences worth knowing:

- **Deployment is `scp skyward pi:` and nothing else.** `skyward --version`
  describes the interface as well as the decoder, and `doctor` reports what is
  actually inside (`web.client   21 files, 1642 KiB embedded`).
- **A checkout that has never run `npm run build` still compiles.** `build.rs`
  writes a placeholder page naming the missing command, so `cargo build` fails
  only for real reasons.
- **`/api/*`, `/healthz` and `/readyz` are excluded from the fallback.** Without
  that a typo'd endpoint returned `200` and a page of HTML, so `fetch` resolved
  happily and then died parsing `<!doctype html>` as JSON. A router fallback
  catches *every* unmatched path, including the ones you meant to 404.
- **`index.html` is never cached; hashed assets are cached for a year.** A
  cached entry point points at asset filenames that no longer exist after an
  upgrade, and the app fails to boot.

### 4.8.2 Two silent failures the client had to learn about

Both are worth internalising because they are the same species of bug as
`stalled` versus `ok` on the server: **a thing that looks healthy because
nothing raised an error.**

**A dead stream does not necessarily raise one.** When the receiver was stopped
behind a dev proxy the socket stayed open, `onerror` never fired, and the view
reported `STREAMING` over a sixteen-second-old snapshot showing `heard 0.1 s` —
the ages looked live because they are computed against the server clock carried
in the envelope, and that had stopped advancing too. The fix is a staleness
watchdog: the stream publishes once a second, and six seconds of silence is
treated as death regardless of what the socket says. Silence is failure; only
an arriving snapshot proves liveness.

**A live stream only proves the *server* is alive.** With the antenna
unplugged, snapshots kept arriving exactly on schedule — they were simply
empty. The server knew (`status: stalled`); nothing asked it. The health poll
now drives a third state, `NO SIGNAL`, distinct from a dropped connection,
because the operator action is completely different: one is "wait", the other
is "go look at the cable".

### 4.8.3 The plot

Aircraft are drawn over an OpenFreeMap vector basemap as a single GeoJSON
source with a symbol layer — not DOM markers, which are a different performance
class once there are a hundred of them updating every second. `icon-rotate`
reads `track_deg` from the feature, and MapLibre's own collision handling does
the label de-confliction.

The range rings survive on top of the map, as true geodesic circles about the
station. Geography answers "where is that aircraft"; the rings answer "how far
out am I hearing, and in which direction", which is a question about the
antenna and the only one this project is really about.

---

## 4.9 `fixtures/` — the recordings, and the contract around them

### 4.9.1 Why the sidecar exists

The `.cu8` files are gitignored — 2.2 GB of IQ does not belong in git — but
every capture has a **committed `.toml` sidecar**. An unlabelled IQ file with an
assumed sample rate is a silent multi-evening bug: at the wrong rate the
preamble template is the wrong width, detection collapses, and nothing in the
output says why.

Three fixtures, all 2.4 MS/s and 49.6 dB on the same hardware:

| fixture | duration | messages | aircraft | positions | msg/min |
|---|---|---|---|---|---|
| `desk` | 120 s | 167 | 3 | 5 | 83.5 |
| `golden` | 180 s | 517 | 7 | 8 | 172.3 |
| `porch` | 180 s | **1350** | **15** | **105** | **450.0** |

`porch` is the outdoor capture and the better optimization target: 2151
candidate preambles against golden's 879, so there is far more marginal signal
for a detector to actually be tested on. A detector tuned on 517 messages is
being tuned on a signal-starved recording.

### 4.9.2 The receiver-position trap

`bench` evaluates the range gate against the **configured** station, not
against wherever the capture was taken. Scoring the Ottawa fixtures from a Troy
config drops `golden` from 72 positions to 8 — with an identical message count
and an identical digest.

That is the range gate working correctly on the wrong input, and it reads
exactly like a decode regression. `porch.toml` is the first sidecar to record
its own `[receiver]` block for this reason. Making `bench` prefer the sidecar's
position over the operator's config is an open item in Known gaps.

### 4.9.3 Capturing your own

```bash
# rtl_tcp holds the device; stop it and skyward first, or rtl_sdr gets
# "usb_claim_interface error -3".
pkill -f rtl_tcp; pkill -f "skyward run"

# -n counts *samples*; each is 2 bytes (I and Q). 180 s at 2.4 MS/s:
#   180 × 2_400_000 = 432_000_000 samples = 864_000_000 bytes
rtl_sdr -f 1090000000 -s 2400000 -g 49.6 -n 432000000 fixtures/raw/mine.cu8

shasum -a 256 fixtures/raw/mine.cu8
skyward bench fixtures/raw/mine.cu8      # fills in [expected]
```

Then write the sidecar. Copy `porch.toml` as the template — it is the one with
`[receiver]` filled in. Record the UTC start and end, and be honest in
`[hardware]` about anything you did not actually measure: `porch.toml` marks
its antenna configuration `"unrecorded"` rather than guessing, because a
placement comparison against a guessed configuration is worthless.

Leave `[headroom]` out unless you have measured it. `golden` carries one
because two alternatives were genuinely run against it; inventing a target
defeats the purpose of having one.

---

# Part V — The theory you need to improve it

## 5.1 Matched filtering and why correlation wins

The detection problem is: given samples $r[n]$, decide whether a known signal
$s[n]$ is present. For additive white Gaussian noise, the optimal linear filter
is the **matched filter** — correlate against a time-reversed copy of the known
signal. It maximises output SNR, and the maximum is
$$\text{SNR}_\text{out} = \frac{2E}{N_0}$$
where $E$ is the signal energy. Note this depends only on *energy*, not on
pulse shape: a long weak pulse and a short strong one with the same energy
detect equally well.

This is why correlating the whole 8 µs preamble beats sampling four points.
You're integrating ~19 samples of energy instead of 4.

**But we've thrown away phase**, which costs us. Coherent integration of $N$
samples improves SNR by a factor of $N$. Non-coherent integration — summing
magnitudes, which is what we must do post-`abs()` — gives somewhere between
$\sqrt N$ and $N$ depending on the input SNR, approaching $\sqrt N$ when SNR is
low. That gap is the **non-coherent integration loss**, and it is the price of
the simplicity in §2.4.

Recovering some of it is what dump1090's `--phase-enhance` does: retry failed
candidates with a phase-corrected re-slice. That's an advanced exercise, and
the seam for it already exists in `Candidate::frac`.

> **Read more:** Skolnik, *Introduction to Radar Systems*, chapters 2 and 5 —
> radar and ADS-B detection are the same problem and the radar literature is
> far richer. Van Trees, *Detection, Estimation, and Modulation Theory* Part I,
> for the formal treatment.

## 5.2 Detection theory: you are choosing a point on a curve

Every detector has two error modes: missing a real message (probability
$P_M = 1 - P_D$) and firing on noise ($P_{FA}$). Moving the threshold trades
one against the other, and sweeping it traces the **receiver operating
characteristic** — the ROC curve. The Neyman–Pearson lemma says the likelihood
ratio test is optimal: maximise $P_D$ subject to a bound on $P_{FA}$.

In this codebase that abstraction is concrete:

- $P_D$ ↔ `crc_ok`, the headline
- $P_{FA}$ ↔ `candidates_per_message`, the cost
- The threshold ↔ `MIN_PULSE` and `PULSE_TO_SILENCE_RATIO`

**A better detector is one whose entire ROC curve is higher**, not one that
sits at a different point on the same curve. That distinction is exactly why
yield is an anti-metric: sliding along the existing curve changes yield without
improving anything.

When you build the correlator, plot the curve. `skyward bench` gives you one
point; a sweep over your threshold gives you the shape, and the shape is what
tells you whether you actually improved the detector.

## 5.3 Syndrome decoding

Covered algebraically in §4.1.2. The practical recipe:

1. For each error pattern $e$ you want to correct, compute $s(e) = \text{CRC}(e)$
   once and store $s \mapsto e$ in a map.
2. On a failed frame, compute $s(r)$. If it's in the map, XOR the stored
   pattern into $r$ and you're done.

For single-bit errors that's 112 entries. For double-bit, 6,216.

**The precision/recall trap.** $6{,}216 \times 6\times10^{-8} \approx
4\times10^{-4}$ per candidate. Over millions of candidates that's hundreds of
false accepts — each one a plausible-looking ICAO address that will appear on
your map as an aircraft. Two defences, both worth implementing:

- Only flip bits the slicer marked low-confidence (needs stage 3 first).
- Refuse to publish an ICAO seen only once — already implemented as
  `ghost_icao_ratio` on the scoreboard.

Watch that metric rise as you turn correction on. You are doing precision/recall
tradeoff on real data, and it is the most honest lesson in the project.

## 5.4 Quantisation and lookup tables

The magnitude LUT is worth thinking about carefully because it's a nice example
of exploiting structure. $I, Q \in \{0..255\}$ gives $65{,}536$ pairs — a 128 KB
table of `u16`, too big for comfort. But $|I - 127.5|$ takes only 129 distinct
values, and magnitude depends only on the absolute deviations. So the table is
$129 \times 129 = 16{,}641$ entries, 33 KB, comfortably in L2 on both a Mac and
a Pi.

The general principle: before optimising arithmetic, look for a symmetry that
collapses the domain.

---

# Part VI — Antennas, which dominated everything

The measurements from the first evening, all on identical hardware and settings:

| placement | msg/min | 1090 peak | 1090 noise floor | FM 98.5 p50 |
|---|---|---|---|---|
| dipole on tripod, top-floor window, 40 cm from glass | **261** | 113 | 1.6 | — |
| same tripod, desk in an interior room | 95 | 89 | 2.1 | 16.5 |
| **suction-cupped to that same window** | 11 | 16.5 | 1.6 | 60.9 |
| window facing a condo tower | 0.5 | 12.6 | 1.6 | 37.1 |

Software changes in this project move reception by tens of percent. **Antenna
placement moved it 24×.** Three things worth understanding.

## 6.1 Why the dipole needs both elements

A dipole is two quarter-wave elements fed differentially. Install only one and
you have a monopole with no counterpoise — the coax braid becomes the missing
half, radiating in an uncontrolled pattern with a bad impedance match. Adding
the second element took us from ~24 to 95 msg/min.

For 1090 MHz: 6.88 cm per element, 13.75 cm tip to tip, **vertical**. ADS-B is
vertically polarised; a cross-polarised receive antenna costs on the order of
20 dB.

## 6.2 Why you must not mount it against glass

This is the one that surprised me, and the reasoning is the most interesting
physics in the project.

At 1090 MHz a resonant dipole's **near field** extends only about
$\lambda/2\pi = 4.4$ cm. Suction-cup it to a low-emissivity coated pane and the
conductive layer sits inside that near field, where it detunes the antenna and
loads it resistively. The result: 11 msg/min against 261 for the same antenna
40 cm back in free air.

The clean evidence is that **the noise floor didn't change** — 1.6 either way —
while peak burst magnitude went 16.5 → 113. A detuned antenna couples less of
*everything*; it doesn't hear less noise, it hears less signal.

At 98.5 MHz the same 15 cm dipole is electrically tiny (a capacitive probe,
$\ell \ll \lambda$) and barely notices the coating, which is why FM went *up*
in the same position.

> I initially dismissed low-E coating on the grounds that a resistive sheet
> attenuates roughly equally across frequency. That's true for **far-field
> transmission** and completely beside the point for an antenna sitting in its
> own near field. Worth remembering as a lesson about applying the right model.

## 6.3 The FM control

The most useful diagnostic in the project, and it costs one command:

- **Both bands drop** → the cable, the connector, or the dongle.
- **FM rises while 1090 drops** → geometry: obstruction, or near-field loading.

It works because a 3 m wavelength diffracts around buildings and ignores small
conductive layers, while a 27.5 cm wavelength does neither.

> **Read more:** Balanis, *Antenna Theory: Analysis and Design*, chapters 2 and
> 4 for dipoles and near/far-field regions. The *ARRL Antenna Book* is far more
> practical and has a good treatment of ground planes and counterpoises. For
> propagation, Rappaport, *Wireless Communications: Principles and Practice*,
> chapter 4.

---

# Part VII — References

## Specifications

- **ICAO Annex 10, Volume IV** — *Aeronautical Telecommunications: Surveillance
  and Collision Avoidance Systems*. Treaty-level; the normative waveform spec.
- **ICAO Doc 9871** — *Technical Provisions for Mode S Services and Extended
  Squitter*. The practical spec: registers, CPR, type codes. **Start here.**
- **RTCA DO-260B** / **EUROCAE ED-102A** — ADS-B 1090ES MOPS.
- **RTCA DO-181E** — Mode S transponder MOPS (transmitter side).

## Books

- **Junzi Sun, *The 1090 Megahertz Riddle*** (2nd ed.) — free at
  `mode-s.org/decode`. Open access, worked examples, the single best entry
  point. Our canonical test vectors come from here.
- **Michael Stevens, *Secondary Surveillance Radar*** (Artech House) — history
  and system design.
- **Richard Lyons, *Understanding Digital Signal Processing***, 3rd ed. —
  chapter 8 on quadrature signals is the best treatment of the IQ gap.
- **Proakis & Salehi, *Digital Communications***, 5th ed. — modulation,
  detection, the complex baseband formalism.
- **Skolnik, *Introduction to Radar Systems***, 3rd ed. — pulse detection,
  integration gain, the radar equation.
- **Van Trees, *Detection, Estimation, and Modulation Theory*, Part I** — the
  formal detection theory behind §5.2.
- **Lin & Costello, *Error Control Coding***, 2nd ed. — cyclic codes, syndrome
  decoding.
- **Balanis, *Antenna Theory*** — near/far field, dipoles.
- **Rappaport, *Wireless Communications***, 2nd ed. — propagation, link budgets.

## Papers and reports

- V. A. Orlando, "The Mode S Beacon Radar System", *Lincoln Laboratory Journal*
  2(3), 1989. Free PDF, very readable, explains *why* Mode S looks like this.
- Peterson & Brown, "Cyclic Codes for Error Detection", *Proc. IRE*, 1961.
- Search "ADS-B security" for the (substantial) literature on spoofing — the
  system has no authentication and this is a live research area.

## Code worth reading

- **dump1090** — `github.com/antirez/dump1090` (original) and
  `github.com/flightaware/dump1090` (maintained). The reference implementation.
  Read `demodulate2400.c` for the hand-derived 2.4 MS/s slicer and
  `crc.c` for the syndrome tables.
- **pyModeS** — `github.com/junzis/pyModeS`. Clear, readable Python
  implementations of everything in `adsb-core`. Excellent for cross-checking.
- **readsb** — `github.com/wiedehopf/readsb`. A heavily optimised fork; look
  here for what production-grade detection looks like.
- **osmocom rtl-sdr** — `osmocom.org/projects/rtl-sdr/wiki`. The `rtl_tcp.c`
  source is the normative definition of the protocol in `tcp.rs`.

## Data and tools

- `mode-s.org` — Junzi Sun's site: the book, decoders, and a message
  playground.
- `adsbexchange.com` — unfiltered aggregated ADS-B, useful for cross-checking
  what you should be seeing.
- `github.com/wiedehopf/adsb-wiki` — the practical community wiki on antennas,
  filters and gain tuning. Genuinely good.

---

# Appendix — A suggested study path

If you want an order, this is the one I'd take.

**Session 1: read and run.** Work through §2.1–2.4 and §3.1–3.3 with the code
open. Run `skyward decode` on a few hex messages from the fixtures and match
each field to the frame diagram by hand. Then read `bits.rs` and `frame.rs` in
full — they're short.

**Session 2: the CRC.** Read §4.1.2, then `crc.rs` including its tests. Compute
a syndrome by hand for a single-bit error and confirm it against
`crc.rs:101-117`. You'll then understand stage 4 completely before writing any
of it.

**Session 3: CPR.** Read §4.1.5 and derive the zone-index cancellation
yourself; it takes about ten lines of algebra and it's satisfying. Then read
`cpr.rs` and the round-trip test.

**Session 3.5: read the registry before writing any DSP.** §4.7. It is 246
lines and it is the mechanism the whole project depends on — how a new
implementation lands beside the old one rather than replacing it. Note the gap
in §4.7.4 before you plan around the flags in `--list-impls`: they are not
implemented, and the route today is a preset plus `--impl-set`.

**Session 4: build the correlator.** This is the big win (~4×). The five
weaknesses are enumerated at `detect.rs:36-59`. Register it (§4.7.3), then

```bash
skyward bench --impl-set thomas --compare runs/baseline.json
```

Sweep your threshold to plot the ROC curve rather than reporting a single
point. **Score it on `porch`, not `golden`** — 2151 candidates against 879 is
far more marginal signal to be tested on, and a detector tuned on a
signal-starved recording learns the wrong lesson. Watch `ghost_icao_ratio`
against *that fixture's own* baseline of 0.133, not against zero (§4.6.2).

**Session 5: the slicer, then error correction.** Integrating over half-bits is
worth ~17% and is easy. Emitting real confidence is what unlocks safe two-bit
correction, so do them in that order — and watch `ghost_icao_ratio` while you
do.

**Session 6: capture your own fixture.** §4.9.3. Everything above is
measured against recordings someone else made; a capture from your own antenna
in your own sky is the point at which the scoreboard starts describing *your*
receiver. Outdoors beat every indoor placement by a factor of 2.6 (§4.9.1),
which is a larger improvement than the correlator is expected to deliver.

**Before any of that, if the dongle is to hand: the gain sweep.** It's stage 0
in the plan for a reason. Max gain is very likely not optimal — it amplifies
noise equally and drives the tuner into compression when a nearby aircraft
transmits. Real stations often peak at 40–45 dB. Expect 95 → 150–250 msg/min
from gain and placement alone, before a single line of DSP changes. Measurement
precedes optimisation.
