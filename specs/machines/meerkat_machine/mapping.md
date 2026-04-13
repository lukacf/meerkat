# MeerkatMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MeerkatMachine`

### Code Anchors
- `meerkat_machine`: `meerkat-runtime/src/meerkat_machine.rs` — authoritative MeerkatMachine command dispatch and state ownership
- `meerkat_public_surface`: `meerkat/src/meerkat_machine.rs` — MeerkatMachine snapshot/diagnostic facade

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work

### Transitions
- `Initialize`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RegisterSession`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `PrepareBindings`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `BeginRunFromIdle`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `AdmissionAcceptedIdleSteer`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `InterruptCurrentRun`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `CancelAfterBoundary`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `BoundaryApplied`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `PublishCommittedVisibleSet`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RunCompleted`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RunFailed`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RunCancelled`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RecoverRuntime`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RetireRequestedFromIdle`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `ResetRuntime`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `StopRuntimeExecutor`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `DestroyRuntime`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`

### Effects
- `RuntimeBound`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RuntimeRetired`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RuntimeDestroyed`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `WorkCompleted`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `WorkFailed`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `WorkCancelled`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RequestCancellationAtBoundary`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `CommittedVisibleSetPublished`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `RuntimeNotice`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`

### Invariants
- `running_has_active_work`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `bound_runtime_has_fence`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`
- `destroyed_has_no_active_work`
  - anchors: `meerkat_machine`, `meerkat_public_surface`
  - scenarios: `bind-run-boundary-terminal`, `retire-reset-destroy`


<!-- GENERATED_COVERAGE_END -->
