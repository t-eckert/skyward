import { chromium } from 'playwright';
import { createServer } from 'node:http';

// The quiet-sky state, driven by a real SSE server that holds the connection
// open and publishes empty snapshots once a second -- exactly as the receiver
// does overnight. Serving a canned body instead would close the stream and
// trip the staleness watchdog, which would be testing the wrong thing.
const dir = process.argv[2];

const heardAt = Date.now() - 6 * 60_000;
let sentFirst = false;

const sse = createServer((req, res) => {
	res.writeHead(200, {
		'content-type': 'text/event-stream',
		'cache-control': 'no-cache',
		connection: 'keep-alive'
	});

	const send = () => {
		// One aircraft on the first tick so "last heard" has something true to
		// report, then silence.
		const payload = sentFirst
			? { now_ms: Date.now(), count: 0, aircraft: [] }
			: {
					now_ms: heardAt,
					count: 1,
					aircraft: [
						{
							icao: 'A3A65F',
							callsign: 'AAL386',
							on_ground: false,
							messages: 412,
							first_seen_ms: heardAt - 300_000,
							last_seen_ms: heardAt,
							seen_ms: 0
						}
					]
				};
		sentFirst = true;
		res.write(`event: snapshot\ndata: ${JSON.stringify(payload)}\n\n`);
	};

	send();
	const timer = setInterval(send, 1000);
	req.on('close', () => clearInterval(timer));
});

await new Promise((resolve) => sse.listen(8099, resolve));

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));

// Only the stream is redirected; stats and receiver still come from the real
// server, so the header and scoreboard stay honest.
await page.route('**/api/v1/stream', (route) =>
	route.continue({ url: 'http://localhost:8099/stream' })
);

await page.goto('http://localhost:5173/', { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(9000);

console.log('state:', await page.locator('.state .text').innerText());
await page.screenshot({ path: `${dir}/s-no-traffic.png` });

console.log('errors:', errors.length ? errors.join('\n  ') : 'none');
await browser.close();
sse.close();
