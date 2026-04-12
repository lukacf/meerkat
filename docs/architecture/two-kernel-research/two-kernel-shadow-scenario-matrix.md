# Two-Kernel Shadow Scenario Matrix

Status: active pre-cutover scenario map

This note turns the landed shadow lanes and aggregate suite helpers into a
concrete scenario matrix for the first cutover-facing runs.

The point is to make each scenario answer a specific refinement question:

- which machine/seam lanes are exercised
- which helper surface should collect the report
- which mismatch classes are most likely to surface

## Scenario 1: Plain Session Reset With Pending Ops

Primary suite helper:

- `capture_all_meerkat_shadow_reports(...)`

Expected active lanes:

- `LifecycleControl`
- `TurnOpsBarrier`
- `PeerDrain`

Likely mismatch classes:

- `implementation_detail`
- `dogma_violation`

## Scenario 2: Attached Recover / Recycle Replay

Primary suite helper:

- `capture_all_meerkat_shadow_reports(...)`

Expected active lanes:

- `LifecycleControl`
- `TurnOpsBarrier`

Likely mismatch classes:

- `implementation_detail`
- `semantic_gap`

## Scenario 3: Tool Visibility Mutation + MCP Apply

Primary suite helper:

- `capture_all_meerkat_shadow_reports(...)`

Expected active lanes:

- `Tools`

Likely mismatch classes:

- `implementation_detail`
- `dogma_violation`

## Scenario 4: Peer Ingress Receive / Trust Mutation / Drain

Primary suite helper:

- `capture_all_meerkat_shadow_reports(...)`

Expected active lanes:

- `PeerDrain`

Likely mismatch classes:

- `implementation_detail`
- `dogma_violation`

## Scenario 5: Mob Provision / Kickoff / Finalize

Primary suite helper:

- `capture_mob_shadow_suite_report(...)`

Expected active lanes:

- `ProvisioningLifecycle`
- `LifecycleSupersession`

Likely mismatch classes:

- `implementation_detail`
- `dogma_violation`

## Scenario 6: Single-Step Flow Run

Primary suite helper:

- `capture_mob_shadow_suite_report(...)`

Expected active lanes:

- `FlowFrameLoop`
- `WorkBridge`

Likely mismatch classes:

- `implementation_detail`
- `semantic_gap`

## Scenario 7: Branch Fallback With Nonterminal Failure History

Primary suite helper:

- `capture_mob_shadow_suite_report(...)`

Expected active lanes:

- `FlowFrameLoop`
- `TaskHistoryRecovery`
- `WorkBridge`

Likely mismatch classes:

- `semantic_gap`
- `implementation_detail`

## Scenario 8: Retire / Destroy Supersession

Primary suite helper:

- `capture_mob_shadow_suite_report(...)`

Expected active lanes:

- `ProvisioningLifecycle`
- `LifecycleSupersession`
- `WorkBridge`

Likely mismatch classes:

- `dogma_violation`
- `implementation_detail`

## Recommended execution order

Run the first cutover-facing shadow scenarios in this order:

1. plain session reset with pending ops
2. tool visibility mutation + MCP apply
3. peer ingress receive / trust mutation / drain
4. Mob provision / kickoff / finalize
5. single-step flow run
6. retire / destroy supersession
7. attached recover / recycle replay
8. branch fallback with nonterminal failure history

Why this order:

- it starts with the most mature Meerkat-only lanes
- then exercises the rebased tools seam
- then moves into the simplest Mob / seam scenarios
- and only afterwards takes on the higher-variance replay / branch paths

## Exit criteria

This matrix is usable once:

- every first cutover-facing scenario names one primary suite helper
- every scenario maps to landed lanes
- every scenario identifies likely mismatch classes up front

The next implementation step is to run these scenarios in code and record the
first real mismatch taxonomy.
