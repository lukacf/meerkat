# Durable Tool Execution v1 — Deferred Findings

This ledger records review findings that are adjacent to, but outside, the
active delivery slice. Entries must identify the owning future slice and must
not be absorbed into current work without re-triage.

## Open

- Phase 3A/3B: runner-owned progress/artifact reporting must add typed artifact
  references without turning the store or worker shell into lifecycle
  authority.
- Storage-provider follow-up: the shared doctor inventories higher-owned
  schema domains such as `jobs`, `runtime-store`, and `workgraph`, but cannot
  certify their supported versions without inverting crate ownership. Normal
  store open and `storage migrate --apply` still refuse future versions typed;
  a future provider-aware certification registry should close the read-only
  doctor gap for all higher-owned domains together.

## Resolved in scope

- Phase 0–1: repeated runtime wake injection while `AgentApplied` lagged could
  duplicate an already accepted one-shot completion continuation. The runtime
  now injects once; boundary-level delivery owns Busy/cursor retry.
- Phase 0–1: macOS could return `EPERM` when probing a process-group identity
  after an accepted drop-time SIGKILL fence, preventing unreturned-operation
  rollback forever. The stale identity is retired only when the earlier kill
  fence is proven; ordinary probe failures remain armed.
- Phase 2A: external-wait writes could collapse back to `Running`, and
  `per_phase` use on terminal transitions could preserve the source phase.
  Explicit generated transitions now preserve waiting writes and enter each
  terminal phase correctly.
- Phase 2A: recovery-sensitive renewals and checkpoint/wait observations could
  regress lease facts or commit after the persisted lease. Renewals are now
  monotonic, lease-bounded writes carry observation time, and focused tests pin
  the behavior.
- Phase 2A: duplicate cancellation and delivery acknowledgements were rejected,
  which made crash/replay recovery brittle. Generated idempotent self-loops plus
  no-op CAS elision now preserve both state and revision.
- Phase 2A: revision saturation could reuse `u64::MAX`, and realm submission
  keys could alias malformed or cross-realm identities. Revision exhaustion
  now fails closed; realm identities and realm-scoped deduplication are
  validated.
- Phase 2B: the SQLite store now uses a versioned typed serialization envelope
  for job specifications, generated authority state, typed terminal results,
  and outbox entries. Pending outbox enumeration and exact reopen recovery are
  pinned by the SQLite store-conformance suite.
- Phase 2B: SQLite revision storage originally used SQLite's signed integer
  range for a `u64` domain, flattened shared SQLite error classification, and
  bypassed the shared TEXT/BLOB JSON codec. Revisions now use an eight-byte
  big-endian CAS token, SQLite errors stay typed, both JSON storage classes are
  accepted, and memory/SQLite share pending-outbox acknowledgement conformance.
