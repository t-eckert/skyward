/**
 * The two directions, which double as a light/dark pair.
 *
 * Applied as a `data-theme` attribute on the root element; the token blocks in
 * `app.css` are scoped to it, so switching is a single attribute write and
 * costs no reload. `?theme=<id>` selects one, and the choice is remembered.
 */

export interface Theme {
	id: string;
	name: string;
	note: string;
	/**
	 * The basemap style.
	 *
	 * OpenFreeMap: no API key, no registration, no request limit. The two
	 * styles share one tile source, so switching theme reloads the style but
	 * not the data. Attribution — OpenStreetMap is ODbL and requires it — rides
	 * in the source TileJSON, so MapLibre renders it without being asked.
	 */
	style: string;
}

export const THEMES: Theme[] = [
	{
		id: 'flightdeck',
		name: 'Flight deck',
		note: 'The cockpit convention, used literally: white current, cyan reference, magenta target, amber caution.',
		style: 'https://tiles.openfreemap.org/styles/dark'
	},
	{
		id: 'chart',
		name: 'Chart',
		note: 'The VFR sectional, on neutral paper. Magenta is what you look for; blue stays structural.',
		style: 'https://tiles.openfreemap.org/styles/positron'
	}
];

const KEY = 'skyward:theme';
const DEFAULT = 'flightdeck';

export function isTheme(id: string | null): id is string {
	return id !== null && THEMES.some((t) => t.id === id);
}

/** Resolve the theme from the URL, then storage, then the default. */
export function initialTheme(search: string): string {
	if (typeof document === 'undefined') return DEFAULT;
	const fromUrl = new URLSearchParams(search).get('theme');
	if (isTheme(fromUrl)) return fromUrl;
	const stored = localStorage.getItem(KEY);
	return isTheme(stored) ? stored : DEFAULT;
}

export function applyTheme(id: string) {
	if (typeof document === 'undefined') return;
	document.documentElement.setAttribute('data-theme', id);
	localStorage.setItem(KEY, id);
}
