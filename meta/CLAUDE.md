# Meta

Companion product and composition layer for the [Meerkat](https://github.com/lukacf/meerkat) multi-agent runtime. Meerkat owns agent and mob runtime authority. Meta composes modules, identity continuity, operational policies, gateways, SDKs, projections, and the admin console around it.

## Key paths

| Area | Path |
|------|------|
| Rust crate | `crates/meerkat-meta/` |
| Console/admin gateway | `crates/meerkat-meta/src/bin/meta_gateway.rs` |
| SDK stdin-RPC gateway | `crates/meerkat-meta/src/bin/rpc_gateway.rs` |
| Python SDK | `sdk/python/meerkat_meta/` |
| Python tests | `sdk/python/tests/` |
| TypeScript SDK | `sdk/typescript/` |
| Docs (Mintlify) | `docs/` |

## Python SDK

Package: `meerkat-meta` (import as `meerkat_meta`).

The SDK version follows the workspace version and is checked by
`make verify-version-parity`. Treat `sdk/python/meerkat_meta/__init__.py` as
the exact export inventory rather than copying a complete list here. Major
families include builders and runtimes, identity-first providers and models,
mob and member operations, jobs, WorkGraph, routing and delivery, memory,
mobpack authoring, event streams, typed errors, and result models.

The open member-state vocabulary currently exports `active`, `retiring`,
`broken`, `completed`, and `unknown` constants. Callers must still tolerate
future values.

Module authoring helpers (`ModuleSpec`, `define_module`, etc.) live in `meerkat_meta.helpers` — not top-level.

Private internals (underscore-prefixed): `_client.py`, `_transport.py`, `_sse.py`.

## Build and test

All Rust commands go through `scripts/repo-cargo`, which isolates `CARGO_HOME` and `CARGO_TARGET_DIR` per repo/worktree. The Makefile uses it via `CARGO ?= ./scripts/repo-cargo`.

```bash
# Rust (via wrapper)
./scripts/repo-cargo check --workspace
./scripts/repo-cargo nextest run --workspace --no-fail-fast

# Or via Makefile (preferred)
make check
make test

# Python
PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/ -v

# TypeScript (typecheck, build, and tests)
npm --prefix sdk/typescript run validate

# Local Make validation aggregate
make ci
```

The Make aggregate is not the complete hosted-CI pipeline. Consult
`.github/workflows/ci.yml` for the required console, voice, TypeScript, and
other hosted gates.

The pre-push hook runs only fast checks; CI owns clippy and the tests. Run
the hook's manual stage for them locally (`pre-commit run --hook-stage manual
cargo-clippy` or `cargo-test`, with `--from-ref origin/main --to-ref HEAD`). A
pull request runs only the CI lanes its changes reach. It leaves the other
configurations of a Rust change (voice, extension-state, Python SDK, console,
flow editor) to the push to main, which runs every lane and gates the release,
unless the change is one of their own inputs or the build configuration.
CONTRIBUTING.md describes the lanes.

## Branch conventions

- `main` — stable, PRs merge here
- Feature branches: `feat/<name>`, `fix/<name>`, `docs/<name>`, `refactor/<name>`
