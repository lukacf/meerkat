# Meerkat 0.5 Machine Formalization Strategy

Status: normative `0.5` implementation strategy note

## Purpose

This document defines how Meerkat `0.5` connects:

- authoritative machine semantics
- model checking
- Rust implementation
- schema/code-generation where it is actually honest

The core rule is simple:

- every canonical `0.5` machine must be formally specified and model-checked
- implementation mode is a separate question from semantic truth

This avoids two bad outcomes:

- pretending hand-written Rust is "the spec"
- pretending every machine is ready for schema-first generation when the
  actual boundary is not closed enough

## Non-Negotiable Decisions

1. The authoritative semantic contract for every canonical `0.5` machine is a
   checked-in machine contract plus a checked-in model-checked formal spec.
2. No machine may simplify away real transition semantics just to fit a favored
   toolchain.
3. By the end of `0.5`, every canonical machine must have one authoritative
   Rust owner and one explicit transition boundary.
4. Schema-authoritative generated kernels are required where they are honest,
   and forbidden where they would create a fake simplified world.
5. "Boundary redesign" is a temporary implementation mode, not an acceptable
   release-state for a finished `0.5`.
6. Verification gates only count once the bundle is in-repo and enforced by
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

### Implementation mode

Implementation mode describes how Rust realizes the already-verified machine.

The allowed modes are:

- `SchemaKernel`
- `PureHandKernel`
- `BoundaryRedesign`

These are engineering modes, not badges of seriousness.

## Implementation Modes

### `SchemaKernel`

Use this only when the machine boundary is genuinely closed and declarative.

Authoritative implementation source:

- checked-in machine schema

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
by construction.

### `PureHandKernel`

Use this when the machine boundary is real and explicit, but the most honest
Rust implementation is still hand-written.

Authoritative semantic sources:

- machine contract
- formal model

Authoritative Rust source:

- one explicit pure `step/apply/transition` boundary

Required properties:

- no ad hoc state mutation outside the kernel
- closed state/input/effect families at the machine boundary
- transition legality/invariants tested over the pure kernel
- mapping note explains which implementation detail is abstracted out of the
  formal model

This is a valid `0.5` release mode when schema-generation would be less honest
than a hand-written kernel.

### `BoundaryRedesign`

Use this only while cutting the real owner/seam.

Required properties:

- contract exists
- formal model exists
- redesign target is explicit
- no claim that the current Rust embodiment is already the final machine owner

This mode is allowed during execution of the plan, but not as the final state
of a completed canonical `0.5` machine.

## Final 0.5 Machine Modes

Every canonical machine is already required to be semantically verified.
The table below defines the required **final implementation mode** for the
finished `0.5` system.

| Machine | Required final mode | Why |
| --- | --- | --- |
| `InputLifecycleMachine` | `SchemaKernel` | Closed per-input lifecycle with explicit terminal semantics |
| `RuntimeIngressMachine` | `SchemaKernel` | Admission/order/queue semantics are closed and machine-shaped |
| `RuntimeControlMachine` | `SchemaKernel` | Runtime lifecycle/preemption/control semantics are explicit and closed |
| `MobLifecycleMachine` | `SchemaKernel` | Narrow top-level lifecycle boundary with small closed state set |
| `OpsLifecycleMachine` | `SchemaKernel` | Shared child-agent lifecycle substrate is closed once the seam is cut explicitly |
| `PeerCommsMachine` | `PureHandKernel` | Trust/auth/normalization/request-registry semantics are real but too rich for honest schema-first generation today |
| `ExternalToolSurfaceMachine` | `PureHandKernel` | Async pending/inflight/reload semantics are explicit, but richer than a good schema-kernel boundary today |
| `TurnExecutionMachine` | `PureHandKernel` | Real machine with explicit boundaries, but execution semantics remain too rich for schema-generation without distortion |
| `FlowRunMachine` | `PureHandKernel` | Durable flow/run truth is closed enough for a pure kernel, but still not an honest schema-kernel candidate |
| `MobOrchestratorMachine` | `PureHandKernel` | Final orchestrator boundary should be explicit, but service/policy composition makes hand-authored kernel the honest mode |

## Starting Modes Before The 0.5 Refactor Lands

This section exists to keep the execution plan honest.

| Machine | Starting mode at beginning of implementation | Required move before `0.5` completes |
| --- | --- | --- |
| `InputLifecycleMachine` | `SchemaKernel` target | implement schema workflow and generated kernel |
| `RuntimeIngressMachine` | `SchemaKernel` target | implement schema workflow and generated kernel |
| `RuntimeControlMachine` | `SchemaKernel` target | implement schema workflow and generated kernel |
| `MobLifecycleMachine` | `SchemaKernel` target | implement schema workflow and generated kernel |
| `OpsLifecycleMachine` | `BoundaryRedesign` | cut seam, then land `SchemaKernel` implementation |
| `PeerCommsMachine` | `PureHandKernel` target | cut explicit owner/kernel and remove legacy bypasses |
| `ExternalToolSurfaceMachine` | `PureHandKernel` target | make runtime-facing notice/delta path the only owner boundary |
| `TurnExecutionMachine` | `BoundaryRedesign` | narrow `Agent`, delete host loop, then land pure kernel |
| `FlowRunMachine` | `PureHandKernel` target | factor explicit machine owner out of current flow/runtime code |
| `MobOrchestratorMachine` | `BoundaryRedesign` | complete decomposition, then land pure kernel |

Release rule:

- no canonical machine may remain in `BoundaryRedesign` at the end of `0.5`

## What Counts As "Implementation Verified"

### For `SchemaKernel` machines

The implementation claim is:

- the checked-in schema is authoritative
- Rust transition code is generated from it
- formal model is generated from it
- drift between Rust kernel and formal semantics is removed by construction

### For `PureHandKernel` machines

The implementation claim is narrower:

- the machine semantics are formally specified and model-checked
- Rust exposes one explicit pure transition boundary
- Rust transition tests and invariants refine the same contract
- the mapping note explains what the formal model deliberately abstracts

This is not the same as generation-by-construction, and the docs must not
pretend otherwise.

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

- checked-in schema
- generated Rust kernel checked into the owning crate
- generated formal model and generated contract artifacts
- shell code forbidden from mutating machine state outside the generated kernel

### Additional gates for `PureHandKernel`

Must have:

- hand-written pure kernel
- exhaustive transition tests when the state/input space is finite, otherwise
  property/trace tests over the pure kernel
- shell code forbidden from mutating machine state outside the kernel

### Disallowed final state

`BoundaryRedesign` is not a valid final `0.5` release state.

If a machine is still in redesign mode, `0.5` is not done.

## Workflow Reference

The concrete repo layout, schema format, generation commands, CI commands, and
artifact rules live in:

- `docs/architecture/0.5/meerkat_machine_schema_workflow_spec.md`

That document is the operational companion to this strategy note.

## Non-Negotiable Honesty Rule

We do **not** simplify a machine schema until it fits a favored toolchain.

If the real semantics do not fit schema-authoritative generation honestly, that
machine stays in `PureHandKernel` mode until the boundary changes.

The point of the formalization program is to make Meerkat more truthful, not
more decorative.
