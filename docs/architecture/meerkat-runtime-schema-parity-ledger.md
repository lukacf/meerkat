# Meerkat Runtime/Schema Parity Ledger

## Purpose

This ledger is the checked-in burn-down surface for Meerkat runtime/schema
parity. It is derived from the ignored in-crate audit:

```bash
cargo test -p meerkat-runtime audit_meerkat_runtime_phase_parity_map \
  -- --ignored --nocapture
```

The goal is to drive the Meerkat parity loop in a controlled way:

- keep runtime as the provisional oracle only when real tests and the design
  docs support it
- avoid widening the schema blindly just to make the quotient look smaller
- separate already-verified rows from rows that still need direct probes

## Current Snapshot (2026-04-14)

- Audited mixed-phase pairs: `Attached ↔ Idle`, `Running ↔ Stopped`,
  `Idle ↔ Retired`
- Interesting rows in scope: `76`
- Live-probed rows: `53`
- `fixed`: `53`
- `align_schema`: `0`
- `align_runtime`: `0`
- `needs_decision`: `23`

Current state:

- the Meerkat registration-helper tranche is aligned
- the durable tool-visibility tranche is aligned
- the helper/query tranche is aligned
- the only open Meerkat audit rows are the reducer/control family in Batch C

## Resolution Rubric

| Label | Meaning |
| --- | --- |
| `fixed` | The live runtime probe agrees with the current schema classification for this pair/input row. |
| `align_schema` | Runtime behavior and the current architecture docs already point one way strongly enough that the schema should be widened or reshaped to match. There are no current Meerkat rows in this bucket. |
| `align_runtime` | The schema is carrying the intended contract and the runtime should be tightened to match it. There are no current Meerkat rows in this bucket. |
| `needs_decision` | We do not yet have enough evidence to align either side safely. In the current ledger this means the row still needs a direct reducer/control probe. |

## Family Read

- `RegisterSession`, `StagePersistentFilter`, `RequestDeferredTools`, and
  `PublishCommittedVisibleSet` are now `fixed` for the audited pairs. The
  schema was widened to match the runtime’s extant-binding and durable
  visibility-owner behavior.
- The helper/query family (`EnsureSessionWithExecutor`, `SetSilentIntents`,
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`) is also now
  `fixed` for the audited pairs.
- The remaining uncertainty is concentrated in the reducer/control family:
  `Abort*`, `Wait`, `Ingest`, `PublishEvent`, `RuntimeState`,
  `LoadBoundaryReceipt`, `Accept*`, `Recycle`, `Prepare`, `Commit`, and `Fail`.

## Pair Ledger

### Attached ↔ Idle

Summary: 25 interesting rows, 15 probed, 15 fixed, 0 mismatches, 10 still to
probe.

| Input | Schema | Audit | Runtime | Proposed | Batch |
| --- | --- | --- | --- | --- | --- |
| `RegisterSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ReconfigureSessionLlmIdentity` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `SetPeerIngressContext` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `NotifyDrainExited` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `StagePersistentFilter` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `RequestDeferredTools` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `PublishCommittedVisibleSet` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Recover` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SetSilentIntents` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ContainsSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasExecutor` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasComms` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `OpsLifecycleRegistry` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `InputState` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ListActiveInputs` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Abort` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AbortAll` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Wait` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Ingest` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `PublishEvent` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `RuntimeState` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `LoadBoundaryReceipt` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AcceptWithCompletion` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AcceptWithoutWake` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Recycle` | `different_surface` | `unprobed` | `-` | `needs_decision` | `C` |

### Running ↔ Stopped

Summary: 32 interesting rows, 21 probed, 21 fixed, 0 mismatches, 11 still to
probe.

| Input | Schema | Audit | Runtime | Proposed | Batch |
| --- | --- | --- | --- | --- | --- |
| `RegisterSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ReconfigureSessionLlmIdentity` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `PrepareBindings` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SetPeerIngressContext` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `NotifyDrainExited` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `InterruptCurrentRun` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `CancelAfterBoundary` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `StagePersistentFilter` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `RequestDeferredTools` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `PublishCommittedVisibleSet` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Recover` | `right_only` | `aligned` | `right_only` | `fixed` | `done` |
| `Retire` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `StopRuntimeExecutor` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `EnsureSessionWithExecutor` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SetSilentIntents` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ContainsSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasExecutor` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasComms` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `OpsLifecycleRegistry` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `InputState` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ListActiveInputs` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Abort` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AbortAll` | `different_surface` | `unprobed` | `-` | `needs_decision` | `C` |
| `Wait` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Ingest` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `PublishEvent` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `RuntimeState` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `LoadBoundaryReceipt` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AcceptWithCompletion` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `AcceptWithoutWake` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Commit` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Fail` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |

### Idle ↔ Retired

Summary: 19 interesting rows, 17 probed, 17 fixed, 0 mismatches, 2 still to
probe.

| Input | Schema | Audit | Runtime | Proposed | Batch |
| --- | --- | --- | --- | --- | --- |
| `RegisterSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `PrepareBindings` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SetPeerIngressContext` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `NotifyDrainExited` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `StagePersistentFilter` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `RequestDeferredTools` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `PublishCommittedVisibleSet` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Recover` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `Retire` | `left_only` | `aligned` | `left_only` | `fixed` | `done` |
| `EnsureSessionWithExecutor` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SetSilentIntents` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ContainsSession` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasExecutor` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `SessionHasComms` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `OpsLifecycleRegistry` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `InputState` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `ListActiveInputs` | `different_surface` | `aligned` | `different_surface` | `fixed` | `done` |
| `AbortAll` | `right_only` | `unprobed` | `-` | `needs_decision` | `C` |
| `Prepare` | `left_only` | `unprobed` | `-` | `needs_decision` | `C` |

## Batch Status

### Batch A: Registration and durable visibility

Status: complete

Closed rows:

- `RegisterSession`
- `StagePersistentFilter`
- `RequestDeferredTools`
- `PublishCommittedVisibleSet`

Outcome:

- the original Meerkat schema/runtime mismatch cluster is gone for the audited
  pairs

### Batch B: Helper/query parity

Status: complete

Closed rows:

- `EnsureSessionWithExecutor`
- `SetSilentIntents`
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`

Outcome:

- the helper/query family is now live-probed and aligned for the audited pairs

### Batch C: Reducer/control probe expansion

Status: open

Open rows:

- `Abort`
- `AbortAll`
- `Wait`
- `Ingest`
- `PublishEvent`
- `RuntimeState`
- `LoadBoundaryReceipt`
- `AcceptWithCompletion`
- `AcceptWithoutWake`
- `Recycle`
- `Prepare`
- `Commit`
- `Fail`

Exit condition:

- reducer/control rows in this ledger are no longer `unprobed`
- any genuine remaining contract disagreements are narrow enough to classify as
  `align_schema` or `align_runtime`

## Next Loop

1. Extend the reducer/control probes in Batch C.
2. Rerun the ignored Meerkat parity audit and refresh this ledger.
3. Once the Meerkat acceptance surface is fully probed, rerun Hopcroft and use
   the new mixed-phase blocks as the simplification input to the DSL work.
