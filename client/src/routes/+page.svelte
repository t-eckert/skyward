<script lang="ts">
	import { onMount } from 'svelte';
	import { Live } from '$lib/live.svelte';
	import Header from '$lib/components/Header.svelte';
	import AircraftMap from '$lib/components/AircraftMap.svelte';
	import DetailPanel from '$lib/components/DetailPanel.svelte';
	import TrafficList from '$lib/components/TrafficList.svelte';
	import Scoreboard from '$lib/components/Scoreboard.svelte';
	import DisconnectedBanner from '$lib/components/DisconnectedBanner.svelte';
	import NoTraffic from "$lib/components/NoTraffic.svelte";
	import SourceDownBanner from "$lib/components/SourceDownBanner.svelte";

	const live = new Live();

	onMount(() => {
		live.start();
		return () => live.stop();
	});

	const frozen = $derived(live.connection === 'disconnected');
	const receiver = $derived({
		lat: live.receiver?.lat ?? null,
		lon: live.receiver?.lon ?? null
	});

	// "Nothing in range" is only honest when the whole chain is healthy: its
	// copy asserts that the decoder is fine and the source is streaming. An
	// empty list during an outage — or with the antenna unplugged — means
	// something else entirely, and the banners own those explanations.
	const quiet = $derived(live.viewState === 'streaming' && live.aircraft.length === 0);

	const quietMs = $derived(live.lastHeard ? live.nowMs - live.lastHeard.atMs : null);
</script>

<Header
	receiver={live.receiver}
	connection={live.connection}
	lastSnapshotMs={live.lastSnapshotMs}
	status={live.viewState}
/>

{#if live.viewState === "source-down"}
	<SourceDownBanner
		silentMs={live.sourceSilentMs}
		reconnects={live.health?.source.reconnects ?? 0}
		description={live.health?.source.description ?? ""}
	/>
{:else if frozen && live.outageMs !== null}
	<DisconnectedBanner
		outageMs={live.outageMs}
		retryInS={live.retryInS}
		onretry={() => live.retryNow()}
	/>
{/if}

<main>
	<AircraftMap
		aircraft={live.aircraft}
		{receiver}
		nowMs={live.nowMs}
		selected={live.selected}
		{frozen}
		onselect={(icao) => (live.selected = icao)}
	/>

	<aside>
		{#if quiet}
			<NoTraffic lastHeard={live.lastHeard} {quietMs} peakRate={live.peakRate} />
		{:else}
			<DetailPanel
				aircraft={live.selectedAircraft}
				nowMs={live.nowMs}
				{frozen}
				count={live.aircraft.length}
			/>
			<TrafficList
				aircraft={live.aircraft}
				{receiver}
				nowMs={live.nowMs}
				selected={live.selected}
				{frozen}
				onselect={(icao) => (live.selected = icao)}
			/>
		{/if}
	</aside>
</main>

<Scoreboard
	stats={live.stats}
	messagesPerMinute={live.messagesPerMinute}
	yieldPct={live.yieldPct}
	realtimeFactor={live.realtimeFactor}
	connection={live.connection}
	retryInS={live.retryInS}
/>

<style>
	main {
		display: flex;
		flex: 1;
		min-height: 0;
	}

	aside {
		display: flex;
		flex-direction: column;
		flex-shrink: 0;
		width: var(--sidebar-w);
		min-height: 0;
		background: var(--color-panel);
		border-left: 1px solid var(--color-line);
	}
</style>
