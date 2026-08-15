import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// The API the client talks to. Everything is proxied in development so the
// browser only ever sees one origin -- which is also how this ships, with the
// built assets served next to the API. CORS therefore never enters into it.
const API = process.env.SKYWARD_API ?? 'http://127.0.0.1:8080';

const proxy = {
	target: API,
	changeOrigin: true,
	// Server-sent events must not be buffered by the dev proxy, or the live
	// view updates in bursts every few seconds instead of every second.
	configure: () => {}
};

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// A static bundle, not a Node server: the Pi already runs one
			// process and does not need a second one just to serve six files.
			adapter: adapter({ fallback: 'index.html', strict: false })
		})
	],
	// MapLibre's tile-parsing worker is an ES module that imports a sibling
	// chunk, so it has to be emitted as one. The default `iife` worker format
	// cannot express those imports.
	worker: { format: 'es' },
	// And it must not be pre-bundled, or the URL the component resolves for it
	// points into `.vite/deps` and 404s.
	optimizeDeps: {
		exclude: ['maplibre-gl']
	},
	server: {
		port: 5173,
		proxy: {
			'/api': proxy,
			'/healthz': proxy,
			'/readyz': proxy
		}
	},
	// The same proxy for `vite preview`, so the built bundle can be exercised
	// against the real receiver before it goes anywhere near the Rust binary.
	preview: {
		port: 4173,
		proxy: {
			'/api': proxy,
			'/healthz': proxy,
			'/readyz': proxy
		}
	}
});
