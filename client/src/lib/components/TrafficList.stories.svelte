<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import { fn } from 'storybook/test';
	import TrafficList from './TrafficList.svelte';
	import { aircraft, RECEIVER_LATLON, envelope } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/TrafficList',
		component: TrafficList,
		tags: ['autodocs'],
		parameters: { layout: 'padded' },
		args: {
			aircraft,
			receiver: RECEIVER_LATLON,
			nowMs: envelope.now_ms,
			selected: null,
			frozen: false,
			onselect: fn()
		}
	});
</script>

<Story name="Default" />
<Story name="With selection" args={{ selected: 'A0A41F' }} />
<Story name="Frozen" args={{ frozen: true }} />
<Story name="Position unset" args={{ receiver: { lat: null, lon: null } }} />
<Story name="Empty" args={{ aircraft: [] }} />
