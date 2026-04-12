# Two-Kernel Shadow Sink Schema

## Purpose

Now that the rebased branch has its first **live** mismatch-producing shadow
run, the pre-cutover program needs a single export shape for collected shadow
taxonomy rather than only in-test assertions.

This document defines the first shared sink/export schema for:

- `MeerkatMachine` aggregate shadow suites
- `MobMachine` aggregate shadow suites
- `Mob↔Meerkat` seam aggregate shadow suites

The sink is intentionally **taxonomy-oriented**, not raw-snapshot-oriented.
Raw machine snapshots stay local to the machine helpers; the sink carries only
the collected mismatch summary needed for cutover triage.

## Export Unit

One exported unit represents one scenario sample.

Suggested shape:

```text
TwoKernelShadowScenarioSample {
  scenario_id: String,
  phase: String,
  timestamp: String,
  meerkat_buckets: Vec<ShadowTaxonomyBucket>,
  mob_buckets: Vec<ShadowTaxonomyBucket>,
  seam_buckets: Vec<ShadowTaxonomyBucket>,
}
```

Where `ShadowTaxonomyBucket` is normalized to:

```text
ShadowTaxonomyBucket {
  scope: "meerkat" | "mob" | "composition",
  lane: String,
  region: String,
  triage: "implementation_detail" | "semantic_gap" | "dogma_violation",
  count: u64,
}
```

Multiple scenario samples from one cutover-facing run may then be grouped as:

```text
TwoKernelShadowScenarioBatch {
  run_id: String,
  samples: Vec<TwoKernelShadowScenarioSample>,
}
```

Multiple cutover-facing runs from one shadow collection session may then be
grouped as:

```text
TwoKernelShadowReportSession {
  session_id: String,
  runs: Vec<TwoKernelShadowRunBatch>,
}
```

And collapsed into one top-level triage summary as:

```text
TwoKernelShadowReportSessionSummary {
  session_id: String,
  run_count: u64,
  scenario_count: u64,
  sample_count: u64,
  green_sample_count: u64,
  mismatch_sample_count: u64,
  meerkat_bucket_count: u64,
  mob_bucket_count: u64,
  seam_bucket_count: u64,
}
```

The first machine-readable export helper now serializes the full
`TwoKernelShadowReportSession` as pretty JSON, so the first brutal cutover can
emit one real artifact without introducing a second ad hoc sink format.

## Scenario Fields

- `scenario_id`
  - stable scenario key from the shadow scenario matrix
  - examples:
    - `meerkat.reset.pending_ops`
    - `mob.flow.single_step`
    - `seam.live_bridge_loss`
- `phase`
  - stable point inside the scenario
  - examples:
    - `initial`
    - `active`
    - `settled`
    - `post_archive`
- `timestamp`
  - wall-clock capture timestamp
  - only for ordering/traceability, never machine semantics

## Scope Rules

- `meerkat_buckets`
  - only buckets derived from `summarize_meerkat_shadow_taxonomy_reports(...)`
- `mob_buckets`
  - only buckets with `scope == "mob"` from
    `summarize_mob_shadow_taxonomy_reports(...)`
- `seam_buckets`
  - only buckets with `scope == "composition"` from
    `summarize_mob_shadow_taxonomy_reports(...)`

The seam remains exported from the Mob-side aggregate because that is where the
current composition shadow helpers live.

Current implementation status on the rebased baseline:

- `MeerkatMachine` can now export:
  - empty broader-smoke samples
  - seeded lifecycle/control drift samples
  - paired `seam.live_bridge_loss` samples while the bridge is still live
  - paired empty `seam.live_bridge_loss` samples after archive on the current
    rebased baseline
- `MobMachine` can now export:
  - the first live seam-mismatch sample (`seam.live_bridge_loss`)

## First Required Scenario

The first scenario that must round-trip through this sink is:

- `seam.live_bridge_loss`

Expected first real buckets:

- `composition / LifecycleSupersession / lifecycle`
- `composition / WorkBridge / work`

Current observed triage on the rebased baseline:

- `composition / LifecycleSupersession / lifecycle / implementation_detail`
- `composition / WorkBridge / work / semantic_gap`

This is the first **live** mismatch-producing taxonomy class on the rebased
baseline, and it should become the anchor sample for any early sink/export
implementation.

Current paired-run truth on the rebased baseline:

- active phase:
  - `MeerkatMachine` exports an empty sample
  - `MobMachine` exports an empty sample
- `post_archive` phase:
  - `MeerkatMachine` currently still exports an empty sample
  - `MobMachine` exports the real seam mismatch buckets above

The first combined runner now exports this scenario as a two-sample batch:

- `run_id = "seam.live_bridge_loss"`
- sample `0`: `active`
- sample `1`: `post_archive`

The first reusable multi-scenario run now exports:

- `run_id = "shadow.cutover.smoke"`
- scenario batch `0`: `mob.flow.single_step.green`
  - one empty `active` sample
- scenario batch `1`: `seam.live_bridge_loss`
  - one non-empty `post_archive` sample carrying the real seam mismatch buckets

This is the first cutover-facing export shape that can hold both:

- a green scenario
- a mismatch-producing scenario

under the same run session without flattening them into one synthetic scenario.

The first shared report session now exports:

- `session_id = "shadow.cutover.session"`
- run batch `0`:
  - `run_id = "shadow.cutover.smoke"`
  - one green scenario batch
  - one mismatch-producing scenario batch
- run batch `1`:
  - `run_id = "shadow.cutover.green-only"`
  - one green-only scenario batch

The first genuinely multi-run live report session now exports:

- `session_id = "shadow.cutover.multi-run"`
- run batch `0`:
  - `run_id = "shadow.cutover.green-run"`
  - one runtime-backed green scenario batch
- run batch `1`:
  - `run_id = "shadow.cutover.drift-run"`
  - one runtime-backed drift scenario batch

This is the first session export that is not assembled from one mixed run. It
collects:

- one healthy live runtime-backed run
- one mismatch-producing live runtime-backed run

under the same shared report-session shape.

The first session-level triage summary now proves:

- `run_count = 2`
- `scenario_count = 2`
- `sample_count = 2`
- `green_sample_count = 1`
- `mismatch_sample_count = 1`
- `seam_bucket_count >= 2`

The first JSON export proof now round-trips:

- `session_id`
- run-level ids
- scenario batch ids
- sample ids
- seam bucket labels

## Non-Goals

- exporting full machine snapshots
- exporting raw event logs
- exposing a stable end-user API
- persisting every green/empty happy-path sample

The first sink is for pre-cutover engineering triage only.

## Reopen Conditions

Reopen this schema only if:

- a new machine or seam scope needs to be exported
- raw snapshot attachment becomes necessary for triage
- live mismatch taxonomy proves insufficiently expressive in cutover runs
