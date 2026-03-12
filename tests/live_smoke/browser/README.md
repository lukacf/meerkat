# Browser/WASM Live Smoke

Manual browser live-smoke coverage for the WASM surface lives here. These assets
are intentionally isolated from demo apps and CI runners.

## Scenarios

- `BROWSER-RAW-SESSION-001` — raw `meerkat-web-runtime` exports: runtime init,
  direct session lifecycle, staged system context, turn execution, event drain,
  teardown against the live Anthropic API through the local proxy.
- `BROWSER-RAW-RECALL-002` — raw session follow-up recall and system-context
  persistence against the live Anthropic API through the local proxy.
- `BROWSER-MOBPACK-SESSION-003` — browser-safe mobpack fixture: inspect,
  runtime bootstrap from mobpack, direct session creation from mobpack, turn
  execution, session state wiring.
- `BROWSER-RAW-MOB-004` — raw mob exports: create mob, spawn turn-driven member,
  member subscription, message dispatch, respawn, retire, member list cleanup.

## Run

```bash
cd tests/live_smoke/browser
npm install
npx playwright install chromium
npm run smoke
```

Run a single scenario:

```bash
npm run smoke -- --scenario BROWSER-RAW-MOB-003
npm run smoke -- --scenario BROWSER-RAW-MOB-004
```

Open a visible browser for manual inspection:

```bash
npm run smoke:headed
```

## Notes

- The harness serves the checked-in raw WASM export bundle under `sdks/web/wasm/`.
- Provider traffic goes through the checked-in `@rkat/web` reverse proxy, so the
  browser client stays browser-safe while still exercising the real live provider.
- `ANTHROPIC_API_KEY` is required for the current browser matrix.
- The browser-safe mobpack fixture is built at runtime from files under
  `fixtures/browser_safe_mobpack/`.
