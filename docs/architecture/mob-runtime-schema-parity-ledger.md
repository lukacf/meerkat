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

## Pair Ledger

- `Running ↔ Stopped`: `27` rows, `27` probed, `27` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Running`: `26` rows, `26` probed, `26` fixed, `0` mismatches,
  `0` unprobed
- `Completed ↔ Stopped`: `12` rows, `12` probed, `12` fixed, `0` mismatches,
  `0` unprobed

## Readout

- The lifecycle triangle is now exact on the audited transition surface.
- The broad quotient result still stands after the parity cleanup, but it is
  now slightly stricter: the raw Mob quotient moved from `195` to `202` once
  the schema stopped over-permitting `SubscribeAgentEvents` from
  member-less completed states.
- The trustworthy raw/phase/full reread is now:
  raw `4,797 -> 202`, phase `4,797 -> 204`, full `4,797 -> 4,797`.
- The dominant mixed block (`2,819` states) is split primarily by
  `event_subscription_count`, `pending_spawn_count`, `task_count`,
  `wiring_edge_count`, `active_run_count`, `active_member_count`,
  `retiring_member_count`, and `active_fence_token`.

## Notes

- The audit loop also smoke-probes the surfaced query/inspection inputs while it
  walks the pair matrix, but the `65` counted rows above are the transition-
  bearing rows used for exact schema/runtime classification.
- This ledger is intentionally narrower than the Meerkat one: it is focused on
  the lifecycle triangle that dominates the current Mob mixed-phase quotient
  block.

## Next Loop

1. Use the exact Mob parity baseline together with the trustworthy Hopcroft
   reread when comparing Mob to Meerkat and planning DSL work.
2. Decide whether the next Mob pass should widen into the query/surface-only
   family or whether the better next gap is the Meerkat big-block field-split
   analysis for DSL design.
