<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import Stat from './Stat.svelte';

	const { Story } = defineMeta({
		title: 'Primitives/Stat',
		component: Stat,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: {
			label: 'Altitude',
			value: '34 025',
			unit: 'ft · baro',
			size: 'value',
			tone: 'default',
			orientation: 'stacked'
		},
		argTypes: {
			size: { control: 'inline-radio', options: ['tile', 'value', 'hero'] },
			tone: { control: 'inline-radio', options: ['default', 'alert', 'accent', 'muted'] },
			orientation: { control: 'inline-radio', options: ['stacked', 'inline'] }
		}
	});
</script>

<!-- The knob-driven one: switch size/tone/orientation live. -->
<Story name="Playground" />

<!-- The forms it replaces across the app, so you can tune them side by side. -->
<Story name="Detail stat" args={{ label: 'Ground spd', value: '458', unit: 'kt' }} />
<Story name="Hero — msg/min" args={{ label: 'Messages / min', value: '172.3', size: 'hero', unit: undefined }} />
<Story name="Scoreboard tile" args={{ label: 'Yield', value: '58.8 %', size: 'tile', unit: undefined }} />
<Story
	name="Realtime alert"
	args={{ label: 'Realtime', value: '0.94 ×', size: 'tile', tone: 'alert', unit: undefined }}
/>
<Story
	name="Inline row"
	args={{ label: 'Quiet for', value: '2 m 14 s', orientation: 'inline', size: 'tile', unit: undefined }}
/>
