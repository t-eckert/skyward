<script lang="ts">
	import type { TrackPoint } from '$lib/api';

	/**
	 * The path over the ground, drawn in the same range-and-bearing frame as
	 * the live plot so the two read as the same picture at different scales.
	 *
	 * Rings are drawn but deliberately unlabelled: this is a shape, not a
	 * measurement, and the numbers beneath it carry the actual range.
	 */

	interface Props {
		points: TrackPoint[];
		receiver: { lat: number | null; lon: number | null };
		/** Highlighted fix, shared with the charts. */
		hoverMs: number | null;
	}

	let { points, receiver, hoverMs }: Props = $props();

	const SIZE = 340;
	const centre = SIZE / 2;

	const projected = $derived.by(() => {
		if (receiver.lat === null || receiver.lon === null || points.length === 0) return null;

		// Equirectangular about the station is exact enough at these scales and
		// keeps the aspect honest by scaling longitude with the cosine of
		// latitude -- without which a north-south leg would look shorter than
		// an east-west one of the same length.
		const lat0 = (receiver.lat * Math.PI) / 180;
		const raw = points.map((p) => ({
			ts: p.ts_ms,
			x: (p.lon - receiver.lon!) * Math.cos(lat0),
			y: -(p.lat - receiver.lat!)
		}));

		const extent = raw.reduce(
			(max, p) => Math.max(max, Math.abs(p.x), Math.abs(p.y)),
			1e-6
		);
		const scale = (SIZE / 2 - 24) / extent;

		return {
			scale,
			path: raw.map((p) => ({ ts: p.ts, x: centre + p.x * scale, y: centre + p.y * scale }))
		};
	});

	const d = $derived(
		projected ? projected.path.map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x} ${p.y}`).join(' ') : ''
	);

	const marker = $derived.by(() => {
		if (!projected || hoverMs === null) return null;
		let best = projected.path[0];
		for (const p of projected.path) {
			if (Math.abs(p.ts - hoverMs) < Math.abs(best.ts - hoverMs)) best = p;
		}
		return best;
	});
</script>

<div class="wrap">
	<svg width={SIZE} height={SIZE} aria-label="Ground track relative to the receiver">
		<circle cx={centre} cy={centre} r={SIZE / 2 - 40} class="ring" />
		<circle cx={centre} cy={centre} r={(SIZE / 2 - 40) / 2} class="ring" />

		{#if projected}
			<path {d} class="track" />
			<circle cx={projected.path[0].x} cy={projected.path[0].y} r="3.5" class="start" />
			<circle
				cx={projected.path[projected.path.length - 1].x}
				cy={projected.path[projected.path.length - 1].y}
				r="4"
				class="end"
			/>
			{#if marker}
				<circle cx={marker.x} cy={marker.y} r="5" class="marker" />
			{/if}
		{/if}

		<circle cx={centre} cy={centre} r="3" class="station" />
	</svg>

	{#if !projected}
		<p class="empty tnum">
			{receiver.lat === null ? 'receiver position unset' : 'no fixes in this window'}
		</p>
	{/if}
</div>

<style>
	.wrap {
		position: relative;
		display: grid;
		place-items: center;
		padding: 12px 0 24px;
	}

	svg {
		display: block;
	}

	.ring {
		fill: none;
		stroke: var(--color-ring);
		stroke-width: 1;
	}

	.track {
		fill: none;
		stroke: var(--color-live);
		stroke-width: 1.75;
		stroke-linejoin: round;
	}

	/* Hollow where the track began, solid where it ends: direction of travel
	   without needing an arrowhead on a curve. */
	.start {
		fill: var(--color-panel);
		stroke: var(--color-live);
		stroke-width: 1.5;
	}

	.end {
		fill: var(--color-live);
	}

	.marker {
		fill: none;
		stroke: var(--color-selected);
		stroke-width: 1.5;
	}

	.station {
		fill: var(--color-station);
	}

	.empty {
		position: absolute;
		margin: 0;
		font-size: 12px;
		color: var(--color-muted);
	}
</style>
