# Meerkat Contributor Onboarding

Meerkat is a library-first Rust agent platform. The repository also ships the
CLI, REST, JSON-RPC, MCP, Python, TypeScript, and browser/WASM surfaces that use
the same runtime contracts.

## Prerequisites

- Rust 1.97.0 (the pinned toolchain in `rust-toolchain.toml`)
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
Cargo test runner before using `make test` or `scripts/affected-tests.mjs`:

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

For a smaller first pass, `node scripts/affected-tests.mjs --run` runs the
tests the current diff affects (see [Affected Tests](#affected-tests)).

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
```

For the tests a change affects, see [Affected Tests](#affected-tests).

Use the repository wrapper for a targeted Cargo command. A test command passes
the crate's test features, which
`node scripts/core-workspace-args.mjs --tests -p <crate>` prints:

```bash
./scripts/repo-cargo test $(node scripts/core-workspace-args.mjs --tests -p meerkat-core) session
```

Pass the same flags to a run narrowed to one integration target
(`--test <target>`): it builds no lib test, so the crate's test-feature check
does not stop it, and without them it compiles the gated tests out without a
word.

For rust-analyzer, set `rust-analyzer.cargo.features` to `"all"` (in VS Code,
the `rust-analyzer.cargo.features` setting). Without features it treats the
code the test features gate, such as the `workspace-tests` test modules, as
inactive, and its Run test lens stops on the test-feature check. A
package-qualified list does not work: Cargo refuses a feature of a package the
command does not select, and rust-analyzer's runnables select one package.

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

Every ```-fenced `rkat` invocation in a `docs/` page is also test data: the
published `rkat` package parses each one against the CLI from
`crates/meerkat-cli/tests/fixtures/doc_examples.txt`, because it carries no
`docs/`. Adding or changing an `rkat` example means refreshing that fixture, as
with schemas and BUILD files:

```bash
UPDATE_DOC_EXAMPLES_FIXTURE=1 cargo test -p rkat --test doc_examples_fixture
```

## Affected Tests

Run the tests your change affects before you push, and let pull request CI run
the full pass. `scripts/affected-tests.mjs` prints the commands, one per line,
with a short summary on stderr:

```bash
node scripts/affected-tests.mjs                     # merge-base(origin/main, HEAD)..HEAD plus uncommitted changes
node scripts/affected-tests.mjs origin/main...HEAD --no-uncommitted
node scripts/affected-tests.mjs -- crates/meerkat-core/src/lib.rs   # what these paths select
node scripts/affected-tests.mjs --run               # run the commands in order, stop at the first failure
```

The script hands the changed paths to the CI planner
(`scripts/ci-cargo-lanes.mjs`) and prints CI's own command for each lane the
plan selects:

- regeneration: machine and protocol codegen, the machine posters, schemas;
- Clippy of the changed crates, and the compile check of their reverse
  dependencies;
- nextest over the changed crates and their reverse dependencies, one run per
  product, plus their feature suites, the integration suites the change
  selects and the tests that read changed non-Rust files
  (`scripts/ci-non-rust-readers.json`);
- TLC for the touched machines, or the whole lane when that cannot be narrowed;
- the Python, TypeScript and Web SDK suites when wire schemas change.

A change that only touches a crate's `tests/*.rs` targets runs just those
targets, with the features they need (cargo's `required-features`, or a
crate-level `#![cfg(feature = "...")]`), and again with the features of an
integration suite that CI runs them in. A target that needs live provider
credentials, or whose tests are all `#[ignore]`d for a lane, is named on stderr
with that lane instead of run. The summary names what CI runs beyond the list
(governance gates, rustdoc, feature-combination checks and so on), so a green
local run is not the whole pass. In the pull request, report the commands you
ran with their exit codes and test counts, and name any listed command you did
not run.

Work in your own worktree: `./scripts/repo-cargo` then gives your runs their
own target directory. Set `CARGO_BUILD_JOBS` for the host's current load.
Branches may overlap: several changes can edit the same crates, modules and
machine DSL files, and each keeps an accountable author and reviewer. When two
changes touch the same contract or machine, their owners agree who reconciles
the combined behavior. Before each push, merge `main` into your branch, resolve
the source declarations, regenerate the generated files on the merged tree
instead of hand-merging them, and run the script again there. TLC follows the
regenerated specs, so a clean textual merge is not a validated one. Merging a
branch does not remove its target directory; clean target directories up for
actual disk pressure, not on merge.

## Before You Open A Pull Request

Run the narrowest relevant tests while iterating, then finish with the broad
gate appropriate to the change:

```bash
node scripts/affected-tests.mjs --run
make docs-check          # when docs changed
make verify-version-parity
```

Call out any live-provider, platform-specific, or end-to-end lane you could not
run. Live-provider tests are opt-in and require configured credentials.
