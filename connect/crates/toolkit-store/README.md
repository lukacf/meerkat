# Toolkit storage contract

`ToolkitStore` is an injected atomic persistence boundary for feature-owned records. It is not a policy owner, credential vault, work scheduler or setup state machine. The application authenticates and authorizes every namespace before calling it. Typed domain adapters validate `RecordData.schema` and `version`, decode their exact types, and authorize each transition.

The shipped `SqliteStore` uses an on-disk SQLite database, WAL mode, FULL synchronous commits, immediate write transactions and database-issued timestamps. Independent processes using the same supported database file share its claims and fences. Independent database copies, unsupported network filesystems and cloud databases are not covered by that claim.

## Atomic facts

- Admission commits the unique admission key, initial record, events and effect intents together. An exact retry returns the current canonical record. Changed content under the same scoped key fails.
- Compare-and-swap checks the record revision and optional expected payload digest, then commits the replacement record, events and new effect intents together. Failed checks or conflicting effects leave all of them unchanged.
- Claims carry the store identity, exact record, worker, monotonically increasing fence and expiry. Store time establishes expiry. A stale or expired token cannot renew, release or commit a claim-guarded mutation.
- `recover_record_claim` observes a still-live claim after a renewal result is lost. It requires the original store, record, worker and fence, returns the current expiry, and never reacquires a released, expired or superseded claim. The host must authorize the original worker before recovery; a stale token alone is not application authority.
- Effect admission checks the current owner revision supplied by the authorized caller. The exact persisted request and idempotency key remain stable across attempts.
- A dispatched effect with an expired worker lease is retryable only when its adapter has established idempotent replay. Otherwise it requires a separately claimed reconciliation. The effect owner must prove it was not applied and that another attempt is permitted before returning it to pending.
- Effect acknowledgments and reconciliation evidence persist atomically with replayable events. Owner records remain separate: an acknowledged effect is not automatic setup readiness.

Store claims protect storage execution, not external actions that have already escaped to a provider. Effect executors must honor domain cancellation/revocation and provider fencing at their own boundary. No store method claims general exactly-once external delivery. Reconciliation must not be inferred from an exception message.

## Conditional publication

`ConditionalAdmissionStore` is an optional required-method extension. SQLite inserts the usual admission, record, events and effect intents in one immediate transaction conditional on exact revisions and digests of up to 64 participating records in the same realm and physical database. Conditions may also require the exact live record claim or InFlight Dispatch claim. Reconciliation claims cannot admit new work. A fresh store-clock sample after the final condition scan checks the deadline and every participating claim expiry before COMMIT. This orders the admission decision; it does not promise that disk synchronization or response delivery finishes before a wall-clock deadline.

Both admission APIs share the same initial request identity. `Existing` returns the current record projection, including later CAS changes. It does not prove which API created that admission or that the replay's predicates held at creation. Replay checks exact initial content but does not recheck current predicates or create another effect. A domain requiring immutable conditional publication must own all writers for its namespace, prevent alternate ordinary creation and unauthorized mutation, validate the recovered typed record, and authorize observation separately.

The conditions must name the actual policy, cancellation and custody owners participating in this database. Copies of another owner's policy cannot fence that owner's updates. Storage predicates do not interpret policy or establish requester permission, complete condition coverage, runtime admission or eventual effect authority. There is no preflight-then-write default implementation for backends without an equivalent atomic boundary.

The [conditional verification record](../../docs/reviews/conditional-publication-store-verification/20260929T232152.227840Z/manifest.json) covers real SQLite connections and processes, commit without acknowledgment, exact claim failures, publication/revocation ordering and rollback. A private test connection scripts SQLite's clock to exercise expiry specifically after the final condition scan; the public store operation and transaction remain real.

## Recovery and observation

Callers can reopen the database after process loss and recover scoped records, events and effect intents. Event cursors are per record, so they do not reveal activity counts in other namespaces. Reads are bounded and scope explicit. A timed-out caller checks the canonical operation/effect before attempting another mutation because cancellation of an await does not prove its transaction rolled back.

After an ambiguous record-lease renewal, the authorized worker recovers the token using its last known lease. Even if that lease's old expiry passed, recovery can observe its still-live renewed generation. Direct mutations continue to reject the pre-renewal token. If another executor acquired the record, the old fence cannot recover that executor's token. A concurrent renewal or claim change may invalidate an observed token immediately; mutation checks remain authoritative.

Database-issued lease timestamps use the host wall clock. Stable wall-clock operation is a deployment assumption, not a claim of monotonic time through clock rollback. Monotonically increasing fences order executor generations independently of timestamps; external effects still need their domain's cancellation and fencing contract.

`RecordData` deliberately redacts its body from `Debug`. The host still supplies sanitized evidence and stores secret handles instead of tokens, PKCE material, authentication pages or unrestricted user data. The backend cannot detect every secret in an arbitrary payload and does not claim that it can.

## Backend conformance

Run `./scripts/repo-cargo test -p toolkit-store` from the repository root. The current cases exercise the actual SQLite backend with independent connections and subprocesses, including:

- Persistence across reopen and exact admission replay.
- Changed-content rejection, namespace isolation and bounded reads.
- Conflicting-effect rollback across record, event, intent and admission writes.
- Same-table competing CAS and cross-process claims.
- Store-clock lease expiry, stale and foreign tokens.
- Lost successful renewal results, process restart and same-generation recovery without reclaiming a successor.
- Exact idempotent retry and uncertain non-idempotent dispatch recovery.
- Exclusive reconciliation and retained retry evidence.
- Stale owner revision at effect admission.
- Process loss after dispatch admission, external completion before acknowledgment, acknowledgment commit and reconciliation commit, using a separate persistent non-idempotent service simulator.

These tests do not certify BigQuery. A future backend must implement every required method with its actual consistency and transaction model. Semantic atomicity may use one conditional canonical row containing the effect/outbox intents and events; projection reconciliation can follow. A backend that cannot provide the selected contract must be rejected instead of advertising unsupported capabilities. No in-memory backend is labeled durable.

The external-effect crash tests terminate child processes after the selected commit and before a result handoff. They exercise application process loss against real local SQLite files, not physical power failure or any live service. The simulator records every attempted write, including repeated keys, so it cannot conceal duplicate calls behind an idempotent API.
