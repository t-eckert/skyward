<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import GroundTrack from './GroundTrack.svelte';
	import { track, RECEIVER_LATLON } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/GroundTrack',
		component: GroundTrack,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: { points: track, receiver: RECEIVER_LATLON, hoverMs: null }
	});
</script>

<Story name="Track" />
<Story name="Highlighted fix" args={{ hoverMs: track[Math.floor(track.length / 2)].ts_ms }} />
<Story name="Position unset" args={{ receiver: { lat: null, lon: null } }} />
