# Meerkat Contributor Onboarding

Meerkat is a library-first Rust agent platform. The repository also ships the
CLI, REST, JSON-RPC, MCP, Python, TypeScript, and browser/WASM surfaces that use
the same runtime contracts.

## Prerequisites

- Rust 1.94.1 (the pinned toolchain in `rust-toolchain.toml`)
- `cargo-nextest` for the default Cargo test lanes
- GNU Make
- Git
- Node.js for normal contributor validation and metadata tooling, including
  version parity and scoped Rust checks
- Python only when changing the Python SDK or running repository scripts that
  require it

Install the pinned Rust toolchain and its rustfmt/Clippy components with:

```bash
make install-build-deps
```

This target bootstraps Rust, not every validation tool. Install the separate
Cargo test runner before using `make test` or `make agent-gate`:

```bash
./scripts/repo-cargo install cargo-nextest --locked
```

Node.js is needed even when you are not editing an SDK. SDK-specific npm
dependencies and local Mintlify preview tooling remain task-specific setup.

## First Checkout

Read these before making a cross-cutting change:

1. `AGENTS.md` for repository-wide command and collaboration rules.
2. `.codex/skills/meerkat-platform/SKILL.md` for public product surfaces.
3. The `meerkat-architecture` skill for crate ownership and runtime authority.
4. The nearest crate documentation and tests for the feature you are changing.

Then establish a clean baseline:

```bash
make check
make test
```

For a smaller first pass, `make agent-gate` derives the Rust lanes affected by
the current diff and runs the scoped checks.

## Architecture In Five Minutes

The important ownership boundaries are:

- `meerkat-core` owns public contracts, typed lifecycle vocabulary, the agent
  loop, sessions, events, config types, and capability interfaces.
- `meerkat` is the facade and composition layer. `AgentFactory` is the normal
  construction path for runtime-backed agents.
- `meerkat-runtime` owns the control plane, generated machine integration,
  runtime handles, delivery coordination, and recovery coordination.
- `meerkat-schedule` owns schedule and occurrence lifecycle authority,
  persistence, planning, drivers, delivery adapters, and schedule tools.
- `meerkat-session` implements ephemeral and persistent `SessionService`
  profiles.
- `meerkat-store` and other store crates own physical persistence. A runtime
  state machine authorizes transitions; it does not replace storage authority.
- `meerkat-models` owns the built-in model catalog. Provider crates own wire
  adapters and provider-specific behavior.
- `meerkat-cli`, `meerkat-rpc`, `meerkat-rest`, and `meerkat-mcp-server` are
  surfaces over the shared runtime, not independent implementations.
- `meerkat-mob`, `meerkat-comms`, WorkGraph, scheduling, jobs, blobs, and
  artifacts extend that runtime with realm-scoped orchestration capabilities.

For long-lived product behavior, prefer runtime-backed construction through
`SessionService` and `FactoryAgentBuilder`. The public `meerkat::AgentBuilder`
still routes through `AgentFactory`, but its default runtime mode is
`StandaloneEphemeral`; it does not acquire runtime-owned wake, recovery,
auth-lease, scheduling, or coordination capabilities by itself. The lower-level
`meerkat_core::AgentBuilder` is an internal/test escape hatch.

## Development Commands

Use Make as the developer-facing command surface:

```bash
make build
make check
make lint
make test
make agent-gate
```

Use the repository wrapper for a targeted Cargo command:

```bash
./scripts/repo-cargo test -p meerkat-core session
```

Avoid raw `cargo` and raw `bb` for normal repository work. The wrapper keeps
multi-worktree and multi-agent build outputs isolated. In one checkout, give
each concurrent agent a distinct `RUST_LANE_ID` when stable warm output roots
matter.

BuildBuddy is opt-in. Start with `make buildbuddy-doctor`, then use the
`make buildbuddy-*` targets or `MEERKAT_BUILDBUDDY=1` forms documented in
`AGENTS.md`.

## Generated Contracts And Machine Authority

Several public schemas, SDK types, state machines, and reference artifacts are
generated. Change the typed owner first, regenerate through the repository
target, and commit the generated result. Do not hand-edit a generated file to
make a freshness check pass.

Useful gates include:

```bash
make verify-schema-freshness
make verify-sdk-codegen-freshness
make machine-check-drift
make verify-version-parity
```

Machine specifications are development and authority ratchets. Runtime code
must still route state transitions through the generated authority and persist
the resulting physical facts through the owning store.

## Documentation Changes

Documentation lives under `docs/` and is published with Mintlify. Update the
closest concept, guide, and reference pages only when each layer adds distinct
value. Validate navigation, links, anchors, frontmatter, and Mintlify markup
with:

```bash
make docs-check
```

Check examples against the current CLI help, public schemas, SDK signatures,
and `meerkat-models` catalog rather than copying an older page.

## Before You Open A Pull Request

Run the narrowest relevant tests while iterating, then finish with the broad
gate appropriate to the change:

```bash
make agent-gate
make docs-check          # when docs changed
make verify-version-parity
```

Call out any live-provider, platform-specific, or end-to-end lane you could not
run. Live-provider tests are opt-in and require configured credentials.
