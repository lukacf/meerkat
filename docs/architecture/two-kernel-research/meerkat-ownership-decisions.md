# Meerkat Ownership Decisions

Status: frozen exact-current-state asset

This note closes `K9` from `meerkat-cutover-checklist.md`.

It resolves the remaining Meerkat ownership questions that would otherwise keep
the machine moving under us while we try to freeze it.

## Decisions

### 1. Completion waiters remain a supporting carrier, not a standalone region

Decision:

- completion-waiter state is part of the frozen `MeerkatMachine` surface
- but it is treated as a supporting carrier, not a standalone semantic region

Reason:

- the current code consistently refines completion waiters from input-owned
  queued work and lifecycle transitions
- the important exact-current split is between:
  - input-owned completion waiters
  - ops-owned `wait_all`
- treating completion waiters as an independent region would create a second
  owner story where the live code does not have one

### 2. Raw peer lineage is canonical only at the queue/authority seam

Decision:

- raw peer lineage is canonical only while it remains in the classified
  peer-ingress queue / authority snapshot
- post-admission reconstruction is sufficient for the exact current machine

Reason:

- the current live Meerkat boundary does not durably replay raw peer queue
  contents
- the live machine truth is the queue/authority snapshot, not a second
  long-lived lineage ledger

### 3. `upsert_trusted_peer(...)` is not a canonical owner seam

Decision:

- canonical runtime trust mutation is the `register_*` / `unregister_*` seam
- `upsert_trusted_peer(...)` is not part of the canonical MeerkatMachine
  ownership story

Reason:

- the freeze should follow the named current owner seam, not a broader helper
  API that the branch has already classified as non-canonical

### 4. Tool-surface machine scope stops at the published authority snapshot

Decision:

- the frozen `MeerkatMachine` includes the published
  `ExternalToolSurfaceSnapshot` fields and their lifecycle invariants
- deeper router connection/cache internals remain outside the frozen machine

Reason:

- the current machine-visible truth is the authority snapshot
- promoting transport/cache internals into the machine would create a larger
  scope than the exact current Meerkat boundary actually owns

### 5. Hidden epoch policy remains governed by the epoch ADR, not this freeze

Decision:

- `binding.epoch_id` remains a hidden binding fact
- exact-current `MeerkatMachine` freeze does **not** require a stronger reset
  rotation rule than the live code already proves
- epoch policy stays governed by:
  - `generation-epoch-mapping.md`
  - `docs/architecture/identity-generation-and-runtime-epoch.md`

Reason:

- the current code and comments still leave epoch rotation subtle enough that
  forcing a stronger rule here would move the machine under us

## Freeze decision

`K9` is considered frozen for the exact current Meerkat boundary.

These decisions deliberately keep the frozen machine aligned to current owner
truth instead of target-state cleanup wishes.
