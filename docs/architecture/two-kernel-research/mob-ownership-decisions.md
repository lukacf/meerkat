# Mob Ownership Decisions

Status: frozen target-state asset

This note resolves the remaining target-state ownership choices that would
otherwise keep `MobMachine` moving under the freeze.

Use it with:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-proof-obligations.md`
- `mob-input-effect-alphabet.md`

## Decisions

### 1. Identity-native keys are canonical in the target machine

Decision:

- `AgentIdentity`, `Generation`, `AgentRuntimeId`, and `FenceToken` are the
  canonical target-state keys.
- `MeerkatId`, `SessionId`, and `MemberRef` remain implementation-baseline
  compatibility carriers, not target-state machine truth.

Reason:

- the whole point of `MobMachine` is to own durable orchestration identity
- leaving session-scoped identifiers as canonical would keep the old split
  architecture alive inside the new machine

### 2. The work ledger is the only canonical work truth

Decision:

- direct member-directed work and flow-step work share one canonical work
  ledger
- raw turn-delivery side paths are not target-state semantic truth

Reason:

- the target machine needs one answer to “what work exists, who owns it, and
  what step/run is it bound to?”
- a second side path would recreate the same duplicate-owner bug class the new
  architecture is trying to remove

### 3. Flow outputs are not a standalone target-state region

Decision:

- terminal outcomes, task-board state, and durable history remain canonical
  output-bearing regions
- `MobMachine` does not add a separate root-output ledger as a first-class
  semantic region in this freeze

Reason:

- earlier convergence work showed that root-output count/materialization is not
  a stable current surface
- the target machine still needs terminal result lineage, but that lineage can
  remain carried by work terminal outcomes, run completion, tasks, and history
  without creating a second “output truth” region

### 4. Topology intent belongs inside MobMachine; transport does not

Decision:

- coordinator binding, topology revision, collaboration edges, and external
  spec presence are machine-owned
- raw wire/unwire transport mechanics, peer ids, and delivery mechanics remain
  outside the target machine

Reason:

- Mob owns who is connected
- Meerkat or perimeter code owns how that connection is materially realized

### 5. Public addressability policy stays outside the frozen target machine

Decision:

- `external_specs_present` is machine-owned topology truth
- broader public addressability / console / connector policy is not part of
  the frozen target `MobMachine`

Reason:

- the machine needs to know whether an external collaboration spec is part of
  topology intent
- it does not need to absorb every external product-policy decision to prove
  orchestration correctness

### 6. Checkpoint and continuity state are machine-owned; storage mechanics are not

Decision:

- checkpoint version and continuity-bound presence remain first-class machine
  state
- storage layout, CAS semantics, and persistence mechanics remain outside the
  machine

Reason:

- orchestration correctness depends on continuity state being explicit
- the machine proves the semantic state, not the storage engine

### 7. Explicit quorum contribution state is canonical target truth

Decision:

- observed quorum contribution count remains an explicit machine-owned flow
  field
- quorum completion is not treated as a free resolution step

Reason:

- earlier strengthening showed that honest target collection semantics need an
  explicit observed-count ledger
- keeping it explicit prevents silent under-modeling of quorum behavior

### 8. Dispatch family is canonical target flow truth

Decision:

- `OneToOne`, `FanIn`, and `FanOut` are explicit per-step machine truth
- dispatch family is not left implicit in helper dispatch code or inferred from
  the flow archetype name

Reason:

- exact-current runtime behavior already sustains distinct dispatch families
- target proof work needs one machine-owned answer to dispatch semantics and
  aggregate-shape class

## Freeze decision

These ownership choices are frozen for the target-state `MobMachine`.
