# MobMachine Flow Family Coverage

Status: frozen target-state flow-family coverage note

This note makes one specific part of the `MobMachine` freeze explicit: how the
target machine covers the flow families we care about, including dispatch
families, without forcing the proof model to enumerate one constant per product
variant.

Use together with:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-refinement-map.md`
- `tla/MobMachineTarget.tla`

## Coverage strategy

The target machine models flow semantics through two layers:

1. **Flow archetype**
   - sequence shape
   - dependency graph
   - branch structure
   - collection-policy shape
   - loop / retry / supervisor structure
2. **Per-step dispatch family**
   - `OneToOne`
   - `FanIn`
   - `FanOut`

The target machine treats dispatch family as explicit per-step machine state,
seeded at `StartFlowRun`. It is not inferred later from helper code or hidden
inside aggregate-output shaping.

Dispatch multiplicity is separate machine truth. The target machine does not
pretend `StartFlowRun` already knows how many work items a step will dispatch.
`DispatchStep` chooses and stores `step_dispatch_width` when the step actually
leaves ready state.

This means the proof surface can cover the important flow family combinations
without multiplying the constant space into one named flow per combination.

## Archetype coverage

### `FlowSingle`

Covers:

- single-step execution
- dispatch-family cross-product on one step
- default collection semantics
- non-structured run shape

### `FlowTwoStep`

Covers:

- sequential dependency chain
- direct dependency satisfaction
- durable multi-step ordered-step shape
- dispatch-family cross-product on both steps

### `FlowBranchFallback`

Covers:

- branch-conditioned steps
- branch labels
- `DependencyMode::Any` join semantics
- join readiness / pending transition
- branch-backed dispatch-family cross-product

### `FlowQuorum`

Covers:

- quorum collection policy
- explicit observed contribution count
- stored quorum threshold
- dispatch-family cross-product with quorum completion rules

### `FlowLoop`

Covers:

- frame structure
- loop presence
- loop iteration ledger
- loop body step semantics
- dispatch-family cross-product inside structured runs

### `FlowRetry`

Covers:

- non-zero retry budget
- failure-counter interaction
- dispatch-family cross-product under retry-capable runs

### `FlowSupervisor`

Covers:

- supervisor escalation threshold
- failure-counter interaction
- dispatch-family cross-product under escalation-capable runs

## Dispatch family coverage

Dispatch family is orthogonal to the flow archetype.

The executable target model therefore seeds an explicit per-step
`step_dispatch_mode` assignment at `StartFlowRun` from the bounded set of legal
dispatch families for the ordered steps in that run.

This gives the target machine coverage of:

- one-to-one direct dispatch
- fan-out direct dispatch
- fan-in aggregation
- dispatch-family variation inside multi-step, branch, quorum, loop, retry, and
  supervisor runs

## Aggregate-shape coverage

Payload values are outside the machine, but aggregate shape class is inside the
machine.

The target machine therefore treats aggregate shape as a derived proof surface:

- `FanIn` => array-style aggregate
- `OneToOne` + `Any` => scalar-like aggregate
- otherwise => keyed-object aggregate

This is covered in the target scaffold by:

- explicit `step_dispatch_mode`
- explicit `step_collection_policy`
- derived predicates:
  - `AggregateScalarStep`
  - `AggregateArrayStep`
  - `AggregateObjectStep`
- `AggregateShapePartitionInvariant`

## Review rule

If a proposed target flow behavior depends on:

- a hidden dispatch family,
- a flow label that silently implies dispatch mode,
- aggregate shape folklore outside machine state, or
- a family combination not representable as
  `flow archetype + per-step dispatch assignment`,

then the behavior is not honestly covered by the frozen target `MobMachine`.
