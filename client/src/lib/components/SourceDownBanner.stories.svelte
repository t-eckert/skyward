<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import SourceDownBanner from './SourceDownBanner.svelte';

	const { Story } = defineMeta({
		title: 'Components/SourceDownBanner',
		component: SourceDownBanner,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			silentMs: 47_000,
			reconnects: 0,
			description: 'tcp:127.0.0.1:1234 (R820T tuner, 29 gain steps) at 2.400 MS/s'
		}
	});
</script>

<Story name="No samples" />
<Story name="After reconnects" args={{ silentMs: 8_000, reconnects: 3 }} />
