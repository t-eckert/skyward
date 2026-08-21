/**
 * The wire contract with `adsb-server`.
 *
 * Field names and units mirror `crates/adsb-server/src/api.rs` exactly. Three
 * of that module's contract notes shape everything here:
 *
 * - Every envelope carries `now_ms`, the *server's* clock. Ages are computed
 *   against it, never against `Date.now()`, or a laptop forty seconds out of
 *   sync renders every aircraft as stale.
 * - All times are epoch milliseconds, never formatted strings.
 * - It is `track_deg`: ADS-B reports ground track, which differs from heading
 *   by the wind correction angle.
 */

/** One aircraft, as the server currently understands it. */
export interface Aircraft {
	icao: string;
	callsign?: string;
	lat?: number;
	lon?: number;
	position_age_ms?: number;
	position_source?: string;
	altitude_ft?: number;
	altitude_source?: string;
	ground_speed_kt?: number;
	track_deg?: number;
	vertical_rate_fpm?: number;
	on_ground: boolean;
	category?: string;
	messages: number;
	first_seen_ms: number;
	last_seen_ms: number;
	/** Milliseconds since this aircraft was last heard, at `now_ms`. */
	seen_ms: number;
	rssi_dbfs?: number;
}

export interface AircraftEnvelope {
	now_ms: number;
	count: number;
	aircraft: Aircraft[];
}

/** A station position, as `PUT /api/v1/receiver` accepts it. */
export interface StationPosition {
	lat: number;
	lon: number;
	altitude_m: number;
}

export interface Receiver {
	lat: number | null;
	lon: number | null;
	altitude_m: number;
	/**
	 * Where the position in force came from: `unset`, a config origin like
	 * `$SKYWARD_RECEIVER_LAT`, the overlay file's path, or `set at runtime`.
	 *
	 * Shown verbatim in the station dialog. The whole reason the server tracks
	 * provenance is so the answer to "did my edit take effect" is visible, and
	 * hiding it in the client would throw that away at the last step.
	 */
	origin: string;
	/** False when the receiver was started with `station_writable = false`. */
	writable: boolean;
	/** What a revert would go back to, if configuration supplies anything. */
	configured: (StationPosition & { origin: string }) | null;
	station: string;
	version: string;
	uptime_s: number;
	sample_rate_hz: number;
	frequency_hz: number;
	impl_set: string;
	source: string;
}

export interface Stats {
	aircraft: number;
	candidates: number;
	configured_sample_rate_hz: number;
	crc_fail: number;
	crc_ok: number;
	effective_sample_rate_hz: number;
	impl_set: string;
	now_ms: number;
	positions: number;
	reconnects: number;
	samples: number;
	uptime_s: number;
	store: {
		dropped: number;
		errors: number;
		retention_deleted: number;
		transactions: number;
		written: number;
	};
}

export interface TrackPoint {
	ts_ms: number;
	lat: number;
	lon: number;
	altitude_ft: number | null;
	ground_speed_kt: number | null;
	track_deg: number | null;
}

export interface TrackResponse {
	now_ms: number;
	count: number;
	track: TrackPoint[];
}

export interface Health {
	status: 'ok' | 'degraded' | string;
	uptime_s: number;
	version: string;
	warnings: string[];
	source: {
		description: string;
		state: string;
		effective_sample_rate_hz: number;
		last_sample_age_ms: number;
		reconnects: number;
	};
	decode: {
		aircraft: number;
		last_message_age_s: number;
		messages: number;
		positions: number;
	};
	store: { dropped: number; enabled: boolean; errors: number; written: number };
}

/**
 * Same origin, always.
 *
 * In development Vite proxies `/api` to the server; in production the built
 * assets sit beside it. Nothing here ever needs to know a host, which is why
 * no CORS preflight ever happens.
 */
async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
	const response = await fetch(path, { signal });
	if (!response.ok) {
		throw new Error(`${path} responded ${response.status}`);
	}
	return (await response.json()) as T;
}

/**
 * A mutating request whose error body is worth reading.
 *
 * The server answers a rejected position with `400` and an `error` naming the
 * field and the range — "lat 91 is not a latitude (-90 to 90)". Throwing
 * `responded 400` instead would replace a sentence the operator can act on
 * with a number they cannot.
 */
async function send<T>(path: string, method: 'PUT' | 'DELETE', body?: unknown): Promise<T> {
	const response = await fetch(path, {
		method,
		headers: body === undefined ? undefined : { 'content-type': 'application/json' },
		body: body === undefined ? undefined : JSON.stringify(body)
	});
	const text = await response.text();
	let parsed: unknown = null;
	try {
		parsed = text ? JSON.parse(text) : null;
	} catch {
		// Fall through to the status-code message below.
	}
	if (!response.ok) {
		const message =
			parsed && typeof parsed === 'object' && 'error' in parsed
				? String((parsed as { error: unknown }).error)
				: `${path} responded ${response.status}`;
		throw new Error(message);
	}
	return parsed as T;
}

export const api = {
	receiver: (signal?: AbortSignal) => get<Receiver>('/api/v1/receiver', signal),

	/**
	 * Move the station. Resolves to the same shape `receiver()` returns, so
	 * the caller applies the response rather than re-fetching and racing its
	 * own write.
	 */
	setReceiver: (position: StationPosition) =>
		send<Receiver>('/api/v1/receiver', 'PUT', position),

	/** Discard a runtime position and go back to what configuration says. */
	clearReceiver: () => send<Receiver>('/api/v1/receiver', 'DELETE'),
	stats: (signal?: AbortSignal) => get<Stats>('/api/v1/stats', signal),
	health: (signal?: AbortSignal) => get<Health>('/healthz', signal),
	aircraft: (signal?: AbortSignal) => get<AircraftEnvelope>('/api/v1/aircraft', signal),

	/**
	 * The recorded track for one aircraft, oldest fix first.
	 *
	 * The server orders newest-first internally so `limit` keeps the recent
	 * tail, then reverses -- so this can be drawn as a line without sorting.
	 */
	track: (icao: string, sinceMs: number, limit = 2000, signal?: AbortSignal) =>
		get<TrackResponse>(
			`/api/v1/aircraft/${encodeURIComponent(icao)}/track?since_ms=${Math.floor(sinceMs)}&limit=${limit}`,
			signal
		)
};
