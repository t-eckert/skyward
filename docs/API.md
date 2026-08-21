# The HTTP API

Every endpoint, with responses captured from a running receiver rather than written from memory.

Base URL is wherever `bind` points — `http://raspberrypi.local:8080` on a typical Pi. There is no authentication; see [Security](CONFIGURATION.md#security).

## Contract notes

Four conventions run through everything below, and each exists because of a specific failure.

**Every envelope carries `now_ms`, the server's clock.** Compute ages against it, never against `Date.now()`. A laptop forty seconds out of sync would otherwise render every aircraft as stale.

**All times are epoch milliseconds.** Never a formatted string. An earlier schema's `"2026-08-06 12:00:00"` is parsed as *local* time by V8 and is `Invalid Date` in older Safari.

**Units are in the field names**, and it is `track_deg` — ADS-B reports ground track, which differs from heading by the wind correction angle.

**An unknown `/api/...` path is a JSON 404, not the app.** The web client is served from a router fallback, which otherwise catches every unmatched path. Without a reserved-prefix list, `GET /api/v1/aircaft` returned `200` and a page of HTML, so `fetch` resolved successfully and the client fell over parsing `<!doctype html>` as JSON — a long way from the URL that was actually wrong.

Optional fields are **omitted**, not null. An aircraft that has not reported an altitude has no `altitude_ft` key; it is not at sea level.

---

## `GET /healthz`

Freshness, not liveness. Returns **`200`** for `ok` and `degraded`, **`503`** for `stalled`, so a bare `curl -f` is a valid probe.

```json
{
  "status": "ok",
  "version": "0.1.0",
  "uptime_s": 12,
  "source": {
    "state": "streaming",
    "description": "file:fixtures/raw/golden.cu8 at 2.400 MS/s (Realtime, looping)",
    "reconnects": 0,
    "effective_sample_rate_hz": 2395876.178,
    "overrun_bytes": 0,
    "last_sample_age_ms": 18
  },
  "decode": {
    "last_message_age_s": 0,
    "messages": 37,
    "positions": 4,
    "aircraft": 4
  },
  "store": { "enabled": true, "written": 8, "dropped": 0, "errors": 0 },
  "warnings": []
}
```

| Field | Meaning |
|---|---|
| `status` | `ok`, `degraded` (storage is failing but decoding continues), or `stalled` (no samples for 30 s, or the source is down) |
| `source.state` | `streaming` or `down` |
| `source.effective_sample_rate_hz` | Samples actually consumed per second. Below the configured rate means the decoder is not keeping up — which looks exactly like bad reception in the message count, and is the half software can fix |
| `source.overrun_bytes` | Bytes the *source* discarded before skyward saw them. Only a buffering source (USB) can be non-zero. This is the number that catches a loss the effective rate hides |
| `decode.last_message_age_s` | `-1` when nothing has ever been decoded |
| `warnings` | Prose, ready to show a person: an unsynced clock, dropped rows, write errors, overruns |

The one thing this endpoint deliberately does *not* do is report `ok` because the process is up. An earlier version answered `ok` whenever SQLite was readable, which stayed true for hours after the decoder died.

## `GET /readyz`

```json
{ "ready": true }
```

`200` once a snapshot has been published, `503` before that with a `reason`. Use it as a startup probe; use `/healthz` as a liveness probe.

---

## `GET /api/v1/receiver`

Station identity and position, plus everything needed to offer to change it.

```json
{
  "lat": 45.421,
  "lon": -75.697,
  "altitude_m": 70.0,
  "origin": "$SKYWARD_RECEIVER_LAT",
  "writable": true,
  "configured": {
    "lat": 45.421, "lon": -75.697, "altitude_m": 70.0,
    "origin": "$SKYWARD_RECEIVER_LAT"
  },
  "station": "skyward",
  "version": "0.1.0",
  "uptime_s": 12,
  "sample_rate_hz": 2400000,
  "frequency_hz": 1090000000,
  "impl_set": "mag=naive detect=naive slice=naive validate=crc-only",
  "source": "file:fixtures/raw/golden.cu8 at 2.400 MS/s (Realtime, looping)"
}
```

`lat` and `lon` are `null` when no position is set anywhere.

`origin` is where the position in force came from: `unset`, a config origin like `$SKYWARD_RECEIVER_LAT` or `skyward.toml`, the overlay file's path, or `set at runtime`. `configured` is what a revert would go back to, and is `null` when configuration supplies nothing.

The point of serving the position at all is that the map's range ring is not hardcoded in the client, so the same client works against any station.

## `PUT /api/v1/receiver`

Move the station. Takes effect within a second — the tracker adopts the new range gate without rebuilding, so nothing currently tracked is lost — and is persisted so it survives a restart.

```bash
curl -X PUT http://localhost:8080/api/v1/receiver \
  -H 'content-type: application/json' \
  -d '{"lat": 45.421, "lon": -75.697, "altitude_m": 70}'
```

`altitude_m` is optional and defaults to 0. It is metres above sea level.

The response body is identical to `GET`, so a client can apply it directly instead of re-fetching and racing its own write.

| Status | When |
|---|---|
| `200` | Accepted, applied and persisted |
| `400` | Not a point on the earth, or the persistence write failed |
| `403` | `station_writable = false` |

A rejected value is never silently clamped. Clamping 91° to 90° would put the station at the pole and produce a receiver that hears nothing, for a reason nobody would think to look for:

```json
{ "error": "lat 91 is not a latitude (-90 to 90)" }
```

`0, 0` is accepted — it is a real coordinate in the Gulf of Guinea, and the API should not second-guess someone who types it. Unsetting is what `DELETE` is for.

Note that a position set this way **outranks the config file and the environment** from then on. See [The station overlay](CONFIGURATION.md#the-station-overlay).

## `DELETE /api/v1/receiver`

Discard a runtime position, delete the overlay file, and revert to whatever configuration says — which may be nothing. Returns the same body as `GET`, or `403` when writes are disabled.

---

## `GET /api/v1/aircraft`

Everything currently tracked, served from the in-memory snapshot the decoder publishes. No locks, no database.

| Query | Meaning |
|---|---|
| `filter=with_position` | Only aircraft that can be drawn |
| `max_age_ms=N` | Drop anything not heard within N milliseconds |

```json
{
  "now_ms": 1787273522614,
  "count": 3,
  "aircraft": [
    {
      "icao": "C0583A",
      "callsign": "ACA906",
      "lat": 45.421158095537606,
      "lon": -75.2460312261814,
      "position_age_ms": 4585,
      "position_source": "global_cpr",
      "altitude_ft": 37000,
      "altitude_source": "baro",
      "ground_speed_kt": 544,
      "track_deg": 54.11968,
      "vertical_rate_fpm": 0,
      "on_ground": false,
      "category": "4-5",
      "messages": 28,
      "first_seen_ms": 1787273502841,
      "last_seen_ms": 1787273522614,
      "seen_ms": 0,
      "rssi_dbfs": -11.74095
    }
  ]
}
```

| Field | Notes |
|---|---|
| `icao` | Six hex digits. Only addresses proved real by a CRC-clean DF17/18 appear at all — the ghost defence |
| `position_source` | `global_cpr`, `local_cpr`, or `surface`. Worth surfacing: a local fix inherits the error of its reference |
| `altitude_source` | `baro` or `gnss`. They differ by hundreds of feet and are not interchangeable |
| `seen_ms` | Milliseconds since last heard, **as of `now_ms`** |
| `rssi_dbfs` | Negative, relative to full scale |

Aircraft leave the snapshot 60 seconds after their last message.

## `GET /api/v1/aircraft/{icao}`

One aircraft, `{ "now_ms": ..., "aircraft": {...} }`, or `404`:

```json
{ "error": "000000 is not currently tracked" }
```

## `GET /api/v1/aircraft/{icao}/track`

The recorded track, from SQLite, **oldest fix first** so it can be drawn as a line without sorting.

| Query | Default | Notes |
|---|---|---|
| `since_ms` | `0` | Epoch milliseconds |
| `limit` | `500` | Capped at 10 000 |

```json
{
  "now_ms": 1787273522751,
  "count": 2,
  "track": [
    { "ts_ms": 1787273517039, "lat": 45.41966842392743, "lon": -75.24891178782389,
      "altitude_ft": 37000, "ground_speed_kt": 544, "track_deg": 54.11967849731445 }
  ]
}
```

The query runs newest-first internally so `limit` keeps the recent tail, then reverses. It happens inside `spawn_blocking`, so a slow SD card parks a blocking thread rather than a tokio worker.

How much history exists depends on `retention_hours` (24 by default) and on the movement filter, which only writes a row when an aircraft has actually moved.

---

## `GET /api/v1/stream`

Server-sent events: one full snapshot per second.

```
event: snapshot
data: {"now_ms":1787273522614,"count":4,"aircraft":[...]}
```

The payload is exactly the `GET /api/v1/aircraft` envelope.

```js
const source = new EventSource('/api/v1/stream');
source.addEventListener('snapshot', (e) => {
  const envelope = JSON.parse(e.data);
});
```

SSE rather than WebSocket because the traffic is one-way, `EventSource` reconnects by itself, it survives proxies, and it is about five lines in SvelteKit. There are no client-to-server messages, so an upgrade handshake would buy nothing.

**A dead stream does not necessarily raise an error.** With the receiver stopped behind a proxy the socket stayed open, `onerror` never fired, and a client happily reported STREAMING over a sixteen-second-old snapshot — ages included, because they are computed against the server clock in the envelope, which had stopped advancing too. Treat silence as failure: the bundled client declares the stream dead after six seconds without a snapshot, and takes over `EventSource`'s own retry so it can show a real countdown.

And note what a live stream proves: that the *server* is alive. Only `/healthz` knows whether samples are still arriving from the tuner. An unplugged antenna produces snapshots on schedule that are simply empty.

## `GET /api/v1/stats`

Decoder counters, polled by the client's scoreboard.

```json
{
  "now_ms": 1787273514529,
  "uptime_s": 12,
  "samples": 28835840,
  "candidates": 53,
  "crc_ok": 37,
  "crc_fail": 16,
  "positions": 4,
  "aircraft": 4,
  "reconnects": 0,
  "effective_sample_rate_hz": 2395876.178,
  "source_overrun_bytes": 0,
  "configured_sample_rate_hz": 2400000,
  "impl_set": "mag=naive detect=naive slice=naive validate=crc-only",
  "store": { "written": 8, "dropped": 0, "errors": 0,
             "transactions": 5, "retention_deleted": 0 }
}
```

Two derived figures are worth computing, and one of them is a trap:

**Realtime factor** — `samples / (configured_sample_rate_hz × uptime_s)`. Below 1.0 means samples are being dropped, which looks like bad reception and is software.

**Yield** — `crc_ok / candidates`. **Report it; never optimise it.** A better detector proposes more marginal candidates, so yield *falls* as messages rise. Tuning for yield tunes the detector backwards.

---

## Everything else

Any path that is not an endpoint above and does not start with `api/`, `healthz` or `readyz` is answered with the web client's `index.html`. That is what makes a deep link work: `/aircraft/A0A41F` is a client route with no file behind it, so a reload has to be resolved in the browser.

`index.html` is served `Cache-Control: no-cache`; everything under `_app/immutable/` is content-hashed and served `immutable` for a year. A cached `index.html` would point at asset names that no longer exist after an upgrade, and the app would fail to boot.
