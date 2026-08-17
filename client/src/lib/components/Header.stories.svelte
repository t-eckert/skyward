<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import Header from './Header.svelte';
	import { receiver } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/Header',
		component: Header,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			receiver,
			connection: 'streaming',
			lastSnapshotMs: receiver.uptime_s ? 1_786_982_988_000 : null,
			status: 'streaming'
		},
		argTypes: {
			status: { control: 'inline-radio', options: ['streaming', 'source-down', 'disconnected'] },
			connection: { control: 'inline-radio', options: ['connecting', 'streaming', 'disconnected'] }
		}
	});
</script>

<Story name="Streaming" />
<Story name="No signal" args={{ status: 'source-down' }} />
<Story name="Disconnected" args={{ status: 'disconnected', connection: 'disconnected' }} />
<Story name="Position unset" args={{ receiver: { ...receiver, lat: null, lon: null } }} />
