# Meerkat 0.5 Execution Plan

Status: normative `0.5` master execution plan

Companion docs:

- `docs/architecture/0.5/meerkat_0_5_architecture_outline.md`
- `docs/architecture/0.5/meerkat_sm_nomenclature.md`
- `docs/architecture/0.5/meerkat_machine_formalization_strategy.md`
- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`
- `docs/architecture/0.5/meerkat_host_mode_cutover_spec.md`
- `docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md`
- `docs/architecture/0.5/meerkat_surface_cutover_matrix.md`
- `specs/machines/`

## Purpose

This document turns the `0.5` architecture into an execution program with:

- explicit phases
- explicit dependencies
- explicit deletion targets
- explicit release gates

`0.5` is the final coherence release for the current Meerkat architecture.
There are no planned deferrals to a later `0.6`.

## Definition Of Done

Meerkat `0.5` is complete only when all are true:

1. there is one canonical ordinary runtime admission/execution path
2. the legacy direct-agent host/comms path is deleted as an execution owner
3. all canonical machines are formally specified and model-checked
4. every canonical machine has one explicit Rust owner and one explicit
   transition boundary
5. every canonical machine uses the checked-in Rust-native catalog/codegen
   workflow
6. every generated kernel has aligned owner tests and model-checking gates
7. `OpsLifecycleMachine` is the shared async-operation substrate for
   mob-backed child work and background async operations
8. `MobActor` is decomposed according to the firm ownership table
9. all public surfaces route ordinary work into the same runtime path
10. the final deletion list in this plan is complete

If any of the above is still missing, `0.5` is not done.

## Workstreams

The program is split into six workstreams.

### Workstream A. Formal Source Of Truth And Repo Gating

Owns:

- maintaining the in-repo checked-in machine bundle under `specs/machines/`
- landing the Rust-native catalog/codegen workflow for canonical machines
- wiring verification into repo CI

Primary companion docs:

- `docs/architecture/0.5/meerkat_machine_formalization_strategy.md`
- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`

### Workstream B. Runtime Core Convergence

Owns:

- canonical runtime admission/control path
- host-mode cutover
- explicit `TurnExecutionMachine` boundary
- final runtime input taxonomy

Primary companion docs:

- `docs/architecture/0.5/meerkat_host_mode_cutover_spec.md`
- `docs/architecture/0.5/meerkat_0_5_architecture_outline.md`

### Workstream C. Shared Async-Operation Lifecycle And Mob Convergence

Owns:

- `OpsLifecycleMachine` seam
- mob child-work and background-op convergence
- mob decomposition onto shared lifecycle substrate

Primary companion docs:

- `docs/architecture/0.5/meerkat_ops_lifecycle_seam_spec.md`
- `docs/architecture/0.5/meerkat_0_5_architecture_outline.md`

### Workstream D. External Tool Surface And Notice Model

Owns:

- `ExternalToolSurfaceMachine` embodiment
- typed notice model
- removal of transcript-visible notices as the primary contract

Primary companion docs:

- `docs/architecture/0.5/meerkat_0_5_architecture_outline.md`
- `docs/architecture/0.5/meerkat_sm_nomenclature.md`

### Workstream E. Surface Cutovers

Owns:

- CLI
- REST
- JSON-RPC
- MCP server
- WASM
- Rust SDK
- Python SDK
- TypeScript SDK

Primary companion doc:

- `docs/architecture/0.5/meerkat_surface_cutover_matrix.md`

### Workstream F. Final Deletions And Hardening

Owns:

- deleting bypass paths
- deleting duplicate lifecycle owners
- deleting transitional sidecars and compatibility scaffolding that would
  otherwise become permanent architecture

## Critical Path

The shortest honest critical path is:

1. land the formal source-of-truth workflow in repo
2. cut the final `OpsLifecycleMachine` seam
3. cut the final `TurnExecutionMachine` / host-mode replacement seam
4. delete the direct-agent ordinary execution path
5. converge all surfaces onto the runtime path
6. remove remaining transitional owners and duplicate state

## Phases

## Phase 0. Lock The Execution Package

Goal:

- freeze the architecture package into one gap-free set of documents and
  checked-in machine specs

Required outputs:

- this execution plan
- host-mode cutover spec
- ops lifecycle seam spec
- surface cutover matrix
- formalization strategy
- machine schema workflow
- full checked-in machine bundle

Exit gate:

- no known architecture gap remains undocumented

## Phase 1. Bring The Formal Bundle In-Repo

Goal:

- make verification enforceable rather than advisory

Required work:

1. normalize the checked-in `specs/machines/` bundle and make it the only authoritative machine-spec location
2. land `meerkat-machine-schema`
3. land `meerkat-machine-codegen`
4. land `cargo xtask machine-codegen`
5. land `cargo xtask machine-verify`
6. wire repo CI jobs:
   - generation drift
   - TLC model check
   - Rust machine-kernel tests

Exit gate:

- machine changes can no longer bypass the checked-in verification artifacts

## Phase 2. Finalize Runtime Input And Control Seams

Goal:

- make the runtime path fully capable of owning all ordinary work kinds

Required work:

1. finalize runtime input taxonomy, including explicit operation/lifecycle
   admission
2. keep control-plane authority traffic fully out-of-band
3. remove any remaining ambiguity about admission-time vs drain-time policy
4. keep canonical admission ordering inside `RuntimeIngressMachine`

Exit gate:

- every ordinary work kind has one explicit runtime admission path

## Phase 3. Land OpsLifecycleMachine As Shared Substrate

Goal:

- make shared async-operation lifecycle convergence real before full mob
  decomposition

Required work:

1. add `OpsLifecycleRegistry` contract in `meerkat-core`
2. add `RuntimeOpsLifecycleRegistry` owner in `meerkat-runtime`
3. add explicit runtime `OperationInput` admission
4. route mob-backed child lifecycle onto the shared registry
5. route background async tool operations, including shell jobs, onto the
   shared registry
6. delete the separate subagent path and use mob control-plane flows instead
7. keep transcript/tool-result completion summaries as projections only

Exit gate:

- mob-backed child work and background async operations share one authoritative
  lifecycle owner, and no separate subagent mechanism survives

## Phase 4. Land TurnExecution Narrowing And Host-Mode Cutover

Goal:

- remove the second ordinary execution path

Required work:

1. narrow `Agent` to a turn executor
2. land runtime-owned idle/wake/drain orchestration
3. replace host-loop subscriber handling with runtime completion-based bridge
4. move continuation scheduling into runtime
5. move dismiss/stop fully onto control plane
6. delete direct host-loop `self.run()` ownership for ordinary work

Exit gate:

- ordinary comms/event work reaches execution only through runtime admission

## Phase 5. Decompose MobActor On Top Of Shared Substrates

Goal:

- complete mob convergence after the lifecycle substrate exists

Required work:

1. cut the final `MobOrchestratorMachine` pure-kernel boundary
2. keep `MobLifecycleMachine` as schema-kernel lifecycle owner
3. keep `FlowRunMachine` as explicit pure-kernel run/step owner
4. keep topology, task board, spawn policy, and supervision in the firm
   non-machine owners already specified by the architecture docs

Exit gate:

- `MobActor` is no longer one monolithic owner of child lifecycle, flow
  execution, topology, and supervision

## Phase 6. Surface Cutovers

Goal:

- make every public surface an early adapter onto the canonical runtime path

Required work:

- execute the surface-specific steps in
  `docs/architecture/0.5/meerkat_surface_cutover_matrix.md`

Ordering guidance:

1. Rust SDK / CLI / REST / JSON-RPC
2. MCP server
3. WASM bridge
4. Python / TypeScript wrappers

Exit gate:

- every surface passes its release gate from the surface matrix

## Phase 7. Final Deletions

Goal:

- remove the transitional structures that would otherwise preserve ambiguity

Deletion set includes:

- legacy direct host-mode execution path in `comms_impl.rs`
- host-synthesized continuation path
- event-injector execution bypasses on CLI/REST/RPC
- direct MCP server run/resume execution path
- authoritative direct-session owner in WASM
- pre-init global JS tool owner in WASM
- separate authoritative mob-child/background-op lifecycle stores
- `SubAgentManager` and `SubagentResult`
- authoritative transcript-visible runtime notices
- silent duplicate mutable state without a written owner/justification

Exit gate:

- no deleted mechanism remains part of ordinary execution ownership

## Phase 8. Verification Closure

Goal:

- close the release with both semantics and implementation gates green

Required work:

1. all canonical machine models pass required TLC profiles
2. catalog-owned generated kernels generate cleanly with no drift
3. generated kernels pass their explicit Rust test suites
4. release checklist confirms no machine remains in `BoundaryRedesign`
5. release checklist confirms every surface release gate is green

Exit gate:

- all `0.5` release gates satisfied

## Cross-Phase Rules

1. No new feature work may introduce a second ordinary work-ingress/drain path.
2. Compatibility APIs are allowed only as early adapters onto the canonical
   path.
3. Any duplicated mutable state requires written justification and a named
   owner.
4. A machine semantic change is not complete until the contract/model/tests are
   updated together.
5. "We will harden that later" is not an allowed closure condition for `0.5`.

## Release Checklist

`0.5` ships only when:

- `Phase 1` through `Phase 8` all pass their exit gates
- no canonical machine remains in `BoundaryRedesign`
- no public surface still owns a second ordinary execution path
- no authoritative direct host-mode path remains
- no authoritative duplicate child lifecycle owner remains

That is the complete plan.
