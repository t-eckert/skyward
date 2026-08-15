import { THEMES, initialTheme, applyTheme } from './index';

/**
 * The active theme, shared reactively.
 *
 * The header owns the control, but the map also needs to know: a basemap is a
 * style, not a stylesheet, so it cannot pick up a CSS custom property and has
 * to be told to swap. Reading `data-theme` off the DOM and watching it with a
 * MutationObserver would work but puts the source of truth in the document
 * instead of the app.
 */
class ThemeState {
	current = $state('flightdeck');

	init() {
		this.current = initialTheme(window.location.search);
		applyTheme(this.current);
	}

	set(id: string) {
		this.current = id;
		applyTheme(id);
	}

	/** The basemap style URL for the active theme. */
	get styleUrl(): string {
		return THEMES.find((t) => t.id === this.current)?.style ?? THEMES[0].style;
	}
}

export const theme = new ThemeState();
