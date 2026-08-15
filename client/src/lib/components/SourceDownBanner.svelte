<script lang="ts">
	import { age } from '$lib/format';

	interface Props {
		silentMs: number | null;
		reconnects: number;
		description: string;
	}

	let { silentMs, reconnects, description }: Props = $props();
</script>

<!-- Distinct from the disconnected banner on purpose. There the receiver is
     unreachable and there is nothing to do but wait; here the receiver is
     answering fine and the *radio* has stopped, which is a physical problem
     at the other end of the cable. The first thing to check is the one that
     is actually most often wrong. -->
<div class="banner" role="status">
	<svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
		<path
			d="M8 1.5 L15 14.5 L1 14.5 Z"
			fill="none"
			stroke="currentColor"
			stroke-width="1.4"
			stroke-linejoin="round"
		/>
		<path d="M8 6 v4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
		<circle cx="8" cy="12.2" r="0.8" fill="currentColor" />
	</svg>

	<strong>
		No samples from the radio{silentMs === null ? '' : ` for ${age(silentMs)}`}
	</strong>
	<span class="detail">
		The receiver is answering, so this is the tuner or the cable — check the antenna is seated
		before suspecting software.
	</span>

	<span class="spacer"></span>
	<span class="tnum next">
		{reconnects}
		{reconnects === 1 ? 'reconnect' : 'reconnects'}
	</span>
	<code class="tnum">skyward doctor</code>
</div>

<style>
	.banner {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-shrink: 0;
		height: 56px;
		padding-inline: 24px;
		background: var(--color-accent-soft);
		border-bottom: 1px solid var(--color-line);
		color: var(--color-alert);
	}

	strong {
		font-size: 14px;
		font-weight: 600;
		white-space: nowrap;
	}

	.detail {
		font-size: 14px;
		color: var(--color-muted);
	}

	.spacer {
		flex: 1;
	}

	.next {
		font-size: 13px;
		white-space: nowrap;
		color: var(--color-muted);
	}

	code {
		padding: 6px 10px;
		font-size: 13px;
		white-space: nowrap;
		color: var(--color-ink);
		background: var(--color-bg);
		border-radius: var(--radius-sm);
	}
</style>
