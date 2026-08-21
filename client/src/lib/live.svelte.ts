import {
	api,
	type Aircraft,
	type AircraftEnvelope,
	type Health,
	type Receiver,
	type StationPosition,
	type Stats
} from './api';

/**
 * The live connection to the receiver, and everything derived from it.
 *
 * # Why the last snapshot is kept on disconnect
 *
 * When the stream drops, this deliberately keeps the last aircraft it saw and
 * marks the whole view frozen, rather than clearing to an empty map. An empty
 * map is indistinguishable from "no traffic", which is the one reading that
 * must never be ambiguous: one means the sky is quiet, the other means the
 * receiver is gone. The banner and the frozen timestamp say which.
 *
 * # Ages come from the server's clock
 *
 * `now_ms` rides in every envelope. Ages are computed against it while
 * connected, and against the freeze point once disconnected -- so a stale view
 * visibly ages instead of sitting at a comfortable 0.4 s forever.
 */

export type Connection = 'connecting' | 'streaming' | 'disconnected';

/** How long to wait before each reconnect attempt, in seconds. */
const BACKOFF_S = [1, 2, 3, 5, 8, 13, 20, 30];

/** The window over which the live message rate is averaged. */
const RATE_WINDOW_MS = 60_000;

/**
 * Treat the stream as dead after this long with no snapshot.
 *
 * The server publishes one per second, so six is a wide margin. This exists
 * because **a dead stream does not necessarily raise an error**: when the
 * receiver was stopped behind a proxy, the socket stayed open, `onerror`
 * never fired, and the client happily reported STREAMING with a sixteen
 * second old snapshot -- ages included, because they are computed against the
 * server clock in the envelope, which had stopped advancing too.
 *
 * That is the same lie as a health endpoint that answers `ok` because SQLite
 * is readable while the decoder has been dead for hours. Silence has to be
 * treated as failure; only an arriving snapshot proves the stream is alive.
 */
const SNAPSHOT_TIMEOUT_MS = 6_000;

interface RateSample {
	t: number;
	messages: number;
}

export class Live {
	/** The most recent snapshot, retained across a disconnect. */
	envelope = $state<AircraftEnvelope | null>(null);
	receiver = $state<Receiver | null>(null);
	stats = $state<Stats | null>(null);
	health = $state<Health | null>(null);

	connection = $state<Connection>('connecting');
	/** When the stream last delivered a snapshot. */
	lastSnapshotMs = $state<number | null>(null);
	/** Wall-clock at which the connection dropped. */
	disconnectedAtMs = $state<number | null>(null);
	/** Seconds until the next reconnect attempt. */
	retryInS = $state<number>(0);

	/** The ICAO the operator has selected, if any. */
	selected = $state<string | null>(null);

	/** Ticks once a second so ages re-render even when no snapshot arrives. */
	tick = $state<number>(Date.now());

	/**
	 * Messages per minute over the recent window.
	 *
	 * Held as state rather than derived from the sample buffer: the buffer is
	 * a plain private field, so a getter over it would never re-run and the
	 * headline figure would sit at its first value forever.
	 */
	messagesPerMinute = $state<number | null>(null);

	/** The best messages-per-minute this session has seen. */
	peakRate = $state<number>(0);
	/** The last aircraft heard, kept so a quiet sky can say what ended it. */
	lastHeard = $state<{ label: string; atMs: number } | null>(null);

	#source: EventSource | null = null;
	#attempt = 0;
	#timers: ReturnType<typeof setInterval>[] = [];
	#reconnect: ReturnType<typeof setTimeout> | null = null;
	#rate: RateSample[] = [];
	#stopped = false;

	start() {
		this.#stopped = false;
		this.#connect();
		void this.#pollMeta();

		this.#timers.push(setInterval(() => void this.#pollMeta(), 5_000));
		this.#timers.push(
			setInterval(() => {
				this.tick = Date.now();
				if (this.retryInS > 0) this.retryInS -= 1;
				this.#checkForSilence();
			}, 1_000)
		);
	}

	stop() {
		this.#stopped = true;
		this.#source?.close();
		this.#source = null;
		if (this.#reconnect) clearTimeout(this.#reconnect);
		this.#timers.forEach(clearInterval);
		this.#timers = [];
	}

	/** Abandon the backoff and try again immediately. */
	retryNow() {
		if (this.#reconnect) clearTimeout(this.#reconnect);
		this.#attempt = 0;
		this.retryInS = 0;
		this.#connect();
	}

	#connect() {
		if (this.#stopped) return;
		this.#source?.close();
		this.connection = this.envelope ? 'connecting' : 'connecting';

		const source = new EventSource('/api/v1/stream');
		this.#source = source;

		source.addEventListener('snapshot', (event) => {
			const envelope = JSON.parse((event as MessageEvent).data) as AircraftEnvelope;
			this.envelope = envelope;
			this.lastSnapshotMs = Date.now();
			this.disconnectedAtMs = null;
			this.connection = 'streaming';
			this.#attempt = 0;

			const newest = envelope.aircraft.reduce<Aircraft | null>(
				(best, a) => (best === null || a.last_seen_ms > best.last_seen_ms ? a : best),
				null
			);
			if (newest) {
				this.lastHeard = {
					label: newest.callsign?.trim() || newest.icao,
					atMs: newest.last_seen_ms
				};
			}
		});

		// EventSource retries on its own schedule, which cannot be read or
		// displayed. Take it over so the banner can show a real countdown and
		// the operator can skip it.
		source.onerror = () => this.#drop(Date.now());
	}

	/**
	 * The stream has stopped being trustworthy. Freeze, and schedule a retry.
	 *
	 * `since` is when the last good snapshot arrived, not when the failure was
	 * noticed -- otherwise a silent death under-reports the outage by however
	 * long the watchdog took to fire.
	 */
	#drop(since: number) {
		this.#source?.close();
		this.#source = null;
		if (this.#reconnect) clearTimeout(this.#reconnect);
		if (this.#stopped) return;

		this.connection = 'disconnected';
		this.disconnectedAtMs ??= since;

		const wait = BACKOFF_S[Math.min(this.#attempt, BACKOFF_S.length - 1)];
		this.#attempt += 1;
		this.retryInS = wait;
		this.#reconnect = setTimeout(() => this.#connect(), wait * 1000);
	}

	/** A stream that has gone quiet is a dead stream, error or no error. */
	#checkForSilence() {
		if (this.connection !== 'streaming' || this.lastSnapshotMs === null) return;
		const silentFor = Date.now() - this.lastSnapshotMs;
		if (silentFor > SNAPSHOT_TIMEOUT_MS) this.#drop(this.lastSnapshotMs);
	}

	async #pollMeta() {
		try {
			const [receiver, stats, health] = await Promise.all([
				api.receiver(),
				api.stats(),
				api.health()
			]);
			this.receiver = receiver;
			this.stats = stats;
			this.health = health;
			this.#recordRate(stats);
		} catch {
			// The stream's own error handling owns the connection state; a
			// failed metadata poll while streaming is not worth a banner.
		}
	}

	#recordRate(stats: Stats) {
		const now = Date.now();
		this.#rate.push({ t: now, messages: stats.crc_ok });
		while (this.#rate.length > 2 && now - this.#rate[0].t > RATE_WINDOW_MS) {
			this.#rate.shift();
		}
		const rate = this.#computeRate();
		this.messagesPerMinute = rate;
		if (rate !== null && rate > this.peakRate) this.peakRate = rate;
	}

	/**
	 * A rolling rate, not `crc_ok / uptime`: the lifetime average of a
	 * receiver that has been up for six days tells you nothing about whether
	 * it is working right now.
	 */
	#computeRate(): number | null {
		if (this.#rate.length < 2) return null;
		const first = this.#rate[0];
		const last = this.#rate[this.#rate.length - 1];
		const seconds = (last.t - first.t) / 1000;
		if (seconds <= 0) return null;
		return ((last.messages - first.messages) / seconds) * 60;
	}

	/**
	 * The radio itself has stopped, even though the server is answering.
	 *
	 * This is a different failure from a dropped stream and has to be shown as
	 * one. When the antenna was unplugged, snapshots kept arriving on schedule
	 * — they were simply empty — so the stream looked perfectly healthy and
	 * the view reported STREAMING over a sky that could not be heard. The
	 * server knew (`status: stalled`, `source.state: down`); nothing asked it.
	 *
	 * A live SSE connection only proves the *server* is alive. Only the health
	 * endpoint knows whether samples are still arriving from the tuner.
	 */
	get sourceDown(): boolean {
		if (this.connection === 'disconnected') return false;
		const source = this.health?.source;
		return source !== undefined && source.state !== 'streaming';
	}

	/** What the header and banner should say, in order of severity. */
	get viewState(): 'streaming' | 'source-down' | 'disconnected' {
		if (this.connection === 'disconnected') return 'disconnected';
		if (this.sourceDown) return 'source-down';
		return 'streaming';
	}

	/** How long the tuner has been silent, per the server. */
	get sourceSilentMs(): number | null {
		const age = this.health?.source.last_sample_age_ms;
		return age === undefined ? null : age;
	}

	/** The reference clock for ages: the server's, frozen at disconnect. */
	get nowMs(): number {
		if (this.connection === 'disconnected' && this.envelope) {
			// Advance the server's last-known clock by the wall time since the
			// freeze, so a stale aircraft keeps visibly ageing.
			const frozenFor = this.disconnectedAtMs ? this.tick - this.disconnectedAtMs : 0;
			return this.envelope.now_ms + Math.max(0, frozenFor);
		}
		return this.envelope?.now_ms ?? this.tick;
	}

	/** How long the source has been unreachable, in milliseconds. */
	get outageMs(): number | null {
		if (this.connection !== 'disconnected' || this.disconnectedAtMs === null) return null;
		return Math.max(0, this.tick - this.disconnectedAtMs);
	}

	/**
	 * Move the station.
	 *
	 * The response is applied directly rather than triggering a re-fetch: the
	 * metadata poll runs every five seconds, so re-fetching would leave the
	 * header showing the old position for up to that long after a save the
	 * user just watched succeed — and would race the poll besides.
	 *
	 * Errors are re-thrown for the caller to render. Swallowing them here
	 * would leave a dialog that closes on a rejected position, which reads as
	 * success.
	 */
	async setStation(position: StationPosition): Promise<void> {
		this.receiver = await api.setReceiver(position);
	}

	/** Discard a runtime position and revert to the configured one. */
	async clearStation(): Promise<void> {
		this.receiver = await api.clearReceiver();
	}

	/** True when nothing anywhere has told the receiver where it is. */
	get stationUnset(): boolean {
		return this.receiver !== null && (this.receiver.lat === null || this.receiver.lon === null);
	}

	get aircraft(): Aircraft[] {
		return this.envelope?.aircraft ?? [];
	}

	/** Aircraft that can actually be drawn, farthest-stale last. */
	get positioned(): Aircraft[] {
		return this.aircraft.filter((a) => a.lat !== undefined && a.lon !== undefined);
	}

	get selectedAircraft(): Aircraft | null {
		if (this.selected === null) return null;
		return this.aircraft.find((a) => a.icao === this.selected) ?? null;
	}

	/**
	 * The fraction of candidate preambles that produced a valid message.
	 *
	 * Reported, never optimised. A better detector proposes more marginal
	 * candidates, so yield *falls* as messages rise -- tuning for it tunes the
	 * detector backwards.
	 */
	get yieldPct(): number | null {
		const s = this.stats;
		if (!s || s.candidates === 0) return null;
		return (s.crc_ok / s.candidates) * 100;
	}

	/**
	 * Samples consumed versus samples the radio should have produced.
	 *
	 * Below 1.0 means the decoder is not keeping up and samples are being
	 * dropped -- which looks exactly like bad reception in the message count,
	 * and is the one of the two that software can fix.
	 */
	get realtimeFactor(): number | null {
		const s = this.stats;
		if (!s || s.uptime_s <= 0 || s.configured_sample_rate_hz <= 0) return null;
		return s.samples / (s.configured_sample_rate_hz * s.uptime_s);
	}
}
