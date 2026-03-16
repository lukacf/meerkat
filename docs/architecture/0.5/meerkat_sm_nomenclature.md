# Meerkat 0.5 Nomenclature

This document defines the canonical naming and conceptual structure for
Meerkat's `0.5` machine-oriented architecture.

Companion document:

- `docs/architecture/0.5/meerkat_0_5_architecture_outline.md`

The goal is to make the system:

- understandable
- consistent
- inspectable
- mechanically mappable into code, diagrams, and formal models

The central rule is that names must reflect architectural role, not just
implementation convenience.

This document is target-oriented, not a claim that every current `0.4` crate
API already uses this vocabulary. Existing public names may lag during
migration. New docs and new architecture-facing code should prefer this
terminology, and legacy names should be mapped explicitly rather than silently
reinterpreted.

Change control:

- this document is the naming companion to the `0.5` architecture outline
- naming changes should be made deliberately and reflected in both documents
  rather than drifting in one place only

## Core Naming Rules

### 1. Use `Machine`, never `StateMachine`

Canonical form:

- `RuntimeControlMachine`
- `PeerCommsMachine`
- `MobOrchestratorMachine`

Avoid:

- `RuntimeControlStateMachine`
- `CommsStateMachine`

Reason:
`Machine` is shorter, cleaner, and pairs naturally with `State`.

### 2. Distinguish top-level machines from owned submachines

Not every machine is at the same architectural level.

- `Top-level machine`
  A primary domain owner in the system.
- `Owned submachine`
  A machine that exists inside a top-level machine and serves that owner.

Example:

- `RuntimeControlMachine` is a top-level machine.
- `RuntimeIngressMachine` is an owned submachine of runtime control.
- `InputLifecycleMachine` is an owned submachine of runtime ingress.

This distinction is mandatory in docs and diagrams. Do not present parent
machines and internal submachines as peers without labeling the difference.

### 3. A `Machine` is a live temporal owner

A type or subsystem should only be called a `Machine` if it:

- owns authoritative state across time
- accepts typed inputs
- performs explicit transitions
- emits typed effects
- has explicit invariants

If it does not own temporal behavior, do not call it a machine.

Important:
some current `0.4` implementation anchors are only machine-shaped precursors.
Calling something a target `0.5` machine means it should either already satisfy
these criteria or be an explicit formalization target during the refactor.

Also important:
the machine name applies to the owned temporal concern, not necessarily to one
already-perfect concrete struct. During migration, a machine family may be
embodied as a state-holding owner plus reducer/validator helpers, or as a
command processor whose transition/effect boundaries are still being made more
explicit.

Formal transition rule:

- a machine's authoritative state should only move through its defined
  transition/apply/reducer boundary
- ad hoc mutation of machine state outside that boundary is not acceptable as a
  `0.5` steady-state design
- if temporary escape hatches exist during migration (`force_transition`,
  compatibility mutators, direct field mutation inside legacy owners), they
  should be treated as explicit debt to remove
- every machine should publish or derive a closed transition relation over its
  authoritative state/input model, even if the concrete Rust implementation is
  staged

### 4. Standard family of names

Every real machine should use the same conceptual family:

- `XMachine`
- `XState`
- `XInput`
- `XEffect`
- `XSnapshot`

Optional:

- `XCommand`
- `XEvent`
- `XGuard`
- `XReducer`
- `XPolicy`

Example:

- `RuntimeIngressMachine`
- `RuntimeIngressState`
- `RuntimeIngressInput`
- `RuntimeIngressEffect`
- `RuntimeIngressSnapshot`

## Non-Machine Terms

These terms should be used consistently and should not be renamed into
`Machine`.

### `Reducer`

A pure deterministic transform.

Use for:

- `HookDecisionReducer`
- `MobSpawnPolicyReducer`
- `ToolVisibilityReducer`
- any pure transition or merge logic

Important:
effectful hook invocation is not itself a reducer. The reducer-shaped part of
hooks is the merge/decision step over hook outcomes.

### `Policy`

A classification, routing, or decision table.

Use for:

- `IngressClassificationPolicy`
- `RuntimeIngressPolicy`
- `ToolSelectionPolicy`

Current implementation anchors may already exist in runtime code.
For example, `DefaultPolicyTable` and `PolicyDecision` are current anchors for
runtime ingress policy-shaped behavior.

### `Bootstrap`

Environment setup, opening, attachment, or initialization.

Use for:

- `RealmBootstrap`
- `PersistenceBootstrap`

### `Adapter`

Boundary wrapper for an external or lower-level implementation.

Use for:

- `McpRouterAdapter`
- `RuntimeSessionAdapter`

Important:
real implementations may combine multiple roles (`Adapter`, `Gateway`,
`Dispatcher`, `Service`). These labels are architectural roles, not mutually
exclusive kinds. Naming should prioritize the dominant role of the abstraction
in context.

### `Dispatcher`

Execution routing abstraction over one or more capabilities.

Use for:

- `AgentToolDispatcher`
- tool dispatchers
- capability dispatch interfaces

Important:
`Dispatcher` is an interface role, not an ownership level. A concrete
dispatcher may also be a `Machine`, but it is not a machine merely because it
dispatches work.

### `Gateway`

Composing or mediating forwarding layer between dispatchers, machines, or
services.

Use for:

- `ToolGateway`
- forwarding/aggregation layers

Important:
Gateways are usually structural composition layers, not temporal owners.

### `Surface`

External protocol, UI, API, or operator-facing boundary.

Use for:

- CLI
- REST
- RPC
- MCP server
- user/operator-facing control planes

### `Service`

Surface-facing or registry-style operational owner.

Use for:

- `MobTaskBoardService`
- `MobTopologyService`
- `MobSupervisorService`
- `SessionService`
- `EphemeralSessionService`
- `PersistentSessionService`

Important:
Services may own live tasks and admission logic, but should only be called
`Machine` when they are themselves the canonical temporal owner being modeled.

### `Projection`

Derived, query-oriented, or UI-oriented view state.

Use for:

- `MobProjection`
- `RuntimeProjection`

Important:

- a `Snapshot` is the authoritative current read view of one machine
- a `Projection` is a derived/read-optimized view that may reshape or combine
  state for query, UI, or reporting purposes

A projection may be built from one or more snapshots, but it is not itself the
canonical owned state of the machine.

### `Artifact`

A durable or serializable domain object, not a temporal owner.

Use for:

- `Session`
- persisted receipts
- persisted snapshots

## Canonical Architectural Vocabulary

### Generic Terms

- `State`
  Durable logical status of a machine.

- `Input`
  Typed thing a machine consumes.

- `Command`
  Imperative request directed at a machine.

  Important:
  a command is a kind of input, but not all inputs are commands.

  Useful distinction:

  - `Input` includes observed/environmental arrivals the machine must handle
  - `Command` is an intentional request from a caller that the machine may
    accept, reject, or defer

- `Event`
  Fact that happened.

- `Notice`
  Typed informational update emitted across a machine boundary that is not
  itself ordinary admitted runtime work.

  Example shapes:

  - peer lifecycle notice
  - surface update notice
  - runtime diagnostic notice

  Firm architecture rule:
  a notice is authoritative in its typed machine-boundary form. If runtime
  chooses to render a notice into transcript-visible synthetic context
  (`[SYSTEM NOTICE]`, pending banners, debug summaries, etc.), that rendered
  message is a projection of the typed notice, not the canonical notice
  itself.

- `Effect`
  Typed output emitted by a transition at a machine boundary.

  Example shapes:

  - request a drain
  - start or continue a turn
  - enqueue admitted peer input
  - emit a surface update notice
  - persist a checkpoint at a named boundary

  Preferred implementation rule:
  use explicit typed effect values at machine boundaries. Owner-internal code
  may execute those effects imperatively, but cross-machine consequences should
  not be hidden inside incidental control flow.

- `Snapshot`
  Read-only current view.

- `Checkpoint`
  Durable captured state written at a named boundary.

- `Transition`
  `State x Input -> State + Effects`

  Preferred formalization rule:
  transitions should be expressible declaratively enough that the authoritative
  machine contract, and schema where one exists, can be transliterated into
  model-checking or theorem-proving tools without semantic guesswork.

- `Guard`
  Condition that allows or blocks a transition.

## Session And Runtime Terms

- `Session`
  Conversation/session state value carrying history, metadata, usage, and
  identifiers.

  Important:
  in current `0.4` code, `Session` serves both as live working state and as a
  serializable/persistable value. `0.5` architecture should distinguish the
  runtime machine from the session value even if a single Rust type continues
  to serve both roles.

- `SessionService`
  Surface-facing service/registry layer that manages live per-session
  ownership, admission, shutdown, inspection, and persistence integration.

- `RuntimeControlMachine`
  Canonical top-level operational machine for per-session control:
  admission decisions, control decisions, ownership of runtime ingress and turn
  execution, running, retiring, recovery, and stop/destroy lifecycle.

- `TurnExecutionMachine`
  Owned submachine of runtime control responsible for the actual turn loop:
  LLM calls, tool dispatch, cooperative suspension, and turn-boundary drain.

Important:
Do not use `session` as a synonym for runtime behavior. In `0.5`
terminology, the runtime is the machine; the session is the session value or
artifact, not the operational owner.

## Runtime-Specific Terms

### `Ingress`

A typed item entering the runtime-owned drain queue.

Canonical queue owner:

- `RuntimeIngressMachine`

### `Admission`

The runtime-owned act of accepting, sequencing, and enqueueing work into
canonical runtime ingress.

Important:

- external producers may submit typed candidate inputs for admission
- `RuntimeControlMachine` owns the admission surface
- `RuntimeIngressMachine` owns canonical sequencing and queue insertion
- external machines should not assign global ingress order on their own

### `Drain`

Consume a FIFO snapshot of ingress items.

### `AdmissibleBoundary`

A legal runtime point at which ingress may be drained.

Examples in the current system family:

- host idle wake points
- cooperative suspension interruption points (for example `wait`)
- turn/run boundary drain points

### `DrainPolicy`

The scheduling policy attached to an ingress item:

- `DrainAsap`
- `Passive`

Meaning:

- `DrainAsap`: force a drain at the earliest admissible boundary
- `Passive`: do not force a drain, but ride along on the next drain

Important:
`DrainPolicy` controls when drain must happen, not which items are dequeued.

Assignment:
`DrainPolicy` is assigned by ingress/runtime policy at admission time. It is
not synonymous with any single current classifier such as
`PeerInputClass::is_actionable()`, though such classifiers may contribute to
policy resolution.

Implementation note:
`DrainPolicy` is a runtime-owned scheduling decision. An implementation may
realize it as stored per-item metadata, as an admission-time wake side effect,
or both.

### `RoutingDisposition`

The post-drain handling assigned to an admitted ingress item.

Examples:

- `StartOrContinueTurn`
- `ContextOnly`
- `Fold`
- `Drop`

Important:
`RoutingDisposition` is separate from `DrainPolicy`.

- `DrainPolicy` answers when drain must happen
- `RoutingDisposition` answers what the runtime should do with the drained item

Assignment:
`RoutingDisposition` is assigned during drain planning by
`RuntimeIngressPolicy` using the admitted item plus the current
runtime/drain snapshot. It should be an explicit runtime-owned decision, not an
implicit side effect of dequeue order.

### `InterruptPolicy`

The policy that answers whether an admitted item interrupts cooperative
suspension such as `wait`.

Examples:

- `InterruptWait`
- `NoInterrupt`

Important:
`InterruptPolicy` is separate from both `DrainPolicy` and
`RoutingDisposition`.

- `InterruptPolicy` answers whether a currently suspended turn should wake now
- `DrainPolicy` answers when drain must happen
- `RoutingDisposition` answers what to do with the item after drain

Assignment:
`InterruptPolicy` is assigned at admission time by runtime ingress policy. It
is a runtime-owned wake decision, not a side effect inferred later from
post-drain routing.

### `PromptPriorityMode`

The admission-priority mode attached to an ordinary user prompt.

Examples:

- `Queue`
- `Steer`

Meaning:

- `Queue`: admit the prompt as ordinary queued work that waits for the next
  ordinary admissible boundary when a run is already in progress
- `Steer`: admit the prompt as ordinary queued work that requests ASAP
  handling at the earliest admissible cooperative boundary

Important:
`PromptPriorityMode` is not a control-plane override.

- it does not turn a user prompt into authority work
- it only changes how ordinary prompt work is scheduled once admitted

`0.5` runtime-policy mapping:

- `Queue`
  - idle: `wake=true`, `process=false`
  - running: `wake=false`, `process=false`
- `Steer`
  - idle: `wake=true`, `process=true`
  - running: `wake=true`, `process=true`

Assignment:
`PromptPriorityMode` is chosen at the user-prompt boundary and preserved
through runtime admission. It must not be reconstructed later from heuristics.

### `ControlPlane`

An out-of-band authority path for runtime control commands.

Examples:

- cancel current run
- stop runtime executor
- dismiss or shutdown style authority signals

Important:
not every control-shaped thing belongs in the ordinary FIFO runtime ingress
queue. A `ControlPlane` is allowed to remain distinct when immediacy or
authority semantics require it.

Precedence:

- control-plane commands preempt ordinary ingress processing
- control-plane commands are not assigned FIFO order relative to admitted
  runtime ingress
- pending drain obligations do not block control-plane authority actions

## Domain Input Taxonomy

Do not flatten everything into "message" or "event".

Use these domain-level nouns:

- `WorkInput`
  Prompt- or event-shaped work admitted into the runtime.

  Common sub-flavors:

  - operator/user prompt
  - orchestration-admitted turn input
  - externally sourced event input (for example current `PlainEvent`-style
    integrations)

- `PeerInput`
  Agent-to-agent conversational traffic.

- `OpEvent`
  Delegated-work lifecycle events such as progress/completion/failure.

- `ControlEvent`
  Runtime authority signals such as dismiss/stop/cancel/shutdown.

This taxonomy exists to keep peer conversation, orchestration, lifecycle, and
control distinct.

Important:
`ControlEvent` is a domain noun, not a guarantee that control must travel on
the ordinary runtime ingress queue. Some control flows may remain on an
explicit `ControlPlane`.

## Canonical Top-Level Machines

These are the canonical first-class top-level machines for `0.5`.

Important:
this is the firm architecture census for `0.5`, even though individual
implementation anchors differ in maturity and may still be embodied across more
than one current concrete type.

- `RuntimeControlMachine`
- `PeerCommsMachine`
- `OpsLifecycleMachine`
- `MobOrchestratorMachine`
- `ExternalToolSurfaceMachine`

## Canonical Owned Submachines

These are the canonical owned submachines for `0.5`, not top-level peers.

- `RuntimeIngressMachine`
- `InputLifecycleMachine`
- `TurnExecutionMachine`
- `MobLifecycleMachine`
- `FlowRunMachine`

## Canonical Non-Machines

These should not be named as machines:

- `HookDecisionReducer`
- `ToolVisibilityReducer`
- `IngressClassificationPolicy`
- `RuntimeIngressPolicy`
- `RealmBootstrap`
- `PersistenceBootstrap`
- `SessionService`

## Layering Guidance

### Runtime

`RuntimeControlMachine` is the top-level per-session operational owner.

It owns:

- runtime lifecycle
- admission and wake semantics
- `RuntimeIngressMachine`
- `TurnExecutionMachine`

It does not own:

- peer trust/auth semantics
- delegated-work lifecycle semantics
- mob orchestration semantics
- external tool surface lifecycle

### Comms

`PeerCommsMachine` owns:

- trust/auth acceptance
- peer classification
- peer lifecycle
- peer response correlation
- peer-to-peer conversational traffic

It produces `PeerInput` into runtime control.

It should not own:

- delegated-work completion
- prompt admission
- runtime control
- mob orchestration injection

### Ops

`OpsLifecycleMachine` owns:

- spawned/delegated work lifecycle
- progress
- completion
- failure
- cancellation

It produces `OpEvent` into runtime control.

### Mob

`MobOrchestratorMachine` owns high-level mob orchestration.

Its owned submachines are:

- `MobLifecycleMachine`
- `FlowRunMachine`

It is not a substitute for peer comms or runtime ingress.

### External Tools

`ExternalToolSurfaceMachine` owns live external tool/server lifecycle and
boundary-visible surface updates.

Tool visibility itself remains a reducer, not a machine.

## Terms To Avoid

These are too overloaded and should be phased out from architecture docs and
new internal design/code comments:

- `message` as a catch-all
- `event` as a catch-all
- `deliver now`
- `deliver end of turn`
- `inner loop`
- `outer loop`
- `plain event`
- `special case`
- `runtime` when you really mean turn execution
- `runtime` when you really mean the outer control plane
- `state machine` in type names

This guidance is for architecture vocabulary. User-facing APIs, guides, CLI
copy, and conversational shorthand may still use natural domain nouns like
`message` where precision would not be improved by heavier terminology.

Likewise, informal shorthands such as `inner loop` and `outer loop` are fine
in conversation when the mapping is clear:

- `inner loop` -> `TurnExecutionMachine`
- `outer loop` / `outer control plane` -> `RuntimeControlMachine`

Preferred replacements:

- `PeerInput`
- `WorkInput`
- `OpEvent`
- `ControlEvent`
- `DrainAsap`
- `Passive`
- `IdleHost`
- `BusyTurn`
- `AdmissibleBoundary`
- `RuntimeControlMachine`
- `TurnExecutionMachine`

## Legacy Mapping Guidance

Current `0.4` names may remain in code during migration. When they do, they
should be treated as legacy implementation anchors for the `0.5` concepts
rather than as competing architecture vocabulary.

Examples:

- `InputStateMachine` in current code is the likely implementation anchor for
  `InputLifecycleMachine`
- `LoopState` is the likely implementation anchor for `TurnExecutionState`
- `RuntimeStateMachine` in `meerkat-runtime` is the likely implementation
  anchor for part of `RuntimeControlMachine`
- `Input` in `meerkat-runtime` is the current implementation anchor for the
  runtime ingress taxonomy
- `DefaultPolicyTable` and `PolicyDecision` in `meerkat-runtime` are current
  implementation anchors for `RuntimeIngressPolicy`
- current `ProjectedInput` in `meerkat-runtime` is a legacy runtime input kind,
  not the same thing as `Projection` in the `0.5` nomenclature; it is a
  transitional compatibility input that `0.5` should remove rather than
  normalize as a canonical category

## File Naming Pattern

Canonical `0.5` layout for machine-oriented code uses module directories, not
flat sibling files.

Canonical pattern:

- `<machine_snake>/mod.rs`
- `<machine_snake>/machine.rs`
- `<machine_snake>/state.rs`
- `<machine_snake>/effect.rs`
- `<machine_snake>/input.rs`

Optional members when the machine needs them:

- `<machine_snake>/policy.rs`
- `<machine_snake>/reducer.rs`
- `<machine_snake>/queue.rs`
- `<machine_snake>/snapshot.rs`

Example:

- `runtime_control/mod.rs`
- `runtime_control/machine.rs`
- `runtime_control/state.rs`
- `runtime_control/effect.rs`
- `runtime_control/input.rs`

Transition note:
legacy flat files may remain temporarily during migration, but the canonical
`0.5` target layout is the module-directory pattern above.

## Final Rule

Do not call something a machine just to make it sound formal.

If it is not a live temporal owner, use the correct term:

- reducer
- policy
- bootstrap
- adapter
- projection
- service
- artifact

Precision in names is part of the architecture.
