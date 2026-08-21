---
title: "Member Tool Policy v1"
description: "Proposal for durable per-member tool materialization, conjunctive execution constraints, and an early consequence-narrowing seam."
icon: "shield-halved"
---

# Proposal: Member Tool Policy v1

Status: Proposed for Meerkat 0.8.26
Baseline: Meerkat `b1f1a9458deb55d4398b894f9b06bdc8695ee832`
Scope: Meerkat tool construction, mob desired state, runtime dispatch, public contracts, and downstream policy-provider composition

## Decision

Meerkat should finish its existing per-member tool-policy path. It should not
build a tool-specific authorization platform.

This is the 0.8.26 design. `v1` names the first member-tool policy contract,
not an earlier Meerkat release.

V1 introduces one durable `MemberToolDeclaration` with two deliberately
separate parts:

1. **Materialization intent** says which Meerkat tool categories are composed
   into this member's executor. It answers whether `shell`, `schedule`,
   `workgraph`, `memory`, `mob`, `comms`, image generation, or web search exist
   for the member.
2. **Execution constraints** say which calls may cross the final dispatcher
   boundary. They compose allow-lists, deny-lists, and consequence ceilings
   conjunctively. No constraint can erase another.

The declaration belongs to the stable member's existing desired identity
intent. A compare-and-set update converges by rematerializing the member's
executor under the same `AgentIdentity`, profile, comms binding, and session.
Changing tool capability must not require changing the member's profile.

V1 also adds a small, bounded application-policy hook at the existing
outermost dispatch gate. The hook may only narrow the result of Meerkat's
static constraints. It evaluates a validated in-memory snapshot and cannot
perform Python, RPC, network, filesystem, or other blocking IO on the dispatch
path. This slice is delivered first so applications can close a live fail-open
without waiting for desired-state convergence work.

V1 does not add `ToolInvocationMachine`, durable suspended tool execution, or
automatic execution after approval. Ordinary Meerkat and MCP tool calls are
stateless requests. If a call is interrupted, Meerkat does not resume it. If
an external effect may have happened, the outcome is uncertain and the agent
decides whether to call again.

If Meerkat later grows a complete authorization and approval platform, that
platform must govern generic actions rather than tools alone. It requires a
separate proposal.

## 1. Current truth

The motivating HomeCore proposal identifies a real product problem, but its
original Meerkat baseline is stale.

| Fact | Current Meerkat | Remaining gap |
|---|---|---|
| Per-member call policy | `mob/spawn` and `SpawnMemberSpec` carry `tool_access_policy` | Declarative desired-member mutation does not expose a complete update path |
| Exact-name constraints | `ToolAccessPolicy` supports `AllowList` and `DenyList` | Constraints are alternatives rather than a conjunction |
| Read-only consequence ceiling | `ToolAccessPolicy::ReadOnly` is enforced from dispatcher-owned mutation declarations | A read-only profile currently replaces a narrower name policy |
| Dispatch enforcement | `ExecutionPolicyGatedDispatcher` wraps the complete dispatcher outermost | Application consequence policy is not attachable at that seam |
| Persistence | Resolved policy is stored in session tooling, and `DesiredMemberOverlay` can carry resolved policy | Ordinary respawn and public declarative paths do not consistently recompile from desired intent |
| Category materialization | `AgentBuildConfig`, `SessionTooling`, and the portable desired profile already carry typed category facts | Public member mutation does not expose those facts as a focused declaration |
| Callback tool materialization | Desired member material can persist exact callback definitions while handlers remain process-local | Public member mutation does not expose a focused exact-set update |
| Approval | `ApprovalService` and `ApprovalLifecycleMachine` already exist | Tool dispatch is not composed with approval, and a full solution is not tool-specific |
| Interrupted call recovery | A dangling `tool_use` is held as ambiguous | The agent, not recovery code, must decide whether to issue another call |

`MobMemberSpecWire` is intentionally a reduced create-if-absent shape.
`ensure_member` returns an existing member unchanged, and roster reconciliation
retains present identities. Adding one field to that wire type would improve
first-create parity but would not make an update reach an existing member.

The latent desired-state substrate is already better than that reduced wire
shape. `IdentityIntentRecord -> DesiredMemberSpec -> DesiredMemberOverlay`
already names a stable identity, intent revision, intent digest, tombstone,
session target, and resolved tool policy. V1 activates that substrate through
a public revisioned member-tool update. It does not invent a second member
store or authority.

## 2. The problems v1 solves

### 2.1 Capability changes are incorrectly coupled to profile changes

A mob profile participates in the durable member binding: `MemberCommsName` is
`{mob_id}/{role}/{member}`, and the role is the profile name used at
construction. An undeclared reassignment therefore changes comms identity and
correctly fails resume. Meerkat 0.8.25 added an explicit, one-shot
`resume_from_role` migration for a real administrative role change; that
authority does not turn role migration into a capability-update mechanism.

Applications have nevertheless used profile reassignment as a capability
update mechanism. That is the wrong operation. A profile is a role and
construction template, not a mutable bag of grants.

V1 makes member tool materialization and execution policy independently
mutable while profile and comms identity remain fixed. Giving an existing
member `shell`, or removing `schedule`, becomes a member-tool declaration
update. An actual role/profile change remains an identity migration and is
outside this proposal, using the existing explicit migration contract.

### 2.2 Current policy composition can widen by accident

Current profile construction selects one `ToolAccessPolicy`. In particular,
`profile.tools.read_only = true` replaces a member `AllowList` or `DenyList`
with `ReadOnly`.

That preserves the no-mutation ceiling, but it can widen a narrow allow-list
to every dispatcher-declared read-only tool. V1 composes both facts:

```text
effective_call_permission =
    every_name_constraint_admits(tool_name)
    AND every_consequence_constraint_admits(mutation_class)
    AND application_narrowing_policy_admits(request)
```

No precedence rule may discard a constraint.

### 2.3 Consequence labels do not currently govern dispatch

HomeCore classifies tools into risk tiers and has an approval UX, but its
callback dispatch path does not consult those facts. A high-risk label can
therefore be true in configuration and irrelevant at execution.

The immediate platform defect is not missing approval state. It is the lack of
an attachable, authoritative narrowing check immediately before tool IO.

V1 supplies that seam first. It does not require the rest of this proposal to
land before applications can deny a risky call.

### 2.4 Provider-native tools bypass the dispatcher

Provider-native web search, code execution, computer use, and similar server
tools do not traverse `AgentToolDispatcher`. The existing execution gate
cannot govern them.

V1 makes that boundary explicit. A member governed by a restrictive member
policy must disable provider-native tool capabilities unless the provider
feature later supplies a typed enforcement seam with equivalent guarantees.
This is a declared capability loss, never an implicit runtime surprise.

## 3. Goals

V1 must provide:

1. Durable per-member tool materialization without changing profile or comms
   identity.
2. Durable per-member execution constraints that survive restart, resume, and
   fresh respawn.
3. Revisioned compare-and-set updates for existing members.
4. One compiler for profile defaults, member declarations, parent delegation
   ceilings, and the existing session execution policy.
5. Conjunctive constraint composition with deny-overrides-allow behavior.
6. One outermost call gate shared by every dispatcher-backed surface.
7. An early in-process application-policy hook that can narrow but never
   widen Meerkat's result.
8. A finite evaluation and member-convergence contract, with typed failure
   rather than silent hangs.
9. Explicit operational behavior for provider unavailability and
   provider-native bypass.
10. Exact migration parity against downstream effective behavior, including
   hardcoded filters.
11. No automatic retry or resume of an ordinary tool call.

## 4. Non-goals

V1 does not:

- Define household roles, tool assignments, risk tiers, approver routing, or
  product UI.
- Turn model visibility into an authorization boundary.
- Make profile reassignment a supported capability-change operation.
- Persist callback closures, clients, credentials, MCP transports, or other
  process-local implementation material.
- Invent durable state for an ordinary tool handler.
- Resume, replay, or automatically retry a call after process loss.
- Promise exactly-once external effects.
- Build a general policy language.
- Build a tool-specific approval or authorization platform.
- Allow a caller to supply the risk or consequence class that governs itself.
- Treat deployment activation identity as policy content identity.

## 5. Semantic facts and owners

| Fact | Canonical owner | Important non-owner |
|---|---|---|
| Member role/profile and comms identity | Existing `MobMemberBinding` and `MobMachine` member lifecycle | Tool policy |
| Profile tool-category defaults | Mob `Profile` | Session resume code |
| Desired member materialization and constraints | Existing sealed `IdentityIntentRecord` desired member material | `MobMemberSpecWire`, roster projection |
| Intent revision and convergence | Existing identity-intent store contract plus generated `MobMachine` identity reconciliation path | Tool-only reconciler, application config file |
| Effective executor materialization | `AgentFactory` composition from the resolved declaration | Surface or SDK |
| Effective static execution constraint | One tool-policy compiler and the outermost dispatcher gate | Visibility filter |
| Model-visible tool set | Existing `ToolScope` authority | Execution policy |
| Application consequence classification | Injected application policy provider | Caller arguments, tool implementation |
| Provider-native capability on an effective request | Final typed request sanitizer at the facade/provider seam | Activation-time check |
| Approval lifecycle | Existing `ApprovalLifecycleMachine` and `ApprovalService` | MobKit pending map or tool dispatcher |
| Ordinary tool execution | Stateless dispatcher request and result | A durable invocation machine |
| Detached operation lifecycle | Existing owning operation or job authority | The call that returned its handle |
| Ambiguous post-effect recovery | Existing session-document durable-tail authority | Automatic retry loop |

The stable declaration survives respawn, so it belongs to desired member
identity. The live executor is disposable, so it is recomposed from that
declaration. Session metadata is a resume projection of the resolved
declaration, not a rival desired owner.

## 6. Public member-tool contract

### 6.1 Declaration shape

The public domain shape is conceptually:

```rust
struct MemberToolDeclaration {
    category_overrides: ToolCategoryOverrides,
    callback_tools: CallbackToolSetDeclaration,
    execution: ToolAccessDeclaration,
    application_policy: ApplicationToolPolicyBinding,
}

struct ToolCategoryOverrides {
    builtins: ToolCategoryOverride,
    shell: ToolCategoryOverride,
    comms: ToolCategoryOverride,
    mob: ToolCategoryOverride,
    memory: ToolCategoryOverride,
    schedule: ToolCategoryOverride,
    workgraph: ToolCategoryOverride,
    image_generation: ToolCategoryOverride,
    web_search: ToolCategoryOverride,
}

enum CallbackToolSetDeclaration {
    Inherit,
    Set(Vec<DesiredLocalCallbackTool>),
}

enum ToolAccessDeclaration {
    Inherit,
    Unrestricted,
    Constraints(Vec<ToolAccessConstraint>),
}

enum ToolAccessConstraint {
    AllowNames(ToolNameSet),
    DenyNames(ToolNameSet),
    ReadOnly,
}

enum ApplicationToolPolicyBinding {
    Unmanaged,
    Inherit,
    Provider {
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
    },
}

struct PolicyEvaluationProvenance {
    revision: PolicyRevision,
    digest: PolicyDigest,
}
```

This is domain vocabulary. Wire types are generated from it or lower into it.
`Option<T>` must not make absence mean both inherit and unrestricted.

`ToolCategoryOverride` already carries the required `Inherit`, `Enable`, and
`Disable` distinction. An enabled category still needs its normal injected
runtime authority. For example, enabling mob operator tools without a valid
`MobToolAuthorityContext` fails typed. A category flag cannot mint operator
authority.

Callback materialization reuses the existing split in desired member material:
the exact name, description, and input schema are durable, while the executable
handler and callback scope remain process-local. Materialization fails typed if
the current host cannot bind every required callback definition. A persisted
definition never becomes an executable closure.

`Unrestricted` is explicit. `DenyNames` is intentionally open-world and may
admit a newly materialized name that is not denied. Callers that require a
closed set use `AllowNames`. V1 does not add a catalog snapshot machine to
make an explicitly open-world declaration look closed.

`Constraints` must contain at least one constraint. An empty vector is rejected
rather than becoming a second spelling of `Unrestricted`.

The member declaration binds the stable identity of the application policy,
not one content revision. The injected provider owns the active immutable
snapshot for each `PolicyId`. Every evaluation reports the exact revision and
digest it used as `PolicyEvaluationProvenance`. Snapshot replacement is an
atomic provider operation, so a routine policy edit does not require a CAS
update and rematerialization of every governed member. Missing policy identity,
an incoherent snapshot, or absent provenance fails closed.

This is an ownership distinction, not an availability shortcut. The member
declaration owns which policy governs the member. The provider owns the current
content of that policy. Static member constraints remain the durable ceiling,
so an application-policy update can never widen past Meerkat's constraints.

The provider registry is host-scoped and injected through the normal
`MobBuilder` and `AgentFactory` composition path. Every governed executor on one
host shares that registry and its atomic snapshot pointer. Multi-host rollout is
not globally atomic: each host reports the revision and digest it actually
used, and a member refuses readiness on a host that lacks its exact provider and
policy binding. Cross-host rollout ordering belongs to the application's
deployment authority, not to a hidden Meerkat consensus protocol.

`ToolCategoryOverrides` is an aggregate carrier, not a claim that all
categories share implementation semantics. Each field lowers through its
existing category owner. Some categories compose dispatchers, some govern
visibility, and provider-native categories require final request sanitization.

### 6.2 Constraint composition

One compiler resolves declarations in this order:

1. Profile category and callback defaults plus profile constraints.
2. Administrative member declaration.
3. Parent delegation ceiling.
4. Existing session execution policy.

Profile and member materialization are template composition. An authenticated
member declaration may enable or disable a category. Parent delegation and
the existing session execution policy can only narrow what the administrator
declared. V1 adds no turn-level execution-policy owner.

Execution constraints always form an intersection:

- Every `AllowNames` constraint must contain the name.
- Any matching `DenyNames` constraint denies.
- Every `ReadOnly` constraint requires the owning dispatcher to declare the
  tool read-only.
- Unknown mutation class fails a `ReadOnly` constraint.
- Application policy runs only after static constraints admit the call.
- Application policy may deny an admitted call but cannot admit a statically
  denied call.

The compiler emits a normalized resolved constraint set plus source
provenance. `Inherit` is forbidden in resolved desired material and at the
dispatch seam.

### 6.3 Tool identity in v1

V1 retains Meerkat's existing exact `ToolName` selector contract. It does not
pretend that a name is a universal authorization resource identity.

Dispatcher composition already owns which live implementation wins a name.
HomeCore's MCP names include their server namespace. Replacing a same-name
implementation remains governed by dispatcher catalog and collision rules.

A future cross-resource authorization platform will need typed resource
provenance and action identity. That requirement does not justify creating a
new durable tool-binding catalog for static per-member allow-lists in v1.

## 7. Updating an existing member

### 7.1 Revisioned apply

Expose one shared command across Rust, RPC, REST where applicable, and
generated SDKs:

```rust
ApplyMemberToolDeclaration {
    mob_id: MobId,
    agent_identity: AgentIdentity,
    request_id: MemberToolMutationId,
    expected_intent_revision: IntentRevision,
    declaration: MemberToolDeclaration,
    convergence: IdentityConvergenceMode,
}

enum IdentityConvergenceMode {
    Drain { max_wait: Duration },
    CancelActive,
}
```

The command updates the tool portion of the existing `IdentityIntentRecord`.
It does not create a second member-policy row.

The `MobHandle` form obtains `mob_id` from the handle. Wire forms carry it
explicitly. `AgentIdentity` is never interpreted outside that mob scope.

`request_id` is an idempotency identity recorded in a new, typed
`IdentityIntentMutationReceipt`. It may share the existing identity store
transaction and physical receipt table, but it does not reuse
`IdentityOperationReceipt`: those receipts are actuator lost-ACK custody for
session creation, retirement, external binding, and initial delivery, not a
generic mutation ledger. The store atomically compares the desired revision,
writes the new sealed intent when admitted, and records the mutation outcome.
No crash may commit one without the other. Repeating the same request id and
payload returns the recorded result. Reusing it with different payload is a
typed conflict.

The response separates the idempotent mutation result from the live
convergence projection:

```rust
struct ApplyMemberToolDeclarationResult {
    commit: MemberToolCommitOutcome,
    convergence: IdentityConvergenceStatus,
}
```

Typed commit outcomes are:

- `Committed { desired_revision }`
- `NoChange { desired_revision }`
- `RevisionConflict { expected, actual }`
- `RequestConflict { request_id }`
- `MemberAbsent`
- `InvalidDeclaration(reason)`

An omitted precondition is not allowed for mutation. Read-modify-write clients
must state which desired revision they observed.

Desired commit and live convergence are different facts. The idempotency
receipt records only `MemberToolCommitOutcome`. Repeating the same request id
and payload returns that recorded commit outcome plus a fresh existing
`IdentityConvergenceStatus`; it never freezes an old convergence projection
into the mutation receipt. A successful CAS remains `Committed` even if later
rematerialization fails. The convergence projection carries the desired intent
revision and the fresh machine-derived identity reconciliation condition. The
active realization observation also carries the exact intent revision and
material digest from which the current executor was built, so callers can
distinguish desired, active, and divergent state without a parallel tool-only
status authority.

`Drain.max_wait` is required, non-zero, and bounded by the platform contract.
The existing identity reconciliation vocabulary gains the exact typed facts it
needs for an authorized material update: admission closed, draining, drain
deadline exceeded, replacement failed, and active intent revision. Deadline
expiry derives a specific `DrainBlocked` convergence condition while an active
turn or call still prevents retirement. New turns remain closed. The old
executor is not silently re-opened under stale policy. If the blocking turn
later ends, level-triggered reconciliation observes that the obstacle is gone
and continues automatically. An operator may instead use a focused identity
convergence command to continue draining under a new finite deadline or
explicitly choose `CancelActive`, always against the exact desired and active
revisions. Binding, construction, or persistence errors derive
`RepairBlocked` or the existing typed failure projection; they are not
mislabeled as deadline expiry.

That command is a separate idempotent `ResolveIdentityConvergenceBlock`
operation carrying its own request id, exact desired revision, observed active
revision, and `IdentityConvergenceMode`. Its name and authority are identity
generic even though member-tool mutation is its first caller. It changes no
desired declaration and feeds the same generated identity classifier.

The generated `MobMachine` identity reconciliation path owns the drain
deadline, admission closure, and replacement obligations. The existing
`ClassifyIdentityReconciliation` decision remains the one total classifier;
member-tool mutation extends its typed observations and decisions rather than
creating a second tool-policy reconciler. Shell code supplies trusted time and
realization observations and realizes retire or cancel effects; it does not
infer convergence from a local timer, waiter, or mutation receipt.

### 7.2 Convergence by rematerialization

V1 does not hot-edit an arbitrary running dispatcher. It uses the existing
identity lifecycle to replace disposable execution material:

1. Validate the candidate declaration and preflight the current target host's
   callback, category, and provider bindings without changing desired or active
   state.
2. Atomically compare-and-set the sealed `IdentityIntentRecord` and its
   `IdentityIntentMutationReceipt`.
3. Observe desired intent revision or material digest diverging from the active
   realization and let `MobMachine` stop admission of new turns for that member
   before any fallible replacement work.
4. Let the active turn drain until its declared deadline, or explicitly cancel
   it when the caller requested an emergency revocation.
5. Retire the old runtime binding.
6. Rebuild through `AgentFactory` from the latest desired member intent.
7. Resume the same session under the same `AgentIdentity`, profile, member
   binding, and comms name with a new runtime incarnation and fence.
8. Report convergence only after the new executor is active.

Preflight failure rejects the mutation before the CAS, so the old desired and
active executor remain coherent. Once the CAS commits, every later failure is a
desired-active divergence and admission remains closed. A crash between the CAS
and the first live reconciliation step is not a special case: restart scans the
sealed desired intent, observes the active revision or digest mismatch, and
re-enters the same level-triggered identity reconciliation path. Neither the
mutation receipt nor an application retry grants convergence authority.

The dedicated member declaration sets the existing category-specific resume
override bits and `tool_access_policy` bit. A generic
`ResumeOverrideField::Tooling` or `ResumeOverrideField::Tools` is not added
because it would collapse
materialization, visibility, execution constraints, and consequence policy.

All respawn, revival, ensure, reconcile, helper, flow-provisioning, and placed
member paths must lower through the same desired-member compiler. A
process-local spawn customizer cannot replace the durable declaration.

### 7.3 Revocation semantics

A policy update does not retroactively stop IO already admitted by the old
executor. The old runtime stops accepting new turns before replacement. A
normal update drains the current turn. An emergency update explicitly cancels
it and accepts the ordinary uncertainty rules for any external call already in
flight.

A normal update does not wait forever. If its drain deadline expires, the
desired revision remains committed, convergence becomes
the machine-derived `DrainBlocked` condition, and new turns stay closed until
convergence is resumed or cancellation is explicitly authorized. A timeout
never silently turns a normal update into force-cancel.

The new policy is effective when the replacement executor becomes active.
Until then, the existing `IdentityConvergenceStatus` reports the exact pending
or divergent identity condition rather than falsely reporting convergence. A
rematerialization failure leaves the desired revision committed and reports
the typed repair-blocking condition with the last active revision.

This is deliberately weaker than instantaneous mid-call revocation and much
simpler than a per-invocation transaction machine. V1 has no requirement that
justifies the latter.

## 8. The early application narrowing seam

### 8.1 Contract

Before the desired-state work lands, add an optional provider to the existing
outermost execution gate:

```rust
trait ToolConsequenceNarrowingPolicy: Send + Sync {
    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure>;
}

trait ToolConsequencePolicySnapshot: Send + Sync {
    fn provenance(&self) -> PolicyEvaluationProvenance;

    fn evaluate(
        &self,
        request: &ToolConsequenceRequest,
    ) -> ToolConsequenceVerdict;
}

enum ToolConsequenceVerdict {
    Allow,
    Deny(ToolConsequenceDenial),
    Indeterminate(ToolConsequenceFailure),
}
```

For a provider-bound declaration, the request requires the canonical
`MobMemberBinding`, exact tool name, the gate's immutable `ToolCallView`, run
and tool-call correlation, and the exact provider id and policy id. Member
identity is optional only for an explicitly `Unmanaged`
standalone session, in which case this provider is not called. It does not
carry a caller-authored risk tier as authority.

The hook runs in process after static constraints and immediately before the
inner dispatcher. `snapshot` is an atomic, non-blocking read of the provider's
current immutable snapshot. The gate captures that snapshot, its provenance,
and its provider generation once for the call. `evaluate` is contractually a
pure, non-blocking operation over those captured bytes. The gate constructs an
owned, closed `ToolConsequenceRequest` before supervisor admission; worker code
never retains a borrow into a live dispatcher stack or a mutable call object.

That contract is still defended mechanically. A host-scoped
`PolicyEvaluationSupervisor` owns a fixed number of provider partitions, each
with a fixed worker set and bounded admission queue. The provider registry and
partition assignment are bounded at host construction, so one provider cannot
consume another provider's reserved evaluation capacity. Evaluation uses
those workers with a finite deadline; it never creates one `spawn_blocking`
task or one operating-system thread per call.

The deadline can return even when one evaluator implementation never returns.
A wedged evaluator may permanently consume its provider partition's worker
until process restart, but the fixed partition and queue capacities bound that
loss. A mechanical timeout marks that provider partition unhealthy for the
rest of the process; snapshot replacement cannot pretend to reclaim a stuck
thread. A panic is caught at the same boundary, reported typed, and marks that
partition unhealthy. Once a provider generation is semantically unhealthy,
mechanically unhealthy, or saturated, later governed calls fail fast as
`policy_indeterminate` instead of allocating more tasks or threads. Installing
a newly validated snapshot creates a new provider generation and may recover
a semantic snapshot failure only while its mechanical partition remains
healthy. Any late result from an old generation is fenced and cannot affect the
new generation's health or verdicts.

`Deny` produces an ordinary `policy_denied` tool result and lets the run
continue. `Indeterminate`, including deadline expiry, produces a typed
run-terminal `policy_indeterminate` condition. Both perform zero
inner-dispatch calls. They remain distinct because one is a policy verdict the
agent may reason around and the other is an authority or mechanism failure that
the model must not retry within the same run.

The dispatcher contract remains `Result<ToolDispatchOutcome, ToolError>`.
V1 adds distinct closed errors for policy denial and policy indeterminacy.
Model-driven collection must special-case `ToolError::PolicyIndeterminate`
before the existing generic conversion of dispatcher errors into ordinary tool
results, lower it to `AgentError::PolicyIndeterminate`, and route it through
the existing canonical fatal run-terminalization path. Direct external
dispatch returns that same typed fatal error without inventing a synthetic run.
`ToolError::PolicyDenied` continues through the ordinary tool-result path. No
surface may flatten the indeterminate variant back into text and feed it to the
model.

Each denial or indeterminate outcome emits a typed observation carrying member,
provider and policy identity, snapshot provenance when available, tool name,
arguments digest, run id, and tool-call id. This is a rebuildable operational
projection, not an execution receipt or permission. It lets a host detect
repeated identical denials without adding a policy-specific retry authority.

The provider may consult an immutable application-owned policy snapshot. It
may reuse an existing product gating engine. It cannot:

- Widen the static Meerkat result.
- Change tool visibility.
- Mutate member desired state.
- Invent approval lifecycle state.
- Suspend or resume a tool handler.
- Trust consequence or risk supplied in tool arguments.
- Perform IO or call back into a Python or remote application host.

MobKit's `f596e90f` correction makes a configured risk tier override a
conflicting caller tier for known actions, but it is not the enforcement seam:
an action absent from that table still retains the caller's tier. The v1
provider ignores caller-authored tier data entirely and classifies every
governed call from its captured snapshot, with unknown actions failing closed.

This seam closes the dispatch fail-open without claiming to be a complete
authorization system.

MobKit may reuse mechanics from its existing access engine when compiling the
gateway's immutable snapshot, but it cannot simply rebind that engine and call
the result done. The current access model treats authenticated operators as
principals, agents as resources, disabled enforcement as allow, and configured
admins as unconditional bypasses. A member-to-tool consequence policy needs a
closed tool action/resource vocabulary, default deny, and no disabled or admin
escape. That is provider composition, not a second member-policy owner. MobKit
must not persist a rival per-member grant and stamp it into one spawn path.

### 8.2 Compilation input and vocabulary ownership

Phase 0 consumes one closed, typed `CompiledApplicationToolPolicy` artifact.
It contains the provider id, policy id, monotonic policy revision, canonical
policy digest, closed action and resource vocabulary, resolved consequence
classifications, and default-deny behavior required by the evaluator. The
digest covers canonical compiled bytes, not a source file path or deployment
activation id. A source-bundle digest may be retained separately as
provenance.

The product host owns source policy semantics and vocabulary mapping. For the
first HomeCore adoption, the source of truth is its immutable
`shared/data/policy/bundle.N` bundle containing `policy.json` and
`compiled-gating.toml`, addressed as `hc-policy-v1:sha256:<digest>`, or a
versioned successor with the same ownership properties. HomeCore's compiler
must map that product vocabulary into the closed typed artifact and reject
unknown actions, ambiguous resource identities, duplicate conflicting rules,
and non-canonical content.

MobKit owns strict parsing and mechanical lowering of that compiled artifact
into the Meerkat snapshot contract. It must validate the digest and schema, but
it cannot infer missing grants, reinterpret HomeCore vocabulary, or retain a
rival policy store. Meerkat owns the snapshot contract, provider-generation
fencing, and enforcement. This division names the Phase 0 starting artifact
without making HomeCore's current file format a Meerkat platform contract.

### 8.3 Availability

Fail-closed consequence policy is correct, but a network dependency on every
tool call would be an avoidable household-wide availability hazard.

Meerkat's provider contract requires:

- An in-process evaluator bound to the exact provider id and policy id.
- A validated immutable policy snapshot whose revision and digest are reported
  on every evaluation.
- Binding validation at construction before a governed member becomes ready.
- Typed `policy_indeterminate` when the required binding is absent,
  inconsistent, exceeds its evaluation deadline, or fails evaluation.
- A run-terminal indeterminate path so the model cannot turn provider failure
  into an in-run retry or route-around loop.
- Failure scoped to calls bound to that provider, not unrelated members or
  ungoverned tool paths.

The provider seam owns its current snapshot pointer and semantic health. The
snapshot pointer must reject any attempted replacement whose policy revision
is lower than the currently installed revision. That provider-owned rollback
check is distinct from Meerkat's provider-generation fencing: revision
monotonicity protects policy content within one provider identity, while
generation fencing prevents late work from an old provider instance from
publishing a verdict after replacement.

The evaluation supervisor owns only bounded provider partitions, mechanical worker
and queue capacity, and mechanical health. The
dispatcher wrapper reports timeout and saturation observations and consumes
the provider generation's fail-fast health result; none of these layers keeps
a rival policy or availability flag that can disagree with the provider.

Meerkat does not fail open to preserve availability. The availability design
keeps evaluation local and prevalidated so the fail-closed path is exceptional
and typed. Snapshot replacement, last-valid retention, application health, and
operator notification belong to the injected provider and its host. Meerkat
projects typed evaluation provenance, denials, and provider failures so that
host health cannot honestly remain green and repeated denials are visible, but
it does not invent an application deployment authority.

### 8.4 Provider-native capabilities

The narrowing provider only covers `AgentToolDispatcher` calls. V1 compiles a
narrow-only `SessionLlmRequestPolicy` from the resolved member declaration. The
typed provider-parameter owner applies it at the end of
`Agent::prepare_calling_llm_request`, after effective provider parameters,
defaults, pre-LLM hooks, extraction, model fallback, and retry composition have
all run, on every attempt. The sanitizer disables provider-native capabilities
unless the provider feature supplies an equivalent typed enforcement seam.

Activation certification is useful evidence, but it is not the enforcement
boundary. Resume, fallback, retry, or an explicit request parameter must not
re-enable a native tool after certification.

The sanitizer composes with existing feature materialization. It does not
duplicate or replace MobKit's image-generation machine gate, Meerkat category
flags, or model capability checks. Those owners decide whether the feature
exists; the final sanitizer can only remove an otherwise available
provider-native capability from the effective request.

Downstream adoption reports the exact lost capabilities. In HomeCore this
includes profiles where `web_search = inherit` currently enables native search
for `gpt-5.6-sol`. Certification must surface that change before activation.

## 9. Stateless calls, approval, and recovery

### 9.1 Ordinary calls are not resumed

An ordinary tool call is one stateless request to a dispatcher. Meerkat does
not persist or resume the handler's internal execution.

If Meerkat crashes before dispatch, no effect occurred. If it crashes after
the external effect may have occurred but before a result was committed, the
outcome is uncertain. Existing durable-tail recovery holds a dangling
`tool_use` as ambiguous and never synthesizes a result or replays the call.

The agent decides whether to call again using available evidence, tool
semantics, and any idempotency key the external system supports. Meerkat does
not make that decision on the agent's behalf.

Streaming calls terminate with their process unless their feature protocol
exposes an explicit continuation handle. Detached tools may return a durable
operation or job handle; the operation's owning authority resumes the
operation. The original tool call remains stateless.

### 9.2 Approval does not resume execution

`ApprovalLifecycleMachine` remains the only owner of approval status. An
approval proves a decision, not execution.

V1 does not automatically continue an approved tool call. The v1 narrowing
provider has no `RequireApproval` verdict. An approval-tier call that lacks
separate authorization is denied. The application may create and route an
approval request through its existing product workflow, and the agent may
later observe the decision and decide whether to submit a fresh call. That
fresh call is evaluated against current policy and current arguments.

Standardizing when an approval authorizes that fresh call, whether it is
one-shot, and how it is consumed belongs to the generic authorization design.
V1 does not hide those semantics inside the tool hook.

### 9.3 A complete authorization platform is a separate design

If Meerkat standardizes approval-backed execution, it must be a separate
cross-resource authorization design with tools as one feature-owned adapter.
It must reuse `ApprovalLifecycleMachine` rather than create a second approval
owner in MobKit. This v1 deliberately does not predeclare that platform's
types, state machine, permit model, or resource taxonomy.

None of those concerns is required to make static member tool policy durable
or to install a narrowing check at dispatch.

## 10. HomeCore adoption

HomeCore motivates the proposal, but HomeCore configuration is not Meerkat
architecture. Adoption proves the platform contract without importing
household policy into Meerkat.

### 10.1 Effective baseline

The migration baseline is the final effective result of
`tools_for_profile(profile, domain)`, including every post-resolution
restriction. It is not only plugin manifests.

The baseline must include the effects of:

- `_DISCOVERY_ALLOWED_TOOL_SUFFIXES`
- `_TRIAGE_ALLOWED_TOOL_SUFFIXES`
- `_LIVE_GOOGLE_READ_SUFFIXES`
- `_AGENT_FORBIDDEN_DIRECT_MEMORY_MUTATION_SUFFIXES`
- the matching semantic-validation logic

Each restriction moves to the owner of the fact it constrains:

- Discovery and triage tool sets become exact member callback-materialization
  sets plus matching execution constraints.
- The live Google read surface remains a live-feature declaration. It is not
  relabeled as stable member policy merely because both contain tool names.
- The direct-memory-mutation prohibition remains a named HomeCore-wide
  invariant, but it compiles into the application narrowing policy. It must
  not survive as a bypassable post-dispatch filter.

The suffix rule itself remains live policy semantics until HomeCore replaces it
with a typed consequence classification. It must not be frozen into an exact
`DenyNames` snapshot and then falsely presented as future-proof. The migration
gate proves equality for the current catalog; a separate late-discovery test
must prove that a newly introduced tool matching the forbidden suffix is still
denied. Discovery and triage use closed exact allow sets, so newly introduced
names remain excluded without depending on a deny enumeration.

The required gate is an empty, bidirectional, name-by-name before/after diff for
all 17 members, computed separately for model-visible and executable tools.
The live-feature surface has its own equivalent diff. Counts are diagnostics,
not acceptance. In particular, migration must preserve the measured
containment that otherwise changes `domain:discovery` from 25 to 44 tools and
`triage:main` from 15 to 26.

### 10.2 Profile and category migration

HomeCore stops changing profiles merely to change tool capability. Existing
members keep their current profile and comms identity.

Member materialization declarations carry category differences. The
`domain:home-automation` shell case is therefore in scope: an administrative
member declaration may enable `shell` while the member remains on its existing
profile. The usual shell dispatcher and authority requirements still apply,
and execution constraints remain the final backstop.

If HomeCore actually needs a new role, provider/model template, or comms role,
that is not a tool-policy update. It requires a new identity or a separately
designed identity migration.

### 10.3 Reversible adoption

Adoption uses a separate candidate activation:

1. Compile HomeCore's immutable source bundle into member declarations and the
   closed typed `CompiledApplicationToolPolicy` artifact.
2. Validate the active policy snapshot revision and digest plus every required
   provider binding.
3. Dry-run reconciliation against all current identities and report every
   difference without writing desired state, parking a member, or
   quarantining an identity.
4. Run the 17-member effective-tool parity census.
5. Report provider-native capabilities that will be disabled.
6. Certify the candidate only when all checks pass.
7. Before fleet rollout, apply and then compensate one complete revisioned
   update on a disposable identity or an explicitly approved canary. Verify
   exact restoration of desired and active material, session continuity,
   durable cursor position, and no replay of already-consumed work.
8. For every realized member that predates durable identity intent, submit one
   full identity declaration through
   `mob/adopt_member_identity_declaration`. The operation requires an explicit
   `expected_absent` precondition, the exact existing session lineage, complete
   desired member material, explicit wiring custody, canonical owned wiring
   when identity custody is selected, declaration provenance, and a
   convergence mode. Externally managed custody is the default and requires
   an empty member-owned edge set; it leaves mob-level topology under its
   existing sole owner. The operation never infers desired state from the
   roster or transfers topology custody as a migration side effect.
   Replaying the same request is idempotent; an existing intent row is a typed
   precondition conflict and is never overwritten.
9. Roll out member declarations with the revisioned per-member command and
   verify convergence after each bounded batch.

A failed dry-run leaves the current activation and identities untouched. The
member-intent store has no atomic 17-member transaction, so rollout must not
claim household-wide atomicity. Once rollout begins, committed and uncommitted
members are reported explicitly; a failure stops further batches and requires
an equally revisioned compensating update where rollback is desired. A real
post-adoption semantic disagreement still fails closed.

The parity activation contains no intentional tool-set change. Any desired
addition or removal discovered during adoption lands only after parity is
established, as a separate reviewed declaration and policy revision. The
parity gate is never widened to hide simultaneous product work.

The compensating rehearsal is not part of the no-write dry-run. It is a
separate, explicitly authorized canary exercise before broad rollout. Merely
describing a rollback command is insufficient evidence: the rehearsal must
prove that compensation preserves strict durable cursors and does not replay
an incident outbox or other already-consumed effects.

The HomeCore activation id may be recorded as provenance. It is not the policy
revision or digest. `PolicyId` identifies the stable governed policy;
`PolicyRevision` and `PolicyDigest` identify the exact content installed in the
provider snapshot. Changed content must advance revision and digest even when
deployment metadata was copied.

## 11. Delivery sequence

### Phase 0: Close the live consequence fail-open

- Add `ToolConsequenceNarrowingPolicy` to the outermost dispatcher gate.
- Bind an exact in-process provider and validated immutable snapshot at agent
  construction.
- Add the fixed-capacity `PolicyEvaluationSupervisor`, bounded queue,
  generation fencing, deadline, and fail-fast saturation behavior.
- Lower `ToolError::PolicyIndeterminate` through the canonical fatal
  run-terminalization path before generic dispatcher-error conversion.
- Ignore caller-authored risk tiers as authority.
- Preserve ordinary `policy_denied` and run-terminal `policy_indeterminate` as
  distinct results, both with zero inner calls.
- Project typed provider failure for host-owned health and activation checks.
- Add no-bypass tests for callback, MCP, builtin, and host dispatchers.
- Add final-request provider-native sanitization for governed members.
- Define and validate the closed typed `CompiledApplicationToolPolicy`
  artifact, canonical digest, and source-provenance contract.
- Deliver the downstream MobKit gateway adapter that strictly parses and
  lowers HomeCore's compiled artifact into this immutable in-process snapshot.
  HomeCore owns source vocabulary translation and must not satisfy the seam
  with a Python callback on each call.

This phase is independently useful and does not wait for identity-intent API
work.

### Phase 1: Make static policy compositional

- Replace the single resolved-policy alternative with a normalized constraint
  set.
- Compile current `AllowList`, `DenyList`, and `ReadOnly` declarations into it.
- Compose profile read-only intent with member and parent constraints instead
  of replacing them.
- Preserve ordinary access-denied tool-result behavior.

### Phase 2: Activate member desired policy

- Add `MemberToolDeclaration` to existing desired member material.
- Expose compare-and-set apply and convergence status.
- Add an atomic `IdentityIntentMutationReceipt` for idempotent member-tool
  mutation results without changing `IdentityOperationReceipt` semantics.
- Extend the existing `ClassifyIdentityReconciliation` observations and
  decisions; do not add a parallel tool-only convergence authority.
- Add per-category and exact callback materialization intent using existing
  desired material and typed overrides.
- Route respawn, revival, ensure, reconcile, helper, flow, and placed-member
  construction through the same compiler.
- Rematerialize an existing member without changing profile, comms name,
  session, or stable identity.

### Phase 3: Contract and surface parity

- Generate wire schemas and Rust, Python, TypeScript, and Web SDK shapes.
- Expose declaration reads, apply outcomes, and convergence status.
- Return typed unsupported results on substrates that cannot supply member
  identity or a required materialization authority.
- Add schema, SDK, and surface parity ratchets.
- Ship the corresponding MobKit client/runtime release as a thin adapter over
  these Meerkat contracts. MobKit adds no member-policy store or mutation
  authority; this is new surface work, not only a dependency repin.

### Phase 4: HomeCore migration

- Compile the measured effective allow sets, including hardcoded frozensets,
  into member declarations.
- Install the in-process consequence provider.
- Run dry-run adoption and the 17-member empty-diff gate.
- Adopt pre-existing realized members with the expected-absent full identity
  declaration before applying later member-tool revisions.
- Declare wiring custody explicitly. MobKit's fleet-scoped managed topology
  remains externally managed; adoption must not copy those edges into
  per-member ownership or mistake non-empty physical topology for member-owned
  desired state.
- Report provider-native capability loss.
- Activate without profile reassignment.
- Remove legacy Python filters only after the shared gate proves parity.

### Separate proposal: Generic governed-action authorization

Approval-backed permits, authenticated decision principals, exact action
binding, and cross-resource authorization are not a later phase of this v1.
They are a separate architectural decision with tools as one adapter.

## 12. Acceptance gates

### Materialization and identity

- Enabling `shell` for one existing member changes neither `AgentIdentity`,
  profile, `MobMemberBinding`, comms name, nor session id.
- The enabled category survives process restart and fresh respawn.
- Enabling a category without required injected authority fails typed.
- Attempting profile reassignment still refuses rather than silently changing
  identity.

### Desired-state update

- Apply with the exact current intent revision commits desired state and then
  reports convergence independently.
- Repeating the same declaration is a no-op.
- Repeating the same request id and payload returns the recorded commit outcome
  plus a fresh convergence snapshot.
- Reusing a request id with different payload conflicts.
- A stale revision conflicts without changing desired or live state.
- A declaration or binding preflight failure leaves desired and active state
  unchanged.
- A crash after the atomic desired CAS but before live replacement is recovered
  by level-triggered desired-active mismatch on restart, without a client retry.
- A policy-driven rematerialization resume with a boot-scoped role-migration
  declaration still armed does not migrate the member and converges normally;
  equal stored and current roles leave the one-shot migration authority inert.
- A failed rematerialization reports desired and active revisions, preserving
  the fact that desired state committed while the new executor is not active;
  admission remains closed.
- A finite drain that exceeds its deadline becomes
  the typed `DrainBlocked` identity condition, keeps new turns closed, and
  never silently force-cancels the active turn.
- Resuming blocked convergence requires an idempotent command fenced by exact
  desired and active revisions; it cannot mutate desired policy implicitly.
- Every member construction and recovery ingress uses the same desired
  compiler.

### Constraint composition

- `AllowNames({a}) AND ReadOnly` admits `a` only when `a` is declared
  read-only.
- Profile read-only intent never widens a member allow-list.
- Parent and existing session constraints can only narrow.
- `DenyNames` wins over every matching allow.
- `Constraints([])` is rejected rather than treated as unrestricted.
- Unresolved `Inherit` fails before persistence and dispatch.
- A denied call makes zero inner-dispatch calls and remains an ordinary tool
  error so the run can continue.

### Application policy

- The provider receives trusted member and call context, not a caller-selected
  risk tier.
- A provider-bound member without an exact `MobMemberBinding` is rejected at
  construction.
- Policy identity includes both `PolicyProviderId` and provider-local
  `PolicyId`.
- Updating the provider-owned policy revision and digest does not mutate or
  rematerialize every member declaration.
- A supplied `risk_tier = r0` cannot weaken policy-derived `r3` treatment.
- `Deny` makes zero inner calls and remains an ordinary tool result.
- `Indeterminate` makes zero inner calls and terminalizes the current run with
  its distinct typed class.
- Model-driven dispatch promotes `ToolError::PolicyIndeterminate` before the
  generic tool-result conversion and reaches canonical fatal terminalization.
  External direct dispatch returns the same typed fatal error without creating
  a synthetic run.
- A deliberately wedged evaluator reaches a finite deadline, marks that
  provider instance unhealthy, and cannot consume an unbounded number of
  runtime workers.
- Fixed per-provider evaluator workers and bounded queues cap stuck work and
  isolate providers. Saturation fails fast, a late old-generation verdict is
  fenced, semantic failure can recover through a valid new generation, and a
  mechanical timeout remains unhealthy until process restart.
- Evaluation performs no Python, RPC, network, filesystem, or other blocking
  IO and uses one validated immutable snapshot.
- Missing or inconsistent required provider binding returns
  `policy_indeterminate` only for calls governed by that provider.
- The injected provider exposes typed failure that the host must include in
  health and activation certification.
- Deny and indeterminate observations carry enough typed correlation for the
  host to detect repeated identical attempts without becoming policy truth.
- Each no-bypass integration test has a paired allowed-call control over the
  same construction path that proves the sentinel inner dispatcher is
  reachable. The denied path is also demonstrated red under a gate-disabled
  test mutation, so a vacuous test cannot satisfy the ratchet.
- Final provider-request sanitization prevents resume, fallback, retry, or
  explicit parameters from re-enabling an ungoverned provider-native tool.
- `SessionLlmRequestPolicy` is applied by the typed provider-parameter owner at
  the end of `Agent::prepare_calling_llm_request` on every attempt.
- HomeCore's compiler emits canonical closed artifacts deterministically;
  unknown actions, ambiguous resources, conflicting duplicates, non-canonical
  content, or digest mismatch fail before provider installation. MobKit
  performs strict parsing and lowering without semantic inference.
- Every compiler-rejection ratchet is mutation-proven: removing that exact
  validation makes its targeted negative test fail, while a valid-artifact
  control still installs through the same path.

### Stateless execution and recovery

- No ordinary tool call creates durable invocation-machine state.
- Approval never automatically executes or resumes a call.
- A dangling `tool_use` remains ambiguous and is not replayed.
- After an uncertain result, the agent decides whether to submit a fresh call.
- Detached operations resume only through their own existing operation or job
  authority.

### HomeCore migration

- Dry-run adoption performs no desired-state write and cannot park or
  quarantine an identity.
- Adoption defaults every current member to externally managed wiring custody,
  requires an empty member-owned edge set, and leaves the mob-level desired
  topology untouched.
- The before/after model-visible and executable-tool diffs are both empty for
  all 17 members, including all hardcoded frozenset effects. The live-feature
  surface has its own equivalent parity assertion.
- Diffs are bidirectional and name-by-name; tool counts are diagnostic only.
- A newly introduced tool matching HomeCore's direct-memory-mutation suffix
  rule is denied even though it was absent from the migration catalog.
- Discovery remains at 25 tools and triage remains at 15 as diagnostics unless
  a separately reviewed policy change intentionally changes those sets.
- Intentional additions or removals are separate revisions after the parity
  activation; they cannot weaken the empty-diff adoption gate.
- A revisioned apply-and-compensate canary restores exact desired and active
  material without resetting durable cursors or replaying consumed work.
- The activation report lists native web-search loss before cutover.
- A failed candidate leaves the prior activation live.

## 13. Rejected alternatives

### Add only `tool_access_policy` to `MobMemberSpecWire`

This affects first creation. It does not update an existing identity because
ensure and reconcile retain present members.

### Add `ResumeOverrideField::Tooling` or `ResumeOverrideField::Tools`

One switch would collapse materialization, visibility, execution constraints,
and consequence policy. The member declaration drives the existing specific
override bits instead.

This variant is not present in the 0.8.25 baseline and must not be introduced
as a 0.8.26 shortcut. It would not solve HomeCore's per-member capability case,
and shipping then removing a generated-SDK enum variant would create
compatibility debt for no architectural gain.

### Pin application policy content in every member declaration

Pinning revision and digest beside `PolicyId` would turn each routine
application-policy edit into an N-member desired-state migration. The provider
owns its current immutable policy snapshot and reports its exact content
provenance on every evaluation. The member owns only which stable policy
identity governs it.

### Treat `Indeterminate` as an ordinary retryable tool result

The agent is the consumer of ordinary tool errors. Showing an unavailable or
wedged policy mechanism to the model as a retryable result invites an in-run
retry loop. `Indeterminate` therefore terminalizes the current run while
`Deny` remains an ordinary tool result.

### Add a policy-specific retry or denial machine

Denied calls perform no external IO and existing run limits bound agentic
retries. V1 projects typed denial evidence so the host can detect repeated
attempts, but it does not create a second retry authority beside the agent loop
and its existing budgets.

### Change profile to change capability

Profile is the member's durable role and part of its comms identity. Using it
as a mutable grant bundle is the application error v1 removes.

### Keep tool-category materialization profile-only

That leaves per-member `shell` and other builtin-category differences
impossible without identity-changing profile reassignment. A separate typed
materialization declaration is the missing semantic level.

### Let a Python post-filter remain the final backstop

Application policy may remain application-owned, but its verdict must execute
through the shared outermost gate. A direct callback path that skips that gate
is still a bypass.

### Build `ToolInvocationMachine`

Static allow-list lookup and an ordinary stateless request do not warrant a
durable per-call machine. External uncertainty is handled by existing durable
tail semantics and an agent decision, not automatic replay.

### Automatically resume after approval

Approval is a decision, not execution authority. The agent decides whether to
make a new call. A future generic authorization platform may define a
one-action permit, but it must not masquerade as resumed tool state.

### Extend the tool seam into a complete authorization platform

Memory writes, comms sends, schedules, devices, WorkGraph mutations, files,
network calls, and tools share the deeper problem. A full platform must be
resource-generic and gets its own proposal.

### Fail open when the policy provider is unavailable

Availability does not authorize a risky effect. Keep evaluation in process,
require an exact provider binding, and fail closed only at the affected call
scope. The injected provider and host own snapshot rollout and health; Meerkat
must preserve their typed failure rather than silently widening.

### Use activation id as policy revision

Deployment identity is useful provenance but does not prove policy content.
Canonical content digest and monotonic policy revision remain the policy
identity.

### Quarantine on adoption dry-run disagreement

Candidate disagreement should fail certification before live desired state is
changed. After adoption, a real semantic disagreement still fails closed.

## 14. Dogma check

- **Authority is singular:** desired member tooling lives in the existing
  identity intent; live executors and session metadata are derived.
- **Generated machines own canonical change:** `MobMachine` owns convergence;
  shell code only rematerializes the authorized executor.
- **Shells and projections are mechanical:** roster, SDK, health, and audit
  views report policy but cannot grant it.
- **Truth is typed:** categories, inheritance, unrestricted access,
  constraints, provider binding, failures, and convergence are distinct
  types.
- **Composability is feature-owned:** applications own product rules;
  Meerkat owns the shared enforcement seam.
- **Surfaces are thin:** every surface lowers into the same apply and dispatch
  paths.
- **Policy stays behind its seam:** caller-supplied risk never outranks the
  provider's resolved policy.
- **Terminality is explicit:** denied, indeterminate, convergence-failed, and
  uncertain are not success.
- **Contracts ratchet:** generated schemas, SDKs, and parity tests cover the
  production paths they describe.

The design intentionally stops where the facts stop. It solves durable member
tool composition and closes the immediate dispatch fail-open. It does not use
those needs as an excuse to invent a tool-specific authorization operating
system.
