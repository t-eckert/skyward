<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import Banner from './Banner.svelte';

	const { Story } = defineMeta({
		title: 'Primitives/Banner',
		component: Banner,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			headline: 'Source unreachable for 12 s',
			detail: 'Showing the last snapshot. These positions are not current.',
			icon: true
		}
	});
</script>

<!-- Shell only, no trailing content. -->
<Story name="Plain" />

<!-- The disconnected form: a countdown and a retry action on the right. Now
     token-driven, so it follows the Chart theme instead of staying beige. -->
<Story name="Disconnected" asChild>
	<Banner
		headline="Source unreachable for 12 s"
		detail="Showing the last snapshot. These positions are not current."
	>
		{#snippet trailing()}
			<span class="tnum meta">next attempt in 3 s</span>
			<button class="action">RETRY NOW</button>
		{/snippet}
	</Banner>
</Story>

<!-- The source-down form: a reconnect count and a command chip. -->
<Story name="Source down" asChild>
	<Banner
		headline="No samples from the radio for 47 s"
		detail="The receiver is answering, so this is the tuner or the cable — check the antenna is seated."
	>
		{#snippet trailing()}
			<span class="tnum meta">0 reconnects</span>
			<code class="tnum chip">skyward doctor</code>
		{/snippet}
	</Banner>
</Story>

<style>
	.meta {
		font-size: 13px;
		white-space: nowrap;
		color: var(--color-muted);
	}
	.action {
		padding: 8px 14px;
		font-size: 11px;
		font-weight: 600;
		letter-spacing: 0.12em;
		color: var(--color-surface);
		background: var(--color-alert);
		border-radius: var(--radius-sm);
	}
	.chip {
		padding: 6px 10px;
		font-size: 13px;
		color: var(--color-ink);
		background: var(--color-bg);
		border-radius: var(--radius-sm);
	}
</style>
