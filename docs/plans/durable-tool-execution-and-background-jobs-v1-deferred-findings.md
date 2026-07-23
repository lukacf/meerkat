# Durable Tool Execution v1 — Deferred Findings

This ledger records review findings that are adjacent to, but outside, the
active delivery slice. Entries must identify the owning future slice and must
not be absorbed into current work without re-triage.

## Open

- Phase 2B: define the SQLite serialization envelope for job specifications,
  generated machine state, typed results, and outbox entries; add pending-outbox
  enumeration and prove it through the shared store-conformance suite.
- Phase 3A/3B: runner-owned progress/artifact reporting must add typed artifact
  references without turning the store or worker shell into lifecycle
  authority.

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
