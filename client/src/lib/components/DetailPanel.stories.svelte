<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import DetailPanel from './DetailPanel.svelte';
	import { aircraft, envelope } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/DetailPanel',
		component: DetailPanel,
		tags: ['autodocs'],
		parameters: { layout: 'padded' },
		args: { aircraft: aircraft[0], nowMs: envelope.now_ms, frozen: false, count: 3 }
	});
</script>

<Story name="Selected" />
<Story name="No callsign" args={{ aircraft: aircraft[1] }} />
<Story name="Frozen (last known)" args={{ frozen: true }} />
<Story name="No selection" args={{ aircraft: null }} />
<Story name="No selection, empty sky" args={{ aircraft: null, count: 0 }} />
