---
title: "Durable Tool Execution v1 Deferred Findings"
description: "Deferred findings discovered while delivering the durable tool execution and background jobs plan."
icon: "list-check"
---

# Durable Tool Execution v1 — Deferred Findings

This ledger records review findings that are adjacent to, but outside, the
active delivery slice. Entries must identify the owning future slice and must
not be absorbed into current work without re-triage.

## Open

- Storage-provider follow-up: the shared doctor inventories higher-owned
  schema domains such as `jobs`, `runtime-store`, and `workgraph`, but cannot
  certify their supported versions without inverting crate ownership. Normal
  store open and `storage migrate --apply` still refuse future versions typed;
  a future provider-aware certification registry should close the read-only
  doctor gap for all higher-owned domains together.
- Test-isolation follow-up: running every `meerkat-rpc::router::tests` case in
  one parallel test process can make seven unrelated live-session tests contend
  for the process-wide realtime projection budget. Phase 4's job and runtime
  projection tests pass in that run; the live tests should receive explicit
  serial/resource-budget isolation without coupling that work to durable jobs.
- SDK live-provider follow-up: the Python and TypeScript SDK suites' image
  scenarios fail when the local RPC subprocess has no image-generation
  executor configured (`no image generation executor configured for provider
  gemini`). Phase 4's deterministic SDK builds, wrapper parity, and non-live
  tests pass; provider-backed image smoke remains an environment/release-lane
  concern outside the durable-jobs protocol slice.
- Machine-poster follow-up: `DetachedJobMachine` and
  `RuntimeDeliveryMachine` are explicitly listed in the poster generator's
  shrink-only coverage-gap roster. Their canonical TLA, contracts, mappings,
  kernels, TLC verification, and authority docs are present; bespoke
  large-format SVG layouts are a separate documentation slice.

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
- Phase 2C: job terminal delivery now crosses an explicit generated
  `job_runtime_delivery` composition. `RuntimeDeliveryMachine` alone
  mints/reuses the runtime sequence and advances the ordered application
  cursor; memory and SQLite stores only CAS opaque authority and inbox rows.
- Phase 2C: crash before the runtime insert leaves the job outbox pending, and
  crash after the runtime insert but before job acknowledgement reuses the
  original runtime sequence. Focused tests also pin concurrent replay, ordered
  one-time application, SQLite reopen, and unchanged job attempt/fence facts.
- Phase 2C: recovery validation no longer expands the numeric range implied by
  a corrupted delivery high-water mark. Persisted submissions re-run
  constructor invariants, and recovered inbox rows must match the generated
  source-sequence authority before application.
- Phase 3A: applied job-terminal inbox rows publish through the runtime-owned
  delivery boundary while operation completion remains only a read projection;
  the durable shell worker and store do not own lifecycle semantics.
- Phase 3A/3B: runner progress, typed artifacts, monitor notifications, and
  checkpoints are durable job facts without turning the worker shell or store
  into lifecycle authority.
- Phase 4 gate containment: the foreground-shell cancellation canary now waits
  for parseable parent/child PID files rather than mere file creation, and the
  two process-heavy durable-monitor stress tests have load-tolerant completion
  deadlines. Their focused tests remain sub-100-ms when the machine is idle.
