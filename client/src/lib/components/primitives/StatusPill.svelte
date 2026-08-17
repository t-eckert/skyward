<script lang="ts">
	/**
	 * A status dot and its label, coloured by state. This is the Header's
	 * `.state` element made reusable — the footer's connection line expresses the
	 * same idea and can share it.
	 *
	 * The palette reserves one warm colour for "attend to this", so anything not
	 * OK reads as caution rather than inventing a second alarm colour.
	 */
	interface Props {
		/** `ok` is green; everything else is the single caution colour. */
		state: 'ok' | 'caution';
		label: string;
	}
	let { state, label }: Props = $props();
</script>

<div class="pill" class:caution={state === 'caution'}>
	<span class="dot"></span>
	<span class="text">{label}</span>
</div>

<style>
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		color: var(--color-ok);
	}

	.pill.caution {
		color: var(--color-alert);
	}

	.dot {
		flex-shrink: 0;
		width: 7px;
		height: 7px;
		border-radius: 4px;
		background: currentColor;
	}

	.text {
		font-size: var(--size-label);
		font-weight: 600;
		letter-spacing: var(--tracking-label);
		line-height: 14px;
	}
</style>
