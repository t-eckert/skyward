/**
 * Sample wire data for Storybook, shaped exactly like `lib/api.ts`.
 *
 * Story-only: nothing in the app imports this, so it tree-shakes out of the
 * shipped bundle. The values are a plausible snapshot of the Troy capture — a
 * handful of aircraft at cruise, one without a callsign, one going stale — so a
 * component under tune shows the cases that actually differ visually.
 */
import type { Aircraft, AircraftEnvelope, Health, Receiver, Stats, TrackPoint } from './api';

const NOW = 1_786_983_000_000;

export const receiver: Receiver = {
	lat: 41.788,
	lon: -76.79,
	altitude_m: 340,
	origin: '$SKYWARD_RECEIVER_LAT',
	writable: true,
	configured: {
		lat: 41.788,
		lon: -76.79,
		altitude_m: 340,
		origin: '$SKYWARD_RECEIVER_LAT'
	},
	station: 'porch',
	version: '0.1.0',
	uptime_s: 62_460,
	sample_rate_hz: 2_400_000,
	frequency_hz: 1_090_000_000,
	impl_set: 'baseline',
	source: 'tcp:127.0.0.1:1234 (R820T tuner, 29 gain steps)'
};

/** A receiver that has never been told where it is. */
export const receiverUnset: Receiver = {
	...receiver,
	lat: null,
	lon: null,
	altitude_m: 0,
	origin: 'unset',
	configured: null
};

/** A position set from this interface, shadowing what configuration asks for. */
export const receiverRuntime: Receiver = {
	...receiver,
	lat: 45.421,
	lon: -75.697,
	altitude_m: 70,
	origin: 'set at runtime'
};

/** A receiver started with `station_writable = false`. */
export const receiverLocked: Receiver = { ...receiver, writable: false };

export const aircraft: Aircraft[] = [
	{
		icao: 'A0A41F',
		callsign: 'UAL1234',
		lat: 42.31,
		lon: -76.02,
		position_age_ms: 900,
		position_source: 'global',
		altitude_ft: 34_025,
		altitude_source: 'baro',
		ground_speed_kt: 458,
		track_deg: 71,
		vertical_rate_fpm: 1_280,
		on_ground: false,
		category: 'A3',
		messages: 1_204,
		first_seen_ms: NOW - 540_000,
		last_seen_ms: NOW - 400,
		seen_ms: 400,
		rssi_dbfs: -18.4
	},
	{
		icao: 'C017ED',
		lat: 41.98,
		lon: -75.72,
		position_age_ms: 2_100,
		position_source: 'global',
		altitude_ft: 11_200,
		altitude_source: 'baro',
		ground_speed_kt: 274,
		track_deg: 299,
		vertical_rate_fpm: -640,
		on_ground: false,
		messages: 318,
		first_seen_ms: NOW - 210_000,
		last_seen_ms: NOW - 1_900,
		seen_ms: 1_900,
		rssi_dbfs: -24.1
	},
	{
		icao: 'AC82EB',
		callsign: 'N512SP',
		altitude_ft: 4_500,
		altitude_source: 'baro',
		track_deg: 12,
		on_ground: false,
		messages: 47,
		first_seen_ms: NOW - 96_000,
		last_seen_ms: NOW - 42_000,
		seen_ms: 42_000,
		rssi_dbfs: -31.7
	}
];

export const envelope: AircraftEnvelope = {
	now_ms: NOW,
	count: aircraft.length,
	aircraft
};

export const stats: Stats = {
	aircraft: 3,
	candidates: 4_212,
	configured_sample_rate_hz: 2_400_000,
	crc_fail: 1_809,
	crc_ok: 2_403,
	effective_sample_rate_hz: 2_400_944,
	impl_set: 'baseline',
	now_ms: NOW,
	positions: 72,
	reconnects: 0,
	samples: 149_760_000_000,
	uptime_s: 62_460,
	store: {
		dropped: 0,
		errors: 0,
		retention_deleted: 0,
		transactions: 610,
		written: 24_180
	}
};

export const health: Health = {
	status: 'ok',
	uptime_s: 62_460,
	version: '0.1.0',
	warnings: [],
	source: {
		description: 'tcp:127.0.0.1:1234 (R820T tuner, 29 gain steps) at 2.400 MS/s',
		state: 'streaming',
		effective_sample_rate_hz: 2_400_944,
		last_sample_age_ms: 24,
		reconnects: 0
	},
	decode: { aircraft: 3, last_message_age_s: 0, messages: 12_050, positions: 72 },
	store: { dropped: 0, enabled: true, errors: 0, written: 24_180 }
};

/** The health payload when the antenna is unplugged but the server is fine. */
export const healthSourceDown: Health = {
	...health,
	status: 'degraded',
	source: { ...health.source, state: 'down', last_sample_age_ms: 47_000 }
};

/** A recorded descent, oldest fix first, for the track components. */
export const track: TrackPoint[] = Array.from({ length: 60 }, (_, i) => {
	const t = NOW - (60 - i) * 15_000;
	return {
		ts_ms: t,
		lat: 42.31 - i * 0.006,
		lon: -76.02 + i * 0.004,
		altitude_ft: Math.round(34_000 - i * 480 + Math.sin(i / 5) * 200),
		ground_speed_kt: Math.round(460 - i * 1.5),
		track_deg: (71 + i * 0.4) % 360
	};
});

export const RECEIVER_LATLON = { lat: receiver.lat, lon: receiver.lon };
