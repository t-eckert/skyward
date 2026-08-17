<script lang="ts">
	import type { Snippet } from 'svelte';
	import WarningIcon from './WarningIcon.svelte';

	/**
	 * The full-width alert bar shared by the disconnected and source-down
	 * banners. They were structurally identical — icon, headline, detail, then a
	 * trailing area — but one hard-coded its colours as hex and so did not follow
	 * the Chart theme. This shell uses the tokens, so both adapt.
	 *
	 * The caller supplies the wording and whatever goes on the right (a countdown
	 * and a retry button, a reconnect count and a command) through `trailing`.
	 */
	interface Props {
		headline: string;
		detail: string;
		icon?: boolean;
		trailing?: Snippet;
	}
	let { headline, detail, icon = true, trailing }: Props = $props();
</script>

<div class="banner" role="status">
	{#if icon}<WarningIcon />{/if}
	<strong>{headline}</strong>
	<span class="detail">{detail}</span>
	<span class="spacer"></span>
	{#if trailing}{@render trailing()}{/if}
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
</style>
