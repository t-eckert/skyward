<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import {
		Map as MapLibreMap,
		NavigationControl,
		setWorkerUrl,
		type GeoJSONSource,
		type MapMouseEvent,
		type MapLayerMouseEvent
	} from 'maplibre-gl';
	import 'maplibre-gl/dist/maplibre-gl.css';
	import workerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';

	/**
	 * Point MapLibre at its own worker, explicitly.
	 *
	 * Left alone, MapLibre spawns the worker from a URL relative to its own
	 * module, and neither Vite mode resolves that: in dev the pre-bundled copy
	 * 404s, and in the production build the worker chunk is never emitted at
	 * all. Both fail the same silent way — the style never finishes loading, so
	 * no glyphs and no vector tiles are requested, and you get an empty map with
	 * nothing in the console.
	 *
	 * `?worker&url` makes Vite bundle it as a real worker entry (it imports a
	 * sibling shared chunk, so copying the file alone is not enough) and hand
	 * back a URL that works in both modes.
	 */
	setWorkerUrl(workerUrl);
	import type { Aircraft } from '$lib/api';
	import { theme } from '$lib/themes/state.svelte';
	import { num } from '$lib/format';

	/**
	 * Aircraft over a real basemap, with the range rings kept on top.
	 *
	 * # Why the rings stayed
	 *
	 * Geography answers "where is that aircraft"; the rings answer "how far out
	 * am I hearing, and in which direction", which is a question about the
	 * antenna and the only one this project is really about. A plain slippy map
	 * loses it. Drawn as real geodesic circles about the station, so they stay
	 * honest at any zoom and projection.
	 *
	 * # Why symbols rather than DOM markers
	 *
	 * Every aircraft is a feature in one GeoJSON source, drawn by a symbol
	 * layer. Markers are DOM nodes, and a hundred of them re-laid-out once a
	 * second is a different performance class. It also hands MapLibre the label
	 * collision problem, which it solves better than the hand-rolled version
	 * this replaces.
	 */

	interface Props {
		aircraft: Aircraft[];
		receiver: { lat: number | null; lon: number | null };
		nowMs: number;
		selected: string | null;
		/** The view is a frozen snapshot, not live. */
		frozen?: boolean;
		onselect: (icao: string | null) => void;
	}

	let { aircraft, receiver, nowMs, selected, frozen = false, onselect }: Props = $props();

	/** An aircraft heard longer ago than this is drawn as stale. */
	const STALE_MS = 30_000;

	/** Range rings, in nautical miles. */
	const RINGS = [50, 100, 150, 200];

	const NM_TO_M = 1852;
	const EARTH_R = 6371008.8;

	let container: HTMLDivElement;
	let map: MapLibreMap | null = null;
	let styleReady = $state(false);

	// ------------------------------------------------------------ helpers --

	/** Read a resolved token, so the map matches the CSS theme exactly. */
	function token(name: string): string {
		return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
	}

	/**
	 * The dart glyph, rasterised once per colour.
	 *
	 * Three flat images rather than one SDF: a rasterised silhouette used as a
	 * distance field renders with the wrong edges, and we only need three
	 * states. Re-added whenever the theme changes, since the colours move.
	 */
	function addGlyph(m: MapLibreMap, id: string, color: string) {
		const size = 48;
		const canvas = document.createElement('canvas');
		canvas.width = size;
		canvas.height = size;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;

		const s = size / 20;
		ctx.translate(size / 2, size / 2);
		ctx.scale(s, s);
		ctx.translate(-10, -10);
		ctx.beginPath();
		ctx.moveTo(10, 1.5);
		ctx.lineTo(16.5, 18);
		ctx.lineTo(10, 14.5);
		ctx.lineTo(3.5, 18);
		ctx.closePath();
		ctx.fillStyle = color;
		ctx.fill();

		if (m.hasImage(id)) m.removeImage(id);
		m.addImage(id, ctx.getImageData(0, 0, size, size), { pixelRatio: 2 });
	}

	/** The point `radiusM` from the station along a great circle at `brgRad`. */
	function destination(lat: number, lon: number, radiusM: number, brgRad: number): number[] {
		const p1 = (lat * Math.PI) / 180;
		const l1 = (lon * Math.PI) / 180;
		const d = radiusM / EARTH_R;
		const p2 = Math.asin(Math.sin(p1) * Math.cos(d) + Math.cos(p1) * Math.sin(d) * Math.cos(brgRad));
		const l2 =
			l1 +
			Math.atan2(
				Math.sin(brgRad) * Math.sin(d) * Math.cos(p1),
				Math.cos(d) - Math.sin(p1) * Math.sin(p2)
			);
		return [(l2 * 180) / Math.PI, (p2 * 180) / Math.PI];
	}

	/** A geodesic circle about the station, as a closed ring of positions. */
	function ring(lat: number, lon: number, radiusM: number, steps = 128): number[][] {
		return Array.from({ length: steps + 1 }, (_, i) =>
			destination(lat, lon, radiusM, (i / steps) * 2 * Math.PI)
		);
	}

	/**
	 * Where a ring's label sits: northeast of the station, on the ring.
	 *
	 * A point, not a label on the line. Line placement repeats the text once
	 * per tile the geometry crosses, so a 100 nm circle came out labelled five
	 * times. One point per ring is one label per ring, at a predictable
	 * bearing, whatever the zoom.
	 */
	const LABEL_BEARING = Math.PI / 4;

	// --------------------------------------------------------- geojson -----

	const aircraftGeoJSON = $derived.by(() => ({
		type: 'FeatureCollection' as const,
		features: aircraft
			.filter((a) => a.lat !== undefined && a.lon !== undefined)
			.map((a) => {
				const stale = nowMs - a.last_seen_ms > STALE_MS;
				return {
					type: 'Feature' as const,
					id: a.icao,
					geometry: { type: 'Point' as const, coordinates: [a.lon!, a.lat!] },
					properties: {
						icao: a.icao,
						label: a.callsign?.trim() || a.icao,
						sub:
							a.altitude_ft !== undefined || a.ground_speed_kt !== undefined
								? `${num(a.altitude_ft)} · ${num(a.ground_speed_kt)} kt`
								: '',
						// A track of exactly 0 and a missing track are different
						// things; -1 marks "unknown" so the layer can draw it
						// unrotated rather than pointing north.
						track: a.track_deg ?? -1,
						state: a.icao === selected ? 'selected' : stale ? 'stale' : 'live'
					}
				};
			})
	}));

	const ringsGeoJSON = $derived.by(() => {
		if (receiver.lat === null || receiver.lon === null) {
			return { type: 'FeatureCollection' as const, features: [] };
		}
		return {
			type: 'FeatureCollection' as const,
			features: RINGS.flatMap((nm) => [
				{
					type: 'Feature' as const,
					geometry: {
						type: 'LineString' as const,
						coordinates: ring(receiver.lat!, receiver.lon!, nm * NM_TO_M)
					},
					properties: { nm, kind: 'ring', label: `${nm} nm` }
				},
				{
					type: 'Feature' as const,
					geometry: {
						type: 'Point' as const,
						coordinates: destination(
							receiver.lat!,
							receiver.lon!,
							nm * NM_TO_M,
							LABEL_BEARING
						)
					},
					properties: { nm, kind: 'label', label: `${nm} nm` }
				}
			])
		};
	});

	const stationGeoJSON = $derived.by(() => ({
		type: 'FeatureCollection' as const,
		features:
			receiver.lat === null || receiver.lon === null
				? []
				: [
						{
							type: 'Feature' as const,
							geometry: {
								type: 'Point' as const,
								coordinates: [receiver.lon, receiver.lat]
							},
							properties: {}
						}
					]
	}));

	// ------------------------------------------------------- map layers ----

	/**
	 * Everything we draw on top of the basemap.
	 *
	 * Re-run on every style load, because `setStyle` discards all custom
	 * sources and layers — switching theme means rebuilding this.
	 */
	function addOverlay(m: MapLibreMap) {
		addGlyph(m, 'ac-live', token('--color-live'));
		addGlyph(m, 'ac-stale', token('--color-stale'));
		addGlyph(m, 'ac-selected', token('--color-selected'));

		m.addSource('rings', { type: 'geojson', data: ringsGeoJSON });
		m.addSource('station', { type: 'geojson', data: stationGeoJSON });
		m.addSource('aircraft', { type: 'geojson', data: aircraftGeoJSON });

		m.addLayer({
			id: 'rings-line',
			type: 'line',
			source: 'rings',
			filter: ['==', ['get', 'kind'], 'ring'],
			paint: {
				'line-color': token('--color-ring'),
				'line-width': 1.2,
				// Dashed so the rings read as something drawn over the map
				// rather than another road or county line. On a plain
				// background a solid stroke was unambiguous; on a basemap it
				// competes with real geography.
				'line-dasharray': [4, 3],
				'line-opacity': 0.95
			}
		});

		m.addLayer({
			id: 'rings-label',
			type: 'symbol',
			source: 'rings',
			filter: ['==', ['get', 'kind'], 'label'],
			layout: {
				'text-field': ['get', 'label'],
				'text-font': ['Noto Sans Regular'],
				'text-size': 10,
				'text-letter-spacing': 0.08,
				'text-allow-overlap': true
			},
			paint: {
				'text-color': token('--color-muted'),
				'text-halo-color': token('--color-bg'),
				'text-halo-width': 1.5
			}
		});

		m.addLayer({
			id: 'station-dot',
			type: 'circle',
			source: 'station',
			paint: {
				'circle-radius': 4,
				'circle-color': token('--color-station'),
				'circle-stroke-width': 1.5,
				'circle-stroke-color': token('--color-bg')
			}
		});

		m.addLayer({
			id: 'aircraft-mark',
			type: 'symbol',
			source: 'aircraft',
			layout: {
				'icon-image': [
					'match',
					['get', 'state'],
					'selected',
					'ac-selected',
					'stale',
					'ac-stale',
					'ac-live'
				],
				'icon-rotate': ['case', ['<', ['get', 'track'], 0], 0, ['get', 'track']],
				'icon-rotation-alignment': 'map',
				'icon-size': 0.55,
				// Aircraft must never be hidden by a label; the whole point is
				// seeing what is up there.
				'icon-allow-overlap': true,
				'text-field': ['concat', ['get', 'label'], '\n', ['get', 'sub']],
				'text-font': ['Noto Sans Regular'],
				'text-size': 11,
				'text-anchor': 'left',
				'text-offset': [0.9, 0],
				'text-justify': 'left',
				'text-optional': true
			},
			paint: {
				'text-color': [
					'match',
					['get', 'state'],
					'selected',
					token('--color-selected'),
					'stale',
					token('--color-stale'),
					token('--color-ink')
				],
				'text-halo-color': token('--color-bg'),
				'text-halo-width': 1.5
			}
		});

		styleReady = true;
	}

	function setData(id: string, data: unknown) {
		const source = map?.getSource(id);
		// `as never` because the GeoJSONSource typing wants its own union and
		// our literal already satisfies it structurally.
		if (source && 'setData' in source) (source as GeoJSONSource).setData(data as never);
	}

	// --------------------------------------------------------- lifecycle --

	onMount(() => {
		appliedStyle = theme.styleUrl;
		const m = new MapLibreMap({
			container,
			style: appliedStyle,
			center: [receiver.lon ?? -76.79, receiver.lat ?? 41.788],
			zoom: 7,
			attributionControl: { compact: true }
		});
		map = m;

		// Reachable from the console and from `scripts/mapdebug.mjs`. A map
		// that renders nothing gives you no stack trace to work from, so being
		// able to ask it what it thinks it is doing is worth the two lines.
		if (import.meta.env.DEV) {
			(window as unknown as { skywardMap?: MapLibreMap }).skywardMap = m;
		}

		m.addControl(new NavigationControl({ showCompass: false }), 'bottom-right');

		m.on('error', (e) => console.error('[maplibre]', e.error?.message ?? e));
		m.on('style.load', () => addOverlay(m));

		m.on('click', 'aircraft-mark', (e: MapLayerMouseEvent) => {
			const icao = e.features?.[0]?.properties?.icao as string | undefined;
			if (icao) onselect(icao === selected ? null : icao);
		});
		// A click on empty map clears the selection, matching the list.
		m.on('click', (e: MapMouseEvent) => {
			const hits = m.queryRenderedFeatures(e.point, { layers: ['aircraft-mark'] });
			if (hits.length === 0) onselect(null);
		});
		m.on('mouseenter', 'aircraft-mark', () => (m.getCanvas().style.cursor = 'pointer'));
		m.on('mouseleave', 'aircraft-mark', () => (m.getCanvas().style.cursor = ''));

		return () => {
			m.remove();
			map = null;
		};
	});

	// Push each snapshot into the existing sources rather than rebuilding them.
	$effect(() => {
		const data = aircraftGeoJSON;
		if (styleReady) untrack(() => setData('aircraft', data));
	});

	$effect(() => {
		const data = ringsGeoJSON;
		if (styleReady) untrack(() => setData('rings', data));
	});

	$effect(() => {
		const data = stationGeoJSON;
		if (styleReady) untrack(() => setData('station', data));
	});

	/**
	 * Swap the basemap when the theme changes — and only then.
	 *
	 * The guard is load-bearing. Without it this effect ran once on mount with
	 * the URL the map had just been constructed with, and calling `setStyle`
	 * mid-load aborted the initial load: the style never finished, so no glyphs
	 * and no vector tiles were ever requested and the map rendered as an empty
	 * background. Our own layers were added and correct the whole time, which
	 * is what made it look like a styling problem rather than a lifecycle one.
	 *
	 * A swap tears down every custom source and layer, so the overlay is marked
	 * gone until `style.load` rebuilds it.
	 */
	let appliedStyle: string | null = null;

	$effect(() => {
		const url = theme.styleUrl;
		untrack(() => {
			if (!map || url === appliedStyle) return;
			appliedStyle = url;
			styleReady = false;
			map.setStyle(url);
		});
	});

	/** Recentre on the station. */
	export function home() {
		if (map && receiver.lat !== null && receiver.lon !== null) {
			map.easeTo({ center: [receiver.lon, receiver.lat], zoom: 7 });
		}
	}
</script>

<div class="map" class:frozen bind:this={container}></div>

{#if receiver.lat === null || receiver.lon === null}
	<p class="notice">receiver position unset · set SKYWARD_RECEIVER_LAT and _LON to plot range</p>
{/if}

<style>
	.map {
		position: relative;
		flex: 1;
		min-width: 0;
		background: var(--color-bg);
	}

	.map.frozen {
		opacity: 0.55;
	}

	.notice {
		position: absolute;
		left: 50%;
		top: 50%;
		transform: translateX(-50%);
		margin: 0;
		font-size: 12px;
		color: var(--color-muted);
		pointer-events: none;
	}

	/* MapLibre's own chrome, brought into the token set so it does not read as
	   a third design. */
	.map :global(.maplibregl-ctrl-attrib) {
		font-size: 10px;
		background: color-mix(in srgb, var(--color-bg) 80%, transparent);
		color: var(--color-muted);
	}

	.map :global(.maplibregl-ctrl-attrib a) {
		color: var(--color-muted);
	}

	.map :global(.maplibregl-ctrl-group) {
		background: var(--color-panel);
		border: 1px solid var(--color-line);
		box-shadow: none;
	}

	.map :global(.maplibregl-ctrl-group button + button) {
		border-top: 1px solid var(--color-line);
	}

	.map :global(.maplibregl-ctrl-group button .maplibregl-ctrl-icon) {
		filter: var(--map-ctrl-filter, none);
	}
</style>
