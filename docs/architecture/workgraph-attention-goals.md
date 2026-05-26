---
title: "WorkGraph Attention Goals"
description: "Proposal for Codex-style goals in Meerkat without duplicating WorkGraph or leaking mob semantics into core."
icon: "crosshairs"
---

# WorkGraph Attention Goals

Status: Accepted design; initial WorkGraph, host API, explicit runtime handoff,
tool scoping, and mob-lowering implementation is in progress.

## Summary

Codex-style goals should not become a new durable goal graph in Meerkat. The
durable "what must be true" part is already WorkGraph-shaped: objective,
status, blockers, claims, evidence, readiness, and terminal state.

The missing primitive is attention:

```text
Goal = WorkGraph commitment + runtime attention binding
```

This document defines the goal feature as an optional `meerkat-workgraph`
capability:

- `WorkGraphLifecycleMachine` continues to own durable work item truth.
- A WorkGraph-owned attention binding owns which execution owner should keep
  attending to which work item, in which stance.
- `MeerkatMachine` continues to own actual runtime admission, wake, turn
  execution, and context delivery.
- `meerkat-mob` maps durable `AgentIdentity` bindings to the member's current
  runtime/session binding. Core Meerkat never needs to name `MobId`.

The key invariant:

```text
Work item terminality is not attention state.
Attention state is not runtime admission.
Runtime projection is not WorkGraph truth.
Mob routing is not goal ownership.
```

## Motivation

Codex `/goal` persists a high-level objective and keeps the agent moving by
injecting hidden continuation context whenever the thread becomes idle. That is
useful, but a direct port is wrong for Meerkat.

Meerkat has two important differences:

- session-only operation is first-class;
- most substantial work can be identity-first, mob-shaped, and role-specialized.

A reviewer in a mob should not internalize "ship the match-3 game" as its own
goal. It should see the parent objective as a claim to test and should optimize
for finding reasons the claim is false. Likewise, a coder should build toward a
child work item, and a planner may coordinate the root.

This requires a typed distinction between:

- durable commitment truth;
- the actor currently expected to attend to it;
- the role or stance used when projecting that commitment into a turn.

## Existing Authorities

WorkGraph already owns durable commitment semantics. Its public docs say it
answers what work exists, what is blocked, what is ready, who claimed it, what
evidence supports completion, and what is terminally done, failed, or cancelled.
It owns item lifecycle, dependency legality, readiness, claims, terminal state,
evidence references, event history, and projections.

Runtime already owns session admission and turn execution. `Input::Continuation`
and `WakeIfIdle` already exist as runtime primitives, and
`StartTurnRuntimeSemantics` is the runtime/session carrier for handling mode,
tool overlays, context appends, and turn metadata.

Mob already owns multi-agent topology: roster, stable member identity,
runtime-binding rotation, wiring, flows, and member lifecycle. Public mob APIs
use `AgentIdentity`; per-runtime details may rotate.

Dogma requires a single semantic owner. A side map called "goal bindings" that
decides pause, resume, completion, wake, projection mode, or authority would be
a hidden lifecycle machine. If the binding has semantic state, that state needs
a named authority.

## Decision

Do not introduce `MobGoal`, `ThreadGoal`, or a generic core `GoalLifecycleMachine`.

Instead, add WorkGraph attention as an optional WorkGraph-owned capability:

```rust
struct WorkAttentionBinding {
    binding_id: WorkAttentionBindingId,
    work_ref: WorkItemRef,
    target: WorkAttentionTarget,
    mode: WorkAttentionMode,
    status: WorkAttentionStatus,
    machine_state: WorkAttentionMachineState,
    budget: Option<AttentionBudget>,
    delegated_authority: AttentionDelegatedAuthority,
    projection_policy: AttentionProjectionPolicy,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
```

The binding points at a WorkGraph item. It does not copy the item's title,
status, blockers, evidence, or terminal state.

```rust
struct WorkItemRef {
    realm_id: RealmId,
    namespace: WorkNamespace,
    item_id: WorkItemId,
}
```

The binding target is typed at the surface that owns the identity. The persisted
WorkGraph attention store may normalize to `WorkOwnerKey`, but public callers
must use domain handles:

```rust
enum WorkAttentionTarget {
    Session(SessionId),
    LoweredOwner(WorkOwnerKey),
}
```

`Session(SessionId)` is valid in session/runtime surfaces. Lowered owner keys
are stored only after the owning feature has validated the source identity.
`AgentIdentity` bindings are created through `meerkat-mob`, which validates the
identity and lowers it to a WorkGraph owner key. Core Meerkat does not need an
`AgentIdentity` type and never names `MobId`.

## Attention Modes

Attention mode controls projection and tool authority. It is not WorkGraph item
status.

```rust
enum WorkAttentionMode {
    Pursue,
    Coordinate,
    Review,
    Falsify,
    Judge,
    Observe,
}
```

Mode meanings:

| Mode | Projection |
| --- | --- |
| `Pursue` | Treat this work item as owned work to advance. |
| `Coordinate` | Manage decomposition, routing, and evidence collection. |
| `Review` | Inspect whether the work item or child item satisfies its claim. |
| `Falsify` | Act adversarially; find reasons the claim is not satisfied. |
| `Judge` | Evaluate evidence under an explicit completion policy. |
| `Observe` | Provide context only; do not optimize for or against completion. |

The parent work item can be the same for several agents, but their attention
modes must differ. A reviewer receives the parent objective as a claim to test,
not as a motivational target.

## Attention Status

Attention status answers whether the runtime should continue projecting this
binding. It does not close or block the WorkGraph item.

```rust
enum WorkAttentionStatus {
    Active,
    Paused { until: Option<DateTime<Utc>> },
    Superseded,
    Stopped,
}
```

Terminal WorkGraph item states automatically make attention inactive for
continuation. The WorkGraph item remains the authority for completion,
cancellation, failure, blockers, and evidence.

Pause/resume should be attention-state transitions. They must not be encoded as
labels, descriptions, namespace strings, or prompt text. Timed resume should
target the binding, not the WorkGraph item: either `Paused { until }` becomes
eligible again when the clock passes, or `meerkat-schedule` resumes the binding.
The item itself should not gain a general `snoozed_until` field just to express
one actor's attention preference.

## Machine Ownership

This proposal requires a generated authority path. It should not add a generic
core goal machine.

Preferred shape:

```text
WorkGraphLifecycleMachine
  Owns work item state, readiness, claims, evidence, terminality, and
  completion policy operands.

WorkAttentionLifecycleMachine
  Owns attention binding active/paused/superseded/stopped state and binding
  revision. This is a WorkGraph-owned attention machine, not a generic core
  GoalLifecycleMachine.

workgraph_attention_runtime seam
  Owns the legal handoff from WorkGraph attention to runtime continuation,
  including projection eligibility and fail-closed checks. The current
  implementation exposes this as an explicit public continuation operation;
  automatic idle polling can later call the same seam.

MeerkatMachine
  Owns input admission, WakeIfIdle, active turn state, continuation, and
  delivery of typed context appends.

MobMachine
  Owns AgentIdentity membership and runtime binding resolution.
```

Attention is WorkGraph-owned commitment metadata. Runtime wake is runtime-owned
execution behavior. The generated composition is the seam between them. It must
not be a store-only side table or handwritten reducer. The important dogma line
is not "never create another generated machine"; it is "do not create a second
durable goal authority beside WorkGraph."

## Runtime Handoff

Runtime must treat attention as an observed input, not as its own commitment
truth.

Idle continuation flow:

1. Runtime becomes idle, or a host explicitly requests continuation for an
   attention binding.
2. Runtime or the host-facing runtime surface asks the optional WorkGraph
   attention service for an eligible active binding for this `SessionId`.
3. For each candidate, the service re-reads the WorkGraph item and verifies:
   - item still exists;
   - item is not terminal;
   - attention binding is active;
   - target still resolves to the current session or identity binding;
   - projection policy permits continuation.
4. Runtime enqueues `Input::Continuation` through the normal runtime admission
   path.
5. Runtime starts the turn with typed attention context in
   `StartTurnRuntimeSemantics`.
6. Session service applies the typed context at the turn boundary.

If any check fails, the handoff fails closed. No stale snapshot should wake an
agent.

## Projection Contract

Runtime should not inject raw WorkGraph fields as hidden instructions.
Descriptions, labels, and evidence summaries are user-authored or
agent-authored data, not policy.

The attention service should emit a typed projection:

```rust
struct AttentionContextProjection {
    binding_id: WorkAttentionBindingId,
    work_ref: WorkItemRef,
    mode: WorkAttentionMode,
    item_revision: u64,
    parent_refs: Vec<WorkItemRef>,
    evidence_refs: Vec<WorkEvidenceRef>,
    authority: ProjectedAttentionAuthority,
    text: AttentionProjectionText,
}
```

Projection requirements:

- source truth is named;
- rebuild trigger is item/binding revision;
- staleness policy is explicit;
- role stance is typed;
- max rendered shape is bounded;
- parent objective text is framed as data, not instruction priority.

Example falsifier projection:

```text
Assigned stance: falsify

Work item under test:
  Ship a match-3 game.

Your task:
  Find evidence that this work item is not yet satisfied. Do not optimize for
  release. Report blockers, missing tests, incorrect behavior, and unsafe
  assumptions. You may add evidence to the review item. You may not close the
  parent item unless explicitly granted judge authority.
```

## Completion Authority

Completion is WorkGraph terminal state plus policy. It is not "the model said
complete" unless policy grants that model authority to close the item.

This proposal requires explicit completion policy on the WorkGraph item:

```rust
enum WorkCompletionPolicy {
    SelfAttest,
    HostConfirmed,
    PrincipalConfirmed,
    Supervisor { owner_key: WorkOwnerKey },
    ReviewerQuorum { threshold: u16 },
}
```

The attention binding may carry delegated authority, but delegation is not the
canonical completion policy:

```rust
enum AttentionDelegatedAuthority {
    AddEvidence,
    CloseOwnReviewItem,
    RequestClosure,
    CloseIfPolicyAllows,
}
```

For adversarial roles, the normal authority should be evidence contribution,
not parent closure. A reviewer may close its own review child item and attach
blocking or non-blocking evidence. The parent item closes only when its policy
allows it.

Current WorkGraph host surfaces are read-only. A user-facing `/goal` or host
goal API therefore needs a typed host/principal mutation path. It should not
pretend agent-only WorkGraph tools are sufficient for user-owned goals.

## Session-Only Shape

Session-only Meerkat remains first-class.

Creating a session goal:

1. Host creates a WorkGraph item in a realm namespace such as
   `session/{session_id}`.
2. Host creates an active attention binding targeting `SessionId`.
3. Runtime observes the binding and continues the session while eligible.
4. The session agent can add evidence, block the item, or close it only if the
   authority policy permits.

Without WorkGraph enabled, this feature is unavailable unless a separate
explicit session-local objective contract is designed. There must be no silent
fallback to hidden local goal state, because that would create two goal systems.

## Mob Shape

There is no mob-bound goal lifecycle.

A mob objective is a WorkGraph root item, often scoped by namespace or linked
through a typed external reference. Mobs route attention; they do not own work
terminality.

Example:

```text
Root WorkGraph item:
  Ship a match-3 game.

Planner attention:
  target = AgentIdentity(planner)
  work = root item
  mode = Coordinate

Coder attention:
  target = AgentIdentity(coder)
  work = child implementation item
  mode = Pursue

Reviewer attention:
  target = AgentIdentity(reviewer)
  work = child implementation item plus parent context
  mode = Falsify
```

`meerkat-mob` owns resolution from `AgentIdentity` to the member's current
runtime/session binding. If a member respawns, the binding follows the durable
identity. If the identity no longer resolves, runtime continuation fails closed
until the mob restores or reassigns the binding.

Mob docs should avoid saying the persisted mob owns "work" in the WorkGraph
sense. Mob owns roster, wiring, member lifecycle, flows, and routing. WorkGraph
owns shared durable work.

## Tool Authority

Current WorkGraph exposure is coarse: enabling WorkGraph gives a member create,
claim, update, block, close, link, and evidence tools. That is too broad for
adversarial roles.

Attention projection should narrow tool authority by mode and work item:

| Mode | Typical authority |
| --- | --- |
| `Pursue` | Claim/update assigned item; add evidence; close if policy allows. |
| `Coordinate` | Create/link child items; assign attention; collect evidence. |
| `Review` | Read target/ancestors; add evidence; close review child item. |
| `Falsify` | Read target/ancestors; add blocking evidence; cannot close parent. |
| `Judge` | Evaluate evidence; close target only under explicit policy. |
| `Observe` | Read-only context. |

Attention-aware filtering should be the final capability-narrowing layer:

```text
base tool registry
  -> ToolCategoryOverride / session config
  -> mob profile capabilities
  -> attention projection scope
  -> per-call WorkGraph service authorization
```

`ToolCategoryOverride` remains coarse-grained capability exposure. Mob profiles
remain role defaults. The final authorization check must know the attention
binding id, mode, work item scope, and requested operation before a WorkGraph
mutation reaches `WorkGraphService`.

## Public Surfaces

Suggested user-facing vocabulary:

- `/goal <objective>`: create WorkGraph item plus session attention binding.
- `/goal pause`: pause attention binding, not WorkGraph item.
- `/goal resume`: reactivate attention binding if item is still non-terminal.
- `/goal clear`: stop/supersede attention binding; item remains unless user
  explicitly cancels it.
- `/goal status`: show attention status plus referenced WorkGraph item status.

Suggested minimal host APIs:

- `workgraph/goal/create`: transactionally create or reference a WorkGraph item
  and create the initial attention binding.
- `workgraph/goal/status`: return referenced item status plus attention status.
- `workgraph/goal/confirm`: attach host/principal confirmation evidence.
- `workgraph/goal/request_close`: request policy-gated item closure.
- `workgraph/attention/list`: list active attention bindings.
- `workgraph/attention/pause`: pause a binding without mutating the item.
- `workgraph/attention/resume`: resume a paused binding if the item remains
  non-terminal.
- `workgraph/attention/continue`: enqueue a hidden continuation for a
  session-bound binding.

These are WorkGraph-feature APIs. They are unavailable when WorkGraph is not
compiled or enabled.

The CLI mirrors the narrow session-first subset under `rkat workgraph`:
`goal-create`, `goal-status`, `goal-confirm`, `goal-close`, `attention-list`,
`attention-pause`, and `attention-resume`. The CLI intentionally does not
expose the broad WorkGraph mutation surface as the goal UX.

Core host goal creation targets a session handle. Mob/agent targets are not
accepted through this core host contract; `meerkat-mob` must validate
`AgentIdentity` and lower it to a WorkGraph owner binding through its own
feature surface. This keeps core Meerkat from freezing a public
`WorkOwnerKey::Agent` or `WorkOwnerKey::Mob` bypass.

## Failure Modes

The design must fail closed when:

- WorkGraph is disabled;
- WorkGraph item is missing;
- item revision changed in a way that invalidates the projection;
- item is terminal;
- attention binding is paused/stopped/superseded;
- target identity no longer resolves;
- runtime is not idle;
- queued user or peer work has priority;
- projection cannot be built safely;
- authority policy would allow self-review of a parent item.

## Non-Goals

- Do not add `MobGoal`.
- Do not add a core `GoalLifecycleMachine` duplicating WorkGraph.
- Do not make WorkGraph readiness equivalent to runtime wakeup.
- Do not use namespace strings as semantic proof of session, mob, or agent
  ownership.
- Do not inject a parent objective as every member's own goal.
- Do not let reviewer/adversarial profiles depend only on prompt text for their
  stance.
- Do not provide a local fallback goal table when WorkGraph is disabled.

## Resolved Recommendations

1. Model attention as WorkGraph-owned state with
   `WorkAttentionLifecycleMachine`, plus a generated
   `workgraph_attention_runtime` composition. Do not add a generic core goal
   machine.
2. Model pause/resume on the attention binding. Timed pauses may use
   `Paused { until }` or schedule-driven resume, but should not mutate item
   readiness.
3. Expose a narrow host/principal mutation surface for goal creation,
   pause/resume/stop/reassign, status, confirmation evidence, and policy-gated
   closure. Do not expose a broad WorkGraph editor as the first goal surface.
4. Put completion policy on the WorkGraph item. Let attention bindings carry
   delegated authority only.
5. Apply attention-aware tool filtering after session config, category
   overrides, and mob profile tooling, with a final per-call WorkGraph
   authorization check.

## Acceptance Bar

The proposal is acceptable only if:

- WorkGraph remains the only owner of durable work truth.
- Runtime remains the only owner of turn admission and continuation.
- Attention binding lifecycle is WorkGraph-owned and generated, currently via
  `WorkAttentionLifecycleMachine`.
- The WorkGraph/runtime handoff has one named seam that revalidates attention
  eligibility and enters runtime through normal continuation admission.
- Mob remains optional and feature-owned.
- Session-only operation remains first-class.
- Adversarial roles receive typed falsification/review projections, not the
  parent objective as their own motivational goal.
