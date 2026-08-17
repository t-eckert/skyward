<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import AircraftGlyph from './AircraftGlyph.svelte';

	const { Story } = defineMeta({
		title: 'Components/AircraftGlyph',
		component: AircraftGlyph,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: { track: 71, size: 48, hollow: false },
		argTypes: {
			track: { control: { type: 'range', min: 0, max: 359, step: 1 } },
			size: { control: { type: 'range', min: 12, max: 96, step: 1 } }
		}
	});
</script>

<Story name="Tracked" />
<Story name="Unknown track (hollow)" args={{ track: null, hollow: true }} />
