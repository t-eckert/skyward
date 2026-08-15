<script lang="ts">
	import { age, clock, num, NONE } from '$lib/format';

	interface Props {
		lastHeard: { label: string; atMs: number } | null;
		quietMs: number | null;
		peakRate: number;
	}

	let { lastHeard, quietMs, peakRate }: Props = $props();

	// Overnight silence is normal and daytime silence is not, so the copy says
	// which case this probably is instead of raising an alarm either way.
	const hour = $derived(new Date().getHours());
	const overnight = $derived(hour >= 23 || hour < 6);
</script>

<section class="panel">
	<div class="head">
		<span class="eyebrow">NO TRAFFIC</span>
		<h1>Nothing in range</h1>
		<p class="body">
			The decoder is healthy and the source is streaming — there have simply been no CRC-valid
			messages{quietMs === null ? '' : ` for ${age(quietMs)}`}.
			{#if overnight}Overnight this is normal.{/if}
		</p>
	</div>

	<dl>
		<div class="row">
			<dt class="label">Last heard</dt>
			<dd class="tnum">
				{#if lastHeard}
					{lastHeard.label} · {clock(lastHeard.atMs)}
				{:else}
					{NONE}
				{/if}
			</dd>
		</div>
		<div class="row">
			<dt class="label">Quiet for</dt>
			<dd class="tnum">{quietMs === null ? NONE : age(quietMs)}</dd>
		</div>
		<div class="row">
			<!-- Deliberately "seen", not "today": this is the peak observed
			     since the page was opened, and the client has no history
			     before that to claim otherwise. -->
			<dt class="label">Peak seen</dt>
			<dd class="tnum">{peakRate > 0 ? `${num(peakRate, 0)} / min` : NONE}</dd>
		</div>
	</dl>

	<div class="hint">
		<p>Still quiet in daylight? Suspect the antenna before the software.</p>
		<code class="tnum">skyward doctor</code>
	</div>
</section>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.head {
		display: flex;
		flex-direction: column;
		gap: 6px;
		padding: 28px 28px 24px;
		border-bottom: 1px solid var(--color-line);
	}

	.eyebrow {
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		line-height: 14px;
		color: var(--color-muted);
	}

	h1 {
		margin: 0;
		font-size: 28px;
		font-weight: 900;
		letter-spacing: -0.02em;
		line-height: 34px;
	}

	.body {
		margin: 6px 0 0;
		font-size: 14px;
		line-height: 22px;
		color: var(--color-muted);
	}

	dl {
		margin: 0;
	}

	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: 45px;
		padding-inline: 28px;
		border-bottom: 1px solid var(--color-line-soft);
	}

	dt {
		margin: 0;
	}

	dd {
		margin: 0;
		font-size: 13px;
		color: var(--color-ink);
	}

	.hint {
		margin-top: auto;
		padding: 24px 28px 28px;
		border-top: 1px solid var(--color-line);
	}

	.hint p {
		margin: 0 0 12px;
		font-size: 13px;
		line-height: 20px;
		color: var(--color-muted);
	}

	code {
		display: block;
		padding: 10px 14px;
		font-size: 13px;
		color: var(--color-ink);
		background: var(--color-bg);
		border-radius: var(--radius-sm);
	}
</style>
