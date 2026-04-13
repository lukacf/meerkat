# Meerkat Owned-Facts Ledger

Status: supporting design draft (target decisions frozen in `meerkat-machine-freeze.md`)

## Purpose

This ledger narrows the broader two-kernel fact inventory down to the Meerkat
side only.

It records the semantic facts that `MeerkatMachine` must own internally if it
is going to close the gap between today's over-split runtime machines.

For each fact, it names:

- the current carriers in code or machine contracts
- the canonical Meerkat region that should own it
- the target Meerkat state variable(s)
- the supporting recovery source or persistence witness
- the projections that should remain derived only
- the main coupling pressure or historical bug class

This is not a field dump. It is a list of the facts that are dangerous when
they become split truth.

## Reading Rule

If a row has multiple current carriers, that is not automatically a bug.

It becomes architectural debt when:

- more than one carrier is treated as authoritative
- recovery rebuilds from the wrong carrier
- another region infers the fact instead of reading the owner
- a projection or convenience buffer can drift from the owner

The target state is:

- one canonical Meerkat owner per fact
- explicit supporting carriers where required
- projections that can be rebuilt, not trusted

## Semantic Facts

| Region | Fact | Current carriers | Canonical owner | Target Meerkat state variable(s) | Recovery source / witness | Derived projections and compatibility | Coupling pressure |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Control and Binding | Registered runtime binding | `MeerkatMachine.sessions`; `RuntimeSessionEntry.driver`; `RuntimeSessionEntry.completions`; `RuntimeSessionEntry.attachment` | Control and Binding | `binding.session_id`, `binding.runtime_id`, `binding.driver_present`, `binding.completions_present`, `binding.attachment_live` | session registration plus live entry publication in `MeerkatMachine` | service helper lookups, ready/not-ready views | split publication; recovery drift |
| Control and Binding | Hidden execution continuity | `RuntimeSessionEntry.epoch_id`; `SessionRuntimeBindings.epoch_id`; ops snapshot epoch in store | Control and Binding | `binding.epoch_id` | durable ops lifecycle snapshot or fresh epoch creation | debug-only displays; never Mob-visible | recovery/resume drift |
| Control and Binding | Epoch cursor continuity | `SessionRuntimeBindings.cursor_state`; runtime persistence cursors | Control and Binding | `binding.cursor_state.agent_applied_cursor`, `binding.cursor_state.runtime_observed_seq`, `binding.cursor_state.runtime_last_injected_seq` | recovered `EpochCursorSnapshot` or fresh zero cursors | diagnostics only | recovery ordering drift |
| Control | Runtime lifecycle phase | `RuntimeControlMachine.phase`; `RuntimeState`; runtime driver state | Control and Binding | `control.phase` | driver recovery plus control reducer | public runtime-state views | wake/drain races |
| Control | Active run ownership and return target | `RuntimeControlMachine.current_run_id`; `RuntimeControlMachine.pre_run_state`; driver-local run state | Control and Binding | `control.current_run_id`, `control.pre_run_phase` | runtime control reduction plus run terminal events | surface run-status views | split-owner truth between control and turn |
| Control | Wake and immediate-process intent | `RuntimeControlMachine.wake_pending`; `RuntimeControlMachine.process_pending`; post-admission signal plumbing | Control and Binding | `control.wake_pending`, `control.process_pending` | control reducer and post-admission signal application | debug counters only | wake/drain race |
| Admission | Accepted input identity and metadata | `RuntimeIngressMachine.admitted_inputs`; `content_shape`; `request_id`; `reservation_key`; `policy_snapshot`; `handling_mode` | Admission and Input Ledger | `inputs.admitted`, `inputs.content_shape`, `inputs.request_id`, `inputs.reservation_key`, `inputs.policy_snapshot`, `inputs.handling_mode` | runtime input store/ledger | queue snapshots, request correlation projections | split-owner truth; field loss |
| Admission | Per-input lifecycle truth | `InputLifecycleMachine`; `RuntimeIngressMachine.lifecycle`; `InputState` ledger | Admission and Input Ledger | `inputs.lifecycle`, `inputs.terminal_outcome`, `inputs.last_run_id`, `inputs.last_boundary_sequence` | authoritative `InputState` records | completion notices, terminal summaries | recovery drift; duplicate lifecycle carriers |
| Admission | Queue and steer ordering | `RuntimeIngressMachine.queue`; `RuntimeIngressMachine.steer_queue`; `EphemeralRuntimeDriver.queue`; `EphemeralRuntimeDriver.steer_queue` | Admission and Input Ledger | `inputs.queue`, `inputs.steer_queue` | rebuild from lifecycle + admitted set during recovery | driver-local queue projections | queue/projection drift |
| Admission | Current staged contributor batch | `RuntimeIngressMachine.current_run`; `current_run_contributors`; driver-local staged state | Admission and Input Ledger | `inputs.current_run_id`, `inputs.current_run_contributors` | stage snapshot / recovered run association | ready-for-run notices | rollback/recovery drift |
| Admission | Silent-intent policy | `silent_intent_overrides`; driver silent comms intents | Admission and Input Ledger | `inputs.silent_intent_overrides` | session adapter policy configuration | policy views | policy/ingress split |
| Turn | Turn phase and primitive identity | `TurnExecutionMachine.phase`; `TurnExecutionMachine.active_run`; `primitive_kind` | Turn Execution | `turn.phase`, `turn.active_run_id`, `turn.primitive_kind` | runtime-driven turn start or recovered terminal state | run-status views | control/turn split |
| Turn | Turn execution capabilities | `admitted_content_shape`; `vision_enabled`; `image_tool_results_enabled` | Turn Execution | `turn.admitted_content_shape`, `turn.vision_enabled`, `turn.image_tool_results_enabled` | primitive application | trace/debug views | cross-boundary field loss |
| Turn | Barrier, pending-op, and cancellation posture | `tool_calls_pending`; `pending_op_refs`; `barrier_operation_ids`; `has_barrier_ops`; `barrier_satisfied`; `cancel_after_boundary` | Turn Execution | `turn.tool_calls_pending`, `turn.pending_op_refs`, `turn.barrier_operation_ids`, `turn.has_barrier_ops`, `turn.barrier_satisfied`, `turn.cancel_after_boundary` | turn machine state plus op lifecycle events | wait summaries only | waiting folklore; wake races |
| Turn | Boundary count, terminal outcome, extraction retries | `boundary_count`; `terminal_outcome`; `extraction_attempts`; `max_extraction_retries` | Turn Execution | `turn.boundary_count`, `turn.terminal_outcome`, `turn.extraction_attempts`, `turn.max_extraction_retries` | turn reducer and run events | surface transcript or run summaries | recovery drift |
| Async Ops | Operation identity, kind, and status | `OpsLifecycleMachine.known_operations`; `operation_kind`; `operation_status`; runtime ops registry | Async Operations | `ops.known_operations`, `ops.operation_kind`, `ops.operation_status` | recovered ops lifecycle snapshot | operation list views | duplicate ownership in tool/mob adapters |
| Async Ops | Peer-ready handoff truth | `OpsLifecycleMachine.peer_ready`; child-operation plumbing | Async Operations | `ops.peer_ready` | recovered ops snapshot or live op events | peer exposure notices only | ops/comms seam leak |
| Async Ops | Watchers, progress, buffered terminals, completed order | `progress_count`; `watcher_count`; `terminal_outcome`; `terminal_buffered`; `completed_order`; `created_at_ms`; `completed_at_ms` | Async Operations | `ops.progress_count`, `ops.watcher_count`, `ops.terminal_outcome`, `ops.terminal_buffered`, `ops.completed_order`, `ops.created_at_ms`, `ops.completed_at_ms` | recovered ops snapshot | progress summaries, UI projections | progress/terminal drift |
| Async Ops | Wait-all truth | `wait_active`; `wait_request_id`; `wait_operation_ids` | Async Operations | `ops.wait_active`, `ops.wait_request_id`, `ops.wait_operation_ids` | recovered ops snapshot | wait summaries | wait/drain race |
| Peer Ingress | Local peer identity and trust admission policy | `CommsRuntime.public_key`; `CommsRuntime.require_peer_auth`; `PeerCommsMachine.trusted_peers`; comms trust store | Peer Ingress | `peer.self_peer_id`, `peer.auth_required`, `peer.trusted_peers` | trust store recovery / attach-time snapshot | peer list views | comms/runtime seam leak |
| Peer Ingress | Authority phase and typed peer submission backlog | `PeerCommsMachine.phase`; `PeerCommsMachine.submission_queue`; runtime comms bridge submission queue | Peer Ingress | `peer.phase`, `peer.submission_queue` | rebuild from undelivered classified peer items plus authority state | debug queue views; runtime peer diagnostics | parallel peer path bugs; hidden dropped/delivered states |
| Peer Ingress | Stable raw item identity, envelope lineage, and normalized classification | `raw_item_id`; `raw_item_peer`; `raw_item_kind`; `classified_as`; `text_projection`; `content_shape`; `request_id`; `reservation_key`; `lifecycle_peer`; `trusted_snapshot` | Peer Ingress | `peer.raw_item_peer`, `peer.raw_item_kind`, `peer.classified_as`, `peer.text_projection`, `peer.content_shape`, `peer.request_id`, `peer.reservation_key`, `peer.lifecycle_peer`, `peer.trusted_snapshot` | inbox/reservation state | host/UI transcript projections | classification drift |
| Tool Surface | Known and visible external tool membership | `ExternalToolSurfaceMachine.known_surfaces`; `visible_surfaces`; `base_state`; router projection | External Tool Surface | `tools.known_surfaces`, `tools.visible_surfaces`, `tools.base_state` | surface authority plus router rebuild | visible tool list, adapter cache | snapshot/projection drift |
| Tool Surface | Staged and pending mutation lineage | `pending_op`; `staged_op`; `staged_intent_sequence`; `pending_task_sequence`; `pending_lineage_sequence` | External Tool Surface | `tools.pending_op`, `tools.staged_op`, `tools.staged_intent_sequence`, `tools.next_staged_intent_sequence`, `tools.pending_task_sequence`, `tools.pending_lineage_sequence`, `tools.next_pending_task_sequence` | router pending obligations and staged payloads | surface notices and diagnostics | split-owner truth |
| Tool Surface | Inflight-call and last-delta truth | `inflight_calls`; `last_delta_operation`; `last_delta_phase` | External Tool Surface | `tools.inflight_calls`, `tools.last_delta_operation`, `tools.last_delta_phase` | router base state + pending lineage | UI notices; debug traces | draining-removal bugs |
| Tool Surface | Snapshot publication alignment | `snapshot_epoch`; `snapshot_aligned_epoch`; `RouterProjectionSnapshot.epoch` | External Tool Surface | `tools.snapshot_epoch`, `tools.snapshot_aligned_epoch` | router projection rebuild | visible snapshot caches | stale snapshot bugs |
| Tool Visibility | Durable visibility state, requested deferred names, and visibility witnesses | `SessionToolVisibilityState`; `ToolVisibilityWitness`; exact catalog and projection names | Tool Visibility | target delta under review; expected to expand `tools.*` with durable visibility owner state | rebased exact-current visibility owner plus session-task mutation seam | visible-name list, catalog projection, schema caches | control/surface split drift |
| Drain | Comms drain lifecycle | `CommsDrainLifecycleMachine.phase`; `CommsDrainLifecycleMachine.mode`; `MeerkatMachine.comms_drain_slots` | Drain and Keep-Alive | `drain.phase`, `drain.mode` | drain slot lifecycle authority | keep-alive status views | wake/drain race |

## Supporting Carriers

These are not independent semantic owners, but they are important state
carriers whose publication must stay aligned with the semantic owner.

| Supporting carrier | Current location | Semantic owner it refines | Why it matters |
| --- | --- | --- | --- |
| Driver handle | `RuntimeSessionEntry.driver` | Control and Binding, Admission and Input Ledger | It is the concrete capability through which control, queueing, recovery, and lifecycle transitions happen. |
| Attachment handle | `RuntimeSessionEntry.attachment` | Control and Binding | Published attachment truth must agree with runtime phase and liveness. |
| Completion registry | `RuntimeSessionEntry.completions`; `MeerkatCompletionWaitersSnapshot` | Admission and Input Ledger | Waiters must resolve from canonical input terminalization, exactly once. The snapshot is diagnostic only and must not become semantic truth. |
| Ops lifecycle registry handle | `RuntimeSessionEntry.ops_lifecycle` | Async Operations | Turn and persistence must share one op registry, not reconstruct one ad hoc. |
| Queue projections | `EphemeralRuntimeDriver.queue`, `EphemeralRuntimeDriver.steer_queue` | Admission and Input Ledger | They are useful execution caches, but must be rebuilt from canonical ingress truth. |
| Router projection snapshot | `RouterProjectionSnapshot` | External Tool Surface | Public tool visibility depends on it, but it is derived from machine-owned surface state. |
| Runtime trust registration seam | `CommsRuntime::register_trusted_peer(...)`; `CommsRuntime::unregister_trusted_peer(...)`; `CommsRuntime::unregister_trusted_pubkey(...)` | Peer Ingress | These are the named runtime mutation paths that keep router-visible trust and inbox-owned peer authority synchronized. |
| Legacy trust helper | `CommsRuntime::upsert_trusted_peer(...)` | Peer Ingress | It updates router-visible trust only and must not be treated as canonical peer-ingress mutation once classified ingress depends on `PeerCommsAuthority`. |

## Facts That Must Not Escape The Meerkat Seam

These facts are important, but they should stay entirely inside
`MeerkatMachine` and must not cross to `MobMachine`.

- `binding.epoch_id`
- `binding.cursor_state.*`
- `control.wake_pending`
- `control.process_pending`
- `inputs.queue`
- `inputs.steer_queue`
- `inputs.current_run_contributors`
- `turn.pending_op_refs`
- `turn.barrier_operation_ids`
- `turn.barrier_satisfied`
- `ops.wait_request_id`
- `peer.raw_item_*` classification lineage
- `peer.lifecycle_peer`
- `tools.pending_lineage_sequence`
- `tools.snapshot_epoch`
- `drain.phase` and `drain.mode`

Mob should see outcomes and lifecycle results, not these internal mechanics.

## Compression Rule

When the Meerkat design is implemented, the rows above should collapse toward
one internal owner per region:

- Control and Binding
- Admission and Input Ledger
- Turn Execution
- Async Operations
- Peer Ingress
- External Tool Surface
- Drain and Keep-Alive

Each region may still expose helpers, reducers, or adapters, but those helpers
must not become competing semantic owners.

## Resolved Target Decisions

The target machine freeze now resolves the previously open ownership questions:

- completion-waiter state becomes an explicit machine-owned subregion
- peer-ingress backlog and lineage remain machine-owned until successful typed
  admission or explicit rejection
- raw helper seams like `CommsRuntime::upsert_trusted_peer(...)` are not part
  of target semantic authority
- router projection remains derived; machine-owned tool truth stays in the
  tool-surface region
- `ResetRuntime` rotates `binding.epoch_id`; `RecoverRuntime` and
  `RecycleRuntime` do not
