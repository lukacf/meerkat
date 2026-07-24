---
title: "Durable Tool Execution and Background Jobs v1"
description: "Architecture and delivery plan for durable detached tools, generated job lifecycle authority, recovery, delivery, and host protocols."
icon: "clock-rotate-left"
---

# Proposal: Durable Tool Execution and Background Jobs v1

Status: Proposed
Baseline: Meerkat `dda5f2b2e`; MobKit PR [#301](https://github.com/lukacf/meerkat-mobkit/pull/301) at `c2920a8a`
Scope: Meerkat platform, MobKit gateway/SDKs, and downstream host-callback consumers such as HomeCore

## Decision

Meerkat should support three first-class tool execution modes:

- Fast: bounded synchronous completion.
- Streaming: synchronous completion with contractual progress and inactivity detection.
- Detached: a short durable submission followed by execution outside the agent turn.

Detached means durable. Today’s `shell(background: true)` is asynchronous but volatile, so it does not satisfy the new definition.

The platform will introduce a feature-owned durable job authority. It will reuse the existing runtime completion-feed wake path, but it will not use runtime operations, WorkGraph, Schedule, or MobKit callbacks as the canonical execution store.

For HomeCore, the key outcome is:

> A security scan is accepted durably within the MobKit callback deadline, runs outside the callback and agent turn, appears as healthy detached activity, re-resolves credentials at execution time, and becomes `WorkerLost`/`NeedsAttention` rather than being blindly replayed after an unrecoverable restart.

## 1. Current state

Meerkat already has useful pieces:

- Detached operations do not become turn barriers.
- Background shell returns a receipt promptly.
- Terminal background operations enter a monotonic completion feed.
- Idle keep-alive sessions are re-woken through the runtime-owned continuation path.
- WorkGraph provides durable commitments, dependencies, claims, and evidence.
- Schedule provides durable occurrence planning and redelivery.
- The rebased storage arc provides shared SQLite mechanics, realm path authority, schema ledgers, maintenance fencing, provider seams, and fail-closed durability.

The missing piece is durable ownership of unfinished execution:

- [`JobManager`](https://github.com/lukacf/meerkat/blob/dda5f2b2e2da4165b85f2591611e5d46d2b5d380/meerkat-tools/src/builtin/shell/job_manager.rs#L148) is in-memory.
- Runtime recovery explicitly discards non-terminal operations in [`from_recovered`](https://github.com/lukacf/meerkat/blob/dda5f2b2e2da4165b85f2591611e5d46d2b5d380/meerkat-runtime/src/ops_lifecycle.rs#L2178).
- [`WorkItem`](https://github.com/lukacf/meerkat/blob/dda5f2b2e2da4165b85f2591611e5d46d2b5d380/meerkat-workgraph/src/types.rs#L660) has no executable specification, attempts, fencing, checkpoint, result, or delivery state.
- Schedule runnables are awaited in-process and do not own restart-safe execution.
- MobKit callback tools currently synchronously await `callback/call_tool`.

There is also an immediate correctness defect: a recovered terminal completion may advise the agent to query `shell_job_status`, even though the recovered process has no matching `JobManager` record.

## 2. Goals

The design must provide:

1. Durable-before-receipt job submission.
2. No provider turn or gateway callback held for the duration of detached work.
3. Restart behavior defined per runner: adopt, resume, replay safely, or declare loss.
4. Fenced worker attempts and renewable leases.
5. Durable, bounded progress, artifacts, results, and cancellation.
6. Replay-safe completion delivery through the existing runtime wake path.
7. Stable submission identity across crash retries and completion-triggered agent retries.
8. Execution-time credential resolution from the current authorized profile.
9. Operator-visible distinction between healthy detached waiting and a wedged provider/callback.
10. Optional WorkGraph and Schedule composition without making either executable.
11. First-class participation in the new Meerkat/MobKit storage-provider architecture.
12. Durable agent-authored monitors that can emit any number of
    conversation notifications without terminalizing the underlying job.

Non-goals:

- Keeping an arbitrary Unix process alive across machine loss.
- Exactly-once external side effects without target-system cooperation.
- Treating streaming as a substitute for detached work.
- Making every detached job a WorkGraph item.
- Preserving an agent/provider turn for hours.

## 3. The actual timeout hierarchy

There is no single platform-wide tool timeout. The effective deadline is the narrowest applicable layer.
| Layer | Current behavior | Contract |
|---|---:|---|
| Built-in foreground shell | 30 seconds by default | Tool-internal deadline |
| SDK/builder `ToolDispatcher` | 30 seconds by default from `ToolTimeoutPolicy`; contributed as `ToolDeadlineOwner::Dispatcher` and enforced around router dispatch | Dispatcher-layer deadline when this wrapper is composed |
| Meerkat normal tool batch | 600 seconds by default in [`ToolsConfig`](https://github.com/lukacf/meerkat/blob/dda5f2b2e2da4165b85f2591611e5d46d2b5d380/meerkat-core/src/config.rs#L2185) | Agent-loop outer deadline |
| MobKit provider operation | 120 seconds public contract | Host callback’s supported work window |
| Python SDK callback wait | 125 seconds in [`_transport.py`](https://github.com/lukacf/meerkat-mobkit/blob/c2920a8afb4fd2c598bb657e8234d18bcb911a50/sdk/python/meerkat_mobkit/_transport.py#L18) | Cancels the host coroutine |
| TypeScript SDK callback wait | 125 seconds in [`transport.ts`](https://github.com/lukacf/meerkat-mobkit/blob/c2920a8afb4fd2c598bb657e8234d18bcb911a50/sdk/typescript/src/transport.ts#L74) | Aborts the host callback |
| Rust gateway callback wire | 130 seconds | Final gateway wire deadline |
| LLM stream inactivity | 300 seconds by default | Provider-stream watchdog, unrelated to tool callbacks |
| Schedule occurrence lease | 60 seconds | Scheduling/redelivery lease, not a long-job execution lease |

Therefore:

- A direct Meerkat tool may have up to its resolved core/tool deadline. SDK/builder paths that compose `ToolDispatcher` have an actual 30-second dispatcher contributor by default, so raising the outer 600-second `ToolsConfig` deadline does not raise that narrower layer unless the host overrides the dispatcher timeout explicitly. The built-in shell timeout and `ToolDispatcher` timeout are distinct enforced layers, even though both defaults currently come from the same `ToolTimeoutPolicy`.
- A MobKit host-callback Fast tool must complete within the 120-second public contract. The 125/130-second layers are cancellation and transport margins, not extra user work time.
- Raising `ToolsConfig` cannot make a MobKit callback run longer.
- Detached submission must itself fit comfortably inside Fast. Proposed default submission deadline: 10 seconds, bounded above by the applicable Fast deadline.

Every resolved execution plan should expose its deadline chain for diagnostics:

```text
effective deadline: 120s
contributors:
  core tool dispatch: 600s
  mobkit public callback: 120s
  sdk callback cancellation: 125s
  gateway wire: 130s
winner: mobkit public callback
```

## 4. Tool execution contract

### 4.1 Internal declaration

Add a core-owned internal `ToolExecutionContract` to `ToolCatalogEntry`, not initially to provider-facing `ToolDef`.

Conceptually:

```rust
struct ToolExecutionContract {
    supported_modes: Set<ToolExecutionMode>,
    default_mode: ToolExecutionMode,
    streaming_policy: Option<StreamingPolicy>,
    detached_policy: Option<DetachedPolicy>,
}
```

This avoids changing provider-facing serialized tool definitions and prompt-cache prefixes.

All dispatcher wrappers—including `ExecutionPolicyGatedDispatcher`, filtered dispatchers, composite dispatchers, MCP adapters, and MobKit callback dispatchers—must forward the complete execution-contract surface.

### 4.2 Pre-dispatch resolution

Static metadata alone is insufficient. The runtime needs:

```rust
resolve_execution_plan(call, context) -> ResolvedToolExecutionPlan
```

The resolved plan contains:

- Exact execution mode.
- Applicable deadline chain.
- Runner identity and version.
- Restart classification.
- Idempotency scope.
- Credential-context references.
- Output and progress policies.

Hybrid tools can inspect typed arguments during migration. The dogmatic target is one default execution class per tool identity—for example, foreground shell and detached shell submission as distinct tools.

`WaitPolicy::Detached` remains a post-dispatch barrier classification. It is not the declaration mechanism.

### 4.3 MobKit registration

MobKit’s callback-built tool specification currently carries only name, description, and input schema. Extend its private host/gateway contract with execution metadata:

```json
{
  "name": "security_scan",
  "description": "Scan the network security posture",
  "input_schema": {"type": "object"},
  "execution": {
    "default_mode": "detached",
    "supported_modes": ["detached"],
    "runner": "homecore.security_scan.v1",
    "restart": "non_resumable",
    "idempotency": "interaction_and_arguments",
    "credential_scopes": ["network"]
  }
}
```

Fast callback tools keep using `callback/call_tool` and the 120/125/130-second hierarchy.

Detached callback tools do not execute their long work inside `callback/call_tool`.

## 5. Durable job authority

Introduce a feature-owned `meerkat-jobs` crate and a catalog-generated `DetachedJobMachine`.

### 5.1 Canonical job state

A durable job owns:

- `JobId`
- Realm and origin session/member
- Interaction lineage and submission key
- Tool and runner identity/version
- Canonical argument/specification hash
- Credential-context references
- Restart classification
- Lifecycle phase
- Attempt counter
- Current worker and fenced lease
- Heartbeat and checkpoint references
- Progress cursor and artifact references
- Cancellation state
- Typed terminal result
- Completion-delivery outbox state

Recommended phases:

```text
Queued
Claimed
Running
WaitingExternal
RetryScheduled
Succeeded
Failed
Cancelled
WorkerLost
NeedsAttention
```

`WorkerLost` and `NeedsAttention` must be explicit typed outcomes, not error strings.

### 5.2 Machine inputs

The generated authority should own transitions such as:

```text
Submit
ClaimAttempt
RenewLease
ReportProgress
RecordCheckpoint
CompleteAttempt
FailAttempt
RequestCancel
AcknowledgeCancel
LeaseExpired
ClassifyWorkerLoss
ScheduleRetry
MarkDeliveryApplied
```

The worker shell realizes effects; it does not decide retry, loss, or terminality.

### 5.3 Attempts and fencing

Every attempt receives:

- Unique attempt ID.
- Monotonically increasing fence token.
- Finite renewable lease.
- Worker identity.
- Heartbeat deadline.

All progress, checkpoint, result, failure, and cancellation acknowledgements carry the attempt and fence. Once a later attempt is claimed, an older worker cannot commit anything.

Fencing is write authority, not recovery identity. Reopening or reconstructing a
worker/runtime must not advance the persisted fence by itself, and recovery
must not require an in-memory pre-restart token that can no longer be produced.
Recovery first rehydrates the latest committed attempt, fence, lease,
checkpoint, and runner handle. It may continue that attempt when the restart
class and persisted lease/adoption facts permit, or ask the machine to mint a
new attempt and fence through an explicit transition. Only that explicit
transition invalidates the older writer. A token mismatch is therefore evidence
of a genuinely newer committed attempt, not merely of process reconstruction.

Retry authority belongs only to `DetachedJobMachine`. Schedule, workers, callbacks, and WorkGraph must not introduce independent retry loops.

## 6. Persistence and storage-provider integration

The storage-unification work changes the correct integration point.

Meerkat’s [`RealmStoreSet`](https://github.com/lukacf/meerkat/blob/dda5f2b2e2da4165b85f2591611e5d46d2b5d380/meerkat/src/storage_provider.rs#L51) should gain:

```rust
job_store: Arc<dyn DetachedJobStore>
```

And `jobs` should become a required fail-closed durability domain alongside sessions, runtime, schedule, WorkGraph, blobs, and artifacts.

The built-in disk implementation should use:

- A canonical `jobs.sqlite3` path owned by `StorageLayout`/`RealmPaths`.
- `meerkat-sqlite`’s Primary profile.
- Schema-domain migrations.
- Per-operation maintenance guards.
- Doctor/migrate/prune participation.
- Store-conformance tests.

The SQLite `OperationGuard` is only an offline-maintenance fence. It must not be confused with job-attempt fencing.

### MobKit PR #301 composition

PR #301 introduces a [`MobKitStorageProvider`](https://github.com/lukacf/meerkat-mobkit/blob/c2920a8afb4fd2c598bb657e8234d18bcb911a50/meerkat-mobkit/src/storage_provider.rs#L137-L178) that wraps the Meerkat `RealmStorageProvider` and supplies one composite backend.

Therefore:

- The job store belongs in the Meerkat-level provider set.
- A MobKit composite provider supplies it through `meerkat_provider()`.
- Do not add a second MobKit-owned semantic job store.
- PR #301’s storage census should include the inherited Meerkat `jobs` durability slot.
- MobKit’s doctor/migration layer should inspect the canonical job database through the composed layout.
- External providers must supply the job store or fail startup/Detached admission typed.

A declared-ephemeral realm may run Fast and Streaming tools. It should not advertise semantic Detached. If volatile background work remains desirable, it should have a different name and explicitly weaker contract.

## 7. Worker and restart semantics

Each runner declares one restart class:

### Adoptable

The actual work is owned by an external durable system with a stable handle. After restart, the worker reconnects and resumes observation.

### Checkpoint-resumable

The runner commits checkpoints and resumes a later attempt from the last accepted checkpoint.

### Replayable

The work is safe to run again under its stable idempotency key.

### Non-resumable

The work cannot be safely continued or replayed automatically. Worker loss produces `WorkerLost`/`NeedsAttention`.

HomeCore security scans should initially be Non-resumable:

- A rebooted scan is not silently replayed.
- The operator or agent can explicitly start a new scan.
- A future scan implementation may become Adoptable or Checkpoint-resumable without changing platform semantics.

Durable job truth surviving restart does not imply that the underlying work
survives. State the guarantee as a restart matrix:

| Restart topology | Required outcome |
|---|---|
| Runtime/gateway restarts while an externally owned worker and stable handle remain alive | An Adoptable runner reconciles and resumes observation without minting a fence merely because the gateway reopened. |
| Runtime/gateway and an in-process or co-deployed Non-resumable worker restart together | The attempt becomes `WorkerLost`/`NeedsAttention`; it is not silently replayed or described as restart-surviving. |
| Runtime/gateway and worker restart, but an accepted checkpoint exists | A Checkpoint-resumable runner may claim a new fenced attempt and resume from the committed checkpoint. |
| Runtime/gateway and worker restart, and replay is safe under the stable idempotency key | A Replayable runner may claim a new fenced attempt and execute again. |

For HomeCore v1, a detached scan running inside the co-deployed Python host
survives a gateway-only restart only when the host process and reconnectable
attempt handle remain alive. A routine upgrade that stops both host and gateway
loses a plain in-process Non-resumable scan. Surviving that topology requires a
future Adoptable implementation backed by an externally supervised process
with a stable handle, or an honest Checkpoint-resumable/Replayable runner; Phase
4 does not provide that property automatically.

Exactly-once side effects are only possible when the runner and external target honor an idempotency protocol.

## 8. MobKit detached host callbacks

Detached host tools need a separate asynchronous callback protocol.

### 8.1 Submission

When the agent selects a Detached callback tool:

1. The gateway resolves the execution plan.
2. It submits the job to `DetachedJobMachine`.
3. The job specification commits durably.
4. The gateway returns a `JobReceipt` to the agent.
5. No long-running Python/TypeScript callback is open.

Example receipt:

```json
{
  "job_id": "job_01...",
  "status": "queued",
  "deduplicated": false,
  "restart_class": "non_resumable",
  "awaitable": true
}
```

### 8.2 Host execution protocol

The gateway’s job worker asks the host to accept an attempt through a short callback, conceptually:

```text
callback/job/start
callback/job/reconcile
callback/job/cancel
```

These callbacks remain subject to the normal 120/125/130-second control-plane deadline and must return promptly.

The host then reports asynchronously through ordinary gateway RPC:

```text
mobkit/jobs/heartbeat
mobkit/jobs/progress
mobkit/jobs/checkpoint
mobkit/jobs/complete
mobkit/jobs/fail
mobkit/jobs/cancel_ack
```

Every mutation includes `job_id`, `attempt_id`, and the current fence.

On reconnection, the gateway and host reconcile active attempts. The machine, not either side’s in-memory task map, decides whether an attempt is adoptable, resumable, replayable, or lost.

## 9. Credential handling

A durable job must never contain resolved secrets.

The job stores only typed references such as:

- Owning realm/profile.
- Required credential scopes.
- Auth binding names or profile-local binding references.
- Policy/config generation for audit.
- Non-secret runner configuration.

At every attempt start:

1. Re-evaluate current authorization and tool policy.
2. Resolve credentials from the current owning profile.
3. Acquire the necessary credential lease.
4. Execute using ephemeral secret material.
5. Drop the material when the attempt ends.

If the binding has been removed, rotated incompatibly, or is no longer authorized, the job becomes `BlockedCredentials` or `NeedsAttention`. It must not fall back to ambient environment variables or stale serialized material.

For HomeCore, a network scan therefore re-resolves UniFi credentials from the network profile when the attempt actually starts.

## 10. Submission idempotency

`session_id + provider tool-call ID` is insufficient: a completion-triggered agent continuation may produce a new tool-call ID.

Introduce a runtime-minted durable `ExecutionIntentId` and an interaction lineage that survives continuation wakeups.

The canonical submission key should include:

```text
realm
origin session
interaction lineage
tool identity and version
normalized argument hash
optional host-defined semantic key
```

A uniqueness constraint on the store maps one submission key to one `JobId`.

Required behavior:

- Crash after durable commit but before receipt: retry returns the original receipt.
- Replayed dispatch of the same tool call: returns the original receipt.
- Re-woken agent re-deriving the same scan within the same interaction lineage: returns the existing receipt.
- A genuinely new user request has a new interaction lineage and may create a new job.
- Tools needing stronger domain idempotency can provide a semantic key such as `security-scan:<target>:<policy-revision>:<intent>`.

No fuzzy cross-turn argument matching should be used. Intentional duplicate work must remain possible through a new lineage or explicit nonce.

The completion continuation should carry typed `JobId`, delivery identity, and interaction lineage rather than only a generic reason string. The full result remains in durable job/feed storage.

## 11. Completion and wake delivery

Job terminalization and runtime delivery cannot assume a cross-database atomic transaction.

Use a job-owned outbox:

1. Commit the terminal result and an outbox entry atomically in the job store.
2. A projector submits the entry through a runtime-owned durable delivery
   inbox; it must not write the in-memory completion feed directly.
3. Runtime authority commits the stable delivery idempotency key before
   publishing the completion-feed projection.
4. A duplicate projector submission therefore reuses the original runtime
   delivery/feed sequence rather than producing a second application.
5. Only after the runtime durable insert succeeds may the projector record
   acknowledgement in the job store.
6. Failed or unacknowledged delivery remains retryable.
7. Runtime cursor logic prevents duplicate agent application.

Every terminal phase uses this path, not only `Succeeded`. `Failed`,
`Cancelled`, `WorkerLost`, and `NeedsAttention` commit their typed terminal
outbox entry atomically with terminal state. A configured agent subscription
therefore receives the failure/loss outcome through ordinary durable delivery
and its canonical comms `HandlingMode`; health/status is an additional operator
projection, not a substitute for telling the responsible agent that its work
was lost. Reopen or projector retry must neither suppress that terminal event
nor apply it twice.

The runtime inbox and its catalog-generated job/runtime delivery composition
ship with the Phase 2 foundation. The current operation completion feed is a
read-only, operation-specific projection and is not by itself a durable ingress
contract for jobs.

The existing runtime continues to own:

- Completion-feed consumption.
- Quiescence checks.
- Continuation injection.
- Agent wakeup.
- Typed transcript notice creation.

The job machine owns job truth. The completion feed is a delivery projection.

### 11.1 Non-terminal monitor notifications

A durable job may produce notification outbox entries before it terminalizes.
Applying one of these entries wakes/notifies a subscribed session, but does not
change the job phase, stop its worker, close its lease, or imply success. A
monitor may therefore notify zero, one, or many times and continue running
until it exits, explicitly completes, is cancelled, or reaches a typed failure.

Each notification owns:

- Stable `JobId`, notification ID, attempt ID, and fence.
- A job-monotonic notification sequence.
- A runner-provided idempotency key when the observed condition has a stable
  identity, such as `github-release:meerkat-mobkit:v0.8.3`.
- Safe title/body data and optional artifact references.
- The interaction lineage and durable notification subscriptions to which it
  may be delivered.
- Delivery, acknowledgement, retry, and suppression state.

Notification outbox delivery uses the same runtime-owned durable inbox as
terminal completion. Duplicate projector submissions or crash/reopen replay
must produce one agent-visible application for a stable notification identity.
A terminal completion and a non-terminal notification are distinct typed
delivery kinds.

Notification subscription is separate from `await_job`. A conversation can
subscribe to monitor notifications without parking an agent turn, and a
monitor can continue after delivering a notification. Unsubscribing a
conversation does not cancel the job unless explicitly requested.

Every subscription declares a delivery kind:

- `record`: retain the observation and advance its durable checkpoint without
  delivering ordinary work to a conversation.
- `notification`: apply a durable user-visible notification without opening a
  provider/agent turn. The notification remains available to later
  conversation context; it merely does not request immediate inference.
- `event`: inject ordinary content-bearing work through the existing
  runtime/comms ingress using the canonical `HandlingMode`.

Monitor `event` delivery reuses the comms nomenclature and semantics exactly:

- `HandlingMode::Steer` requests injection at the earliest admissible
  cooperative boundary while remaining ordinary work, not an out-of-band
  control-plane command.
- `HandlingMode::Queue` leaves the current run untouched and delivers at the
  outer-loop/next-turn boundary.

`Steer` is the default handling mode for monitor events because a condition
selected for agent judgment is normally intended to affect current work.
Callers choose `Queue` explicitly when the event is important but must not
perturb the active run. The durable inbox retains either mode until its
runtime-owned admissible boundary; monitor code must not implement a separate
steer/queue fallback or scheduling path.

This preserves the design boundary between predicate detection and judgment.
Release/version changes, HTTP conditions, file changes, and numeric thresholds
can normally remain turn-free. Camera triage, ambiguous household events, and
other judgment-bearing decisions use the monitor only as the cheap predicate
and then select `event` delivery with the appropriate `HandlingMode`; the
monitor primitive must not replace the reasoning turn.

Monitor events use the existing comms content and rendering vocabulary,
including `ContentInput`, `RenderMetadata`, and `RenderClass::ExternalEvent`,
rather than defining parallel message, urgency, or presentation fields.

### 11.2 Agent-authored script monitors

Expose a first-class monitor runner/tool that lets an agent supply a custom
script or command plus an explicit output protocol. It returns a durable
`JobId` immediately and never keeps a provider turn or host callback open.

Two stdout protocols are required:

1. `framed_jsonl` (default): a line such as
   `{"type":"notify","key":"release:v0.8.3","message":"..."}` requests a
   durable notification. Ordinary stdout remains bounded diagnostic/progress
   output. Frames also support `checkpoint`, `progress`, and explicit
   `complete`; a `notify` frame is non-terminal unless it separately requests
   completion.
2. `lines`: every complete stdout line requests a notification. This provides
   the direct shell-monitor power of “print a line to ping the conversation,”
   but is opt-in because accidental noisy output can otherwise flood delivery.

The runner must drain stdout and stderr independently so a quiet notification
stream cannot deadlock behind diagnostic output. It applies bounded line/frame
sizes, bounded retained logs, rate limits, coalescing/backpressure policy, and
typed malformed-frame/flood outcomes. Suppression must remain observable; it
must not silently discard notifications or silently kill the monitor.

Script exit is terminal according to the declared exit policy. Emitting a
notification is not. A script may loop indefinitely, emit when a condition
changes, update its checkpoint, and continue monitoring subsequent changes.
Cancellation remains an explicit durable request and acknowledgement.

All monitors declare restart behavior from their actual dependencies and
durable source semantics, not merely from whether their implementation is
called “native” or “domain.” For example, an HTTP release watcher with a stable
ETag/version can be Replayable; a local file watcher on persistent storage may
be Checkpoint-resumable; an external event system with a durable cursor may be
Adoptable; and a HomeCore callback that requires a currently running Python
host remains explicitly host-dependent.

Agent-authored monitor scripts declare their restart class honestly:

- A Replayable monitor receives its stable submission key and latest durable
  checkpoint on every attempt and must tolerate polling the same condition
  again.
- A Checkpoint-resumable monitor resumes from the latest accepted checkpoint.
- An Adoptable monitor reconnects to an external durable watcher.
- A script that keeps essential deduplication state only in process memory is
  Non-resumable and becomes `WorkerLost`/`NeedsAttention` after worker loss.

The monitor protocol provides a checkpoint channel so generated scripts do not
need to rely only on shell variables such as `rel_last`. Recovery rehydrates
the latest committed checkpoint and current attempt authority; merely
reopening the runtime must not advance the fence or make a valid monitor
impossible to resume.

Scripts execute under an explicit sandbox/capability policy. Credentials are
resolved from authorized profile references on each attempt and are never
embedded in the job specification, checkpoint, event frames, logs, or
notification payloads.

The unrestricted script runner is a high-trust capability distinct from
broadly grantable first-class predicates. Durable background execution,
arbitrary shell, network/file access, and notification delivery form a powerful
combination and must be gated fail-closed through the ordinary execution
profile/capability policy.

### 11.3 Trigger authority and steady-state cost

First-class watches do not create an independent timer subsystem or one
sleeping runtime task/thread per watch. Time-based evaluation composes the
existing Schedule authority:

```text
Schedule occurrence
  -> predicate evaluation
  -> durable comparison/checkpoint
  -> notification outbox
```

Where a source supports push, use a durable event adapter and cursor instead:

```text
Push/event source
  -> durable source cursor
  -> predicate/comparison
  -> notification outbox
```

Prefer push/event sources over polling. Prefer conditional reads such as HTTP
ETag/If-Modified-Since and operating-system file notifications where their
durability contract is honest. Poll-based monitors require a minimum/coarse
interval policy, bounded concurrency, jitter, backoff, per-source budgets, and
visible steady-state cost. Monitor age alone is not unhealthy, but excessive
poll load, repeated source failures, or a growing delivery backlog are typed
health conditions.

An indefinitely looping script with its own sleep remains a high-trust escape
hatch for custom behavior, not the default first-class watch implementation.
It is still job-owned, metered, cancellable, and subject to runner/resource
health policy.

## 12. Parking and observability

Detached launch and durable waiting are separate operations.

### 12.1 Default behavior

A detached tool returns a receipt. The agent may continue working or end the turn. The job runs independently and completion notification remains enabled.

### 12.2 Explicit awaiting

An explicit `await_job(job_id)` records that the session/member is intentionally waiting for the job. It does not keep a provider call open.

The per-session wait binding belongs to `MeerkatMachine`; job completion belongs to `DetachedJobMachine`. A generated composition defines the handoff.

### 12.3 Member status

Do not add `AwaitingDetached` to `MobMemberStatus`; that enum describes member lifecycle.

Add a separate execution-activity projection:

```json
{
  "status": "active",
  "health": "healthy",
  "activity": {
    "kind": "awaiting_detached",
    "since": "2026-07-23T...",
    "job_ids": ["job_01..."]
  },
  "detached_jobs": {
    "active": 1,
    "needs_attention": 0,
    "oldest_active_ms": 123456
  }
}
```

Possible activity kinds:

```text
idle
calling_provider
executing_fast_tool
streaming_tool
awaiting_detached
draining
```

If a job is active but the member did not explicitly await it, the member remains `idle` or `running`; `detached_jobs.active` still exposes the job.

This distinguishes:

- Healthy parked work: `awaiting_detached`, valid job, current worker lease or queued state.
- Provider wedge: `calling_provider` beyond its watchdog/deadline.
- Callback wedge: Fast callback beyond 120/125/130 seconds.
- Worker fault: expired job lease, missing runner, or blocked delivery.

### 12.4 Health surfaces

Awaiting a detached job is healthy and must not degrade liveness/readiness.

MobKit should preserve probe compatibility:

- `GET /healthz` with ordinary text negotiation continues returning `200 ok`.
- `GET /healthz` with `Accept: application/json` returns structured job/activity health.
- `mobkit/status`, `mobkit/capabilities`, runtime health, and console projections expose the full model.

Example:

```json
{
  "status": "ok",
  "detached_jobs": {
    "queued": 1,
    "running": 2,
    "awaiting_members": 1,
    "stale_leases": 0,
    "needs_attention": 0,
    "delivery_backlog": 0
  }
}
```

Health becomes degraded only for conditions such as:

- Job store unavailable.
- No eligible worker for a due job.
- Stale lease or heartbeat.
- Persistent completion-delivery backlog.
- Cancellation that cannot be acknowledged.
- Credentials blocked beyond policy threshold.

Long duration by itself is not unhealthy.

## 13. WorkGraph and Schedule composition

### WorkGraph

WorkGraph remains the commitment authority:

- A job may link to a `WorkItem`.
- Job progress can project references.
- Terminal artifacts can become evidence.
- WorkGraph closure follows its completion policy.

Executable payloads, attempt state, fencing, and results remain job-owned.

Detached execution must work when WorkGraph is disabled.

### Schedule

Schedule owns when a submission becomes due.

A `HostRunnable` used for long work should instead:

1. Derive a stable submission key from `occurrence_id`.
2. Submit or ensure the durable job.
3. Return immediately after durable acceptance.

The schedule occurrence must not remain open for the job duration. Schedule redelivery then re-ensures the same job rather than launching duplicate work.

## 14. Public job surface

Expose a domain-handle API consistently through CLI, RPC/REST, MCP where appropriate, and MobKit SDKs:

```text
jobs/get
jobs/list
jobs/cancel
jobs/progress
jobs/result
jobs/artifacts
jobs/retry
jobs/subscribe
jobs/unsubscribe
monitors/start
```

App-facing responses expose `JobId`, runner display name, status, progress, timestamps, restart class, and safe result summaries.

Worker IDs, raw fence tokens, secret references, and sensitive canonical arguments remain internal.

`monitors/start` is a convenience surface over durable job submission, not a
second lifecycle authority. It accepts the script/command, output protocol,
restart declaration, checkpoint policy, notification subscription, resource
limits, and optional terminal condition. `jobs/cancel`, `jobs/get`, and the
ordinary job health/status surfaces remain authoritative.

Likewise, a “watch registry” is a query/projection over durable jobs and their
subscriptions, not another mutable authority. Console/RPC views may expose
active watches, last committed observation, last notification, next scheduled
evaluation, source health, and delivery backlog without owning lifecycle state.

## 15. Immediate containment before the full system lands

Before durable execution ships:

1. Stop recovered shell completion notices from suggesting `shell_job_status` when no matching record exists.
2. State explicitly that current shell background jobs are process-local and volatile.
3. Document the full timeout hierarchy, including MobKit’s 120/125/130-second callback tier.
4. Add diagnostics that report the effective deadline and its winning owner.
5. Refuse to describe current `background: true` as restart-surviving.

## 16. Delivery sequence

### Phase 0 — Truth and containment

- Fix misleading recovered shell status guidance.
- Document timeout hierarchy.
- Add effective-deadline diagnostics.
- Inventory every dispatcher/wrapper and callback registration path.

### Phase 1 — Execution contracts

- Add `ToolExecutionContract`.
- Add pre-dispatch plan resolution.
- Preserve provider-facing `ToolDef`.
- Extend MobKit callback tool registration metadata.
- Ratchet wrapper forwarding and mixed-batch behavior.

No detached execution behavior changes yet.

### Phase 2 — Durable job foundation

- Add `DetachedJobMachine`.
- Add `meerkat-jobs`.
- Add `DetachedJobStore` to the Meerkat storage-provider set.
- Integrate SQLite mechanics, path authority, doctor/migrate, and conformance.
- Add fenced attempts, leases, cancellation, progress, results, and terminal outbox.
- Generalize the job outbox and runtime inbox for idempotent non-terminal
  notification deliveries as well as terminal completion.
- Add the runtime-owned durable delivery inbox and generated job/runtime
  outbox-delivery composition before publishing job completions to the existing
  wake path.
- Add core status and health projections.

### Phase 3 — Built-in shell migration

- Move background shell onto the durable job service.
- Split or resolve foreground versus detached shell behavior.
- Remove `JobManager` as semantic authority.
- Implement Adoptable/Replayable/Non-resumable classifications honestly.
- Add the agent-authored monitor runner/tool with framed JSONL and opt-in
  line-as-notification stdout protocols.
- Add first-class predicate watches composed through Schedule or durable
  push/event adapters; do not add a parallel timer/registry authority.
- Make notification emission non-terminal so a monitor can notify repeatedly
  while continuing to run.
- Add durable monitor checkpoints, subscriptions, cancellation, output bounds,
  flood/backpressure handling, and secret-redaction tests.
- Add `record`, `notification`, and `event` delivery kinds. Event delivery
  reuses comms `HandlingMode::{Steer, Queue}` with `Steer` as the monitor
  default, so predicate evaluation consumes a provider turn only when ordinary
  agent work is explicitly requested.
- Add fail-closed capability tiers separating broadly grantable predicates from
  unrestricted durable script execution.

### Phase 4 — MobKit and HomeCore callbacks

- Implement async detached callback protocol.
- Add Python and TypeScript SDK support.
- Add execution-time profile credential resolution.
- Add stable interaction-lineage idempotency.
- Add `member_status`, `healthz`, status, capabilities, and console projections.
- Update PR #301’s provider census, doctor/migrate, and conformance coverage for the inherited jobs domain.

Observability ships with detached callbacks, not as later polish.

### Phase 5 — Streaming tools

- Add typed progress sink and cancellation token to dispatch context.
- Reuse the newly landed LLM inactivity-watchdog pattern conceptually.
- Require declared Streaming implementations to provide liveness; fail registration closed if they cannot.
- Reset inactivity only on accepted progress frames.
- Retain a hard absolute ceiling.

### Phase 6 — WorkGraph and Schedule composition

- Add typed job references and terminal evidence composition.
- Make scheduled long work enqueue jobs.
- Add explicit durable `await_job` composition.

## 17. Acceptance gates

The implementation is complete only when all of these hold:

1. Fast MobKit callbacks are documented and tested at 120-second public, 125-second host, and 130-second wire tiers.
2. A detached submit commits before returning a receipt.
3. A crash after commit but before receipt returns the same `JobId` on retry.
4. A completion-triggered agent retry cannot launch a second HomeCore scan for the same interaction intent.
5. No provider turn or callback remains open while a detached job runs.
6. A stale worker cannot report progress or terminalize after a newer fence.
7. A non-resumable scan becomes `WorkerLost`/`NeedsAttention` after worker loss and is never automatically replayed.
8. An adoptable or checkpointed runner resumes according to its declared contract.
9. Credentials are re-resolved on every attempt and no secret bytes appear in job storage, logs, events, or artifacts.
10. Terminal result plus outbox survives a crash before runtime-feed delivery.
11. Duplicate outbox/feed delivery produces one agent-visible application.
12. `member_status` distinguishes awaiting-detached from provider/callback execution.
13. `healthz` remains healthy while jobs are legitimately queued/running/awaited and degrades on stale workers or delivery failure.
14. Detached execution works with WorkGraph disabled.
15. External storage providers must supply the jobs domain or fail closed.
16. Schedule redelivery ensures one job per occurrence.
17. Graceful shutdown stops claiming new work and reconciles leases without waiting for long jobs.
18. Every dispatcher wrapper preserves execution contracts and resolution.
19. Public `ToolDef` serialization remains unchanged unless a separately versioned public contract explicitly adds execution metadata.
20. Machine schema, alphabet, composition, storage-conformance, and generated wire-schema ratchets remain green.
21. Crash/reopen recovery rehydrates the latest committed attempt and fence
    without advancing either; an adoptable or checkpoint-resumable worker can
    resume from persisted authority, while a genuinely older writer is still
    rejected after an explicit later claim advances the fence.
22. An agent-authored monitor script can emit a durable notification, continue
    running, emit a later notification, and terminalize only on its declared
    exit/completion policy or cancellation.
23. `framed_jsonl` notification, progress, checkpoint, and completion frames
    are distinguished typed; a notification frame alone never completes the
    job.
24. Opt-in line mode turns each stdout line into a notification while stderr
    and bounded diagnostics cannot block the notification stream.
25. Crash after notification outbox commit but before runtime delivery retries
    the notification, and crash after runtime inbox commit but before job
    acknowledgement does not create a duplicate conversation application.
26. Monitor restart receives the latest committed checkpoint without advancing
    the attempt fence merely because the process/runtime reopened.
27. Notification idempotency suppresses a replayed observation but still
    permits a later distinct observation from the same continuing monitor.
28. Flood/rate-limit behavior is bounded, typed, observable, and recoverable;
    it neither silently drops events nor silently terminates the monitor.
29. Cancelling or unsubscribing has distinct semantics: cancellation stops the
    durable job through machine authority, while unsubscribe only stops future
    delivery to that subscription.
30. A `notification` predicate crossing produces a durable user-visible
    notification without opening a provider turn; `event` delivery requests
    ordinary agent work only after the predicate has requested judgment.
31. Monitor event delivery uses the canonical comms `HandlingMode` unchanged:
    `Steer` targets the earliest admissible cooperative boundary, `Queue`
    preserves the current run for outer-loop/next-turn handling, and `Steer`
    is the explicit monitor default.
32. First-class time-based watches are driven by the existing Schedule
    authority, and push-capable sources use durable source cursors; no second
    timer or mutable watch-registry authority is introduced.
33. Polling enforces minimum intervals, bounded concurrency, jitter, backoff,
    and per-source budgets, and exposes its steady-state load and failures
    through typed health.
34. Native and host/domain sources advertise durability from their real
    dependency and restart contract. A host-dependent callback watcher cannot
    claim the same availability as a replayable stable-HTTP predicate.
35. A restart test commits a baseline observation, kills the gateway during a
    later evaluation, reopens without advancing the fence, resumes from the
    committed checkpoint without re-notifying the baseline, emits exactly one
    notification for each of two later distinct observations, and continues
    monitoring afterward.
36. Unavailable host-dependent sources degrade visibly and retry according to
    policy without losing the watch, fabricating progress, or spending an
    agent turn merely to rediscover source unavailability.
37. The active-watch console/RPC registry is derived from durable jobs,
    checkpoints, subscriptions, Schedule state, and health; it cannot mutate
    lifecycle outside the job and Schedule authorities.
38. `Failed`, `Cancelled`, `WorkerLost`, and `NeedsAttention` terminalize with
    the same atomic typed outbox protocol as `Succeeded`; a configured agent
    subscription receives the failure/loss exactly once according to its
    canonical comms `HandlingMode`, rather than discovering it only through
    health polling.
39. Restart acceptance covers the topology matrix explicitly: gateway-only
    restart can reconcile an Adoptable worker with a stable live handle;
    co-restarting a gateway and in-process Non-resumable worker produces
    `WorkerLost`; and co-restart continuation is claimed only for an actual
    externally Adoptable, Checkpoint-resumable, or safely Replayable runner.

The architecture guidance materially changes two parts of the original draft: job lifecycle must be a generated singular authority, and—given MobKit PR #301—there must be one Meerkat-level job store inherited through the composite provider, not parallel Meerkat and MobKit job truths.
