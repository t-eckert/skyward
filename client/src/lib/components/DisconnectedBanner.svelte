<script lang="ts">
	import { age } from '$lib/format';

	interface Props {
		outageMs: number;
		retryInS: number;
		onretry: () => void;
	}

	let { outageMs, retryInS, onretry }: Props = $props();
</script>

<!-- Says three things in order: how long it has been wrong, that what you are
     looking at is old, and when it will try again. A banner that only says
     "disconnected" leaves the operator to guess whether the aircraft on the
     plot are real. -->
<div class="banner" role="status">
	<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
		<path
			d="M8 1.5 L15 14.5 L1 14.5 Z"
			fill="none"
			stroke="var(--color-alert)"
			stroke-width="1.4"
			stroke-linejoin="round"
		/>
		<path d="M8 6 v4" stroke="var(--color-alert)" stroke-width="1.4" stroke-linecap="round" />
		<circle cx="8" cy="12.2" r="0.8" fill="var(--color-alert)" />
	</svg>

	<strong>Source unreachable for {age(outageMs)}</strong>
	<span class="detail">Showing the last snapshot. These positions are not current.</span>

	<span class="spacer"></span>
	<span class="tnum next">next attempt in {retryInS} s</span>
	<button onclick={onretry}>RETRY NOW</button>
</div>

<style>
	.banner {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-shrink: 0;
		height: 56px;
		padding-inline: 24px;
		background: #f7ece8;
		border-bottom: 1px solid #e6cfc7;
		color: var(--color-alert);
	}

	strong {
		font-size: 14px;
		font-weight: 600;
	}

	.detail {
		font-size: 14px;
		color: #7d4530;
	}

	.spacer {
		flex: 1;
	}

	.next {
		font-size: 13px;
		color: #7d4530;
	}

	button {
		padding: 8px 14px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		color: var(--color-surface);
		background: var(--color-alert);
		border-radius: var(--radius-sm);
	}

	button:hover {
		background: #8d3f23;
	}
</style>
