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
		/** Opens the station dialog. Omitted in Storybook and in `?chrome=0`. */
		oneditstation?: () => void;
	}

	let { receiver, connection, lastSnapshotMs, status, oneditstation }: Props = $props();

	// An unset position is not a cosmetic gap: it disables the range gate and
	// local CPR, and it leaves the map with nothing to centre on. It gets the
	// one warm colour in the palette so it reads as something to fix.
	const unset = $derived(receiver !== null && (receiver.lat === null || receiver.lon === null));

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
		{#if oneditstation}
			<button
				class="tnum place editable"
				class:unset
				onclick={oneditstation}
				title="Set the receiver position"
			>
				{receiver?.station ?? 'skyward'} · {station(
					receiver?.lat ?? null,
					receiver?.lon ?? null
				)}
				{#if receiver && !unset}· {Math.round(receiver.altitude_m)} m{/if}
			</button>
		{:else}
			<span class="tnum place" class:unset>
				{receiver?.station ?? 'skyward'} · {station(
					receiver?.lat ?? null,
					receiver?.lon ?? null
				)}
				{#if receiver && !unset}· {Math.round(receiver.altitude_m)} m{/if}
			</span>
		{/if}
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

	.editable {
		padding: 2px 6px;
		margin-inline-start: -6px;
		font-family: inherit;
		background: transparent;
		border: var(--border-w) solid transparent;
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	.editable:hover {
		color: var(--color-ink);
		border-color: var(--color-line);
	}

	.editable:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 1px;
	}

	/* Rust, never red -- the same warm colour the disconnected state uses,
	   because an unset position is the same class of problem: the receiver is
	   running and cannot do its job. */
	.place.unset {
		color: var(--color-alert);
	}

	.editable.unset::after {
		content: ' · SET';
		font-size: var(--size-label);
		font-weight: var(--weight-label);
		letter-spacing: var(--tracking-label);
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
