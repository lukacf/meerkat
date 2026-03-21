# Meerkat 0.5 Machine Formalization Strategy

Status: normative `0.5` implementation strategy note

## Purpose

This document defines how Meerkat `0.5` connects:

- authoritative machine semantics
- model checking
- Rust implementation
- catalog/code-generation where it is actually honest

The core rule is simple:

- every canonical `0.5` machine must be formally specified and model-checked
- final executable authority must be Rust-catalog-backed, not merely
  documented

This avoids two bad outcomes:

- pretending hand-written Rust is "the spec"
- accepting hand-written kernels plus mapping notes as an adequate permanent
  anti-drift story for long-lived canonical machines

## Non-Negotiable Decisions

1. The authoritative semantic contract for every canonical `0.5` machine is a
   checked-in machine contract plus a checked-in model-checked formal spec.
2. No machine may simplify away real transition semantics just to fit a favored
   toolchain.
3. By the end of `0.5`, every canonical machine must have one authoritative
   Rust owner and one explicit transition boundary.
4. By the end of `0.5`, every canonical machine must have one Rust-native
   catalog-backed executable authority path. There is no permanent
   handwritten-kernel escape hatch.
5. If the current authority/catalog language cannot express a machine
   honestly, extending that language is part of the `0.5` work.
6. If the current machine boundary is not closed enough for catalog-backed
   authority, redesigning that boundary is part of the `0.5` work.
7. "Schema language extension" and "Boundary redesign" are temporary execution
   statuses, not acceptable release states for a finished `0.5`.
8. Verification gates only count once the bundle is in-repo and enforced by
   repo CI.

## Two Axes, Not A Trust Ladder

There are two separate questions:

1. Is the machine semantically verified?
2. How is the Rust implementation bound to that semantic contract?

Only the first is the product-level trust claim.

### Semantic status

Every canonical `0.5` machine must reach:

- `SemanticallyVerified`

Meaning:

- checked-in machine contract
- checked-in formal model
- checked-in CI-bounded model profile
- model checking passes

No canonical `0.5` machine is considered done without this.

### Implementation status

Implementation status describes what still remains before Rust can be bound to
catalog-backed executable authority.

The allowed statuses are:

- `SchemaKernel`
- `SchemaExtension`
- `BoundaryRedesign`

These are engineering statuses, not trust badges.

## Implementation Modes

### `SchemaKernel`

Use this only when the machine boundary is genuinely closed and declarative.

Authoritative implementation source:

- checked-in Rust-native catalog definition under
  `meerkat-machine-schema/src/catalog/`

Generated from it:

- pure Rust transition kernel
- formal model
- contract/tables/diagrams

Required properties:

- closed state set
- closed input alphabet
- closed effect algebra
- no hidden mutable side-channel changes machine state
- transition legality does not depend on opaque implementation detail
- machine state changes only through the generated kernel

This is the strongest implementation mode because Rust/kernel drift is removed
by construction through the catalog + codegen path.

### `SchemaExtension`

Use this when the machine boundary is real and explicit, but the current
authority/catalog language and codegen stack cannot yet express it honestly
enough to become the authoritative executable source.

Required properties:

- the missing schema capability is named explicitly
- the formal model stays honest about the real semantics
- any temporary hand-written kernel is treated as migration scaffolding only,
  not as the final anti-drift mechanism
- the implementation plan names the authority/catalog/codegen work required to
  eliminate this status before `0.5` closes

### `BoundaryRedesign`

Use this only while cutting the real owner/seam.

Required properties:

- contract exists
- formal model exists
- redesign target is explicit
- no claim that the current Rust embodiment is already the final machine owner

This status is allowed during execution of the plan, but not as the final state
of a completed canonical `0.5` machine.

## Final 0.5 Machine Modes

Every canonical machine is already required to be semantically verified.
The table below defines the required **final implementation mode** for the
finished `0.5` system.

| Machine | Required final mode | Why |
| --- | --- | --- |
| `InputLifecycleMachine` | `SchemaKernel` | Closed per-input lifecycle with explicit terminal semantics |
| `RuntimeIngressMachine` | `SchemaKernel` | Admission/order/queue semantics are canonical enough to justify catalog-owned generation only if the authority/catalog stack can express recovery/coalescing/supersession honestly |
| `RuntimeControlMachine` | `SchemaKernel` | Runtime lifecycle/preemption/control semantics are explicit and closed |
| `MobLifecycleMachine` | `SchemaKernel` | Narrow top-level lifecycle boundary with small closed state set |
| `MobMemberLifecycleAnchorMachine` | `SchemaKernel` | Formal observation anchor for member lifecycle boundary routes that must stay explicit while the full owner extraction matures |
| `MobRuntimeBridgeAnchorMachine` | `SchemaKernel` | Formal observation anchor for runtime bridge run/stop routes so bridge semantics are not left as informal shell choreography |
| `MobWiringAnchorMachine` | `SchemaKernel` | Formal observation anchor for peer-admission/runtime-work wiring routes while canonical wiring ownership remains decomposed |
| `MobHelperResultAnchorMachine` | `SchemaKernel` | Formal observation anchor for helper-facing result classes so helper outcome surfaces remain explicit in the machine registry |
| `OpsLifecycleMachine` | `SchemaKernel` | Shared async-operation lifecycle substrate is closed once the seam is cut explicitly |
| `PeerCommsMachine` | `SchemaKernel` | Trust/auth/normalization/request-registry semantics are drift-sensitive and must converge to catalog-backed executable authority |
| `ExternalToolSurfaceMachine` | `SchemaKernel` | Live add/remove/reload behavior is user-visible and must converge to catalog-backed executable authority |
| `TurnExecutionMachine` | `SchemaKernel` | The core turn loop is too central to leave on prose-plus-kernel alignment forever |
| `FlowRunMachine` | `SchemaKernel` | Durable flow/run truth must converge to catalog-backed executable authority |
| `MobOrchestratorMachine` | `SchemaKernel` | Long-lived orchestration semantics must converge to catalog-backed executable authority |
| `CommsDrainLifecycleMachine` | `SchemaKernel` | Comms drain lifecycle with explicit terminal semantics and suppression flag is closed and catalog-ready |

## Starting Status Before The 0.5 Refactor Lands

This section exists to keep the execution plan honest.

| Machine | Starting status at beginning of implementation | Missing closure before `0.5` completes |
| --- | --- | --- |
| `InputLifecycleMachine` | `SchemaKernel` target | implement catalog/codegen workflow and generated kernel |
| `RuntimeIngressMachine` | `SchemaExtension` | land schema support for ordered queue transforms, multi-contributor staging, and recovery rollback/promotion semantics |
| `RuntimeControlMachine` | `SchemaKernel` target | implement catalog/codegen workflow and generated kernel |
| `MobLifecycleMachine` | `SchemaKernel` target | implement catalog/codegen workflow and generated kernel |
| `OpsLifecycleMachine` | `BoundaryRedesign` | cut seam, then land `SchemaKernel` implementation |
| `PeerCommsMachine` | `SchemaExtension` | land schema support for keyed reservation/request registries, trust snapshots, and normalization-driven typed effects |
| `ExternalToolSurfaceMachine` | `SchemaExtension` | land schema support for staged async target maps, inflight/pending epochs, and typed outward lifecycle deltas |
| `TurnExecutionMachine` | `BoundaryRedesign + SchemaExtension` | narrow `Agent`, delete host loop, and land schema support for hierarchical run-local loop transitions and rich effect payloads |
| `FlowRunMachine` | `SchemaExtension` | land schema support for dependency/ready-set evaluation, durable step/run replay semantics, and ordered graph-derived transitions |
| `MobOrchestratorMachine` | `BoundaryRedesign + SchemaExtension` | complete decomposition and land schema support for revisioned member/topology maps, pending-spawn ledgers, and supervision/coordinator decisions |

Release rule:

- no canonical machine may remain in `BoundaryRedesign` or `SchemaExtension`
  at the end of `0.5`

## Schema-Extension Inventory

This is the concrete pre-execution inventory for machines that are not already
honestly schema-ready.

| Machine | Why it is not schema-ready yet | Exact schema/codegen capability required |
| --- | --- | --- |
| `RuntimeIngressMachine` | It needs to transform ordered queues and contributor sets while preserving runtime-authoritative batch semantics across stage/apply/recover paths. | ordered collection transforms over `Seq`/maps; named helpers for coalesce/supersede/rollback; transition rows that can promote/rollback subsets of contributors without hand-written hidden logic |
| `PeerCommsMachine` | It owns reservation and request registries plus trust snapshots that feed normalization and admission effects. | keyed registry/map transforms; snapshot-capture fields in effects; richer guards over request/reservation presence and trust state; generated support for normalization outputs that are more structured than simple enums |
| `ExternalToolSurfaceMachine` | It tracks staged async add/remove/reload flows across target-local pending/inflight state and emits typed outward deltas. | map-of-target state transitions with staged/inflight epochs; typed effect payload families; support for asynchronous phase progression without collapsing machine-owned state into router code |
| `TurnExecutionMachine` | It still mixes several run-local subphases and rich outcome/effect families from LLM, tool, cancel, retry, and yield behavior. | hierarchical/submachine-style transition decomposition; richer effect unions with typed payloads; guard reuse across multi-step run-local loops; codegen that can emit readable generated kernels from larger transition tables |
| `FlowRunMachine` | It derives next-step behavior from dependency state, replay state, and ordered flow/run bookkeeping. | dependency/ready-set helpers; ordered graph-derived transition helpers; replay/recovery-friendly schema constructs for step/run status maps and deterministic selection |
| `MobOrchestratorMachine` | It coordinates member/topology/supervision decisions across revisioned orchestration state that is still partly actor-shaped today. | revisioned map/set support; typed orchestration decision effects; helper constructs for pending-spawn/member-binding/topology revision transitions; generated shell hooks that keep orchestration policy out of handwritten reducers |

Interpretation rule:

- if a machine appears in this table, the answer is **not** "leave it
  hand-written"
- the answer is "extend the authority/catalog language and/or finish the
  boundary redesign until catalog-backed authority becomes honest"

## What Counts As "Implementation Verified"

### For `SchemaKernel` machines

The implementation claim is:

- the checked-in Rust-native catalog definition is authoritative
- Rust transition code is generated from it
- formal model is generated from it
- drift between Rust kernel and formal semantics is removed by construction

### For machines currently in `SchemaExtension`

The implementation claim is intentionally incomplete:

- the machine semantics are formally specified and model-checked
- any temporary Rust kernel or shell boundary exists only to let work proceed
  while authority/catalog capability catches up
- the implementation plan must name the authority/catalog/codegen work
  required to remove
  this status

This status does **not** count as a finished anti-drift story.

## Release Gates By Mode

### Gates for every canonical machine

Must have:

- checked-in contract
- checked-in formal model
- checked-in CI config
- checked-in mapping note
- explicit Rust owner
- explicit Rust transition boundary
- explicit invariants
- CI model check wired into repo verification

### Additional gates for `SchemaKernel`

Must have:

- checked-in Rust-native catalog definition
- generated Rust kernel checked into the owning crate
- generated formal model and generated contract artifacts
- shell code forbidden from mutating machine state outside the generated kernel

### Additional gates for clearing `SchemaExtension`

Must have:

- the missing authority/catalog/codegen capability landed in-repo
- the machine's checked-in catalog definition became the authoritative source
- any temporary handwritten kernel or shadow transition owner is deleted or
  demoted to shell code only

### Disallowed final state

`BoundaryRedesign` and `SchemaExtension` are not valid final `0.5` release
states.

If a machine is still in redesign or schema-extension status, `0.5` is not
done.

## Workflow Reference

The concrete repo layout, Rust-native catalog format, generation commands, CI
commands, and
artifact rules live in:

- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`

That document is the operational companion to this strategy note.

## Non-Negotiable Honesty Rule

We do **not** simplify a machine authority definition until it fits a favored
toolchain.

If the real semantics do not fit catalog-authoritative generation honestly,
that means the authority language or the machine boundary must change before
`0.5` closes. It does **not** justify a permanent handwritten-kernel
destination for
canonical machines.

The point of the formalization program is to make Meerkat more truthful, not
more decorative.
