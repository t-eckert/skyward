<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import { fn } from 'storybook/test';
	import ThemeSwitcher from './ThemeSwitcher.svelte';

	const { Story } = defineMeta({
		title: 'Components/ThemeSwitcher',
		component: ThemeSwitcher,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: { current: 'flightdeck', onchange: fn() },
		argTypes: { current: { control: 'inline-radio', options: ['flightdeck', 'chart'] } }
	});
</script>

<!-- Note: this control writes the real `?theme=` and `data-theme`, so clicking
     it also drives the whole preview — the same thing it does in the app. -->
<Story name="Flight deck selected" />
<Story name="Chart selected" args={{ current: 'chart' }} />
