<script lang="ts">
	import { THEMES, applyTheme } from '$lib/themes';

	/**
	 * Switches between the two directions.
	 *
	 * Styled from the token set rather than fixed values, unlike the throwaway
	 * comparison control it replaces: this ships, so it should look like part
	 * of the instrument in whichever theme is showing. Hidden by `?chrome=0`,
	 * which is how the screenshots are taken.
	 */
	interface Props {
		current: string;
		onchange: (id: string) => void;
	}

	let { current, onchange }: Props = $props();

	function pick(id: string) {
		applyTheme(id);
		onchange(id);
		const url = new URL(window.location.href);
		url.searchParams.set('theme', id);
		history.replaceState({}, '', url);
	}
</script>

<div class="switcher" role="group" aria-label="Display theme">
	{#each THEMES as t (t.id)}
		<button class:on={t.id === current} onclick={() => pick(t.id)} title={t.note}>
			{t.name}
		</button>
	{/each}
</div>

<style>
	.switcher {
		display: flex;
		gap: 2px;
		padding: 2px;
		background: var(--color-bg);
		border: 1px solid var(--color-line);
		border-radius: var(--radius-sm);
	}

	button {
		padding: 4px 10px;
		font-size: var(--size-label);
		font-weight: var(--weight-label);
		letter-spacing: var(--tracking-label);
		text-transform: uppercase;
		color: var(--color-muted);
		border-radius: calc(var(--radius-sm) - 1px);
		white-space: nowrap;
	}

	button:hover {
		color: var(--color-ink);
	}

	button.on {
		color: var(--color-ink);
		background: var(--color-panel);
	}
</style>
