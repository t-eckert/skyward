<script lang="ts">
	import '../app.css';

	let { children } = $props();
</script>

<svelte:head>
	<title>Skyward</title>
	<meta name="description" content="ADS-B receiver — live aircraft by range and bearing" />
</svelte:head>

<div class="app">
	{@render children()}
</div>

<style>
	.app {
		display: flex;
		flex-direction: column;
		height: 100vh;
		min-height: 0;
	}
</style>
