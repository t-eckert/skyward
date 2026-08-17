/// <reference types="vite/client" />
import type { Preview } from '@storybook/sveltekit';
import { withThemeByDataAttribute } from '@storybook/addon-themes';

// The whole interface is driven by the tokens in app.css — type scale, row
// heights and density included, not just hue. Importing it here is what makes a
// story in isolation look exactly like it does in the app.
import '../src/app.css';

const preview: Preview = {
	parameters: {
		// app.css paints `body { background: var(--color-bg) }`, so the canvas
		// already follows the active theme. The backgrounds toolbar would only
		// fight it.
		backgrounds: { disable: true },
		layout: 'padded',
		controls: {
			matchers: {
				color: /(background|color)$/i,
				date: /Date$/i
			}
		},
		a11y: { test: 'todo' }
	},

	// The theme toggle in the toolbar writes `data-theme` on <html>, which is the
	// exact attribute ThemeSwitcher sets in the real app — so what you tune here
	// is what ships. Flight deck is the default, matching app.css.
	decorators: [
		withThemeByDataAttribute({
			themes: {
				'Flight deck': 'flightdeck',
				Chart: 'chart'
			},
			defaultTheme: 'Flight deck',
			attributeName: 'data-theme'
		})
	]
};

export default preview;
