<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import NoTraffic from './NoTraffic.svelte';
	import { envelope } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/NoTraffic',
		component: NoTraffic,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			lastHeard: { label: 'UAL1234', atMs: envelope.now_ms - 134_000 },
			quietMs: 134_000,
			peakRate: 172
		}
	});
</script>

<Story name="Quiet" />
<Story name="Never heard" args={{ lastHeard: null, quietMs: null, peakRate: 0 }} />
