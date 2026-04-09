# MeerkatMachine Glossary

Status: frozen target-state terminology handoff

This note defines the target vocabulary used across the `MeerkatMachine`
freeze package.

Use it with:

- `meerkat-machine-freeze.md`
- `meerkat-machine-proof-obligations.md`
- `meerkat-machine-transition-catalog.md`

## Purpose

The target machine should not depend on interpretive phrases whose meaning has
to be inferred from implementation folklore.

This glossary fixes the meaning of the most load-bearing terms used in the
target freeze and proof handoff.

## Terms

### live binding

A machine state in which `binding` exists and lifecycle has not reached the
destroyed state.

Consequences:

- `binding.session_id`, `binding.runtime_id`, and `binding.epoch_id` are valid
- drain ownership may exist only in this state
- active or recoverable runtime work may exist only in this state

### settled state

A lifecycle state after all machine-owned obligations for that lifecycle
command have been consumed except those explicitly allowed to outlive it.

Examples:

- settled `Retired`:
  - no current-run binding remains
  - no input-owned completion waiter remains
  - already-admitted work has drained or been abandoned according to machine
    semantics
- settled `Stopped`:
  - no queue or steer work remains
  - no current-run binding remains
  - ops-owned `wait_all` may still be live if the lifecycle semantics allow it
- settled `Destroyed`:
  - binding is absent
  - input-owned queue and completion state are gone
  - drain ownership is absent

“Settled” therefore does not always mean “all machine state is empty.” It
means the lifecycle-specific obligations are resolved to the level promised by
the target semantics.

### ordinary admitted input

Any admitted input that enters the normal execution ledger and participates in
ordinary queue / steer / staging / consumption semantics, regardless of its
origin.

This includes:

- surface prompts
- steered prompts
- typed peer work after admission
- detached continuation inputs after injection

It excludes:

- raw peer backlog that has not yet been admitted
- purely observational projections

### machine-visible cancellation boundary

A transition point in the turn region where cancellation may legally change run
posture without violating turn coherence.

Examples include:

- a boundary-drain decision point
- a post-primitive cancellation observation point
- another target-defined turn transition that explicitly consumes cancellation
  intent

It does not mean “any arbitrary instant the implementation could check a flag.”

### boundary drain

The machine phase in which already-issued turn work is being reconciled and
boundary consequences are being applied before either continuation or terminal
completion occurs.

It is where the machine may:

- apply staged tool mutations
- resolve `CancelAfterBoundary`
- emit continuation or completion outcomes

### quiescent machine state

A state in which no active turn work or pending transition blocks safe
continuation injection.

At minimum, quiescence requires:

- no active run executing a primitive step
- no unresolved boundary-drain step that would race the injection
- no outstanding higher-priority lifecycle transition that forbids new
  admitted work

The TLA+ model should define quiescence explicitly as a predicate over machine
state, not as a prose intuition.

### outstanding lifecycle obligations

Machine obligations created by a lifecycle transition that have not yet been
consumed by later machine transitions.

Examples:

- queue abandonment required by `ResetRuntime`
- completion-waiter resolution required by `DestroyRuntime`
- deferred `wait_all` preservation allowed after `RetireRuntime`
- drain shutdown required by `AbortDrain`

This term exists so lifecycle commands are modeled as semantic transitions with
follow-through, not as one-shot flag flips.

### drain ownership

The machine-owned right and obligation to maintain a drain task for the
selected drain mode.

Drain ownership includes:

- whether a task is required
- whether a task is live
- whether respawn is required after exit

It is distinct from the OS process or task handle itself.

### canonical record

Authoritative machine-owned state used for reconstruction, as opposed to a
projection, cache, channel buffer, or helper snapshot.

Recovery and recycle semantics may only reconstruct from canonical records.

### peer terminalized lineage

The machine-owned record that a peer item has already reached a terminal peer
fate inside `MeerkatMachine`.

Consequences:

- a terminalized peer item cannot be replayed again
- a terminalized peer item cannot be re-received as fresh machine work
- admitted peer work is a subset of terminalized peer lineage

This lineage exists so replay uniqueness is provable from machine state rather
than from implementation folklore.

### implementation delta

A place where the exact-current baseline and the target machine intentionally
differ.

Implementation deltas are not proof failures by themselves. They are the
planned gaps that must be reviewed after the target proof exists.
