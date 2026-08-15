<script lang="ts">
	import type { Aircraft } from '$lib/api';
	import AircraftGlyph from './AircraftGlyph.svelte';
	import { num, age, haversineKm, kmToNm } from '$lib/format';

	type Sort = 'range' | 'altitude' | 'seen';

	interface Props {
		aircraft: Aircraft[];
		receiver: { lat: number | null; lon: number | null };
		nowMs: number;
		selected: string | null;
		frozen?: boolean;
		onselect: (icao: string | null) => void;
	}

	let { aircraft, receiver, nowMs, selected, frozen = false, onselect }: Props = $props();

	let sort = $state<Sort>('range');

	const STALE_MS = 30_000;

	const LABEL: Record<Sort, string> = {
		range: 'BY RANGE',
		altitude: 'BY ALTITUDE',
		seen: 'BY LAST SEEN'
	};

	const ORDER: Sort[] = ['range', 'altitude', 'seen'];

	function rangeNm(a: Aircraft): number | null {
		if (receiver.lat === null || receiver.lon === null) return null;
		if (a.lat === undefined || a.lon === undefined) return null;
		return kmToNm(haversineKm(receiver.lat, receiver.lon, a.lat, a.lon));
	}

	const rows = $derived.by(() => {
		const list = [...aircraft];
		// Aircraft with no position sort last in every mode: they cannot be
		// ranged, and floating them to the top would bury the traffic that can.
		const missingLast = (v: number | null) => (v === null ? Number.POSITIVE_INFINITY : v);
		if (sort === 'range') {
			list.sort((a, b) => missingLast(rangeNm(a)) - missingLast(rangeNm(b)));
		} else if (sort === 'altitude') {
			list.sort((a, b) => (b.altitude_ft ?? -1) - (a.altitude_ft ?? -1));
		} else {
			list.sort((a, b) => a.seen_ms - b.seen_ms);
		}
		return list;
	});

	const located = $derived(aircraft.filter((a) => a.lat !== undefined).length);
</script>

<div class="list-header">
	<span class="label">
		Traffic
		<span class="count tnum">
			{aircraft.length}
			{frozen ? 'held' : 'tracked'} · {located} located
		</span>
	</span>
	<button
		class="sort"
		onclick={() => (sort = ORDER[(ORDER.indexOf(sort) + 1) % ORDER.length])}
		title="Change sort order"
	>
		{LABEL[sort]}
	</button>
</div>

<div class="rows">
	{#each rows as a (a.icao)}
		{@const isSelected = a.icao === selected}
		{@const stale = nowMs - a.last_seen_ms > STALE_MS}
		<button
			class="row"
			class:selected={isSelected}
			class:stale
			onclick={() => onselect(isSelected ? null : a.icao)}
		>
			<span class="glyph">
				<AircraftGlyph track={a.track_deg} hollow={stale} />
			</span>

			<span class="ident">
				<span class="tnum call">{a.callsign?.trim() || a.icao}</span>
				<span class="tnum icao">
					{a.callsign?.trim() ? a.icao : 'no callsign'}
				</span>
			</span>

			<span class="tnum figure alt">{num(a.altitude_ft)}</span>
			<span class="tnum figure spd">{num(a.ground_speed_kt)}</span>
			<span class="tnum seen" class:alert={stale}>{age(nowMs - a.last_seen_ms)}</span>
		</button>
	{/each}

	{#if rows.length === 0}
		<p class="empty tnum">no aircraft tracked</p>
	{/if}
</div>

<style>
	.list-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		flex-shrink: 0;
		height: var(--list-head-h);
		padding-inline: var(--panel-pad);
		border-bottom: 1px solid var(--color-line);
	}

	.count {
		margin-left: 8px;
		font-size: 11px;
		font-weight: 400;
		letter-spacing: 0;
		text-transform: none;
	}

	.sort {
		font-size: var(--size-label);
		font-weight: 600;
		letter-spacing: var(--tracking-label);
		color: var(--color-muted);
	}

	.sort:hover {
		color: var(--color-accent);
	}

	.rows {
		flex: 1;
		overflow-y: auto;
		min-height: 0;
	}

	/* Fixed-width lanes for every column. Relying on gap alone would let a
	   four-digit altitude shift the whole row out of line with its neighbours. */
	.row {
		position: relative;
		display: flex;
		align-items: center;
		gap: 14px;
		width: 100%;
		height: var(--row-h);
		padding-inline: var(--panel-pad);
		text-align: left;
		color: var(--color-live);
		border-bottom: 1px solid var(--color-line-soft);
	}

	.row:hover {
		background: var(--color-line-soft);
	}

	.row.selected {
		background: var(--color-accent-soft);
	}

	.row.selected::before {
		content: '';
		position: absolute;
		left: 0;
		top: 0;
		width: var(--select-bar-w);
		height: 100%;
		background: var(--color-selected);
	}

	.row.stale {
		color: var(--color-stale);
	}

	.glyph {
		flex-shrink: 0;
	}

	.ident {
		display: flex;
		flex-direction: column;
		flex-shrink: 0;
		gap: 2px;
		width: 112px;
	}

	.call {
		font-size: var(--size-row-call);
		font-weight: 600;
		letter-spacing: var(--tracking-call);
		line-height: 18px;
		color: inherit;
	}

	.icao {
		font-size: var(--size-row-sub);
		line-height: 14px;
		color: var(--color-muted);
	}

	.figure {
		flex-shrink: 0;
		font-size: var(--size-row-figure);
		line-height: 18px;
		text-align: right;
		color: var(--color-ink);
	}

	.row.stale .figure {
		color: var(--color-stale);
	}

	.alt {
		width: 64px;
	}

	.spd {
		width: 48px;
	}

	.seen {
		flex: 1;
		font-size: 12px;
		line-height: 16px;
		text-align: right;
		white-space: nowrap;
		color: var(--color-muted);
	}

	.seen.alert {
		color: var(--color-alert);
	}

	.empty {
		margin: 0;
		padding: 28px;
		font-size: 12px;
		color: var(--color-muted);
	}
</style>
