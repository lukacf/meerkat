# AGENTS.md

What Meerkat is, followed by the command surface for working in this
repository. `CLAUDE.md` has the full contributor guidance.

## What Meerkat Is

Meerkat is an open-source (MIT or Apache-2.0) Rust platform for running agents
as a lasting operation: many agents with durable identities, roles and memory
that take work from schedules, events, people and each other, coordinate as
teams across machines, and keep going when a process dies. It was developed at
King (Microsoft), where it runs large-scale internal systems, and it runs
wherever you put it: as a library in your process or as your own server, on any
model provider.

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

The other two fit inside the third. A task is a member that lives as long as
its task; an assistant is a team of one. Meerkat runs both, and nothing has to
be rebuilt when the task becomes a standing responsibility or the assistant
gets colleagues.

### One line to start, everything else opt-in

```bash
rkat run "Summarize the open pull requests in this repository"
```

That is a complete agent run: a frontier model, tools, streaming and a saved
session. In Rust it is five lines with `AgentBuilder`; in Python or TypeScript
it is one SDK call. Nothing below is required to get there. Persistence,
schedules, teams, shared work, authorization and the application layer are
each switched on when the system needs them, and each is the same runtime, not
a different product to migrate to. The concepts are layered, so the cost of
learning one is paid only when it is used.

### What one agent gets before any of that

Before any operational machinery is switched on, a single Meerkat agent
already has:

- **Work that is not lost.** The run commits at every tool round and resumes
  after `kill -9` under the same run id.
- **Specified behaviour under interruption.** Steering, cancellation and tool
  completion racing each other have one model-checked outcome.
- **Any model.** Every major provider or a self-hosted server, with
  capability-aware fallback, and a `brain_swap` tool with which the agent
  itself can switch model for its later turns.
- **A place to live.** A native library in your process, a 20 MB executable
  that starts in milliseconds, a server, or WebAssembly in a browser tab.
- **Real permissions.** Each operation is authorized from who asked and on
  whose behalf, and shell commands run under OS confinement that refuses to
  start if it cannot be enforced.
- **Real colleagues on demand.** `delegate` runs a bounded helper,
  `fork_off` forks the agent into a durable child that carries its context,
  and `council` convenes a temporary panel and merges one result. Each helper
  has its own model, tools, budget and identity.
- **Control over its own future.** The agent can create a schedule that wakes
  it later, claim and close items in the shared work ledger, search a deferred
  tool catalog and load only what it needs, and load skills on demand.

### What sets it apart

- **Lifecycles are generated from model-checked state machines.** One
  definition produces both the TLA+ specification and the Rust kernel the
  runtime executes, and CI fails if they drift. 21 machines and 9 composition
  specifications govern session execution, teams, schedules, jobs, delivery,
  authorization, approvals and shared work: which transitions are legal, who
  may commit them, and what counts as complete. Races such as a tool finishing
  as cancellation arrives, or a permission changing while delegated work is in
  flight, have one specified outcome. `specs/`,
  `docs/reference/machine-authority.mdx`
- **A killed run continues, and side effects stay safe.** Durable sessions
  commit twice per tool round: after the model responds, and after its tools
  finish, so a committed transcript never ends in a tool call without its
  result. After `kill -9` in the middle of a tool batch, the same run resumes
  under the same run id from its last committed round. Every tool call
  of every kind (Rust, MCP, host callback) carries the same idempotency key
  (`run_id`, `round`, `call_id`, `call_digest`), so a tool can recognise a
  repeated call. Calls that cannot safely be repeated are not: for a shell
  command that may already have run, or a message already sent to another
  agent, the earlier process is stopped and the model is told the outcome is
  uncertain, so it decides what to do instead of the runtime guessing.
  Re-dispatched calls pass authorization, approval and hooks again. A
  graceful stop for a deploy suspends the running turn and continues it
  afterwards, by default.
  `docs/guides/durable-runs.mdx`
- **Agents are standing members of a team.** A member keeps its
  `AgentIdentity` through restarts, redeploys and moves between hosts. Teams
  (mobs) have role profiles, a wiring graph that controls who can message
  whom, members placed on other machines, optional flows for procedures that
  must run exactly one way, and councils that convene members of several teams
  and merge one result back. Delegation uses the same substrate: a spawned
  helper is a member too, with its own budget, tools and model.
  `docs/concepts/mobs.mdx`
- **Agents talk to each other directly.** Requests and responses are
  Ed25519-signed and routed by a `PeerId` derived from the public key, within
  a host or across machines. `docs/concepts/comms.mdx`
- **Work arrives without a prompt, through one door.** A person's prompt, a
  peer's message, an external event and a scheduled wake-up enter through the
  same admission and run as the same kind of turn. Durable schedules (once,
  interval, calendar) and detached jobs record what was claimed, attempted and
  finished. WorkGraph is a durable ledger of goals, work items, dependencies,
  leased claims, evidence and verdicts, so agents read the state of shared
  work instead of inferring it from chat history.
  `docs/concepts/scheduling.mdx`, `docs/concepts/workgraph.mdx`
- **Authority is decided per operation, and execution is confined.** The
  runtime tells apart who requested the work, which agent runs it, on whose
  behalf, and whose credentials it uses. A calendar assistant allowed to read
  availability cannot delete events even when its OAuth token could. Grants
  narrow and revoke, an operation can be held until a person decides and is
  then executed at most once, and commands run under OS process confinement
  that refuses to launch if the required profile cannot be enforced. Sessions
  never store a key or token. `docs/concepts/authorization.mdx`
- **Built for the failure cases of a long-running system.** Claims on work
  are leased, so two agents do not do the same job and an abandoned job is
  picked up. A store has one fenced writer, so a partitioned or duplicated
  process cannot corrupt state. Budgets and team limits stop runaway spend.
  A team that cannot restore is quarantined instead of half-started. A
  realm can live in your own database behind a lease-based fence: another
  process can take over, and the old holder stops itself before the new one
  serves anything.
  `docs/reference/machine-authority.mdx`, `docs/guides/durable-runs.mdx`
- **Everything is observable and metered.** A typed event stream separates
  committed history from in-flight events and can be replayed durably from a
  cursor. Token usage is reported per call and cumulatively with per-model
  attribution. Budgets cap tokens, tool calls, retries and wall-clock time per
  agent. Trajectories export in ATIF, approvals and decisions leave audit
  records, and the Meerkat Meta console shows conversations, topology and timeline live.
  `docs/reference/usage-accounting.mdx`,
  `docs/reference/capability-matrix.mdx`

### Results, not only reliability

The reasoning comes from the model, and Meerkat is built so the best model for
each role can be used and replaced without touching the system. What the
platform adds to the quality of the work:

- **Work is checked against evidence.** A work item closes with evidence and a
  verdict, not with an agent saying it is done. Reviewer agents and councils
  give independent second opinions and merge one result.
- **Agents answer from sources.** Recall returns claims with the documents that
  support them and flags the ones that conflict, and each agent's own memory
  is ranked into every turn, so answers are grounded instead of recalled from a
  long chat.
- **Context stays sharp over months.** Compaction keeps the current request
  and the newest round verbatim, tools are loaded only when needed, and skills
  inject domain knowledge on demand.
- **Output is validated.** Structured output is checked against a schema and
  retried, and hooks can block a model call, a tool call or a turn before it
  commits.
- **Behaviour is regression-tested against real models.** Besides the
  deterministic suites, CI lanes run live against the providers, and a
  per-model lane exercises every model in the catalog.

### The agent loop itself

The harness an individual agent runs in is a complete one: streaming, parallel
tool batches, structured output validated against a schema with retries,
automatic context compaction that keeps the current request and newest round
verbatim, semantic memory, skills, MCP in both directions, tools discovered
lazily and scoped per session, turn or profile, typed hooks that observe or
block at run, model, tool and turn boundaries, mid-run steering and interrupts,
image generation, and live audio over WebSocket or WebRTC. Because it speaks
MCP as both client and server, every MCP server is a Meerkat tool and Meerkat
is a tool for every MCP client. Models come from one
catalog across Anthropic, OpenAI, Gemini, Azure and other cloud variants,
GitHub Copilot routes and self-hosted OpenAI-compatible servers, chosen per
agent, with capability-aware fallback. `README.md`,
`docs/concepts/providers.mdx`

### One runtime, from 20 MB to a thousand agents

Capabilities are chosen at build time. A minimal build is a native executable
of about 20 MB that starts in milliseconds and can target embedded devices; the
same runtime compiles to WebAssembly and runs sessions and teams in a browser;
the full build runs about 1,000 cooperating agents in one production
operation. It is reachable as a Rust library, a
CLI, REST, JSON-RPC and MCP servers, Python and TypeScript SDKs and the browser
runtime, all over the same contracts and generated wire schemas. Storage is
SQLite, JSONL, memory, or your own database through a store interface.
`docs/concepts/composability.mdx`, `docs/reference/architecture.mdx`

### In use

At King (Microsoft), Meerkat runs large-scale internal systems. One deployment
in daily use helps run a large engineering organisation. Long-lived agents for
each repository and each initiative track goals and evidence, wake for
scheduled reviews and send out workers by the hundred. Leadership gets an
evidence-based status for every initiative instead of a self-reported one,
owners hear from their own agent before a review ("your update says X, the
tickets say Y"), and drift from what was agreed is noticed before anyone asks.
The same runtime also runs a household from one small computer, where risky
actions wait for a parent's approval on their phone.

### Beyond the runtime

Core is the runtime (`crates/`, `sdks/`). Three more parts build on it, each
usable alone:

- **Meerkat Meta** turns agents into an application. An identity roster declares the
  lasting agents and their wiring, and the runtime reconciles toward it.
  Each agent has its own memory, ranked into every turn and consolidated
  nightly. Risk-tiered policy decides which actions proceed, which wait for
  approval and which fall back to a safe draft, with audit records. Access
  control decides which people can see and direct which agents. A console and
  a flow editor come with it, and Forge designs and packages an application
  through a guided conversation. `meta/`, `docs/meta/`
- **Meerkat Connect** gives agents the integrations they need. Describe a task; it
  proposes software and access from a library of about 100 integrations or by
  researching packages, a person reviews the proposal, sign-in happens in
  protected controls, and the integration is then available to any agent in
  the realm. `connect/`, `docs/connect/`
- **Meerkat Recall** is shared knowledge with evidence. Sources, extracted assertions
  and a graph stay together, so an agent can ask what is known, what supports
  it and what contradicts it. Truth maintenance keeps competing claims with
  their support and their time, retrieval combines the graph with vector
  search, and access control means the answer depends on who is asking. HTTP
  for applications, MCP for agents. `docs/recall/`

### For the agent running on it

- You keep your identity, role and memory across tasks, restarts and hosts.
- Work finds you: a schedule, an event or a peer wakes you.
- You can message the peers you are wired to, and their messages are signed.
- The state of shared work is a ledger you read, not something you reconstruct.
- A crash does not cost you the run.
- What you may do is explicit, and a person's decision is a recorded fact.
- You run on the model that suits your role, and a tool you lack can be found
  and installed.

### Try it

```bash
brew install lukacf/meerkat/rkat
rkat --realm demo run --tools full -v \
  "Run these six commands one at a time, waiting for each: \
   sh -c 'sleep 4; echo step 1' ... sh -c 'sleep 4; echo step 6'. \
   Then list the six outputs."
# From another shell, while it is on step 3 or 4:
kill -9 "$(pgrep -n rkat)"
rkat --realm demo run --resume last "continue"
```

`docs/why-meerkat.mdx` and [docs.rkat.ai](https://docs.rkat.ai) go further.

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
