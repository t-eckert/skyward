<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import WarningIcon from './WarningIcon.svelte';

	const { Story } = defineMeta({
		title: 'Primitives/WarningIcon',
		component: WarningIcon,
		tags: ['autodocs'],
		parameters: { layout: 'centered' },
		args: { size: 32 }
	});
</script>

<Story name="Default" />

<!-- It inherits currentColor, so it takes the caution colour from its context. -->
<Story name="Alert colour" asChild>
	<span style="color: var(--color-alert)"><WarningIcon size={40} /></span>
</Story>
