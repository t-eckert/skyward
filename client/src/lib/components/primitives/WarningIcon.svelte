<script lang="ts">
	/**
	 * The warning triangle, drawn in `currentColor`. It was duplicated verbatim
	 * inside both banners; here once so the two cannot drift apart.
	 */
	interface Props {
		size?: number;
	}
	let { size = 16 }: Props = $props();
</script>

<svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
	<path
		d="M8 1.5 L15 14.5 L1 14.5 Z"
		fill="none"
		stroke="currentColor"
		stroke-width="1.4"
		stroke-linejoin="round"
	/>
	<path d="M8 6 v4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
	<circle cx="8" cy="12.2" r="0.8" fill="currentColor" />
</svg>

<style>
	svg {
		flex-shrink: 0;
		display: block;
	}
</style>
