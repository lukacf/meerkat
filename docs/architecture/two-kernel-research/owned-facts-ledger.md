# Two-Kernel Owned-Facts Ledger

Status: working draft

## Purpose

This ledger lists the semantic facts that matter to the proposed two-kernel architecture.

For each fact it records:

- where the fact is carried today
- who should own it canonically
- where it should live after collapse
- which current carriers become projections or compatibility shims
- whether the fact crosses the `MobMachine <-> MeerkatMachine` seam
- which historical bug family pressures the row

The goal is not to inventory every field. The goal is to inventory every semantic fact that can become split truth.

## Reading Rule

If a fact has multiple current carriers, that is not automatically wrong.

It becomes architectural debt when:

- more than one carrier is treated as authoritative
- the carriers can diverge during recovery, wake/drain, replay, or respawn
- a boundary consumer has to reconstruct the fact from partial projections

The future architecture should make each row have:

- one canonical owner
- zero or more clearly derived projections
- an explicit seam crossing only when the other kernel truly needs to see the fact

## Kernel Facts

| Fact | Current carriers | Canonical owner | Future home | Derived projections / compatibility | Crosses seam? | Historical bug pressure |
| --- | --- | --- | --- | --- | --- | --- |
| Durable member identity | MobKit `AgentIdentity` layer outside the current repo; internal bridge/runtime indices that still retain `meerkat-mob::ids::MeerkatId` during cleanup | `MobMachine` | Identity registry inside `MobMachine` | No public compatibility alias; public 0.6 surfaces use `AgentIdentity` directly | Yes | split-owner truth; cross-surface field loss |
| Reset generation | MobKit runtime-id suffix outside the current repo; mob reset lifecycle and reset event lineage; implicit post-reset epoch behavior | `MobMachine` | Identity lifecycle substate inside `MobMachine` | Human-readable runtime-id string formatting | Yes | split-owner truth; recovery/resume drift |
| Current runtime incarnation (`AgentRuntimeId`) | MobKit runtime-id layer outside the current repo; current respawn/reset plumbing; implicit current-incarnation semantics in mob runtime code | `MobMachine` | Identity lifecycle and incarnation registry inside `MobMachine` | Human-readable runtime-id string formatting | Yes | split-owner truth; recovery/resume drift |
| Current authority state and `FenceToken` | MobKit continuity or lease layer outside the current repo; ad hoc current-owner behavior in mob/runtime bridge code | `MobMachine` | Authority ledger inside `MobMachine` | Health/heartbeat metrics; lease persistence rows and expiry mechanics in perimeter infrastructure | Yes | split-brain risk; recovery/resume drift |
| Active incarnation binding (`AgentIdentity -> AgentRuntimeId -> hidden session runtime`) | `MobActor.roster`; internal `RosterEntry.member_ref`; internal bridge-session lookup paths | `MobMachine` | Runtime-binding substate inside `MobMachine` | Internal bridge/runtime lookup indices only; no public `MemberRef` or Mob-facing `SessionId` surface | Yes | split-owner truth; recovery/resume drift |
| Hidden execution epoch (`RuntimeEpochId`) | `RuntimeSessionEntry.epoch_id`; `SessionRuntimeBindings.epoch_id`; persisted ops lifecycle snapshot epoch id | `MeerkatMachine` | Hidden execution-binding substate inside `MeerkatMachine` | Debugging views only; never a Mob identity surface | No | recovery/resume drift |
| Member lifecycle state | `RosterEntry.state`; pending spawn lineage; disposal pipeline; `MobLifecycleMachine`; mob reset/stop/complete events | `MobMachine` | Lifecycle region inside `MobMachine` | Roster list views; lifecycle notices | Yes | split-owner truth; recovery/resume drift |
| Roster membership, profile assignment, runtime mode, and labels | `MobActor.roster`; `RosterEntry.profile`; `RosterEntry.runtime_mode`; `RosterEntry.labels`; `MobDefinition.members`; mob spawn/retire event lineage | `MobMachine` | Roster region inside `MobMachine` | `list_members()` views; console roster projections | No, except through spawn/retire commands | split-owner truth |
| Spawn policy and auto-spawn rules | `set_spawn_policy`; `spawn_policy`; inline spawn behavior in the actor | `MobMachine` | Orchestration policy region inside `MobMachine` | Operator controls and diagnostics | No | flow/runtime seam leak |
| Top-level mob lifecycle phase | `MobLifecycleMachine`; lifecycle authority; mob reset/stop/complete events | `MobMachine` | Top-level lifecycle region inside `MobMachine` | Lifecycle views | No | flow/runtime seam leak |
| Coordinator/orchestrator authority | `MobOrchestratorMachine`; definition-level orchestrator identity; `MobActor` serialization | `MobMachine` | Orchestration authority region inside `MobMachine` | Supervisor notices | No | flow/runtime seam leak |
| Collaboration topology intent | `MobDefinition` wiring rules; `RosterEntry.wired_to`; `RosterEntry.external_peer_specs`; peer-wire and peer-unwire event lineage | `MobMachine` | Collaboration-topology region inside `MobMachine` | Wiring projections and visualization | No | split-owner truth; comms seam leak |
| Flow-run durable progress and dependency satisfaction | `FlowRunMachine`; flow kernel; actor-local run bookkeeping | `MobMachine` | Durable flow-run region inside `MobMachine` | Flow event stream | Yes | duplicate-path drift; flow/runtime seam leak |
| Frame-local execution frontier and branch outcome | `FlowFrameMachine`; flow engine frame bookkeeping | `MobMachine` | Frame scheduler region inside `MobMachine` | Flow event stream; debug traces | Yes | duplicate-path drift |
| Loop iteration progress | `LoopIterationMachine`; flow engine loop bookkeeping | `MobMachine` | Loop sub-automaton inside `MobMachine` | Flow event stream; debug traces | Yes | duplicate-path drift |
| Work assignment intent and `WorkSpec` (`WorkRef -> target incarnation + work spec`) | `run_flow`; `internal_turn`; actor dispatch bookkeeping | `MobMachine` | Work ledger keyed by `WorkRef` inside `MobMachine` | Injection buffers and delivery queues such as `RuntimeSessionState.queued_turns`; compatibility lowering from current raw turn-delivery paths until `WorkRef` / `WorkSpec` become explicit | Yes | split-owner truth; duplicate-path drift |
| Task-board state | `task_create`; `task_update`; task-board service events | `MobMachine` | Coordination/task-board region inside `MobMachine` | Console task views | No | cross-surface field loss |
| Durable identity-scoped event history | `MobEvent` append log; MobKit console event store outside the current repo | `MobMachine` | Identity-scoped event/history region inside `MobMachine` | Live projections, UI deques, and session-local runtime notices | Yes | cross-surface field loss; recovery/resume drift |
| Session-scoped runtime control phase | `RuntimeControlMachine`; `MeerkatMachine.sessions`; attachment publication and control-plane methods | `MeerkatMachine` | Top-level control region inside `MeerkatMachine` | Surface views exposed through service helpers | No | wake/drain race; recovery/resume drift |
| Admitted work queue, handling mode, and contributor set | `RuntimeIngressMachine` | `MeerkatMachine` | Admission region inside `MeerkatMachine` | Queue snapshots, debug counters, and transport/projection carriers such as `EphemeralRuntimeDriver.queue`, `EphemeralRuntimeDriver.steer_queue`, and mob-to-runtime queued turn buffers | Yes | split-owner truth; wake/drain race |
| Accepted work execution, boundary progression, cancellation, retries, and turn admission | `InputLifecycleMachine`; `TurnExecutionMachine`; `SessionTurnAdmissionMachine`; runtime loop execution state | `MeerkatMachine` | Work-execution region inside `MeerkatMachine` | Completion notices; member terminal summaries | Yes | wake/drain race; duplicate-path drift; recovery/resume drift |
| Async-op lifecycle, barrier membership, and wait-all truth | `OpsLifecycleMachine`; runtime ops lifecycle registry; detached wake flags and progress counters | `MeerkatMachine` | Async-op region inside `MeerkatMachine` | Progress snapshots; wait summaries | No, except through terminal work outcomes | wake/drain race |
| Tool-surface visibility and pending mutation truth | `ExternalToolSurfaceMachine`; `SessionToolVisibilityMachine`; `McpRouter.servers`; `RouterProjectionSnapshot`; `ToolScope` staging | `MeerkatMachine` | Tool-surface region inside `MeerkatMachine` | Visible tool snapshots; MCP notices | No | split-owner truth; cross-surface field loss |
| Trusted-peer graph, raw envelope classification, and peer-input admission | `PeerCommsMachine`; comms runtime trust edges; `RuntimeCommsBridge` | `MeerkatMachine` | Comms region inside `MeerkatMachine` | Mob-level notices derived from runtime/comms events | No | split-owner truth; comms seam leak |
| Comms drain and keep-alive lifecycle | `CommsDrainLifecycleMachine`; `MeerkatMachine.comms_drain_slots`; keep-alive drain task controls | `MeerkatMachine` | Drain-control region inside `MeerkatMachine` | Host keep-alive status views | No | wake/drain race |
| Session-scoped runtime recovery snapshot | persistent session store; runtime recovery and registration flow; `MeerkatMachine.sessions` | `MeerkatMachine` | Recovery region inside `MeerkatMachine` | Ready/recovered lifecycle receipts back to `MobMachine` | Indirectly, through lifecycle results | recovery/resume drift |
| Work acceptance and terminal outcome | `InputLifecycle` terminal outcome; runtime completion plumbing | `MeerkatMachine` | Result emission region inside `MeerkatMachine` | Mob completion projections, member terminal classifiers, and helper result anchors | Yes | split-owner truth; cross-surface field loss |

## Derived-Only Facts

These facts should not survive as standalone owners in the two-kernel design.

| Fact | Current carriers | Canonical owner | Future home | Derived projections / compatibility | Crosses seam? | Historical bug pressure |
| --- | --- | --- | --- | --- | --- | --- |
| Helper convenience workflows and terminal class | `spawn_helper`; `fork_helper`; `wait_one`; `wait_all`; `MobHelperResultAnchorMachine`; member terminal classifier | Derived from `MobMachine` orchestration plus `MeerkatMachine` work outcomes | Derived-only | Helper APIs and helper views | Yes | cross-surface field loss |
| Runtime bridge submission/completion counters | `MobRuntimeBridgeAnchorMachine`; session backend sidecar state | Derived from seam events between `MobMachine` work ledger and `MeerkatMachine` result emission | Derived-only | Bridge diagnostics | Yes | split-owner truth |
| Wiring projection | `MobWiringAnchorMachine`; roster-projected `wired_to`; trust-edge views | Derived from `MobMachine` collaboration topology intent plus `MeerkatMachine` comms truth | Derived-only unless explicitly promoted later | Debugging and visualization | No | split-owner truth; comms seam leak |
| Live observability projections and UI deques | roster projections; session-local runtime notices; UI deques and console materializations | Derived from durable identity-scoped event history plus live notices | Derived-only | Console streams and dashboards | Yes | cross-surface field loss |
| Member lifecycle anchor views | `MobMemberLifecycleAnchorMachine`; roster and terminalization notices | Derived from `MobMachine` lifecycle plus `MeerkatMachine` work terminalization | Derived-only | Console/member status views | Yes | recovery/resume drift |

## Perimeter Facts

These facts exist, but they are intentionally outside the present two-kernel boundary.

| Fact | Current carriers | Canonical owner | Future home | Derived projections / compatibility | Crosses seam? | Historical bug pressure |
| --- | --- | --- | --- | --- | --- | --- |
| Public addressability policy | `external_addressable` member/profile settings; bridge `internal_turn()` bypass; external send-path checks | Tentatively a perimeter comms/policy layer | Perimeter after an explicit migration boundary is chosen | UI policy indicators; compatibility shims while internal dispatch is separated from public send | No | split-owner truth; contract bypass |
| Peer directory reachability and last send failure | `PeerDirectoryReachabilityMachine`; comms directory/transport layer | `MeerkatMachine` | Peer-ingress and reachability region inside `MeerkatMachine` | Peer health and reachability views | No | transport edge cases |
| Schedule revision, trigger policy, and occurrence planning | `ScheduleLifecycleMachine`; schedule service planning state | Schedule kernel | Perimeter unless promoted into a third kernel | Operator views and delivery diagnostics | No | out-of-bound for the current two-kernel target |
| Occurrence claim, lease, and delivery lifecycle | `OccurrenceLifecycleMachine`; schedule driver claim/deliver/terminalize state | Schedule kernel | Perimeter unless promoted into a third kernel | Delivery receipts and audit views | No | out-of-bound for the current two-kernel target |

## Intentional Open Questions

These are real semantic facts, but their final ownership still needs an explicit decision.

| Fact | Current carriers | Candidate owners | Why it is still open | Historical bug pressure |
| --- | --- | --- | --- | --- |
| Identity continuity record and checkpoint lineage | MobKit `ContinuityRecord` and checkpoint versioning outside the current repo; mob resume/reset plumbing; persisted session linkage | `MobMachine` or a perimeter continuity service | If identity continuity is core product truth, it belongs in `MobMachine`. If it is an operational overlay, it can stay above the kernel line. | recovery/resume drift |
| Ownership and migration of public addressability policy | `external_addressable` member/profile settings; bridge `internal_turn()` bypass; external send-path checks | perimeter comms/policy layer or `MobMachine` policy surface | The target architecture wants internal work dispatch separated from public send policy, but current settings and bridge behavior still make this feel partly mob-owned. The migration boundary needs to be explicit. | split-owner truth; contract bypass |
| Mutable member-spec inputs and reloadable member metadata | roster labels; profile-bound launch/runtime fields; other higher-layer member configuration that can change without identity replacement | `MobMachine` or higher-level product configuration | The bridge can absorb some of this into `MemberSpec`, but not every mutable profile surface belongs in kernel truth. The split between roster truth and higher-level product configuration still needs to be named explicitly. | cross-surface field loss |

## Notes

- The most dangerous rows are the ones where today's system already carries the same fact in a mob projection, a runtime sidecar, and a session-local field.
- The highest-risk seam rows are:
  - runtime incarnation, authority state, and `FenceToken`
  - active incarnation binding
  - hidden execution epoch versus semantic incarnation
  - collaboration topology intent versus Meerkat comms truth
  - work assignment intent and `WorkSpec`
  - admitted work acceptance and terminal outcome
  - durable identity-scoped event history
- The open-question rows should be resolved before implementation planning hardens, otherwise the migration plan will smuggle a third architecture back in through adapters.
- Not every Mob API concern needs its own bridge primitive. Some concerns, such as roster reconciliation or flow control, can remain Mob-level operations that lower into lifecycle, topology, and work rows already listed above.
- The explicit mapping between `Generation`, `AgentRuntimeId`, and hidden `RuntimeEpochId` is captured in `generation-epoch-mapping.md`.
