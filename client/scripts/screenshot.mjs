import { chromium } from 'playwright';

const url = process.argv[2] ?? 'http://localhost:5173/';
const out = process.argv[3] ?? 'shot.png';
const waitMs = Number(process.argv[4] ?? 6000);

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });

const errors = [];
page.on('console', (m) => {
	if (m.type() === 'error') errors.push('console: ' + m.text());
});
page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));

await page.goto(url, { waitUntil: 'networkidle' }).catch(() => {});
// Give SSE a couple of snapshots to land.
await page.waitForTimeout(waitMs);
await page.screenshot({ path: out });

console.log('title:', await page.title());
console.log('errors:', errors.length ? errors.join('\n  ') : 'none');
await browser.close();
