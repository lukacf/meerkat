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

The Rust-owned `make e2e-live` / `make e2e-smoke` lanes bootstrap their browser
prerequisites. The direct npm commands below are for surface-level debugging.

Before the direct run, provide a compatible raw WASM export bundle containing
`meerkat_web_runtime.js` and `meerkat_web_runtime_bg.wasm`. To build it, have
the Rust toolchain with the `wasm32-unknown-unknown` target and `wasm-pack`
available (or set `RKAT_WASM_PACK_BIN` / `WASM_PACK` to a wasm-pack executable):

```bash
# From the repository root
npm --prefix sdks/web run build:wasm
```

Alternatively, export an absolute `MEERKAT_WEB_WASM_OUT_DIR` pointing to a
compatible prebuilt bundle, and keep that export in the harness's environment.
`npm run smoke` serves existing runtime files; it does not build the bundle.

```bash
# From the repository root
cd tests/live_smoke/browser
npm install
npx playwright install chromium
npm run smoke
```

Run a single scenario:

```bash
npm run smoke -- --scenario BROWSER-MOBPACK-SESSION-003
npm run smoke -- --scenario BROWSER-RAW-MOB-004
```

Open a visible browser for manual inspection:

```bash
npm run smoke:headed
```

## Notes

- The harness serves the raw WASM export bundle from
  `MEERKAT_WEB_WASM_OUT_DIR` when set, otherwise `sdks/web/wasm/`.
- Provider traffic goes through the checked-in `@rkat/web` reverse proxy, so the
  browser client stays browser-safe while still exercising the real live provider.
- `ANTHROPIC_API_KEY` is required for the current browser matrix.
- The browser-safe mobpack fixture is built at runtime from files under
  `fixtures/browser_safe_mobpack/`. The committed `signature.toml` signs the
  pack (signer id `browser-smoke`, repo dev key hex `'09' * 32`) so the
  runtime's default strict trust policy admits it once the runner registers
  the signer via `mobpack_trust.trusted_signers`. After changing any fixture
  file, re-sign and refresh the committed signature:

  ```bash
  rm tests/live_smoke/browser/fixtures/browser_safe_mobpack/signature.toml
  printf '09%.0s' {1..32} > /tmp/browser-smoke-signing.key
  ./scripts/repo-cargo run -p rkat --features mob -- mob pack \
    tests/live_smoke/browser/fixtures/browser_safe_mobpack \
    --output /tmp/browser-safe-signed.mobpack \
    --sign /tmp/browser-smoke-signing.key --signer-id browser-smoke
  tar -xzf /tmp/browser-safe-signed.mobpack -C /tmp signature.toml
  mv /tmp/signature.toml tests/live_smoke/browser/fixtures/browser_safe_mobpack/
  ```
