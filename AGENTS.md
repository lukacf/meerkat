# AGENTS.md

This repository uses Make as the developer-facing command surface.

Cargo is the default backend:

```bash
make build
make check
make lint
make test
make agent-gate
```

BuildBuddy is opt-in. When the machine has BuildBuddy access, the same broad
local lanes can run through the macOS arm64 Bazel/RBE path:

```bash
MEERKAT_BUILDBUDDY=1 make build
MEERKAT_BUILDBUDDY=1 make check
MEERKAT_BUILDBUDDY=1 make lint
MEERKAT_BUILDBUDDY=1 make test
MEERKAT_BUILDBUDDY=1 make e2e-fast
MEERKAT_BUILDBUDDY=1 make e2e-system
MEERKAT_BUILDBUDDY=1 make e2e-live
MEERKAT_BUILDBUDDY=1 make e2e-smoke
```

Explicit BuildBuddy targets are available when you do not want environment
switching: `make buildbuddy-build`, `make buildbuddy-check`,
`make buildbuddy-clippy`, `make buildbuddy-test`, `make buildbuddy-test-unit`,
`make buildbuddy-test-int`, `make buildbuddy-e2e-fast`, and
`make buildbuddy-e2e-system`. Live-provider lanes are available as
`make buildbuddy-e2e-live` and `make buildbuddy-e2e-smoke` when provider keys
are configured.

Run `make buildbuddy-doctor` when BuildBuddy setup looks suspicious. It checks
the API key, pinned `bb` CLI, generated Bazel files, selector behavior, and lane
isolation without printing secrets. Use `BUILDBUDDY_DRY_RUN=1` with explicit
BuildBuddy Make targets to inspect the selected command without running it.

For same-checkout multi-agent work, set a distinct `RUST_LANE_ID` per agent if
you want stable warm local output roots. Separate Git worktrees are already
isolated by path hash.

Avoid raw `cargo` and raw `bb` for normal repo work. Use `./scripts/repo-cargo`
for targeted Cargo commands and `scripts/buildbuddy-dev` only when extending or
debugging the BuildBuddy local facade.
