<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api, type Receiver, type TrackPoint } from '$lib/api';
	import { Live } from '$lib/live.svelte';
	import Header from '$lib/components/Header.svelte';
	import TrackChart from '$lib/components/TrackChart.svelte';
	import GroundTrack from '$lib/components/GroundTrack.svelte';
	import {
		num,
		clock,
		age,
		bearing,
		haversineKm,
		kmToNm,
		NONE
	} from '$lib/format';

	const icao = $derived(page.params.icao?.toUpperCase() ?? '');

	const RANGES = [
		{ label: '15 m', ms: 15 * 60_000 },
		{ label: '1 h', ms: 60 * 60_000 },
		{ label: '6 h', ms: 6 * 60 * 60_000 },
		{ label: '24 h', ms: 24 * 60 * 60_000 }
	];

	let rangeMs = $state(RANGES[1].ms);
	let view = $state<'chart' | 'table'>('chart');
	let hoverMs = $state<number | null>(null);

	let points = $state<TrackPoint[]>([]);
	let limitHit = $state(false);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let receiver = $state<Receiver | null>(null);

	// The live connection runs here too, so the header keeps telling the truth
	// about the receiver while you are reading history.
	const live = new Live();

	const LIMIT = 2000;

	async function load() {
		if (!icao) return;
		loading = true;
		error = null;
		try {
			const response = await api.track(icao, Date.now() - rangeMs, LIMIT);
			points = response.track;
			limitHit = response.count >= LIMIT;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		live.start();
		api.receiver().then((r) => (receiver = r)).catch(() => {});
		const timer = setInterval(load, 10_000);
		return () => {
			live.stop();
			clearInterval(timer);
		};
	});

	// Reload whenever the aircraft or the window changes.
	$effect(() => {
		void icao;
		void rangeMs;
		void load();
	});

	const first = $derived(points.at(0) ?? null);
	const last = $derived(points.at(-1) ?? null);

	const altitudes = $derived(
		points.map((p) => p.altitude_ft).filter((v): v is number => v !== null)
	);
	const speeds = $derived(
		points.map((p) => p.ground_speed_kt).filter((v): v is number => v !== null)
	);

	const ceiling = $derived(altitudes.length ? Math.max(...altitudes) : null);
	const peakSpeed = $derived(speeds.length ? Math.max(...speeds) : null);

	const maxRangeNm = $derived.by(() => {
		if (!receiver || receiver.lat === null || receiver.lon === null) return null;
		let max = 0;
		for (const p of points) {
			max = Math.max(max, kmToNm(haversineKm(receiver.lat, receiver.lon, p.lat, p.lon)));
		}
		return points.length ? max : null;
	});

	/**
	 * Climb rate across the window, from the recorded altitudes.
	 *
	 * Derived here rather than read from a message: the positions table stores
	 * what was observed, and the average over the window is the honest summary
	 * of it. An instantaneous rate would be the last message's guess.
	 */
	const climbFpm = $derived.by(() => {
		if (!first || !last || first.altitude_ft === null || last.altitude_ft === null) return null;
		const minutes = (last.ts_ms - first.ts_ms) / 60_000;
		if (minutes <= 0) return null;
		return (last.altitude_ft - first.altitude_ft) / minutes;
	});

	const liveNow = $derived(live.aircraft.find((a) => a.icao === icao) ?? null);

	const rows = $derived(view === 'table' ? [...points].reverse() : []);

	/** Four decimals, with the same typographic minus the rest of the UI uses. */
	const coord = (v: number) => v.toFixed(4).replace('-', '−');

	function exportCsv() {
		const header = 'ts_ms,iso,lat,lon,altitude_ft,ground_speed_kt,track_deg\n';
		const body = points
			.map((p) =>
				[
					p.ts_ms,
					new Date(p.ts_ms).toISOString(),
					p.lat,
					p.lon,
					p.altitude_ft ?? '',
					p.ground_speed_kt ?? '',
					p.track_deg ?? ''
				].join(',')
			)
			.join('\n');
		const blob = new Blob([header + body], { type: 'text/csv' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `${icao}-track.csv`;
		a.click();
		URL.revokeObjectURL(url);
	}
</script>

<Header
	receiver={live.receiver ?? receiver}
	connection={live.connection}
	lastSnapshotMs={live.lastSnapshotMs}
/>

<div class="subhead">
	<a class="back" href="/">
		<span class="chev">‹</span>
		<span class="label">Traffic</span>
	</a>
	<span class="rule"></span>
	<h1>{liveNow?.callsign?.trim() || icao}</h1>
	<span class="tnum ident">
		{icao}{#if liveNow?.category}&nbsp;· {liveNow.category}{/if}
	</span>

	<span class="spacer"></span>

	<span class="tnum window">
		{#if loading && points.length === 0}
			loading…
		{:else if error}
			<span class="alert">{error}</span>
		{:else}
			{num(points.length)}{limitHit ? ` of ${num(LIMIT)}+` : ''} points
			{#if first}· since {clock(first.ts_ms)}{/if}
		{/if}
	</span>

	<div class="segmented">
		{#each RANGES as r (r.label)}
			<button class:on={rangeMs === r.ms} onclick={() => (rangeMs = r.ms)}>{r.label}</button>
		{/each}
	</div>

	<div class="segmented strong">
		<button class:on={view === 'chart'} onclick={() => (view = 'chart')}>CHART</button>
		<button class:on={view === 'table'} onclick={() => (view = 'table')}>TABLE</button>
	</div>
</div>

<main>
	<div class="content">
		{#if view === 'chart'}
			<TrackChart
				title="Altitude · barometric"
				note={ceiling !== null
					? `ceiling ${num(ceiling)} ft   ${climbFpm !== null ? `${climbFpm >= 0 ? 'climb +' : 'descend '}${num(climbFpm, 0)} fpm` : ''}`
					: ''}
				{points}
				value={(p) => p.altitude_ft}
				unit="ft"
				fill
				zeroBased
				{hoverMs}
				onhover={(ms) => (hoverMs = ms)}
			/>

			<TrackChart
				title="Ground speed"
				note={peakSpeed !== null && first && last
					? `peak ${num(peakSpeed)} kt   ${clock(first.ts_ms)} → ${clock(last.ts_ms)}`
					: ''}
				{points}
				value={(p) => p.ground_speed_kt}
				unit="kt"
				minSpan={80}
				{hoverMs}
				onhover={(ms) => (hoverMs = ms)}
			/>
		{:else}
			<div class="table-card">
				<table>
					<thead>
						<tr>
							<th class="label">Time</th>
							<th class="label right">Lat</th>
							<th class="label right">Lon</th>
							<th class="label right">Alt ft</th>
							<th class="label right">GS kt</th>
							<th class="label right">Trk</th>
						</tr>
					</thead>
					<tbody>
						{#each rows as p (p.ts_ms)}
							<tr
								class:on={hoverMs !== null &&
									Math.abs(p.ts_ms - hoverMs) < 1000}
								onpointerenter={() => (hoverMs = p.ts_ms)}
							>
								<td class="tnum">{clock(p.ts_ms)}</td>
								<td class="tnum right">{coord(p.lat)}</td>
								<td class="tnum right">{coord(p.lon)}</td>
								<td class="tnum right">{num(p.altitude_ft)}</td>
								<td class="tnum right">{num(p.ground_speed_kt)}</td>
								<td class="tnum right">{bearing(p.track_deg)}</td>
							</tr>
						{/each}
					</tbody>
				</table>

				{#if rows.length === 0 && !loading}
					<p class="empty tnum">no fixes recorded in this window</p>
				{/if}

				<div class="table-foot tnum">
					<span>{num(rows.length)} rows · newest first</span>
					{#if first && last}
						<span>{clock(first.ts_ms)} → {clock(last.ts_ms)}</span>
					{/if}
				</div>
			</div>
		{/if}
	</div>

	<aside>
		<div class="panel-head">
			<span class="label">Ground track</span>
			<span class="tnum count">{num(points.length)} fixes</span>
		</div>

		<GroundTrack
			{points}
			receiver={{ lat: receiver?.lat ?? null, lon: receiver?.lon ?? null }}
			{hoverMs}
		/>

		<dl>
			<div class="row">
				<dt class="label">First seen</dt>
				<dd class="tnum">{first ? clock(first.ts_ms) : NONE}</dd>
			</div>
			<div class="row">
				<dt class="label">Last fix</dt>
				<dd class="tnum">{last ? clock(last.ts_ms) : NONE}</dd>
			</div>
			<div class="row">
				<dt class="label">Duration</dt>
				<dd class="tnum">{first && last ? age(last.ts_ms - first.ts_ms) : NONE}</dd>
			</div>
			<div class="row">
				<dt class="label">Messages</dt>
				<dd class="tnum">{liveNow ? num(liveNow.messages) : NONE}</dd>
			</div>
			<div class="row">
				<dt class="label">Positions</dt>
				<dd class="tnum">{num(points.length)}</dd>
			</div>
			<div class="row">
				<dt class="label">Max range</dt>
				<dd class="tnum">{maxRangeNm === null ? NONE : `${num(maxRangeNm, 1)} nm`}</dd>
			</div>
			<div class="row">
				<dt class="label">Best signal</dt>
				<dd class="tnum">{liveNow?.rssi_dbfs !== undefined ? `${num(liveNow.rssi_dbfs, 1)} dBFS` : NONE}</dd>
			</div>
			<div class="row">
				<dt class="label">Solver</dt>
				<dd class="tnum">{liveNow?.position_source ?? NONE}</dd>
			</div>
		</dl>

		<div class="panel-foot">
			<span class="tnum source">from positions table</span>
			<button class="export" onclick={exportCsv} disabled={points.length === 0}>EXPORT CSV</button>
		</div>
	</aside>
</main>

<style>
	.subhead {
		display: flex;
		align-items: center;
		gap: 16px;
		flex-shrink: 0;
		height: 60px;
		padding-inline: 24px;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-line);
	}

	.back {
		display: flex;
		align-items: center;
		gap: 6px;
		color: var(--color-muted);
	}

	.back:hover {
		color: var(--color-accent);
	}

	.chev {
		font-size: 16px;
		line-height: 1;
	}

	.rule {
		width: 1px;
		height: 20px;
		background: var(--color-line);
	}

	h1 {
		margin: 0;
		font-size: 22px;
		font-weight: 900;
		letter-spacing: -0.02em;
	}

	.ident {
		font-size: 13px;
		color: var(--color-muted);
	}

	.spacer {
		flex: 1;
	}

	.window {
		font-size: 12px;
		color: var(--color-muted);
	}

	.window .alert {
		color: var(--color-alert);
	}

	.segmented {
		display: flex;
		background: var(--color-bg);
		border-radius: var(--radius-sm);
		padding: 3px;
	}

	.segmented button {
		padding: 5px 12px;
		font-size: 12px;
		border-radius: 2px;
		color: var(--color-muted);
	}

	.segmented button.on {
		background: var(--color-surface);
		color: var(--color-ink);
		box-shadow: 0 1px 2px rgb(15 26 36 / 0.08);
	}

	.segmented.strong button {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
	}

	.segmented.strong button.on {
		background: var(--color-accent);
		color: var(--color-surface);
	}

	main {
		display: flex;
		flex: 1;
		min-height: 0;
	}

	.content {
		display: flex;
		flex-direction: column;
		gap: 20px;
		flex: 1;
		min-width: 0;
		padding: 24px;
		overflow-y: auto;
	}

	aside {
		display: flex;
		flex-direction: column;
		flex-shrink: 0;
		width: 404px;
		background: var(--color-surface);
		border-left: 1px solid var(--color-line);
		overflow-y: auto;
	}

	.panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 22px 28px 0;
	}

	.count {
		font-size: 12px;
		color: var(--color-muted);
	}

	dl {
		margin: 0;
		border-top: 1px solid var(--color-line);
	}

	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: 40px;
		padding-inline: 28px;
		border-bottom: 1px solid var(--color-line-soft);
	}

	dt,
	dd {
		margin: 0;
	}

	dd {
		font-size: 13px;
	}

	.panel-foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-top: auto;
		padding: 20px 28px;
	}

	.source {
		font-size: 12px;
		color: var(--color-muted);
	}

	.export {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		color: var(--color-accent);
	}

	.export:disabled {
		color: var(--color-muted);
		cursor: default;
	}

	.table-card {
		display: flex;
		flex-direction: column;
		background: var(--color-surface);
		border: 1px solid var(--color-line);
		border-radius: var(--radius-md);
		overflow: hidden;
	}

	table {
		width: 100%;
		border-collapse: collapse;
	}

	th {
		padding: 14px 20px;
		text-align: left;
		border-bottom: 1px solid var(--color-line);
	}

	th.right {
		text-align: right;
	}

	td {
		padding: 8px 20px;
		font-size: 13px;
		border-bottom: 1px solid var(--color-line-soft);
	}

	td.right {
		text-align: right;
	}

	tbody tr.on {
		background: var(--color-accent-soft);
	}

	tbody tr.on td {
		color: var(--color-accent);
	}

	.table-foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 14px 20px;
		font-size: 12px;
		color: var(--color-muted);
	}

	.empty {
		margin: 0;
		padding: 40px 20px;
		text-align: center;
		font-size: 12px;
		color: var(--color-muted);
	}
</style>
