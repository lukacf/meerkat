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

## Slice 33 - MeerkatMachine comms-drain shape

Goal:

- turn the runtime-owned comms-drain carrier from "visible but unvalidated"
  into an executable MeerkatMachine slice
- keep the slice strictly inside same-owner runtime truth so it satisfies the
  cutover gate discipline for observability work
- avoid waiter/ledger or barrier/ops joins that still depend on non-atomic
  multi-lock reads

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added drain-shape invariants to `validate_meerkat_machine_snapshot()`:
    - no phase/mode/handle may appear without a published drain slot
    - a published slot must carry a phase
    - `Inactive` drain cannot carry mode or handle
    - `Starting | Running | ExitedRespawnable | Stopped` must carry mode
    - `Inactive | ExitedRespawnable | Stopped` cannot still hold a task handle
  - added a focused synthetic validator test covering:
    - missing-slot metadata leakage
    - slot-without-phase
    - inactive-with-mode/handle
    - stopped-without-mode and stopped-with-handle
  - added a joined positive test proving a real stopped comms drain remains a
    valid `MeerkatMachine` snapshot
- `meerkat-runtime/src/session_adapter.rs`
  - tightened the idle spine test to assert the absent-drain shape explicitly
  - added a runtime-spine test for a live `Stopped` comms-drain slot:
    - slot remains published
    - phase is `Stopped`
    - mode is preserved
    - task handle is cleared

Why this slice matters:

- `drain` was one of the remaining Meerkat runtime-spine regions that had real
  owner data in the snapshot but no executable machine checks
- the comms-drain lifecycle is also a recurring seam hazard because it mixes
  keep-alive policy, background task ownership, and stop/exit cleanup
- this slice is a good fit for the cutover-gate discipline: it strengthens the
  observational machine without inventing a new reducer or depending on racy
  cross-crate joins

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_comms_drain_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_stopped_comms_drain_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_drain_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `git diff --check`

Result:

- MeerkatMachine now validates the runtime-owned comms-drain carrier in the
  same style as peer ingress, tool-surface, and ingress lifecycle shape
- the live stopped-drain state is now represented and tested explicitly, which
  matters for later cutover work because stop/cleanup states are where the old
  architecture usually lies

Backtracks encountered:

- the tempting next slice was completion-waiter vs admitted-input terminality,
  but that still crosses separate runtime locks and would have pushed us toward
  snapshot folklore instead of owner-backed machine truth
- staying with drain was the right call because its phase/mode/handle shape is
  captured from one owner lock and survives both positive and negative tests

Cutover gate read:

- this slice improves MeerkatMachine observability and executable coverage, but
  it does **not** move MeerkatMachine past the cutover gate yet
- we are still in the "observational model over real owners" stage:
  write authority has not moved, semantic freeze is not complete, and the
  stronger cross-region joins (especially timing-sensitive ones) are still
  intentionally deferred

Next likely step:

- return to `MobMachine` for another narrow owner-backed slice, or keep
  Meerkat-first if the next most valuable gap is still clearly same-owner and
  stable under the cutover-gate rules

## Slice 34 - MobMachine tracked run dependency-shape

Goal:

- deepen `MobMachine`'s tracked run view with one more proof-relevant slice of
  flow-kernel truth
- stay entirely inside the persisted run-store surface, avoiding actor cleanup
  timing and avoiding new shadow state
- use the cutover gate discipline to distinguish real live kernel truth from
  values that only exist on input/setup paths

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_dependencies()` reader for the persisted
    `step_dependencies` kernel field
  - extended the existing reader test to assert dependency map shape for a
    simple flow
  - added a focused negative test that rejects malformed dependency entries
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_dependencies`
  - added tracked-run invariants for:
    - every ordered step must have a dependency-map entry
    - dependency-map keys must refer to ordered steps
    - dependency targets must refer to ordered steps
  - extended the synthetic tracked-run validator test with dependency-shape
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface an empty dependency map for its only step

Why this slice matters:

- step/dependency metadata is proof-relevant flow truth, but unlike actor task
  trackers it lives in one durable owner surface
- this gives `MobMachine` another meaningful piece of kernel state without
  drifting into cleanup races or projecting behavior from event order
- it also sharpens the future TLA+ boundary by making more of the flow-kernel
  alphabet readable without spelunking raw `KernelValue` maps in validators

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_dependencies_reject_invalid_dependency_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- MobMachine now sees and validates another stable slice of tracked flow-kernel
  structure from the live run store
- the live flow-accounting path still passes after the richer tracked-run
  parsing, so the slice is grounded in the actual runtime rather than a purely
  synthetic validator fixture

Backtracks encountered:

- the first version of this slice tried to treat `step_ids` as persisted live
  kernel truth because it is present in the `CreateRun` input surface
- that was wrong: the live tracked run failed immediately with
  `step_ids missing or invalid`, which means `step_ids` is not a stable stored
  field in the current run aggregate
- the correct recovery was to back out `step_ids` entirely and anchor the slice
  to fields that really survive in durable state:
  `ordered_steps` and `step_dependencies`

Cutover gate read:

- this slice improves MobMachine observability, but it does **not** move
  MobMachine past the cutover gate
- the important signal here is that the gate is still doing useful work:
  it caught a tempting but false "machine fact" before we promoted it into a
  larger semantic story

Next likely step:

- either keep deepening persisted tracked-run metadata in similarly small
  slices, or move sideways to another owner-backed Mob kernel region if the
  next flow-kernel fact starts depending on cleanup timing rather than stable
  stored truth

## Slice 35 - MeerkatMachine ops-shape

Goal:

- tighten `MeerkatMachine` around one more same-owner runtime seam without
  stepping into the still-racy cross-crate joins
- make the existing ops snapshot executable instead of just descriptive by
  validating count shape, wait-all shape, wait-target coverage, and
  detached-wake presence pairing
- keep the slice entirely on the current runtime-owned read path so the
  cutover gate can still distinguish owner truth from snapshot folklore

What landed:

- `meerkat/src/meerkat_machine.rs`
  - added ops-shape invariant variants for:
    - `OpsOperationCountMismatch`
    - `WaitOperationIdsWithoutWaitRequest`
    - `WaitRequestWithoutTrackedOperations`
    - `WaitTargetsUnknownOperation`
    - `DetachedWakePresenceMismatch`
  - factored the runtime-owned checks into `push_ops_mismatches(...)`
  - added a focused synthetic validator test that mutates a live base snapshot
    instead of inventing a completely synthetic machine fixture

Why this slice matters:

- the ops snapshot already comes from one runtime-owned lock path, so it is a
  safer place to strengthen executable machine checks than the timing-sensitive
  barrier/turn joins
- the new invariants catch the exact kind of owner/shell drift we have seen in
  earlier backtracks: a wait request without a tracked set, tracked wait
  targets without a wait request, or snapshot operation counts that no longer
  line up with live records
- detached-wake presence pairing is small, but it is one of the cleanup/handoff
  carriers that historically tends to rot quietly if we do not make it explicit

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another owner-backed region of runtime truth
  instead of only exposing it diagnostically
- the live runtime and Meerkat library lanes stayed green, which is a useful
  sign that these checks are anchored in real runtime authority rather than in
  aspirational machine shape

Backtracks encountered:

- I explicitly did **not** strengthen `active_count` into a full
  status-derived equality check yet, because the current operation snapshot is
  still built from authority plus shell records and I did not want to blur
  "authority mismatch" with "projection missing shell record" in the same step
- I also kept barrier/ops coupling out of this slice; that join still crosses
  timing-sensitive read paths and would not yet satisfy the cutover-gate bar

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- write authority is still in the old owners, semantic freeze is still not
  complete, and the stronger timing-sensitive joins are still intentionally
  deferred

Next likely step:

- either deepen one more same-owner ops invariant (if it can be justified from
  one runtime-owned snapshot path), or return to another Meerkat region before
  attempting the still-riskier barrier/turn coupling

## Slice 36 - MobMachine tracked run dependency-mode shape

Goal:

- continue `MobMachine` on the same persisted flow-kernel path without falling
  back into actor cleanup timing
- expose one more piece of durable run truth that is semantically meaningful to
  flow execution: the per-step dependency interpretation mode
- use the cutover gate to avoid promoting config/input-only facts that are not
  actually persisted in the live run aggregate

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_dependency_modes()` reader for the persisted
    `step_dependency_modes` kernel field
  - extended the kernel-reader test to assert live dependency-mode shape for a
    simple flow
  - added a focused negative parse test for unknown `DependencyMode` variants
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_dependency_modes`
  - added tracked-run invariants for:
    - every ordered step must have a dependency-mode entry
    - dependency-mode map keys must refer to ordered steps
  - extended the synthetic tracked-run validator test with dependency-mode
    violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `DependencyMode::All` for its only step

Why this slice matters:

- dependency mode is real flow-kernel truth, not just config decoration: it is
  what makes `all` versus `any` dependency readiness semantically different at
  execution time
- unlike actor task trackers or cleanup counters, it lives in durable run state
  and survives the current read path cleanly
- it also reinforces the cutover-gate discipline after the previous
  `step_ids` backtrack: we are still only promoting facts that actually survive
  in the stored run aggregate

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_dependency_modes_reject_unknown_variant`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now sees and validates another durable slice of tracked
  flow-kernel truth without leaning on actor cleanup timing
- the live flow-accounting snapshot continues to satisfy the richer validator,
  so this slice stays grounded in the actual runtime rather than in a
  synthetic-only model

Backtracks encountered:

- there was no code-level backtrack in this slice, but the selection itself was
  a deliberate backtrack from "any next flow field" to "only fields that are
  clearly persisted kernel truth"
- I explicitly did **not** promote collection policy or other config-shaped
  surfaces in this pass because I had not yet re-established that they are the
  right stable read boundary for the live run aggregate

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- the tracked-run surface is getting firmer, but write authority is still in
  the old runtime/store/orchestrator components and the semantic boundary is
  still not frozen

Next likely step:

- either keep deepening durable tracked-run metadata in similarly small slices,
  or move sideways to another owner-backed Mob region if the next tracked-run
  fact starts to depend on actor cleanup timing instead of stable persisted
  truth

## Slice 37 - MeerkatMachine per-operation ops shape

Goal:

- deepen `MeerkatMachine` ops validation without crossing into the still-racy
  barrier/turn join
- promote one more owner-backed slice of runtime truth from "count-like shell
  shape" into explicit per-operation semantics
- keep the slice grounded in the existing runtime authority read path so it can
  be judged against the cutover gate rather than against an idealized machine

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with richer per-operation ops
    mismatch cases:
    - detached wake presence mismatch with the runtime binding
    - active-count versus computed nonterminal operation count mismatch
    - wait-all request already satisfied by the tracked operation set
    - peer-ready/handle inconsistencies
    - terminal outcome and completion timestamp mismatches
    - active operation missing start timestamp
    - terminal operation still carrying watchers
  - strengthened `push_ops_mismatches(...)` to validate those invariants from
    the joined runtime spine
  - extended the focused negative validator test so the richer per-operation
    mismatches are actually exercised
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the positive runtime-spine test to assert concrete per-op
    shape on a live operation snapshot, including start/completion timing and
    peer-handle absence

Why this slice matters:

- it moves ops validation closer to the real historical failure surface:
  operations are not just aggregate counts, they carry lifecycle, wait, wake,
  peer handoff, and completion truth
- it stays on one runtime-owned read path, which keeps it inside the
  cutover-gate discipline instead of overreaching into timing-sensitive joins
- it also gives us a more honest signal about whether the observational model
  is merely describing the shell or actually matching owner-backed runtime
  truth

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_runtime_ops_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`

Result:

- `MeerkatMachine` now validates richer per-operation ops truth rather than
  only aggregate runtime shape
- the focused runtime and joined Meerkat tests stayed green, which is a good
  sign that these checks are still anchored in the real runtime authority path

Backtracks encountered:

- the first pass of the richer negative ops test moved the base snapshot too
  early and then tried to clone it again; that was a test-harness mistake, not
  a machine-design issue, and it was fixed by cloning earlier
- I still deliberately kept barrier/ops coupling out of this slice because it
  would not yet satisfy the cutover-gate requirement for stable owner
  boundaries and non-racy reads

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- write authority still lives in the old runtime owners, semantic freeze is
  still incomplete, and the stronger timing-sensitive joins are still
  intentionally deferred

Next likely step:

- either take one more same-owner runtime slice that clearly satisfies the
  cutover gate, or move back to another Meerkat region before attempting the
  still-riskier barrier/turn coupling

## Slice 38 - MobMachine tracked run collection-policy and quorum shape

Goal:

- continue `MobMachine` on the durable tracked-run path rather than dropping
  back into actor cleanup timing
- validate that another piece of flow-kernel meaning survives in persisted run
  truth, not just in the create-run input surface
- use the cutover gate to reject any field that turns out to be setup-shaped
  rather than stable kernel truth

What landed:

- `meerkat-mob/src/run.rs`
  - added typed readers for:
    - `step_collection_policy_kinds()`
    - `step_quorum_thresholds()`
  - introduced `RunCollectionPolicyKind` so collection policy can be read as
    typed kernel truth instead of raw strings
  - extended the existing kernel-reader test to assert collection policy and
    quorum shape for a simple persisted run
  - added a focused negative parse test for unknown collection-policy variants
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with:
    - `step_collection_policy_kinds`
    - `step_quorum_thresholds`
  - added tracked-run invariants for:
    - every ordered step must have a collection policy entry
    - every ordered step must have a quorum-threshold entry
    - policy and quorum maps may only reference ordered steps
    - `Quorum` steps must have a nonzero threshold
    - `All` and `Any` steps must keep threshold `0`
  - extended the synthetic tracked-run validator test with the new violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface both `RunCollectionPolicyKind::All` and quorum threshold `0`

Why this slice matters:

- collection policy and quorum threshold are semantically active flow-kernel
  truth; they govern when a step may become ready, not just how the original
  flow happened to be authored
- the live flow-accounting snapshot proved they really survive in persisted run
  state, which is the exact question the cutover gate asks us to answer before
  promoting a field into the machine
- it is also a direct continuation of the earlier `step_ids` backtrack: we are
  still refusing to model fields as kernel truth until the live runtime proves
  they actually live there

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_collection_policy_kinds_reject_unknown_variant`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another durable slice of tracked flow-kernel truth
  without leaning on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful proof
  that this is persisted kernel state rather than config echo

Backtracks encountered:

- there was no code-level backtrack in the final slice, but the selection
  itself was a deliberate backtrack from "read any nearby flow field" to "only
  promote facts that the live run aggregate actually preserves"
- that discipline is exactly the one the cutover gate is meant to enforce

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- write authority is still in the old runtime/store/orchestrator components,
  the machine boundary is still converging, and semantic freeze is still not
  complete

Next likely step:

- either keep deepening clearly persisted tracked-run metadata in similarly
  narrow slices, or move sideways to another owner-backed Mob region if the
  next candidate starts to depend on actor cleanup timing instead of durable
  truth

## Slice 39 - MeerkatMachine completion waiter coherence

Goal:

- strengthen the completion-waiter carrier without crossing into new owner
  boundaries
- make the joined `MeerkatMachine` validator reject waiter state that already
  points at resolved input truth
- keep the slice grounded entirely in the runtime-owned spine: admitted input
  lifecycle and terminal outcome plus the existing completion-waiter snapshot

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `CompletionWaiterZeroCount`
    - `CompletionWaiterResolvedInput`
  - strengthened completion-waiter validation so:
    - waiter entries may not have `waiter_count == 0`
    - waiter entries may not point at inputs that are already terminal or that
      already expose a terminal outcome
  - extended the focused negative joined-snapshot test to exercise:
    - zero-count waiter entries
    - waiter entries attached to an input already marked
      `Consumed`/`terminal_outcome = Consumed`

Why this slice matters:

- completion waiters are a supporting carrier, not canonical truth, so they
  are exactly the kind of thing that tends to drift unless the machine makes
  the carrier constraints explicit
- this slice stays fully on one runtime-owned read path and therefore fits the
  cutover-gate rule of "strengthen already-agreed ownership, do not invent a
  new boundary"
- it also tightens one of the historical handoff surfaces: callers waiting on
  input completion should never be represented as still pending after the input
  is already resolved

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`

Result:

- `MeerkatMachine` now validates a stricter completion-waiter carrier shape
  without inventing new runtime semantics
- the live Meerkat and runtime-spine tests stayed green, which is the right
  signal that the new rule is aligned with current owner-backed runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice, but the selection was
  deliberately narrower than several tempting alternatives
- I explicitly did **not** try to infer callback-boundary semantics or new
  completion classes here; this slice only uses truth already present in the
  runtime spine

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- completion waiter write authority still lives in the existing runtime
  component, and semantic freeze is still incomplete

Next likely step:

- either take another same-owner runtime slice around input/completion carrier
  coherence, or move sideways to another Meerkat region before revisiting the
  still-riskier timing-sensitive joins

## Slice 40 - MobMachine tracked run condition-flag shape

Goal:

- continue `MobMachine` on the persisted tracked-run path
- promote one more flow-kernel field only if the live stored run aggregate
  proves it really survives there
- validate step-level condition-presence shape without dragging in dispatch
  timing or actor cleanup state

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_has_conditions()` reader over the persisted
    `step_has_conditions` kernel map
  - extended the kernel-reader test to assert condition-presence shape for a
    simple persisted run
  - added a focused negative parse test for non-boolean condition flags
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_has_conditions`
  - extended tracked-run validation so:
    - every ordered step must have a condition-flag entry
    - condition-flag keys may only reference ordered steps
  - extended the synthetic tracked-run validator test with condition-flag
    missing/unknown-step violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `step_has_conditions = { start -> false }`

Why this slice matters:

- condition presence is not just authoring metadata; it changes dispatch
  legality because condition results must be recorded before a guarded step can
  run
- the live flow-accounting snapshot proved that the field is actually preserved
  in the stored run aggregate, which is exactly the bar the cutover gate asks
  us to clear before promoting a field into the machine
- it also keeps us on the durable flow-kernel path rather than drifting back
  into actor timing or cleanup folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_has_conditions_rejects_non_bool_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another persisted step-level flow-kernel map
  without relying on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful signal
  that `step_has_conditions` is real tracked-run truth rather than create-run
  config echo

Backtracks encountered:

- the only backtrack was the usual selection discipline: I promoted
  `step_has_conditions` only after the live run projection proved it survives
  as stored kernel state
- there was no code-level correction once that boundary was chosen

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- tracked-run write authority is still in the existing runtime/store/kernel
  stack, and semantic freeze is still incomplete

Next likely step:

- either keep deepening clearly persisted tracked-run shape in similarly narrow
  slices, or move sideways to another owner-backed Mob region if the next
  candidate starts depending on actor timing instead of durable truth

## Slice 41 - MeerkatMachine ingress routing and contributor linkage

Goal:

- strengthen `MeerkatMachine` around runtime-ingress facts that are already
  canonical in `RuntimeIngressAuthority`
- validate queue routing truth and active contributor linkage without touching
  the still-timing-sensitive barrier/turn seam
- stay entirely on one owner path: admitted input metadata, queue membership,
  current-run contributors, and stored run/boundary lineage

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `QueueContainsWrongHandlingMode`
    - `QueueAppearsInBothQueues`
    - `CurrentRunContributorRunMismatch`
    - `CurrentRunContributorPendingConsumptionMissingBoundary`
  - strengthened validation so:
    - `queue` entries must carry `HandlingMode::Queue`
    - `steer_queue` entries must carry `HandlingMode::Steer`
    - the same input may not appear in both queues
    - current-run contributors must carry `last_run_id == current_run_id`
    - contributors already in `AppliedPendingConsumption` must have a recorded
      `last_boundary_sequence`
  - extended the focused negative joined-snapshot test to exercise these cases
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the queued-prompt runtime-spine test to assert the live
    snapshot carries `handling_mode = Queue`

Why this slice matters:

- these are all real runtime-ingress facts already owned by the authority:
  queue routing, per-input handling mode, `last_run`, and boundary sequence are
  not shell folklore
- it tightens one of the recurring seam classes directly: "an input is in the
  queue but not semantically queued the same way everywhere"
- it also gives the joined machine a better model of run-linkage without
  reaching into the still-riskier turn/boundary timing seam

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`
- `cargo test -p meerkat --lib capture_meerkat_machine_snapshot_joins_runtime_spine_with_live_turn_state`

Result:

- `MeerkatMachine` now validates more of the ingress-routing and contributor
  lineage truth that the runtime already owns canonically
- the live runtime-spine and joined Meerkat tests stayed green, which is the
  right sign that these invariants are observing current owner-backed truth
  rather than inventing a replacement ahead of time

Backtracks encountered:

- the only code-level backtrack was an unqualified `InputLifecycleState`
  reference in the validator; that was a plain implementation miss, not a
  semantic retreat
- the slice itself was intentionally narrower than several tempting ingress
  ideas; I did **not** promote prompt/content-shape folklore or timing-sensitive
  boundary claims that are not yet justified by the live owner path

Cutover gate read:

- this improves `MeerkatMachine` observability and executable validation, but
  it does **not** move `MeerkatMachine` past the cutover gate
- ingress write authority is still in the existing runtime authority and
  semantic freeze is still incomplete

Next likely step:

- either take another same-owner ingress/runtime slice, or move sideways to a
  different Meerkat region before revisiting any timing-sensitive cross-crate
  joins

## Slice 42 - MobMachine tracked run branch map shape

Goal:

- continue the durable tracked-run path in `MobMachine`
- promote one more step-level kernel map only if the persisted run aggregate
  proves it really survives there
- validate branch membership shape without dragging in actor cleanup timing or
  runtime execution details

What landed:

- `meerkat-mob/src/run.rs`
  - added typed `MobRun::step_branches()` reader over the persisted
    `step_branches` kernel map
  - extended the kernel-reader test to assert `None` branch shape for a simple
    persisted run
  - added a focused negative parse test for invalid non-string/non-`None`
    branch entries
- `meerkat-mob/src/mob_machine.rs`
  - extended `TrackedRunStoreSnapshot` with `step_branches`
  - extended tracked-run validation so:
    - every ordered step must have a branch-map entry
    - branch-map keys may only reference ordered steps
  - extended the synthetic tracked-run validator test with missing/unknown
    branch-map violations
- `meerkat-mob/src/runtime/tests.rs`
  - extended the live flow-accounting snapshot test so a single-step live run
    must surface `step_branches = { start -> None }`

Why this slice matters:

- branch membership is part of real flow semantics: it drives branch-winner and
  join behavior, so it is not just authoring metadata
- the live tracked-run snapshot proved that the field survives in persisted run
  truth, which is exactly the cutover-gate bar we want before promoting it into
  the machine
- it keeps us on the durable flow-kernel path instead of drifting into actor
  cleanup timing or setup-only configuration

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_mob_run_step_branches_reject_invalid_entry`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`

Result:

- `MobMachine` now validates another persisted step-level flow-kernel map
  without relying on actor cleanup timing
- the live flow-accounting snapshot stayed green, which is the useful signal
  that `step_branches` is real tracked-run truth rather than config echo

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the meaningful backtrack was still the selection discipline itself: the field
  only got promoted after the stored run aggregate and live snapshot path made
  it clear that the branch map actually persists as kernel truth

Cutover gate read:

- this improves `MobMachine` observability and executable validation, but it
  does **not** move `MobMachine` past the cutover gate
- tracked-run write authority is still in the existing runtime/store/kernel
  stack, and semantic freeze is still incomplete

Next likely step:

- either keep deepening clearly persisted tracked-run maps in the same style,
  or move sideways to another owner-backed Mob region if the next candidate no
  longer clears the durable-truth bar

## Slice 43 - MeerkatMachine ingress completeness and queue ownership

Goal:

- tighten the ingress side from the admitted-input perspective instead of only
  validating queue entries
- make sure admitted inputs still carry the authority-owned metadata that makes
  queue placement meaningful
- close the reverse seam where an input can be semantically `Queued` but no
  longer appear in its owning ingress lane

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `AdmittedInputMissingContentShape`
    - `AdmittedInputMissingHandlingMode`
    - `AdmittedInputMissingLifecycle`
    - `QueuedInputMissingOwningQueue`
  - strengthened validation so:
    - every admitted input must still carry `content_shape`
    - every admitted input must still carry `handling_mode`
    - every admitted input must still carry `lifecycle`
    - any input whose lifecycle is `Queued` must appear in its owning queue
      based on `handling_mode`
  - extended the focused negative joined-snapshot test to exercise missing
    metadata and missing queue membership
- `meerkat-runtime/src/session_adapter.rs`
  - strengthened the live queued-prompt runtime-spine test so it now asserts:
    - the input is present in `inputs.queue`
    - `inputs.steer_queue` stays empty
    - `content_shape` is populated

Why this slice matters:

- the first ingress slices only validated the queue-to-input direction; this
  slice closes the opposite direction
- `content_shape`, `handling_mode`, and `lifecycle` are not shell convenience
  fields; they are canonical ingress facts used to classify boundaries and
  queue ownership
- "queued but not actually owned by a queue anymore" is exactly the kind of
  silent drift the joined machine is meant to catch before cutover

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_queued_prompt_input`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates ingress completeness from both sides:
  queue entries must point to the right admitted input state, and queued inputs
  must remain owned by the right ingress lane
- the live runtime-backed snapshot path stayed green, which is the important
  sign that the slice is observing current authority truth rather than
  projecting future semantics

Backtracks encountered:

- the only backtrack was patch-shape drift while extending the negative test
  fixture; there was no semantic retreat
- I deliberately did **not** turn this into wake/process-flag validation,
  because current control/ingress signaling still carries timing asymmetry that
  has not cleared the same-owner stability bar

Cutover gate read:

- this still leaves `MeerkatMachine` on the observability side of the gate
- the slice strengthens owner-backed ingress truth, but it does not change
  write authority or freeze the remaining timing-sensitive lifecycle seams

Next likely step:

- another same-owner `MeerkatMachine` slice is still possible, but the best
  candidates are now narrower and more selective; anything timing-sensitive
  should continue to wait

## Slice 44 - MobMachine tracked run branch-condition coherence

Goal:

- promote the first real branch semantic in `MobMachine`, not just another map
  presence check
- use stored run truth that already survives into the tracked-run snapshot
- verify the branch semantics against both a synthetic malformed run and a live
  branch flow

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchWithoutCondition`
  - strengthened tracked-run validation so:
    - any step carrying a non-`None` branch label must also carry
      `step_has_conditions[step_id] = true`
  - extended the synthetic tracked-run validator fixture to exercise a branch
    label on a step that claims `step_has_conditions = false`
- `meerkat-mob/src/runtime/tests.rs`
  - added a live branch-flow snapshot test:
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
  - the test runs the real `branching` flow, captures a joined `MobMachine`
    snapshot while the run is still tracked, and asserts:
    - the declared ordered step sequence survives
    - branch steps carry `step_has_conditions = true`
    - the `repair` branch label survives on both branch members

Why this slice matters:

- this is the first tracked-run invariant that ties two persisted kernel maps
  together to express real flow semantics
- it is grounded in current branch-spec rules, where branch-labelled steps are
  expected to be condition-bearing steps
- the live branch-flow snapshot is important evidence that this is not just a
  synthetic store-shape rule

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates a real persisted branch semantic rather than only
  independent per-step field presence
- the live branch-flow snapshot stayed green, which is the key signal that the
  branch/condition relationship survives into current tracked-run truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the main selection discipline was the real guardrail here: I chose a
  branch-condition rule that current spec/runtime behavior already enforces,
  instead of promoting more setup-shaped flow config just because it persists

Cutover gate read:

- `MobMachine` also remains firmly on the observability side of the gate
- the slice strengthens tracked-run semantics, but write authority still lives
  in the current flow kernel / store / actor stack

Next likely step:

- the next honest `MobMachine` slice should either relate another pair of
  persisted tracked-run maps semantically, or move sideways to a different
  owner-backed Mob region if the next flow field looks too config-shaped

## Slice 45 - MeerkatMachine ingress boundary metadata shape

Goal:

- strengthen `MeerkatMachine` with one more same-owner ingress invariant
- validate that boundary metadata still reflects the real `RuntimeIngress`
  transition shape instead of drifting into shell folklore
- keep the slice inside the runtime-owned input ledger and away from the
  still timing-sensitive multi-crate joins

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `InputBoundarySequenceWithoutRun`
    - `InputBoundarySequenceIllegalLifecycle`
  - strengthened ingress validation so:
    - `last_boundary_sequence` must imply `last_run_id`
    - queued or staged inputs must not already carry a boundary marker
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_ingress_boundary_shape_violations`

Why this slice matters:

- `BoundaryApplied` is not an annotation convenience; it is a specific ingress
  transition that only occurs after the run is staged and bound
- that means boundary metadata is a good same-owner truth surface for the
  observational machine: it should never float free of run ownership or appear
  on pre-boundary queued/staged work
- this is exactly the kind of low-noise invariant that helps us converge
  toward semantic freeze without accidentally encoding timing folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ingress_boundary_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another real ingress-owned semantic relation,
  not just field presence and queue membership
- the live runtime-backed snapshot path stayed green, which is the important
  sign that the rule matches current owner behavior rather than target-state
  wishful thinking

Backtracks encountered:

- there was no code-level semantic backtrack in the final slice
- the useful design backtrack was selection itself: I did **not** force a
  stronger boundary/current-run invariant that would have crossed into the
  known timing-sensitive join surface

Cutover gate read:

- this is still an observability-only strengthening slice
- `MeerkatMachine` is not ready to switch; the ownership model is converging,
  but the remaining multi-region lifecycle seams are still not frozen

Next likely step:

- another same-owner `MeerkatMachine` slice is still possible, but candidates
  are now increasingly selective; timing-sensitive joins should keep waiting

## Slice 46 - MobMachine branch-group dependency coherence

Goal:

- promote the next real branch semantic in `MobMachine`
- mirror the existing flow spec rule that branch-group members must share the
  same dependency set
- verify it both synthetically and against the live branch-flow snapshot path

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchConflictingDependencies`
  - strengthened tracked-run validation so:
    - steps sharing the same non-`None` branch label must resolve to the same
      dependency set (set comparison, not insertion-order comparison)
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live branch-flow snapshot test so it now also asserts the
    two `repair` branch members preserve matching dependencies on `start`

Why this slice matters:

- this is a better branch semantic than simply adding more persisted fields;
  it checks a relationship the flow spec already treats as meaningful
- the rule is grounded in stored tracked-run truth, not just create-run input
  config
- using set comparison keeps the validator aligned with existing spec
  semantics rather than overfitting to field ordering

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates a second real branch-group semantic relation over
  persisted tracked-run state
- the live branch-flow path stayed green, which is the important sign that the
  dependency-coherence rule already survives into current runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the meaningful guardrail was again selection discipline: I only promoted a
  branch-group rule that is already present in current spec validation, rather
  than inventing a new branch semantic for the tracked-run snapshot

Cutover gate read:

- `MobMachine` also remains on the observability side of the gate
- the tracked-run semantic surface is getting richer, but ownership is still
  clearly in the current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either extend branch-group truth
  one notch further, or pivot to another persisted flow semantic if the next
  branch rule looks too config-shaped

## Slice 47 - MeerkatMachine steer-processing intent coherence

Goal:

- promote one more same-owner ingress rule into `MeerkatMachine`
- verify that `process_requested` keeps its runtime-owned meaning rather than
  drifting into shell folklore
- prove the rule against a live steered admission path, not only a synthetic
  validator mutation

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `ProcessRequestedWithoutWake`
    - `ProcessRequestedWithoutSteerQueue`
  - strengthened ingress validation so:
    - `process_requested` must imply `wake_requested`
    - `process_requested` must imply non-empty `steer_queue`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_process_request_shape_violations`
- `meerkat-runtime/src/session_adapter.rs`
  - added a live adapter test:
    `meerkat_machine_spine_snapshot_tracks_steered_prompt_input`
  - the test drives a real prompt with `RuntimeTurnMetadata.handling_mode =
    Steer` through `accept_input_with_completion(...)` and asserts:
    - queue is empty
    - steer queue owns the input
    - `wake_requested = true`
    - `process_requested = true`
    - the admitted input snapshot preserves `HandlingMode::Steer`

Why this slice matters:

- `process_requested` is not just a convenience flag; the ingress authority
  sets it only for steer admissions that should be processed at the earliest
  admissible boundary
- that makes it a good same-owner truth surface for the observational machine:
  it should not survive without wake intent or without steer-owned work
- adding the live adapter test keeps the validator grounded in actual runtime
  behavior instead of synthetic snapshot expectations

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_steered_prompt_input`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_process_request_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates a real steer-processing semantic relation and
  has a live runtime-backed proof path for it
- this strengthens the ingress region without stepping into the known
  timing-sensitive control/runtime handoff seam

Backtracks encountered:

- there was no semantic backtrack in the final slice
- the important selection discipline was choosing `process_requested`, which is
  purely ingress-owned, instead of forcing a stronger wake/control join that
  still has timing asymmetry

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- the ownership model is still converging, but this slice is a clean example
  of owner-backed semantics becoming explicit without changing write authority

Next likely step:

- the next honest `MeerkatMachine` slice should again be same-owner and
  selective; if the next candidate crosses timing-sensitive joins, it should
  wait

## Slice 48 - MobMachine `DependencyMode::Any` branch-dependency semantics

Goal:

- promote another persisted flow semantic into `MobMachine`
- mirror the existing flow spec rule that `depends_on_mode = any` only makes
  sense when at least one dependency is branch-labelled
- verify it synthetically and against the live branch-flow tracked-run path

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunAnyDependencyModeWithoutBranchDependency`
  - strengthened tracked-run validation so:
    - any step with `DependencyMode::Any` must have at least one dependency
      whose tracked branch label is non-`None`
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_any_dependency_mode_without_branch_dependency`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened the live branch-flow snapshot test so it now also asserts:
    - `summarize` preserves `DependencyMode::Any`
    - the branch flow's dependency map still references the branch-labelled
      repair steps

Why this slice matters:

- it promotes a real semantic relation the current flow spec already enforces
- the rule is expressed entirely in persisted tracked-run truth:
  `step_dependency_modes`, `step_dependencies`, and `step_branches`
- this is exactly the sort of relation we want before cutover: not merely that
  fields exist, but that they still encode the semantics they are supposed to
  carry

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_any_dependency_mode_without_branch_dependency`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates another persisted branch/join semantic over
  tracked-run state
- the live branch-flow path stayed green, which is the key sign that the rule
  already survives into current runtime truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was again selection discipline: I promoted a rule that
  already exists in spec validation, rather than inventing a new tracked-run
  branch meaning

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- the flow semantic surface is getting richer, but ownership is still clearly
  in the current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either deepen branch/join
  semantics one notch further or pivot to another persisted flow semantic if
  the next branch rule starts looking too config-shaped

## Slice 49 - MeerkatMachine ingress run-binding coherence

Goal:

- promote another same-owner ingress lifecycle relation into `MeerkatMachine`
- validate that active boundary-phase input states still carry the run/boundary
  metadata the ingress authority is supposed to own
- stay inside the runtime ingress ledger and avoid the timing-sensitive
  multi-crate joins

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `InputLifecycleMissingRunBinding`
    - `AppliedPendingConsumptionMissingBoundarySequence`
  - strengthened ingress validation so:
    - `Staged` and `AppliedPendingConsumption` inputs must carry `last_run_id`
    - `AppliedPendingConsumption` inputs must carry
      `last_boundary_sequence`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_ingress_run_binding_shape_violations`

Why this slice matters:

- the ingress authority does not invent `Staged` or
  `AppliedPendingConsumption` in isolation; those states come from specific
  run-bound transitions
- that makes run binding and boundary metadata part of the semantic truth for
  those lifecycle states, not optional annotations
- this is a good observational-kernel slice because it strengthens owner truth
  without reaching into the unstable control/runtime timing seam

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ingress_run_binding_shape_violations`
- `cargo test -p meerkat --lib`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo check -p meerkat --lib --features comms`
- `git diff --check`

Result:

- `MeerkatMachine` now validates another real ingress-owned lifecycle relation
  rather than only field presence or queue ownership
- the rule is grounded directly in the current ingress authority transitions:
  `StageDrainSnapshot` and `BoundaryApplied`

Backtracks encountered:

- there was no semantic backtrack in the final slice
- the useful guardrail was again slice selection: I avoided stronger
  current-run/coordinator joins that would have crossed into the known
  timing-sensitive boundary

Cutover gate read:

- `MeerkatMachine` remains in observability mode
- the ownership model is still converging, but this slice is another example
  of machine truth becoming explicit without changing write authority

Next likely step:

- the next honest `MeerkatMachine` slice should remain same-owner and
  selective; if it needs timing or cross-owner reconstruction, it should wait

## Slice 50 - MobMachine branch-group cardinality

Goal:

- promote the next persisted branch-group semantic into `MobMachine`
- mirror the current flow spec rule that a branch group must contain at least
  two members
- verify it synthetically while keeping the live branch-flow path green

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - extended `MobMachineInvariantViolation` with:
    - `TrackedRunBranchGroupTooSmall`
  - strengthened tracked-run validation so:
    - each non-`None` branch label must appear on at least two steps
  - added a focused negative test:
    `validate_mob_machine_snapshot_reports_branch_group_too_small`

Why this slice matters:

- branch groups are not just labels; the current spec already treats their
  cardinality as semantic truth
- this rule is expressed entirely in persisted tracked-run state through
  `step_branches`, which is exactly the kind of owner-backed relation we want
  before cutover
- it complements the earlier condition and dependency checks without inventing
  new branch meaning

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_group_too_small`
- `cargo test -p meerkat-mob --lib`
- `git diff --check`

Result:

- `MobMachine` now validates another branch-group semantic directly over
  tracked-run truth
- the broader mob test lane remained green, which is the important sign that
  the rule aligns with current runtime behavior

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was again selection discipline: I promoted an existing
  spec rule instead of inventing a new tracked-run branch heuristic

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- the flow semantic surface is richer, but ownership is still clearly in the
  current flow kernel / actor / store stack

Next likely step:

- the next honest `MobMachine` slice should either continue branch/join
  semantics if the rule is already spec-backed, or pivot to a different
  persisted flow truth surface

## Slice 51 - MeerkatMachine wake-request / queued-work coherence

Goal:

- promote one more same-owner ingress rule into `MeerkatMachine`
- validate that `wake_requested` is not carried without any queued work
- stay inside `RuntimeIngressAuthority` semantics rather than crossing into
  the timing-sensitive control/runtime wake seam

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `WakeRequestedWithoutQueuedWork`
  - strengthened ingress validation so:
    - `wake_requested` requires at least one queued input in either `queue` or
      `steer_queue`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_wake_without_queued_work`

Why this slice matters:

- in the current ingress authority, `wake_requested` is raised only when queued
  work exists and is cleared when the current drain snapshot starts
- that makes `wake_requested` part of the ingress-owned "work is pending"
  carrier, not a free-floating hint
- this is exactly the kind of conservative observational-kernel rule we want:
  it strengthens machine truth without trying to prove the larger wake/control
  handoff too early

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_wake_without_queued_work`

Result:

- `MeerkatMachine` now validates another explicit ingress-owned relation
- the new rule is grounded directly in the current `RuntimeIngressAuthority`
  transitions rather than target-state cleanup

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the main restraint was not reaching for the stronger but still timing-shaped
  reverse rule (`queued work implies wake_requested`)

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice strengthens owner-backed truth but does not change write authority

Next likely step:

- the next honest `MeerkatMachine` slice should again stay same-owner and avoid
  crossing into the unstable multi-crate barrier/turn join

## Slice 52 - MobMachine live collection-policy durability

Goal:

- strengthen `MobMachine` against the live codebase without inventing another
  tracked-run rule too early
- prove that collection-policy kind and quorum threshold really survive into the
  tracked-run snapshot for a runtime-backed flow

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
  - the new test runs a real quorum collection flow and asserts the joined
    `MobMachine` snapshot preserves:
    - ordered steps
    - dependency map
    - dependency mode
    - branch/condition flags
    - collection policy kind
    - quorum threshold

Why this slice matters:

- the validator already checks collection-policy / quorum-threshold shape, but
  the honest question was whether those facts are truly durable in the current
  run aggregate
- this slice answers that with a live runtime-backed proof instead of guessing
- it keeps the Mob side moving while avoiding a speculative new semantic rule
  that might still be config folklore

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`

Result:

- `MobMachine` now has live evidence that quorum collection-policy truth
  survives into tracked-run snapshots, not just synthetic validator coverage

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate choice here was methodological: use a live durability proof
  rather than promote a new tracked-run invariant before the persistence shape
  had proved itself

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in existing tracked-run facts, but it is still
  validation scaffolding rather than switched write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another rule only if
  it is clearly persisted and spec-backed, or continue strengthening live
  runtime-backed proofs for the facts already in the validator

## Slice 53 - MeerkatMachine reverse contributor coverage

Goal:

- strengthen the ingress run-binding surface in `MeerkatMachine`
- validate the reverse direction of current-run contributor ownership:
  run-bound contributor-compatible inputs must still appear in the current run
  contributor set
- stay entirely inside the ingress authority's own state instead of crossing
  into the timing-sensitive turn/runtime boundary

What landed:

- `meerkat/src/meerkat_machine.rs`
  - extended `MeerkatMachineInvariantViolation` with:
    - `CurrentRunBoundInputMissingContributor`
  - strengthened ingress validation so:
    - any admitted input whose `last_run_id` matches `current_run_id` and whose
      lifecycle is contributor-compatible (`Staged`, `Applied`, or
      `AppliedPendingConsumption`) must appear in
      `current_run_contributors`
  - added a focused negative test:
    `validate_meerkat_machine_snapshot_reports_missing_run_bound_contributor`

Why this slice matters:

- we were already validating `contributors -> inputs`, but not the reverse
  relation
- in the current ingress authority, contributor-compatible run-bound inputs are
  the canonical membership set for the active drain snapshot
- that makes this a real ownership rule, not just a cleanup preference

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_missing_run_bound_contributor`

Result:

- `MeerkatMachine` now validates a fuller ingress-owned picture of run-bound
  contributor truth

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the useful guardrail was keeping the rule purely ingress-owned rather than
  overreaching into turn-execution state

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- the rule strengthens owner-backed truth without moving write authority

Next likely step:

- the next honest `MeerkatMachine` slice should keep targeting same-owner
  ingress/ops/drain facts until the remaining gaps are mostly cross-crate joins

## Slice 54 - MobMachine live `Any` collection-policy durability

Goal:

- keep strengthening `MobMachine` against the live codebase without promoting a
  speculative new tracked-run invariant
- prove that the `CollectionPolicy::Any` variant survives into the joined
  tracked-run snapshot with the expected kind/threshold shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
  - the new test runs a real `CollectionPolicy::Any` flow and asserts the
    joined `MobMachine` snapshot preserves:
    - `RunCollectionPolicyKind::Any`
    - zero quorum threshold

Why this slice matters:

- the validator already checks the generic shape relation for collection policy
  kinds and quorum thresholds
- this slice adds live evidence for the `Any` variant specifically, complementing
  the existing quorum-backed durability proof
- that is the cautious way to move the Mob side forward while the next semantic
  promotion is still under evaluation

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`

Result:

- `MobMachine` now has live runtime-backed evidence for both quorum and any
  collection-policy durability in tracked-run snapshots

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate choice here was again methodological: extend live durability
  evidence before promoting another tracked-run law

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in existing tracked-run facts, but it is still
  diagnostic scaffolding rather than switched write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another clearly
  persisted and spec-backed rule, or continue widening live proof coverage for
  the tracked-run facts already modeled

## Slice 55 - MeerkatMachine wait-all carrier coherence

Goal:

- expose one more runtime-owned fact inside the Meerkat ops slice without
  crossing a timing-sensitive join
- make the hidden `wait_all` carrier visible so the joined validator can catch
  authority/carrier drift instead of only watching aggregate wait metadata

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - extended `RuntimeOpsDiagnosticSnapshot` with `pending_wait_present`
  - sourced it directly from `ShellState.pending_wait.is_some()`
- `meerkat-runtime/src/meerkat_machine.rs`
  - extended `MeerkatOpsSnapshot` with `pending_wait_present`
- `meerkat-runtime/src/session_adapter.rs`
  - threaded the new field into the runtime spine snapshot
  - strengthened the live `wait_all` snapshot test to require the carrier while
    the wait future is active
- `meerkat/src/meerkat_machine.rs`
  - added two new invariants:
    - `PendingWaitCarrierWithoutWaitRequest`
    - `WaitRequestMissingPendingWaitCarrier`
  - extended the focused negative ops test to prove both sides of the pairing

Why this slice matters:

- `wait_all` installs the authority-owned wait request and the shell-owned
  pending wait sender under the same ops registry write lock
- that makes this a good same-owner slice for the cutover gate: stronger than a
  pure aggregate count, but still grounded in stable current ownership
- it also makes one more hidden carrier explicit before we start talking about
  switch readiness

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_runtime_ops_state`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_wait_all_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`

Result:

- `MeerkatMachine` now validates the exact pairing between active `wait_all`
  authority and the pending wait carrier

Backtracks encountered:

- the useful backtrack happened before code landed: I rejected a stronger
  ingress lifecycle/current-run rule because recovery can legitimately clear
  `current_run` while keeping applied inputs
- the wait-all carrier slice was chosen specifically because it avoids that
  timing/recovery trap

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger owner-backed truth, not a write-authority move

Next likely step:

- keep targeting same-owner Meerkat regions until the remaining gaps are mostly
  cross-crate joins or explicit switch preparation work

## Slice 56 - MobMachine live `All` collection-policy durability

Goal:

- finish live durability coverage for the full `{All, Any, Quorum}` tracked
  collection-policy set
- improve confidence in existing tracked-run policy/quorum rules without
  promoting a speculative new invariant

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - added a live tracked-run test:
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
  - the new test runs a real `CollectionPolicy::All` flow and asserts the
    joined `MobMachine` snapshot preserves:
    - `RunCollectionPolicyKind::All`
    - zero quorum threshold

Why this slice matters:

- the validator already enforces the generic shape relation between collection
  policy kind and quorum threshold
- this closes the remaining live proof gap for the three concrete policy
  variants actually stored in tracked run truth
- that is still the safer Mob move while the next semantic promotion is under
  evaluation

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`

Result:

- `MobMachine` now has live runtime-backed durability evidence for
  `CollectionPolicy::{All, Any, Quorum}`

Backtracks encountered:

- there was no code-level backtrack in the final slice
- the deliberate methodological choice was to prefer a live proof over another
  new tracked-run law until the next persisted relation is clearly worth
  promoting

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice raises confidence in already-modeled tracked-run truth without
  changing ownership or write authority

Next likely step:

- the next honest `MobMachine` slice should either promote another clearly
  persisted and spec-backed relation, or continue widening live proof coverage
  where the semantic promotion bar is not met yet

## Slice 57 - MeerkatMachine wait-all request ID pairing

Goal:

- strengthen the `wait_all` carrier slice without leaving the same-owner ops
  boundary
- prove that the shell-owned pending wait carrier not only exists, but tracks
  the same `WaitRequestId` as the authority-owned wait request

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - extended `RuntimeOpsDiagnosticSnapshot` with `pending_wait_request_id`
  - sourced it directly from `ShellState.pending_wait.wait_request_id`
- `meerkat-runtime/src/meerkat_machine.rs`
  - extended `MeerkatOpsSnapshot` with `pending_wait_request_id`
- `meerkat-runtime/src/session_adapter.rs`
  - threaded the new field into the runtime spine snapshot
  - strengthened the live `wait_all` snapshot test to require exact ID
    agreement between the carrier and authority
- `meerkat/src/meerkat_machine.rs`
  - added new invariants:
    - `PendingWaitCarrierShapeMismatch`
    - `PendingWaitCarrierRequestMismatch`
  - extended the focused negative ops test to prove request-ID drift is caught

Why this slice matters:

- this is the same semantic seam as Slice 55, but one level stronger
- the carrier and authority live under the same registry write lock, so exact
  ID agreement is real current truth rather than a speculative cross-crate join
- it turns the hidden wait sender from a boolean presence marker into a typed
  obligation we can reason about during cutover

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_tracks_wait_all_state`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_cross_region_violations`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now validates exact agreement between the active `wait_all`
  request and the pending wait carrier request ID

Backtracks encountered:

- there was no code-level backtrack in the final Meerkat slice
- the guardrail was methodological: stay on the same-owner ops path instead of
  reaching for a stronger but timing-sensitive ingress or barrier invariant

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is still convergence work over current owner truth, not a switch to
  write authority

Next likely step:

- keep mining same-owner Meerkat slices until the remaining unknowns are mostly
  explicit cross-crate joins or cutover-preparation work

## Slice 58 - MobMachine healthy flow failure-counter durability

Goal:

- move the Mob side one step forward on tracked failure-counter truth without
  promoting a speculative new invariant
- verify that the joined tracked-run snapshot carries stable zeroed failure
  counters on a healthy live flow

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
  - the active tracked-run assertion now requires:
    - `failure_count == 0`
    - `consecutive_failure_count == 0`

Why this slice matters:

- the tracked-run snapshot already carries both counters, and the validator
  already knows their basic ordering relation
- this adds live joined-snapshot evidence on the stable non-failing path rather
  than inventing a new law or overreaching into cleanup timing
- it keeps the Mob side moving while respecting the cutover gate’s “current
  owner truth first” rule

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_flow_accounting_coherence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live tracked-run durability evidence that a healthy
  active flow keeps both failure counters cleared

Backtracks encountered:

- I first tried a stronger live proof on a retrying failing flow
- that timed out waiting for failure counters to remain visible on the joined
  tracked-run path, which is a good signal that the current tracker/store join
  is not yet stable enough for that proof
- I backed that out and replaced it with the safer healthy-flow durability
  assertion instead of weakening the timing model or forcing a flaky test

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice improves confidence in an already-modeled fact while explicitly
  recording where the failure-counter proof surface is still too timing-shaped

Next likely step:

- the next honest `MobMachine` slice should either prove another stable tracked
  fact on the healthy path, or revisit failure counters only after the
  tracker/store lifetime is modeled more explicitly

## Slice 59 - MeerkatMachine legacy detached-wake latch consistency

Goal:

- tighten one real Meerkat carrier seam instead of only observing it
- make the legacy detached-wake latch match its documented meaning:
  `signaled` means “wake sent and not yet consumed”

What landed:

- `meerkat-runtime/src/runtime_loop.rs`
  - introduced `clear_legacy_detached_wake_signal(...)`
  - the legacy idle wake arm now clears both `pending` and `signaled` after
    successfully injecting the continuation input
  - added a focused unit test proving the helper resets both flags
- `meerkat/src/meerkat_machine.rs`
  - added `DetachedWakeSignaledWithoutPending`
  - the joined validator now rejects the stale latch state
    `detached_wake_pending == false && detached_wake_signaled == true`
  - extended the focused negative ops test to prove that stale legacy state is
    now treated as invalid

Why this slice matters:

- this is not just a prettier observer rule; it corrects a real implementation
  inconsistency in the legacy detached-wake path
- before this slice, the runtime loop could consume a legacy detached wake and
  leave `signaled = true`, which no longer matched the documented meaning of
  the latch
- the validator tightening is only justified because the implementation now
  excludes that stale state

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-runtime --lib clear_legacy_detached_wake_signal_resets_pending_and_signaled`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_ops_shape_violations`

Result:

- the legacy detached-wake carrier is now internally self-consistent
- `MeerkatMachine` can safely reject a stale signaled-without-pending snapshot

Backtracks encountered:

- the interesting backtrack here was architectural, not syntactic
- while tracing the carrier I noticed the feed-backed runtime does not actually
  use the legacy `signaled` latch on the active path, which is why I kept this
  slice narrowly scoped to legacy self-consistency instead of inventing a
  broader live invariant too early

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice improves a real carrier seam, but it does not yet move write
  authority under the joined machine

Next likely step:

- either refine another same-owner Meerkat seam such as completion-waiter
  carrier shape, or explicitly map how detached wake should behave under the
  feed-backed path before promoting stronger live invariants there

## Slice 60 - MobMachine branch-flow collection-policy durability

Goal:

- keep the Mob side moving without repeating the retry/cleanup timing mistake
- prove that branch flows preserve full tracked collection-policy state, not
  just branch/dependency shape

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
  - the live branch-flow snapshot must now also preserve:
    - default `RunCollectionPolicyKind::All` for every branch-flow step
    - zero quorum thresholds for every non-quorum branch-flow step

Why this slice matters:

- we already had live proofs for collection-policy durability on single-step
  `{All, Any, Quorum}` flows
- this extends that evidence to a multi-step branch flow, which is a better
  proxy for real flow-kernel shape than another isolated policy demo
- it keeps the Mob side on persisted tracked-run truth rather than retry or
  cleanup timing

Verification:

- `cargo fmt --all`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`

Result:

- `MobMachine` now has live durability evidence that branch flows preserve both
  branch semantics and collection-policy kernel state together

Backtracks encountered:

- there was no new code-level backtrack in the final Mob slice
- the restraint was methodological: use branch-flow durability proof rather
  than force a new tracked-run invariant on a timing-sensitive path

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this slice adds breadth to persisted tracked-run evidence, but it does not
  change the underlying authority boundary yet

Next likely step:

- keep the Mob side on durable tracked-run facts until the remaining gaps are
  clearly semantic rather than cleanup-shaped

## Slice 61 - MeerkatMachine detached-wake owner proof at the ops registry seam

Goal:

- complement the legacy detached-wake runtime-loop fix with a same-owner proof
  at the registry seam
- verify that a terminal `BackgroundToolOp` arms pending wake state without
  mutating the legacy `signaled` latch directly

What landed:

- `meerkat-runtime/src/ops_lifecycle.rs`
  - added `background_terminal_sets_detached_wake_pending_without_signaled_latch`
  - the test wires a real `DetachedWakeState` into the registry, completes a
    `BackgroundToolOp`, and asserts:
    - `pending == true`
    - `signaled == false`

Why this slice matters:

- Slice 59 fixed consumption semantics in the legacy runtime loop
- this slice proves the producer side of the same carrier
- together they make the detached-wake latch understandable at both ends:
  ops lifecycle arms pending, runtime loop consumes and clears the full latch

Verification:

- `cargo test -p meerkat-runtime --lib background_terminal_sets_detached_wake_pending_without_signaled_latch`

Result:

- `MeerkatMachine` now has direct owner-side evidence for how the detached-wake
  carrier is produced before the joined validator reasons about it

Backtracks encountered:

- there was no code-level backtrack in the final Meerkat slice
- the discipline was choosing a same-owner registry proof instead of reaching
  for a broader live feed-path invariant that the current runtime does not yet
  justify

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is stronger owner-backed evidence, not a switch of write authority

Next likely step:

- continue only on Meerkat seams whose producer and consumer meanings can both
  be grounded in current owner code

## Slice 62 - MobMachine completed-run step-status contract invariant

Goal:

- mirror one more existing `FlowRunMachine` contract invariant inside the joined
  `MobMachine`
- use only fields already projected from the durable tracked-run kernel state

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunCompletedRunHasNonCompletedStep`
  - `MobMachine` now rejects `MobRunStatus::Completed` runs that still report a
    step status other than `Completed` or `Skipped`
  - extended the focused synthetic tracked-run validator harness to prove the
    new violation is surfaced

Why this slice matters:

- the generated flow-run contract already says completed runs must contain only
  completed or skipped steps
- this slice promotes a genuine machine invariant, not just another persisted
  field
- it uses stable tracked-run state we already project (`status` +
  `step_statuses`) without reaching into cleanup timing

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_tracked_run_kernel_shape_violations`

Result:

- `MobMachine` now mirrors one more formal flow-run invariant using current
  durable run truth

Backtracks encountered:

- no code-level backtrack in the final Mob slice
- the methodological choice was to add a contract-derived invariant rather than
  another policy or retry-shaped live proof

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this strengthens semantic convergence, but the current tracked-run boundary
  is still read/validate only

Next likely step:

- keep pulling contract-derived tracked-run invariants into `MobMachine` so
  long as they only depend on already-projected durable state

## Slice 63 - MeerkatMachine recycle preserves live completion waiters and drops stale ones

Goal:

- make the recycle/recovery seam explicit in the Meerkat spine instead of
  leaving it only in detached control-plane tests
- prove both sides of completion-waiter reconciliation:
  - active queued waiters survive recycle
  - stale waiter entries are terminated and removed

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle`
    to prove the joined Meerkat spine still shows the active queued waiter
    after `recycle()`
  - added
    `meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters`
    to seed one stale internal waiter, recycle the runtime, and prove the
    stale waiter is terminated with `"recycled input no longer pending"` while
    the live queued waiter survives

Why this slice matters:

- recycle/recovery is a high-value cutover seam because it is where carrier
  truth and ledger truth can silently diverge
- the adapter already had the reconciliation logic; this slice promotes that
  logic into the observable MeerkatMachine build instead of trusting it
  implicitly

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has direct owner-backed evidence that recycle preserves
  live completion-waiter linkage and reconciles stale waiter carrier state

Backtracks encountered:

- no machine-level backtrack was needed after the slice direction was chosen
- the only deliberate compromise was seeding the stale waiter directly through
  the completion registry because the public API does not currently expose a
  way to manufacture that recovery-only state

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens a recovery seam but does not move write authority

Next likely step:

- keep choosing Meerkat slices at lifecycle handoff seams where producer and
  consumer truth are already explicit in current owner code

## Slice 64 - MobMachine live proof of branch-group durability

Goal:

- strengthen the live branch-flow tracked-run proof without inventing a new
  invariant
- confirm that branch-group facts already enforced by the validator survive the
  real tracked-run projection path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    assert:
    - the `repair` branch group still contains both branch members
    - both members preserve the same dependency set in tracked-run state

Why this slice matters:

- the branch-group invariants already exist in `MobMachine`, but they were
  still mostly justified by synthetic harnesses
- this adds a live durability proof that the real tracked-run snapshot carries
  the same branch membership and dependency coherence the validator expects

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that branch-group structure
  survives the real flow-run projection path

Backtracks encountered:

- no code-level backtrack was needed in the final Mob slice
- the restraint was intentionally keeping this as a live proof instead of
  promoting another new invariant before the tracked-run durability bar clearly
  justified it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this increases confidence in current tracked-run durability without shifting
  write authority

Next likely step:

- continue with durable tracked-run facts or lifecycle seams that can be proved
  against live owner state without leaning on cleanup timing

## Slice 65 - MeerkatMachine recover reconciles completion waiters

Goal:

- test whether `recover()` handles completion waiters with the same carrier
  discipline as `recycle()`
- use the Meerkat spine to distinguish “current behavior we should preserve”
  from “hidden lifecycle bug we need to fix”

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover`
    to prove active queued waiters survive a recover handoff
  - added
    `meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
    to seed one stale waiter and check that recover drops it
  - fixed `RuntimeSessionAdapter::recover()` so it now reconciles completion
    waiters against `active_input_ids()` and terminates stale entries with
    `"recovered input no longer pending"`

Why this slice matters:

- `CompletionRegistry::resolve_not_pending()` already documented recovery as a
  valid reconciliation site, but the adapter only used it for recycle
- the new stale-waiter test failed red first, which means this slice closed a
  real lifecycle bug instead of merely adding another observer

Verification:

- red proof before the fix:
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
- green after the fix:
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters`
  - `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover`
  - `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that recover preserves live
  completion waiters and clears stale carrier entries instead of letting them
  hang indefinitely

Backtracks encountered:

- this slice intentionally started test-first and failed red
- the failure was useful and precise: recover left a stale waiter visible in
  the spine (`input_count == 2` instead of `1`)
- the final fix reused the recycle reconciliation pattern rather than inventing
  a new recovery-only path

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is exactly the kind of lifecycle seam bug we want to burn down before
  considering a switch

Next likely step:

- keep selecting Meerkat slices where lifecycle handoff and carrier cleanup can
  be expressed through current owner code and tested red-to-green

## Slice 66 - MobMachine live proof of any-join branch durability

Goal:

- strengthen the live branch-flow proof around the `depends_on_mode=any` join
  step without promoting a new invariant prematurely

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    prove that the `summarize` join step:
    - still has at least one dependency in tracked-run state
    - depends only on branch-labelled steps

Why this slice matters:

- `MobMachine` already validates the synthetic rule that `DependencyMode::Any`
  requires at least one branch-backed dependency
- this slice shows that the real tracked-run projection keeps enough durable
  branch information for that rule to be meaningful in live state

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that the any-join branch rule
  survives the real tracked-run projection path

Backtracks encountered:

- no code-level backtrack in the final Mob slice
- the deliberate restraint was keeping this as a live durability proof instead
  of promoting another invariant before the tracked-run boundary justified it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in current tracked-run durability without shifting
  write authority

Next likely step:

- keep `MobMachine` focused on persisted tracked-run facts or live durability
  proofs that do not depend on cleanup timing

## Slice 67 - MeerkatMachine preserves active wait_all across recover

Goal:

- prove that `recover()` preserves the active `wait_all` carrier coherently
  when the same ops registry remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recover`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives recover
    - the shell-owned pending wait carrier survives recover
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target operation
      completes

Why this slice matters:

- this is the ops-lifecycle analogue of the completion-waiter recovery work
- it verifies a real lifecycle handoff at the Meerkat seam without inventing a
  new owner or a new reducer path
- the current code path says recover replays the runtime driver but keeps the
  same live ops registry, so the observer needs to prove that the wait carrier
  remains coherent under that ownership model

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recover`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives recover coherently instead of drifting across the authority/shell
  seam

Backtracks encountered:

- the detached-wake reset/destroy idea was intentionally not promoted into a
  rule because it was not yet clear that the current attachment lifecycle made
  that a single-owner semantic fact
- switching to `wait_all` was the better call because the ownership story was
  already explicit in the ops authority

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this slice strengthens lifecycle-handoff confidence, but it does not change
  write authority

Next likely step:

- keep selecting Meerkat seams where lifecycle handoff can be expressed
  directly through current owner code and where a green result actually reduces
  cutover risk

## Slice 68 - MobMachine live proof of active tracked-run step status coverage

Goal:

- strengthen the live tracked-run proof around step-status durability without
  depending on the cleanup-sensitive terminal visibility window

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    prove that the active tracked branch run:
    - surfaces at least one materialized step status
    - never exposes a step-status entry outside the ordered-step universe

Why this slice matters:

- the branch-flow tracked run already exposes durable ordered-step truth
- this slice proves that the live status projection stays inside kernel-known
  step identity without assuming full coverage or terminal visibility

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active tracked runs preserve
  materialized step-status identity across the joined snapshot

Backtracks encountered:

- the first attempt tried to prove completed-run terminal status shape through
  the joined snapshot
- that went red for a good reason: terminal tracked runs do not remain visible
  long enough to be a stable proof surface before local cleanup drains them
- the second attempt tried to prove full step-status coverage for all ordered
  steps while the run was still active
- that also went red: active tracked runs only materialize status entries for
  the steps that have actually entered the ledger so far
- the final slice backed off to the active-run window and to the smaller fact
  the current code really owns: materialized status keys stay inside the
  ordered-step universe

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run status durability without moving
  write authority

Next likely step:

- continue selecting Mob slices that either prove existing invariants against
  live tracked-run state or surface new durable facts only after that live
  proof exists

## Slice 69 - MeerkatMachine preserves active wait_all across recycle

Goal:

- prove that `recycle()` preserves the active `wait_all` carrier coherently
  when the same ops registry remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives recycle
    - the shell-owned pending wait carrier survives recycle
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes

Why this slice matters:

- this is the recycle-side counterpart to the recover `wait_all` handoff proof
- it verifies another Meerkat lifecycle transition where driver state is
  rebuilt while the ops registry remains the owner of the active wait request

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives both recover and recycle coherently instead of drifting across the
  authority/shell seam

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was not promoting a reset/destroy rule before the current
  ownership semantics made that boundary explicit

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle-handoff confidence, but it does not change write
  authority

Next likely step:

- keep selecting Meerkat seams where lifecycle handoff is explicit in current
  owner code and where a green result materially reduces cutover risk

## Slice 70 - MobMachine live proof of healthy collection-policy run durability

Goal:

- strengthen the live tracked-run proof for collection-policy flows without
  assuming terminal visibility or full active-step coverage

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape` to
    prove that the active tracked collection-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- the healthy single-step tracked flow already showed zeroed failure counters;
  this extends the same durability proof to a tracked run that also carries
  collection-policy metadata
- it stays on currently materialized tracked-run truth instead of assuming more
  status coverage than the live projection actually guarantees

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy collection-policy tracked
  runs preserve both zeroed failure counters and kernel-bounded materialized
  step-status identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- this slice deliberately reused the smaller active-run status identity fact
  learned from the previous branch-flow backtracks

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in collection-policy tracked-run durability without
  moving write authority

Next likely step:

- keep extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth

## Slice 71 - MeerkatMachine preserves active wait_all after reset

Goal:

- prove that `reset()` preserves the active `wait_all` carrier coherently
  when ops lifecycle remains the canonical owner

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_reset`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives reset
    - the shell-owned pending wait carrier survives reset
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes

Why this slice matters:

- this completes the current Meerkat lifecycle trio for active `wait_all`
  carrier survival across recover, recycle, and reset
- it stays grounded in present owner code: reset clears queued input work, but
  it does not replace the ops lifecycle registry

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_reset`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that active `wait_all` state
  survives the current reset path coherently instead of drifting across the
  authority/shell seam

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was intentional: this stays observational about current reset
  semantics rather than asserting a stronger desired policy for destroy

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves lifecycle-handoff coverage, but it does not change
  write authority

Next likely step:

- keep selecting Meerkat lifecycle seams where current owner behavior is clear
  enough to prove without inventing target-state semantics

## Slice 72 - MobMachine live proof of healthy any-policy run durability

Goal:

- strengthen the live tracked-run proof for `CollectionPolicy::Any` without
  assuming more status coverage than the current projection materializes

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
    to prove that the active tracked any-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - surfaces at least one materialized step status
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- it extends the same healthy tracked-run durability proof to another
  collection-policy variant without widening the semantic claims
- it reuses the smaller active-run status identity fact that the previous
  branch-flow backtracks established as stable

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_any_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy any-policy tracked runs
  preserve zeroed failure counters and kernel-bounded materialized step-status
  identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- the deliberate restraint was keeping this on healthy active-run durability
  instead of reopening terminal or full-coverage assumptions

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without moving write
  authority

Next likely step:

- continue extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth

## Slice 89 - MeerkatMachine proves attached-loop reset preserves wait_all while abandoning queued work

Goal:

- cover the remaining attached-loop lifecycle seam where `reset()` abandons
  queued work but should still preserve the ops-owned `wait_all` carrier

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `reset()` abandons the queued input (`inputs_abandoned == 1`)
    - runtime phase returns to `Idle`
    - the authority-owned `wait_request_id` survives
    - the shell-owned pending wait carrier survives with the same
      `WaitRequestId`
    - the tracked wait target list stays intact until the operation settles
    - queued attached-loop work is bypassed without calling executor `apply()`
      or `control()`

Why this slice matters:

- it closes the last obvious attached-loop `wait_all` gap across the current
  Meerkat lifecycle handoff matrix
- it sharpens current semantics instead of guessing target-state behavior:
  attached-loop reset behaves like idle reset for ops waiters, even while it
  eagerly abandons queued input work

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives `recover`, `recycle`, `reset`, `stop`, `retire`, and
  `destroy`, even though input-completion waiters do not survive all of those
  same seams

Backtracks encountered:

- no code backtrack in the final slice
- the restraint was to preserve only the current owner truth and not assume
  anything stronger about executor control delivery on reset

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle-handoff coverage, but it is still observational
  rather than write-authoritative

Next likely step:

- keep selecting lifecycle seams where input waiters and ops waiters diverge,
  because those continue to surface real current-owner semantics

## Slice 90 - MobMachine strengthens live root-frame tracked-run durability

Goal:

- extend the active root-frame proof from coarse structure (`frame_count > 0`)
  to the durable tracked-run step maps that should already survive on the
  frame-aware single-step path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
    so the live root-frame tracked run must now preserve:
    - `ordered_steps == [start]`
    - `step_dependencies == { start: [] }`
    - `step_dependency_modes == { start: All }`
    - `step_has_conditions == { start: false }`
    - `step_branches == { start: None }`
    - `step_collection_policy_kinds == { start: All }`
    - `step_quorum_thresholds == { start: 0 }`
  - retained the existing healthy-run structural checks:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - zeroed failure counters
    - no terminal completion timestamp

Why this slice matters:

- it proves the frame-aware path carries the same kernel-owned step maps as the
  simpler non-frame single-step flow, rather than only surfacing frame counts
- it stays on durable tracked-run truth and avoids the timing-sensitive output
  or status surfaces that previously failed the live-truth bar

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active root-frame tracked runs
  preserve their durable step/dependency/kernel-policy maps, not just their
  frame presence

Backtracks encountered:

- no new code backtrack in the final slice
- the deliberate restraint was to strengthen only clearly durable tracked-run
  structure, not step-status timing or output projection

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in the tracked-run kernel shape without changing
  canonical write authority

Next likely step:

- continue promoting only tracked-run facts that the live frame-aware path can
  sustain across active snapshots without timing folklore

## Slice 91 - MeerkatMachine proves attached-loop recycle preserves wait_all through replay

Goal:

- cover the attached-loop `recycle()` seam where the adapter preserves queued
  work, restores `Attached`, and wakes the loop again to replay that work

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `recycle()` transfers the queued input (`inputs_transferred == 1`)
    - the pending wait carrier and authority-owned `wait_request_id` survive
    - queued work is replayed by the attached executor exactly once
    - runtime phase temporarily re-enters `Running` while replay is in flight
    - runtime returns to `Attached` once replay finishes
    - the wait target list remains intact until the operation itself settles
    - no executor `control()` call is used on the recycle path

Why this slice matters:

- it fills in the attached-loop recycle seam, which is one of the last major
  lifecycle handoffs not yet represented in the Meerkat observation matrix
- it captures current replay semantics instead of flattening recycle into a
  simpler idle-only story

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives recycle and remains coherent across the replay phase and
  the return to `Attached`

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was to assert only the current replay-phase transitions, not a
  broader target-state story about how recycle should behave after cutover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves lifecycle-handoff coverage, but it is still an
  observational proof over current owner code

Next likely step:

- keep filling the remaining attached-loop lifecycle matrix only where the
  current runtime actually owns a distinct seam

## Slice 92 - MobMachine backs off root-frame step-status timing and keeps durable root-frame truth

Goal:

- try to strengthen the active root-frame proof with materialized step-status
  visibility, but keep only what the live tracked-run path actually sustains

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - attempted to require non-empty `step_statuses` for the active root-frame
    snapshot
  - the live path failed that requirement immediately
  - backed the slice down to a smaller durable proof on the same root-frame
    run:
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
    - plus the already-proven frame-aware step maps and healthy-run counters

Why this slice matters:

- it gives a clean negative signal: active root-frame runs still do not own
  materialized step-status strongly enough to treat it as cutover-grade truth
- it keeps progress honest by strengthening only the durable scalars that the
  tracked-run store actually preserves on the root-frame path

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that active root-frame tracked
  runs preserve default retry/escalation kernel fields, while step-status
  timing remains explicitly non-authoritative on this path

Backtracks encountered:

- the first version of the slice failed because the active root-frame run did
  not surface any materialized step status
- the correct response was to remove that claim instead of forcing the runtime
  into a cleaner observer model

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is exactly the kind of backtrack the gate is meant to surface:
  boundaries stay stable, but not every representable fact is owner-backed yet

Next likely step:

- continue promoting only root-frame facts that survive the live tracked-run
  path without depending on timing-sensitive step-status materialization

## Slice 93 - MeerkatMachine proves attached-loop recover preserves wait_all and returns to Attached

Goal:

- cover the remaining attached-loop `recover()` seam where queued work is
  preserved and replayed through a still-live executor loop

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop`
  - proved that on an attached runtime with queued work and a live background
    operation:
    - `recover()` reports `inputs_recovered == 1`
    - the authority-owned `wait_request_id` survives
    - the shell-owned pending wait carrier survives with the same request id
    - the attached executor replays recovered queued work exactly once
    - runtime phase temporarily re-enters `Running` while replay is in flight
    - runtime returns to `Attached` after replay completes
    - the tracked wait target list remains intact until the operation settles
    - no executor `control()` call is used on the recover path

Why this slice matters:

- it closes another major attached-loop lifecycle seam in the Meerkat
  observation matrix
- more importantly, it exposed a real semantic correction:
  the first hypothesis was that recover would land in `Idle`, but the live
  owner path proved that attached-loop recover returns to `Attached`

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop
  `wait_all` survives recover, replay, and the return to `Attached`

Backtracks encountered:

- the first version of the slice expected recover replay to land in `Idle`
- the live runtime disproved that; the correct stable phase after replay is
  `Attached`
- this was a good semantic backtrack, not a flaky test failure

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is strong convergence signal because the boundary did not move, but we
  were still learning real lifecycle semantics from current code

Next likely step:

- continue selecting attached-loop lifecycle seams only where the live runtime
  still has distinct current behavior we have not yet frozen

## Slice 94 - MobMachine extends healthy loop durability with default retry/escalation scalars

Goal:

- strengthen the active loop-path proof with durable scalar fields that should
  survive even though active loop step-status timing still does not

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `max_step_retries == 0`
    - `escalation_threshold == 0`
  - kept the existing durable loop-path truths:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - zeroed failure counters
    - no terminal completion timestamp

Why this slice matters:

- it mirrors the successful root-frame scalar proof onto the loop path
- it keeps Mob progress on durable tracked-run truth and avoids reintroducing
  timing-shaped claims about active loop status materialization

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has stronger live evidence that healthy active loop runs
  preserve default retry/escalation kernel fields as well as loop/frame
  structure

Backtracks encountered:

- none in the final loop slice
- the important restraint was choosing durable loop scalars instead of trying
  to promote active loop step-status timing again

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without changing canonical
  write authority

Next likely step:

- continue promoting only loop-path facts that survive the live tracked-run
  path without depending on materialized active step-status timing

## Slice 95 - MeerkatMachine proves attached-loop recover preserves completion waiters through replay

Goal:

- cover the attached-loop `recover()` seam for input-owned completion waiters,
  not just ops-owned `wait_all`, so the input-vs-ops split stays explicit in
  the lifecycle matrix

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop`
  - proved that on an attached runtime with queued progress work and a live
    completion waiter:
    - `recover()` reports `inputs_recovered == 1`
    - the completion-waiter carrier survives while the attached loop replays
      recovered work
    - runtime phase temporarily re-enters `Running` during replay
    - the attached executor replays the recovered queued work exactly once
    - no executor `control()` call is used on the recover path
    - once replayed work completes, the completion waiter resolves and the
      carrier clears
    - runtime returns to `Attached` after replay completes

Why this slice matters:

- it fills the completion-waiter side of the attached-loop recover seam, which
  was still missing even after the `wait_all` side was covered
- it keeps the lifecycle map honest by showing that recover preserves
  input-owned waiters long enough to drain recovered work, rather than
  flattening them into stop/reset/destroy behavior

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recover
  preserves completion waiters during replay and clears them only after the
  recovered work actually completes

Backtracks encountered:

- none in the final slice
- the restraint was to mirror only currently owned replay semantics, not to
  invent a broader target-state story for recover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another strong convergence slice because it extended current-owner
  lifecycle truth without moving any owner boundary

Next likely step:

- continue filling only the remaining attached-loop lifecycle seams where
  completion waiters and ops waiters still diverge in current code

## Slice 96 - MobMachine proves live loop runs preserve durable step maps

Goal:

- strengthen the active loop-path proof with durable tracked-run step maps,
  but only if the live flow path sustains them without depending on active
  step-status timing

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `ordered_steps == [body]`
    - empty dependency map for `body`
    - `DependencyMode::All` for `body`
    - `step_has_conditions[body] == false`
    - no branch label for `body`
    - `RunCollectionPolicyKind::All` for `body`
    - zero quorum threshold for `body`
  - kept the already-proven loop-path truths:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - zeroed failure counters
    - default retry/escalation scalars
    - no terminal completion timestamp

Why this slice matters:

- it upgrades the loop path from “structure exists” to “durable step kernel
  maps survive on the live loop path,” matching what the root-frame path was
  already proving
- it still stays on durable tracked-run truth and avoids reintroducing the
  active step-status timing claims that previously failed

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy active loop runs preserve
  their durable step/dependency/kernel-policy maps, not just loop/frame
  counters

Backtracks encountered:

- none in the final slice
- the key restraint was to strengthen only the durable step maps and leave
  active loop step-status timing out of scope

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is a good convergence slice because it strengthens durable tracked-run
  truth without expanding the boundary or inventing a cleaner runtime than the
  live code actually owns

Next likely step:

- continue promoting only active loop facts that the tracked-run store
  actually preserves across live snapshots without timing folklore

## Slice 97 - MeerkatMachine proves attached-loop recycle preserves completion waiters through replay

Goal:

- cover the attached-loop `recycle()` seam for input-owned completion waiters,
  not just ops-owned `wait_all`, so recycle is represented on both sides of
  the input-vs-ops split

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop`
  - proved that on an attached runtime with queued progress work and a live
    completion waiter:
    - `recycle()` reports `inputs_transferred == 1`
    - the completion-waiter carrier survives while the attached loop replays
      preserved work
    - runtime phase temporarily re-enters `Running` during replay
    - the attached executor replays preserved work exactly once
    - no executor `control()` call is used on the recycle path
    - once replayed work completes, the completion waiter resolves and the
      carrier clears
    - runtime returns to `Attached` after replay completes

Why this slice matters:

- it fills the completion-waiter side of the attached-loop recycle seam, which
  was still missing even after the `wait_all` path was covered
- it confirms that recycle behaves like recover here: input-owned waiters stay
  live long enough to drain preserved work instead of being flattened into the
  stop/reset/destroy class

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop recycle
  preserves completion waiters during replay and clears them only once the
  preserved work actually completes

Backtracks encountered:

- none in the final slice
- the key discipline was to mirror only the current replay behavior already
  owned by the adapter and executor seam

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens the lifecycle matrix again, but it is still observational
  coverage over current owner code rather than a switch to new write authority

Next likely step:

- continue filling only the remaining attached-loop lifecycle seams where
  completion waiters and ops waiters still diverge in current code

## Slice 98 - MobMachine proves active loop runs preserve durable identity and running status

Goal:

- take one small active-loop tracked-run step that stays durable and avoids the
  step-status timing surfaces that have already failed us

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so the active loop tracked run must now preserve:
    - `flow_id == demo`
    - `status == Running`
  - kept the already-proven loop-path truths:
    - `schema_version == 4`
    - frame/loop/iteration structure
    - durable step/dependency/kernel-policy maps
    - zeroed failure counters
    - default retry/escalation scalars
    - no terminal completion timestamp

Why this slice matters:

- it adds a first small live proof that the active loop path preserves durable
  run identity and current non-terminal status, not just structural counters
- it stays well clear of the timing-shaped active step-status materialization
  that previously forced backtracks

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active loop runs preserve durable
  `flow_id` identity and stay in `Running` while the never-terminal body keeps
  the run alive

Backtracks encountered:

- none in the final slice
- the important restraint was choosing durable run identity/state rather than
  another materialized step-status claim

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in active tracked-run truth without changing the
  canonical write boundary

Next likely step:

- continue promoting only active-loop facts that survive live snapshots
  without depending on cleanup timing or step-status folklore

## Slice 99 - MeerkatMachine proves attached-loop retire splits completion and wait-all lifetimes

Goal:

- capture the current attached-loop `retire()` seam with both an input-owned
  completion waiter and an ops-owned `wait_all` live at the same time, so the
  split is proved directly instead of inferred from separate tests

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
  - proved that on an attached runtime with queued progress work, a live
    completion waiter, and an active background operation under `wait_all`:
    - `retire()` reports `inputs_pending_drain == 1`
    - runtime phase temporarily re-enters `Running` while the attached loop
      drains preserved work
    - the completion-waiter carrier survives through that replay/drain phase
    - the authority-owned `wait_request_id` and pending wait carrier also
      survive through that same phase
    - no executor `control()` call is used on the retire path
    - once the preserved queued work finishes, completion waiters clear and
      runtime returns to `Retired`
    - the ops-owned `wait_all` carrier remains live after that point and only
      clears once the background operation itself settles

Why this slice matters:

- it is the clearest direct proof so far that current Meerkat lifecycle
  really does split input waiters from ops waiters on the same attached-loop
  retire path
- it moves us from “separate tests imply a split” to “the joined model sees
  the split happen in one live scenario”

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop retire
  clears input-owned completion waiters when drained work completes, while
  preserving ops-owned `wait_all` until the underlying operation settles

Backtracks encountered:

- none in the final slice
- the important discipline was keeping the test entirely on current owner
  seams rather than using it to “normalize” retire into a simpler lifecycle

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is strong convergence signal because it deepens the lifecycle matrix
  without changing any owner boundary

Next likely step:

- continue selecting only lifecycle seams where current input-owned and
  ops-owned behavior still diverge in a way the joined machine needs to freeze

## Slice 100 - MobMachine proves active branch runs preserve durable identity and running status

Goal:

- take the same small durable run-identity/state step on the active branch
  path that already held on the active loop path

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
    so the active branch tracked run must now preserve:
    - `flow_id == branching`
    - `status == Running`
  - kept the already-proven branch-path truths:
    - ordered steps
    - condition flags
    - branch labels and group membership
    - dependency/dependency-mode maps
    - collection policy kind and quorum threshold maps
    - materialized step-status keys remain inside the ordered-step universe

Why this slice matters:

- it upgrades the branch path from pure structural durability to a small but
  useful active-run identity/state proof
- it stays on durable tracked-run truth and avoids the timing-sensitive branch
  completion or cleanup surfaces

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that active branch runs preserve durable
  `flow_id` identity and remain in `Running` while delayed branch work is
  still in flight

Backtracks encountered:

- none in the final slice
- the key restraint was choosing durable active-run identity/state, not adding
  another timing-shaped branch status claim

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in active tracked-run truth without moving the
  canonical write boundary

Next likely step:

- continue promoting only active-branch facts that survive live snapshots
  without depending on cleanup timing or terminal visibility

## Slice 87 - MeerkatMachine makes attached-loop reset semantics explicit

Goal:

- pin down what current `reset()` does when an attached runtime still has
  queued work and completion waiters, without assuming it behaves like stop,
  destroy, or retire

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can carry queued work plus a live completion waiter
      before reset
    - `reset()` abandons that queued work immediately
    - completion waiters are cleared immediately
    - the waiter resolves with `RuntimeTerminated("runtime reset")`
    - runtime control phase returns to `Idle`
    - current reset does not call the attached executor's `apply()` or
      `control()` hooks on this path

Why this slice matters:

- it closes the last obvious missing attached-loop completion-waiter lifecycle
  seam alongside stop, retire, and destroy
- it makes current reset semantics explicit instead of inferred:
  reset is a hard queued-work teardown, but it returns to `Idle` rather than
  `Stopped` or `Destroyed`

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop reset
  bypasses executor control and clears queued-input completion waiters
  immediately while returning the runtime to `Idle`

Backtracks encountered:

- no semantic backtrack was required in the final slice
- the useful restraint was checking the control authority before asserting the
  post-reset phase, rather than guessing from the other lifecycle paths

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is another concrete lifecycle read, not a write-authority switch signal

Next likely step:

- keep selecting attached-loop lifecycle seams where current owner behavior is
  still easy to misread from analogy alone

## Slice 88 - MobMachine strengthens the live root-frame proof with healthy structured state

Goal:

- extend the existing active root-frame tracked-run proof using only facts the
  live aggregate already sustains cleanly

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
  - the live proof now requires that an active root-frame tracked run preserves:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count == 0`
    - `loop_iteration_count == 0`
    - `failure_count == 0`
    - `consecutive_failure_count == 0`
    - `completed_at_present == false`

Why this slice matters:

- it broadens the healthy active-run durability proof outside collection and
  loop flows, without introducing any new tracked-run field
- it confirms that root-frame structured state and zeroed failure counters are
  stable owner-backed truth for `MobMachine`

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has a stronger active root-frame proof while still staying
  on non-timing-sensitive tracked-run facts

Backtracks encountered:

- no new red run in the final slice
- this was chosen specifically as a smaller structural proof after earlier loop
  and branch work showed where active step-status timing is still too soft

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run structural truth without moving write
  authority

Next likely step:

- continue preferring active-run structure and healthy counters over timing- or
  cleanup-shaped facts until the live path proves otherwise

## Slice 83 - MeerkatMachine makes attached-loop destroy semantics explicit

Goal:

- observe what current `destroy()` does to queued work and completion waiters
  when a runtime still has a live attached executor, without projecting target
  semantics onto the path

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can carry queued input plus a live completion waiter
      before destroy
    - `destroy()` abandons that queued input immediately
    - completion waiters are cleared immediately
    - the waiter future resolves with
      `RuntimeTerminated(\"runtime destroyed\")`
    - the runtime enters `RuntimeState::Destroyed`
    - current destroy does not call the attached executor's `apply()` or
      `control()` hooks on this path

Why this slice matters:

- it sharpens another lifecycle seam where attached-loop behavior could easily
  have been guessed wrong
- it gives the observational model a concrete answer for destroy with a live
  executor: current code bypasses executor control and tears down queued-input
  completion state synchronously

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop destroy is
  a hard teardown for queued input waiters, even though ops-owned `wait_all`
  remains a distinct lifecycle story

Backtracks encountered:

- no code-level backtrack was required in the final slice
- the important restraint was not assuming destroy would coordinate through the
  executor just because stop/retire sometimes do

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better picture of current destroy semantics, not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where attached-loop and no-loop paths
  genuinely diverge in current owner behavior

## Slice 84 - MobMachine backs out root-output count and stays on stable branch truth

Goal:

- test whether durable root output count belongs in the tracked-run MobMachine
  snapshot today, and back out cleanly if the live owner path does not support
  it

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - removed `root_output_count` from `TrackedRunStoreSnapshot`
  - removed the synthetic validator branch that treated root-output count as a
    tracked-run invariant
- `meerkat-mob/src/runtime/tests.rs`
  - restored
    `test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape` to
    wait for the smaller active-run truth the runtime actually surfaces today:
    materialized step status
  - removed the attempted durable-root-output proof after the live code showed
    it was not an owner-backed fact on this path

Why this slice matters:

- it is a clean example of the method working as intended: a synthetic fact
  parsed cleanly, but the real system did not actually own it strongly enough
  to promote into `MobMachine`
- backing it out avoids building a second, cleaner architecture on top of
  fields the live runtime does not yet sustain

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_branch_dependency_conflicts`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_branch_condition_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` keeps the stable active-branch proof surface
- root-output count is not treated as current tracked-run kernel truth

Backtracks encountered:

- the first live branch proof timed out waiting for active tracked runs to
  surface durable root outputs
- a direct durable-store proof then also failed: the terminal branch run did
  not preserve root outputs on this live path either
- that combination was the decision point to remove the slice instead of
  rationalizing it

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is a convergence improvement because it removes a false-positive fact
  from the observational model

Next likely step:

- continue preferring frame/loop structure and other clearly durable tracked-run
  facts over output durability until the live owner path proves otherwise

## Slice 85 - MeerkatMachine makes attached-loop destroy plus wait_all semantics explicit

Goal:

- pin down what current `destroy()` does when an attached runtime has both
  queued work and an active ops-owned `wait_all`, without assuming it behaves
  like stop or retire

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can hold queued input plus an active authority-owned
      wait request at the same time
    - `destroy()` abandons the queued input immediately
    - `destroy()` preserves the ops-owned `wait_request_id`
    - the pending wait carrier remains present and keeps the same
      `WaitRequestId`
    - the tracked wait target remains visible until the operation settles
    - runtime control phase becomes `Destroyed`
    - current destroy bypasses both executor `apply()` and executor `control()`
      on this path

Why this slice matters:

- it closes a real lifecycle gap between the attached-loop completion-waiter
  destroy proof and the previously idle-only `wait_all` destroy proof
- it makes the current asymmetry explicit:
  - queued-input completion waiters are torn down immediately
  - ops-owned `wait_all` state survives destroy until the operation itself
    resolves

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop destroy
  behaves like idle destroy for `wait_all`, even though it hard-tears down
  queued-input completion waiters on the same path

Backtracks encountered:

- the first red run was only a harness/API mismatch:
  - wrong `OperationResult` fields
  - wrong `wait_all` assertion shape
- after aligning to the real API, the semantic claim held without further
  changes

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger picture of current destroy semantics, not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where current behavior is surprising
  enough that we should name it explicitly before any cutover decision

## Slice 86 - MobMachine keeps loop structure and backs off active step-status timing

Goal:

- strengthen the live loop-flow proof only as far as the current tracked-run
  path genuinely supports it

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the test now proves that an active loop-aware tracked run preserves:
    - `schema_version == 4`
    - `frame_count > 0`
    - `loop_count > 0`
    - `loop_iteration_count > 0`
    - `failure_count == 0`
    - `consecutive_failure_count == 0`
    - `completed_at_present == false`

Why this slice matters:

- it extends the healthy active-run durability proofs into the loop-aware flow
  path without introducing another new tracked-run field
- it confirms that loop structure and healthy failure counters are stable owner
  truth for `MobMachine`

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has a stronger live proof for active loop-aware tracked runs
  while staying on facts the aggregate actually sustains today

Backtracks encountered:

- the first version required active loop runs to surface materialized
  `step_statuses`
- that timed out on the live path, which told us active loop step-status timing
  is not a stable proof surface yet
- the final slice backed off cleanly to loop structure plus healthy failure
  counters instead of encoding timing folklore

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is convergence toward stable tracked-run truth, not a switch signal

Next likely step:

- continue preferring loop/frame structure and healthy-run counters over active
  step-status timing until the loop path proves otherwise

## Slice 75 - MeerkatMachine makes attached-loop retire plus wait_all semantics explicit

Goal:

- pin down the current `retire()` behavior when two different carriers are live
  at once:
  - attached-loop queued work that must drain
  - an ops-owned `wait_all` request that is still waiting on a background
    operation

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop`
  - the new test proves that:
    - retire preserves the authority-owned `wait_request_id`
    - retire preserves the shell-owned pending wait carrier and their shared
      `WaitRequestId`
    - retire reports `inputs_pending_drain == 1` when queued work is handed to
      the attached loop
    - control phase temporarily re-enters `Running` while that queued work is
      draining
    - once the attached loop finishes draining, control returns to `Retired`
      even though the ops-owned `wait_all` is still active
    - the wait carrier only clears after the tracked operation itself settles

Why this slice matters:

- it exposes a real lifecycle asymmetry instead of assuming retire is a single
  phase transition
- it shows that the current runtime already treats attached-loop drain and
  ops-owned waits as distinct semantic surfaces during retire

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  can pass through:
  - `Attached`
  - temporary `Running` drain
  - back to `Retired`
  while preserving active `wait_all` state until the tracked operation itself
  completes

Backtracks encountered:

- no code-level backtrack in the final slice
- the key design choice was to avoid inventing a target retire policy and only
  record the exact handoff the current owners implement

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better map of current retire semantics, not a switch-ready write
  authority

Next likely step:

- keep selecting Meerkat lifecycle seams where attached-loop and non-loop
  behavior diverge, because that is where current semantic truth is still most
  likely to surprise us

## Slice 76 - MobMachine makes loop presence imply frame structure

Goal:

- promote one more durable tracked-run structural fact without stepping into
  timing-sensitive flow behavior

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `TrackedRunLoopWithoutFrameStructure`
  - `validate_mob_machine_snapshot(...)` now rejects tracked runs that expose
    persisted loop state without any persisted frame structure
  - added
    `validate_mob_machine_snapshot_reports_loop_without_frame_structure`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the live loop-path proof now waits for and asserts:
    - `loop_count > 0`
    - `frame_count > 0`
    - `loop_iteration_count > 0`

Why this slice matters:

- it turns loop presence from an isolated counter into a structural relation
  over the durable run aggregate
- it keeps the Mob side moving through persisted frame/loop truth rather than
  timing-shaped active execution behavior

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_loop_without_frame_structure`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has explicit evidence that persisted loop state does not
  float on its own in the current durable aggregate; it remains anchored to
  persisted frame structure

Backtracks encountered:

- no model backtrack in the final slice
- the important restraint was keeping this purely structural and not promoting
  broader loop execution semantics before the durable owner path is tighter

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this is still diagnostic read-side convergence, not a write-authority cutover

Next likely step:

- continue promoting only those Mob tracked-run facts that stay durable and
  structurally visible through the live aggregate path

## Slice 77 - MeerkatMachine makes attached-loop stop plus wait_all semantics explicit

Goal:

- pin down current `stop_runtime_executor()` behavior when an attached loop has:
  - queued work that has not started applying yet
  - an active ops-owned `wait_all` request over a background operation

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop`
  - the new test proves that:
    - attached-loop stop preserves the authority-owned `wait_request_id`
    - attached-loop stop preserves the shell-owned pending wait carrier and
      request-id agreement
    - stop preempts queued attached-loop work before `apply()` runs
    - the attached executor observes exactly one
      `StopRuntimeExecutor` control command
    - active `wait_all` still resolves normally once the tracked operation
      settles

Why this slice matters:

- it fills the gap between:
  - existing non-loop `wait_all` stop coverage
  - existing attached-loop completion-waiter stop coverage
- it gives us a clearer Meerkat lifecycle map for attached stop semantics:
  queued work and ops waits do not behave the same way

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that attached-loop stop:
  - can preempt queued work before apply
  - does not cancel active `wait_all`
  - eventually publishes `RuntimeState::Stopped`
    while preserving the wait carrier until the target operation completes

Backtracks encountered:

- the first version of the test assumed `stop_runtime_executor()` would publish
  `Stopped` synchronously on the attached-loop path
- the red run showed that this was wrong: when a live control sender exists,
  stop queues the control command and returns before the phase necessarily
  flips
- I backtracked the test to wait for the control-phase publication instead of
  weakening the semantic claim about preserved `wait_all`

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a better map of current attached-loop stop semantics, not a
  switch-ready write authority

Next likely step:

- keep selecting Meerkat lifecycle seams where loop-attached behavior differs
  from no-loop behavior, because those continue to reveal the highest-value
  current semantics

## Slice 78 - MobMachine surfaces schema version for structured tracked-run state

Goal:

- promote one more durable tracked-run fact that helps distinguish legacy run
  shape from modern frame/loop-aware run shape without stepping into cleanup or
  active-execution timing

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - `TrackedRunStoreSnapshot` now carries `schema_version`
  - `tracked_run_snapshot_from_run(...)` now projects `MobRun.schema_version`
  - added
    `TrackedRunStructuredStateWithLegacySchemaVersion`
  - validator now rejects tracked runs that expose frame/loop/loop-iteration
    structure while still claiming legacy schema state
  - added
    `validate_mob_machine_snapshot_reports_structured_state_with_legacy_schema_version`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the live loop-path proof now also asserts `schema_version == 4`

Why this slice matters:

- it turns schema version from passive storage metadata into an explicit
  structural fact in the joined MobMachine view
- it gives us one more durable guardrail against accidentally treating modern
  frame/loop state as if it belonged to a legacy aggregate shape

Verification:

- `cargo test -p meerkat-mob --lib validate_mob_machine_snapshot_reports_structured_state_with_legacy_schema_version`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has explicit evidence that structured loop-aware tracked
  runs surface both:
  - persisted frame/loop structure
  - the modern schema version that owns that structure

Backtracks encountered:

- no semantic backtrack in the final slice
- the main care point was mechanical: every synthetic tracked-run fixture had
  to be updated so the new field stayed explicit rather than silently defaulted

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves durable tracked-run shape checking, but it is still not a
  write-authority cutover signal

Next likely step:

- continue preferring durable tracked-run structure or durable output shape over
  flow behavior that still depends on timing or cleanup visibility

## Slice 75 - MeerkatMachine makes retire-with-live-loop drain semantics explicit

Goal:

- observe the current completion-waiter handoff when `retire()` preserves
  queued work for an attached runtime loop to drain

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop`
  - the new test proves that:
    - an attached runtime can hold a queued completion waiter before retire
    - `retire()` preserves that queued work instead of abandoning it
    - the completion waiter carrier remains present across the handoff
    - the preserved work drains exactly once through the attached runtime loop
    - the completion waiter carrier clears only after the drained work settles

Why this slice matters:

- it closes the gap between the existing no-loop retire proof and the real
  attached-loop retire path
- it captures a subtle but important current behavior: retire on an attached
  runtime is not a steady `Retired` plateau while preserved work drains

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  hand preserved queued work to the live loop and keep the completion-waiter
  carrier intact until that work finishes

Backtracks encountered:

- the first version of the test assumed the snapshot would remain in
  `RuntimeState::Retired` immediately after retire
- that was false: once retire wakes the attached loop, current code
  temporarily re-enters `RuntimeState::Running` while draining preserved work,
  then returns to `Retired` after completion
- the test was updated to record that current semantic instead of forcing a
  cleaner but incorrect story

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this is a stronger lifecycle-handoff proof, but it is still describing
  current owner behavior rather than freezing target-state retire semantics

Next likely step:

- keep selecting Meerkat lifecycle seams where live-loop and no-loop behavior
  diverge, because those handoffs are where cutover risk is highest

## Slice 76 - MobMachine surfaces live loop-iteration ledger presence

Goal:

- promote one more durable structural fact from the flow-run aggregate without
  inventing a new behavioral invariant

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - `TrackedRunStoreSnapshot` now carries `loop_iteration_count`
  - tracked-run capture now maps `MobRun.loop_iteration_ledger.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - strengthened
    `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
    so it waits for and asserts both:
    - `loop_count > 0`
    - `loop_iteration_count > 0`

Why this slice matters:

- it moves `MobMachine` one step past “a loop exists” toward “the loop’s
  durable execution ledger is visible in the joined machine view”
- it stays structural and durable; it does not assume terminal loop outcomes or
  cleanup timing

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that the joined tracked-run view can see
  persisted loop-iteration ledger rows, not just loop snapshot presence

Backtracks encountered:

- no code-level backtrack in the final slice
- the useful restraint was not adding a new validator rule until the live loop
  path first proved the ledger is stably observable

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves structural coverage of tracked runs, but it does not yet imply
  write-authority readiness

Next likely step:

- continue preferring durable flow-run structure over cleanup-shaped behavior,
  especially for frames, loops, and iteration ledgers

## Slice 81 - MeerkatMachine maps retire-side carrier asymmetry explicitly

Goal:

- pin down current `retire()` semantics for input completion waiters and
  ops-owned `wait_all` without prematurely normalizing them into one policy

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop`
  - added `meerkat_machine_spine_snapshot_preserves_wait_all_after_retire`
  - the new tests prove that:
    - retiring a queued input with no live runtime loop abandons the input
      immediately
    - that same no-loop retire path clears completion waiters immediately
    - an active ops-owned `wait_all` survives retire
    - the authority-owned `wait_request_id` and shell-owned pending wait
      carrier stay aligned across retire
    - the surviving `wait_all` future still resolves once the tracked
      operation completes

Why this slice matters:

- it closes the remaining lifecycle gap in the current carrier map
- it makes the current asymmetry explicit:
  - input waiters are tied to queue-drain availability
  - ops waiters are tied to operation lifecycle authority instead

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop`
- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_retire`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current retire semantics
  split completion waiters and ops waiters when no runtime loop is present:
  queued input waiters terminate immediately, while active `wait_all` remains
  live until its operation settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was not trying to “fix” the asymmetry in the observer;
  this slice records current truth rather than target policy

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this materially improves the lifecycle map, but it still does not freeze the
  target retire policy for cutover

Next likely step:

- continue selecting lifecycle seams where two different carriers expose
  meaningfully different current-owner behavior

## Slice 82 - MobMachine surfaces tracked loop presence

Goal:

- take one small step from tracked run truth toward the eventual loop-machine
  boundary using a durable structural fact rather than another scalar setting

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `loop_count` to `TrackedRunStoreSnapshot`
  - populated it from durable `MobRun.loops.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - added `test_capture_mob_machine_snapshot_tracks_live_loop_presence`
  - the new test builds an active root-loop flow and proves that:
    - the tracked run becomes visible through the joined `MobMachine` snapshot
    - `loop_count > 0` once loop state is materialized
    - the tracked run is still non-terminal while that loop structure is
      visible

Why this slice matters:

- it is the loop counterpart to the earlier `frame_count` slice
- it gives the joined `MobMachine` view its first durable loop-structure fact
  without overclaiming full loop semantics

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_loop_presence`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `git diff --check`

Result:

- `MobMachine` now exposes one durable loop-structure fact through the joined
  tracked-run view, with live proof that an active root-loop flow actually
  carries it

Backtracks encountered:

- no code-level backtrack in the final slice
- the key choice was to prefer loop structure over another config-shaped field

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring small structural frame/loop facts when they can be observed
  cleanly from current durable state

## Slice 79 - MeerkatMachine preserves active wait_all after stop

Goal:

- pin down current stop semantics for ops-owned `wait_all` without smoothing
  them into the completion-waiter behavior

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor`
  - the new test proves that:
    - `stop_runtime_executor()` moves the runtime control phase to `Stopped`
    - the authority-owned `wait_request_id` survives stop
    - the shell-owned pending wait carrier survives stop
    - both sides still agree on the same `WaitRequestId`
    - the active `wait_all` future still resolves normally after the tracked
      operation completes

Why this slice matters:

- it sharpens a real lifecycle asymmetry in current code:
  - stop clears completion waiters immediately
  - stop does not currently clear the ops lifecycle `wait_all` carrier
- that is exactly the sort of current-semantics fact we need frozen before
  cutover and TLA+ work

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed evidence that current stop semantics
  preserve active `wait_all` state until the tracked operation itself settles,
  even though completion waiters terminate immediately

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was not inventing a new stop policy just because the
  completion-waiter path looks tidier

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this improves the lifecycle map, but it does not yet imply the target stop
  policy is settled

Next likely step:

- continue selecting lifecycle seams where the producer and consumer sides of a
  carrier can both be stated from real current-owner behavior

## Slice 80 - MobMachine surfaces tracked root-frame presence

Goal:

- take one small step from flow-run scalar truth toward frame-owned structure
  without pretending `MobMachine` already owns full frame semantics

What landed:

- `meerkat-mob/src/mob_machine.rs`
  - added `frame_count` to `TrackedRunStoreSnapshot`
  - populated it from durable `MobRun.frames.len()`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
  - the new test builds a root-frame flow, captures the joined `MobMachine`
    snapshot while the run is active, and proves:
    - the tracked run becomes visible through the joined machine view
    - `frame_count > 0` once the root frame is materialized
    - the tracked run is still non-terminal while that frame state is visible

Why this slice matters:

- it is the first small bridge from the current tracked run aggregate toward
  the eventual frame-machine boundary
- unlike another stored scalar, `frame_count` gives us a structural foothold in
  the current durable run shape

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_root_frame_presence`
- `cargo test -p meerkat-mob --lib`
- `cargo test -p meerkat --lib`
- `git diff --check`

Result:

- `MobMachine` now exposes one durable frame-structure fact through the joined
  tracked-run view, with live proof that a root-frame flow actually carries it

Backtracks encountered:

- no code-level backtrack in the final slice
- the key choice was to prefer a small structural frame fact over another
  config-shaped scalar

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring structural tracked-run or frame-owned facts when they can be
  observed cleanly from current durable state

## Slice 75 - MeerkatMachine destroys completion waiters while abandoning queued work

Goal:

- close the missing destroy-side observational gap for completion waiters
- promote only the piece of destroy semantics the current control plane already
  owns explicitly

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy`
  - the new test proves that:
    - a queued completion waiter is visible before destroy
    - `destroy()` transitions runtime control to `Destroyed`
    - completion waiters are cleared immediately after destroy
    - the waiter resolves as `RuntimeTerminated("runtime destroyed")`
    - destroy abandons the queued input (`inputs_abandoned == 1`)
- `meerkat/src/meerkat_machine.rs`
  - added `DestroyedRuntimeStillHasCompletionWaiters`
  - the validator now rejects destroyed runtime snapshots that still carry
    completion waiters
  - added
    `validate_meerkat_machine_snapshot_reports_destroyed_completion_waiters`

Why this slice matters:

- it complements the earlier destroy-side `wait_all` observation with the
  opposite carrier: input completion waiters terminate immediately, while
  ops-owned `wait_all` currently survives until the tracked operation settles
- it strengthens the Meerkat lifecycle map using current owner truth rather
  than target-state preference

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_destroyed_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed destroy semantics for both waiter
  carriers:
  - completion waiters terminate immediately on destroy
  - active `wait_all` currently survives destroy until the tracked op settles

Backtracks encountered:

- the first red run was useful: I initially asserted
  `DestroyReport.inputs_abandoned == 0`
- the real current behavior is `inputs_abandoned == 1` for the queued input,
  which matches driver semantics; the waiter-clearing part of the proof still
  held

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this strengthens lifecycle truth, but it still does not mean destroy policy
  is frozen for the eventual cutover

Next likely step:

- keep selecting Meerkat lifecycle seams where the concrete owner behavior is
  stable enough to distinguish “current truth” from “future desired policy”

## Slice 76 - MobMachine surfaces persisted max_step_retries in tracked runs

Goal:

- extend the tracked-run snapshot with one more durable kernel field without
  inventing a new cleanup-sensitive invariant

What landed:

- `meerkat-mob/src/run.rs`
  - added typed reader `MobRun::max_step_retries()`
  - extended the kernel reader unit test to prove the field parses correctly
- `meerkat-mob/src/mob_machine.rs`
  - added `max_step_retries` to `TrackedRunStoreSnapshot`
  - populated it from the persisted `MobRun`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`
  - the new live proof shows that a healthy retry-limited run preserves:
    - `max_step_retries == 2`
    - zeroed failure counters
    - at least one materialized step status
    - step-status keys bounded by `ordered_steps`

Why this slice matters:

- it moves `MobMachine` a bit further from pure status projection toward a
  fuller durable flow-kernel map
- it stays honest about the current proof surface by projecting a persisted
  limit rather than forcing retry/cleanup timing into the validator

Verification:

- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_retry_limit_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now carries the persisted retry budget through the joined
  tracked-run snapshot, and the live codebase confirms it survives the current
  runtime path

Backtracks encountered:

- no code-level backtrack in the final slice
- the main restraint was methodological: do not add a retry-behavior invariant
  yet, only the durable retry-budget fact

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- keep preferring durable tracked-run facts or live durability proofs over
  retry/cleanup semantics that still depend on timing

## Slice 77 - MeerkatMachine stop clears completion waiters immediately

Goal:

- extend the lifecycle map for completion waiters from destroy into the
  stop-runtime-executor path
- keep the slice grounded in the current direct owner path, not in hypothetical
  future stop semantics

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor`
  - the new test proves that:
    - a queued completion waiter is visible before stop
    - `stop_runtime_executor()` transitions runtime control to `Stopped`
    - completion waiters are cleared immediately after stop
    - the waiter resolves as `RuntimeTerminated("runtime stopped")`
- `meerkat/src/meerkat_machine.rs`
  - added `StoppedRuntimeStillHasCompletionWaiters`
  - the validator now rejects stopped runtime snapshots that still carry
    completion waiters
  - added
    `validate_meerkat_machine_snapshot_reports_stopped_completion_waiters`

Why this slice matters:

- it makes the current completion-waiter lifecycle map more explicit:
  - stop clears completion waiters immediately
  - destroy clears completion waiters immediately
  - recover/recycle preserve live completion waiters
  - reset terminates completion waiters
- that gives us another owner-backed lifecycle cut we can eventually compare
  against the target Meerkat policy during cutover

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_stopped_completion_waiters`
- `cargo test -p meerkat-runtime --lib session_adapter`
- `cargo test -p meerkat --lib`
- `cargo check -p meerkat --lib --features comms`

Result:

- `MeerkatMachine` now has owner-backed stop-side completion-waiter semantics
  in addition to the existing destroy/recover/recycle/reset map

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was to stay on completion waiters only and not force a
  stronger claim yet about the ops-owned `wait_all` carrier under stop

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this sharpens current lifecycle truth, but it is still not a write-authority
  cutover signal

Next likely step:

- keep selecting Meerkat lifecycle seams where current owner behavior is
  explicit enough to separate “observed current semantics” from “desired final
  semantics”

## Slice 78 - MobMachine surfaces persisted escalation_threshold in tracked runs

Goal:

- extend the tracked-run map with one more durable supervisor-related kernel
  fact without promoting new escalation behavior invariants too early

What landed:

- `meerkat-mob/src/run.rs`
  - added typed reader `MobRun::escalation_threshold()`
  - extended the kernel reader unit test to prove the field parses correctly
- `meerkat-mob/src/mob_machine.rs`
  - added `escalation_threshold` to `TrackedRunStoreSnapshot`
  - populated it from the persisted `MobRun`
- `meerkat-mob/src/runtime/tests.rs`
  - added
    `test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`
  - the new live proof shows that a healthy supervisor-threshold run preserves:
    - `escalation_threshold == 3`
    - zeroed failure counters
    - at least one materialized step status
    - step-status keys bounded by `ordered_steps`

Why this slice matters:

- it keeps broadening the durable Mob tracked-run map using facts the current
  run aggregate actually stores
- it gives the future `MobMachine` a cleaner bridge from supervisor config into
  persisted kernel state without pretending the escalation behavior is already
  formalized

Verification:

- `cargo test -p meerkat-mob --lib test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot`
- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_supervisor_threshold_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now carries persisted supervisor escalation threshold through the
  joined tracked-run snapshot, and the live code path confirms it survives the
  current runtime projection

Backtracks encountered:

- no code-level backtrack in the final slice
- the restraint was methodological: add the durable threshold fact first, not a
  new claim about when or how escalation must happen

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves the tracked-run map, but it is still not a write-authority
  cutover signal

Next likely step:

- continue preferring durable tracked-run facts or live durability proofs over
  escalation/retry cleanup behavior that still depends on timing

## Slice 73 - MeerkatMachine preserves active wait_all after destroy

Goal:

- observe what happens to an active `wait_all` carrier across the current
  `destroy()` path without inventing target-state semantics

What landed:

- `meerkat-runtime/src/session_adapter.rs`
  - added
    `meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy`
  - the new test proves that:
    - the authority-owned `wait_request_id` survives destroy
    - the shell-owned pending wait carrier survives destroy
    - both sides still agree on the same `WaitRequestId`
    - the active wait future still resolves normally after the target
      operation completes
    - runtime control phase is `Destroyed` while that ops-owned wait state is
      still visible

Why this slice matters:

- it extends the Meerkat lifecycle map from recover/recycle/reset into destroy
- it gives us an explicit observational answer to a non-obvious question in
  current code: destroy terminates input waiters, but it does not currently
  replace or cancel the ops lifecycle registry

Verification:

- `cargo test -p meerkat-runtime --lib meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy`
- `cargo test -p meerkat-runtime --lib session_adapter`

Result:

- `MeerkatMachine` now has owner-backed evidence that current destroy semantics
  preserve active `wait_all` state until the tracked operation itself settles

Backtracks encountered:

- no code-level backtrack in the final slice
- the key restraint was treating this as an observation of current semantics,
  not as proof that destroy should keep this behavior after cutover

Cutover gate read:

- `MeerkatMachine` remains on the observability side of the gate
- this improves our picture of current lifecycle semantics, but it does not yet
  imply the target destroy policy is decided

Next likely step:

- keep selecting Meerkat lifecycle seams where the current owner behavior can
  be stated precisely, even when that behavior may later be judged undesirable

## Slice 74 - MobMachine live proof of healthy all-policy run durability

Goal:

- strengthen the live tracked-run proof for `CollectionPolicy::All` without
  assuming more status coverage than the current projection materializes

What landed:

- `meerkat-mob/src/runtime/tests.rs`
  - extended
    `test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
    to prove that the active tracked all-policy run:
    - keeps `failure_count == 0`
    - keeps `consecutive_failure_count == 0`
    - surfaces at least one materialized step status
    - only exposes step-status entries whose keys stay inside the ordered-step
      universe

Why this slice matters:

- it completes the healthy tracked-run durability trio across quorum, any, and
  all collection-policy variants
- it stays on the same smaller active-run status identity fact established by
  the earlier backtracks

Verification:

- `cargo test -p meerkat-mob --lib test_capture_mob_machine_snapshot_tracks_live_all_collection_policy_shape`
- `cargo test -p meerkat-mob --lib`

Result:

- `MobMachine` now has live evidence that healthy all-policy tracked runs
  preserve zeroed failure counters and kernel-bounded materialized step-status
  identity

Backtracks encountered:

- no new code-level backtrack in the final slice
- the restraint was again to avoid terminal or full-coverage claims and stay on
  the durable active-run surface

Cutover gate read:

- `MobMachine` remains on the observability side of the gate
- this improves confidence in tracked-run durability without moving write
  authority

Next likely step:

- continue extending live durability proofs only where the joined snapshot has
  already demonstrated stable owner-backed truth
