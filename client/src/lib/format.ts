/**
 * Number and time rendering, matching the design exactly.
 *
 * Two typographic decisions carry through the whole interface:
 *
 * **Space-grouped digits.** `34 025`, not `34,025`. A comma in a column of
 * figures reads as punctuation and costs a full cell; a space groups without
 * implying anything.
 *
 * **A real minus sign.** `−18.4` uses U+2212, not the hyphen-minus. At the
 * sizes these values are set, a hyphen sits too high and too short to read as
 * negation.
 */

const MINUS = '−';

/** The dash shown wherever a value is genuinely unknown. */
export const NONE = '—';

/**
 * Group digits in threes with spaces, and use a typographic minus.
 *
 * `null`/`undefined` become an em dash rather than `0`: an aircraft that has
 * not reported its altitude is not at sea level, and the difference matters.
 */
export function num(value: number | null | undefined, digits = 0): string {
	if (value === null || value === undefined || !Number.isFinite(value)) return NONE;
	const negative = value < 0;
	const fixed = Math.abs(value).toFixed(digits);
	const [whole, fraction] = fixed.split('.');
	const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
	const body = fraction ? `${grouped}.${fraction}` : grouped;
	return negative ? MINUS + body : body;
}

/** Signed, for vertical rate: a climb must read as `+1 280`. */
export function signed(value: number | null | undefined, digits = 0): string {
	if (value === null || value === undefined || !Number.isFinite(value)) return NONE;
	if (value > 0) return '+' + num(value, digits);
	return num(value, digits);
}

/** Three-digit compass bearing, as aviation writes it: `042°`. */
export function bearing(value: number | null | undefined): string {
	if (value === null || value === undefined || !Number.isFinite(value)) return NONE;
	const normalized = Math.round(((value % 360) + 360) % 360) % 360;
	return String(normalized).padStart(3, '0') + '°';
}

/**
 * A short age: `0.4 s`, `48 s`, `4 m 12 s`, `6d 04:11`.
 *
 * Sub-minute ages keep a decimal because the difference between a 0.4 s and a
 * 4 s fix is the difference between a live track and a stalling one.
 */
export function age(ms: number | null | undefined): string {
	if (ms === null || ms === undefined || !Number.isFinite(ms)) return NONE;
	const seconds = Math.max(0, ms) / 1000;
	if (seconds < 10) return `${seconds.toFixed(1)} s`;
	if (seconds < 60) return `${Math.round(seconds)} s`;
	const minutes = Math.floor(seconds / 60);
	if (minutes < 60) return `${minutes} m ${Math.round(seconds % 60)} s`;
	const hours = Math.floor(minutes / 60);
	if (hours < 24) return `${hours} h ${minutes % 60} m`;
	return `${Math.floor(hours / 24)} d ${hours % 24} h`;
}

/** Uptime in the header's form: `up 6d 04:11`. */
export function uptime(totalSeconds: number | null | undefined): string {
	if (totalSeconds === null || totalSeconds === undefined || !Number.isFinite(totalSeconds)) {
		return NONE;
	}
	const s = Math.max(0, Math.floor(totalSeconds));
	const days = Math.floor(s / 86400);
	const hours = Math.floor((s % 86400) / 3600);
	const minutes = Math.floor((s % 3600) / 60);
	const hm = `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}`;
	return days > 0 ? `${days}d ${hm}` : hm;
}

/** Wall-clock `13:55:12`, in the viewer's local zone. */
export function clock(ms: number): string {
	const d = new Date(ms);
	return [d.getHours(), d.getMinutes(), d.getSeconds()]
		.map((v) => String(v).padStart(2, '0'))
		.join(':');
}

/** `13:55`, for chart axes where seconds are noise. */
export function hhmm(ms: number): string {
	const d = new Date(ms);
	return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** The station position as the header writes it: `41.788 N, 76.790 W`. */
export function station(lat: number | null, lon: number | null): string {
	if (lat === null || lon === null) return 'position unset';
	const ns = lat >= 0 ? 'N' : 'S';
	const ew = lon >= 0 ? 'E' : 'W';
	return `${Math.abs(lat).toFixed(3)} ${ns}, ${Math.abs(lon).toFixed(3)} ${ew}`;
}

const NM_PER_KM = 1 / 1.852;
const EARTH_RADIUS_KM = 6371.0088;

/** Great-circle distance in kilometres. Mirrors `adsb_core::cpr::haversine_km`. */
export function haversineKm(lat1: number, lon1: number, lat2: number, lon2: number): number {
	const p1 = (lat1 * Math.PI) / 180;
	const p2 = (lat2 * Math.PI) / 180;
	const dp = p2 - p1;
	const dl = ((lon2 - lon1) * Math.PI) / 180;
	const a =
		Math.sin(dp / 2) ** 2 + Math.cos(p1) * Math.cos(p2) * Math.sin(dl / 2) ** 2;
	return 2 * EARTH_RADIUS_KM * Math.asin(Math.min(1, Math.sqrt(a)));
}

export function kmToNm(km: number): number {
	return km * NM_PER_KM;
}

/** Initial bearing from point 1 to point 2, in degrees true. */
export function bearingDeg(lat1: number, lon1: number, lat2: number, lon2: number): number {
	const p1 = (lat1 * Math.PI) / 180;
	const p2 = (lat2 * Math.PI) / 180;
	const dl = ((lon2 - lon1) * Math.PI) / 180;
	const y = Math.sin(dl) * Math.cos(p2);
	const x = Math.cos(p1) * Math.sin(p2) - Math.sin(p1) * Math.cos(p2) * Math.cos(dl);
	return ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360;
}
