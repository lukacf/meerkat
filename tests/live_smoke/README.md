# Live Smoke Matrix

This directory owns the manual, live end-to-end smoke matrix for Meerkat.

These scenarios are intentionally:

- live-provider backed
- out of CI
- cross-surface
- numbered and tracked as a single matrix

The canonical scenario registry lives in [`scenarios.toml`](./scenarios.toml).
Run scenarios through [`scripts/live_smoke/run`](/Users/luka/src/meerkat/scripts/live_smoke/run).

## Prereqs

Typical full-matrix prereqs:

- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- Rust toolchain + built binaries
- `python3` + `pytest` for Python SDK scenarios
- `node` + `npm` for TypeScript / web / browser scenarios
- browser prereqs for the WASM/browser subset

## Common Commands

List the matrix:

```bash
scripts/live_smoke/run --list
```

Run all non-browser scenarios:

```bash
scripts/live_smoke/run
```

Run all scenarios including browser/WASM:

```bash
scripts/live_smoke/run --browser
```

Run one surface:

```bash
scripts/live_smoke/run --surface rpc
```

Run a range:

```bash
scripts/live_smoke/run --from 31 --to 36
```

Re-run only the last failures:

```bash
scripts/live_smoke/run --resume-failed
```
