# ADR: Identity, Generation, and Hidden Runtime Epoch

**Status**: Proposed
**Date**: 2026-04-08

## Problem

The two-kernel research work introduces Mob-owned durable member identity:

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`

Meerkat runtime code already has a separate hidden continuity token:

- `RuntimeEpochId`

Without an explicit decision, these concepts can collapse into a second split
identity scheme:

- Mob may start reasoning about runtime epoch churn as if it were semantic
  member identity.
- runtime/session recovery may start smuggling hidden execution facts into the
  Mob seam.
- future work may overgeneralize "multiple processes in one realm" into
  "multiple processes may co-own the same session runtime."

That would recreate the same class of gap this architecture is trying to close.

## Decision

### 1. Mob semantic identity and Meerkat execution continuity are different facts

- `AgentRuntimeId = (AgentIdentity, Generation)` is the Mob semantic incarnation
  key.
- `RuntimeEpochId` is a hidden Meerkat execution-ordering token.

They are related through the hidden runtime binding, but they are not
interchangeable.

### 2. `RuntimeEpochId` does not cross the Mob seam

`RuntimeEpochId` is not part of the Mob-facing contract.

Mob-level lifecycle, roster, flow, observability, and proof obligations must
not depend on:

- `RuntimeEpochId`
- raw session-local identifiers
- runtime-internal continuity churn

### 3. `ResetMember` is the only semantic generation boundary

`Generation` advances only on intentional semantic replacement of the member.

That means:

- respawn does not advance generation
- hidden runtime/session recovery does not advance generation
- hidden execution epoch churn does not advance generation

If semantic lineage should be invalidated, the operation must be modeled as
`ResetMember`.

### 4. Realm multiplicity is not shared-session co-ownership

The architecture permits:

- multiple independent processes in the same realm or repo
- multiple independent sessions in that realm

The architecture does not assume:

- multiple active processes co-owning or concurrently driving the same session
  runtime

Ordinary multi-process realm concurrency therefore does not, by itself, require
durable distributed lease ownership for a session runtime.

If shared-session handoff or takeover becomes a supported mode later, it must be
introduced as an explicit Meerkat runtime ownership model, not inferred from
realm multiplicity.

### 5. Hidden Meerkat epoch rotation remains an internal choice

Whether in-place Meerkat runtime `reset()`:

- preserves `RuntimeEpochId`, or
- rotates `RuntimeEpochId`

is a Meerkat-internal decision.

Either choice is acceptable only if:

- it stays hidden from `MobMachine`
- `Generation` remains the sole semantic reset boundary
- code comments and runtime behavior agree

## Consequences

### Positive

- Mob identity continuity has one semantic clock: `Generation`
- Meerkat execution continuity has one hidden clock: `RuntimeEpochId`
- respawn, recovery, and hidden rebinding can be modeled without inventing a
  second public identity layer
- issue pressure around cross-process realm multiplicity is kept proportional to
  the actual supported mode

### Negative

- Meerkat debugging surfaces may still want to expose runtime epoch information,
  but must label it as runtime-internal continuity rather than agent identity
- hidden runtime rebinding remains more subtle than a simple "new session means
  new member" rule
- Meerkat code still needs an explicit internal decision on reset-driven epoch
  rotation

## Constraints

- Do not expose `RuntimeEpochId` in the Mob member contract.
- Do not infer `Generation` from `SessionId` or `RuntimeEpochId`.
- Do not use `RuntimeEpochId` as evidence of durable member identity.
- Do not treat many processes in one realm as proof that one session needs
  distributed co-ownership.

## Related Research

- [generation-epoch-mapping.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/generation-epoch-mapping.md)
- [abstract-member-contract.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/abstract-member-contract.md)
- [bridge-alphabet.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/bridge-alphabet.md)
- [owned-facts-ledger.md](/Users/luka/.codex/worktrees/c5c6/meerkat/docs/architecture/two-kernel-research/owned-facts-ledger.md)
