<script lang="ts">
	import type { TrackPoint } from '$lib/api';
	import { num, hhmm, clock } from '$lib/format';

	/**
	 * One recorded series against time, with a shared crosshair.
	 *
	 * The line is drawn from the stored fixes and nothing is interpolated
	 * across gaps: when the receiver loses an aircraft for two minutes, the
	 * chart shows a break rather than a confident straight line through
	 * airspace nobody observed.
	 */

	interface Props {
		title: string;
		note?: string;
		points: TrackPoint[];
		/** Pulls the plotted value out of a fix; null means "not reported". */
		value: (p: TrackPoint) => number | null;
		unit: string;
		/** Fill under the line. Used for altitude, which reads as a profile. */
		fill?: boolean;
		/**
		 * Anchor the axis at zero.
		 *
		 * Altitude needs this. Auto-scaling turns the 150 ft of jitter in a
		 * level cruise into what looks like a dive, because the axis quietly
		 * spans 21 000 to 21 150 ft. Height has a meaningful zero, so show it.
		 */
		zeroBased?: boolean;
		/** Never render a band narrower than this, for the same reason. */
		minSpan?: number;
		decimals?: number;
		/** Shared crosshair time, in epoch ms. */
		hoverMs: number | null;
		onhover: (ms: number | null) => void;
	}

	let {
		title,
		note = '',
		points,
		value,
		unit,
		fill = false,
		zeroBased = false,
		minSpan = 0,
		decimals = 0,
		hoverMs,
		onhover
	}: Props = $props();

	const PAD = { top: 28, right: 24, bottom: 30, left: 62 };

	let width = $state(940);
	const height = 340;

	interface Sample {
		t: number;
		v: number;
	}

	const samples = $derived.by((): Sample[] =>
		points
			.map((p) => ({ t: p.ts_ms, v: value(p) }))
			.filter((s): s is Sample => s.v !== null && Number.isFinite(s.v))
	);

	const domain = $derived.by(() => {
		if (samples.length === 0) return null;
		const t0 = samples[0].t;
		const t1 = samples[samples.length - 1].t;
		const values = samples.map((s) => s.v);
		const vMin = Math.min(...values);
		const vMax = Math.max(...values);
		// A flat series still needs a band, or it divides by zero and vanishes.
		const pad = vMax - vMin < 1e-6 ? Math.max(1, Math.abs(vMax) * 0.1) : (vMax - vMin) * 0.12;

		let lo = zeroBased ? 0 : Math.max(0, vMin - pad);
		let hi = vMax + pad;

		// Widen a too-narrow band around its own centre, so a level cruise
		// reads as level instead of as violent manoeuvring.
		if (hi - lo < minSpan) {
			const mid = zeroBased ? minSpan / 2 : (vMin + vMax) / 2;
			lo = zeroBased ? 0 : Math.max(0, mid - minSpan / 2);
			hi = lo + minSpan;
		}

		return { t0, t1: t1 === t0 ? t0 + 1 : t1, vMin: lo, vMax: hi };
	});

	const plotW = $derived(width - PAD.left - PAD.right);
	const plotH = height - PAD.top - PAD.bottom;

	const x = $derived((t: number) =>
		domain ? PAD.left + ((t - domain.t0) / (domain.t1 - domain.t0)) * plotW : 0
	);
	const y = $derived((v: number) =>
		domain
			? PAD.top + plotH - ((v - domain.vMin) / (domain.vMax - domain.vMin)) * plotH
			: 0
	);

	/** Fixes more than this far apart are treated as separate segments. */
	const GAP_MS = 120_000;

	const segments = $derived.by((): Sample[][] => {
		const out: Sample[][] = [];
		let run: Sample[] = [];
		for (const s of samples) {
			if (run.length > 0 && s.t - run[run.length - 1].t > GAP_MS) {
				out.push(run);
				run = [];
			}
			run.push(s);
		}
		if (run.length > 0) out.push(run);
		return out;
	});

	const linePath = $derived(
		segments
			.map((seg) => seg.map((s, i) => `${i === 0 ? 'M' : 'L'}${x(s.t)} ${y(s.v)}`).join(' '))
			.join(' ')
	);

	const fillPath = $derived(
		fill
			? segments
					.filter((seg) => seg.length > 1)
					.map((seg) => {
						const top = seg.map((s, i) => `${i === 0 ? 'M' : 'L'}${x(s.t)} ${y(s.v)}`).join(' ');
						const base = PAD.top + plotH;
						return `${top} L${x(seg[seg.length - 1].t)} ${base} L${x(seg[0].t)} ${base} Z`;
					})
					.join(' ')
			: ''
	);

	/** Four horizontal gridlines at round values. */
	const ticksY = $derived.by(() => {
		if (!domain) return [];
		const count = 4;
		const raw = (domain.vMax - domain.vMin) / count;
		const magnitude = Math.pow(10, Math.floor(Math.log10(Math.max(raw, 1))));
		const stepChoices = [1, 2, 2.5, 5, 10].map((m) => m * magnitude);
		const stepSize = stepChoices.find((s) => s >= raw) ?? magnitude * 10;
		const out: number[] = [];
		for (let v = Math.ceil(domain.vMin / stepSize) * stepSize; v <= domain.vMax; v += stepSize) {
			out.push(v);
		}
		return out;
	});

	const ticksX = $derived.by(() => {
		if (!domain || samples.length === 0) return [];
		const count = 6;
		return Array.from(
			{ length: count + 1 },
			(_, i) => domain.t0 + ((domain.t1 - domain.t0) * i) / count
		);
	});

	/**
	 * Below ten minutes the axis needs seconds, or six ticks across a
	 * three-minute pass all print the same two or three minute labels.
	 */
	const timeLabel = $derived(
		domain && domain.t1 - domain.t0 < 10 * 60_000 ? clock : hhmm
	);

	/** The sample nearest the crosshair, for the readout. */
	const hovered = $derived.by((): Sample | null => {
		if (hoverMs === null || samples.length === 0) return null;
		let best = samples[0];
		for (const s of samples) {
			if (Math.abs(s.t - hoverMs) < Math.abs(best.t - hoverMs)) best = s;
		}
		return best;
	});

	function pointerMove(event: PointerEvent) {
		if (!domain) return;
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		const px = event.clientX - rect.left;
		const ratio = (px - PAD.left) / plotW;
		if (ratio < 0 || ratio > 1) {
			onhover(null);
			return;
		}
		onhover(domain.t0 + ratio * (domain.t1 - domain.t0));
	}
</script>

<figure bind:clientWidth={width}>
	<figcaption>
		<span class="label">{title}</span>
		{#if note}<span class="tnum note">{note}</span>{/if}
	</figcaption>

	<div
		class="plot"
		role="presentation"
		onpointermove={pointerMove}
		onpointerleave={() => onhover(null)}
	>
		<svg {width} {height}>
			{#each ticksY as t (t)}
				<line x1={PAD.left} y1={y(t)} x2={width - PAD.right} y2={y(t)} class="grid" />
				<text x={PAD.left - 10} y={y(t) + 4} class="tick" text-anchor="end">{num(t, 0)}</text>
			{/each}

			<!-- The first and last labels anchor inward, or they overhang the
			     card and get clipped by its rounded corner. -->
			{#each ticksX as t, i (i)}
				<text
					x={x(t)}
					y={height - 10}
					class="tick"
					text-anchor={i === 0 ? 'start' : i === ticksX.length - 1 ? 'end' : 'middle'}
				>
					{timeLabel(t)}
				</text>
			{/each}

			{#if fill && fillPath}
				<path d={fillPath} class="area" />
			{/if}
			<path d={linePath} class="line" />

			{#if hovered}
				<line
					x1={x(hovered.t)}
					y1={PAD.top}
					x2={x(hovered.t)}
					y2={PAD.top + plotH}
					class="crosshair"
				/>
				<circle cx={x(hovered.t)} cy={y(hovered.v)} r="4" class="knob" />
			{/if}

			{#if samples.length > 0}
				<circle
					cx={x(samples[samples.length - 1].t)}
					cy={y(samples[samples.length - 1].v)}
					r="4"
					class="knob"
				/>
			{/if}
		</svg>

		{#if samples.length === 0}
			<p class="empty tnum">no {title.toLowerCase()} recorded in this window</p>
		{/if}

		{#if hovered}
			<div
				class="readout tnum"
				style="left: {Math.min(x(hovered.t) + 14, width - 190)}px; top: {y(hovered.v) - 10}px"
			>
				<span class="time">{timeLabel(hovered.t)}</span>
				<span class="row">
					<span class="swatch"></span>
					<strong>{num(hovered.v, decimals)} {unit}</strong>
				</span>
			</div>
		{/if}
	</div>
</figure>

<style>
	figure {
		margin: 0;
		background: var(--color-panel);
		border: 1px solid var(--color-line);
		border-radius: var(--radius-md);
	}

	figcaption {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding: 20px 24px 0;
	}

	.note {
		font-size: 12px;
		color: var(--color-muted);
	}

	.plot {
		position: relative;
	}

	svg {
		display: block;
	}

	.grid {
		stroke: var(--color-line-soft);
		stroke-width: 1;
	}

	.tick {
		font-size: 11px;
		fill: var(--color-muted);
	}

	.line {
		fill: none;
		stroke: var(--color-accent);
		stroke-width: 1.75;
		stroke-linejoin: round;
		stroke-linecap: round;
	}

	.area {
		fill: var(--color-accent-soft);
		opacity: 0.75;
	}

	.crosshair {
		stroke: var(--color-muted);
		stroke-width: 1;
	}

	.knob {
		fill: var(--color-accent);
	}

	.readout {
		position: absolute;
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 10px 14px;
		background: var(--color-panel);
		border: 1px solid var(--color-line);
		border-radius: var(--radius-sm);
		box-shadow: 0 2px 8px rgb(0 0 0 / 0.28);
		pointer-events: none;
		white-space: nowrap;
	}

	.time {
		font-size: 11px;
		color: var(--color-muted);
	}

	.row {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14px;
	}

	.swatch {
		width: 12px;
		height: 2px;
		background: var(--color-accent);
	}

	.empty {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		margin: 0;
		font-size: 12px;
		color: var(--color-muted);
	}
</style>
