# Mob Runtime/Schema Parity Ledger

## Purpose

This ledger is the checked-in burn-down surface for Mob runtime/schema parity.
It is derived from the ignored in-crate audit:

```bash
cargo test -p meerkat-mob audit_mob_runtime_phase_parity_map \
  -- --ignored --nocapture
```

The goal is to keep the Mob simplification loop honest:

- close runtime/schema gaps before using Hopcroft results as simplification
  evidence
- distinguish real schema/runtime disagreement from bad probe setup
- evaluate guard-sensitive rows against representative pre-states instead of
  treating every phase-local transition as always enabled

The live JSON report is written to the system temp directory as
`mob-runtime-phase-parity.json`.

## Current Snapshot (2026-04-15)

- Audited lifecycle pairs: `Running ↔ Stopped`, `Completed ↔ Running`,
  `Completed ↔ Stopped`
- Transition-bearing rows in scope: `65`
- Live-probed rows: `65`
- `fixed`: `65`
- `align_schema`: `0`
- `align_runtime`: `0`
- `needs_decision`: `0`

Current state:

- the Mob lifecycle triangle is fully probed and green on the current
  representative pre-state frontier
- the audit now evaluates guard-sensitive schema rows against the representative
  runtime pre-snapshots where it has enough field information to do so
- the transition-bearing parity surface is exact for the current triangle:
  `65 / 65 / 65 / 0 / 0`
- the formal machine no longer carries the pure shadow fields
  `inflight_work_id`, `task_count`, or `event_subscription_count`
- the formal machine also no longer carries the seam-shadow
  `current_generation` field; `RequestRuntimeBinding` now emits generation
  directly from the transition bindings instead of replaying a top-level
  stored copy

## Resolution Rubric

| Label | Meaning |
| --- | --- |
| `fixed` | The live runtime probe agrees with the current schema classification for this pair/input row. |
| `align_schema` | Runtime behavior and the current architecture notes already point strongly enough one way that the schema should be reshaped to match. There are no current Mob rows in this bucket. |
| `align_runtime` | The schema is carrying the intended contract and the runtime should be tightened to match it. There are no current Mob rows in this bucket. |
| `needs_decision` | We do not yet have enough evidence to align either side safely. There are no current Mob rows in this bucket. |

## What Changed

- The audit fixture now builds phase-appropriate representative states for the
  full `Running` / `Stopped` / `Completed` triangle instead of only the original
  high-signal mismatch rows.
- `TaskUpdate` and `CancelFlow` were closed as probe-shape issues:
  - `TaskUpdate` needed a real owner-bearing fixture
  - `CancelFlow` needed a synthetic run id on non-`Running` sides so phase
    rejection could be observed without trying to stop an active run first
- `SubscribeAgentEvents` was the last real schema gap in the triangle. The
  runtime requires a live member, so the schema now models that with
  `active_members_present`, and the audit evaluates that guard against the
  representative pre-state.
- The next simplification tranche removed three pure shadow fields from the
  formal machine:
  - `inflight_work_id`
  - `task_count`
  - `event_subscription_count`
- None of those fields participated in guards or emitted semantics, and the
  exact lifecycle-triangle parity audit stayed green after the cut.
- The following tranche removed `current_generation` from the formal state and
  switched `RequestRuntimeBinding` generation emission to come directly from the
  transition bindings.
- That cut also stayed green on the exact lifecycle triangle, and the truthful
  raw/phase/full Hopcroft readout remained unchanged (`1,390 -> 202`,
  `1,390 -> 204`, `1,390 -> 1,390`), which means `current_generation` was
  fully correlated with the remaining state rather than carrying independent
  labeled behavior.

## Pair Ledger

- `Running ↔ Stopped`: `27` rows, `27` probed, `27` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Running`: `26` rows, `26` probed, `26` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Stopped`: `12` rows, `12` probed, `12` fixed, `0` mismatches,
  `0` unprobed

## Readout

- The lifecycle triangle is now exact on the audited transition surface.
- The broad quotient result still stands after the parity cleanup, and the
  shadow-field cut made that read much cleaner: the raw Mob quotient stayed at
  `202` even while the truthful state space collapsed from `4,797` to `1,390`.
- The trustworthy raw/phase/full reread is now:
  raw `1,390 -> 202`, phase `1,390 -> 204`, full `1,390 -> 1,390`.
- The dominant mixed block (`705` states) is now split primarily by
  `pending_spawn_count`, `wiring_edge_count`, `active_run_count`,
  `active_member_count`, `retiring_member_count`, `active_fence_token`,
  `active_runtime_id`, and `coordinator_bound`.

## Notes

- The audit loop also smoke-probes the surfaced query/inspection inputs while it
  walks the pair matrix, but the `65` counted rows above are the transition-
  bearing rows used for exact schema/runtime classification.
- This ledger is intentionally narrower than the Meerkat one: it is focused on
  the lifecycle triangle that dominates the current Mob mixed-phase quotient
  block.

## Next Loop

1. Decide whether `active_runtime_id` / `active_fence_token` / `active_identity`
   are genuinely load-bearing Mob semantics or the next seam-shadow family to
   normalize.
2. Re-run the same parity-plus-Hopcroft loop after that decision before making
   any broader DSL-facing claims.
