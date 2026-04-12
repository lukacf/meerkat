# Two-Kernel Cutover Readiness

This note is the final pre-cutover handoff for the current rebased baseline.

It answers one narrow question:

- are `MeerkatMachine`, `MobMachine`, and the seam strong enough to start the
  brutalist cutover phase?

The answer is yes.

## What Is Frozen

- `MeerkatMachine` target freeze and exact-current baseline are closed.
- `MobMachine` target freeze and exact-current baseline are closed.
- the `MobMachine` <-> `MeerkatMachine` seam contract is frozen at the small
  member abstraction boundary.

## What Is Proven

- `MeerkatMachine` bounded safety passes.
- `MobMachine` bounded safety passes.
- `MobMachine` focused liveness lanes pass.
- the seam composition safety and focused liveness lanes pass.

That is enough to stop treating the machines as speculative and start treating
them as cutover candidates.

## What Is Shadowed

The first-wave shadow lanes are implemented and green on the rebased baseline.

`MeerkatMachine`:

- lifecycle/control
- tools
- turn/ops/barrier
- peer/drain

`MobMachine`:

- provisioning/lifecycle
- flow/frame/loop
- task/history/recovery

Seam:

- lifecycle/supersession
- work bridge

The first-wave scenario matrix is complete, and both kernels now emit shared
scenario samples, scenario batches, run batches, and report sessions.

## What The Shadow Runs Already Taught Us

The current rebased baseline is not perfectly clean in contact with the seam.
The first live mismatch-producing run, `seam.live_bridge_loss`, already shows
two real classes:

- `composition / LifecycleSupersession / lifecycle / implementation_detail`
- `composition / WorkBridge / work / semantic_gap`

That is good news. It means the shadow stack is already catching real
implementation contact instead of only passing green happy paths.

## Why Cutover Is Justified Now

We now have all of the pieces we wanted before authority transfer:

- frozen target machines
- frozen exact-current baselines
- executable proof models
- explicit refinement deltas
- cutover-lowering inventories
- live shadow lanes
- aggregate scenario runners
- machine-readable sink exports
- real mismatch taxonomy from live drift

At this point, more freeze/proof authoring would mostly be churn.

## What Will Probably Break First

The most likely first-contact failures are:

- `MeerkatMachine.tools`
  - exact-current tool visibility vs target ownership centralization
- seam work bridge
  - Mob nonterminal work outliving the currently visible Meerkat bridge
- seam supersession bookkeeping
  - replacement/archive ordering around respawn and destroy
- old implementation bypasses
  - live code paths that still mutate around the machine-owned seam

These are expected cutover failures, not evidence that the machine work was a
mistake.

## Recommended Cutover Order

The architecture remains binary:

- one `MeerkatMachine`
- one `MobMachine`

But the authority transfer should still be staged inside that architecture:

1. transfer `MeerkatMachine` authority first
2. validate against the existing shadow/report sink
3. fix the first lowering/bypass failures
4. then transfer `MobMachine`
5. finally remove the old implementation-owned seams

## Success Standard For The First Brutalist Pass

The first cutover pass is successful if:

- the machines remain the source of truth
- failures can be classified quickly as:
  - `implementation_detail`
  - `semantic_gap`
  - `dogma_violation`
- the shadow/report sink gives us compact reproducible evidence
- we do not retreat back into speculative freeze authoring

## Bottom Line

The two-machine program is ready to leave the preparation phase.

We should now expect reality to break against the machines in a few places, and
that is exactly what the next phase is for.
