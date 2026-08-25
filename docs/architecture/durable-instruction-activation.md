---
title: "Durable Instruction Activation"
description: "Architecture decision for reproducible instruction evolution in durable sessions."
icon: "timeline"
---

# Durable Instruction Activation

Status: Accepted for implementation, pending integrated review and CI
Date: 2026-08-25
Scope: persistent Meerkat sessions and model-visible textual instructions

## Need

Long-lived agents need to adopt reviewed instruction revisions without
silently changing the meaning of an earlier transcript. The durable change
must be reproducible, auditable, exactly-once under retry, safe while a turn is
active, and distinguishable from a controller's desired assignment.

The application remains responsible for authoring instruction content,
evaluating it, approving it, and choosing a rollout. This primitive does not
hot-swap executable skill resources, tool implementations, hooks, provider
configuration, or build composition. Those remain with their existing typed
factory and runtime owners and may require respawn or a new lineage.

## Existing authority is the baseline

Meerkat does not need another chronological delivery mechanism:

- `Session` owns the ordered transcript.
- `Message::System` is repeatable and position-sensitive.
- provider lowering consumes the transcript order when the resolved model can
  represent mid-conversation System rows.
- the persistent session service already serializes control work with turn
  finalization and recovery gates.
- an append admitted while a content turn is active cannot alter that in-flight
  request.

The missing generic capability is typed artifact and activation identity, CAS,
durable receipts, safe persistence, compatibility admission, introspection,
and rewrite protection on that existing ordered transcript fact.

`session/update_system_prompt` owns a different semantic. It versions a
replaceable prompt slot through transcript rewrite and projects only the latest
version. Reusing it would silently reinterpret older conversation under a new
prompt, so it is not the instruction-activation authority.

## Rejected alternatives

### A parallel activation ledger or pending table

Rejected. It would create a second answer to what entered model context and a
cross-store commit problem. Desired rollout is application/MobKit state;
effective durable context is Meerkat transcript state.

### Generic append metadata or string conventions

Rejected. `source`, idempotency strings, or rendered prefixes cannot prove
artifact identity, content digest, supersession, rollback, or stale-writer
rejection. Typed truth must live on the transcript row.

### Keyed prompt replacement

Rejected. Latest-only projection erases the chronological transition from
model context and changes the interpretation of prior conversation.

### Cumulative raw instruction bodies

Rejected. Two equal-priority bodies that happen to occur in order do not
define supersession. Contradictory revisions would both remain instructions.

### Fork on every revision

Rejected as the default. A new lineage is valid for destructive reset or a
provider that cannot represent the transition, but ordinary compatible agents
should retain continuity.

## Domain fact and model projection

An activation is an ordinary ordered `Message::System` row carrying a sealed
typed instruction-activation identity. It is not a new queue, message role, or
session metadata ledger. The canonical row stores one exact, versioned render
envelope. The v1 envelope contains JSON-quoted single-line identity headers, a
blank-line delimiter, then the exact approved UTF-8 body without normalization.
The typed identity stores `render_version = 1` and the explicit predecessor in
`supersedes`. Deserialization re-renders from the typed fields and body and
requires byte equality with `SystemMessage.content`.

In the origin session, model materialization preserves the row's role, position,
and already-stored envelope bytes. The origin envelope states:

- the named revision becomes effective from this transcript boundary
- it supersedes the earlier activation for the same namespace and key
- the exact approved instruction body follows

The same render version also permanently defines an inherited historical-only
envelope for a child `SessionId`; it quotes the original envelope as historical
context and states that it is not an activation in the child. This explicit
versioned rendering, rather than ordering folklore, gives the
later body its same-key supersession meaning and makes replay independent of a
future renderer upgrade. Earlier rows remain durable evidence for conversation
that occurred under them. New render versions are additive and permanently
ratcheted; an existing row is never re-rendered under a newer version.

The exact v1 origin envelope uses LF separators, headers in the order below,
canonical JSON string escaping for string values, lowercase digest hex, decimal
body byte length, one empty line, then the body bytes with no added suffix:

```text
[meerkat-instruction-activation-v1]
namespace=<json-string>
key=<json-string>
revision=<json-string>
activation=<json-string>
supersedes=<json-string-or-null>
origin_session_id=<json-string>
content_sha256=sha256:<64-lowercase-hex>
body_bytes=<decimal>

<exact-utf8-body>
```

The inherited v1 projection changes only the first line to
`[meerkat-inherited-instruction-v1]`, inserts
`child_session_id=<json-string>` after `origin_session_id`, and inserts
`status=historical_only_not_active_in_child` before `body_bytes`. These exact
algorithms remain available for every persisted `render_version = 1` row.

The sealed identity contains:

- `InstructionNamespace`, an application or issuer namespace
- `InstructionKey`
- `InstructionRevisionId`
- `InstructionContentDigest`
- `InstructionActivationId`
- `supersedes: Option<InstructionActivationId>`
- `origin_session_id`, the Meerkat `SessionId` that minted the row
- `render_version`

The digest is canonical lowercase `sha256:<64 hex digits>` over the exact UTF-8
bytes of the approved body. Requests enforce configured body and identifier
bounds. Meerkat verifies the digest before mutation. Within one retained
session lineage, one `(namespace, key, revision_id)` can bind only one digest
and body. Global cross-session artifact immutability remains application-owned;
Meerkat is not an artifact registry.

The System row's semantic carriers must be mutually exclusive. An ordinary
append identity, replaceable prompt-version identity, and instruction
activation identity cannot coexist. All constructors, deserialization paths,
head materialization, rewrites, compaction, and activation validate this
closed invariant.

`origin_session_id` is the lineage fact. Meerkat does not infer a MobKit
generation or invent a second lineage identifier. In the origin session, the
latest activation for a key is effective. In a fork whose `SessionId` differs,
copied rows are inherited historical context until that child appends an
origin-local activation.

## Compatibility, machine admission, and effective truth

An activation receipt means the row is committed to the authoritative
persistent session. `Applied` additionally means the already-materialized
session projection was converged to that committed transcript before the
command completed. No disposition claims that a provider request has already
occurred.

Repository evidence rules out a separate actor-retirement mechanism for the
compatible text path. Every turn rebuilds its model request from
`Session::messages_for_model_boundary`; a compatible provider does not retain a
second instruction context inside the actor. The existing runtime-turn
finalization boundary already serializes a transcript mutation after any active
turn and before a new one. The activation facade therefore holds that boundary,
asks the generated runtime owner for the current transcript-edit admission
verdict, checks the machine-owned resolved capability surface for the current
materialization, rejects an open realtime channel, and only then calls the
persistent typed mutation seam. Model reconfiguration threads its already
resolved target capability through live apply and rollback; it never re-reads a
mutable registry between generated admission and installation.

This is intentionally not a new MeerkatMachine transition. The machine already
owns the active-work admission fact used here. Adding another generated state
plane would duplicate transcript authority without adding a runtime fact. The
persistent service remains unable to expose activation directly through the
generic control extension; surfaces must use the facade that holds runtime
admission and compatibility.

Admission outcomes are:

- a model without mid-conversation System support returns
  `UnsupportedCurrentLowering` and appends nothing
- a detached, dormant, staged, unknown, or otherwise unmaterialized target
  returns `TargetNotMaterialized` and appends nothing in v1
- an open live channel returns `LiveChannelOpen` and appends nothing; v1 does
  not claim a close/reopen handoff it has not completed
- a staged session returns `TargetNotMaterialized`
- an ephemeral service returns `DurabilityUnavailable`

Store-backed reads remain available for detached and dormant sessions. Mutation
does not pretend that persisted LLM metadata alone proves a current lowering.
Provider adapters remain a final fail-closed guard after the catalog-owned
admission check. A later model hot-swap that cannot represent the already
recorded ordered System rows must also fail closed before the new identity is
installed.

## CAS, idempotency, and rollback

`InstructionActivationExpectation` is a closed enum:

- `Absent`
- `Effective(InstructionActivationId)`

There is no optional or wildcard predecessor. Activation IDs are scoped by
session lineage, namespace, and key and bind an immutable request fingerprint.

- An exact retry while that activation is still effective returns `Duplicate`.
  When the caller supplied an external write fence, the retry still enters the
  target-locked RuntimeStore boundary and revalidates that fence before the
  receipt is returned; it appends no second activation row.
- Reusing an activation ID with another ref or body returns
  `ActivationIdentityConflict`.
- Replaying an exact old activation ID after a later activation returns
  `RecordedButNotEffective`; it never proves adoption and never reactivates it.
- A stale predecessor returns `EffectiveActivationConflict` and appends
  nothing.
- A repeated revision ID with another body or digest returns
  `ImmutableRevisionConflict`.

Rollback is a new activation ID naming an earlier immutable revision. It is an
append at a later transcript boundary, never erasure or restore. This prevents
ABA ambiguity.

## Persistent and cancellation-safe mutation

Only `PersistentSessionService` implements durable activation. The operation
uses the existing turn-finalization and recovery gates and the existing actor
command path. The session actor derives and appends the exact activation. The
persistent owner then commits that actor state through the configured
WholeBlob or HeadCanonical boundary. There is no detached candidate document,
post-commit actor convergence step, or policy-specific recovery state machine.

The command runs in a detached owner task that retains the turn boundary until
the actor-owned durable commit completes. Transport or caller cancellation can
drop only its waiter, not the commit owner. A persistence failure returns no
receipt and discards the mutated actor through the existing cleanup path, so
that actor cannot admit a later turn or overwrite canonical state. A lost
successful response is recovered by querying canonical activation records
before retry; an exact retry returns `Duplicate`.

When live support is enabled, activation first acquires the existing
machine-owned live/open lifecycle lease, then the turn-finalization boundary,
and rechecks live-channel absence and generated admission while both are held.
This prevents a new live/open from crossing the actor/store commit and makes an
already-started live/open finish before activation decides admission.

An external composition may supply a generic `RuntimeStoreWriteFence` through
the explicit fenced activation verb. Meerkat first owns its existing live-open
lease and turn-finalization boundary. The selected RuntimeStore then invokes
the fence synchronously from inside its own target write lock or SQLite
transaction, with the physical target publication itself as the fenced
operation. `Applied` means that operation ran exactly once. `Conflict` and
`Backoff` mean it did not run, so neither durable transcript nor live session
projection advances.

The fence must acquire external lifecycle authority without waiting, reread
its canonical facts, and retain its serialization guard while invoking the
target publication. This avoids lock inversion with an external retirement
path that already owns lifecycle authority and is waiting for Meerkat's safe
boundary. MobKit uses the seam for stable identity, generation, session, and
fencing-token continuity. Meerkat does not persist or interpret a transient
runtime alias, and the fence remains a precondition on the existing
RuntimeStore commit rather than another activation authority. RuntimeStore
decorators must opt in to the fenced verb explicitly; the default is a typed
unsupported refusal so a decorator cannot silently bypass its projection or
recovery behavior.

Physical store revisions and fencing tokens remain server-internal. A public
command receipt binds session ID, lineage, full activation identity,
predecessor, a stable activation ordinal, the current transcript projection
witness, and the event's typed disposition. Persistence introspection derives
that record from the RuntimeStore-backed transcript, never from a receipt
cache. It does not reconstruct the historical live-projection disposition.
Current rollout observation may join that durable record with runtime
availability, but it may never replace the record with a cache.

## Rewrite, compaction, restore, and fork

Activation rows are immutable transcript-boundary anchors.

- Ordinary append and prompt-update APIs cannot mint or adopt them.
- Generic same-session rewrite and prompt update require exact equality of
  every activation row at its transcript index; they cannot mint, alter, move,
  erase, or restore an activation across conversational content.
- Same-session restore requires every activation row to remain byte-for-byte
  equal at its exact transcript index. It cannot move an activation across
  conversational content or restore an erased activation boundary.
- Deserialization validates current rows and every retained revision body.
- Fork-at may copy only exact activation rows from the selected source prefix.
  Generic fork replacement rejects every activation-bearing replacement, so it
  cannot forge inherited or effective authority.
- In a child session, copied rows render with a permanently versioned
  historical-only envelope. A child with inherited rows cannot start a model
  turn until it has appended an explicit origin-local activation for every
  inherited effective key. The child activation may name the inherited row in
  `supersedes` while its CAS expectation remains `Absent` for the child origin.

Meerkat's current compactor retains System rows. V1 deliberately follows that
existing rule: every activation anchor and exact body stays in the live ordered
transcript, while ordinary conversation may be summarized around it.
Compaction never invents, reorders, edits, or deletes an activation and the
model-boundary projection preserves every retained anchor in place. This keeps
idempotency, revision immutability, rollback evidence, and replay independent
of a second retained-history lookup.

This is safe but not free: activation bodies accumulate for the life of the
lineage. The 256 KiB per-body limit and 1 MiB total retained activation-body
budget bound v1 growth. An exact retry of the current activation remains legal
at the budget because it appends nothing. Growth beyond the budget is a stable
typed refusal. Retiring an old body is a future schema change that must first
define a typed summary/witness mapping and make duplicate and
immutable-revision checks consult retained audit history.

## Typed public contract

All identifiers are non-empty trimmed UTF-8, at most 128 bytes, and reject
control characters. Instruction bodies are at most 256 KiB. Namespace naming
does not introduce an authorization system: the application remains
responsible for exposing the activation command only through its governed
approval workflow, while Meerkat authenticates and authorizes the containing
surface according to that surface's existing rules.

`InstructionActivationRequest` contains:

- namespace, key, revision ID, declared digest, and exact body
- activation ID
- closed CAS expectation `Absent | Effective(id)`
- explicit `supersedes`, which must equal the observed previous same-key
  activation when present but is not the CAS carrier

`InstructionActivationReceipt` contains session ID, origin session ID, full
activation identity, predecessor activation ID, the stable zero-based ordinal
among activation rows, the current projection witness, and `Applied |
Duplicate`. Identity and ordinal are stable across compaction. The projection
witness contains the current zero-based message index and current prefix
revision, so it can change when compaction summarizes earlier conversation.
`Applied` proves the actor-owned materialized session state was committed by
the lifecycle/persistence owner.

`InstructionActivationRecord` is persistence-only and contains the durable
identity, origin, predecessor, stable activation ordinal, and current
projection witness.

`InstructionActivationReadQuery` accepts optional namespace/key filters and a
bounded offset with `limit` from 1 through 200.
`InstructionActivationReadPage` returns durable records in transcript order
and an optional next offset. When the query names one exact namespace and key,
it also returns core-derived `InstructionActivationKeyState`: the effective
origin-local record, chronological head, child explicit-activation
requirement, and exact expectation/supersedes values for the next request.
MobKit consumes this state instead of reimplementing transcript or fork
semantics.

Stable `InstructionActivationErrorCode` variants are:

- `invalid_request`
- `digest_mismatch`
- `effective_activation_conflict`
- `activation_identity_conflict`
- `immutable_revision_conflict`
- `recorded_but_not_effective`
- `inherited_activation_requires_explicit_activation`
- `malformed_activation_history`

Stable `InstructionActivationAdmissionErrorCode` variants are:

- `target_not_materialized`
- `unsupported_current_lowering`
- `live_channel_open`
- `durability_unavailable`
- `session_busy`
- `external_write_fence_conflict`
- `external_write_fence_backoff`

JSON-RPC maps admission refusals to stable typed data and preserves domain
codes from the persistent seam. No activation refusal is inferred by parsing
its message.

Core types are:

- `InstructionNamespace`, `InstructionKey`, `InstructionRevisionId`
- `InstructionContentDigest`, `InstructionRevisionRef`
- `InstructionActivationId`, `InstructionActivationIdentity`
- `InstructionActivationExpectation`, `InstructionActivationRequest`
- `InstructionActivationDisposition`, `InstructionActivationReceipt`
- `InstructionActivationRecord`
- `InstructionActivationError` with stable error codes for every refusal above

Service/facade methods are:

- `MeerkatSessionRuntime::activate_instruction`
- `MeerkatSessionRuntime::activate_instruction_with_write_fence`
- `MeerkatSessionRuntime::read_instruction_activations`
- `SessionServiceHistoryExt::read_instruction_activation_records` as the
  persistence-only internal/read-model seam

Surfaces:

- JSON-RPC `session/activate_instruction` and
  `session/instruction_activations`
- generated Rust wire and JSON Schema contracts
- model catalog capability `supports_mid_conversation_system_messages`

Transport handlers only parse, authorize, invoke the service, and lower typed
errors. They do not classify state or rebuild receipts.

## Ownership and failure table

| Fact | Canonical owner | Failure behavior |
| --- | --- | --- |
| Content, evaluation, approval | Application | Meerkat validates declared identity and digest only |
| Desired rollout | Application composition / MobKit | Never treated as adoption proof |
| Ordered effective activation | Persistent Meerkat Session via RuntimeStore | No commit means no effective change |
| Active-turn boundary | MeerkatMachine admission plus runtime facade | Wait behind the finalization boundary, then commit |
| Model/live compatibility | Resolved Meerkat runtime capabilities | Typed refusal before append |
| Same-lineage activation audit | Transcript plus retained revision graph | Load fails closed on malformed state |
| Rollback | Later typed activation | No deletion or restore |

## Development binding and delivery

This isolated correction branch is an implementation proof on the published
paired 0.8.29 line. Its manifest and lockfile still name the early immutable
Realtime revision `e3b8fa99da062919a0f85f7f42f0a74e7a9f829d`; this branch is
therefore not a publishable integration candidate.

The reviewed durable commit is replayed onto the release authority's exact
qualified realtime Meerkat head. That integrated head must preserve the
Realtime source and immutable revision recorded by that head's manifest and
lockfile. MobKit then binds its Meerkat dependency to the exact integrated
commit, not to a package release or movable branch, and preserves the same
locked Realtime source. Cross-repository CI consequently tests the development
graph it claims to review.

After the integrated gate, the release authority owns BuildBuddy Turbo S
e2e-smoke diagnosis, repair, rerun, and green evidence on that exact Meerkat
commit. The exact Meerkat/MobKit pair must then pass isolated no-deploy OB3 and
HomeCore qualification before the Meerkat lead may publish 0.8.30. This feature
lane does not independently merge, tag, or publish either repository.

## Dogma check

- The ordered transcript remains the sole model-context authority.
- No new activation machine, queue, or ledger is introduced.
- RuntimeStore alone proves physical currentness.
- Provider capability and live state fail closed before mutation.
- Typed core code owns semantics; surfaces stay thin.
- Global policy governance stays outside Meerkat.
- Durable receipts are unavailable where durability cannot be proved.
