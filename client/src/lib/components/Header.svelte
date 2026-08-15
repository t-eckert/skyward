<script lang="ts">
	import { onMount } from 'svelte';
	import type { Receiver } from '$lib/api';
	import type { Connection } from '$lib/live.svelte';
	import { station, uptime, clock } from '$lib/format';
	import { theme } from '$lib/themes/state.svelte';
	import ThemeSwitcher from './ThemeSwitcher.svelte';

	interface Props {
		receiver: Receiver | null;
		connection: Connection;
		lastSnapshotMs: number | null;
		/** Overrides `connection` when the server is up but the radio is not. */
		status?: 'streaming' | 'source-down' | 'disconnected';
	}

	let { receiver, connection, lastSnapshotMs, status }: Props = $props();

	const view = $derived(status ?? (connection === "streaming" ? "streaming" : "disconnected"));
	const live = $derived(view === 'streaming');

	const LABEL = {
		streaming: 'STREAMING',
		'source-down': 'NO SIGNAL',
		disconnected: 'DISCONNECTED'
	} as const;

	const rate = $derived(
		receiver ? `${(receiver.sample_rate_hz / 1_000_000).toFixed(3)} MS/s` : ''
	);

	// The source string already carries the tuner description, which is more
	// than the header has room for. Keep the endpoint and drop the parenthetical.
	const endpoint = $derived(receiver ? receiver.source.replace(/\s*\(.*$/, '') : '');

	// The header owns the control; the shared store owns the value, because the
	// map needs it too — a basemap is a style, not a stylesheet, so it cannot
	// pick the change up from a CSS custom property.
	let showSwitcher = $state(false);

	onMount(() => {
		theme.init();
		showSwitcher = new URLSearchParams(window.location.search).get('chrome') !== '0';
	});
</script>

<header>
	<div class="brand">
		<span class="wordmark">SKYWARD</span>
		<span class="rule"></span>
		<span class="tnum place">
			{receiver?.station ?? 'skyward'} · {station(receiver?.lat ?? null, receiver?.lon ?? null)}
			{#if receiver}· {Math.round(receiver.altitude_m)} m{/if}
		</span>
	</div>

	<div class="status">
		<div class="state" class:down={!live}>
			<span class="dot"></span>
			<span class="text">{LABEL[view]}</span>
		</div>
		<span class="tnum meta">
			{#if view === 'streaming'}
				{endpoint} · {rate}
			{:else if view === 'source-down'}
				{endpoint} · no samples
			{:else}
				{endpoint} · unreachable
			{/if}
		</span>
		<span class="rule"></span>
		<span class="tnum meta">
			{#if live}
				up {uptime(receiver?.uptime_s)}
			{:else}
				last frame {lastSnapshotMs ? clock(lastSnapshotMs) : '—'}
			{/if}
		</span>

		{#if showSwitcher}
			<ThemeSwitcher current={theme.current} onchange={(id) => theme.set(id)} />
		{/if}
	</div>
</header>

<style>
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		flex-shrink: 0;
		height: var(--header-h);
		padding-inline: 32px;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-line);
	}

	.brand {
		display: flex;
		align-items: center;
		gap: 18px;
	}

	.wordmark {
		font-family: var(--font-display);
		font-size: var(--size-wordmark);
		font-weight: var(--weight-wordmark);
		letter-spacing: var(--tracking-wordmark);
		line-height: 22px;
		color: var(--color-ink);
	}

	.rule {
		flex-shrink: 0;
		width: 1px;
		height: 20px;
		background: var(--color-line);
	}

	.place,
	.meta {
		font-size: var(--size-meta);
		line-height: 16px;
		color: var(--color-muted);
		white-space: nowrap;
	}

	.status {
		display: flex;
		align-items: center;
		gap: 24px;
	}

	.state {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--color-ok);
	}

	/* Rust, never red: the palette reserves one warm colour for "attend to
	   this", so it still reads as urgent when it is the only warm thing. */
	.state.down {
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
