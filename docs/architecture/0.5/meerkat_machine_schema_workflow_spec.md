# Meerkat 0.5 Machine Schema Workflow

Status: normative `0.5` workflow spec

## Purpose

This document defines the concrete workflow for connecting:

- machine contracts
- TLA+ models
- Rust kernels
- code generation for schema-eligible machines
- repo CI gates

It turns the formalization strategy into an executable implementation workflow.

## Scope

This workflow applies to the full canonical `0.5` machine set.

It does **not** require every machine to use schema generation.

Instead:

- every machine must be semantically verified
- only `SchemaKernel` machines use a checked-in declarative schema as the
  authoritative code-generation source

## Final Repo Layout

The steady-state repo layout is:

```text
docs/architecture/0.5/
  outline.md
  nomenclature.md
  execution_plan.md
  host_mode_cutover.md
  ops_lifecycle_seam.md
  surface_cutover_matrix.md
  machine_formalization_strategy.md
  machine_schema_workflow.md

specs/machines/<machine>/
  contract.md
  model.tla
  ci.cfg
  mapping.md
  schema.yaml              # only for SchemaKernel machines
  generated/               # generated outputs when applicable

xtask/
  src/machines.rs

meerkat-machine-schema/    # schema parser + validator crate
meerkat-machine-codegen/   # rust/tla/doc codegen crate
```

Current rule:

- the formal machine bundle now lives in-repo under `specs/machines/`
- implementation gating is only real if reviewers and CI use the in-repo
  artifacts rather than any out-of-repo copy

## Artifact Rules

### Required for every machine

Every canonical machine must have:

- `contract.md`
- `model.tla`
- `ci.cfg`
- `mapping.md`

### Required only for `SchemaKernel` machines

Schema-kernel machines additionally must have:

- `schema.yaml`
- generated Rust kernel
- generated contract tables
- generated TLA+ model

### Required only for `PureHandKernel` machines

Pure-hand machines must additionally have:

- one explicit Rust `step/apply/transition` kernel
- kernel-focused Rust tests in the owning crate

## Authoritative Source By Mode

### `SchemaKernel`

Authoritative source:

- `schema.yaml`

Generated from it:

- Rust kernel
- `contract.md`
- `model.tla`
- transition tables/diagrams

`mapping.md` remains hand-authored because it connects abstract semantics to
the surrounding impure shell.

### `PureHandKernel`

Authoritative semantic sources:

- `contract.md`
- `model.tla`

Authoritative Rust source:

- hand-written pure kernel

`mapping.md` explains the abstraction boundary between the formal model and the
hand-written kernel.

## Schema Format

`schema.yaml` is intentionally small and declarative.

Required top-level keys:

- `machine`
- `version`
- `rust`
- `state`
- `inputs`
- `effects`
- `terminal_states`
- `invariants`
- `transitions`

Required semantic content:

- closed state variants
- closed input variants
- closed effect variants
- explicit transition rows
- guards by name
- target state
- emitted effects
- invariant list

Illustrative shape:

```yaml
machine: RuntimeControlMachine
version: 1
rust:
  crate: meerkat-runtime
  module: machines::runtime_control
state:
  enum: RuntimeControlState
  variants:
    - Uninitialized
    - Idle
    - Running
    - Yielding
    - Retiring
    - Stopped
inputs:
  enum: RuntimeControlInput
  variants:
    - AdmitWork
    - RunStarted
    - RunCompleted
    - ControlStop
effects:
  enum: RuntimeControlEffect
  variants:
    - StartRun
    - StopRuntime
terminal_states:
  - Stopped
invariants:
  - stopped_rejects_further_work
transitions:
  - from: Idle
    on: AdmitWork
    to: Running
    effects: [StartRun]
  - from: Running
    on: ControlStop
    to: Retiring
    effects: [StopRuntime]
```

Schema rule:

- a schema may omit implementation detail
- it may not omit anything that changes transition legality, invariants,
  boundary-visible effects, ordering semantics, or liveness assumptions

## Initial SchemaKernel Set

The initial schema-kernel machine set for `0.5` is:

- `InputLifecycleMachine`
- `RuntimeIngressMachine`
- `RuntimeControlMachine`
- `MobLifecycleMachine`
- `OpsLifecycleMachine` once the seam in
  `docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md` is landed

## Initial PureHandKernel Set

The initial pure-hand kernel machine set for `0.5` is:

- `PeerCommsMachine`
- `ExternalToolSurfaceMachine`
- `TurnExecutionMachine`
- `FlowRunMachine`
- `MobOrchestratorMachine`

## Code Generation Tooling

`0.5` should introduce two crates plus one `xtask` entrypoint:

- `meerkat-machine-schema`
  parses and validates `schema.yaml`
- `meerkat-machine-codegen`
  renders Rust, TLA+, and contract artifacts
- `cargo xtask ...`
  runs generation and verification consistently

Canonical commands:

```bash
cargo xtask machine-codegen --machine runtime_ingress
cargo xtask machine-codegen --all
cargo xtask machine-verify --machine runtime_ingress
cargo xtask machine-verify --all
cargo xtask machine-check-drift --all
```

Command meanings:

- `machine-codegen`
  regenerate checked-in artifacts for schema-kernel machines
- `machine-verify`
  run TLC on required profiles plus Rust kernel tests for the selected machine
- `machine-check-drift`
  fail if generated artifacts differ from checked-in artifacts

## Rust Output Shape

Generated Rust should land inside the owning crate, close to the machine owner.

Illustrative shape:

```text
meerkat-runtime/src/machines/runtime_control/
  mod.rs
  generated.rs
  shell.rs
  tests.rs
```

Rules:

- `generated.rs` contains the pure transition kernel and transition table
- `shell.rs` contains impure owner logic that executes effects, persists state,
  and interacts with channels/stores
- shell code may not mutate machine state except by invoking the generated
  kernel

For pure-hand machines, the parallel shape is:

```text
<crate>/src/<machine>/
  mod.rs
  kernel.rs
  shell.rs
  tests.rs
```

## Formal Model Output Shape

For schema-kernel machines:

- `model.tla` is generated from `schema.yaml`
- `ci.cfg` is hand-authored only when the bounded profile needs machine-specific
  constants or fairness settings

For pure-hand machines:

- `model.tla` is hand-authored and checked in
- `ci.cfg` is hand-authored and checked in

## CI Gates

The required CI jobs for `0.5` are:

1. `machines-generate-check`
   - runs `cargo xtask machine-check-drift --all`
2. `machines-model-check`
   - runs TLC for every required CI profile
3. `machines-rust-kernel-tests`
   - runs Rust tests for every machine owner/kernel

No machine-affecting PR is complete unless all three pass.

## Implementation Drift Rules

### For `SchemaKernel` machines

The drift rule is strict:

- generated Rust, generated TLA+, and generated docs must be regenerated in the
  same change that edits `schema.yaml`

### For `PureHandKernel` machines

The drift rule is:

- contract/model/kernel/tests/mapping must be updated together whenever the
  machine semantics change

Semantic change means any change to:

- state variants
- input variants
- effect variants
- legal transitions
- invariants
- ordering/preemption/liveness semantics

## Rust Conformance Rule

`SchemaKernel` machines get their implementation alignment from generation.

`PureHandKernel` machines do not get to claim by-construction equivalence.
Their implementation claim is:

- one explicit pure transition kernel
- one explicit mapping note
- tests that refine the same contract/model

This is an honesty rule, not a downgrade:

- no machine may overclaim implementation proof it does not actually have

## Phase 0 Deliverables

Before broad `0.5` coding begins, Phase 0 must land:

1. checked-in machine-spec bundle under `specs/machines/`
2. `meerkat-machine-schema`
3. `meerkat-machine-codegen`
4. `cargo xtask machine-codegen`
5. `cargo xtask machine-verify`
6. first generated kernels for:
   - `InputLifecycleMachine`
   - `RuntimeIngressMachine`
   - `RuntimeControlMachine`
   - `MobLifecycleMachine`

`OpsLifecycleMachine` joins the schema-kernel set immediately after the seam in
`docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md` is implemented.
