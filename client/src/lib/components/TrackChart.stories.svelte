<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import { fn } from 'storybook/test';
	import type { TrackPoint } from '$lib/api';
	import TrackChart from './TrackChart.svelte';
	import { track } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/TrackChart',
		component: TrackChart,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			title: 'Altitude',
			note: 'baro',
			points: track,
			value: (p: TrackPoint) => p.altitude_ft,
			unit: 'ft',
			fill: true,
			zeroBased: true,
			minSpan: 2_000,
			decimals: 0,
			hoverMs: null,
			onhover: fn()
		}
	});
</script>

<Story name="Altitude profile" />
<Story
	name="Ground speed"
	args={{
		title: 'Ground speed',
		note: '',
		value: (p: TrackPoint) => p.ground_speed_kt,
		unit: 'kt',
		fill: false,
		zeroBased: false
	}}
/>
<Story name="No data" args={{ points: [] }} />
