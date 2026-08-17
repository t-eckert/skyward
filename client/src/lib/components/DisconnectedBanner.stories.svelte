<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import { fn } from 'storybook/test';
	import DisconnectedBanner from './DisconnectedBanner.svelte';

	const { Story } = defineMeta({
		title: 'Components/DisconnectedBanner',
		component: DisconnectedBanner,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: { outageMs: 12_000, retryInS: 3, onretry: fn() }
	});
</script>

<Story name="Just dropped" />
<Story name="Long outage" args={{ outageMs: 372_000, retryInS: 20 }} />
