<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import Scoreboard from './Scoreboard.svelte';
	import { stats } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/Scoreboard',
		component: Scoreboard,
		tags: ['autodocs'],
		parameters: { layout: 'fullscreen' },
		args: {
			stats,
			messagesPerMinute: 172.3,
			yieldPct: 58.8,
			realtimeFactor: 361.5,
			connection: 'streaming',
			retryInS: 0
		}
	});
</script>

<Story name="Streaming" />
<Story name="Realtime falling behind" args={{ realtimeFactor: 0.94 }} />
<Story name="Disconnected" args={{ connection: 'disconnected', retryInS: 3 }} />
<Story name="No stats yet" args={{ stats: null, messagesPerMinute: null, yieldPct: null, realtimeFactor: null }} />
