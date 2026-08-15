<script lang="ts">
	/**
	 * The dart that marks an aircraft, rotated to its ground track.
	 *
	 * Rotation is `track_deg` and nothing else. ADS-B does not report heading,
	 * and substituting one for the other would silently draw every aircraft a
	 * few degrees off in a crosswind.
	 *
	 * An aircraft with no track at all is drawn unrotated and hollow, so
	 * "pointing north" is never confused with "direction unknown".
	 */
	interface Props {
		track?: number | null;
		/** Overrides the variant's `--glyph-size`. */
		size?: number | null;
		color?: string;
		/** Hollow, for an aircraft whose track is unknown. */
		hollow?: boolean;
	}

	let { track = null, size = null, color = 'currentColor', hollow = false }: Props = $props();

	const known = $derived(track !== null && track !== undefined && Number.isFinite(track));
	const rotation = $derived(known ? (track as number) : 0);
</script>

<svg
	style={size === null ? undefined : `width:${size}px;height:${size}px`}
	viewBox="0 0 20 20"
	xmlns="http://www.w3.org/2000/svg"
>
	<path
		d="M10 1.5 L16.5 18 L10 14.5 L3.5 18 Z"
		transform="rotate({rotation} 10 10)"
		fill={hollow || !known ? 'none' : color}
		stroke={hollow || !known ? color : 'none'}
		stroke-width={hollow || !known ? 1.5 : 0}
		stroke-linejoin="round"
	/>
</svg>

<style>
	svg {
		flex-shrink: 0;
		display: block;
		width: var(--glyph-size);
		height: var(--glyph-size);
		filter: var(--plot-glow);
	}
</style>
