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

There are now two audit layers:

- the acceptance/surface map:

  ```bash
  cargo test -p meerkat-runtime audit_meerkat_runtime_phase_parity_map \
    -- --ignored --nocapture
  ```

- the stricter full-row map that also probes schema `same_surface` rows using a
  schema-aligned runtime field projection:

  ```bash
  cargo test -p meerkat-runtime audit_meerkat_runtime_phase_full_parity_map \
    -- --ignored --nocapture
  ```

## Current Snapshot (2026-04-15)

- Audited mixed-phase pairs: `Attached ↔ Idle`, `Attached ↔ Running`,
  `Running ↔ Stopped`, `Running ↔ Retired`, `Idle ↔ Retired`,
  `Idle ↔ Stopped`, `Attached ↔ Retired`, `Attached ↔ Stopped`,
  `Idle ↔ Running`, `Retired ↔ Stopped`
- Interesting rows in scope: `322`
- Live-probed rows: `322`
- `fixed`: `322`
- `align_schema`: `0`
- `align_runtime`: `0`
- `needs_decision`: `0`

Current state:

- the Meerkat registration-helper tranche is aligned
- the durable tool-visibility tranche is aligned
- the helper/query tranche is aligned
- the reducer/control tranche is aligned
- the cross-pair public-phase expansion tranche is aligned
- Meerkat acceptance parity is now green across the current public-phase
  frontier

## Full-row Snapshot (2026-04-15)

- Audited mixed-phase pairs: `Attached ↔ Idle`, `Attached ↔ Running`,
  `Running ↔ Stopped`, `Running ↔ Retired`, `Idle ↔ Retired`,
  `Idle ↔ Stopped`, `Attached ↔ Retired`, `Attached ↔ Stopped`,
  `Idle ↔ Running`, `Retired ↔ Stopped`
- Full rows in scope: `340`
- Live-probed rows: `340`
- aligned: `339`
- mismatched: `1`
- unprobed: `0`

Current exact-parity state:

- acceptance parity is still green
- modeled formal-state parity is green at `185 / 185`
- exact full-row parity is down to one remaining mismatch
- the pair audit now compares runtime behavior against the simulated schema
  outcome from the same representative pre-state rather than against static
  transition topology

Remaining mismatch:

- pair: `Attached ↔ Running`
- input: `Destroy`
- schema classification: `same_surface`
- runtime classification: `different_surface`
- runtime-visible difference: `DestroyReport.inputs_abandoned` is `0` from
  `Attached` and `1` from `Running`

Interpretation:

- the Meerkat schema is no longer missing obvious acceptance guards on the
  current public-phase frontier
- the remaining exact gap is no longer a broad phase/guard problem
- the remaining exact gap is about report/input-ledger semantics that the
  current formal machine does not yet encode
- this is the concrete answer to “is the machine under-modeled?”:
  yes, but now in a single named gap rather than in the already-audited command
  surface

## Resolution Rubric

| Label | Meaning |
| --- | --- |
| `fixed` | The live runtime probe agrees with the current schema classification for this pair/input row. |
| `align_schema` | Runtime behavior and the current architecture docs already point one way strongly enough that the schema should be widened or reshaped to match. There are no current Meerkat rows in this bucket. |
| `align_runtime` | The schema is carrying the intended contract and the runtime should be tightened to match it. There are no current Meerkat rows in this bucket. |
| `needs_decision` | We do not yet have enough evidence to align either side safely. There are no current Meerkat rows in this bucket. |

## Family Read

- `RegisterSession`, `StagePersistentFilter`, `RequestDeferredTools`, and
  `PublishCommittedVisibleSet` are now `fixed` across the audited frontier.
  The schema was widened to match the runtime’s extant-binding and durable
  visibility-owner behavior.
- The helper/query family (`EnsureSessionWithExecutor`, `SetSilentIntents`,
  `ContainsSession`, `SessionHasExecutor`, `SessionHasComms`,
  `OpsLifecycleRegistry`, `InputState`, `ListActiveInputs`) is also now
  `fixed` across the audited frontier.
- The reducer/control family is now also closed. Three sub-results matter:
  - direct runtime probes confirmed that `Recycle`, `Prepare`, `Commit`, and
    `Fail` already matched the current schema surface for the audited frontier
  - the schema was widened to match the runtime’s existing acceptance surface
    for `Abort*`, `Wait`, `Ingest`, `PublishEvent`, `RuntimeState`,
    `LoadBoundaryReceipt`, and `Accept*`
  - `InterruptCurrentRun` and `CancelAfterBoundary` are now modeled as
    attached-loop control commands: `Attached` accepts them as self-loops,
    while `Running` keeps the active-work surface and `Idle` / `Retired` /
    `Stopped` still reject them

## Pair Ledger

- `Attached ↔ Idle`: `33` interesting, `33` probed, `33` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Running`: `35` interesting, `35` probed, `35` fixed, `0`
  mismatches, `0` unprobed
- `Running ↔ Stopped`: `33` interesting, `33` probed, `33` fixed, `0`
  mismatches, `0` unprobed
- `Running ↔ Retired`: `35` interesting, `35` probed, `35` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Retired`: `27` interesting, `27` probed, `27` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Stopped`: `30` interesting, `30` probed, `30` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Retired`: `34` interesting, `34` probed, `34` fixed, `0`
  mismatches, `0` unprobed
- `Attached ↔ Stopped`: `34` interesting, `34` probed, `34` fixed, `0`
  mismatches, `0` unprobed
- `Idle ↔ Running`: `36` interesting, `36` probed, `36` fixed, `0`
  mismatches, `0` unprobed
- `Retired ↔ Stopped`: `25` interesting, `25` probed, `25` fixed, `0`
  mismatches, `0` unprobed

The final live mismatch cluster was `Attached ↔ Running` on
`InterruptCurrentRun` and `CancelAfterBoundary`. Closing it required modeling
the existing attached-loop control-channel behavior rather than tightening the
runtime.

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

Status: complete

Closed rows:

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

Outcome:

- the runtime probe surface now covers the full currently targeted Meerkat
  reducer/control acceptance frontier
- the schema widening needed for this batch is landed and verified
- there are no current Meerkat acceptance mismatches left in the initial
  three-pair tranche

### Batch D: Cross-pair public-phase expansion

Status: complete

Closed pairs:

- `Attached ↔ Running`
- `Running ↔ Retired`
- `Idle ↔ Stopped`
- `Attached ↔ Retired`
- `Attached ↔ Stopped`
- `Idle ↔ Running`
- `Retired ↔ Stopped`

Outcome:

- the Meerkat runtime parity map now covers the full public-phase frontier we
  care about for the current simplification pass
- the last live mismatch cluster was closed by adding `Attached` self-loops for
  `InterruptCurrentRun` and `CancelAfterBoundary`
- there are no current Meerkat acceptance mismatches left in the 10-pair
  audited frontier

## Next Loop

1. Keep using the acceptance map as the “green frontier” for command-surface
   parity.
2. Decide whether `DestroyReport.inputs_abandoned` belongs inside the formal
   machine authority boundary or remains an intentionally out-of-model report
   surface.
3. Use the trustworthy post-parity Hopcroft rerun as the Meerkat
   simplification baseline:
   raw `59,371 -> 385`, phase `59,371 -> 390`, full `59,371 -> 58,038`.
4. Read that baseline together with the largest-block field projection from
   [`docs/architecture/machine-simplification-proposal.md`](machine-simplification-proposal.md):
   the dominant Meerkat mixed block is now measured as `32,248` states over
   `16,859` extended-state tuples, with `9,647` tuples reused across multiple
   phases.
5. Read that baseline together with the now-green Mob lifecycle-triangle
   ledger in
   [`docs/architecture/mob-runtime-schema-parity-ledger.md`](mob-runtime-schema-parity-ledger.md).
6. Feed the refreshed mixed-phase blocks into the DSL-design work instead of
   the older pre-expansion bearings.
