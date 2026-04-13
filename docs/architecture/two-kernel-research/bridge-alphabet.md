# Mob-Meerkat Bridge Alphabet

Status: frozen seam alphabet

## Purpose

This document defines the exact semantic alphabet for the `MobMachine <->
MeerkatMachine` seam.

It refines:

- `abstract-member-contract.md`
- `owned-facts-ledger.md`

The goal is to make the seam small enough to prove, but explicit enough that we
do not recreate hidden state by accident.

This alphabet is now part of the frozen composition package rather than an open
draft. The exact-current implementation may still lower into this alphabet
through older runtime/session entry points, but the target seam itself is no
longer provisional.

See also:

- `mob-meerkat-composition-freeze.md`
- `mob-meerkat-composition-proof-handoff.md`
- `abstract-member-contract.md`

## Design Rules

- The seam is keyed by `AgentRuntimeId` plus `FenceToken`, not by `SessionId`.
- `MobMachine` owns identity, generation, authority, and work assignment intent.
- `MobMachine` also owns collaboration topology intent, but not raw comms mechanism.
- `MeerkatMachine` owns session-scoped execution, tools, ops, comms, and work outcomes.
- Command acceptance is transport-level only. Canonical semantic truth is established by emitted events.
- No Meerkat-to-Meerkat message delivery command exists in this alphabet.
- No raw wire or unwire command exists in this alphabet.
- Public addressability policy is not part of this alphabet; internal work dispatch must not depend on it.
- No raw session-local identifiers, runtime epoch identifiers, queue depth, barrier state, or wake/drain internals may cross the seam.
- `WorkRef` and `WorkSpec` are canonical seam abstractions. Current code may
  still express parts of this behavior through raw turn-delivery paths.
- Hiding `SessionId` is still target-state refinement work. Current roster and
  tooling surfaces still leak it.

## Semantic Entities

### Primary keys

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`
- `WorkRef`

### Idempotency refs

- `ProvisionRef`
  - Idempotency handle for one attempt to realize an incarnation.
- `LifecycleRef`
  - Idempotency handle for retirement, destruction, or bulk-cancel commands.
- `TopologyRef`
  - Reserved idempotency handle for any future high-level topology-sync command.
- `WorkRef`
  - Also serves as the idempotency handle for work submission and work cancellation.

### Opaque payload handles

- `MemberSpec`
  - Spawn/recover specification authored by `MobMachine`.
  - May include profile selection, launch mode, runtime mode, tool policy, mutable member metadata, continuity hints, and other non-session semantic inputs.
- `WorkSpec`
  - Work description authored by `MobMachine`.
  - Proposed new abstraction that lowers current raw turn and flow-step delivery into an explicit bridge unit.
  - May include prompt content, flow-step metadata, handling mode, overlays, and render metadata.
- `PayloadRef`
  - Opaque pointer to observational payload material.
- `ResultRef`
  - Opaque pointer to terminal successful output material.
- `FailureRef`
  - Opaque pointer to terminal failure material.

## Lifecycle Intents Versus Bridge Commands

`MobMachine` may expose higher-level lifecycle intents such as:

- `SpawnMember`
- `RespawnMember`
- `ResetMember`
- `RetireMember`
- `DestroyMember`

The bridge alphabet is smaller than that:

- `SpawnMember` lowers to `ProvisionIncarnation(mode = Fresh)`.
- `RespawnMember` lowers to `ProvisionIncarnation(mode = Recover)` using the same `AgentRuntimeId` and a newer `FenceToken`.
- `ResetMember` lowers to `RetireIncarnation(old)` followed by `ProvisionIncarnation(mode = Fresh)` with a new `Generation`, new `AgentRuntimeId`, and new `FenceToken`.
- `RetireMember` lowers to `RetireIncarnation`.
- `DestroyMember` lowers to `DestroyIncarnation`.

This keeps reset and respawn semantics owned by `MobMachine` while keeping the
cross-kernel command surface compact.

Higher-level Mob APIs may still exist above this seam. For example:

- `ReconcileRoster` can stay a Mob API that lowers to lifecycle and topology operations.
- `run_flow`, `flow_status`, and `cancel_flow` can stay Mob APIs that lower to work-lane activity.

Those APIs do not need one-to-one bridge primitives as long as their lowering is
explicit.

## Command Alphabet

### `ProvisionIncarnation`

```text
ProvisionIncarnation {
  provision_ref: ProvisionRef,
  identity: AgentIdentity,
  generation: Generation,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  mode: Fresh | Recover,
  member_spec: MemberSpec,
}
```

Preconditions:

- `runtime_id` belongs to `identity` and `generation`.
- `fence_token` is the current authority token for `identity`.
- No different live binding already exists for `(runtime_id, fence_token)`, unless this is a replay of the same `provision_ref`.
- For `mode = Recover`, a recoverable hidden runtime binding or continuity target exists.

Postconditions:

- Exactly one of `IncarnationReady` or `ProvisionFailed` is eventually emitted for `provision_ref`.
- On success, `MeerkatMachine` binds one hidden session-scoped runtime to `(runtime_id, fence_token)`.

Idempotency:

- Keyed by `provision_ref`.
- Replay with identical payload is allowed.
- Replay with different payload is a protocol error.

### `RetireIncarnation`

```text
RetireIncarnation {
  lifecycle_ref: LifecycleRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}
```

Preconditions:

- `(runtime_id, fence_token)` is current, or this is a replay of an already completed retirement.

Postconditions:

- No new `SubmitWork` may be accepted once retirement takes effect.
- Existing accepted work may drain, complete, fail, or be abandoned according to `MeerkatMachine` semantics.
- `IncarnationRetired` is eventually emitted unless the command was stale or invalid.

Notes:

- Retirement is the normal draining path.
- Retirement by itself does not imply immediate destruction.
- Higher-level Mob policy may pair retirement with `CancelAllWork`, but that is a separate semantic choice.

Idempotency:

- Keyed by `lifecycle_ref`.
- Unknown-or-already-retired may be treated as idempotent success if the same lifecycle goal has already been achieved.

### `DestroyIncarnation`

```text
DestroyIncarnation {
  lifecycle_ref: LifecycleRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}
```

Preconditions:

- `(runtime_id, fence_token)` is current, or this is a replay of an already completed destruction.
- Normal use follows a prior successful retirement of the same incarnation.
- Force-destroy without prior retirement is a higher-level Mob policy decision and should be modeled explicitly, not treated as the default path.

Postconditions:

- The hidden session-scoped runtime no longer exists.
- Any remaining pending completion waiters are resolved.
- `IncarnationDestroyed` is eventually emitted unless the command was stale or invalid.

Idempotency:

- Keyed by `lifecycle_ref`.

### `SubmitWork`

```text
SubmitWork {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
  work_spec: WorkSpec,
}
```

Preconditions:

- `(runtime_id, fence_token)` is current and ready.
- The incarnation is not retired or destroyed.
- `work_ref` has not previously been used with a different `work_spec`.

Postconditions:

- Exactly one of `WorkAccepted` or `WorkRejected` is eventually emitted for `work_ref`.

Idempotency:

- Keyed by `work_ref`.
- Replay with identical payload is allowed.
- Replay with different payload is a protocol error.

### `CancelWork`

```text
CancelWork {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
}
```

Preconditions:

- `work_ref` names work previously submitted against `(runtime_id, fence_token)`, or the cancellation is a replay against already terminal work.

Postconditions:

- If the work was accepted and still cancelable, `WorkCanceled` is eventually emitted.
- If the work is already terminal, no new terminal event is required.

Idempotency:

- Keyed by `work_ref`.

### `CancelAllWork`

```text
CancelAllWork {
  lifecycle_ref: LifecycleRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}
```

Preconditions:

- `(runtime_id, fence_token)` is current, or this is a replay of an already completed bulk-cancel request.

Postconditions:

- In-flight accepted work is interrupted according to `MeerkatMachine` cancellation semantics.
- No direct lifecycle transition is implied by this command.

Idempotency:

- Keyed by `lifecycle_ref`.

## Event Alphabet

### Lifecycle events

```text
IncarnationReady {
  provision_ref: ProvisionRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}

ProvisionFailed {
  provision_ref: ProvisionRef,
  identity: AgentIdentity,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  reason_class: ProvisionFailureClass,
}

IncarnationRetired {
  lifecycle_ref: LifecycleRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}

IncarnationDestroyed {
  lifecycle_ref: LifecycleRef,
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
}
```

Rules:

- `IncarnationReady` is emitted at most once per successful `provision_ref`.
- `ProvisionFailed` is emitted at most once per failed `provision_ref`.
- `IncarnationRetired` is emitted at most once per retirement `lifecycle_ref`.
- `IncarnationDestroyed` is emitted at most once per destruction `lifecycle_ref`.

### Work events

```text
WorkAccepted {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
}

WorkRejected {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
  reason_class: WorkRejectClass,
}

WorkCompleted {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
  result_ref: ResultRef,
}

WorkFailed {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
  failure_class: WorkFailureClass,
  failure_ref: FailureRef,
}

WorkCanceled {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
}
```

Rules:

- `WorkAccepted` is emitted at most once per `work_ref`.
- `WorkRejected` is terminal for `work_ref` and must not be followed by `WorkAccepted`.
- Exactly one of `WorkCompleted`, `WorkFailed`, or `WorkCanceled` may follow `WorkAccepted`.

### Observability events

```text
MemberNotice {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  notice_kind: MemberNoticeKind,
  payload_ref: PayloadRef,
}

WorkNotice {
  runtime_id: AgentRuntimeId,
  fence_token: FenceToken,
  work_ref: WorkRef,
  notice_kind: WorkNoticeKind,
  payload_ref: PayloadRef,
}
```

Rules:

- Notices are observational only.
- Notices may be dropped, compacted, or projected without changing canonical lifecycle or work truth.
- No flow proof may depend on notices unless a notice class is explicitly promoted into the formal contract.
- Operator-facing surfaces may still consume notices for transcript rendering, progress display, and artifact discovery.

## Common Rejection Classes

These are semantic classes, not wire-level error enums.

- `StaleFenceToken`
  - The command references a superseded authority token.
- `UnknownIncarnation`
  - No current hidden binding exists for the supplied `(runtime_id, fence_token)`.
- `InvalidLifecycleState`
  - The command conflicts with the current lifecycle state, such as submitting work to a retired incarnation.
- `RecoverableStateMissing`
  - `mode = Recover` was requested but no recoverable binding exists.
- `DuplicateRefPayloadMismatch`
  - The same idempotency ref was replayed with different payload.
- `WorkNotFound`
  - Cancellation or inspection targeted a nonexistent work item.

## Stale-Token Rule

The stale-token rule is the most important rule in the seam:

- Any command carrying a stale `FenceToken` is rejected as non-canonical.
- Any event carrying a stale `FenceToken` is ignored by `MobMachine` as non-canonical.
- A newer `FenceToken` permanently fences all older tokens for the same `AgentIdentity`.

This is what prevents respawn split-brain when `AgentRuntimeId` is preserved.

## Invariants

- One `AgentIdentity` has at most one current `FenceToken`.
- One `(AgentRuntimeId, FenceToken)` binds to at most one live hidden Meerkat incarnation.
- `ProvisionIncarnation` produces at most one lifecycle result for its `provision_ref`.
- `SubmitWork` produces at most one acceptance decision for its `work_ref`.
- `WorkRejected` is the only terminal refusal path for unaccepted work.
- `WorkCompleted`, `WorkFailed`, and `WorkCanceled` require prior `WorkAccepted`.
- No stale-token event may change `MobMachine` truth.
- No raw `SessionId`, `MemberRef`, `RunId`, `InputId`, `OperationId`, or `PeerId` is required for any Mob-level proof obligation.
- No raw `RuntimeEpochId` is required for any Mob-level proof obligation.

## Explicitly Excluded Surface

This alphabet intentionally excludes:

- `DeliverEnvelope`
- `WirePeer`
- `UnwirePeer`
- `PublicSend`
- raw trusted-peer mutation commands

That exclusion is deliberate. `MeerkatMachine` owns comms truth.

If `MobMachine` keeps a first-class collaboration topology intent, that intent must either:

- be folded into provisioning semantics as a policy snapshot or provisioning-time capability, or
- be introduced later as a separate high-level topology-sync command keyed by identities, not peer ids or raw trust edges

but not by smuggling raw comms delivery into the seam.

## Current Rough Mapping

This is not a 1:1 API mapping. It is a grounding note for the current codebase.

| Target bridge command | Current rough anchor |
| --- | --- |
| `ProvisionIncarnation(mode = Fresh)` | `MobHandle::spawn_spec` -> `MobProvisioner::provision_member` -> `MeerkatMachine::prepare_bindings` + `create_session` |
| `ProvisionIncarnation(mode = Recover)` | current respawn / attach / recovery plumbing around `MobHandle::respawn`, `attach_existing_session`, and runtime recovery paths |
| `RetireIncarnation` | `MobHandle::retire` -> `SessionBackend::retire_member` -> `MeerkatMachine::retire_runtime` + archive |
| `DestroyIncarnation` | current destroy/dispose paths plus runtime destroy/archive handling |
| `SubmitWork` | `internal_turn` / `start_turn` / `start_flow_step` -> runtime input creation -> `MeerkatMachine::accept_input_with_completion` |
| `CancelAllWork` | `MobHandle::force_cancel_member` -> `SessionBackend::interrupt_member` -> `MeerkatMachine::interrupt_current_run` |

The current mapping is intentionally uneven:

- `SessionId` still leaks today through roster and tool views.
- `WorkRef` and `WorkSpec` do not yet exist as explicit code-level concepts.
- flow APIs already exist at the mob layer and should stay there; the work lane is the lower-level unit they lower into.

## Open Questions

- Should topology intent be realized only at provisioning time, or does it need its own high-level sync command later?
- Should any observability classes be promoted into proof-relevant work truth?
- Which mutable member-spec inputs should be treated as roster truth versus higher-level product configuration?

## Bottom Line

The intended seam is:

- a small lifecycle command set
- a small work command set
- a work/lifecycle event stream keyed by `AgentRuntimeId` plus `FenceToken`

and nothing else.
