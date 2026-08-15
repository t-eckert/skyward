<script lang="ts">
	import type { Stats } from '$lib/api';
	import type { Connection } from '$lib/live.svelte';
	import { num, NONE } from '$lib/format';

	interface Props {
		stats: Stats | null;
		messagesPerMinute: number | null;
		yieldPct: number | null;
		realtimeFactor: number | null;
		connection: Connection;
		retryInS: number;
	}

	let { stats, messagesPerMinute, yieldPct, realtimeFactor, connection, retryInS }: Props =
		$props();

	const live = $derived(connection === 'streaming');
</script>

<footer>
	<div class="hero">
		<span class="label">Messages / min</span>
		<span class="tnum figure" class:alert={!live}>
			{live ? num(messagesPerMinute, 1) : num(0, 1)}
		</span>
	</div>

	<span class="rule"></span>

	<div class="tile">
		<span class="label">Messages</span>
		<span class="tnum value">{num(stats?.crc_ok)}</span>
	</div>

	<!-- Reported, never optimised: a better detector proposes more marginal
	     candidates, so yield falls as messages rise. -->
	<div class="tile">
		<span class="label">Yield</span>
		<span class="tnum value">{yieldPct === null ? NONE : `${num(yieldPct, 1)} %`}</span>
	</div>

	<div class="tile">
		<span class="label">Realtime</span>
		<span class="tnum value" class:alert={realtimeFactor !== null && realtimeFactor < 0.98}>
			{realtimeFactor === null ? NONE : `${num(realtimeFactor, 2)} ×`}
		</span>
	</div>

	<!-- The ghost guard is computed by `skyward bench`, not by the live
	     decoder, so there is nothing truthful to put here yet. A zero would
	     read as "measured and clean", which is exactly the wrong impression. -->
	<div class="tile">
		<span class="label">Ghosts</span>
		<span class="tnum value dim" title="Only measured by `skyward bench`">{NONE}</span>
	</div>

	<div class="spacer"></div>

	<div class="connection tnum">
		<span class:alert={!live}>
			{live ? 'sse connected' : 'sse disconnected'}
		</span>
		<span class="rule"></span>
		<span>
			{#if live}
				{stats ? `${num(stats.store.written)} rows stored` : ''}
			{:else}
				retrying in {retryInS} s
			{/if}
		</span>
	</div>
</footer>

<style>
	footer {
		display: flex;
		align-items: center;
		gap: 32px;
		flex-shrink: 0;
		height: var(--footer-h);
		padding-inline: 32px;
		background: var(--color-surface);
		border-top: 1px solid var(--color-line);
	}

	.hero {
		display: flex;
		flex-direction: column;
		gap: 1px;
	}

	/* The one piece of display type on the screen. Everything else on this
	   rail is a supporting figure, and the size difference says so. */
	.figure {
		font-family: var(--font-display);
		font-size: var(--size-hero);
		font-weight: var(--weight-hero);
		letter-spacing: -0.02em;
		line-height: calc(var(--size-hero) + 14px);
		color: var(--color-ink);
	}

	.figure.alert {
		color: var(--color-alert);
	}

	.rule {
		flex-shrink: 0;
		width: 1px;
		height: 44px;
		background: var(--color-line);
	}

	.tile {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.value {
		font-size: var(--size-tile);
		line-height: 18px;
		color: var(--color-ink);
	}

	.value.dim {
		color: var(--color-muted);
	}

	.value.alert {
		color: var(--color-alert);
	}

	.spacer {
		flex: 1;
	}

	.connection {
		display: flex;
		align-items: center;
		gap: 16px;
		font-size: var(--size-meta);
		line-height: 16px;
		color: var(--color-muted);
	}

	.connection .rule {
		height: 20px;
	}

	.connection .alert {
		color: var(--color-alert);
	}
</style>
