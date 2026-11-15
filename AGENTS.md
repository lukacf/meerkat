# AGENTS.md

What Meerkat is, followed by the command surface for working in this
repository. `CLAUDE.md` has the full contributor guidance.

## What Meerkat Is

Meerkat is an open-source (MIT or Apache-2.0) Rust platform for running agents
as a lasting operation: many agents with durable identities, roles and memory
that take work from schedules, events, people and each other, coordinate as
teams across machines, and keep going when a process dies. It was developed at
King (Microsoft) and runs wherever you put it: as a library in your process
or as your own server.

### Built around an operation

Agent systems are built around different things:

- **A task.** Coding agents such as Claude Code, Codex and Cursor. A person
  sets a goal, the agent works on it for hours or days, often with subagents,
  a person judges the result, and the session ends.
- **A person.** Personal assistants: one long-lived agent that knows one
  person's mail, calendar and messages and acts for them.
- **An operation.** Meerkat. Agents are standing members of a running system.
  Each has an identity, a role, memory and permissions before any work arrives
  and after it is finished. Tasks pass through; the operation continues for
  months.

In a task-centric system an agent is configuration instantiated for a run, and
schedules, workflows and approvals sit beside the chat loop with their own
ideas of "running" and "done". Meerkat makes the agent the durable thing, and
recovery, scheduling, identity and authority are part of how every piece of
work proceeds.

### What sets it apart

- **Lifecycles are generated from model-checked state machines.** One
  definition produces both the TLA+ specification and the Rust kernel the
  runtime executes, and CI fails if they drift. 21 machines and 9 composition
  specifications govern session execution, teams, schedules, jobs, delivery,
  authorization and shared work. `specs/`,
  `docs/reference/machine-authority.mdx`
- **A killed run continues.** Durable sessions commit at every tool round.
  After `kill -9` in the middle of a tool batch, the same run resumes under
  the same run id from its last committed round, with a defined contract for
  what a tool sees when its call runs again. `docs/guides/durable-runs.mdx`
- **Agents are standing members of a team.** A member keeps its identity
  through restarts, redeploys and moves between hosts. Members message each
  other directly with Ed25519-signed requests, over a wiring graph the
  application controls, within a host or across machines.
  `docs/concepts/mobs.mdx`, `docs/concepts/comms.mdx`
- **Work arrives without a prompt, through one door.** A person's prompt, a
  peer's message, an external event and a scheduled wake-up enter through the
  same admission and run as the same kind of turn. Shared work lives in a
  durable ledger of goals, claims, evidence and verdicts, so nobody infers
  state from chat history. `docs/concepts/scheduling.mdx`,
  `docs/concepts/workgraph.mdx`
- **Authority is decided per operation.** The runtime tells apart who
  requested the work, which agent runs it, on whose behalf, and whose
  credentials it uses. A calendar assistant allowed to read availability cannot
  delete events even when its OAuth token could, and an operation can be held
  until a person decides. `docs/concepts/authorization.mdx`

Each agent can run on a different provider or a self-hosted model. The same
runtime is a Rust library, a CLI, three servers, Python and TypeScript SDKs
and a browser runtime, and a minimal build is a native executable of about
20 MB that starts in milliseconds.

### In use

At King (Microsoft), Meerkat runs large-scale internal systems. One deployment
in daily use helps run a large engineering organisation: long-lived agents for
each repository and initiative track goals and evidence, wake for scheduled
reviews and send out workers by the hundred, with operational peaks of roughly
1,000 agents. The same runtime also runs a household from one small computer.

### Beyond the runtime

Core is the runtime (`crates/`, `sdks/`). Three more parts build on it, each
usable alone: **Meta** for applications with lasting agent identities, memory,
access policy, approvals and a console (`meta/`); **Connect**, which lets
agents find and install the integrations they need (`connect/`); and
**Recall**, shared knowledge that keeps every fact with its source evidence
and decides who may see it (`docs/recall/`).

### Try it

```bash
brew install lukacf/meerkat/rkat
rkat --realm demo run --yolo -v \
  "Run these six commands one at a time, waiting for each: \
   sh -c 'sleep 4; echo step 1' ... sh -c 'sleep 4; echo step 6'. \
   Then list the six outputs."
# From another shell, while it is on step 3 or 4:
kill -9 "$(pgrep -n rkat)"
rkat --realm demo run --resume last "continue"
```

Meerkat is pre-1.0 and moves quickly; pin the crate family exactly and see
`docs/guides/upgrading.mdx` when you bump. `docs/why-meerkat.mdx` and
[docs.rkat.ai](https://docs.rkat.ai) go further.

## Working in This Repository

This repository uses Make as the developer-facing command surface.

Cargo is the default backend:

```bash
make build
make check
make lint
make test
```

To test a change, `node scripts/affected-tests.mjs` prints the commands the CI
planner selects for it; `--run` runs them and stops at the first failure.

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
