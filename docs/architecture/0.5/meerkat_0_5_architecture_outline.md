# Meerkat 0.5 Architecture Outline

Status: normative `0.5` planning outline

Companion docs:

- `docs/architecture/0.5/meerkat_sm_nomenclature.md`
- `docs/architecture/0.5/meerkat_0_5_execution_plan.md`
- `docs/architecture/0.5/meerkat_machine_formalization_strategy.md`
- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`
- `docs/architecture/0.5/meerkat_host_mode_cutover_spec.md`
- `docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md`
- `docs/architecture/0.5/meerkat_surface_cutover_matrix.md`
- `specs/machines/`

This note captures the current architectural direction for Meerkat `0.5`.
It is the planning-time architectural source of truth for the core elements
we want to make first-class, the layers we want to keep distinct, and the
major confusions we want to remove.

Once a canonical machine lands in checked-in schema or explicit Rust kernel
form, that executable machine definition becomes the long-term semantic source
of truth for that machine and this document becomes explanatory/derived for
that landed owner.

This note is target-oriented. It defines the destination architecture, not a
claim that the `0.4` public API already uses these names.

This outline is one document in the full `0.5` execution package. The
companion docs above carry the executable details for host-mode cutover,
shared child-agent lifecycle convergence, surface rollout, schema/codegen
workflow, and final release sequencing.

Decision trail:

- decisions recorded here are final for the `0.5` package until they are
  replaced by landed machine authority in the repo
- implementation deviations are not allowed to drift silently; the
  authoritative machine definition, verification, and companion docs must move
  together

The guiding goal is coherence:

- the right first-class elements
- the right ownership boundaries
- the right temporal machines
- the right reducers and bootstraps
- the smallest possible amount of cross-layer ambiguity

## Firm Vs Provisional Decisions

Some decisions in this note are intended to be firm `0.5` direction, while
others are still provisional design structure.

Firm direction:

- machine vs non-machine taxonomy
- single-writer per-session runtime ownership
- explicit out-of-band control plane
- converge onto the existing runtime admission path as the canonical execution
  path
- classify/admit peer input before runtime ingress enqueue
- one canonical runtime ingress model with explicit admission ordering
- top-level machine census:
  `RuntimeControlMachine`, `PeerCommsMachine`, `OpsLifecycleMachine`,
  `MobOrchestratorMachine`, `ExternalToolSurfaceMachine`
- owned submachine census:
  `RuntimeIngressMachine`, `InputLifecycleMachine`, `TurnExecutionMachine`,
  `MobLifecycleMachine`, `FlowRunMachine`
- two user-facing agent wiring levels: `Comms` and `Mobs`
- subagent behavior is a mob preset/workflow, not a separate mechanism
- one common ops lifecycle substrate for async/background operations,
  including mob-backed child work and background tool operations
- runtime is not the same thing as session/service/facade layers

More provisional design shape:

- exact private helper names and transitional compatibility shims

## Why 0.5 Exists

`0.5` is the "get the core mechanisms coherent" release.

The main architectural problem today is not that Meerkat lacks machinery. It
is that several different kinds of concerns still share the same channels,
labels, or owners and only get separated late in execution.

Examples of this smell today:

- peer comms, orchestration injection, control, and delegated-work results
  still overlap too much
- "runtime" refers to more than one level of ownership
- parent machines and internal submachines are easy to talk about as if they
  were peers
- some reducer-shaped concepts are still described informally as if they were
  machines

`0.5` should make those boundaries explicit.

## Existing Runtime Foundation

Much of the target architecture already exists in `meerkat-runtime`.

Existing anchors include:

- `RuntimeStateMachine` and the per-session `RuntimeLoop`
- `RuntimeSessionAdapter::accept_input()` as a single runtime admission point
- `RuntimeDriver` queue ownership and FIFO dequeue
- `InputStateMachine` for per-input lifecycle
- the current legacy runtime input enum, including `SystemGenerated` and
  `Projected`, as precursor implementation surface rather than normative `0.5`
  taxonomy
- `PeerConvention::{Message, Request, ResponseProgress, ResponseTerminal}`
- `DefaultPolicyTable::resolve()` and `PolicyDecision`
- a distinct out-of-band `RunControlCommand` control channel
- `RuntimeCommsInputSink` as an existing comms-to-runtime bridge

So `0.5` should be framed primarily as convergence onto this runtime path,
plus a smaller set of genuinely new design moves:

- making the runtime path universal
- deleting the legacy direct-agent bypass path
- formalizing `TurnExecutionMachine`
- establishing a common `OpsLifecycleMachine` substrate
- decomposing `MobActor`

The important architectural asymmetry is that runtime ingress, admission, and
per-input lifecycle are mostly already built, while turn execution unification
is still the core structural refactor.

## Convergence Rules

The following rules are firm implementation constraints for `0.5`:

1. No new feature should introduce a second ordinary work-ingress/drain path.
2. Every inbound ordinary work item should be normalized and admitted at most
   once into one canonical path.
3. Trust/auth, normalization, wake urgency, and admission/queue semantics
   should be decided at admission; runtime-owned drain/boundary routing may
   still depend on current machine state.
4. Legacy APIs should adapt into the canonical path as early as possible,
   never reimplement it in parallel.
5. Duplicated authoritative mutable state should be treated as temporary debt
   unless there is an explicit ownership and performance justification.

Clarification for rule 5:

- duplicated mutable state requires a written justification
- it requires a named owner responsible for consistency
- silent or incidental duplication is not an acceptable `0.5` design outcome

## Two Execution Paths Today

The current system still has two materially different paths for incoming work.

- runtime path
  surface or bridge -> runtime admission -> policy resolution -> runtime queue
  -> runtime loop -> core executor -> agent turn execution
- legacy direct-agent path
  comms inbox -> `drain_comms_inbox()` / `host_mode_loop()` ->
  direct classification/routing/injection -> agent turn execution

When runtime admission is wired, the first path is already active. When it is
not, the second path still handles host-mode comms directly.

`0.5` should make the runtime path the only ordinary execution path.

## WASM Runtime Convergence

WASM is an important `0.5` test case because it currently contains its own
local version of the two-path problem.

Today the browser runtime has two materially different execution models:

- service-backed runtime state
  `init_runtime()` / `init_runtime_from_config()` create a shared
  `EphemeralSessionService<FactoryAgentBuilder>` plus `MobMcpState` used for
  mob lifecycle, member subscriptions, and service-backed orchestration
- direct session-handle path
  `create_session()`, `create_session_simple()`, and `start_turn()` keep a
  separate direct-session registry and rebuild an `Agent` per turn, preserving
  session value and event buffers inside `meerkat-web-runtime`

That split is not acceptable in `0.5`. The browser surface should converge
onto the same runtime-owned admission/execution path as the rest of the
system.

### Current WASM Split

| Current WASM concern | Current implementation | `0.5` target |
| --- | --- | --- |
| runtime bootstrap | thread-local `RuntimeState` with `EphemeralSessionService` + `MobMcpState` | runtime-owned browser state with `EphemeralSessionService`, `RuntimeSessionAdapter`, `MobMcpState`, and browser tool-surface state |
| direct sessions | thread-local direct-session registry + per-handle `Session` value | façade handle map from opaque handle -> real `SessionId` backed by runtime/service |
| prompt turns | `start_turn()` builds an `Agent` directly and runs immediately | admit prompt through runtime path and await completion |
| direct event buffering | per-handle `pending_events` buffer inside web runtime | runtime/service-backed event stream with polling/subscription adapters |
| browser external tools | pre-init thread-local JS tool registry | runtime-scoped browser tool surface owned by initialized runtime |
| browser external events | no explicit runtime-backed event admission API | explicit `WorkInput::Event` / `ExternalEvent` admission exports |

### Firm WASM Decisions For 0.5

#### 1. One runtime-backed browser execution path

Ordinary browser work should route through the same canonical runtime path as
other surfaces.

Firm rule:

- direct-session exports must stop building/running `Agent` directly
- browser ordinary work must flow through runtime admission, runtime policy,
  runtime queueing, and turn execution

In practice, this means the current direct-session path is a transitional
compatibility surface, not an architectural owner.

#### 2. Runtime initialization becomes the ordinary browser bootstrap

`init_runtime()` / `init_runtime_from_config()` should be the required
bootstrap for ordinary browser work.

Preferred `0.5` shape:

- `runtime_version()` and archive inspection can remain standalone utilities
- direct-session creation, direct turns, mob APIs, browser tool registration,
  and browser event admission all require initialized runtime state

The canonical initialized browser runtime state should own:

- `EphemeralSessionService<FactoryAgentBuilder>`
- `RuntimeSessionAdapter`
- `MobMcpState`
- runtime-scoped browser tool-surface state
- a façade handle map from numeric direct-session handles to real `SessionId`

#### 3. Direct sessions become adapters over real runtime-backed sessions

The public direct-session surface may remain, but it should stop being its own
execution subsystem.

Firm implementation target:

- `create_session()` / `create_session_simple()` allocate a real session
  through the initialized session service/runtime pair
- the returned numeric handle is only a façade identity for browser callers
- the authoritative runtime/session identity is `SessionId`

This means the browser runtime should delete the custom direct-session owner
that currently stores:

- a separate `Session` value
- a separate run counter
- a separate pending-event buffer
- per-handle build-config templates as an independent execution owner

Those concerns should be runtime/service-owned, with the browser handle layer
reduced to mapping and convenience.

#### 4. `start_turn()` becomes runtime admission plus completion waiting

The browser `turn()` UX should stay simple, but its implementation should use
the runtime path rather than bypass it.

Firm implementation target:

- prompt turns are admitted as `Input::Prompt`
- direct-session request/response behavior is implemented with
  `RuntimeSessionAdapter::accept_input_with_completion()` or an equivalent
  runtime-backed helper
- the browser export waits for the completion handle, then materializes the
  existing JSON result contract from the completed runtime result

This preserves the current synchronous-feeling `Session.turn()` surface while
keeping the architecture honest.

#### 5. Direct-session polling and inspection become adapters

Browser convenience APIs may remain, but they should not own authoritative
state outside the runtime/session pair.

Firm implementation target:

- `get_session_state(handle)` returns a projection/export of the runtime-backed
  session state
- `poll_events(handle)` becomes an adapter over a runtime/service-backed
  session event stream
- direct `subscribe()` is backed by the same session event infrastructure used
  for mob-member subscriptions
- `append_system_context(handle, ...)` targets the runtime-backed session
  rather than a private direct-session registry
- `destroy_session(handle)` delegates to the runtime/session control path

#### 6. Browser external events become first-class runtime inputs

The browser should not need comms-shaped hacks for host-driven event
integration.

Firm `0.5` rule:

- browser-originated external stimuli map to `WorkInput::Event`
- implementation anchor is the existing runtime `Input::ExternalEvent`
- browser event admission should use explicit runtime admission APIs, not
  bespoke direct-session routing

Recommended exported shape:

- raw wasm exports admit `ExternalEvent`-shaped payloads
- the TypeScript wrapper exposes ergonomic helpers such as
  `Session.emitEvent(...)` and `Mob.emitEvent(...)`

Minimum data contract:

- `event_type`
- `payload`
- optional source/idempotency/supersession metadata consistent with runtime
  input headers

This should be documented as the browser-safe equivalent of external event
admission, not as a WASM-only special case outside the runtime model.

#### 7. Browser tool registration becomes runtime-scoped surface state

The current pre-init JS tool registry is convenient, but it does not fit the
`0.5` ownership model.

Firm `0.5` direction:

- JS-registered browser tools are part of a browser-local external tool
  surface
- tool registration/unregistration/clearing should be runtime-scoped, not
  thread-local pre-init global state
- runtime-owned browser tool-surface updates should integrate with the same
  boundary-update model used elsewhere, even though the browser has no MCP
  client

Public-surface implication:

- `@rkat/web` tool registration should move from static pre-init helpers
  toward runtime-instance methods or clearly runtime-scoped operations

This does not require the browser to pretend it has MCP. It does require the
browser to stop treating host-provided tools as invisible mutable global state.

#### 8. WASM keeps its browser capability envelope unless explicitly expanded

`0.5` convergence should not accidentally promise native capabilities in the
browser.

The following remain firm browser constraints unless separately implemented:

- ephemeral sessions only
- in-process comms only
- no shell tools
- no filesystem mutation/persistence
- no TCP/UDS comms transports
- no MCP protocol client

Mob note:

- browser child-agent behavior, where supported, uses the same mob-backed
  member orchestration model as other surfaces
- browser-local tools remain supported, but as runtime-scoped local tool
  surface state rather than pre-init global ownership
- unsupported browser capabilities must fail early and explicitly rather than
  inventing browser-only substitute semantics

### WASM-Specific Expected Deletions

`0.5` convergence should remove the following browser-specific duplicate
mechanisms:

- the custom direct-session registry as an authoritative execution owner
- per-turn direct `build_agent()` inside `start_turn()`
- direct-session-owned buffered event state as the canonical event source
- pre-init thread-local JS tool registration as the authoritative browser tool
  owner
- the split between service-backed mob execution and direct-session execution
  inside `meerkat-web-runtime`

### WASM Public Surface Stance

The preferred public-surface stance is conservative:

- keep `MeerkatRuntime` as the main JS/TS entry point
- keep direct-session and mob convenience APIs where they are useful
- let those APIs become earlier, thinner adapters onto the canonical runtime
  path
- avoid exposing raw runtime/machine internals directly to browser callers

So `0.5` should mostly preserve the browser developer experience while
substantially changing the internal execution model underneath it.

## Native And SDK Surface Convergence

The same surface-by-surface exercise should be applied to every non-WASM
public surface.

The goal is not "this will probably be fine." The goal is to identify which
surfaces are already on the canonical runtime path, which still carry legacy
bypass behavior, and what concrete work `0.5` must do for each one.

### Surface Summary

| Surface | Current path shape | `0.5` stance | Main work |
| --- | --- | --- | --- |
| Rust SDK | mixed: canonical service/runtime path exists, low-level direct `Agent` path still visible | preserve high-level APIs, narrow low-level execution ownership | make runtime/service path the documented default and reduce direct host-loop ownership on `Agent` |
| CLI | mostly converged for `run`/`resume` | keep user-facing commands stable | remove legacy event-injection/bypass behavior and keep CLI as an early adapter onto runtime |
| REST | mostly converged | keep high-level routes stable, with one canonical external-event name | replace legacy `/sessions/{id}/event` with `/sessions/{id}/external-events` and keep low-level runtime admission explicit |
| JSON-RPC | mostly converged | keep high-level methods stable, with one canonical external-event name | replace legacy `event/push` with `session/external_event`; keep `runtime/*` low-level methods canonical |
| MCP server | not yet converged | preserve tool contract, change internals substantially | migrate `meerkat_run` / `meerkat_resume` onto runtime admission/completion path |
| Python SDK | thin RPC wrapper | mostly compatibility work | track RPC contract/version changes and avoid independent semantics |
| TypeScript SDK | thin RPC wrapper | mostly compatibility work | track RPC contract/version changes and avoid independent semantics |

### Rust SDK

The Rust surface is really two surfaces today:

- high-level embedding through `AgentFactory`, `FactoryAgentBuilder`,
  `EphemeralSessionService` / `PersistentSessionService`, and
  `RuntimeSessionAdapter`
- expert/low-level embedding through direct `Agent` construction and turn/host
  execution entry points

Firm `0.5` direction:

- the runtime/service-backed path is the canonical Rust integration path
- `Agent` remains the primary execution component inside
  `TurnExecutionMachine`, but should stop being the documented owner of host
  waiting, ingress draining, or multi-turn coordination
- direct low-level execution entry points may remain for advanced use, but
  they should be treated as narrower execution APIs, not as competing runtime
  frameworks

Concrete Rust SDK work:

- update Rust docs/examples to prefer runtime/service-backed integration
- audit public guidance that currently implies direct `Agent` ownership of host
  mode or multi-turn coordination
- provide runtime-backed helpers where a simple embedding path is needed,
  rather than forcing embedders to recreate runtime control themselves
- decide a staged deprecation/compatibility posture for direct host-loop
  methods that no longer match the architecture

Compatibility stance:

- high-level Rust embedding should remain stable in shape
- low-level/internal Rust consumers should expect the most refactor pressure in
  `0.5`

### CLI

The CLI is already relatively close to the target architecture for its main
`run` / `resume` flows.

Current shape:

- `run` / `resume` already stage session creation and route prompt execution
  through `RuntimeSessionAdapter` with runtime completion waiting
- CLI mob operations already sit as façade/operational surfaces over the
  underlying mob runtime
- CLI still has legacy/event-injector-style seams for host-driven external
  input such as `--stdin` host-mode injection

Firm `0.5` direction:

- CLI remains a façade surface, not a special runtime
- ordinary run/resume behavior continues to use the runtime path
- host-driven external input should be admitted as canonical runtime external
  events rather than injected through legacy event-injector paths

Concrete CLI work:

- replace `stdin_events` host-mode injection with runtime-backed
  `ExternalEvent` admission
- remove any remaining CLI-local assumptions that host mode owns its own event
  drain path outside runtime
- keep `rkat mob ...` as an operator façade while aligning its internals with
  the firm mob/ops decomposition
- verify that CLI-local MCP/live-tool integration continues to align with the
  canonical `ExternalToolSurfaceMachine` boundary model

Compatibility stance:

- command shapes should remain stable
- behavior should become more predictable and uniform across host-mode,
  comms-driven, and event-driven usage

### REST

REST is already close to the converged model.

Current shape:

- session create/continue routes are runtime-backed through a runtime adapter
  plus runtime executor
- REST already exposes runtime/input endpoints for low-level control
- REST still carries a legacy external-event endpoint that injects via the
  session event injector rather than canonical runtime input admission

Firm `0.5` direction:

- high-level REST session routes remain convenience façades
- low-level runtime/input routes are the canonical protocol expression of
  runtime admission/control
- external events should become ordinary admitted runtime work, not a special
  injection side channel

Concrete REST work:

- keep low-level runtime admission explicit as `POST /runtime/{id}/accept`
  with `Input::ExternalEvent`
- make `POST /sessions/{id}/external-events` the canonical high-level REST
  convenience route
- delete legacy `POST /sessions/{id}/event` after at most one early-adapter
  phase
- keep SSE/event-stream behavior as a surface concern layered over runtime
  event streams, not as a second execution path

Compatibility stance:

- high-level `POST /sessions` and `POST /sessions/{id}/messages` should remain
  stable
- the main visible change should be that event injection becomes a typed,
  runtime-owned contract with one canonical route name

### JSON-RPC

JSON-RPC is also already close to the target model.

Current shape:

- `session/create` + `turn/start` route through `SessionRuntime` and
  `RuntimeSessionAdapter`
- callback tool registration and notification streaming are already modeled as
  transport/server concerns layered over runtime
- JSON-RPC still has a legacy `event/push` method implemented through the
  event injector rather than through canonical runtime input admission

Firm `0.5` direction:

- `session/*` and `turn/*` remain the primary ergonomic RPC methods
- `runtime/*` methods remain the canonical low-level runtime/control methods
- `session/external_event` is the canonical high-level RPC convenience method
- `event/push` is deleted after at most one early-adapter phase

Concrete JSON-RPC work:

- implement `session/external_event` over `Input::ExternalEvent`
- keep `runtime/accept` as the authoritative low-level admission method
- remove the assumption that event push is merely "queued for next host-mode
  drain" outside the runtime model
- ensure callback tool registration/execution remains transport/server-owned
  and does not introduce a second execution path
- keep runtime/input inspection methods aligned with the firm runtime
  nomenclature and policy model

Compatibility stance:

- Python and TypeScript SDKs should not need a conceptual rewrite because the
  high-level RPC method family remains stable
- low-level RPC behavior becomes more consistent with REST/CLI/runtime

### MCP Server

The MCP server is one of the main non-WASM surfaces that still needs real
execution-path convergence work.

Current shape:

- `meerkat_run` and `meerkat_resume` still drive execution primarily through
  `PersistentSessionService::create_session()` / `start_turn()` semantics
- they do not currently use the runtime adapter as the ordinary execution path
- callback tool handling and per-session live MCP adapters are already strong
  surface features, but they sit on top of a less-converged execution path

Firm `0.5` direction:

- `meerkat_run` / `meerkat_resume` should become runtime-backed façades
- session creation/resume should stage or rebuild session state, then admit the
  prompt through runtime and await completion
- host-mode comms behavior should use the same runtime sink/runtime admission
  rules as the other surfaces

Concrete MCP server work:

- introduce or reuse a per-server/per-session `RuntimeSessionAdapter`
- route `meerkat_run` prompt execution through runtime admission plus
  completion waiting
- route `meerkat_resume` through the same runtime-backed path
- wire `runtime_adapter_for_sink` so host-mode comms continues to flow through
  runtime rather than direct agent-side host logic
- preserve the existing callback tool contract and pending-tool-call UX while
  changing the underlying execution path
- keep per-session live MCP router state aligned with the canonical external
  tool surface model

Compatibility stance:

- the external MCP tool contract should remain recognizable
- this is an internal architectural migration with potentially the largest
  implementation delta among non-WASM public surfaces

### Python SDK

The Python SDK is a thin JSON-RPC client surface.

Firm `0.5` direction:

- Python should not invent independent runtime semantics
- it should remain a wrapper over the canonical RPC method family and server
  notifications

Concrete Python SDK work:

- track any RPC contract changes made during runtime/event convergence
- update version compatibility checks and generated type bindings
- keep callback-tool, streaming, and session abstractions aligned with the RPC
  server rather than layering Python-only behavior on top
- update docs/examples once the canonical RPC event/input contracts settle

Compatibility stance:

- low API churn expected
- most changes should be versioning, docs, and compatibility shims rather than
  architecture work inside the SDK

### TypeScript SDK

The TypeScript SDK has the same architectural stance as the Python SDK.

Firm `0.5` direction:

- keep it as a thin RPC client over the canonical server/runtime surface
- do not let it accrete independent execution semantics

Concrete TypeScript SDK work:

- track RPC method/event contract changes
- update generated bindings, version checks, and examples
- keep callback-tool and streaming behavior aligned with the RPC server rather
  than layering separate TypeScript-only execution assumptions

Compatibility stance:

- low API churn expected
- most of the real work happens in RPC, not in the SDK itself

### Cross-Surface Firm Rule

The key surface rule for `0.5` is:

- no public surface should own a second ordinary execution path
- high-level convenience routes/methods may remain
- they must adapt into the canonical runtime path as early as possible

This is especially important for:

- browser direct sessions
- MCP server run/resume tools
- REST / JSON-RPC external event admission
- Rust low-level host-mode guidance

Those are the places where `0.5` should produce real deletion and convergence,
not just renamed architecture prose.

## Architectural Principles

The following principles should govern all `0.5` work.

### 1. One owner per temporal concern

If a concern has durable state over time, one machine should own it.

Bad smell:

- multiple subsystems independently interpreting the same temporal concern
- the same lifecycle inferred in more than one place

### 2. Top-level machines and owned submachines are different levels

A parent machine and one of its internal submachines must not be presented as
architectural peers without saying so explicitly.

### 3. The runtime drains one canonical ingress queue

Per session/runtime instance, there should be one canonical runtime-owned
drain queue.

Different producers may feed it, but the runtime should not have to drain
multiple semantically overlapping ingress systems.

Important:
this principle applies to ordinary admitted runtime work. It does not forbid a
separate out-of-band control plane for authority commands like stop/cancel if
those semantics remain cleaner outside ordinary FIFO ingress.

### 4. Not everything is comms

Conversational peer traffic is not the same thing as:

- delegated-work lifecycle
- prompt/work admission
- runtime control
- tool-surface lifecycle

### 5. Not everything is a machine

Reducers, policies, bootstraps, services, and artifacts should keep their own
names. Formality should come from precision, not from naming inflation.

### 6. Boundary semantics must be explicit

Draining, boundary apply, wake behavior, tool-surface updates, and hook
invocation points should all be named architecture concepts, not incidental
control flow.

### 7. Concurrency ownership must stay explicit

Single-writer ownership is one of the cleanest properties in the current
system and should be preserved unless there is a compelling reason to change
it.

In particular:

- one top-level runtime owner per session/runtime instance
- no shared mutable agent execution state across tasks
- background tasks, I/O, and external producers communicate with the owner via
  typed channels/signals rather than by directly mutating the runtime

## First-Class Top-Level Machines

These are the canonical top-level live temporal owners for `0.5`.

This is the firm `0.5` machine census, even though individual implementation
anchors differ in maturity and may be embodied across more than one current
concrete type during migration.

## Canonical Rust Ownership And Module Layout

`0.5` should use the following canonical crate/module ownership.

| Architecture concern | Canonical crate | Canonical module directory | Canonical primary types |
| --- | --- | --- | --- |
| `RuntimeControlMachine` | `meerkat-runtime` | `src/runtime_control/` | `RuntimeControlMachine`, `RuntimeControlState`, `RuntimeControlEffect` |
| `RuntimeIngressMachine` | `meerkat-runtime` | `src/runtime_ingress/` | `RuntimeIngressMachine`, `RuntimeIngressState`, `RuntimeIngressPolicy`, `RuntimeIngressQueue` |
| `InputLifecycleMachine` | `meerkat-runtime` | `src/input_lifecycle/` | `InputLifecycleMachine`, `InputLifecycleState` |
| `TurnExecutionMachine` | `meerkat-core` | `src/turn_execution/` | `TurnExecutionMachine`, `TurnExecutionState`, `TurnExecutionEffect` |
| `PeerCommsMachine` | `meerkat-comms` | `src/peer_comms/` | `PeerCommsMachine`, `PeerCommsState`, `PeerCommsEffect`, `PeerInputNormalizer` |
| `OpsLifecycleMachine` shared contracts | `meerkat-core` | `src/ops_lifecycle/` | `OpEvent`, `ChildAgentId`, `ChildAgentStatus`, `ChildAgentSpec` |
| `OpsLifecycleMachine` runtime owner | `meerkat-runtime` | `src/ops_lifecycle/` | `OpsLifecycleMachine`, `OpsLifecycleState`, `OpsLifecycleEffect`, `OpsRegistry` |
| `MobOrchestratorMachine` | `meerkat-mob` | `src/orchestrator/` | `MobOrchestratorMachine`, `MobOrchestratorState` |
| `MobLifecycleMachine` | `meerkat-mob` | `src/orchestrator/mob_lifecycle/` | `MobLifecycleMachine`, `MobLifecycleState` |
| `FlowRunMachine` | `meerkat-mob` | `src/orchestrator/flow_run/` | `FlowRunMachine`, `FlowRunState` |
| `ExternalToolSurfaceMachine` | `meerkat-mcp` | `src/external_tool_surface/` | `ExternalToolSurfaceMachine`, `ExternalToolSurfaceState`, `ExternalToolSurfaceEffect`, `SurfaceUpdateNotice` |

Canonical supporting concerns under `meerkat-mob/src/orchestrator/`:

- `topology.rs` -> `MobTopologyService`
- `task_board.rs` -> `MobTaskBoardService`
- `spawn_policy.rs` -> `MobSpawnPolicyReducer`
- `supervision.rs` -> `MobSupervisorService`

These names are now part of the implementation plan for `0.5`.

### RuntimeControlMachine

Owns per-session operational control:

- runtime lifecycle
- input admission
- drain/control decisions
- out-of-band control-plane consumption
- retire/reset/recover/stop semantics
- ownership and coordination of `RuntimeIngressMachine`
- ownership and coordination of `TurnExecutionMachine`

This is the canonical per-session operational machine.

Current implementation anchors:

- `RuntimeStateMachine`
- `RuntimeSessionAdapter`
- `RuntimeLoop`

It owns the following submachines:

- `RuntimeIngressMachine`
- `TurnExecutionMachine`

Admission contract:

- external producers do not mutate canonical ingress directly
- external producers submit typed candidate inputs to a runtime-owned admission
  surface
- `RuntimeControlMachine` owns that admission surface and admission decisions
- accepted items are handed to `RuntimeIngressMachine` for canonical sequencing
  and queue insertion

Important:
this is a root coordinator/owner, not a claim that one concrete Rust type
must directly implement every bit of ingress, scheduling, and turn logic. Its
submachines should own their internal local semantics.

It should not own:

- peer trust/auth semantics
- delegated-work lifecycle semantics
- mob orchestration semantics
- external tool surface lifecycle

### PeerCommsMachine

Owns conversational agent-to-agent traffic:

- trust/auth acceptance
- peer classification
- peer lifecycle
- peer response correlation
- peer-to-peer messaging and requests

It produces typed `PeerInput` candidates into runtime admission.

Admission rule:
`PeerCommsMachine` should perform trust/auth acceptance and classification
before enqueue into `RuntimeIngressMachine`. Runtime ingress should receive
typed admitted `PeerInput`, not raw transport envelopes or deferred
post-drain classification artifacts.

Integration rule:
`PeerCommsMachine` should submit typed `PeerInput` through the
`RuntimeControlMachine` admission surface rather than assigning global ingress
sequence on its own.

It should not own:

- delegated-work completion
- runtime control
- prompt/work admission
- orchestration-only injected work

### OpsLifecycleMachine

Owns delegated/background work lifecycle:

- register
- provision
- progress
- completion
- failure
- cancellation
- retire
- status observation

It produces typed `OpEvent` candidates into runtime admission.

Important:
mob-backed child work is not "just ops." It participates in both:

- a peer plane, via `PeerCommsMachine`
- an ops plane, via `OpsLifecycleMachine`

Firm direction:
`OpsLifecycleMachine` should be the common async-operation lifecycle substrate
for mob-backed child work and background tool operations. It owns
register/progress/completion/failure/cancellation/retirement regardless of
which higher-level mob or tool surface initiated that work.

Likely implementation anchors in today's code:

- completion/watch registries
- shell/background job registries
- background task/status registries that are currently scattered across
  surfaces and orchestration owners

This does not imply a mandatory new crate. The first useful embodiment may be
an explicit per-session ops registry and event model inside existing runtime
or core layers.

Integration rule:
`OpsLifecycleMachine` should submit typed `OpEvent` through the
`RuntimeControlMachine` admission surface rather than mutating canonical
ingress directly.

This is a major refactor. In particular, moving delegated-work
completion out of comms/inbox paths is an intentional architectural change,
not a small cleanup.

### MobOrchestratorMachine

Owns multi-agent orchestration:

- roster/member ownership
- orchestration lifecycle
- flow execution ownership
- spawn/provision/wire/retire orchestration
- orchestration-level policy and supervision

It owns the following submachines:

- `MobLifecycleMachine`
- `FlowRunMachine`

It is not a substitute for peer comms or runtime ingress.

### ExternalToolSurfaceMachine

Owns live external tool surface lifecycle:

- MCP server add/remove/reload lifecycle
- pending connection/removal states
- generation/version tracking
- inflight-call-aware removal and drain behavior
- active external tool surface snapshot
- boundary-visible external tool updates

It should interact with runtime through explicit boundary update mechanisms,
not by masquerading as peer comms or generic runtime ingress.

Current implementation anchor:

- `McpRouter`

This is already one of the most mature state-machine-shaped parts of the
system and should be treated as an architectural anchor, not as a blank-slate
placeholder.

## Owned Submachines

These are real machines, but they are internal to a top-level owner rather
than first-class top-level peers.

### RuntimeIngressMachine

Owned by `RuntimeControlMachine`.

Owns:

- canonical ingress queue
- canonical admission sequence / total order
- sequencing
- drain obligations
- drain scheduling metadata

It does not own:

- external producer-facing admission APIs
- peer trust/auth classification
- delegated-work lifecycle classification
- authority control-plane handling

Conceptual ingress taxonomy:

- `WorkInput`
- `PeerInput`
- `OpEvent`

Important:
drain policy controls when drain must happen, not which items are dequeued.
The architecture should avoid class-based selective dequeue from the FIFO
queue. Class-based routing after a FIFO drain is expected and is not in
conflict with this rule.

Ordering contract:
multiple producers may feed runtime ingress, but canonical order is assigned at
admission into `RuntimeIngressMachine`. The runtime should not rely on
independent producer-local ordering as its global ordering model.

Interface contract:
accepted items reach `RuntimeIngressMachine` only through the runtime-owned
admission surface. This gives one serialization point for global ordering.

Important:
out-of-band runtime control commands are owned by `RuntimeControlMachine`
rather than being forced through ordinary FIFO ingress.

### InputLifecycleMachine

Owned by `RuntimeIngressMachine`.

Owns lifecycle of individual ingress items:

- accepted
- queued
- staged
- applied
- consumed
- abandoned
- superseded/coalesced

This is an internal lifecycle machine, not a top-level system domain.

Current implementation anchor:

- `InputStateMachine`

Current reality:
this lifecycle is already substantially implemented in `meerkat-runtime`; the
`0.5` work here is mostly formalization and convergence, not invention from
scratch.

Clarification:
the current anchor is reducer/validator-shaped. The target
`InputLifecycleMachine` refers to the owned ingress-item lifecycle concern as a
whole, not a claim that today's `InputStateMachine` struct already fully
embodies that machine by itself.

### TurnExecutionMachine

Owned by `RuntimeControlMachine`.

Owns turn-time execution semantics:

- calling the LLM
- tool dispatch
- cooperative suspension
- run-boundary drain points
- transition between idle host and active turn execution

Important:
this is the inner turn-loop machine. It should not be conflated with the
outer runtime control plane.

Structural note:
`0.5` unifies two currently separate execution paths:

- the turn-state loop currently modeled by `LoopState`
- the host-mode idle/wake/drain loop that currently lives outside `LoopState`

This is a significant refactor, not just a rename of the current inner loop.

Agent boundary:
the current `Agent` should shrink toward a purer turn executor. In the
converged model, idle waiting, ingress draining, and multi-turn coordination
belong to runtime control, while `Agent::run()` becomes a focused execution
component for one admitted run/turn cycle with `LoopState` as its internal
execution machine.

This does not necessarily make `Agent` unimportant. It does mean its role
becomes narrower and cleaner: turn execution rather than host/runtime
coordination.

### Firm TurnExecution Contract

`TurnExecutionMachine` should be treated as the execution owner for one runtime-
admitted run at a time.

Canonical input contract:

- runtime-owned execution begins from `RunPrimitive`
- `RunPrimitive` remains the only runtime-to-core execution primitive family
- turn execution consumes the current session value plus one admitted
  `RunPrimitive` / run context

Canonical output/effect contract:

- `TurnExecutionEffect` is the existing `RunEvent` family
- the required baseline events are:
  - `RunStarted`
  - `BoundaryApplied`
  - `RunCompleted`
  - `RunFailed`
  - `RunCancelled`

Ownership rule:

- `TurnExecutionMachine` owns LLM/tool/wait/checkpoint execution within a run
- `RuntimeControlMachine` owns idle waiting, wake handling, ingress admission,
  run scheduling, and control-plane coordination
- `Agent` remains the primary execution component inside
  `TurnExecutionMachine`, but not the owner of host-mode idling or multi-turn
  runtime coordination

Implementation baseline:

- `RunPrimitive` in core is the canonical execution input family
- `RunEvent` in core is the canonical execution output/effect family
- `LoopState` is the current anchor for the internal execution-state machine

Practical consequence:
`0.5` should remove host/runtime coordination behavior from `Agent` and leave
it as the focused turn executor that runtime drives.

### MobLifecycleMachine

Owned by `MobOrchestratorMachine`.

Owns top-level mob status transitions:

- creating
- running
- stopping
- stopped
- destroyed

### FlowRunMachine

Owned by `MobOrchestratorMachine`.

Owns per-run and per-step orchestration semantics:

- run creation
- step dispatch
- dependencies
- retries
- completion/failure/cancel

It does not execute turns directly.
Its work crosses the machine boundary by admitting `WorkInput` into
`RuntimeControlMachine` / `TurnExecutionMachine`.

### Firm MobActor Decomposition

The current `MobActor` command set should decompose as follows.

| Current concern / commands | Canonical `0.5` owner | Owner kind |
| --- | --- | --- |
| `Stop`, `ResumeLifecycle`, `Complete`, `Destroy`, `Reset`, `Shutdown` | `MobLifecycleMachine` | machine |
| `RunFlow`, `CancelFlow`, `FlowStatus`, `FlowFinished` | `FlowRunMachine` | machine |
| `Spawn`, `SpawnProvisioned`, `Retire`, `Respawn`, `RetireAll`, `Wire`, `Unwire` | `MobTopologyService` | service |
| `ExternalTurn`, `InternalTurn` | runtime admission seam via `WorkInput` | cross-machine admission path |
| `TaskCreate`, `TaskUpdate` | `MobTaskBoardService` | service |
| `SetSpawnPolicy` | `MobSpawnPolicyReducer` | reducer/policy |
| supervision / host monitoring / escalation | `MobSupervisorService` | service |

Firm rules:

- lifecycle transitions belong to `MobLifecycleMachine`
- flow/run-step orchestration belongs to `FlowRunMachine`
- roster membership and peer wiring belong to `MobTopologyService`
- turn dispatch crosses into runtime through ordinary admission, not a mob-local
  execution loop
- task board mutation is a service concern, not a lifecycle machine
- spawn policy is policy/reducer-shaped, not a machine
- supervision is a service concern, not a lifecycle machine
- no additional mob-local machines beyond `MobOrchestratorMachine`,
  `MobLifecycleMachine`, and `FlowRunMachine` should be introduced in `0.5`
  without updating the authoritative machine definitions and companion package

This decomposition is now part of the implementation plan, not only a sketch.

## Reducers, Policies, Services, Bootstraps, Artifacts

These are first-class concepts, but they are not all machines.

### Reducers

These should stay reducer-shaped:

- `HookDecisionReducer`
- `ToolVisibilityReducer`

Both are deterministic boundary transforms, not top-level temporal owners.

### Policies

These should stay policy-shaped:

- `IngressClassificationPolicy`
- `RuntimeIngressPolicy`

Policies answer classification/routing questions. They do not own time.

For runtime ingress specifically, the important policy outputs are:

- admission decision
- `InterruptPolicy`
- `DrainPolicy`
- `RoutingDisposition`

Implementation anchor:
the current runtime already has a richer policy model in `PolicyDecision`
covering `ApplyMode`, `WakeMode`, `QueueMode`, `ConsumePoint`, transcript
eligibility, and operator visibility. The three-axis architecture model in
this document should be treated as a conceptual slice of that existing policy
system, not as a claim that `0.5` needs to invent a new, smaller policy table
from scratch.

### Services

These should stay service-shaped:

- `SessionService`
- `EphemeralSessionService`
- `PersistentSessionService`

They are surface/registry ownership and integration layers, not the canonical
runtime machine.

### Bootstraps

These should stay bootstrap-shaped:

- `RealmBootstrap`
- `PersistenceBootstrap`

They own environment setup and attachment, not long-lived temporal behavior.

### Artifacts

These should stay artifact-shaped:

- `Session`
- persisted receipts
- persisted snapshots

`Session` is the session value/artifact, not the operational runtime itself.
Today it still serves both live working-state and serializable/persistable
roles; `0.5` should make that conceptual split explicit even if one Rust type
continues to serve both roles.

## Canonical Runtime Ingress Model

`RuntimeControlMachine` should own one canonical ingress queue, with
`TurnExecutionMachine` performing drains at admissible boundaries under
runtime-owned policy.

That queue should conceptually carry:

```text
RuntimeIngress =
  WorkInput
  | PeerInput
  | OperationInput
  | ContinuationInput
```

This is not a requirement that every implementation type be named exactly this
way on day one. It is the conceptual model that `0.5` should converge toward.

### Admission Path

The authoritative admission path should be:

1. an external producer forms one or more typed candidate inputs
2. `RuntimeControlMachine` admission accepts, rejects, or defers it
3. accepted items are sequenced and enqueued by `RuntimeIngressMachine`
4. `TurnExecutionMachine` drains according to runtime-owned policy

Important:

- global ordering is assigned at runtime admission, not by producer-local send
  order
- producers may classify/normalize their own domain inputs before submission
- producers should not bypass runtime admission to enqueue directly
- producers do not get to author opaque staged runs; runtime forms drain
  snapshots from admitted inputs
- one staged run may consume multiple admitted inputs, with
  `contributing_input_ids` preserved in runtime order

Implementation anchor:
the current `RuntimeSessionAdapter::accept_input()` plus `RuntimeDriver`
already embody most of this path today.

### Control Plane

`0.5` should preserve an explicit out-of-band control path for runtime
authority commands.

In current code, this is represented by `RunControlCommand` on a separate
control channel. That is a meaningful semantic distinction, not just an
implementation accident, and the architecture should preserve it unless there
is a compelling reason to collapse it into ordinary ingress.

So the preferred model is:

- ordinary admitted work uses the canonical FIFO runtime ingress queue
- authority commands use a dedicated control plane owned by
  `RuntimeControlMachine`

Precedence contract:

- control-plane commands preempt ordinary ingress processing
- control-plane commands may interrupt cooperative suspension immediately
- control-plane commands are not assigned FIFO order relative to ordinary
  runtime ingress
- pending drain obligations do not block authority actions such as cancel,
  stop, dismiss, or shutdown

### Drain Semantics

Each ingress item should carry a drain policy:

- `DrainAsap`
- `Passive`

Meaning:

- `DrainAsap`
  Force a drain at the earliest admissible boundary.
- `Passive`
  Do not force drain, but ride along on the next drain.

Important invariants:

- one FIFO runtime ingress queue
- no class-based selective dequeue from the FIFO queue
- draining consumes a FIFO snapshot that may contain multiple admitted inputs
- drain policy controls urgency, not selection

Implementation anchor:
the current runtime policy/queue model is already richer than this simplified
description. In practice, queueing/coalescing/supersession/consume-point
behavior should converge onto the existing runtime policy table unless there is
an explicit architectural reason to simplify it.

### Routing Semantics

Drain urgency is not sufficient by itself. Admitted ingress also needs a
post-drain routing decision.

Conceptual routing dispositions include:

- `StartOrContinueTurn`
- `ContextOnly`
- `Fold`
- `Drop`

These dispositions answer what the runtime should do with a drained item after
it has been admitted and dequeued.

Important:

- `DrainPolicy` answers when drain must happen
- routing disposition answers what the runtime should do after drain
- `RuntimeIngressPolicy` should own this routing decision, with
  `RuntimeControlMachine` / `TurnExecutionMachine` consuming the result rather
  than rediscovering it ad hoc

Timing rule:

- admission-time policy decides whether an item is accepted and assigns
  `InterruptPolicy` and `DrainPolicy`
- drain-time policy assigns `RoutingDisposition` using the admitted item plus
  the current runtime/drain snapshot

This split exists because urgency/wake semantics must be known before the next
boundary, while post-drain handling may legitimately depend on runtime state at
the drain point.

The exact enum names may change, but `0.5` should preserve this separation.

### Interruption Semantics

Cooperative-suspension interruption is a third policy axis and should not be
collapsed into either drain urgency or post-drain routing.

Examples:

- some items may interrupt `wait` and later route to `ContextOnly`
- some items may interrupt `wait` and route to `StartOrContinueTurn`
- some items may not interrupt cooperative suspension at all

Conceptually, runtime ingress policy should be able to decide all three:

- `InterruptPolicy`
- `DrainPolicy`
- `RoutingDisposition`

Recommended ownership split:

- admission policy resolves `InterruptPolicy` and `DrainPolicy`
- drain planning resolves `RoutingDisposition`

### Landing Zone For Current Runtime Inputs

The current runtime input model is richer and more mixed than the `0.5`
canonical ingress taxonomy.

Current implementation anchor:
the runtime already exposes these inputs directly as
`Input::{Prompt, Peer, FlowStep, ExternalEvent, SystemGenerated, Projected}`
with `PeerConvention::{Message, Request, ResponseProgress, ResponseTerminal}`.

Current-to-target direction:

- `Prompt` -> `WorkInput::Prompt`
- `FlowStep` -> `WorkInput::PromptLike` admitted by orchestration
- `Peer` -> `PeerInput`
- `ExternalEvent` -> `WorkInput::Event`
- `Operation` -> `OperationInput`
- `Continuation` -> `ContinuationInput`
- `SystemGenerated` -> removed as a top-level ingress family; each current use
  must become explicit `ContinuationInput`, explicit `OperationInput`, or a
  boundary-local reducer/effect path outside runtime ingress
- `Projected` -> removed as a top-level ingress family; each current use must
  become explicit `ContinuationInput`, explicit `OperationInput`, or
  reducer/projection output outside runtime ingress
- current comms `PlainEvent` / external event-listener input -> `WorkInput::Event`
  even if it enters through comms infrastructure today

Peer lifecycle note:

- lifecycle-like peer notices can remain `PeerInput` when they arrive through
  peer comms, with routing policy deciding lifecycle folding rather than
  reclassifying them as `OpEvent`

Important:
`Projected` in current `0.4` runtime input is not the same thing as
`Projection` in the `0.5` nomenclature.

Firm `0.5` rule:
`Projected` is not a durable `0.5` runtime category and does not survive as a
top-level ingress family.

### Firm Runtime Policy Baseline

`0.5` work should use the existing runtime `DefaultPolicyTable` /
`PolicyDecision` behavior as the implementation baseline while making the
semantics explicit as `InterruptPolicy`, `DrainPolicy`,
`RoutingDisposition`, queue discipline, and consume point.

Current baseline:

- `Prompt`
  `ApplyMode::StageRunStart`; `WakeMode::WakeIfIdle` when idle and
  `WakeMode::None` when already running; `QueueMode::Fifo`;
  `ConsumePoint::OnRunComplete`
- `PeerConvention::Message`
  `ApplyMode::StageRunStart`; `WakeMode::WakeIfIdle` when idle and
  `WakeMode::InterruptYielding` when already running; `QueueMode::Fifo`;
  `ConsumePoint::OnRunComplete`
- `PeerConvention::Request`
  same baseline as `PeerConvention::Message`
- `PeerConvention::ResponseProgress`
  `ApplyMode::StageRunBoundary`; `WakeMode::None`; `QueueMode::Coalesce`;
  `ConsumePoint::OnRunComplete`
- `PeerConvention::ResponseTerminal`
  `ApplyMode::StageRunStart`; `WakeMode::WakeIfIdle` when idle and
  `WakeMode::None` when already running; `QueueMode::Fifo`;
  `ConsumePoint::OnRunComplete`
- `FlowStep`
  `ApplyMode::StageRunStart`; `WakeMode::WakeIfIdle` when idle and
  `WakeMode::None` when already running; `QueueMode::Fifo`;
  `ConsumePoint::OnRunComplete`
- `ExternalEvent`
  `ApplyMode::StageRunStart`; `WakeMode::WakeIfIdle` when idle and
  `WakeMode::None` when already running; `QueueMode::Fifo`;
  `ConsumePoint::OnRunComplete`
- `SystemGenerated`
  `ApplyMode::InjectNow`; `WakeMode::None`; `QueueMode::None`;
  `ConsumePoint::OnAccept`
- `Projected`
  `ApplyMode::Ignore`; `WakeMode::None`; consume on accept; no canonical
  prompt-turn routing, with queue handling following the current runtime policy
  table

The conceptual terms in this document map onto that implementation roughly as:

- `InterruptPolicy` aligns primarily with `WakeMode::InterruptYielding`
- `DrainPolicy` aligns primarily with wake/apply timing concerns
- `RoutingDisposition` aligns primarily with `ApplyMode` plus runtime-state
  specific drain planning

Important nuance:
the current `WakeMode` combines two concerns that the conceptual model treats
separately:

- interrupting an already-yielding runtime
- waking an idle runtime so drain/application can begin

`0.5` may separate those concerns more explicitly or may keep them unified in
implementation if that proves simpler, but the current overlap should be
treated as intentional baseline behavior rather than an accidental mismatch in
the docs.

If `0.5` simplifies or replaces this model, the authoritative runtime machine
definitions, verification, and companion docs must all change together rather
than drifting apart.

### Wait And Cooperative Suspension

`wait` should not define the whole architecture.

The dominant runtime states are:

- `IdleHost`
- `BusyTurn`

`wait` is a secondary special case inside `BusyTurn`: a cooperative
suspension point that allows an earlier admissible drain boundary.

## How Comms And Mobs Fit

### Comms

`Comms` is the lightweight peer plane:

- real-time steerable agent-to-agent communication
- request/response correlation
- trust/auth classification

It does not own child creation, background work lifecycle, or orchestration.

### Mobs

`Mobs` is the only child-agent orchestration path in `0.5`.

A "spawn a subagent" request is therefore a mob preset/workflow:

- create or reuse a mob
- ensure the calling agent is the orchestrator
- add and wire a member
- observe lifecycle and report-back behavior through mob and ops-lifecycle
  machinery

This means `0.5` has exactly two user-facing wiring levels:

- `Comms`
- `Mobs`

There is no third lightweight subagent mechanism in between them.

### Firm Convergence

The ownership model is:

- `PeerCommsMachine` owns bidirectional conversational traffic
- `MobOrchestratorMachine` owns member creation, attachment, wiring, roster,
  flow, and supervision semantics
- `OpsLifecycleMachine` owns async/background operation lifecycle and
  terminality for mob-backed child work and background tool operations

Implementation consequence:

- `SubAgentManager` is deleted rather than preserved as an owner or façade
- `SubagentResult` is deleted as a lifecycle/comms contract
- browser or host "subagent" behavior, where supported, is implemented as mob
  member orchestration rather than as a separate mechanism

## Hooks, Realms, Persistence, And External Tools

### Hooks

Hooks are reducer-shaped.

They should be invoked at named runtime/turn boundaries and return decisions
or patches, but they should not be modeled as a competing live machine unless
background hook orchestration eventually warrants that.

### Realms

Realms are bootstrap/environment concepts first.

They matter for:

- attachment
- namespace/root ownership
- backend selection
- durability integration

They should not be promoted into a top-level temporal machine unless the
system later needs formal realm attach/resume/migrate/close lifecycle.

### Persistence

Persistence should stay bootstrap-and-adapter shaped at the architecture
level, with runtime/session artifacts being persisted through explicit stores.

### External Tools And MCP

Live external tool lifecycle is a real machine:

- staged add/remove/reload
- pending activation/removal
- active tool surface snapshot
- boundary update emission

Tool visibility, however, remains a reducer applied at boundaries.

This is a good example of two first-class concepts that should stay distinct:

- external tool surface lifecycle
- provider-visible tool selection

Runtime-facing seam:

- synchronous staged-apply deltas are pulled/applied at named runtime
  boundaries by `RuntimeControlMachine`
- async completions/failures are emitted as typed `SurfaceUpdateNotice`
  messages on a dedicated surface-update channel owned by
  `RuntimeControlMachine`

They should not be tunneled through peer comms or disguised as ordinary
runtime ingress.

Firm runtime-notice rule:

- machine-originated diagnostic/status updates must cross runtime boundaries as
  typed notices first (`SurfaceUpdateNotice`, `RuntimeNotice`, etc.)
- runtime may choose to project some notices into transcript-visible synthetic
  context for model/operator visibility
- those transcript messages are derivative projections, not the authoritative
  notice transport
- `0.5` should not treat `[SYSTEM NOTICE]`, `[MCP_PENDING]`, or similar
  synthetic user messages as the primary machine-to-runtime interface

This means the current synthetic transcript notices are acceptable as
transitional UX, but the durable architecture contract is typed notice flow
plus optional projection policy.

## Effect Model

`0.5` does not require every machine to become fully event-sourced or purely
effect-driven.

The architectural requirement is that transition-time outputs become explicit
enough to reason about, test, and compose.

Canonical effect families for `0.5`:

- `TurnExecutionEffect`
  canonical baseline is the existing `RunEvent` family:
  `RunStarted`, `BoundaryApplied`, `RunCompleted`, `RunFailed`,
  `RunCancelled`
- `RuntimeControlEffect`
  `ScheduleDrain`, `SubmitRunPrimitive`, `ApplyControlCommand`,
  `EmitRuntimeNotice`
- `PeerCommsEffect`
  `SubmitPeerInput`, `EmitPeerNotice`, `DropTransportInput`
- `OpsLifecycleEffect`
  `SubmitOpEvent`, `NotifyOpWatcher`, `CancelChildAgent`
- `ExternalToolSurfaceEffect`
  `ApplySurfaceDelta`, `EmitSurfaceUpdateNotice`, `ScheduleSurfaceCompletion`

Implementations may realize these effects in different ways:

- reducers and policy layers should prefer returning typed effect values
- machine owners may execute those typed effects imperatively at the owner
  boundary
- direct cross-machine method calls should be modeled as execution of explicit
  typed effects, not hidden inside reducer logic

What matters is that the effect boundary is explicit. `0.5` should not hide
cross-machine consequences inside incidental control flow.

Notice projection rule:

- machine owners emit typed notices/effects
- `RuntimeControlMachine` owns any policy that projects selected notices into
  transcript-visible synthetic context
- transcript projection must be optional and derivative
- model-visible synthetic context must not be the sole authoritative record of
  runtime/tool-surface state

Firm naming rule:
these effect-family names are the canonical architecture names for `0.5`, even
if concrete Rust enums or structs are introduced incrementally during
migration.

## Cross-Machine Error And Recovery Semantics

`0.5` should make cross-machine failure handling explicit.

Preferred shape:

- `TurnExecutionMachine` emits typed failure/recovery effects upward to
  `RuntimeControlMachine`
- `RuntimeControlMachine` owns the decision to retry, recover, retire, stop,
  or surface failure
- `PeerCommsMachine` transport/admission failures remain comms-local unless
  explicitly escalated as runtime notices or control actions
- `OpsLifecycleMachine` failures surface as `OpEvent` when they are relevant to
  the owning runtime
- `ExternalToolSurfaceMachine` activation/removal failures surface as
  boundary-visible notices/effects, not as peer inputs

The key rule is that one machine may observe or emit another machine's failure
information, but responsibility for recovery/termination must still have a
clear owner.

## Testing Posture

`0.5` should preserve and strengthen the current testability advantages of the
core/runtime architecture.

Preferred properties:

- transition validation remains independently unit-testable
- reducers and policies can be tested without I/O or mocks
- machine owners expose state/effect boundaries clearly enough that tests can
  assert both state movement and emitted effects
- I/O, persistence, transport, and background task runners remain outside the
  pure transition core where possible

This does not require every machine to be implemented as one pure function. It
does require that the rules of the machine remain testable without driving the
whole world around it.

## Formal Verification Posture

`0.5` should lean into formal verification as a product-level differentiator,
not stop at ordinary unit testing.

Firm direction:

- every top-level machine and owned submachine should have an explicit state
  model, input alphabet, transition relation, and invariant set
- finite transition spaces should have exhaustive transition-coverage tests in
  Rust over the closed `State x Input` relation
- higher-value coordination machines should also have abstract formal specs for
  model checking rather than relying only on implementation tests

Preferred formal baseline:

- use the Rust machine/reducer implementation for executable transition tests
- use an abstract machine spec language such as TLA+ for model checking safety
  and liveness properties of the most important machines
- treat the formal model as the authoritative machine contract; Rust
  implementation should refine it rather than drift from it

Theorem-prover option rule:
machine schemas where present, and formal specs for every machine, should be
written declaratively enough that they could be transliterated into stronger
proof systems such as Lean without semantic reinterpretation, even if `0.5`
itself stops at model checking rather than full theorem proving.

Priority targets for formal specs/model checking:

- `RuntimeControlMachine`
- `RuntimeIngressMachine`
- `InputLifecycleMachine`
- `PeerCommsMachine`
- `TurnExecutionMachine`
- `OpsLifecycleMachine`
- `ExternalToolSurfaceMachine`
- `MobOrchestratorMachine`
- `MobLifecycleMachine`
- `FlowRunMachine`

Checked-in formal-spec rollout:

- the checked-in machine-spec bundle now lives under `specs/machines/`
- `Phase 1` in `meerkat_0_5_execution_plan.md` makes this bundle the enforced
  repo verification surface and wires CI gating around it
- the current checked-in validated set is:
  - `RuntimeIngressMachine`
  - `RuntimeControlMachine`
  - `InputLifecycleMachine`
  - `PeerCommsMachine`
  - `ExternalToolSurfaceMachine`
  - `TurnExecutionMachine`
  - `OpsLifecycleMachine`
  - `MobOrchestratorMachine`
  - `MobLifecycleMachine`
  - `FlowRunMachine`
- the checked-in bundle now covers the full firm `0.5` machine census with
  CI-bounded model profiles
- current bounded TLC snapshot:
  - `RuntimeIngressMachine`: `12,127` distinct states
  - `RuntimeControlMachine`: `84`
  - `InputLifecycleMachine`: `17`
  - `PeerCommsMachine`: `13,672`
  - `ExternalToolSurfaceMachine`: `7,056`
  - `TurnExecutionMachine`: `53`
  - `OpsLifecycleMachine`: `49,729`
  - `MobOrchestratorMachine`: `32`
  - `MobLifecycleMachine`: `6`
  - `FlowRunMachine`: `80`
- model-driven clarifications already folded back into the checked-in specs:
  - terminal per-instance machines model explicit quiescent terminal stutter
    rather than treating terminality as a deadlock
  - coalesced input lifecycle does not require clearing `last_run`; the proven
    invariant is about boundary metadata, not forced run-metadata erasure

Minimum properties to prove/check:

- no illegal state transition is reachable
- terminal states reject further transitions unless explicitly modeled
- admitted work is not silently dropped, duplicated, or consumed twice
- pending drain obligations eventually resolve under the modeled fairness
  assumptions
- control-plane preemption obeys its precedence rules
- external tool lifecycle preserves boundary-apply/removal invariants

Important scope note:
the goal is not necessarily theorem-proving the full Rust implementation.
The goal is to have a mathematically checked model of the machine semantics and
to keep the Rust implementation aligned with that model through exhaustive
transition tests, invariant tests, and reviewable refinement.

Expected implementation discipline:

- each machine should expose or derive a closed transition table/relation
- each machine should list its invariants explicitly in docs and tests
- migration steps that significantly alter a top machine should update the
  formal model alongside the implementation
- if a machine cannot yet be modeled exhaustively, the gap should be called out
  explicitly rather than hand-waved

### Verification Mechanism

The verification story for `0.5` should be concrete and repeatable, not just a
statement of intent.

Companion workflow docs:

- `meerkat_machine_formalization_strategy.md`
- `meerkat_machine_schema_workflow_spec.md`

Artifact rules:

- every canonical machine must carry:
  - checked-in contract
  - checked-in formal model
  - checked-in CI profile
  - checked-in mapping note
  - one explicit Rust owner
  - one explicit Rust transition boundary
- `SchemaKernel` machines additionally carry:
  - checked-in `schema.yaml`
  - generated Rust kernel
  - generated contract/model artifacts
- `PureHandKernel` machines additionally carry:
  - one hand-written pure kernel
  - kernel-focused Rust tests
- no canonical machine may remain in `BoundaryRedesign` at the end of `0.5`

Repo-shape and command rule:

- the authoritative repo layout, schema format, codegen commands, and CI gates
  are defined in `meerkat_machine_schema_workflow_spec.md`

### Verification Gates

`0.5` should treat verification as a delivery gate for machine work.

Minimum gate for introducing or materially changing a machine:

- contract updated
- formal model updated
- CI profile updated if needed
- mapping note updated if the abstraction boundary changed
- Rust kernel/tests updated
- generated artifacts refreshed for `SchemaKernel` machines

Verified-set gate:
the following machines should be considered mandatory formal-spec machines for
`0.5`:

- `RuntimeControlMachine`
- `RuntimeIngressMachine`
- `InputLifecycleMachine`
- `PeerCommsMachine`
- `ExternalToolSurfaceMachine`
- `TurnExecutionMachine`
- `OpsLifecycleMachine`
- `MobOrchestratorMachine`
- `MobLifecycleMachine`
- `FlowRunMachine`

The verified set should run in CI. If model checking is too expensive for the
default fast path, it should run in required verification CI rather than being
optional or manual.

Implementation-mode gate:

- the required final implementation mode for each machine is defined in
  `meerkat_machine_formalization_strategy.md`
- `BoundaryRedesign` is allowed only as an execution-phase state, never as the
  final release state of a canonical `0.5` machine

Important interpretation rule:

- TLC state counts should be read as reachable bounded microstates of the
  formal model, not as counts of named machine states
- large-but-manageable microstate counts are expected once queue order,
  lifecycle position, pending work, and control flags are part of the model

### Recommended Test/Proof Stack

Preferred layered mechanism:

- Rust unit tests for explicit transition legality
- Rust property tests / random trace tests for invariant preservation under
  generated transition sequences
- TLA+ model checking for safety/liveness of the architecture-level machine
- optional trace replay tests where execution traces from Rust are checked
  against the allowed formal transition relation

This gives three kinds of confidence:

- implementation correctness for enumerated transitions
- robustness under long/random traces
- mathematical confidence that the architecture-level machine admits no illegal
  reachable states under the modeled assumptions

### Proof Posture

When we say "proof" in the `0.5` plan, the intended meaning is:

- machine semantics are formally modeled
- safety/liveness claims are machine-checked against that model
- Rust implementations are constrained by explicit transition APIs and tested
  against the same contract

This is stronger than ordinary testing, while remaining realistic about the
fact that we are not theorem-proving the entire Rust codebase.

## Concurrency And Ownership Model

The preferred `0.5` concurrency model is conservative:

- preserve one mutable runtime owner per session/runtime instance
- preserve channel/signal-based ingress from background work and I/O
- avoid introducing shared mutable execution state for the core turn loop

Expected shape:

- `RuntimeControlMachine`
  single-writer owner per session/runtime instance
- `PeerCommsMachine`
  may have shared transport/listener infrastructure, but per-session admitted
  peer input still enters the owning runtime through typed ingress
- `OpsLifecycleMachine`
  has a per-session registry view even if some worker pools/executors are
  shared process-wide
- `MobOrchestratorMachine`
  remains an owner of mob-local state, not a shared mutable overlay on agent
  runtime internals

This preserves the current "dedicated task owns the mutable session/runtime"
property while still allowing richer typed producers around it.

## Assembly, Facades, And Migration

`0.5` should not pretend the current facade/factory layer does not exist.

Today, major assembly happens in:

- `AgentFactory::build_agent()`
- `SessionService`
- `RuntimeSessionAdapter`

That assembly/facade layer is load-bearing and should remain explicit.

Preferred `0.5` stance:

- keep a factory/assembly layer
- keep surface-facing services/facades
- let those layers construct and wire the canonical machines
- do not force every surface to instantiate low-level machines directly

Likely migration behavior:

- existing public APIs such as `SessionService`, `AgentFactory`,
  `CommsRuntime`, `Agent`, and related builders remain as facades/adapters for
  at least part of the transition
- new machine vocabulary should appear first in architecture docs and new
  internal seams
- deprecation/renaming should be deliberate and staged, not assumed

Preferred migration order:

1. clarify `RuntimeControlMachine` vs `TurnExecutionMachine`, while preserving
   the current single-writer runtime owner and explicit control plane
2. stabilize `RuntimeIngressMachine` / `InputLifecycleMachine` and normalize
   the landing zone for current runtime input kinds
3. extract `PeerCommsMachine` around classify-before-enqueue admitted peer
   ingress
4. align `ExternalToolSurfaceMachine` explicitly with runtime boundary and
   surface-update seams
5. extract `OpsLifecycleMachine`, including moving delegated-work completion
   out of comms-shaped inbox paths
6. decompose `MobActor` toward `MobOrchestratorMachine`,
   `MobLifecycleMachine`, and `FlowRunMachine`

Dependency note:

- step 1 should resolve how `RuntimeLoop` subsumes the current host
  idle/wake/drain loop while preserving the firm `TurnExecutionMachine`
  contract before host-mode cutover lands
- step 5 must define the concrete `OpsLifecycleMachine` trait/registry seam
  before step 6 begins, because `MobActor` decomposition depends on that
  shared async-operation substrate existing first

### Expected Deletions And Simplifications

The payoff of convergence should include real deletion, not only renaming.

Target deletions/simplifications include:

- the legacy direct host-mode comms processing path in `comms_impl.rs`
- direct `drain_comms_inbox()`-driven runtime bypass behavior
- host idle/wake/drain coordination living inside `Agent` rather than runtime
  control
- the current classified-inbox / dual-channel comms mechanism once peer
  normalization is moved to `PeerCommsMachine` admission-time processing
- direct comms-side routing/batching logic that duplicates runtime policy-table
  behavior

Important:
this does not mean trust/auth snapshotting, peer normalization, or peer intent
classification disappear. Those responsibilities stay, but migrate into
`PeerCommsMachine` as admission-time normalization rather than inbox-specific
mechanics.

## Current 0.4 To Target 0.5 Mapping

The following table is intentionally approximate. It exists to keep the
target model grounded in the current codebase.

| Current 0.4 concept/type | Target 0.5 concept | Notes |
| --- | --- | --- |
| `LoopState` | `TurnExecutionState` | Current anchor for the inner turn loop |
| core agent run loop | `TurnExecutionMachine` | Owns LLM/tool/wait/boundary execution semantics |
| `RunPrimitive` | canonical `TurnExecutionMachine` input family | Runtime-to-core execution primitive |
| `RunEvent` | canonical `TurnExecutionEffect` family | Core-to-runtime execution effect family |
| `RuntimeStateMachine` | part of `RuntimeControlMachine` | Outer runtime control lifecycle |
| `RuntimeLoop` | part of `RuntimeControlMachine` runtime owner | Current per-session wake/dequeue/apply loop |
| `RunControlCommand` + control channel | `ControlPlane` | Out-of-band runtime authority path |
| `RuntimeSessionAdapter` | runtime/service adapter | Bridge between session-facing services and runtime control |
| `RuntimeDriver::accept_input()` + runtime queue | `RuntimeIngressMachine` | Canonical runtime admission and ingress owner |
| `InputStateMachine` | `InputLifecycleMachine` | Per-ingress-item lifecycle submachine |
| `DefaultPolicyTable` + `PolicyDecision` | `RuntimeIngressPolicy` anchor | Existing runtime policy model is richer than the simplified architecture slice |
| `CommsRuntime` + `RuntimeCommsInputSink` | `PeerCommsMachine` anchor | Current peer ingress/admission bridge into runtime |
| `SubAgentManager` | deleted legacy owner | Child-agent flows become mob configuration over `MobOrchestratorMachine` + `OpsLifecycleMachine` |
| `MobActor` | over-centralized precursor to `MobOrchestratorMachine` | Should be decomposed around lifecycle/flow/host supervision concerns |
| `Agent<C, T, S>` | primary execution component inside `TurnExecutionMachine` | Should shrink toward a focused turn executor |
| `McpRouter` | `ExternalToolSurfaceMachine` anchor | Already mature and state-machine-shaped |
| WASM `RuntimeState` | browser runtime bootstrap owner | Should grow to own runtime adapter, browser tool surface, and direct-session handle mapping |
| WASM direct-session registry / handle map | transitional browser session facade | Should collapse into façade handle -> `SessionId` mapping over runtime-backed sessions |
| WASM `start_turn()` direct build/run path | transitional compatibility export | Should delegate to runtime admission + completion waiting |
| WASM pre-init JS tool registry | transitional browser tool surface anchor | Should become runtime-scoped browser tool-surface state |
| `ToolScope` | `ToolVisibilityReducer` + boundary-owned state | Not a top-level machine |
| `SessionService` | service layer | Surface/registry/service concept, not canonical runtime machine |
| `Session` | session state value with dual role today | Currently serves both live working-state and serializable/persistable roles |
| `AgentFactory::build_agent()` | assembly/factory layer | Central integration seam that should remain explicit |

## Architectural Confusions To Remove In 0.5

These are the most important current sources of incoherence.

### 1. Runtime overload

Today "runtime" can mean both:

- the outer control plane
- the inner turn execution loop

`0.5` should use:

- `RuntimeControlMachine`
- `TurnExecutionMachine`

### 2. Comms carrying non-comms concepts

The architecture should stop relying on peer comms paths for:

- delegated-work result delivery
- orchestration-only injected work
- control conventions like stringly dismiss messages

### 3. Mixed ingress taxonomies

The current runtime input model mixes several axes at once:

- semantic kind
- provenance
- orchestration source
- derived/internal status

`0.5` should normalize the canonical ingress model so these axes are not
randomly flattened together.

### 4. Parent machines and submachines presented as peers

`RuntimeIngressMachine`, `InputLifecycleMachine`, and `FlowRunMachine` are real
machines, but they should not appear in the same conceptual list as
top-level domain owners without explicit labeling.

### 5. Session and runtime conflation

`Session` is not the runtime machine.
`SessionService` is a service.
`RuntimeControlMachine` is the canonical operational machine.

Current note:
today's `Session` value still serves both live working-state and durable
serialization roles. `0.5` should make the conceptual split explicit even if
the code continues to reuse one Rust type for both roles.

### 6. Mob actor over-centralization

The current mob actor still appears to own too many concerns at once:

- member lifecycle
- orchestration lifecycle
- autonomous host loop supervision
- flow execution
- MCP process management
- task board mutation

`0.5` should separate those concerns more explicitly around the machine
census above.

## Firm Architectural Decisions

The current firm architecture decision set for `0.5` is:

- one canonical runtime ingress queue per runtime instance
- top-level machines:
  `RuntimeControlMachine`, `PeerCommsMachine`, `OpsLifecycleMachine`,
  `MobOrchestratorMachine`, `ExternalToolSurfaceMachine`
- owned submachines:
  `RuntimeIngressMachine`, `InputLifecycleMachine`, `TurnExecutionMachine`,
  `MobLifecycleMachine`, `FlowRunMachine`
- reducers:
  hooks and tool visibility
- bootstraps:
  realms and persistence
- services:
  session-facing orchestration and surface integration
- artifacts:
  session snapshots/history and persisted receipts

## What Success Looks Like

By the end of the `0.5` architectural convergence, a reader should be able to
answer the following questions quickly and unambiguously:

- Which machine owns this lifecycle?
- Is this thing a top-level machine or an internal submachine?
- Is this ingress, comms, ops, control, or tool-surface update?
- Is this a machine, reducer, policy, bootstrap, service, or artifact?
- Where does a given event enter the system?
- Which boundary applies it?
- Which owner is allowed to transition it?

If those answers are clear, the system will be much closer to the "platonic"
shape the refactor is aiming for.
