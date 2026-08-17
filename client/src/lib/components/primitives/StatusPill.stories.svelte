<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import StatusPill from './StatusPill.svelte';

	const { Story } = defineMeta({
		title: 'Primitives/StatusPill',
		component: StatusPill,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: { state: 'ok', label: 'STREAMING' },
		argTypes: {
			state: { control: 'inline-radio', options: ['ok', 'caution'] }
		}
	});
</script>

<Story name="Streaming" />
<Story name="No signal" args={{ state: 'caution', label: 'NO SIGNAL' }} />
<Story name="Disconnected" args={{ state: 'caution', label: 'DISCONNECTED' }} />
