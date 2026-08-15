<script lang="ts">
	import type { Aircraft } from '$lib/api';
	import { num, signed, bearing, age, NONE } from '$lib/format';

	interface Props {
		aircraft: Aircraft | null;
		nowMs: number;
		frozen?: boolean;
		count: number;
	}

	let { aircraft, nowMs, frozen = false, count }: Props = $props();

	const heardMs = $derived(aircraft ? nowMs - aircraft.last_seen_ms : 0);

	// "climb" / "descend" beats a bare sign at a glance, and the sign is still
	// there for anyone reading the number rather than the label.
	const verticalNote = $derived.by(() => {
		const rate = aircraft?.vertical_rate_fpm;
		if (rate === undefined || rate === null) return 'fpm';
		if (rate > 64) return 'fpm · climb';
		if (rate < -64) return 'fpm · descend';
		return 'fpm · level';
	});
</script>

<section class="detail">
	{#if aircraft}
		<div class="head">
			<span class="eyebrow">{frozen ? 'LAST KNOWN' : 'SELECTED'}</span>
			<h1>{aircraft.callsign?.trim() || aircraft.icao}</h1>
			<p class="tnum sub">
				{aircraft.icao}
				{#if aircraft.category}· {aircraft.category}{/if}
				· {frozen ? `frozen ${age(heardMs)} ago` : `heard ${age(heardMs)}`}
			</p>
		</div>

		<div class="grid">
			<div class="stat">
				<span class="label">Altitude</span>
				<span class="tnum value">{num(aircraft.altitude_ft)}</span>
				<span class="tnum unit">ft · {aircraft.altitude_source ?? NONE}</span>
			</div>
			<div class="stat">
				<span class="label">Ground spd</span>
				<span class="tnum value">{num(aircraft.ground_speed_kt)}</span>
				<span class="tnum unit">kt</span>
			</div>
			<div class="stat">
				<span class="label">Track</span>
				<span class="tnum value">{bearing(aircraft.track_deg)}</span>
				<span class="tnum unit">true</span>
			</div>
			<div class="stat">
				<span class="label">Vertical</span>
				<span class="tnum value">{signed(aircraft.vertical_rate_fpm)}</span>
				<span class="tnum unit">{verticalNote}</span>
			</div>
			<div class="stat">
				<span class="label">Signal</span>
				<span class="tnum value">{num(aircraft.rssi_dbfs, 1)}</span>
				<span class="tnum unit">dBFS</span>
			</div>
			<div class="stat">
				<span class="label">Last fix</span>
				<span class="tnum value" class:alert={frozen}>
					{aircraft.position_age_ms !== undefined ? age(aircraft.position_age_ms) : NONE}
				</span>
				<span class="tnum unit">{frozen ? 'no update' : (aircraft.position_source ?? NONE)}</span>
			</div>
		</div>

		<a class="history tnum" href="/aircraft/{aircraft.icao}">View recorded history →</a>
	{:else}
		<div class="head">
			<span class="eyebrow muted">NO SELECTION</span>
			<h1 class="dim">{count > 0 ? 'Pick an aircraft' : 'Standing by'}</h1>
			<p class="tnum sub">
				{#if count > 0}
					{count} tracked · choose one on the plot or in the list below
				{:else}
					nothing tracked right now
				{/if}
			</p>
		</div>
	{/if}
</section>

<style>
	.detail {
		display: flex;
		flex-direction: column;
		flex-shrink: 0;
		gap: 22px;
		padding: var(--panel-pad) var(--panel-pad) calc(var(--panel-pad) - 4px);
		border-bottom: 1px solid var(--color-line);
	}

	.head {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.eyebrow {
		font-size: var(--size-label);
		font-weight: 600;
		letter-spacing: var(--tracking-label);
		line-height: 14px;
		color: var(--color-accent);
	}

	.eyebrow.muted {
		color: var(--color-muted);
	}

	h1 {
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--size-title);
		font-weight: var(--weight-title);
		letter-spacing: -0.02em;
		line-height: var(--leading-title);
		color: var(--color-ink);
	}

	h1.dim {
		color: var(--color-muted);
	}

	.sub {
		margin: 0;
		font-size: var(--size-meta);
		line-height: 20px;
		color: var(--color-muted);
	}

	/* Three fixed columns, so the second row's labels sit in the same lanes as
	   the first row's even when a value is missing. */
	.grid {
		display: grid;
		grid-template-columns: repeat(3, 116px);
		row-gap: 18px;
	}

	.stat {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.value {
		font-size: var(--size-value);
		font-weight: 500;
		line-height: calc(var(--size-value) + 4px);
		color: var(--color-ink);
	}

	.value.alert {
		color: var(--color-alert);
	}

	.unit {
		font-size: var(--size-unit);
		line-height: 14px;
		color: var(--color-muted);
	}

	.history {
		font-size: 12px;
		line-height: 16px;
		color: var(--color-accent);
	}

	.history:hover {
		text-decoration: underline;
	}
</style>
