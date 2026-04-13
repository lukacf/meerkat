# Generation And Epoch Mapping

Status: working draft

## Purpose

- Define the relationship between Mob-owned semantic incarnation and Meerkat-owned execution epoch.
- Prevent `Generation` / `AgentRuntimeId` and `RuntimeEpochId` from collapsing into a second split-owner identity scheme.
- Ground the mapping in the current runtime code so migration work has a concrete target.

## Two Different Chronologies

The two-kernel architecture needs two different clocks:

- Mob semantic chronology
  - `AgentIdentity`
  - `Generation`
  - `AgentRuntimeId`
  - `FenceToken`
- Meerkat execution chronology
  - `SessionId`
  - `RuntimeEpochId`

They answer different questions.

- `Generation` answers: "which semantic incarnation of this agent is current?"
- `RuntimeEpochId` answers: "which continuous async-ordering domain is this runtime currently executing in?"

These must not be treated as interchangeable.

## Current Code Grounding

Current runtime code already gives `RuntimeEpochId` a narrower meaning than member identity:

- `RuntimeEpochId` is documented as a continuous async-ordering domain; the same session may span multiple epochs, and the same identity may span multiple sessions and epochs.
- `SessionRuntimeBindings` treats `epoch_id` as an epoch-local binding fact consumed by the factory rather than as a public identity handle.
- `MeerkatMachine::recover_or_create_ops_state()` preserves the recovered epoch when a durable ops snapshot exists, and allocates a fresh epoch when no snapshot exists or recovery fails.
- `MeerkatMachine::prepare_bindings()` is idempotent and returns the existing entry's `epoch_id` for the same registered session.

There is also an important current-code ambiguity:

- comments say the epoch rotates on reset or restart without recovery
- the live `MeerkatMachine` reset path does not currently rewrite `RuntimeSessionEntry.epoch_id`

So the architecture should not rely on "reset rotates epoch" until that behavior is made explicit in code.

## Recommended Mapping

The clean relationship is:

- `AgentRuntimeId = (AgentIdentity, Generation)`
  - Mob-owned semantic incarnation key
  - stable across respawn
  - changes on intentional reset
- `(SessionId, RuntimeEpochId)`
  - hidden Meerkat execution binding key
  - names the concrete session-backed runtime and its current async-ordering domain
  - never becomes the Mob-facing incarnation key

The binding chain is therefore:

- `AgentIdentity -> Generation -> AgentRuntimeId -> hidden (SessionId, RuntimeEpochId)`

## Ownership Cut

- `MobMachine` owns:
  - `AgentIdentity`
  - `Generation`
  - `AgentRuntimeId`
  - `FenceToken`
  - the current binding from `AgentRuntimeId` to one live hidden runtime binding
- `MeerkatMachine` owns:
  - `SessionId`
  - `RuntimeEpochId`
  - all state that makes an epoch meaningful: ops lifecycle registry, completion cursors, admitted work, turn execution, wake/drain, comms, and recovery details

## Cardinality And Invariants

- One `AgentRuntimeId` binds to at most one live hidden `(SessionId, RuntimeEpochId)` at a time.
- One live `(SessionId, RuntimeEpochId)` belongs to at most one `AgentRuntimeId` at a time.
- Different `AgentRuntimeId`s must never share the same live epoch binding.
- `Generation` must never be inferred from `SessionId` or `RuntimeEpochId`.
- `RuntimeEpochId` must never be used as public evidence of durable member identity.
- `FenceToken`, not `RuntimeEpochId`, is what fences stale writers within the same `AgentRuntimeId`.

## Transition Mapping

| Transition | `AgentIdentity` | `Generation` | `AgentRuntimeId` | `SessionId` | `RuntimeEpochId` | `FenceToken` |
| --- | --- | --- | --- | --- | --- | --- |
| Fresh provision | same durable name or newly created identity | new or initial | new | new | new | new |
| Durable respawn with successful runtime/session recovery | same | same | same | same | same | new |
| Respawn after unrecoverable runtime loss but without semantic reset | same | same | same | new binding allowed | new | new |
| Intentional member reset | same | advances | new | new binding | new | new |
| Internal Meerkat recycle preserving work | same | same | same | same | same in current code | same |

Notes:

- The third row is the important one. A semantic incarnation can survive a hidden binding replacement without advancing `Generation`.
- If product policy decides that such a degraded respawn is illegal, the transition becomes a failure path instead of a legal rebind. The mapping still shows why `RuntimeEpochId` cannot stand in for semantic incarnation.
- `ResetMember` is the only transition in this table that advances `Generation`.

## What An Epoch Change Means

An epoch change means:

- Meerkat has entered a new execution-ordering domain
- old async-ordering state such as cursor continuity or ops-lifecycle continuity may no longer be trusted as the same domain

An epoch change does not, by itself, mean:

- a new durable identity
- a new semantic incarnation
- a new Mob roster member

That distinction is why the epoch must stay hidden from the Mob seam.

## What A Generation Change Means

A generation change means:

- the previous semantic incarnation is no longer canonical
- checkpoint lineage from the old incarnation is invalid for the new one unless explicitly migrated
- flow and observability history must treat the new incarnation as a replacement, not as a continuation of the old one

This is stronger than an epoch change. Generation is the semantic reset boundary.

## Design Constraints

- Do not expose `RuntimeEpochId` on the Mob-facing contract.
- Do not let Mob flows, roster logic, or operator identity projections depend on epoch churn.
- If a transition should invalidate semantic lineage, model it as `ResetMember` and advance `Generation`.
- If a transition only rebuilds execution plumbing while keeping the same semantic incarnation, keep `Generation` stable and treat any epoch change as hidden Meerkat truth.
- If debugging surfaces expose epoch information, they must label it as runtime-internal continuity, not as agent identity.

## Current Implementation Pressure

Current code already supports most of this mapping:

- recovery preserves epoch when durable ops state is recovered
- repeated `prepare_bindings()` calls preserve epoch for the same registered session
- current mob lifecycle code already reasons about member replacement separately from low-level session mechanics, especially in respawn and disposal paths

The remaining pressure is to make epoch rotation semantics explicit:

- either keep in-place Meerkat `reset()` within the same epoch
- or rotate epoch on in-place `reset()`

Either choice can fit the architecture, but only if:

- the choice remains hidden from `MobMachine`
- `ResetMember` remains the sole semantic generation boundary
- code comments and runtime behavior agree

## Bottom Line

`Generation` is the Mob semantic reset clock.

`RuntimeEpochId` is the Meerkat execution-ordering clock.

They are related through the hidden runtime binding, but they are not substitutes for each other.
