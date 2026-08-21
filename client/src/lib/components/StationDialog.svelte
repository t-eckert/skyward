<script lang="ts">
	import { untrack } from 'svelte';
	import type { Receiver, StationPosition } from '$lib/api';
	import { station as formatStation } from '$lib/format';

	/**
	 * Where the receiver thinks it is, and how to move it.
	 *
	 * # Why this exists at all
	 *
	 * The station position is the one setting whose correct value is
	 * discovered rather than configured: you put the Pi somewhere, you move
	 * the antenna, someone borrows the whole thing. Until now that meant
	 * editing a `.env` on a machine you deliberately do not log into and
	 * restarting, which drops every tracked aircraft. The server accepts it at
	 * runtime; this is the surface for it.
	 *
	 * # Three decimals, and why the form says so
	 *
	 * Local CPR only needs the reference to land in the same ~670 km zone as
	 * the aircraft, and the range gate works at 400 km. Three decimals is
	 * about 100 m and is already far more than enough. More precision buys
	 * nothing here and publishing a home address costs something, so the field
	 * rounds on blur rather than merely suggesting it.
	 *
	 * # The origin line is not decoration
	 *
	 * The server tracks where every value came from precisely so "did my edit
	 * take effect" is answerable. A dialog that showed the numbers but not
	 * their provenance would throw that away at the last step — and the
	 * confusing case is real: a position set here is persisted and outranks
	 * the config file, so someone who later edits `.env` and sees no change
	 * needs to be told why, here, where they can undo it.
	 */
	interface Props {
		receiver: Receiver | null;
		onsave: (position: StationPosition) => Promise<void>;
		onrevert: () => Promise<void>;
		onclose: () => void;
	}

	let { receiver, onsave, onrevert, onclose }: Props = $props();

	// Seeded once, deliberately. The metadata poll refreshes `receiver` every
	// five seconds, and a field that re-seeded from it would erase whatever the
	// user had half-typed mid-keystroke. `untrack` makes that intent explicit
	// rather than leaving it to a lint warning.
	let lat = $state(untrack(() => receiver?.lat?.toFixed(3) ?? ''));
	let lon = $state(untrack(() => receiver?.lon?.toFixed(3) ?? ''));
	let altitude = $state(untrack(() => String(Math.round(receiver?.altitude_m ?? 0))));

	let error = $state<string | null>(null);
	let busy = $state(false);
	/** Set while the browser is resolving geolocation, which can take seconds. */
	let locating = $state(false);

	const writable = $derived(receiver?.writable ?? false);
	const canRevert = $derived(
		writable && receiver?.configured != null && receiver.origin !== receiver.configured.origin
	);

	/**
	 * Parse a field, accepting what people actually paste.
	 *
	 * A coordinate copied out of a maps application arrives as `45.421, -75.697`
	 * or `45.421° N`. Rejecting those as "not a number" would be technically
	 * correct and would make the first attempt fail for most people, so the
	 * comma-joined pair is split by the caller and the degree sign is dropped
	 * here. Hemisphere letters are honoured because `75.697 W` is a real way to
	 * write a negative longitude.
	 */
	function parse(raw: string): number | null {
		const text = raw.trim().replace(/°/g, '').toUpperCase();
		const hemisphere = text.match(/[NSEW]$/)?.[0];
		const body = hemisphere ? text.slice(0, -1).trim() : text;
		const value = Number(body);
		if (!Number.isFinite(value)) return null;
		return hemisphere === 'S' || hemisphere === 'W' ? -Math.abs(value) : value;
	}

	/** A pasted `45.421, -75.697` fills both fields rather than failing. */
	function onLatInput() {
		const parts = lat.split(',');
		if (parts.length === 2 && parts[1].trim() !== '') {
			lat = parts[0].trim();
			lon = parts[1].trim();
		}
	}

	function useMyLocation() {
		if (!navigator.geolocation) {
			error = 'This browser does not offer a location.';
			return;
		}
		locating = true;
		error = null;
		navigator.geolocation.getCurrentPosition(
			(position) => {
				lat = position.coords.latitude.toFixed(3);
				lon = position.coords.longitude.toFixed(3);
				if (position.coords.altitude !== null && Number.isFinite(position.coords.altitude)) {
					altitude = String(Math.round(position.coords.altitude));
				}
				locating = false;
			},
			(e) => {
				locating = false;
				// The browser's own messages are terse and the common cause is
				// not an error at all: geolocation is refused outright on a
				// plain-HTTP origin, which is exactly how a Pi on a LAN is
				// reached. Say that rather than "permission denied".
				error =
					window.isSecureContext === false
						? 'Browsers only give a location over HTTPS or from localhost. ' +
							'Type the coordinates instead.'
						: `Could not get a location: ${e.message}`;
			},
			{ enableHighAccuracy: false, timeout: 15_000, maximumAge: 300_000 }
		);
	}

	async function save() {
		const parsedLat = parse(lat);
		const parsedLon = parse(lon);
		const parsedAlt = altitude.trim() === '' ? 0 : parse(altitude);

		if (parsedLat === null || parsedLon === null) {
			error = 'Latitude and longitude must both be numbers.';
			return;
		}
		if (parsedAlt === null) {
			error = 'Altitude must be a number of metres, or blank.';
			return;
		}

		busy = true;
		error = null;
		try {
			// Rounded here, not merely recommended in the hint: ~100 m is more
			// than the range gate or local CPR can use, and the extra digits
			// are someone's address.
			await onsave({
				lat: Number(parsedLat.toFixed(3)),
				lon: Number(parsedLon.toFixed(3)),
				altitude_m: Math.round(parsedAlt)
			});
			onclose();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function revert() {
		busy = true;
		error = null;
		try {
			await onrevert();
			onclose();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	function onkeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') onclose();
	}
</script>

<svelte:window on:keydown={onkeydown} />

<div
	class="scrim"
	role="button"
	tabindex="-1"
	aria-label="Close"
	onclick={onclose}
	onkeydown={(e) => e.key === 'Enter' && onclose()}
></div>

<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="station-title">
	<h2 id="station-title">Receiver position</h2>

	<p class="origin tnum">
		{formatStation(receiver?.lat ?? null, receiver?.lon ?? null)} · {receiver?.origin ?? '—'}
	</p>

	{#if !writable}
		<p class="locked">
			This receiver was started with <code>station_writable = false</code>. The position can
			only be changed in its configuration.
		</p>
	{/if}

	<div class="fields">
		<label>
			<span>LATITUDE</span>
			<input
				class="tnum"
				bind:value={lat}
				oninput={onLatInput}
				disabled={!writable || busy}
				inputmode="decimal"
				placeholder="45.421"
			/>
		</label>
		<label>
			<span>LONGITUDE</span>
			<input
				class="tnum"
				bind:value={lon}
				disabled={!writable || busy}
				inputmode="decimal"
				placeholder="−75.697"
			/>
		</label>
		<label>
			<span>ALTITUDE · M</span>
			<input
				class="tnum"
				bind:value={altitude}
				disabled={!writable || busy}
				inputmode="numeric"
				placeholder="70"
			/>
		</label>
	</div>

	<p class="hint">
		Three decimals — about 100 m — is all the range gate and local CPR can use, so the fields
		round to that. Metres above sea level, not feet. Takes effect within a second; no restart,
		and nothing currently tracked is lost.
	</p>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}

	<div class="actions">
		<button class="ghost" onclick={useMyLocation} disabled={!writable || busy || locating}>
			{locating ? 'LOCATING…' : 'USE MY LOCATION'}
		</button>
		{#if canRevert}
			<button class="ghost" onclick={revert} disabled={busy}>
				REVERT TO {receiver?.configured?.origin.toUpperCase()}
			</button>
		{/if}
		<span class="spacer"></span>
		<button class="ghost" onclick={onclose} disabled={busy}>CANCEL</button>
		<button class="primary" onclick={save} disabled={!writable || busy}>
			{busy ? 'SAVING…' : 'SAVE'}
		</button>
	</div>
</div>

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 40;
		background: color-mix(in srgb, var(--color-bg) 72%, transparent);
		border: 0;
		padding: 0;
	}

	.dialog {
		position: fixed;
		z-index: 41;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		width: min(520px, calc(100vw - 48px));
		padding: var(--panel-pad);
		background: var(--color-surface);
		border: var(--border-w) solid var(--color-line);
		border-radius: var(--radius-md);
	}

	h2 {
		margin: 0 0 var(--space-2);
		font-size: var(--size-value);
		font-weight: 500;
		color: var(--color-ink);
	}

	.origin {
		margin: 0 0 var(--space-4);
		font-size: var(--size-meta);
		color: var(--color-muted);
	}

	.locked {
		margin: 0 0 var(--space-4);
		padding: var(--space-2) var(--space-3);
		font-size: var(--size-meta);
		color: var(--color-alert);
		border-left: var(--select-bar-w) solid var(--color-alert);
		background: var(--color-accent-soft);
	}

	.fields {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: var(--space-3);
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	label span {
		font-size: var(--size-label);
		font-weight: var(--weight-label);
		letter-spacing: var(--tracking-label);
		color: var(--color-muted);
	}

	input {
		width: 100%;
		padding: var(--space-2);
		font: inherit;
		font-size: var(--size-tile);
		color: var(--color-ink);
		background: var(--color-bg);
		border: var(--border-w) solid var(--color-line);
		border-radius: var(--radius-sm);
	}

	input:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 1px;
	}

	input:disabled {
		color: var(--color-muted);
	}

	.hint {
		margin: var(--space-3) 0 0;
		font-size: var(--size-meta);
		line-height: 1.5;
		color: var(--color-muted);
	}

	.error {
		margin: var(--space-3) 0 0;
		padding: var(--space-2) var(--space-3);
		font-size: var(--size-meta);
		color: var(--color-alert);
		border-left: var(--select-bar-w) solid var(--color-alert);
	}

	.actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-4);
	}

	.spacer {
		flex: 1;
	}

	button {
		padding: var(--space-2) var(--space-3);
		font-family: inherit;
		font-size: var(--size-label);
		font-weight: var(--weight-label);
		letter-spacing: var(--tracking-label);
		border-radius: var(--radius-sm);
		cursor: pointer;
	}

	button:disabled {
		opacity: 0.45;
		cursor: default;
	}

	.ghost {
		color: var(--color-muted);
		background: transparent;
		border: var(--border-w) solid var(--color-line);
	}

	.ghost:not(:disabled):hover {
		color: var(--color-ink);
		border-color: var(--color-muted);
	}

	.primary {
		color: var(--color-bg);
		background: var(--color-accent);
		border: var(--border-w) solid var(--color-accent);
	}

	button:focus-visible {
		outline: 2px solid var(--color-accent);
		outline-offset: 2px;
	}
</style>
