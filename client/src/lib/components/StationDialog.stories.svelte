<script module lang="ts">
	import { defineMeta } from '@storybook/addon-svelte-csf';
	import StationDialog from './StationDialog.svelte';
	import { receiver, receiverUnset, receiverRuntime, receiverLocked } from '$lib/mocks';

	const { Story } = defineMeta({
		title: 'Components/StationDialog',
		component: StationDialog,
		parameters: { layout: 'fullscreen' }
	});

	/** Stories never talk to a server; a save resolves and closes. */
	const noop = async () => {};
</script>

<Story name="Configured">
	<StationDialog {receiver} onsave={noop} onrevert={noop} onclose={() => {}} />
</Story>

<!-- The state a fresh receiver actually starts in, and the reason the dialog
     exists: no position anywhere, so no range gate and no local CPR. -->
<Story name="Unset">
	<StationDialog receiver={receiverUnset} onsave={noop} onrevert={noop} onclose={() => {}} />
</Story>

<!-- A position set here is persisted and outranks the config file. The origin
     line and the REVERT action are what keep that from becoming the "I edited
     .env and nothing happened" mystery. -->
<Story name="Set at runtime">
	<StationDialog receiver={receiverRuntime} onsave={noop} onrevert={noop} onclose={() => {}} />
</Story>

<!-- station_writable = false: every field disabled, and a reason given. An
     inert Save button with no explanation would read as a bug. -->
<Story name="Writes disabled">
	<StationDialog receiver={receiverLocked} onsave={noop} onrevert={noop} onclose={() => {}} />
</Story>

<!-- The server rejects out-of-range values with a sentence, and it has to
     survive all the way to the operator rather than becoming "400". -->
<Story name="Rejected by the server">
	<StationDialog
		{receiver}
		onsave={async () => {
			throw new Error('lat 91 is not a latitude (-90 to 90)');
		}}
		onrevert={noop}
		onclose={() => {}}
	/>
</Story>
