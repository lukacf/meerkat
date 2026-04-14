# Two-Kernel Collapse Matrix

Status: working draft

## Working Assumptions

- Target kernel boundary is:
  - `MeerkatMachine`: single-session interactive runtime
  - `MobMachine`: multi-agent orchestration
- Scheduling stays outside the present kernel boundary.
- Observation anchors are not treated as final semantic owners.

## Meerkat Internal

| Current machine | Owned facts today | Proposed fate | Placement |
| --- | --- | --- | --- |
| `InputLifecycleMachine` | Per-input lifecycle state, run association, boundary sequence, input terminal outcome | Absorb into per-work substate inside `MeerkatMachine`; no standalone proof boundary | Internal to Meerkat |
| `RuntimeIngressMachine` | Admitted work set, queue and steer ordering, policy snapshot, handling mode, contributor staging, wake/process flags | Merge into the admission and work-queue region of `MeerkatMachine` | Internal to Meerkat |
| `RuntimeControlMachine` | Runtime phase, active run, executor attachment, control-plane commands, admission resolution, recycle/reset/recover lifecycle | Merge into the top-level control region of `MeerkatMachine` | Internal to Meerkat |
| `TurnExecutionMachine` | Run execution semantics, barrier membership, pending async-op refs, cancellation timing, extraction retries, turn terminal outcome | Merge into the run-execution region of `MeerkatMachine` | Internal to Meerkat |
| `SessionTurnAdmissionMachine` | Turn admission truth, interrupt intent, shutdown intent, active/completing lifecycle | Merge into the run-execution and control regions of `MeerkatMachine` | Internal to Meerkat |
| `OpsLifecycleMachine` | Async operation truth, peer-ready truth, watcher/progress counts, terminal buffering, wait-all truth, completed ordering | Merge into the async-op region of `MeerkatMachine` | Internal to Meerkat |
| `SessionToolVisibilityMachine` | Staged/active tool filters, deferred request truth, visibility witness sets, active/staged revision sequencing | Merge into the tool-surface and boundary-apply region of `MeerkatMachine` | Internal to Meerkat |
| `PeerCommsMachine` | Trusted-peer snapshot, raw envelope classification, typed peer input candidate truth, content/correlation preservation | Merge into the typed peer-ingress region of `MeerkatMachine` | Internal to Meerkat |
| `CommsDrainLifecycleMachine` | Keep-alive drain mode, spawn/abort/respawn truth for drain tasks | Fold into keep-alive and comms-drain control state inside `MeerkatMachine` | Internal to Meerkat |
| `ExternalToolSurfaceMachine` | Tool-surface visibility, staged add/remove/reload truth, pending surface ops, snapshot alignment, inflight call rejection | Absorb into a tool-surface region inside `MeerkatMachine`; keep router/adapter code mechanical only | Internal to Meerkat |
| `PeerDirectoryReachabilityMachine` | Resolved-directory peer reachability and last send-failure reason | Merge into the typed peer-ingress and reachability region of `MeerkatMachine` | Internal to Meerkat |

## Mob Internal

| Current machine | Owned facts today | Proposed fate | Placement |
| --- | --- | --- | --- |
| `MobLifecycleMachine` | Top-level mob phase, active run count, cleanup pending | Merge into the top-level lifecycle region of `MobMachine` | Internal to Mob |
| `MobOrchestratorMachine` | Coordinator binding, pending spawns, active flows, topology revision, supervisor activation, member force-cancel authority | Merge into the orchestration region of `MobMachine` | Internal to Mob |
| `FlowRunMachine` | Durable flow-run truth, dependency readiness, collection policy satisfaction, failure/escalation truth, target outcome truth, slot scheduling | Keep semantics, but move it inside `MobMachine` instead of keeping it as a separate kernel boundary | Internal to Mob |
| `FlowFrameMachine` | Frame-local ready frontier, node status, branch winners, node scheduling, output recording | Absorb into an internal frame-scheduler region of `MobMachine` | Internal to Mob |
| `LoopIterationMachine` | Loop instance stage, current iteration, active body frame, until-condition progression | Absorb into an internal loop sub-automaton inside `MobMachine` | Internal to Mob |

## Derived Only

| Current machine | Owned facts today | Proposed fate | Placement |
| --- | --- | --- | --- |
| `MobHelperResultAnchorMachine` | Mirrored helper-facing terminal classes and force-cancel observations | Delete as an owner; keep only as a derived query or debug/test projection if still useful | Derived-only |
| `MobMemberLifecycleAnchorMachine` | Mirrored operation peer exposure and member terminalization observations | Delete as an owner; derive from the member bridge contract and terminal events | Derived-only |
| `MobRuntimeBridgeAnchorMachine` | Mirrored runtime submission/completion/failure/cancel/stop observations | Delete as an owner; derive from runtime bridge events | Derived-only |
| `MobWiringAnchorMachine` | Mirrored trusted operation peers, peer-input admission observations, runtime-work admission observations | Delete as an owner; derive from roster/wiring/peer bridge state | Derived-only |

## Perimeter

| Current machine | Owned facts today | Proposed fate | Placement |
| --- | --- | --- | --- |
| `ScheduleLifecycleMachine` | Schedule revision and phase, trigger binding, target binding, misfire/overlap/missing-target policy, planning cursor | Keep outside the present kernel boundary; if later promoted, make it part of a third kernel rather than folding it into `MeerkatMachine` or `MobMachine` | Perimeter |
| `OccurrenceLifecycleMachine` | Occurrence claim/lease truth, dispatch stage, completion, skip/misfire/delivery failure/supersession truth | Keep outside the present kernel boundary; if later promoted, make it part of the same third kernel as scheduling | Perimeter |

## Notes

- This matrix intentionally treats the current runtime spine as one over-split kernel:
  - `InputLifecycleMachine`
  - `RuntimeIngressMachine`
  - `RuntimeControlMachine`
  - `TurnExecutionMachine`
  - `SessionTurnAdmissionMachine`
  - `OpsLifecycleMachine`
  - `SessionToolVisibilityMachine`
  - `PeerCommsMachine`
  - `PeerDirectoryReachabilityMachine`
  - `CommsDrainLifecycleMachine`
  - `ExternalToolSurfaceMachine`
- This matrix intentionally treats the current mob side as one orchestration kernel plus internal flow sub-automata:
  - `MobLifecycleMachine`
  - `MobOrchestratorMachine`
  - `FlowRunMachine`
  - `FlowFrameMachine`
  - `LoopIterationMachine`
- The four mob anchor machines are treated as transitional observation scaffolding rather than final semantic owners.
- The abstract member contract between `MeerkatMachine` and `MobMachine` is described in `abstract-member-contract.md`.
