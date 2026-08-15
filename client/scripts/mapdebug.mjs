// Diagnoses a map that renders nothing.
//
// Both times this was needed the failure looked identical from outside: an
// empty basemap, our overlay layers present and correct, and not one error in
// the console. The signal is in the network log and in what the map thinks its
// own load state is -- so this prints both.
//
//   node scripts/mapdebug.mjs out.png [http://localhost:4173]
import { chromium } from 'playwright';

const browser = await chromium.launch({
	args: ['--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader']
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

page.on('console', (m) => console.log(`[${m.type()}]`, m.text().slice(0, 200)));
page.on('pageerror', (e) => console.log('[pageerror]', e.message.slice(0, 300)));
page.on('requestfailed', (r) => console.log('[reqfail]', r.url().slice(0, 120), r.failure()?.errorText));
page.on('worker', (w) => console.log('[worker]', w.url().slice(0, 120)));

let tileOk = 0;
page.on('response', (r) => {
	if (r.url().includes('openfreemap')) {
		if (r.status() === 200) { tileOk += 1; console.log('[200]', r.url().slice(0, 110)); }
		else console.log('[resp]', r.status(), r.url().slice(0, 100));
	}
});

const base = process.argv[3] ?? 'http://localhost:5173';
await page.goto(`${base}/?theme=flightdeck`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(10000);

const info = await page.evaluate(() => {
	const canvas = document.querySelector('.maplibregl-canvas');
	const gl = document.createElement('canvas').getContext('webgl2');
	return {
		canvasPresent: !!canvas,
		canvasSize: canvas ? `${canvas.width}x${canvas.height}` : null,
		webgl2: !!gl,
		renderer: gl
			? (() => {
					const d = gl.getExtension('WEBGL_debug_renderer_info');
					return d ? gl.getParameter(d.UNMASKED_RENDERER_WEBGL) : 'unknown';
				})()
			: 'none'
	};
});

const state = await page.evaluate(() => {
	const m = window.skywardMap;
	if (!m) return { map: 'not exposed' };
	const style = m.getStyle();
	return {
		styleLoaded: m.isStyleLoaded(),
		loaded: m.loaded(),
		layers: style?.layers?.length ?? 0,
		ourLayers: (style?.layers ?? []).map((l) => l.id).filter((id) => id.startsWith('rings') || id.startsWith('aircraft') || id.startsWith('station')),
		sources: Object.keys(style?.sources ?? {}),
		sourceLoaded: m.isSourceLoaded('openmaptiles'),
		tilesLoaded: m.areTilesLoaded(),
		center: m.getCenter(),
		zoom: m.getZoom(),
		size: [m.getCanvas().width, m.getCanvas().height]
	};
});
console.log('MAP STATE', JSON.stringify(state, null, 2));
console.log('openfreemap 200s:', tileOk);
console.log(JSON.stringify(info, null, 2));
await page.screenshot({ path: process.argv[2] ?? '/tmp/mapdebug.png' });
await browser.close();
