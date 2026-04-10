# MobMachine Fairness Assumptions

Status: frozen target-state liveness handoff

This note records the fairness assumptions for target-state `MobMachine`
liveness work.

## Purpose

The target machine proof must not smuggle scheduler or runtime optimism into
safety invariants. When liveness is claimed, it must be justified against
explicit fairness assumptions.

## Assumptions

These assumptions map directly to the named fairness bundle `TargetFairness`
inside `tla/MobMachineTarget.tla`, grouped as:

- `ProvisionProgress`
- `KickoffProgress`
- `WorkProgress`
- `FlowDispatchProgress`
- `FlowTerminalProgress`
- `FlowStructureProgress`
- `RecoveryProgress`
- `HistoryTaskProgress`

The frozen package now also carries named temporal proof targets in
`MobMachineTarget.tla`, but they are not yet canonical freeze gates because the
current executable scaffold still uses `StateConstraint == step_count <=
MaxSteps` to bound the state space.

### Provision fairness

If a provision action remains enabled for an identity forever, then either:

- it eventually finalizes
- or it eventually fails

### Work fairness

If live work remains accepted or running forever and is not fenced, then it
eventually terminalizes.

### Flow-step fairness

If a run remains running and step/work progress remains enabled forever, then
the machine eventually takes the corresponding progress class:

- step dispatch
- step completion/failure/skip/cancel
- branch resolution
- collection resolution
- frame / loop structural advancement
- run terminalization

The focused flow proof harness also includes `WorkProgress`, because
step-dispatch liveness depends on the work ledger being allowed to accept,
start, and terminalize bound work.

### Recovery fairness

If a restore failure is repairable and repair remains enabled forever, then the
machine eventually clears the failure or re-records a newer authoritative
failure.

The focused recovery proof harness now checks the clear path directly.

### History / task fairness

If durable event or task persistence is enabled forever, then append/update
effects eventually complete.

The focused task proof harness now checks the close path directly.

## Non-assumptions

The target proof must not assume:

- instant Meerkat acceptance
- instant network delivery
- instant scheduler wakeups
- instant operator repair

Those appear only as separate perimeter assumptions in later refinement work;
they are not implicit Mob liveness assumptions.
