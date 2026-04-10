# MobMachine Refinement Delta

Status: frozen target-state refinement handoff

This note makes the exact-current vs target-state delta explicit for
`MobMachine`.

It exists so later proof and cutover work can review real target deltas instead
of quietly assuming the rebased implementation already matches the machine we
intend to prove.

Use together with:

- `mob-machine-exact-current-freeze.md`
- `mob-machine-freeze.md`
- `mob-lowering-map.md`
- `mob-ownership-decisions.md`

## Exact-current vs target delta

### 1. Identity model

Exact-current:

- keyed primarily by `MeerkatId`
- session-oriented member binding still visible
- no canonical `AgentRuntimeId` / `FenceToken` machine surface

Target:

- canonical `AgentIdentity`
- canonical `Generation`
- canonical `AgentRuntimeId`
- canonical `FenceToken`
- hidden Meerkat-side binding beneath the Mob seam

### 2. Provisioning and authority

Exact-current:

- pending spawn lineage, kickoff barriers, and restore-failure diagnostics are
  real
- authority/fencing is not yet the canonical public machine language

Target:

- authority and fencing are first-class machine truth
- respawn and reset are machine transitions, not adapter folklore
- kickoff state and restore-failure state remain explicit machine regions

### 3. Work model

Exact-current:

- tracked flow/run state is durable
- direct work and flow-step dispatch are visible, but not yet presented as one
  canonical machine-owned work ledger

Target:

- one canonical work ledger
- direct work and flow-step work share one machine-owned submission /
  acceptance / running / terminalization story
- terminal step and terminal run cleanup of bound work is machine-owned truth

### 4. Topology and addressability

Exact-current:

- roster-centered topology and external peer-spec projections
- coordinator/topology revision already visible

Target:

- topology intent is explicitly machine-owned
- raw delivery mechanics remain outside the machine
- external peer-spec presence stays machine-owned, while broader public
  addressability policy remains perimeter or refinement work

### 5. Flow / frame / loop semantics

Exact-current:

- durable tracked-run structure already exists for many families
- active materialization remains partial in places, especially around live
  step-status timing
- dispatch families are sustained by the current runtime and tests, but not yet
  promoted into one explicit per-step machine field

Target:

- one internal `flows` region owns run, frame, loop, collection, retry, and
  supervisor semantics
- explicit per-step dispatch-mode truth
- explicit `Any` join ready state
- explicit quorum contribution state
- explicit no-dispatch-capacity failure path

### 6. Tasks / history / recovery

Exact-current:

- restore-failure diagnostics and tracked history/task surfaces exist
- some surfaces remain diagnostic joins rather than canonical machine language

Target:

- task board, restore-failure truth, checkpoint / continuity truth, and
  identity-scoped history are explicit machine regions

## Review rule

If a target-state Mob proof step depends on behavior that is only true in the
target machine and not in `mob-machine-exact-current-freeze.md`, it must be
reviewed as an explicit refinement delta, not smuggled in as “already true in
the code”.
