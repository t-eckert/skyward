import { chromium } from 'playwright';
import { execSync, spawn } from 'node:child_process';

// Proves the outage path end to end by actually stopping the receiver:
// the client must freeze the last snapshot, age it, count down, and then
// recover by itself when the server comes back.
const dir = process.argv[2];
const ROOT = '/Users/thomaseckert/Repos/github.com/t-eckert/skyward';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));

await page.goto('http://localhost:5173/', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(9000);

// Select an aircraft so the frozen panel shows the LAST KNOWN treatment.
await page.locator('.rows button').first().click().catch(() => {});
await page.waitForTimeout(500);

const before = await page.locator('.state .text').innerText();
console.log('before outage:', before);

execSync('pkill -f "target/release/skyward run" || true');
await page.waitForTimeout(16000);

await page.screenshot({ path: `${dir}/s-disconnected.png` });
console.log('during outage:', await page.locator('.state .text').innerText());

// Bring it back and confirm the client reconnects with no reload.
const server = spawn('./target/release/skyward', ['run'], {
	cwd: ROOT,
	detached: true,
	stdio: 'ignore'
});
server.unref();

await page.waitForTimeout(20000);
const after = await page.locator('.state .text').innerText();
console.log('after restart:', after);
await page.screenshot({ path: `${dir}/s-recovered.png` });

console.log('errors:', errors.length ? errors.join('\n  ') : 'none');
await browser.close();
