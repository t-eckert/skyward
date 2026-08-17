<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';

	// Composition stories: no single `component`, just the real pieces arranged
	// the way `routes/+page.svelte` arranges them, so a whole layout can be tuned
	// at once. The live map is left out — it needs MapLibre's worker and network
	// tiles — and stood in for by a labelled panel.
	const { Story } = defineMeta({
		title: 'Layouts/Dashboard',
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' }
	});
</script>

<script lang="ts">
	import { fn } from 'storybook/test';
	import Header from '$lib/components/Header.svelte';
	import Scoreboard from '$lib/components/Scoreboard.svelte';
	import DetailPanel from '$lib/components/DetailPanel.svelte';
	import TrafficList from '$lib/components/TrafficList.svelte';
	import NoTraffic from '$lib/components/NoTraffic.svelte';
	import { aircraft, envelope, receiver, stats, RECEIVER_LATLON } from '$lib/mocks';

	const now = envelope.now_ms;
</script>

<!-- The right-hand rail: selected aircraft over the sortable list. -->
<Story name="Sidebar" asChild>
	<aside class="rail">
		<DetailPanel aircraft={aircraft[0]} nowMs={now} count={aircraft.length} />
		<TrafficList
			{aircraft}
			receiver={RECEIVER_LATLON}
			nowMs={now}
			selected="A0A41F"
			onselect={fn()}
		/>
	</aside>
</Story>

<!-- The same rail when the sky is quiet. -->
<Story name="Sidebar — quiet" asChild>
	<aside class="rail">
		<NoTraffic lastHeard={{ label: 'UAL1234', atMs: now - 134_000 }} quietMs={134_000} peakRate={172} />
	</aside>
</Story>

<!-- Header and scoreboard around the plot area, full height. -->
<Story name="Chrome — header & scoreboard" asChild>
	<div class="app">
		<Header {receiver} connection="streaming" lastSnapshotMs={now - 400} status="streaming" />
		<div class="plot">plot / map area</div>
		<Scoreboard
			{stats}
			messagesPerMinute={172.3}
			yieldPct={58.8}
			realtimeFactor={361.5}
			connection="streaming"
			retryInS={0}
		/>
	</div>
</Story>

<style>
	.rail {
		display: flex;
		flex-direction: column;
		width: var(--sidebar-w);
		height: 100vh;
		min-height: 0;
		background: var(--color-panel);
		border-left: 1px solid var(--color-line);
	}

	.app {
		display: flex;
		flex-direction: column;
		height: 100vh;
		min-height: 0;
	}

	.plot {
		display: flex;
		flex: 1;
		align-items: center;
		justify-content: center;
		min-height: 0;
		color: var(--color-muted);
		font-size: var(--size-label);
		letter-spacing: var(--tracking-label);
		text-transform: uppercase;
		background: var(--color-bg);
	}
</style>
