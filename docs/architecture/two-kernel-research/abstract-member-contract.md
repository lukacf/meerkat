# Identity-Native Abstract Member Contract

Status: frozen seam contract

## Purpose

- Define the sole semantic boundary between `MobMachine` and `MeerkatMachine`.
- Make `MobMachine` identity-centric rather than session-centric.
- Keep Meerkat-to-Meerkat comms out of the Mob seam.

## Core Split

The intended ownership cut is:

- `MobMachine` owns who a member is.
- `MeerkatMachine` owns how the current session-scoped incarnation executes.

That means:

- `MobMachine` owns stable `AgentIdentity`, generation tracking, roster state, lifecycle intent, flow-owned work assignment, and identity-scoped observability.
- `MeerkatMachine` owns `SessionId`, `MemberRef`, turn execution, tools, ops, comms, wake/drain behavior, and recovery of the current session-scoped runtime.
- The boundary between them is not keyed by `SessionId`. It is keyed by the current member incarnation.

## Implication For Current Layering

The current opaque `MeerkatId` is too weak to be the long-term semantic key for an identity-centric mob.

The intended replacement is:

- `AgentIdentity` as the durable mob-owned identity
- `AgentRuntimeId` as the current incarnation handle
- `SessionId` and `MemberRef` remaining internal to `MeerkatMachine`

## Freeze Note

This document is the semantic seam contract used by the target composition
proof, not a claim that the exact-current implementation already exposes this
surface directly.

The exact-current branch still leaks some today-state details, especially:

- `SessionId` through current roster and tooling surfaces
- raw turn-delivery entry points such as `internal_turn`, `start_turn`, and
  `start_flow_step`
- some addressability and topology-adjacent plumbing

Those are implementation and lowering concerns. They do not change the target
Mob-visible seam that is now frozen and composition-proven.

See also:

- `mob-meerkat-composition-freeze.md`
- `mob-meerkat-composition-proof-handoff.md`
- `bridge-alphabet.md`

## Contract Entities

### Mob-visible entities

- `AgentIdentity`
  - Durable identity for a mob member, such as `coordinator` or `lead`.
  - Stable across restarts, session replacement, and reset generations.
- `Generation`
  - Reset epoch for an `AgentIdentity`.
  - Advances only on intentional reset, not on respawn.
- `AgentRuntimeId`
  - Derived incarnation identifier: `(AgentIdentity, Generation)`.
  - Stable across respawn.
  - Changes on reset.
- `FenceToken`
  - Monotonic authority token for the currently authorized live incarnation of an identity.
  - Changes whenever authority is reissued, superseded, or transferred.
  - Distinguishes stale and current incarnations even when `AgentRuntimeId` is preserved across respawn.
- `TopologyIntent`
  - Mob-owned declaration of which identities should collaborate.
  - Identity-keyed and generation-stable.
  - Describes desired communication relationships without exposing `PeerId`, trusted-peer specs, or transport mechanics.
- `WorkRef`
  - Mob-owned handle for a unit of work assigned to a member.
  - Canonical seam key, regardless of how the current codebase internally
    realizes work submission.
- `WorkSpec`
  - Abstract description of the work to execute.
  - Authored by `MobMachine`, interpreted by `MeerkatMachine`.
  - Canonical seam payload, regardless of whether the current implementation
    still lowers through raw turn-delivery helpers.

### Hidden from `MobMachine`

- `SessionId`
- `RuntimeEpochId`
- `MemberRef`
- `RunId`
- `InputId`
- `OperationId`
- `PeerId`
- wake causes
- queue depth
- barrier membership
- async-op internal state
- comms drain and keep-alive internals
- trusted-peer and routing internals

These may exist inside `MeerkatMachine`, but they are not part of the Mob-facing contract.

Current code still leaks some of these details, especially `SessionId`; hiding
them remains explicit refinement/cutover work.

## Ownership Matrix

| Semantic fact | Owner | Notes |
| --- | --- | --- |
| Stable agent identity | `MobMachine` | Durable mob-level truth |
| Current generation | `MobMachine` | Advances on reset |
| Current runtime incarnation | `MobMachine` | Exposed as `AgentRuntimeId` |
| Current authority and fence token | `MobMachine` | Determines which live incarnation is allowed to act |
| Binding from incarnation to hidden runtime binding | `MobMachine` | The binding exists at the seam, but `SessionId` and `RuntimeEpochId` remain hidden |
| Collaboration topology intent | `MobMachine` | Identity-to-identity collaboration truth, not transport truth |
| Hidden execution epoch | `MeerkatMachine` | Runtime-ordering truth inside the current binding, not a public identity signal |
| Current session/runtime execution state | `MeerkatMachine` | Internal to Meerkat |
| Member work execution and terminal outcome | `MeerkatMachine` | Returned through the contract in Mob-level terms |
| Member-to-member comms | `MeerkatMachine` | Not part of the Mob seam |
| Public addressability policy | Perimeter or later `MobMachine` policy surface | Explicit migration decision; internal work dispatch must not depend on it |
| Identity-scoped event history | `MobMachine` | May be projected from runtime notices |

## Topology Intent

Topology needs an explicit ownership split:

- `MobMachine` owns which identities should be connected for collaboration and flow purposes.
- `MeerkatMachine` owns peer identifiers, trust edges, routing realization, and message delivery.

That means topology intent is part of mob truth, but raw comms mechanism is not. The bridge must not reintroduce:

- `WirePeer`
- `UnwirePeer`
- `DeliverEnvelope`

If topology needs to cross the seam, it should cross only as high-level identity-keyed intent or as provisioning-time capability, not as raw peer mutation.

## Authority And Fencing

Authority is first-class `MobMachine` truth.

If `MobMachine` owns durable identity, respawn, reset, and "which incarnation is the real one," then it must also own the rules that prevent split-brain. Lease and fencing are therefore part of the semantic model, not optional operational garnish.

### Definitions

- Lease
  - The temporary right for one live incarnation to act on behalf of an `AgentIdentity`.
- Fence token
  - A monotonic token attached to that authority.
  - Any stale actor with an older token is fenced out of future state-changing work.

### Semantic split

- `MobMachine` owns:
  - whether an identity currently has authority
  - which `FenceToken` is current
  - when authority is granted, revoked, retired, or superseded
- Perimeter infrastructure owns:
  - heartbeats
  - wall-clock expiry
  - storage and compare-and-swap mechanics
  - delivery of timeout or health signals that propose authority transitions

This keeps time and storage mechanics out of the proof core while keeping authority correctness inside it.

### Authority semantics

- At most one active `FenceToken` may exist for an `AgentIdentity` at a time.
- A newer `FenceToken` permanently fences older ones.
- Respawn may preserve `Generation` and `AgentRuntimeId`, but it must not preserve stale authority.
- Reset advances `Generation`, creates a fresh `AgentRuntimeId`, and issues fresh authority.
- Any lifecycle result, work result, or observability notice from `MeerkatMachine` is valid only if tagged with the current `FenceToken`.
- Any state-changing action aimed at `MeerkatMachine` is valid only when scoped to the current `FenceToken`.

### Why the token is required at the seam

`RespawnMember` is defined to preserve `AgentRuntimeId`.

That means an old half-dead incarnation and a newly respawned incarnation can share the same:

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`

The `FenceToken` is what separates "current owner" from "stale ghost."

This is now part of the proven seam shape, not only a design preference.

## Lifecycle Lane

The lifecycle lane manages the existence and incarnation of a member.

### Mob inputs

- `SpawnMember(identity, member_spec)`
  - Create a new member for an absent `AgentIdentity`.
  - Initial generation is `0`.
- `RespawnMember(identity)`
  - Recover the current incarnation of an existing identity.
  - Generation does not advance.
  - `AgentRuntimeId` does not change.
- `ResetMember(identity, member_spec)`
  - Intentionally replace the current incarnation.
  - Generation advances by one.
  - A fresh `AgentRuntimeId` is minted.
- `RetireMember(identity)`
  - Begin draining retirement for the current incarnation.
  - No new work should be admitted once retirement takes effect.
- `DestroyMember(identity)`
  - Remove the identity from the active roster after retirement or failure handling.

### Meerkat outputs

- `MemberReady(agent_runtime_id, fence_token)`
- `MemberProvisionFailed(identity, reason)`
- `MemberRetired(agent_runtime_id, fence_token)`
- `MemberDestroyed(identity)`

### Lifecycle semantics

- Respawn preserves identity and generation.
- Respawn reissues authority and must result in a strictly newer `FenceToken` than any previously active incarnation.
- Reset preserves identity but advances generation.
- Reset issues a fresh `FenceToken` together with the fresh generation.
- `SessionId` replacement is an internal implementation detail of reset/respawn, not contract truth.
- Retirement is draining, not abrupt disappearance.
- The normal destroy path is `RetireMember` -> `MemberRetired` -> `DestroyMember`.
- `DestroyMember` may also be used as an emergency terminal step, but only after `MobMachine` has already decided that draining cannot complete or should not be awaited.

## Work Lane

The work lane lets `MobMachine` assign flow-owned work to the active incarnation of a member.

### Mob inputs

- `SubmitWork(agent_runtime_id, fence_token, work_ref, work_spec)`
- `CancelWork(work_ref)`
- `CancelAllWork(agent_runtime_id, fence_token)`

### Meerkat outputs

- `WorkAccepted(agent_runtime_id, fence_token, work_ref)`
- `WorkRejected(agent_runtime_id, fence_token, work_ref, reason)`
- `WorkCompleted(agent_runtime_id, fence_token, work_ref, result_ref)`
- `WorkFailed(agent_runtime_id, fence_token, work_ref, failure_ref)`
- `WorkCanceled(agent_runtime_id, fence_token, work_ref)`

### Work semantics

- Flow planning remains identity-centric inside `MobMachine`.
- Flow runs remain a Mob-level concept; the work lane is the lower-level unit used to realize flow steps and other directed member work.
- Crossing into `MeerkatMachine` targets the currently active `AgentRuntimeId`.
- Crossing into `MeerkatMachine` also targets the currently active `FenceToken`.
- Internal work dispatch must not depend on public addressability policy.
- `WorkRef` and `WorkSpec` are proposed new abstractions that make the seam explicit; today the code still routes this behavior through raw turn delivery.
- `MobMachine` should reason only over acceptance and terminalization, not over Meerkat queueing folklore such as busy/idle flags.
- `CancelAllWork` is a bulk work operation, not a lifecycle fact, even when it is commonly paired with retire or reset policy.

## Observability Lane

The observability lane exists for mob-level insight, console projection, and debugging. It is not the source of canonical control truth.

### Example notices

- `MemberNotice(agent_runtime_id, fence_token, notice_kind, payload_ref)`
- `WorkNotice(agent_runtime_id, fence_token, work_ref, notice_kind, payload_ref)`

Typical examples include:

- readiness transitions
- human-readable progress text
- streaming output availability
- artifact availability
- usage or cost snapshots
- summarized failure context

### Observability rule

- `MobMachine` may record and project these notices into identity-scoped history.
- Flow semantics must not depend on observability notices unless a notice class is explicitly promoted into the formal contract.
- Operator-facing surfaces such as consoles may still use observability notices for rendering transcripts, progress, and artifacts. The restriction applies to flow planning and lifecycle decisions, not to display projection.

## Explicitly Out Of Contract

The following concerns do not belong in the abstract member contract:

- Meerkat-to-Meerkat comms delivery
- trust graph and wiring internals
- topology realization mechanics
- raw peer envelopes
- public send-path policy checks
- barrier, wake, drain, and retry internals
- raw session identifiers and session-local runtime ids
- raw runtime epoch identifiers

This is a deliberate boundary choice. Mob manages identities, lifecycle, and work assignment. Meerkat manages execution and comms.

## Contract Invariants

- One `AgentIdentity` has at most one live active generation in the roster at a time.
- One live `AgentRuntimeId` binds to at most one live session-scoped Meerkat incarnation at a time.
- One `AgentIdentity` has at most one active `FenceToken` at a time.
- A superseding `FenceToken` irrevocably fences older tokens for the same identity.
- `RespawnMember` preserves `Generation` and `AgentRuntimeId`.
- `RespawnMember` must result in a newer `FenceToken` than any prior active incarnation of the same identity.
- `ResetMember` strictly advances `Generation` and creates a fresh `AgentRuntimeId`.
- `ResetMember` must also issue a fresh `FenceToken`.
- No `WorkCompleted`, `WorkFailed`, or `WorkCanceled` may appear without a prior `WorkAccepted` for the same `WorkRef`.
- At most one terminal work outcome may appear for an accepted `WorkRef`.
- A rejected work item may not later terminalize.
- No lifecycle event, work event, or notice tagged with a stale `FenceToken` may be accepted as canonical truth.
- Observability notices are never canonical lifecycle or work truth by themselves.
- No session-scoped identifier is required for any Mob-level proof obligation.

## Minimum Identity-Native Move

This contract does not require every current MobKit concept to move down into `MobMachine` immediately.

The minimum identity-native move is:

- `AgentIdentity` becomes first-class inside `MobMachine`.
- `Generation` and `AgentRuntimeId` become first-class inside `MobMachine`.
- Authority state and `FenceToken` become first-class inside `MobMachine`.
- Respawn and reset become mob-native lifecycle operations.
- Roster and event streams become identity-keyed.
- `SessionId` disappears from the Mob-facing semantic boundary.
- Current `SessionId` exposure is treated as migration debt, not as future kernel truth.

## Deferred Questions

These questions remain open and should be decided separately from the core contract:

- Should continuity records and checkpoint lineage become part of `MobMachine`, or stay as an external persistence layer that projects onto the same identity model?
- Should any observability classes be promoted into formal flow-driving truth, or should they remain strictly non-semantic?
- Should topology intent cross the seam only at provisioning time, or does it need its own later high-level sync command?
- Which mutable member-spec inputs, such as labels or other reloadable profile metadata, belong to `MobMachine` truth versus higher-level product configuration?

The mapping between `Generation`, `AgentRuntimeId`, and hidden runtime `epoch_id` is defined separately in `generation-epoch-mapping.md`.

## Bottom Line

If identity continuity is a real product semantic, the seam should not be:

- `MobMachine` reasoning over `SessionId`

It should be:

- `MobMachine` owning `AgentIdentity` and `Generation`
- `MobMachine` owning authority and `FenceToken`
- `MeerkatMachine` owning the current session-scoped runtime
- one narrow contract keyed by `AgentRuntimeId` plus `FenceToken`
