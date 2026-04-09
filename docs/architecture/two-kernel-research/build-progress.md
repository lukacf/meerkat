# Two-Kernel Build Progress

Status: active implementation log

This file tracks the slow, verified implementation path from the research docs
toward real code.

## 2026-04-09 — Slice 1: Meerkat runtime spine snapshot

Goal:

- start building `MeerkatMachine` without forcing a premature semantic collapse
- verify the architecture against the current codebase continuously
- create an explicit Meerkat-side internal surface in code before moving
  behavior between owners

What landed:

- new internal diagnostic module:
  - `meerkat-runtime/src/meerkat_machine.rs`
- new internal runtime adapter method:
  - `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)`
- new ingress getters needed to read canonical Meerkat admission truth:
  - content shape
  - request id
  - reservation key
- driver accessors needed to project runtime identity cleanly:
  - ephemeral `runtime_id()`
  - persistent `runtime_id()`
- first verification tests in `meerkat-runtime/src/session_adapter.rs`

Current scope of the snapshot:

- `binding`
  - session id
  - runtime id
  - driver kind
  - attachment liveness
  - detached wake presence
  - hidden epoch id
- `control`
  - runtime phase
  - current run id
  - pre-run phase
  - wake/process flags
- `inputs`
  - ingress phase
  - admission order
  - queue and steer queue
  - current run contributors
  - wake/process request flags
  - silent intent overrides
  - per-input lifecycle and metadata snapshot

Deliberate non-scope for slice 1:

- no semantic rewrite
- no new reducer
- no mob changes
- no attempt yet to fold ops, peer ingress, tool surface, or drain lifecycle
  into the same projected structure

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the first compile exposed that the snapshot was mixing
  `runtime_control_authority::HandlingMode` with `meerkat_core::types::HandlingMode`
- `ContentShape` needed cloning rather than copying
- `InputLifecycleState` had to be imported from `input_state`, not from the
  authority wrapper

Why this slice matters:

- it gives the Meerkat refactor a real internal code anchor
- it verifies that the current runtime spine can already be observed as one
  grouped unit
- it keeps us honest about where current truth still lives

Next likely slice:

- extend the Meerkat internal projection to include `ops` and `drain`
- then decide whether to move `RuntimeSessionEntry` into an explicit
  Meerkat-side kernel struct or keep growing the projection first

## 2026-04-09 — Slice 2: `ops` and `drain` join the Meerkat spine

Goal:

- extend the existing Meerkat runtime projection with more of the real kernel
  coupling points
- keep the slice observational rather than rewriting runtime behavior
- verify the new projection against the authority-owned ops lifecycle and comms
  drain state

What landed:

- `MeerkatOpsSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `MeerkatDrainSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `RuntimeOpsLifecycleRegistry::diagnostic_snapshot()` in
  `meerkat-runtime/src/ops_lifecycle.rs`
- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects:
  - operation count
  - active operation count
  - authority-owned wait request id
  - authority-owned wait operation ids
  - operation lifecycle snapshots
  - detached wake pending/signaled flags
  - comms drain slot presence
  - comms drain phase
  - comms drain mode
  - comms drain handle presence
- new adapter tests covering:
  - live operation state
  - wait-all state
  - comms drain state

Why this slice matters:

- it brings two of the highest-bug-density Meerkat regions into the same
  explicit projected surface
- it proves that the current code already exposes authoritative wait/drain
  truth without needing a semantic rewrite first
- it keeps detached wake and comms drain visible as Meerkat internal facts
  rather than shell folklore

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --lib`

Result:

- passed

Backtracks encountered:

- the first wait-all test incorrectly minted its own `WaitRequestId`; the
  runtime authority owns that id, so the test now snapshots the real wait id
  and verifies it matches the eventual `WaitAllSatisfied` result
- the initial test also used a non-existent `OperationResult::Text(...)`
  helper; it had to be rewritten against the real struct shape used by
  `RuntimeOpsLifecycleRegistry`

## 2026-04-09 — Slice 3: strengthen hidden `binding`

Goal:

- move the Meerkat binding projection closer to the internal kernel shape
- expose hidden epoch cursor facts and kernel-handle presence without changing
  runtime behavior

What landed:

- `MeerkatCursorSnapshot` in `meerkat-runtime/src/meerkat_machine.rs`
- `MeerkatBindingSnapshot` now includes:
  - `driver_present`
  - `completions_present`
  - `ops_registry_present`
  - `cursor_state`
- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects
  `EpochCursorState` from the real `RuntimeSessionEntry`
- new adapter test covering:
  - epoch cursor projection from mutated live cursor state

Why this slice matters:

- it makes the hidden Meerkat binding more explicit as a kernel handle rather
  than just a runtime id plus attachment bit
- it confirms that epoch/cursor continuity is already a first-class Meerkat
  fact in the current codebase
- it gives us a firmer base before attempting a more structural Meerkat
  refactor

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat-runtime --lib`

Result:

- passed

Current shape of the projected Meerkat spine:

- `binding`
- `control`
- `inputs`
- `ops`
- `drain`

Still deliberately out of scope:

- no turn/execution-region projection yet
- no peer ingress region yet
- no tool-surface region yet
- no structural replacement of `RuntimeSessionEntry`

Next likely slice:

- project the Meerkat turn/execution region if the current runtime exposes a
  stable enough truth surface
- otherwise, make `RuntimeSessionEntry` itself a more explicit Meerkat kernel
  handle before attempting turn/tools collapse

## 2026-04-09 — Slice 4: cross-crate execution snapshot for Meerkat turn truth

Goal:

- expose the real turn/execution authority without inventing a fake
  runtime-local owner
- verify the Meerkat turn region against the current codebase as it actually
  exists today: in `meerkat-core` and the live session task
- create a mapping path from core agent state to session service that later
  MeerkatMachine work can consume

What landed:

- `AgentExecutionSnapshot` in `meerkat-core/src/agent.rs`
- `Agent::execution_snapshot()` in `meerkat-core/src/agent/runner.rs`
- `SessionAgent::execution_snapshot()` default hook in
  `meerkat-session/src/ephemeral.rs`
- new session-task command path:
  - `SessionCommand::ExecutionSnapshot`
  - `EphemeralSessionService::execution_snapshot(...)`
  - `PersistentSessionService::execution_snapshot(...)`
- `FactoryAgent` now forwards the real core-agent execution snapshot in
  `meerkat/src/service_factory.rs`

Current scope of the execution snapshot:

- coarse loop state
- rich turn phase from `TurnExecutionAuthority`
- active run id
- primitive kind
- admitted content shape
- vision/image-tool flags
- tool-call pending count
- pending async operation ids
- barrier operation ids
- barrier flags
- boundary count
- cancel-after-boundary
- terminal outcome
- extraction retry state
- applied cursor

Why this slice matters:

- it confirms that the Meerkat turn region is not owned by
  `meerkat-runtime`; it lives in the core agent loop and has to be mapped
  across crates honestly
- it gives the MeerkatMachine work its first real read path into the turn
  authority instead of forcing runtime-local mirroring
- it exposes both `LoopState` and `TurnPhase` side by side, which is useful
  because they are related views, not identical truths

Verification:

- `cargo test -p meerkat-core --lib test_execution_snapshot_reflects_turn_authority`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`
- `cargo test -p meerkat --lib factory_agent_execution_snapshot_forwards_core_state`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the planned runtime-only turn projection was wrong: `RuntimeSessionAdapter`
  does not own the core agent, so trying to add turn truth there first would
  have created another shadow seam
- adding the ops diagnostic slice made a few old `#[expect(dead_code)]`
  markers in `ops_lifecycle_authority` inaccurate; those were removed once the
  helpers became part of the live diagnostic path

Current MeerkatMachine picture after slice 4:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected region:
  - `turn`

Still deliberately out of scope:

- no peer ingress region projection yet
- no tool-surface region projection yet
- no unified Meerkat cross-crate snapshot that joins runtime + turn in one
  place yet
- no structural replacement of `RuntimeSessionEntry`

Next likely slice:

- either build the first cross-crate Meerkat diagnostic join between runtime
  spine and session execution snapshot
- or, if that join is still too awkward, make the hidden runtime handle more
  explicit before attempting peer/tools projection

## 2026-04-09 — Slice 5: first joined Meerkat snapshot across real owners

Goal:

- build the first single read path that sees both halves of current
  `MeerkatMachine` truth
- keep the join at an honest integration point instead of pretending
  `meerkat-runtime` owns turn execution
- verify the resulting surface against real runtime-backed session creation

What landed:

- `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` is now
  hidden-public scaffolding instead of crate-private only
- `meerkat-runtime` now hidden-reexports the runtime-side Meerkat snapshot
  types so another crate can compose them without copying state
- new internal facade module:
  - `meerkat/src/meerkat_machine.rs`
- new internal helper:
  - `capture_meerkat_machine_snapshot(...)`
- new internal joined snapshot:
  - `MeerkatMachineSnapshot { spine, turn }`
- new internal execution-source trait covering both:
  - `EphemeralSessionService`
  - `PersistentSessionService`

Current scope of the joined snapshot:

- runtime-owned regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- session/core-owned region:
  - `turn`

Why this slice matters:

- it is the first place in real code where the current Meerkat split can be
  observed as one machine-shaped surface
- it proves that the correct join point is the facade/integration layer, not
  `meerkat-runtime` itself
- it makes an important current state explicit: runtime registration can exist
  before a live session task is attached, so `turn = None` is sometimes honest
  rather than erroneous

Verification:

- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features session-store,memory-store capture_meerkat_machine_snapshot`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`

Result:

- passed

Backtracks encountered:

- the first instinct was to put the joined snapshot helper into
  `meerkat-runtime`; that would have pulled the turn/execution seam back under
  the runtime crate and repeated the ownership mistake from slice 4
- the session-side execution source needed a `'static` builder bound because
  the live session task keeps the builder behind async task ownership; the
  narrower bound compiled in the small but not in the joined helper

Current MeerkatMachine picture after slice 5:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected region:
  - `turn`
- facade-level joined read path:
  - `MeerkatMachineSnapshot`

Still deliberately out of scope:

- no peer ingress region projection yet
- no tool-surface region projection yet
- no structural replacement of `RuntimeSessionEntry`
- no reducer or transition collapse; this is still observational scaffolding

Next likely slice:

- inspect `Peer Ingress` versus `External Tool Surface` and pick the smaller
  honest projection target for the next Meerkat region
- keep that slice runtime-first only if the real owner truly lives there;
  otherwise add another cross-crate join deliberately

## 2026-04-09 — Slice 6: tool-scope visibility joins the Meerkat view

Goal:

- extend the joined Meerkat snapshot with another real Meerkat region that
  already has explicit owner state
- avoid forcing a dishonest peer-ingress snapshot while the classified inbox
  still lives in transient channels
- keep the new slice grounded in real functions used by the live agent loop

What landed:

- `ToolScopeSnapshot` in `meerkat-core/src/tool_scope.rs`
- `ToolScope::snapshot()` in `meerkat-core/src/tool_scope.rs`
- `Agent::tool_scope_snapshot()` in `meerkat-core/src/agent/runner.rs`
- `SessionAgent::tool_scope_snapshot()` default hook plus session-task command
  path in `meerkat-session/src/ephemeral.rs`
- `PersistentSessionService::tool_scope_snapshot(...)` forwarding in
  `meerkat-session/src/persistent.rs`
- `FactoryAgent` forwarding in `meerkat/src/service_factory.rs`
- `MeerkatMachineSnapshot` now includes `tools` in
  `meerkat/src/meerkat_machine.rs`

Current scope of the tool-scope snapshot:

- known base tool names
- currently visible tool names
- base filter
- active external filter
- active turn overlay (allow/deny)
- active revision
- staged external filter
- staged revision

Why this slice matters:

- it covers a real subset of the Meerkat external-tool surface using state that
  already has one owner: `ToolScope`
- it maps directly to the live execution functions that matter:
  - `Agent::stage_external_tool_filter(...)`
  - `ToolScope::apply_staged(...)`
  - provider-visible tool selection at CallingLlm boundaries
- it expands the joined Meerkat read path without inventing another shell-side
  cache or projection

Verification:

- `cargo test -p meerkat-core --lib snapshot_reflects_active_and_staged_scope_state`
- `cargo test -p meerkat-session --test regression_lifecycle tool_scope_snapshot_returns_live_agent_tool_scope_state`
- `cargo test -p meerkat --lib factory_agent_tool_scope_snapshot_forwards_core_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib --features session-store,memory-store capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-session --test regression_lifecycle execution_snapshot_returns_live_agent_execution_state`

Result:

- passed

Backtracks encountered:

- `Peer Ingress` looked like the next semantic region, but the current
  classified inbox still keeps queued ingress truth inside transient channel
  buffers. A non-destructive snapshot there would have required either
  shadow state or an owner refactor first
- that made `External Tool Surface` the smaller honest slice, but only its
  current `ToolScope` subregion, not the full future router/surface kernel

Current MeerkatMachine picture after slice 6:

- runtime-owned projected regions:
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- core/session-owned projected regions:
  - `turn`
  - `tools` (current tool-scope visibility subregion)
- facade-level joined read path:
  - `MeerkatMachineSnapshot { spine, turn, tools }`

Still deliberately out of scope:

- no explicit peer-ingress region projection yet
- no full external-tool router/surface state beyond `ToolScope`
- no structural replacement of `RuntimeSessionEntry`
- no reducer or transition collapse

Next likely slice:

- either refactor classified inbox ownership so peer-ingress queue truth becomes
  explicitly snapshotable
- or continue filling Meerkat from the core side by mapping another explicit
  owner region before attempting the peer refactor

## 2026-04-09 — Slice 7: peer ingress queue becomes explicit owner state

Goal:

- take the next honest Meerkat peer-ingress slice without fabricating a shadow
  queue model
- move classified ingress queue truth out of transient receiver state and into
  an explicit owned structure
- expose that queue through a hidden cross-crate diagnostic seam before trying
  to collapse broader peer semantics

What landed:

- `meerkat-comms/src/inbox.rs`
  - replaced the classified `mpsc::Receiver` storage with an explicit bounded
    `ClassifiedInboxQueue`
  - kept raw inbox channel behavior unchanged
  - added non-destructive `classified_snapshot()`
  - added explicit closed-state handling for classified senders
- `meerkat-core/src/interaction.rs`
  - added `PeerIngressKind`
  - added `PeerIngressEntrySnapshot`
  - added `PeerIngressQueueSnapshot`
- `meerkat-core/src/agent.rs`
  - added hidden `CommsRuntime::peer_ingress_queue_snapshot()` capability
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - implemented `peer_ingress_queue_snapshot()` over the real inbox owner
- `meerkat/src/meerkat_machine.rs`
  - `MeerkatMachineSnapshot` now includes optional `peer` state
  - the join path queries the live comms runtime via the trait seam rather than
    reaching into `meerkat-comms` directly

Why this slice matters:

- it is the first honest peer-ingress MeerkatMachine slice: queue truth now has
  a concrete owner that can be observed without draining it
- it avoids downcasting or concrete-crate reach-through by extending the core
  trait contract instead
- it exposes an important hidden semantic that the old channel-based shape had
  been carrying implicitly: classified ingress also needs explicit closed-state
  semantics once queue ownership becomes real

Verification:

- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`

Result:

- passed

Backtracks encountered:

- the first design nearly exposed peer state by downcasting concrete
  `meerkat_comms::CommsRuntime`; that would have violated the trait-owned
  architecture, so the snapshot became a hidden `meerkat-core` capability
- replacing the classified channel with an explicit queue removed the receiver's
  implicit closed semantics; an explicit `closed` flag had to be added to avoid
  regressing `send_classified()` behavior after `Inbox` drop
- this slice intentionally stopped at queued ingress state; it does not yet
  claim to model trusted-peer sets, peer-directory reachability, or the full
  `PeerCommsAuthority` surface

Current shape of the joined Meerkat snapshot:

- `spine`
  - `binding`
  - `control`
  - `inputs`
  - `ops`
  - `drain`
- `turn`
- `tools`
- `peer`
  - queued classified ingress only

Next likely slice:

- decide whether to grow peer ingress further through trusted-peer / routing
  truth, or switch to a different Meerkat region first if the next peer step
  would force too much semantic collapse
- if peer stays next, map the current `PeerCommsAuthority` fields against the
  new queue snapshot so we can see exactly what is still implicit versus now
  explicit

## 2026-04-09 — Slice 8: trust membership joins the peer snapshot

Goal:

- extend the new peer-ingress queue slice with the smallest additional facts
  that already have one clear owner in live code
- keep peer-directory reachability and transport health out of the kernel until
  we explicitly decide they belong there
- verify the joined Meerkat snapshot against a real per-session comms runtime

What landed:

- `meerkat-core/src/interaction.rs`
  - added `PeerIngressRuntimeSnapshot`
- `meerkat-core/src/agent.rs`
  - added hidden `CommsRuntime::peer_ingress_runtime_snapshot()` capability
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - implemented the runtime snapshot from:
    - local `public_key`
    - `require_peer_auth`
    - shared `trusted_peers`
    - existing classified queue snapshot
- `meerkat/src/meerkat_machine.rs`
  - `MeerkatMachineSnapshot.peer` now carries the runtime-level peer snapshot
    instead of queue data alone
  - added a positive integration test with a real runtime-backed session using
    `build.comms_name`
- updated Meerkat research docs to reflect:
  - `peer.self_peer_id`
  - `peer.auth_required`
  - `peer.trusted_peers`

Why this slice matters:

- it brings the first non-queue peer facts into MeerkatMachine using live owner
  state rather than projection folklore
- it proves the Meerkat facade can join runtime peer state through the existing
  `SessionService -> CommsRuntime` seam without reaching into concrete crates
- it draws a firmer line around what is still outside the kernel:
  peer-directory reachability and transport liveness remain perimeter concerns

Verification:

- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot`
- `cargo test -p meerkat --lib`

Result:

- passed

Backtracks encountered:

- the first version considered using `PeerDirectoryEntry` as the trust snapshot,
  but that would have silently imported reachability and directory synthesis
  into the Meerkat kernel. The final shape uses `TrustedPeerSpec` instead.
- this slice deliberately does not assert that queued peer work originates from
  trusted peers; plain-event ingress remains part of the same queue surface and
  must stay representable in the snapshot

Current Meerkat peer snapshot shape:

- `self_peer_id`
- `auth_required`
- `trusted_peers`
- `queue`
  - queued classified ingress only

Next likely slice:

- compare the new peer snapshot against `PeerCommsAuthority` and decide whether
  the next honest step is:
  - lifting more of raw-item lineage into an explicit owner snapshot, or
  - pausing peer work and moving to another Meerkat region before peer starts
    pulling too much transport semantics into the kernel

## 2026-04-09 — Slice 9: explicit completion-waiter carrier in the Meerkat spine

Goal:

- make the hidden completion-waiter carrier visible in the Meerkat runtime
  spine without promoting it into canonical machine truth
- verify tricky admission cases against the real runtime behavior:
  in-flight deduplication and hard runtime reset
- keep the joined facade snapshot aligned with the runtime-owned carrier

What landed:

- `meerkat-runtime/src/completion.rs`
  - added `CompletionRegistrySnapshot`
  - added `CompletionWaiterEntrySnapshot`
  - added `CompletionRegistry::diagnostic_snapshot()`
- `meerkat-runtime/src/meerkat_machine.rs`
  - added `MeerkatCompletionWaitersSnapshot`
  - added `MeerkatCompletionWaiterSnapshot`
  - `MeerkatMachineSpineSnapshot` now includes `completion_waiters`
- `meerkat-runtime/src/session_adapter.rs`
  - `RuntimeSessionAdapter::meerkat_machine_spine_snapshot(...)` now projects
    the live completion registry carrier
  - added tests covering:
    - idle runtime with no waiters
    - one queued prompt with one waiter
    - deduplicated in-flight admission joining a second waiter to one input id
    - `reset_runtime()` clearing the carrier and terminating the waiter
- `meerkat/src/meerkat_machine.rs`
  - joined facade snapshot test now asserts the projected waiter carrier
- `meerkat-owned-facts-ledger.md`
  - documented the completion registry snapshot explicitly as a supporting
    carrier rather than a new semantic owner

Why this slice matters:

- completion handles are not semantic runtime truth, but they sit on a
  historically bug-prone seam between admission, terminalization, and control
  actions like reset
- making the carrier explicit gives the Meerkat refactor a way to verify that
  this plumbing continues to track the real input lifecycle instead of becoming
  another folklore surface
- the tests deliberately exercise non-trivial behavior rather than just
  structural snapshot shape

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- passed

Backtracks encountered:

- the first test patch targeted the wrong context in `session_adapter.rs`; the
  fix was to patch smaller sections rather than forcing a broad edit
- this slice intentionally does not redefine completion waiters as a machine
  subregion. They remain supporting carrier state refined from canonical input
  terminalization

Next likely slice:

- choose the next Meerkat region that can be made explicit without inventing a
  new owner:
  - turn-side barrier and pending-op posture, if the current execution snapshot
    already exposes it honestly
  - or tool-surface mutation lineage, if that surface is currently better owned
    than the remaining peer authority path

## 2026-04-09 — Slice 10: external tool-surface lineage joins the Meerkat view

Goal:

- make the external tool-surface machine observable through the same Meerkat
  diagnostic path as runtime spine, turn, tools, and peer ingress
- keep the snapshot on the real dispatcher/router authority path instead of
  reaching around it with crate-specific downcasts
- verify the slice from producer authority through session, factory, and facade
  joins

What landed:

- new core diagnostic types in `meerkat-core/src/tool_scope.rs`:
  - `ExternalToolSurfaceGlobalPhase`
  - `ExternalToolSurfaceEntrySnapshot`
  - `ExternalToolSurfaceSnapshot`
- hidden dispatcher capability:
  - `AgentToolDispatcher::external_tool_surface_snapshot()`
- forwarding through the tool composition chain:
  - `FilteredToolDispatcher`
  - `ToolGateway`
  - `DynamicToolComposite`
  - `ToolDispatcher`
  - `FilteredDispatcher`
  - `CompositeDispatcher`
- live MCP authority snapshot:
  - `ExternalToolSurfaceAuthority::diagnostic_snapshot()`
  - `McpRouter::external_tool_surface_snapshot()`
  - `McpRouterAdapter` cache plus fallback support
- session, factory, and Meerkat joins:
  - `AgentRunner::external_tool_surface_snapshot()`
  - `SessionAgent::external_tool_surface_snapshot()`
  - session-service query path
  - `FactoryAgent` forwarding
  - `capture_meerkat_machine_snapshot(...)` now includes `tool_surface`

Why this slice matters:

- it pulls one of the higher-risk Meerkat mutation domains into the joined
  machine view without inventing a second owner
- it verifies that the production builder path preserves the same diagnostic
  truth as the lower-level session and MCP paths
- it makes external tool mutation lineage and snapshot-alignment state visible
  for later Meerkat kernel work and TLA mapping

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mcp --lib`
- `cargo test -p meerkat-session --test regression_lifecycle`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features mcp`

Result:

- passed

Backtracks encountered:

- the first session-level expectation used `snapshot_epoch = 1` for a purely
  staged `StageAdd` surface; that was wrong against the current authority and
  had to be corrected back to `0`
- the first Meerkat integration idea tried to place the join inside
  `meerkat-runtime`; that would have recreated a false owner for tool-surface
  truth, so the final join stays in the facade path
- the MCP-backed Meerkat test had to be `mcp`-gated; the non-`mcp` build still
  needs to stay green because the abstract Meerkat surface exists even when no
  live MCP subsystem is compiled in

Current limitation:

- `ToolGateway::external_tool_surface_snapshot()` returns the first dispatcher
  that exposes surface state. That matches the current single live external
  surface authority assumption, but it would need redesign if Meerkat ever
  hosts multiple independent external-tool lifecycle authorities at once

Next likely step:

- return to the remaining Meerkat peer-ingress gap and decide whether the live
  peer authority can be made as explicit as the tool-surface authority
- if peer remains too entangled with transient transport buffers, pivot to a
  different Meerkat region before attempting more peer collapse

## 2026-04-09 — Slice 11: peer ingress gains stable raw identity and stored correlation

Goal:

- tighten the peer-ingress slice without forcing `PeerCommsAuthority` onto the
  live hot path before the owner mapping is honest
- carry stable ingress-time identity and correlation through the queued peer
  snapshot instead of dropping them at the queue boundary
- align ingress-prepared text projection with the live drain path so stored
  projection can become real runtime data rather than test-only scaffolding

What landed:

- `meerkat-comms/src/lib.rs`
  - `peer_comms_authority` is now explicitly compiled into the crate instead of
    existing as effectively dormant code
- `meerkat-comms/src/classify.rs`
  - `PreparedIngressItem` now provides the coarse `PeerIngressKind` used by the
    queued snapshot
  - message ingress projection now preserves the message body as the canonical
    prompt projection, even when multimodal blocks are present
  - legacy `classify()` scaffolding is test-only; live code now goes through
    `prepare(...)`
- `meerkat-comms/src/inbox.rs`
  - classified queue entries now retain:
    - `raw_item_id`
    - `kind`
    - `request_id`
    - stored `text_projection`
  - non-destructive peer queue snapshots now expose:
    - stable raw ingress identity
    - stored request/reply correlation
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - classified drain now uses the ingress-stored text projection instead of
    recomputing request/response/plain-event prompt text at drain time
  - added assertions that snapshot raw ids stay stable for plain events
- `meerkat-core/src/interaction.rs`
  - `PeerIngressEntrySnapshot` now includes `raw_item_id` and `request_id`

Why this slice matters:

- it makes the peer-ingress queue a better match for the Meerkat owned-facts
  ledger: stable raw item identity and request correlation now survive the
  queue boundary instead of being implicit in lower layers
- it strengthens the eventual authority handoff shape without pretending the
  authority already executes the live transitions
- it reduces one specific class of drift: ingress-classified request/response
  text no longer has to be recomputed later from the raw envelope

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat-comms --lib`

Result:

- passed

Backtracks encountered:

- the first version changed multimodal message projection by blindly using the
  generic `CommsMessage::to_user_message_text()` result at drain time; that
  regressed the current runtime contract, which treats the message body as the
  canonical prompt projection for peer messages with blocks
- the fix was to move the semantic choice earlier: ingress preparation now
  computes message projection in the same shape the live drain already expects,
  and only then does the drain path consume the stored projection
- the first `InboxSender::send_classified()` patch also borrowed the prepared
  ingress descriptor after partially moving it into the queue entry; the fix
  was to compute the coarse ingress kind before consuming the prepared item

Current limitation:

- `PeerCommsAuthority` is now compiled, aligned, and tested, but it is still
  not the live owner of peer ingress transitions
- the queued peer snapshot now carries more of the right facts, but the hot
  path still updates the classified queue directly rather than driving the
  authority state first and projecting from it

Next likely step:

- decide whether the next honest peer slice is:
  - routing live queue admission through `PeerCommsAuthority`, or
  - stopping peer again and filling another Meerkat region before the peer
    integration step becomes too invasive for one slice

## 2026-04-09 — Slice 12: peer authority becomes reusable across session lifetime

Goal:

- critically review whether `PeerCommsAuthority` is actually fit to become a
  live Meerkat owner instead of assuming the existing model is already sound
- fix the authority's phase semantics so a long-lived session can accept more
  than one batch of peer work
- keep the fix narrow: improve the machine model first, without pretending the
  live runtime has already delegated ingress to it

What landed:

- `meerkat-comms/src/peer_comms_authority.rs`
  - `TrustPeer` now acts as a self-loop from any phase instead of resetting the
    machine to `Absent`
  - `ReceivePeerEnvelope` now accepts new ingress from `Dropped` and
    `Delivered`, so the authority can model repeated batches inside one live
    session
  - untrusted ingress no longer poisons a non-empty queue: if queued trusted
    work already exists, the machine stays in `Received` while still recording
    `trusted_snapshot = false` for the dropped raw item
  - added new tests covering:
    - trust mutation from `Dropped` and `Delivered`
    - resuming from `Dropped`
    - starting a new batch from `Delivered`
    - untrusted ingress while trusted queued work is still pending
    - accepting future work after a fully delivered batch

Why this slice matters:

- the earlier authority model was effectively single-use; once it reached
  `Delivered` or `Dropped`, it could not model further ingress for the same
  runtime
- that made it unfit as a serious candidate for the live Meerkat owner, even
  before any wiring concerns
- fixing the model first is the right backtrack: it lets the runtime
  integration step be about ownership and mapping, not about patching a machine
  that fundamentally could not represent a long-lived session

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib peer_comms_authority`
- `cargo test -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the original review target was live peer authority integration, but the code
  inspection showed that would have been premature because the authority could
  not represent a steady-state runtime after `Delivered` or `Dropped`
- this slice therefore intentionally stopped at the model level; it does not
  yet route live `send_classified()` or drain through the authority

Current limitation:

- `PeerCommsAuthority` is now session-reusable, but live trust membership and
  live queue ownership are still maintained outside it
- the next integration step must decide how trust changes and queued work stay
  synchronized without inventing a shadow authority

Next likely step:

- either add the smallest honest live synchronization path between trusted-peer
  mutations, ingress preparation, and `PeerCommsAuthority`
- or pause peer again and take another Meerkat region if that synchronization
  step is still too broad for one slice

## 2026-04-09 — Slice 13: peer authority learns auth-open admission and trust removal

Goal:

- align `PeerCommsAuthority` with real runtime comms behavior before giving it
  any more live ownership
- make the machine capable of expressing both supported peer-admission modes:
  - `require_peer_auth = true`
  - `require_peer_auth = false`
- add the missing trust-removal semantic so future live synchronization does
  not need to smuggle revocation around the authority

What landed:

- `meerkat-comms/src/peer_comms_authority.rs`
  - added machine-owned `auth_required`
  - added `PeerCommsAuthority::new_with_auth_required(...)`
  - added `RemoveTrustedPeer`
  - `ReceivePeerEnvelope` now admits untrusted peers when auth is open, while
    still recording `trusted_snapshot = false`
  - trust removal now revokes future trusted admission without disturbing
    already queued work
- `meerkat-comms/src/classify.rs`
  - added a round-trip test proving that auth-open runtime ingress preparation
    and auth-open authority admission agree on an unknown peer request

Why this slice matters:

- before this change, the authority could only model the strict-auth runtime.
  That meant it could not become a real Meerkat owner for sessions configured
  with `require_peer_auth = false`
- this is exactly the kind of mismatch that creates shadow logic: the live
  runtime would have to special-case admission outside the authority
- fixing the model first keeps the next live synchronization step honest

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib peer_comms_authority`
- `cargo test -p meerkat-comms --lib classify`
- `cargo test -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the original intent for this turn was to start live synchronization between
  trust state, queue state, and the authority
- code inspection showed that would still be premature because the authority
  could not express auth-open runtime semantics or trust revocation
- this slice therefore stayed at the model boundary on purpose

Current limitation:

- the authority now matches more of the live runtime semantics, but the runtime
  still owns trust mutation and classified queue state separately
- plain external events remain outside the authority model
- the Meerkat joined snapshot scaffolding still emits dead-code warnings in
  `meerkat/src/meerkat_machine.rs` because it is diagnostic infrastructure, not
  production wiring yet

Next likely step:

- wire one minimal live synchronization path for trusted-peer mutation into the
  peer authority, or
- step sideways and fill another Meerkat region if peer integration still
  needs one more model cleanup pass first

## 2026-04-09 — Slice 14: live peer trust and external ingress synchronize through one runtime seam

Goal:

- stop treating router-local trust mutation as if it were canonical Meerkat
  peer state
- wire one minimal live synchronization path between:
  - trusted-peer mutation
  - external peer envelope receive
  - typed peer dequeue/drain
  - `PeerCommsAuthority`
- keep the slice narrow enough that plain events and unrelated comms transport
  details stay outside the authority for now

What landed:

- `meerkat-comms/src/classify.rs`
  - `prepare(...)` now always prepares external peer envelopes and records
    `trusted_sender`, instead of dropping strict-auth unknown peers before the
    authority ever sees them
  - test-only `classify(...)` still preserves the old strict-auth drop
    semantics for focused classification tests
- `meerkat-comms/src/inbox.rs`
  - classified queue receive now asks `PeerCommsAuthority` whether an external
    envelope should be admitted
  - dequeue/drain now advances authority submission state in lockstep with the
    queue
  - trusted-peer add/remove notifications now synchronize into the
    inbox-owned authority
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - internal runtime tests now use the async
    `meerkat_core::agent::CommsRuntime::add_trusted_peer` seam instead of
    mutating `router.add_trusted_peer(...)` directly
  - added live tests for:
    - trust -> receive -> drain on auth-required runtimes
    - unknown-peer receive -> drain on auth-open runtimes
  - `upsert_trusted_peer(...)` is now documented as a legacy router-only helper,
    not an authoritative runtime seam
- `meerkat-comms/src/runtime/comms_bootstrap.rs`
  - child inproc bootstrap now uses the async trusted-peer registration seam
    rather than the router-only helper
- `meerkat/tests/factory_build_agent.rs`
  - shared-runtime sentinel trust setup now uses the same async registration
    seam as production runtime code

Why this slice matters:

- before this change, the peer authority could look correct in isolation while
  live runtime code still mutated trust through a second path
- that is exactly the kind of “owner plus bypass” shape the two-kernel work is
  trying to eliminate
- moving strict-auth unknown-peer drop into the authority path also fixed a
  deeper mismatch: the authority was previously unable to observe some real
  ingress attempts at all

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --test factory_build_agent shared_comms_runtime_skipped_when_comms_name_set --features comms`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first live runtime test failed because strict-auth unknown peers were
  still being dropped in classification before the authority saw them
- fixing that exposed a second bypass: several existing tests still mutated
  trust through `router.add_trusted_peer(...)`
- rather than teaching the authority about those bypasses, the fix was to move
  internal callers onto the async runtime registration seam

Current limitation:

- `upsert_trusted_peer(...)` still exists for legacy callers that only need
  router-visible trust and do not participate in classified ingress
- plain external events still bypass `PeerCommsAuthority`
- the Meerkat joined snapshot scaffolding in `meerkat/src/meerkat_machine.rs`
  still emits dead-code warnings because it is diagnostic infrastructure, not
  production wiring yet

Next likely step:

- decide whether the next honest Meerkat slice is:
  - promoting the async trusted-peer registration seam as the only supported
    runtime mutation path, or
  - stepping sideways into another Meerkat region before returning to peer
    ingress again

## 2026-04-09 — Slice 15: canonical runtime trust registration becomes a named Meerkat seam

Goal:

- stop making canonical peer-trust mutation depend on trait-call syntax or
  router writes
- give `MeerkatMachine` an explicit runtime-level trust registration seam that
  keeps peer discovery, peer authority, and classified ingress aligned
- move the real runtime-facing consumers onto that named seam

What landed:

- `meerkat-comms/src/runtime/comms_runtime.rs`
  - added `CommsRuntime::register_trusted_peer(...)`
  - added `CommsRuntime::unregister_trusted_peer(...)`
  - added `CommsRuntime::unregister_trusted_pubkey(...)`
  - the trait-level `add_trusted_peer` / `remove_trusted_peer` path now
    delegates to the same runtime implementation
  - added a focused test proving that `register_trusted_peer(...)`:
    - preserves `PeerMeta`
    - synchronizes inbox-owned peer authority
    - removes cleanly through the paired unregister seam
- `meerkat-comms/src/runtime/comms_bootstrap.rs`
  - child inproc bootstrap now uses the named runtime registration method
- `meerkat/tests/factory_build_agent.rs`
  - shared-runtime sentinel trust setup now uses the same named runtime seam
- `examples/035-mdm-tux-rs/src/bin/target.rs`
  - target runtime add/remove trust paths now use canonical runtime methods
    instead of `upsert_trusted_peer(...)` and raw router removal

Why this slice matters:

- the previous slice established that raw router mutation was a bypass
- this slice turns the fix into a stable API surface instead of leaving the
  “right path” implicit in trait calls
- it also closes the remove-path drift, not just the add-path drift

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib`
- `cargo test -p meerkat --test factory_build_agent shared_comms_runtime_skipped_when_comms_name_set --features comms`
- `cargo check --manifest-path examples/035-mdm-tux-rs/Cargo.toml --bin mdm-target`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first peer-authority fix still left runtime callers choosing between the
  trait seam, the sync helper, and raw router mutation
- that was too easy to misuse, so this slice added explicit runtime methods and
  migrated the meaningful callers onto them

Current limitation:

- `upsert_trusted_peer(...)` still exists as a legacy router-only helper
- standalone example code in `examples/035-mdm-tux-rs/src/lib.rs` and
  `examples/035-mdm-tux-rs/src/bin/tux.rs` still mutates raw router state
  because it is not using `CommsRuntime`'s classified-ingress ownership path
- Meerkat snapshot scaffolding in `meerkat/src/meerkat_machine.rs` still emits
  dead-code warnings because it is diagnostic infrastructure, not production
  wiring yet

Next likely step:

- decide whether to retire or hard-deprecate `upsert_trusted_peer(...)`, or
- step sideways into the next Meerkat region instead of continuing to deepen
  peer ingress in one uninterrupted run

## 2026-04-09 — Slice 16: peer authority phase enters the live Meerkat snapshot

Goal:

- expose one more honest owner-side fact from the peer ingress region
- distinguish:
  - queued classified work
  - authority-owned peer submission backlog
  - dropped / delivered / absent authority states
- verify that the joined `MeerkatMachine` view can now see this difference
  without inventing a new injection seam

What landed:

- `meerkat-core/src/interaction.rs`
  - added `PeerIngressAuthorityPhase`
  - `PeerIngressRuntimeSnapshot` now carries:
    - `authority_phase`
    - `submission_queue_len`
- `meerkat-comms/src/inbox.rs`
  - added a non-destructive runtime peer snapshot path that captures:
    - classified queue projection
    - authority phase
    - authority submission queue length
  - this happens under one lock so the snapshot is not stitched from two
    different moments
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - `peer_ingress_runtime_snapshot()` now maps internal
    `PeerIngressState -> PeerIngressAuthorityPhase`
  - added focused tests proving:
    - plain events can leave `authority_phase = Absent` while the classified
      queue is non-empty
    - dropped peer ingress is visible through the runtime snapshot even when
      the queue remains empty
- `meerkat/src/meerkat_machine.rs`
  - joined Meerkat snapshot test now verifies that live peer runtime state
    carries the new authority-phase facts across the cross-crate join

Why this slice matters:

- the previous snapshot could show queue contents but could not explain whether
  the peer owner considered ingress absent, dropped, received, or delivered
- that meant one of the actual machine states was still hidden behind helper
  tests instead of being visible through the real runtime seam
- `submission_queue_len` also makes it explicit that the authority-owned peer
  backlog is not the same thing as the broader classified queue

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_trusted_peers_and_queue`
- `cargo test -p meerkat-comms --lib test_peer_ingress_runtime_snapshot_reflects_dropped_peer_authority_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_live_peer_runtime_state --features comms`
- `cargo check -p meerkat --lib --features comms`

Result:

- passed

Backtracks encountered:

- the first joined Meerkat test idea tried to manufacture a dropped peer
  ingress state through the session-level comms trait object
- that would have required a new helper seam or a downcast path, which was too
  much surface for this slice
- the fix was to keep dropped-state verification at the comms runtime owner
  level and only assert phase propagation in the joined Meerkat snapshot

Current limitation:

- plain events still bypass `PeerCommsAuthority`, so
  `authority_phase = Absent` with a non-empty classified queue remains a real
  and expected state
- the joined Meerkat snapshot still emits dead-code warnings because it is
  diagnostic infrastructure, not production wiring yet

Next likely step:

- either surface another owner-side peer fact, such as trusted/drop lineage for
  the last classified raw item, or
- step sideways into a different Meerkat region before continuing to deepen the
  peer ingress surface

## 2026-04-09 — Slice 17: executable joined-snapshot invariants

Goal:

- stop treating the joined `MeerkatMachine` snapshot as pure observation
- turn the first safe cross-region invariants into executable validation
- verify those invariants against the live runtime, turn, tool, and peer joins
  before adding more surface area

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `MeerkatMachineInvariantViolation`
  - added `validate_meerkat_machine_snapshot(...)`
  - the first validator pass checks:
    - `Running => current_run_id # None`
    - `current_run_id # None => control.phase \in {Running, Retired}`
    - control/input current-run alignment
    - turn/control run alignment for non-ready, non-terminal turn phases
    - `WaitingForOps => pending_operation_ids # None`
    - peer authority phase vs submission queue coherence
    - tool-surface visibility/base-state coherence
    - `active_ops <= total_ops`
  - existing joined snapshot tests now assert that live captured snapshots pass
    validation
  - added a focused negative test that mutates a captured live snapshot and
    proves the validator reports the expected cross-region violations
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - updated the candidate invariants to match the real control authority:
    `Retired` may legitimately retain `current_run_id` during drain
  - recorded which invariants are now executable in code

Why this slice matters:

- it changes the Meerkat work from "we can see more state" to
  "we can check whether the joined state is coherent"
- this is the first place where the research docs now constrain the live code
  bidirectionally: the docs propose invariants, and the code proves or rejects
  them
- it also flushed out one important false assumption early: `Retired` is not a
  no-run state if a drain is still completing an in-flight run

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first validator draft used `control.phase = Running <=> current_run_id # None`
- live authority review showed that is wrong: `Retired` can retain
  `current_run_id` while a run drains after `RetireRequested`
- the validator and sketch were tightened to reflect the actual reducer rather
  than an over-simplified architecture story

Current limitation:

- the validator is still intentionally conservative; it does not yet check:
  - barrier satisfaction against concrete ops terminality
  - completion-waiter alignment against input terminalization
  - recovery-specific binding/cursor invariants
- it remains diagnostic infrastructure in `meerkat/src/meerkat_machine.rs`, so
  the joined snapshot module still carries dead-code warnings in non-test lib
  builds

Next likely step:

- deepen the validator with one more region that already has clear live owner
  truth, probably tool-surface visibility/base-state coherence or
  barrier/ops coupling, before opening `MobMachine`

## 2026-04-09 — Slice 18: runtime-spine input-ledger coherence

Goal:

- deepen the executable Meerkat validator without depending on a non-atomic
  cross-crate join
- validate the runtime-spine input ledger against queue membership, current-run
  contributors, and completion-waiter references
- keep the slice anchored in one authority-owned snapshot so failures represent
  real Meerkat mismatches instead of join timing noise

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - every `queue` and `steer_queue` item exists in the admitted-input ledger
    - queued items have lifecycle `Queued`
    - every `current_run_contributors` item exists in the admitted-input ledger
    - contributor lifecycles stay within
      `{Staged, Applied, AppliedPendingConsumption}`
    - completion-waiter `input_count` matches `waiting_inputs.len()`
    - completion-waiter `waiter_count` matches the summed entry counts
    - every waiting completion input still exists in the admitted-input ledger
  - expanded the synthetic negative test so it now proves these runtime-spine
    violations are detected alongside the earlier cross-region ones
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the new executable input-ledger invariants

Why this slice matters:

- it moves more of `MeerkatMachine` validation into the runtime spine, where
  the snapshot is substantially more coherent than the cross-crate turn/runtime
  join
- it verifies the current admitted-input ledger is strong enough to act as the
  anchor for queue truth, contributor truth, and waiter plumbing references
- it keeps the completion carrier explicit without promoting waiter counts into
  semantic truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first candidate follow-up was `turn.barrier_satisfied => AllTerminal(...)`
  against the ops snapshot
- that looks right in the abstract sketch, but the current joined snapshot is
  not atomic across crates: runtime ops are read before the later turn
  snapshot, so a legitimate barrier-satisfaction transition can race and
  manufacture a false mismatch
- the fix was to pivot to runtime-spine invariants that live inside one
  authority-owned snapshot and postpone barrier/ops validation until the join
  is tighter or the validator can account for the sampling boundary explicitly

Current limitation:

- barrier/ops coupling is still documented but not executable in the joined
  validator yet, for the non-atomic snapshot reason above
- completion waiters are only validated structurally; the validator still does
  not assert terminal/non-terminal lifecycle alignment because the waiter and
  ingress snapshots are captured under different locks

Next likely step:

- either tighten the joined Meerkat snapshot semantics enough to support safe
  barrier/ops validation, or keep extending the runtime-spine validator with
  another same-owner invariant before moving to `MobMachine`

## 2026-04-09 — Slice 19: ingress lifecycle and cohort coherence

Goal:

- keep strengthening `MeerkatMachine` inside the runtime-owned ingress spine
- validate that current-run/cohort shape and terminal-outcome shape remain
  coherent with the live ingress authority
- avoid widening the validator into a cross-crate race when the runtime spine
  still offers cleaner truth

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - `inputs.current_run_id = None <=> current_run_contributors = << >>`
    - every terminal admitted input carries a terminal outcome
    - non-terminal admitted inputs do not carry terminal outcomes
  - added a focused negative test proving:
    - contributors without a current run are rejected
    - illegal contributor lifecycle is rejected
    - terminal lifecycle without terminal outcome is rejected
    - non-terminal lifecycle with terminal outcome is rejected
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the new executable ingress-lifecycle invariants

Why this slice matters:

- it tightens the admitted-input ledger into a more trustworthy Meerkat anchor
- it proves that the current ingress authority already carries enough shape to
  validate both cohort structure and terminal bookkeeping without touching
  higher-level surfaces
- it keeps the work inside one owner domain, which is exactly the discipline we
  want before we start opening `MobMachine`

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_runtime_spine_lifecycle_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- I considered jumping from here into deeper tool-surface invariants or back to
  barrier/ops coupling
- the runtime ingress authority offered a cleaner next truth surface, so I kept
  the slice there instead of mixing in another cross-boundary concern too soon

Current limitation:

- `current_run_id Some => contributors non-empty` is now executable, but the
  validator still does not reason about the *contents* of `current_run_id`
  beyond matching control and ingress
- barrier/ops coupling remains documented but intentionally non-executable in
  the joined validator until the snapshot semantics are tighter

Next likely step:

- extend the tool-surface portion of the validator with one more same-owner MCP
  invariant, or
- start designing a tighter joined-snapshot coherence boundary if barrier/ops
  validation is the higher priority before `MobMachine`

## 2026-04-09 — Slice 20: richer tool-surface authority validation

Goal:

- expand the `MeerkatMachine` validator using same-owner MCP authority facts
- validate more of the external tool-surface machine than just
  visible/base-state membership
- keep the slice fully within the tool-surface snapshot so it remains free of
  cross-crate join races

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added validator checks that:
    - pending op is compatible with base state
    - inflight call counts only exist on `Active | Removing`
    - forced delta phase implies remove delta operation
    - staged intent sequence exists iff staged op is not `None`
    - pending task/lineage sequences exist iff pending op is not `None`
  - added a focused negative test proving each of those richer tool-surface
    invariants is detected
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded the richer tool-surface invariants in both the candidate list and
    the executable-subset list

Why this slice matters:

- it turns the tool-surface region into a substantially more trustworthy part
  of the Meerkat validator, not just a visibility check
- it is pulled directly from the live MCP authority’s own invariant checks, so
  we are validating real codebase truth rather than inventing a new layer
- it strengthens `MeerkatMachine` without widening the still-risky
  runtime/turn/ops sampling boundary

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- none in code, which is a useful signal by itself: the richer invariants held
  cleanly against both synthetic failures and the live staged-MCP snapshot
- that reinforces the choice to deepen same-owner regions before returning to
  more ambiguous cross-region joins

Current limitation:

- tool-surface validation is now strong, but the validator still does not
  account for removal-timing membership because that fact is not exposed
  through the current core snapshot
- barrier/ops coupling remains the main intentionally deferred Meerkat
  invariant family

Next likely step:

- either expose one more MCP-authority fact such as removal-timing membership
  through the snapshot, or
- stop deepening Meerkat’s same-owner regions and begin sketching the first
  narrow `MobMachine` slice once the remaining Meerkat risks are judged low
  enough

## 2026-04-09 — Slice 21: tool-surface removal-timing validation

Goal:

- expose the last obvious same-owner MCP authority fact still missing from the
  joined Meerkat snapshot
- validate that removal timing exists iff a tool surface is in `Removing`
  base state
- keep the slice fully inside the external tool-surface snapshot so it remains
  free of runtime/turn sampling races

What landed:

- `meerkat-core/src/tool_scope.rs`
  - added `has_removal_timing` to
    `ExternalToolSurfaceEntrySnapshot`
- `meerkat-mcp/src/external_tool_surface_authority.rs`
  - threaded live removal-timing membership into the diagnostic snapshot from
    the canonical authority state
- `meerkat/src/meerkat_machine.rs`
  - added `ToolSurfaceRemovalTimingMismatch`
  - validated `has_removal_timing <=> base_state = Removing`
  - expanded the focused negative tool-surface test with a
    `Removing`-without-timing failure case
- `meerkat-session/tests/regression_lifecycle.rs`
  - updated the session-side expected snapshot shape for the added field
- `docs/architecture/two-kernel-research/meerkat-state-machine-sketch.md`
  - recorded removal-timing membership in both the candidate invariant list
    and the currently executable subset

Why this slice matters:

- it closes the last clear MCP same-owner gap in the current joined
  `MeerkatMachine` validator
- it reuses a fact the live MCP authority already enforces internally, so the
  validator remains grounded in real ownership rather than a parallel model
- it raises confidence that the external tool-surface region is genuinely
  ready, allowing the next decision to be about remaining Meerkat cross-region
  joins versus starting the first narrow `MobMachine` slice

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_tool_surface_shape_violations`
- `cargo test -p meerkat --lib --features mcp capture_meerkat_machine_snapshot_joins_live_external_tool_surface_state`
- `cargo test -p meerkat-session --test regression_lifecycle tool_scope_snapshot_returns_live_agent_tool_scope_state`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first session test filter was wrong (`tool_surface_...` instead of the
  existing `tool_scope_...` test name); I corrected the verification lane
  rather than inventing a redundant test
- the slice otherwise held cleanly without code backtracks, which is useful
  signal that this was the right final same-owner MCP invariant to expose

Current limitation:

- the joined `MeerkatMachine` snapshot is still a diagnostic surface, not a
  production-owned API, so the existing dead-code warnings remain expected
- barrier/ops coupling remains intentionally deferred until the runtime/turn
  join semantics are tighter

Next likely step:

- start a first narrow `MobMachine` slice while keeping the deferred
  barrier/ops join explicitly tracked as remaining Meerkat work, or
- if Meerkat stays first for one more slice, focus on tightening runtime/turn
  snapshot coherence rather than adding more same-owner regions

## 2026-04-09 — Slice 22: first MobMachine roster/lifecycle snapshot

Goal:

- open the first real `MobMachine` implementation slice without jumping
  straight into flow/orchestrator complexity
- map live `MobHandle` read paths into one diagnostic snapshot
- validate only roster/lifecycle/member-projection invariants already justified
  by current code

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `MobMachineSnapshot { phase, roster, members }`
  - added `capture_mob_machine_snapshot(&MobHandle)`
  - added a conservative validator for:
    - roster member set vs projected member set
    - structural projection coherence between `RosterEntry` and
      `MobMemberListEntry`
    - `Active` vs `Retiring` status/state coherence
    - `Broken` requiring an error payload
    - member finality matching the current status classification
  - added a synthetic negative validator test
- `meerkat-mob/src/runtime/tests.rs`
  - added a live-handle test that spawns and wires members, captures the new
    snapshot from `MobHandle`, and proves the validator stays clean on the
    current runtime path
- `meerkat-mob/src/lib.rs`
  - wired in the new internal module

Why this slice matters:

- it starts `MobMachine` the same way `MeerkatMachine` started: by exposing a
  diagnostic read surface over real owners instead of inventing a second
  control plane
- it keeps the first Mob scope small enough to review honestly:
  lifecycle state, roster truth, and enriched member status projection
- it creates a concrete integration point for later identity-native migration
  work without pretending the current `MeerkatId`-centric API is already the
  target design

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first synthetic test tried to create a one-way wiring inconsistency
  through the public `Roster` API; that failed because the current projection
  mutators already preserve reciprocity
- that is good architectural signal: some topology invariants are protected
  before the new validator ever runs, so the validator should observe and
  corroborate those guarantees rather than bypassing them for the sake of a
  test fixture

Current limitation:

- the current `MobMachine` slice is still roster/lifecycle only
- flow runs, orchestrator state, kickoff barriers, pending spawns, and
  topology-intent vs realized-wiring gaps are not yet part of the diagnostic
  snapshot
- like `MeerkatMachine`, the new module is still diagnostic scaffolding, so
  the current dead-code warnings are expected

Next likely step:

- extend `MobMachine` with one more owner-backed region, most likely
  lifecycle/orchestrator counters or topology intent, while keeping flow
  execution out until the read surface is honest enough

## 2026-04-09 — Slice 23: Mob machine-state diagnostics

Goal:

- extend `MobMachine` beyond roster/member projection into actual
  machine-owned lifecycle and orchestrator state
- do that through real actor-backed diagnostic reads rather than by re-deriving
  counters from public surfaces
- keep the invariant set conservative and tied to authority semantics already
  encoded in `MobLifecycleAuthority` and `MobOrchestratorAuthority`

What landed:

- `meerkat-mob/src/runtime/mob_lifecycle_authority.rs`
  - added `MobLifecycleSnapshot { phase, active_run_count, cleanup_pending }`
  - added `snapshot()` for diagnostics while preserving canonical ownership
- `meerkat-mob/src/runtime/mob_orchestrator_authority.rs`
  - extended `MobOrchestratorSnapshot` to include `phase`
- `meerkat-mob/src/runtime/state.rs`
  - added a non-test `DiagnosticKernelSnapshot` command
- `meerkat-mob/src/runtime/actor.rs`
  - wired `DiagnosticKernelSnapshot` to return the live lifecycle snapshot and
    optional orchestrator snapshot from the actor-owned authorities
- `meerkat-mob/src/runtime/handle.rs`
  - added internal `diagnostic_kernel_snapshot()`
- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineSnapshot` with `kernel`
  - validated:
    - public phase matches lifecycle phase
    - terminal lifecycle phases have zero active runs
    - cleanup pending appears only in `Stopped | Completed`
    - public phase matches orchestrator phase when orchestrator exists
    - terminal orchestrator phases have zero pending spawns and active flows
    - destroyed orchestrator snapshots are no longer bound or supervisor-active
  - expanded the synthetic negative test with machine-state violations

Why this slice matters:

- it upgrades `MobMachine` from “roster plus projection” to “roster plus real
  canonical machine state”
- it creates a proper bridge between the public mob handle and the actual
  lifecycle/orchestrator authorities without making those authorities public
  control surfaces
- it gives later flow/topology work a grounded place to attach instead of
  forcing everything through `list_members*()` projections

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_stop_clears_pending_spawn_count_and_failed_member_projection`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_pending_spawn_ownership_and_revision`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first compile pass broke existing lifecycle tests by removing the
  `active_run_count()` helper too aggressively; I restored the test-only helper
  instead of rewriting authority tests mid-slice
- adding `phase` to `MobOrchestratorSnapshot` also needed a manual `Default`
  impl because `MobState` does not implement `Default`
- the suspected fresh-create lifecycle/orchestrator/public-phase mismatch did
  not reproduce in the live handle capture lane, which is useful signal that
  the real startup path is currently coherent enough for this validator level

Current limitation:

- `MobMachine` still does not model flow kernel state, kickoff barriers,
  pending-spawn lineage contents, or topology intent vs realized wiring
- the new lifecycle/orchestrator snapshots are still diagnostic scaffolding, so
  their current dead-code warnings are expected just like the Meerkat side

Next likely step:

- extend `MobMachine` with one more honest owner-backed region:
  either topology intent/revision coherence or a narrow flow/orchestrator join
  around active-flow accounting

## Slice 24 - MobMachine topology coherence

Goal:

- add the first topology-owned lane to `MobMachine`
- verify that the live topology owner and the orchestrator authority are not
  silently drifting on spawn and lifecycle transitions
- fix the owner path if the new diagnostic join exposes a real mismatch

What landed:

- `meerkat-mob/src/runtime/topology.rs`
  - added `MobTopologySnapshot { coordinator_bound, revision }`
- `meerkat-mob/src/runtime/flow.rs`
  - exposed topology diagnostics and small owner-preserving topology mutation
    helpers on `FlowEngine`
- `meerkat-mob/src/runtime/mod.rs`
  - extended `MobKernelDiagnosticSnapshot` with topology state
- `meerkat-mob/src/runtime/actor.rs`
  - wired topology snapshot into `DiagnosticKernelSnapshot`
  - synchronized topology revision/binding updates with the same live
    orchestrator-driven transitions that already mutate:
    - `StageSpawn`
    - `CompleteSpawn`
    - `StopOrchestrator`
    - `ResumeOrchestrator`
  - mirrored the same unbind/rebind behavior in destroy/reset paths
- `meerkat-mob/src/mob_machine.rs`
  - added topology-aware validation:
    - topology `coordinator_bound` matches orchestrator `coordinator_bound`
    - topology `revision` matches orchestrator `topology_revision`
  - expanded the synthetic negative test to prove both mismatches are caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live test that captures `MobMachine` snapshots through:
    - initial running mob
    - staged spawn
    - settled spawn
    - stop
    - resume
  - each snapshot must satisfy the topology-aware validator

Why this slice matters:

- it exposed a real architectural pressure point rather than just another read
  model: topology revision already had a live owner (`MobTopologyService`), but
  the orchestrator authority was independently carrying its own revision counter
- instead of picking one by fiat mid-slice, the new diagnostic join makes the
  duplication visible and forces the two paths to stay coherent while the
  longer-term two-kernel collapse continues
- it also gives `MobMachine` its first owner-backed region beyond
  lifecycle/orchestrator/roster that is not just a projection of member status

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_pending_spawn_ownership_and_revision`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first review pass found a real ownership smell: `MobTopologyService`
  existed, but the staged/settled spawn path and stop/resume path were only
  mutating the orchestrator authority, not the topology owner
- I did not paper over that by weakening the validator; I added the topology
  snapshot first, then wired the missing owner-side updates onto the real actor
  transitions so the live test could verify them
- the only non-semantic cleanup after verification was lowering
  `MobTopologyService::snapshot()` to crate visibility so the diagnostic-only
  snapshot type did not leak a `private_interfaces` warning

Current limitation:

- topology intent/revision is now joined, but `MobMachine` still does not model
  flow kernel state itself, kickoff barriers, or the detailed contents of
  pending-spawn lineage
- the topology/orchestrator pairing is still two-owner reality kept coherent by
  live wiring; it is not yet the final collapsed kernel shape

Next likely step:

- extend `MobMachine` with one more owner-backed slice around active-flow
  accounting or flow/orchestrator coherence before touching the larger flow
  kernel surface

## Slice 25 - MobMachine flow-accounting coherence

Goal:

- join actor-owned flow tracker state into the diagnostic `MobMachine` view
- verify that three live carriers are actually aligned at stable actor
  boundaries:
  - `MobLifecycle.active_run_count`
  - `MobOrchestrator.active_flow_count`
  - actor-owned `run_tasks` / `run_cancel_tokens` tracker maps

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobFlowTrackerSnapshot { run_task_ids, cancel_token_ids, stream_ids }`
  - extended `MobKernelDiagnosticSnapshot` with `flow_trackers`
- `meerkat-mob/src/runtime/actor.rs`
  - added `flow_tracker_snapshot()`
  - changed `DiagnosticKernelSnapshot` to include the live flow tracker snapshot
  - reused the same snapshot for the existing test-only `FlowTrackerCounts`
    command so the diagnostic path and the older debug path stop drifting
- `meerkat-mob/src/mob_machine.rs`
  - added flow-aware validation:
    - lifecycle `active_run_count` must match tracked run-task count
    - orchestrator `active_flow_count` must match tracked run-task count
    - tracked run tasks and cancel tokens must name the same runs
    - scoped event streams must be a subset of tracked runs
  - expanded the synthetic negative test to prove each mismatch is caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live in-flight flow test that captures `MobMachine` snapshots:
    - while a delayed single-step flow is active
    - after terminalization and tracker drain
  - both snapshots must satisfy the new flow-aware validator

Why this slice matters:

- it gives `MobMachine` a real join across three previously separate carriers
  of the same semantic fact instead of trusting the old debug helper counts
- it also confirms something important for the future two-kernel collapse:
  at actor command boundaries, these flow-count carriers are already coherent
  enough to validate directly rather than only by convention

Verification:

- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_joins_live_roster_and_member_projection`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_flow_activation_and_lifecycle_transitions`
- `cargo test -p meerkat-mob --lib test_flow_tracker_maps_remain_coherent_under_concurrent_run_and_cancel_commands`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first synthetic negative test used `RunId::from(\"...\")`, but `RunId`
  is UUID-backed, so the failure was in the harness rather than the model
- I fixed the test to use real `RunId::new()` values and re-ran the full slice
  instead of weakening the new validator

Current limitation:

- this slice only validates tracker/count coherence, not full flow-run kernel
  truth such as per-run terminal phase, frame state, or step-ledger projection
- `stream_ids` are only validated as a subset relation; there is still no
  stronger semantic check on scoped-event stream lifecycle

Next likely step:

- take the next Mob slice through a narrow flow-run kernel join:
  active run identity vs `MobRunStore` terminal/non-terminal truth, while
  keeping the validator atomic and actor-owned

## Slice 26 - MobMachine tracked-run store identity

Goal:

- join actor-owned tracked run identity with durable `MobRunStore` identity
- verify that tracked runs resolve in the store and carry the same `flow_id`
  the actor believes they do
- keep the invariant conservative enough to survive the real
  terminalization-before-cleanup window

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - extended `MobFlowTrackerSnapshot` with `tracked_flows: RunId -> FlowId`
- `meerkat-mob/src/runtime/actor.rs`
  - `flow_tracker_snapshot()` now exposes that actor-owned run-to-flow mapping
- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunStoreSnapshot { flow_id, status }`
  - `capture_mob_machine_snapshot()` now resolves every tracked run ID through
    `MobHandle::flow_status()`
  - added validation that:
    - every tracked run ID exists in `MobRunStore`
    - actor-owned `RunId -> FlowId` agrees with persisted `MobRun.flow_id`
  - expanded the synthetic negative test to prove missing store entries and
    flow-id mismatches are caught
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live in-flight flow test so the tracked active run must
    resolve from the store with the expected `flow_id`

Why this slice matters:

- it is the first `MobMachine` join that crosses from actor-owned live control
  state into durable run aggregate truth
- it proves a real mapping to existing code paths rather than to a new
  abstraction: the join is grounded in `run_cancel_tokens` plus
  `MobHandle::flow_status()` / `MobRunStore::get_run()`
- it keeps us honest about what is not safe to prove yet: run existence and
  identity are stable enough, but status-phase equivalence is still too
  timing-sensitive at the current cleanup boundary

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_orchestrator_snapshot_tracks_flow_activation_and_lifecycle_transitions`
- `cargo test -p meerkat-mob --lib test_flow_tracker_maps_remain_coherent_under_concurrent_run_and_cancel_commands`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- passed

Backtracks encountered:

- the first instinct was to validate tracked runs as non-terminal, but the
  current actor path can legitimately terminalize a run before the cleanup
  command drains local trackers
- instead of encoding a flaky stronger invariant, this slice stops at store
  existence and `flow_id` identity

Current limitation:

- `MobMachine` still does not validate whether the persisted `MobRun.status`
  phase is aligned with local tracker presence; that remains a known timing
  seam
- the run-store join is point-in-time and non-atomic with respect to actor
  cleanup, so only stable identity facts are validated

Next likely step:

- either collapse the cleanup seam enough to validate tracked-run terminal
  phase safely, or move sideways into a different owner-backed Mob region such
  as kickoff barriers or flow-run kernel projection

## Slice 27 - MobMachine pending-spawn lineage

Goal:

- expose pending-spawn lineage as a first-class Mob diagnostic lane instead of
  relying on actor-local debug assertions
- verify that staged pending spawns stay aligned with orchestrator-owned
  pending count and do not materialize into roster/member projection early
- keep the slice grounded in the real async provisioning path, not the older
  kickoff barrier shim

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobPendingSpawnLineageSnapshot` to the diagnostic kernel surface
- `meerkat-mob/src/runtime/pending_spawn_lineage.rs`
  - added a lineage snapshot that exposes:
    - metadata ticket IDs
    - task ticket IDs
    - ticket-to-member identity mapping
    - fully provision-bound tickets
    - partial progress tickets
- `meerkat-mob/src/runtime/actor.rs`
  - `DiagnosticKernelSnapshot` now includes the live pending-spawn lineage
- `meerkat-mob/src/mob_machine.rs`
  - added validation that:
    - orchestrator pending count matches lineage size
    - metadata and task ticket sets stay aligned
    - a pending member identity cannot appear twice
    - pending members do not appear in roster or projected member state
    - pending progress never exposes a partial session/operation binding
  - expanded the synthetic negative test to prove those violations are caught
- `meerkat-mob/src/runtime/tests.rs`
  - added a live delayed-spawn test that captures `MobMachine` during
    in-flight provisioning and after finalization settlement

Why this slice matters:

- pending spawn lineage is a real owner-backed Mob seam: it bridges actor task
  state, orchestrator counters, and eventual roster materialization
- the actor already depended on this alignment for cleanup and duplicate-ID
  prevention, but the joined `MobMachine` view could not see it before
- this keeps us on stable identity facts rather than forcing speculative
  stronger flow-status proof

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- pending-spawn lineage is now part of the visible Mob kernel surface, and the
  live delayed-spawn path satisfies the new invariants

Backtracks encountered:

- I chose not to promote kickoff waiters as the next Mob seam because the code
  now treats kickoff as a compatibility barrier over synchronous runtime input
  admission, which makes it a weaker semantic center than pending spawn
  lineage
- I also stopped short of requiring every provision-bound pending ticket to
  have reached roster materialization, because finalization legitimately
  settles that after the async provisioning task completes

Current limitation:

- the validator still does not reason about the timing between
  `provision_bound_ticket_ids` and final roster admission; it only forbids
  premature materialization and partial binding

Next likely step:

- either validate one stable kickoff/initial-runtime readiness fact now that
  pending spawn lineage is visible, or move into a narrow flow-kernel join
  around persisted step/activation truth

## Slice 28 - MobMachine restore-failure ownership

Goal:

- stop treating `Broken` member status as a loose projection by joining it back
  to the canonical restore-failure map
- verify that restore diagnostics, roster membership, and projected broken
  member material stay aligned through real resume failures
- keep the seam honest by going through a named handle-level diagnostic read
  path rather than reaching into handle internals directly

What landed:

- `meerkat-mob/src/runtime/handle.rs`
  - added `diagnostic_restore_failures_snapshot()` that normalizes the
    internal `HashMap` into a deterministic `BTreeMap`
- `meerkat-mob/src/mob_machine.rs`
  - added `restore_failures` to `MobMachineSnapshot`
  - added restore-aware invariants:
    - every restore failure maps to a roster member
    - every restore failure maps to a projected member
    - projected member status must be `Broken`
    - projected session binding and error reason must match the restore
      diagnostic
    - every projected `Broken` member must have a restore diagnostic
  - added a focused negative unit test proving the validator rejects a
    projected broken member without canonical restore failure
- `meerkat-mob/src/runtime/tests.rs`
  - added a live resume-failure test that captures `MobMachine` after a
    missing-persisted-session restore and proves the joined snapshot is clean

Why this slice matters:

- `Broken` is not an independent lifecycle state today; it is restore-failure
  truth exposed through member projection
- before this slice, `MobMachine` could see broken members but not the owner
  that made them broken, which made the boundary fuzzier than it needed to be
- joining the restore map tightens an actual product-facing failure mode
  without introducing new timing assumptions

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_broken_projection_without_restore_failure`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_restore_failure_projection`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_topology_coherence`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- restore-failure ownership is now visible in the joined Mob snapshot, and the
  live broken-resume path satisfies the new invariants

Backtracks encountered:

- the first implementation tried to read `MobHandle.restore_diagnostics`
  directly from `mob_machine.rs`, which was a good failure: it crossed a
  private boundary instead of using a named diagnostic seam
- fixing that by adding a handle accessor also forced deterministic ordering at
  the seam, which is better for tests and future TLA-facing snapshots

Current limitation:

- this still models restore failure as a handle-side joined read path, not as
  actor-owned lifecycle truth; that is appropriate for today’s code, but it
  means `Broken` remains a projection over roster plus restore diagnostics

Next likely step:

- either pull one stable kickoff/readiness fact into the joined MobMachine
  view, or switch back to a narrow Meerkat-side slice if the next Mob seam is
  too timing-sensitive

## Slice 29 - MeerkatMachine peer authority and queue coherence

Goal:

- tighten the peer-ingress validator from "authority queue empty vs non-empty"
  to exact agreement between the authority-owned submission queue and the
  queued authority-tracked ingress entries
- keep the rule grounded in the live owner boundary: the inbox snapshot is
  captured atomically under one lock, so this count equality is stable enough
  to validate without cross-crate timing races
- avoid smuggling plain external events into peer authority semantics

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added `PeerAuthorityTrackedEntryCountMismatch`
  - added `push_peer_authority_mismatches(...)`
  - validator now checks that `submission_queue_len` equals the number of
    queued entries whose kind is not `PlainEvent`
  - added a focused negative test proving the validator rejects a snapshot
    where peer authority claims two queued submissions but only one
    authority-tracked ingress entry exists

Why this slice matters:

- the previous phase/queue-length rule could still miss a subtler owner drift:
  a `Received` authority with a non-zero queue length but the wrong number of
  actual peer entries visible at the ingress boundary
- exact count agreement is a stronger statement and still honest because both
  numbers come from the same inbox-owned snapshot path
- excluding `PlainEvent` from the comparison preserves the important
  architecture cut: plain external events are queued ingress, but not peer
  authority truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- MeerkatMachine now validates the exact size of the authority-tracked peer
  ingress surface, not just the authority phase

Backtracks encountered:

- the tempting but wrong invariant was "submission queue length matches total
  queue length"; the comms runtime tests make it clear that plain external
  events may coexist with an `Absent` or `Dropped` peer authority
- the right invariant therefore counts only authority-tracked entries and
  leaves plain events outside the peer authority ledger

Next likely step:

- either deepen Meerkat's peer side one more step with trust-membership
  surface checks, or return to a narrow MobMachine slice once this Meerkat
  seam feels solid enough

## Slice 30 - MeerkatMachine ingress-time trust snapshots

Goal:

- stop reconstructing peer-entry trust from the current trusted-peer set and
  carry the canonical ingress-time trust decision into the queued peer snapshot
- validate that strict-auth runtimes never expose an admitted untrusted peer
  entry, while plain external events remain outside peer authority truth
- tighten the classified inbox seam so the peer authority is seeded from the
  same initial trust context used by ingress classification

What landed:

- `meerkat-core/src/interaction.rs`
  - added `trusted_snapshot: Option<bool>` to `PeerIngressEntrySnapshot`
- `meerkat-comms/src/peer_comms_authority.rs`
  - added `trusted_snapshot_for(...)` accessor
- `meerkat-comms/src/inbox.rs`
  - `ClassifiedInboxEntry` now carries `trusted_snapshot`
  - `send_classified()` now threads the authority-owned trust decision into
    queued entries
  - `Inbox::new_classified()` now seeds the peer authority with the initial
    trusted peers from `IngressClassificationContext`
  - inbox snapshot tests now assert trusted peer entries carry
    `Some(true)` and plain events carry `None`
- `meerkat-comms/src/runtime/comms_runtime.rs`
  - live runtime snapshot tests now assert plain events keep
    `trusted_snapshot = None`
  - auth-open unknown-peer admission now proves
    `trusted_snapshot = Some(false)` on the admitted peer entry
- `meerkat/src/meerkat_machine.rs`
  - validator now checks:
    - non-plain entries must carry a trust snapshot
    - plain events must not
    - auth-required runtimes may not expose admitted untrusted peer entries
  - authority-tracked entry counting now keys off the explicit trust snapshot
    rather than inferring from ingress kind alone
  - added a focused negative test for trust-shape violations

Why this slice matters:

- before this change, the Meerkat peer snapshot knew "who is trusted now" but
  not "was this queued peer item trusted when it was admitted"
- that made it impossible to distinguish a valid auth-open untrusted peer
  entry from a malformed strict-auth ingress state without re-deriving truth
- seeding the authority from the initial classification context closes a real
  split-owner gap: classified ingress and peer authority now start from the
  same trust basis

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-comms --lib classified_snapshot`
- `cargo test -p meerkat-comms --lib peer_ingress_runtime_snapshot`
- `cargo test -p meerkat-comms --lib auth_open`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_trust_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_peer_authority_count_violations`
- `cargo test -p meerkat --lib --features comms capture_meerkat_machine_snapshot_joins_live_peer_runtime_state`
- `cargo check -p meerkat-comms --lib`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- MeerkatMachine now sees peer ingress with the same trust decision the peer
  authority used at admission time, and the classified inbox no longer starts
  with a silent authority/trust mismatch

Backtracks encountered:

- the first pass assumed the inbox tests should already report
  `trusted_snapshot = Some(true)` for seeded trusted peers; when they instead
  reported `Some(false)`, that exposed a real initialization gap rather than a
  bad test
- fixing the root cause in `Inbox::new_classified()` was the right move; it
  brought the authority into line with the classification context instead of
  weakening the new trust assertions

Next likely step:

- Meerkat's peer seam is now much firmer, so the next deliberate move can
  either be one more same-owner peer/trust invariant or a return to a narrow
  MobMachine slice without building on soft ground

## Slice 31 - MobMachine kickoff barrier ownership

Goal:

- make live autonomous kickoff-barrier state visible inside the joined
  `MobMachine` kernel snapshot instead of leaving it as a handle-only wait path
- validate the strongest same-owner seam first: a member cannot still be in
  pending spawn lineage and already be tracked by the kickoff barrier
- verify the live delayed-autonomous-start path against real actor state

What landed:

- `meerkat-mob/src/runtime/mod.rs`
  - added `MobKickoffBarrierSnapshot { pending_member_ids }`
  - added `kickoff_barrier` to `MobKernelDiagnosticSnapshot`
- `meerkat-mob/src/runtime/actor.rs`
  - added `kickoff_barrier_snapshot()` using the existing
    `autonomous_initial_turns` map and the same finished-handle pruning logic
    as `snapshot_kickoff_barrier_state()`
  - `DiagnosticKernelSnapshot` now includes the live kickoff barrier snapshot
- `meerkat-mob/src/mob_machine.rs`
  - added kickoff-aware invariants:
    - kickoff-pending member must not overlap pending spawn lineage
    - kickoff-pending member must still resolve in roster + projected member
    - kickoff-pending member must still have a session binding
    - kickoff-pending member must not already be `Broken`
  - added a focused synthetic validator test for kickoff-specific violations
  - extended the main synthetic projection test so kickoff/pending-spawn
    overlap is exercised alongside the earlier lineage assertions
- `meerkat-mob/src/runtime/tests.rs`
  - added a live test with delayed autonomous startup showing:
    - the spawned member appears in `kickoff_barrier.pending_member_ids`
    - the same member is already past pending spawn lineage
    - the settled snapshot drains the kickoff barrier after
      `wait_for_kickoff_complete()`

Why this slice matters:

- kickoff barrier state is real Mob runtime truth: callers use it to decide
  whether autonomous startup has actually settled
- before this slice, MobMachine could see pending spawn lineage and member
  projection, but not the intermediate autonomous-start barrier state between
  those phases
- the overlap check closes a useful ownership seam without relying on timing:
  pending spawn lineage and kickoff barrier are both actor-owned and captured in
  the same diagnostic kernel snapshot

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_projection_violations`
- `cargo test -p meerkat-mob --lib mob_machine::tests::validate_mob_machine_snapshot_reports_kickoff_barrier_projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_kickoff_barrier_state`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_pending_spawn_lineage`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_restore_failure_projection`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine now sees and validates the live autonomous kickoff barrier, and
  the actor-owned handoff from pending spawn lineage to kickoff tracking is
  executable instead of implicit

Backtracks encountered:

- the first kickoff-specific unit test was shaped wrong: it expected a missing
  projected entry for a member that was still present in the projected member
  list
- splitting that into distinct fixture members produced a cleaner test and a
  better mapping from each violation to one semantic mistake

Next likely step:

- the next honest Mob slice is probably a flow-kernel join, now that the
  lifecycle/orchestrator/topology/pending-spawn/kickoff stack is visible in one
  place

## Slice 32 - MobMachine tracked flow-kernel shape

Goal:

- make `MobMachine` read typed flow-kernel run state instead of treating
  `flow_status()` as just `{ flow_id, status }`
- validate stable run-kernel invariants that survive cleanup races:
  completion timestamp shape, ordered-step membership, and terminal
  non-dispatched step state
- keep the slice honest by avoiding "tracked run must already be cleaned up"
  folklore while a terminal run is still draining through actor cleanup

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun` readers for:
    - `ordered_steps()`
    - `step_status_snapshot()`
    - `failure_count()`
    - `consecutive_failure_count()`
  - centralized `StepRunStatus` parsing with
    `StepRunStatus::from_flow_run_kernel_value(...)`
  - added focused unit coverage for the new typed readers and explicit unknown
    variant rejection
- `meerkat-mob/src/runtime/flow_run_kernel.rs`
  - now reuses the new `StepRunStatus` parser instead of carrying its own
    duplicate decoding logic
- `meerkat-mob/src/mob_machine.rs`
  - replaced the old tracked-run projection with a richer
    `TrackedRunSnapshot` model:
    - `Missing`
    - `Present(TrackedRunStoreSnapshot)`
    - `Malformed(TrackedRunMalformedSnapshot)`
  - `capture_mob_machine_snapshot()` now parses live `MobRun` kernel state into
    typed tracked-run snapshots and records parse failures explicitly instead of
    silently collapsing them into missing-store state
  - added tracked-run invariants for:
    - terminal status ↔ `completed_at` presence
    - duplicate ordered-step entries
    - step-status keys outside the ordered-step set
    - terminal runs still carrying `Dispatched` step status
    - `consecutive_failure_count <= failure_count`
  - added a focused synthetic validator test for tracked-run kernel-shape
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting MobMachine test to assert the richer
    tracked-run projection:
    - `completed_at_present` stays false while the run is active
    - ordered steps match the single-step flow definition
    - surfaced step statuses stay within the kernel-known step set

Why this slice matters:

- this is the first MobMachine slice that reads through typed flow-kernel state
  rather than only actor counters and run-store identity
- `MobRun.flow_state` already contains the canonical flow-run machine truth, but
  without typed readers the diagnostic path had to stop at shallow projection
- making malformed tracked runs explicit is important: a corrupt kernel state is
  not the same thing as a missing run, and collapsing those two would create
  another diagnostic blind spot

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib mob_run_kernel_readers_surface`
- `cargo test -p meerkat-mob --lib step_status_snapshot_rejects_unknown_variant`
- `cargo test -p meerkat-mob --lib tracked_run_kernel_shape`
- `cargo test -p meerkat-mob --lib projection_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo check -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine can now see and validate a meaningful slice of flow-kernel truth
  for tracked runs, not just actor-owned run counters
- the flow-run parser path is now centralized enough that future tracked-run
  diagnostics can build on `MobRun` helpers instead of duplicating `KernelValue`
  decoding logic

Backtracks encountered:

- the first compile pass exposed two self-inflicted issues:
  - partial move of `run.status` before reading the rest of the aggregate
  - a test pattern that moved the error string it still wanted to print
- fixing both early was useful signal that the richer tracked-run projection was
  still a modest, localized slice rather than a broader ownership problem

Next likely step:

- the next honest Mob slice is probably one of:
  - a deeper flow-kernel join around frame/run readiness state
  - a return to MeerkatMachine if the next Mob flow invariant starts depending
    on cleanup timing rather than stable owner truth
