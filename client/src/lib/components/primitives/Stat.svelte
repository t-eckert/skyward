<script lang="ts">
	/**
	 * A labelled figure: the label above (or beside) a value, with an optional
	 * unit. This is the shape repeated across DetailPanel's `.stat`, the
	 * Scoreboard's `.hero`/`.tile`, and NoTraffic's `<dl>` rows — one primitive
	 * so the sizes and the `alert` colour live in exactly one place.
	 *
	 * Presentational only: it takes an already-formatted string, so the caller
	 * keeps owning `format.ts` and deciding what an em dash means.
	 */
	interface Props {
		label: string;
		/** Pre-formatted; pass the em dash yourself for "unknown". */
		value: string;
		unit?: string;
		/** `stacked` is the panel/scoreboard form; `inline` is the NoTraffic row. */
		orientation?: 'stacked' | 'inline';
		/** Type scale for the value. `hero` also switches to display type. */
		size?: 'tile' | 'value' | 'hero';
		/** Colour of the value. */
		tone?: 'default' | 'alert' | 'accent' | 'muted';
	}

	let {
		label,
		value,
		unit,
		orientation = 'stacked',
		size = 'value',
		tone = 'default'
	}: Props = $props();
</script>

<div class="stat" class:inline={orientation === 'inline'}>
	<span class="label">{label}</span>
	<span class="value tnum {size} {tone}">{value}</span>
	{#if unit}<span class="tnum unit">{unit}</span>{/if}
</div>

<style>
	.stat {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	/* Label left, value right, baseline-aligned — the dl row form. */
	.stat.inline {
		flex-direction: row;
		align-items: baseline;
		justify-content: space-between;
		gap: 16px;
	}

	.value {
		color: var(--color-ink);
	}

	/* --- size ------------------------------------------------------------- */
	.value.tile {
		font-size: var(--size-tile);
		line-height: 18px;
	}

	.value.value {
		font-size: var(--size-value);
		font-weight: 500;
		line-height: calc(var(--size-value) + 4px);
	}

	.value.hero {
		font-family: var(--font-display);
		font-size: var(--size-hero);
		font-weight: var(--weight-hero);
		letter-spacing: -0.02em;
		line-height: calc(var(--size-hero) + 14px);
	}

	/* --- tone ------------------------------------------------------------- */
	.value.alert {
		color: var(--color-alert);
	}
	.value.accent {
		color: var(--color-accent);
	}
	.value.muted {
		color: var(--color-muted);
	}

	.unit {
		font-size: var(--size-unit);
		line-height: 14px;
		color: var(--color-muted);
	}
</style>
