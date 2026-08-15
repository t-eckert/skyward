import { chromium } from 'playwright';

// Captures both directions against the same live traffic, with an aircraft
// selected so the detail panel is exercised. Comparing themes against
// different skies would be comparing the traffic, not the design.
const dir = process.argv[2];
const IDS = ['flightdeck', 'chart'];

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));

for (const id of IDS) {
	await page.goto(`http://localhost:5173/?theme=${id}`, { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(6000);
	await page.locator('.rows button').first().click().catch(() => {});
	await page.waitForTimeout(1200);
	await page.screenshot({ path: `${dir}/t-${id}.png` });
	console.log('captured', id);
}

console.log('errors:', errors.length ? errors.join('\n  ') : 'none');
await browser.close();
