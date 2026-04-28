# Live Smoke Matrix

This directory owns the live smoke assets and browser fixtures for Meerkat.

The authoritative numbered live/smoke matrix now lives in Rust metadata under
`tests/integration/src/e2e_lanes.rs`, and the supported top-level runners are
Cargo lane commands instead of a standalone Python dispatcher.

These scenarios remain intentionally:

- live-provider backed
- out of CI
- cross-surface
- numbered and tracked as a single matrix

## Prereqs

Typical full-matrix prereqs:

- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GEMINI_API_KEY` or `GOOGLE_API_KEY`
- Rust toolchain + built binaries
- `python3` + `pytest` for Python SDK scenarios
- `node` + `npm` for TypeScript / web / browser scenarios
- browser prereqs for the WASM/browser subset

## Common Commands

List the live lane tests:

```bash
cargo test -p meerkat-integration-tests --test e2e_live_lane -- --ignored --list
```

Run the targeted live-provider lane:

```bash
cargo e2e-live
```

Run the compound live smoke lane:

```bash
cargo e2e-smoke
```

Inside the repo, prefer the wrapped form so target-dir isolation and cache
settings match CI:

```bash
./scripts/repo-cargo e2e-live
./scripts/repo-cargo e2e-smoke
```

The Make target also supports smoke-lane selection without duplicating the
catalog:

```bash
make e2e-smoke
make e2e-smoke TEST=e2e_smoke_mob_live_smoke
make e2e-smoke SCENARIO=62
make e2e-smoke SUITE=mob-live-smoke
```

Prebuilt mode first materializes the selected Rust binaries/test executables
through `./scripts/repo-cargo`, writes an artifact manifest, and then runs the
same harness with nested cargo builds disabled:

```bash
MEERKAT_E2E_EXECUTION_MODE=prebuilt make e2e-smoke
MEERKAT_E2E_EXECUTION_MODE=prebuilt make e2e-smoke TEST=e2e_smoke_mob_live_smoke
MEERKAT_E2E_EXECUTION_MODE=prebuilt make e2e-smoke SCENARIO=62
MEERKAT_E2E_EXECUTION_MODE=prebuilt make e2e-smoke SUITE=mob-live-smoke
```

Use `MEERKAT_E2E_ARTIFACT_MANIFEST=/path/to/manifest.json` to choose or inspect
the manifest path. In prebuilt mode, `CommandSpec::CargoTest` entries execute
the manifest's test executable directly instead of invoking `cargo test`.

Run one scenario by cargo test filter:

```bash
cargo nextest run -p meerkat-integration-tests --test e2e_live_lane e2e_live_s15_rpc_full_lifecycle_and_recall --run-ignored ignored-only --test-threads=1
```

Run one smoke suite:

```bash
cargo nextest run -p meerkat-integration-tests --test e2e_smoke_lane e2e_smoke_mob_flow_runtime_suite --run-ignored ignored-only
```

Browser fixtures under `tests/live_smoke/browser` are still used by the WASM
smoke scenarios; Cargo now invokes them through the Rust-owned lane harness.

The Rust harness also bootstraps Python, TypeScript, and browser prerequisites
for the numbered matrix scenarios. Treat direct `pytest`, `node --test`, or
`npm run smoke` invocations as surface-level debugging tools rather than the
canonical top-level test entrypoints.
