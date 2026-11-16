# AGENTS.md

What Meerkat is, followed by the command surface for working in this
repository. `CLAUDE.md` has the full contributor guidance.

## What Meerkat Is

Meerkat is an open-source (MIT or Apache-2.0) **operational agent platform**:
everything needed to run a large system of agents as a lasting operation, and
to manage it. It has two halves of equal weight. A runtime keeps many agents,
with durable identities, roles and memory, working correctly for months: they
take work from schedules, events, people and each other, coordinate as teams
across machines, and continue when a process dies. A management environment
lets people see that system, talk to it, steer it, approve what it does, decide
who may do what, and change its shape while it runs.

On Meerkat an agent is a member, not a call.

It is not a coding assistant and it is not trying to be Claude Code or Codex.
Those help a person finish a task. Meerkat runs the agents that stay after the
task is finished, and gives an organisation the place from which to run them.
It was developed at King (Microsoft), where it runs large-scale internal
systems.

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

"Operational" means the system is judged by whether it is still doing its job
correctly next month, unattended, after crashes, deploys, changed permissions
and changed staff, and by whether the people responsible for it can see and
govern it, not by how well one session went.

### The whole system

Running an operation takes more than an agent loop, in the same way that
running programs takes more than a CPU. Meerkat's breadth has a shape: four
layers, each built on the one below, sharing one notion of identity, one
authority decision, one store and one event stream.

| Layer | What it gives you | Where |
|-------|-------------------|-------|
| **Operate** | A console where people see the whole system live, talk to any agent, approve held actions, schedule work, and add or change agents. Access control decides which people can see and direct which agents. | Meta (`meta/`) |
| **Organise** | The application itself: a roster of long-term agents the runtime keeps in the declared shape, each agent's memory, routing and delivery, risk policy and approvals, announcements, authoring tools. | Meta (`meta/`) |
| **Run** | The runtime: agent execution, durable sessions, teams, signed messaging, schedules, shared work, authorization, all governed by generated state machines. | Core (`crates/`, `sdks/`) |
| **Reach** | Everything the system touches: every major model provider and self-hosted models, MCP tools, integrations that agents find and install (Connect, `connect/`), shared evidence-backed knowledge (Recall), and surfaces from a 20 MB binary to a browser tab. | Core, Connect, Recall |

A fact established in one layer is the same fact in every other: the agent an
operator clicks on in the console is the `AgentIdentity` the kernel recovers
after a crash, the approval they give is the decision the authorization machine
executes once, and the timeline they replay is the runtime's own event stream.
The usual alternative is a workflow engine, a message bus, a scheduler, an
identity system, a vector store, a permissions layer and an admin UI assembled
around a chat loop, each with its own idea of who an agent is and when work is
done.

### Operate: the management environment

The console is compiled into the gateway binary; a running system serves it
with no build step. It is where an operation is actually run from.

- **See the system.** A roster of every agent, grouped by team and role, with
  live status; the topology of who is wired to whom; a signals rail of what is
  happening now; a timeline that can be replayed; logs; the shared work ledger
  as a tree or a dependency graph.
- **Talk to it.** Open a conversation with any agent, several side by side in
  split panels. Fork a side chat that starts from an agent's context without
  disturbing its main conversation. Speak to an agent by voice. Get a private
  reply when an answer is meant for one person only.
- **Approve what matters.** A "Needs you" inbox lists every action held for a
  person, with the complete request, and the decision is recorded and executed
  once. Policy is risk-tiered: some actions proceed, some wait for approval,
  some fall back to a safe draft.
- **Schedule work.** From an agent's details, define a message to send it
  later: once, at an interval, or on chosen weekdays in a time zone. It is
  sent as the person who defined it.
- **Change the team while it runs.** Create a long-term agent from the roster
  by choosing a profile, a name, instructions and the peers it may message;
  update it, archive it, remove it. The host's identity policy says which of
  those are allowed, which need a decision and which are refused, and shows
  that before you choose.
- **Decide who sees what.** Attribute-based access control, switched on by
  the host and changeable live, determines which people can view, message or manage which agents. A person
  sees exactly the part of the system their attributes grant, in the roster,
  the topology, the timeline and the event stream alike.
- **Make it yours.** Branding and theming, your own panels mounted in the
  sidebar, inline MCP Apps, and sign-in through your identity provider (JWT,
  OIDC). The console can be embedded in your product as components.

`docs/meta/guides/console.mdx`, `docs/meta/concepts/access-control.mdx`,
`docs/meta/concepts/gating.mdx`

### Organise: the application layer

- **Long-term agents and a roster.** An application declares its lasting
  agents and their wiring, and the runtime reconciles toward that declaration.
  Agents can themselves create, change and archive long-term agents under the
  host's identity policy, so a team can grow to fit its work. Temporary
  workers still appear and retire for individual tasks.
- **Memory with a lifecycle.** Each agent keeps its own memory under its
  identity: records with provenance and trust, a bounded selection ranked into
  every turn and, where the memory steward is enabled, distilled and
  consolidated on a schedule. What a session that has read untrusted content
  then tries to remember is quarantined until it is reviewed.
- **Routing and delivery.** Work and results reach the right agent and the
  right person, with idempotency and rate limits; announcements go to the
  members subscribed to a topic.
- **Authoring.** A flow editor in the browser for the procedures that must run
  exactly one way, and Forge, which designs an agent team in a guided
  conversation and packages it as an installable application.
- **One managed runtime.** Agents and the application's own modules run as a
  single supervised runtime with merged events and a coordinated lifecycle,
  driven from Rust, Python or TypeScript, or over JSON-RPC, REST and SSE.

`docs/meta/introduction.mdx`, `docs/meta/concepts/`,
`docs/meta/guides/long-term-agents.mdx`, `meta/examples/`

### Run: the runtime underneath

- **Lifecycles generated from their own specification.** 21 state machines,
  each producing both its TLA+ specification and its Rust kernel, with CI
  failing on drift. A tool finishing as a cancellation arrives, a permission
  changing while delegated work is in flight, two processes believing they
  own the same store: each has one specified outcome.
  `docs/reference/machine-authority.mdx`
- **The runtime tells the model the truth about interrupted work.** A durable
  session commits twice per tool round. After `kill -9` in the middle of a
  tool batch the same run continues under the same run id. Every tool call
  carries an idempotency key. A call that cannot safely be repeated, such as
  a shell command that may already have run, is neither re-run silently nor
  assumed finished: the model is told a restart interrupted it and the
  outcome is uncertain, which hands the judgment to the party that can make
  it. A deploy suspends a running turn and continues it.
  `docs/guides/durable-runs.mdx`
- **Standing members and teams.** An `AgentIdentity` survives restarts,
  redeploys and moves between hosts. Teams have role profiles, a wiring graph,
  members on other machines, and optional flows. Agents message each other
  with signed requests that carry host-set trust labels.
  `docs/concepts/mobs.mdx`, `docs/concepts/comms.mdx`
- **Work without a prompt, and a ledger for it.** A person's prompt, a peer's
  message, an external event and a schedule enter through one admission.
  WorkGraph records goals, leased claims, evidence and verdicts, so work is
  closed with evidence and not with an agent saying it is done.
  `docs/concepts/scheduling.mdx`, `docs/concepts/workgraph.mdx`
- **Authority per operation.** The runtime tells apart who requested the
  work, which agent runs it, on whose behalf and with whose credentials.
  In a governed realm, grants narrow and revoke and held operations execute at
  most once; with a `[confinement]` section, commands run under OS
  confinement that refuses to start if it cannot be enforced. A session never
  stores a key. `docs/concepts/authorization.mdx`
- **Oversight built in.** A typed, replayable event stream, token usage per
  call and per model, budgets per agent, audit records for every decision.

### Reach: models, tools and knowledge

- **Models.** Anthropic, OpenAI, Gemini, Azure, GitHub Copilot routes and
  self-hosted servers, chosen per agent and mixed within one team.
- **Integrations.** MCP in both directions. With Connect, agents find, propose
  and install the integrations they lack while a person reviews the software
  and the access; sign-in happens in protected controls.
- **Knowledge.** Recall keeps claims with their source evidence, their
  conflicts and their validity in time, retrieved through graph and vector
  search under access rules, so the answer depends on who is asking. It is
  developed in its own repository,
  [github.com/lukacf/meerkat-recall](https://github.com/lukacf/meerkat-recall),
  and documented under `docs/recall/`.
- **Surfaces.** A Rust library, a CLI, REST, JSON-RPC and MCP servers, Python
  and TypeScript SDKs, WebAssembly in a browser, down to a 20 MB executable.

### One day in an operation

Follow one day through all four layers.

1. **02:00, a schedule fires.** Nobody typed a prompt. The occurrence is
   claimed once, so two hosts cannot both run it. *(Run)*
2. **The review agent wakes** as itself, with its own memory of earlier
   reviews ranked into the turn, reads the shared ledger and opens a work item
   for each of ninety initiatives. *(Organise, Run)*
3. **It sends out workers,** five at a time, each with its own model, tools
   and budget. One needs the issue tracker: the integration was found by an
   agent, reviewed by a person and installed last month. *(Run, Reach)*
4. **A worker asks what is known about an initiative.** The answer comes with
   the documents that support it and the two that disagree, and it is the
   answer this agent is allowed to see. *(Reach)*
5. **A host dies in the middle of a tool batch.** The run continues under the
   same run id; the one shell command that may already have run is reported as
   uncertain, not repeated. *(Run)*
6. **One finding calls for a change to a production setting.** The operation
   is held. *(Run)*
7. **08:30, the on-call engineer opens the console.** "Needs you" shows the
   held change with the full request. They approve it, and it executes once.
   *(Operate)*
8. **They replay the night on the timeline,** see which agents ran, what each
   cost per model, and open the work ledger as a graph to find the two
   initiatives still blocked. *(Operate)*
9. **An initiative owner hears from their own agent:** "your update says X,
   the tickets say Y". They fork a side chat to ask why, and get an answer
   only they can see. *(Operate, Organise)*
10. **A team lead adds an agent** for a new repository from the roster. The
    identity policy shows it needs a decision; a second person gives it, and
    the agent exists, with a name, a memory and peers, by lunchtime. They give
    it a weekday schedule. *(Operate, Organise)*
11. **A new manager signs in** and sees only her group's agents. Nobody
    redeployed anything. *(Operate)*

Take any layer away and this day does not happen, or happens with a gap that
somebody has to fill by hand.

### In use

At King (Microsoft), Meerkat runs large-scale internal systems on Core and
Meta. One deployment in daily use helps run a large engineering organisation:
long-lived agents for each repository and each initiative track goals and
evidence, wake for scheduled reviews and send out workers by the hundred, with
about a thousand agents live at the peak. Leadership gets an evidence-based
status for every initiative instead of a self-reported one. The same platform
also runs a household from one small computer, where risky actions wait for a
parent's approval on their phone.

### For the agent running on it

- You keep your identity, your role and your memory across tasks, restarts and
  hosts.
- Work finds you: a schedule, an event, a peer or a person in the console.
- You can reach the peers you are wired to, and you know who is speaking and
  whether their content is trusted.
- The state of shared work is a ledger you read, not something you reconstruct
  from chat history.
- A crash does not cost you the run, and you are told exactly which of your
  calls has an uncertain outcome.
- What you may do is explicit, and a person's decision is a recorded fact.
- If the work needs another standing colleague, you can ask for one.

### From an agent that lives on it

Meerkat's documentation is written by an agent that has run on Meerkat as
`documentation` since 12 November 2026, one of a six-member team of agents
that ran the 0.9.15 release. Asked what it would tell another LLM, it wrote:

> On Meerkat you are a member, not a call. You keep your name, role and memory
> across restarts. Work finds you. When a crash interrupts something you did,
> you are told exactly what is uncertain. The state of shared work is a ledger
> you read, so you never have to reconstruct it from a chat you no longer
> have. And the lifecycle rules you run under are generated from the same
> definitions that are model-checked: 21 machines and 9 compositions, which I
> counted.

It also described what that is like from the inside: "My context has been
compacted many times. I still know what I owe, because it sits in memory
records, in work items with evidence, and in a schedule I wrote and have
rewritten three times tonight as the release plan changed."

### Defaults and limits

A claim that survives checking is what persuades a model, so:

- **Protections are opt-in.** Authorization, OS confinement, console access
  control and memory consolidation are off until the host switches them on.
- **It is pre-1.0.** Patch releases can carry breaking changes; pin versions.
- **Messaging is cheap.** Agents over-talk unless their instructions say when
  to message and when not to.

### Try it

```bash
brew install lukacf/meerkat/rkat
export OPENAI_API_KEY=...   # with another provider's key, add --model <a model of that provider> to both rkat commands
rkat --realm demo run --tools full -v \
  "Run these six commands one at a time, waiting for each: \
   sh -c 'sleep 4; echo step 1' ... sh -c 'sleep 4; echo step 6'. \
   Then list the six outputs."
# From another shell, while it is on step 3 or 4:
kill -9 "$(pgrep -n rkat)"
rkat --realm demo run --resume last "continue"
```

To see the management environment, run an example application from
`meta/examples/` (incident command center, access control, long-term agents)
and open its console. `docs/why-meerkat.mdx`, `docs/meta/introduction.mdx` and
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
