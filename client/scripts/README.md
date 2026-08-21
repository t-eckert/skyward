# Verification scripts

Not a test suite — three scripts that drive the client into states you cannot reach by refreshing, and screenshot the result. Run the receiver and `npm run dev` first.

```bash
node scripts/screenshot.mjs http://localhost:5173/ live.png
node scripts/quiet.mjs   /tmp        # the empty-sky state
node scripts/outage.mjs  /tmp        # stops the receiver, then restarts it
```

## Why `outage.mjs` stops the real server

It found a bug that mocking could not.

Aborting the request at the browser gives you a *failed connection*, which the client always noticed. Stopping the actual receiver gives you something worse: the socket stayed open behind the dev proxy, `onerror` never fired, and the client reported `STREAMING` with a sixteen-second-old snapshot — showing `heard 0.1 s` the whole time, because ages are computed against the server clock in the envelope and that had stopped advancing too.

The fix was to stop trusting the absence of an error and add a staleness watchdog (`SNAPSHOT_TIMEOUT_MS` in `src/lib/live.svelte.ts`). This script is what proves it still works: it asserts `STREAMING → DISCONNECTED → STREAMING` across a real stop and restart.

It leaves the receiver running when it finishes.
