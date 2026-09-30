# ADR-001 authority and recovery review, r3

Verdict: GREEN for architectural coherence and project-owner re-review. This is not implementation approval or evidence of a shipped security guarantee.

Reviewed candidate: `/Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md`, 904 lines, SHA-256 `a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e`.

Scope: full reread, emphasizing the interactions among participating authority fences, generated owner obligations, durable evidence, owner-store loss, snapshot rollback/cloning, fresh-context reset, reserved notification evidence, approvals and relationship authority. Source and doctrine were inspected statically. No ADR/source edits, builds, tests, deployment, or bus messages were performed.

## Material findings

None remaining in this candidate within the reviewed scope. The previous authority/recovery objections are resolved as architectural requirements. The rules now reject the concrete unsafe executions below without requiring an audit-driven executor, a second lifecycle journal, or a universal security machine.

This verdict depends on reading the explicit unsupported-profile refusals as requirements, not implementation latitude. An adapter cannot claim support while postponing its owner, fence, durable-custody or reconciliation contract.

## Adversarial checks

### 1. No audit-as-execution or competing recovery owner

ADR lines 95-104, 351-376 and 643-710 distinguish operation authority, durable evidence and release permission. The named session owner is `MeerkatMachine`; `ToolDispatchAdmission` is an adapter seam. Async/detached effect truth remains with the existing operation lifecycle and `DetachedJobMachine`. Completion/publication preserves runtime-completion authority and the existing completion/outbox carriers. Non-session operations require their own declared owner/handoff rather than artificial session turns.

The proposed orthogonal `EvidencePending` obligation has exact pending attempt/receipt identities and affects dependent continuation. It does not replace the underlying execution outcome. Receipt-store recovery provides evidence to a generated transition; store health cannot itself clear the obligation. Per-attempt durable records are allowed, but those records cannot choose a new execution or retry.

Counterexample tested: a tool sends successfully, receipt append fails, caller disappears, then a spool reader discovers the intent. The ADR preserves known success with evidence pending; only the receipt is retried while that success is known. If a crash loses the result, recovery becomes Unknown rather than fabricating receipt-only knowledge. The intent/spool cannot replay the effect.

Code evidence: `crates/meerkat-core/src/tool_execution_policy.rs:63-96` already describes a machine-backed admission seam with an ordinary no-op settlement default; `:548-573` currently admits before awaited consequence evaluation and can return a settlement error after dispatch. The new ordering and typed outcome/evidence contract are therefore genuine required changes, not existing guarantees. `crates/meerkat-runtime/src/input_state.rs:416-444` owns the durable terminal candidate and recipients, while `:679-692` explicitly makes the outbox shell payload rather than a competing lifecycle machine. The ADR preserves that distinction. `crates/meerkat-machine-schema/src/catalog/compositions.rs:2637-2678` provides an existing explicit ops-to-turn handoff pattern; it does not implement the proposed security handoff.

### 2. Independent authority races and rolling replicas

ADR lines 334-349 require every participating grant/policy/resource/binding authority to hold the relevant generation through one declared durable entry point. A final read under only the operation owner's lock is expressly insufficient. Lines 378-398 define revocation ordering, destination enforcement, explicit weaker leases, and a coordination domain that includes processes/replicas sharing the owner or resource.

Counterexample tested: process A evaluates P1; process B activates P2 or revokes the grant; A retains a local mutex and enters. This violates the participant fence contract. Another counterexample is a stale worker retaining an old epoch after rolling replacement; the current incarnation/fencing generation must reject its entry. Merely delivering an epoch notification does not establish ordering.

Code evidence: `crates/meerkat-runtime/src/store/mod.rs:7985-7988` explicitly says store atomicity is not a distributed lease for multiple live machines. `:9013-9035` shows the existing external-fence contract holding both the target transaction and authority guard through commit. This is precedent for the required shape, not proof of a generic grant/resource composition. Concrete lock order, release, expiry, restart and acknowledgement behavior remains an implementation/model obligation.

### 3. Snapshot antirollback and clone authority

ADR lines 185-195 cover revocations, approval consumption, replay protection, policy epochs, executor fences and receipt anchors. A copied epoch/instance ID is not evidence of currency. Activation must compare against a surviving rollback-resistant authority or watermark outside the reverted snapshot, and forward replay requires completeness proof. Restores/clones cannot hydrate protected data or perform effects before fresh incarnation authorization and current dependency validation.

Counterexample tested: restore a backup taken before revocation and recreate its matching local policy/grant/receipt databases. All those databases agreeing does not satisfy the independent surviving authority requirement. If that authority is unavailable or was also lost, the general missing-authority rule refuses governed work. A copied production credential does not authorize test-clone processing. Receipt-chain forks must be detected or explicitly separated into authorized namespaces.

Interaction with owner-store loss is coherent: the restore cannot reconstruct a live executor solely from historical evidence, and the owner-store recovery rule independently requires retiring/fencing the old incarnation.

### 4. Fresh reset binds routing, not just an empty transcript

ADR lines 520-536 require a fresh session/context identity, removal of every enumerated content-bearing input/cache/provider-state channel, independently admitted new input and freshly authorized configuration. Old restrictions, outstanding effects and pending evidence remain with their original owners. Continuing old work retains its mandate.

Counterexample tested: reset clears messages, then a late detached result addressed by logical agent name enters the new context. The new binding-owner rule atomically changes context generation and fences admission; old results remain generation-bound. The proposed reset therefore cannot launder a previous result or make reset an implicit retry. A future implementation that only swaps a session pointer but accepts name-routed late results fails this rule.

Selective carry-forward is explicitly deferred. The contract does not promise that removing one secret message proves all existing summaries safe.

### 5. Owner-store loss cannot infer safe replay from absence

ADR lines 630-636 preserve immutable association/work/payload evidence without turning it into a grant. Lines 701-710 first retire/fence the previous incarnation, then require authoritative non-entry or reconciliation with the actual effect owner/destination. Missing rows and absent receipts do not establish non-entry. Completed work is not re-executed; unknown work stays unknown until destination evidence/idempotency permits a safe action.

Counterexample tested: an external payment commits, the owner store is lost before recording its result, and an operator reconstructs input from audit. The audit row alone cannot trigger admission, and changing the downstream idempotency identity is forbidden. Recovery records the operator as actor while preserving the proven requester/ceilings, exact payload and current authorization. An unknown requester cannot be repaired by substituting the operator because the governing admission contract still requires the missing authority.

This permits recovery evidence to help a legitimate owner transition. It does not give the receipt store a replay queue or accept an operator's prose as authorization.

### 6. Reserved notification evidence is bounded and still durable

ADR lines 712-722 permit a separately provisioned receipt backend for an explicitly authorized notification class. Sources, destinations, payload fields, authority/mandate and exhaustion behavior are fixed; arbitrary private context and recipient widening are excluded. Evidence commits before dispatch, and the ordinary operation owner retains both attempt and settlement authority. Cross-backend append uncertainty retains the same identity.

Counterexample tested: ordinary storage is full, so an adapter sends an alarm first and later queues a note for audit. This violates the rule. Another counterexample is treating the reserved receipt backend as a substitute for unavailable execution-owner custody; the general pre-effect custody rule still applies. Exhausted reserve capacity or missing mandatory authority refuses the operation. This is compatible with mandatory governed audit and does not claim life-safety delivery.

### 7. Receipt ambiguity and physical batching

ADR lines 652-680 distinguish each logical protected attempt from physical group commit and specify committed proof, definite non-commit and unresolved append outcomes. Immutable content is bound to stable append identity. Reconciliation may finish the original protocol but cannot invent an execution outcome or authorize a new effect.

Counterexample tested: append commits but its acknowledgement is lost. A retry uses the same identity/content and establishes the original commit. It does not append a new independent operation or replay its effect. Conflicting content under the same identity is rejected. Twenty independently entered operations may share a physical flush, but each retains a distinct entry/obligation and cannot run before its required evidence commits.

Admission-time storage health cannot eliminate a later live receipt outage. Lines 362-371 and the matrix at 682-699 cover that outage rather than assuming it away. Full protected read/use/disclosure coverage remains mandatory; no turn-boundary-only exemption was introduced.

### 8. Approval and relationship authority remain distinct

ADR lines 288-296 bind human approval to the authenticated approver event, exact displayed canonical operation, generation, expiry and replay protection, consumed through `ApprovalLifecycleMachine`. A quoted email, model assertion, peer claim or successful login cannot replace the capture. Lines 89-90 and 142-153 assign typed relationship facts, protected writes and revocation dependency to an identity/relationship authority rather than a roster or knowledge graph label.

Counterexample tested: a model repeats a parent's old approval after guardianship was revoked. Neither the text nor the historical relationship can create a current bound approval/grant. Configuration-backed authority remains possible only under the same typed, authorized, monotonic and audited contract.

Code evidence: `crates/meerkat-core/src/approval.rs:253-265` currently stores a decision actor and optional provenance; `:664-732` accepts a supplied actor and invokes the current lifecycle operation. `crates/meerkat-machine-schema/src/catalog/dsl/approval_lifecycle.rs:62-84` has the existing lifecycle inputs, without the full new ingress challenge proof. `crates/meerkat-core/src/auth/principal.rs:86-92` defines an actor/subject pair, not proof of guardianship or authority to approve. These additions remain proposal requirements.

## Implementation proof boundaries

The following are not remaining ADR defects, but must not be converted into presumed implementation facts:

- `EvidencePending`, the participating security-entry composition, grant/policy lifecycle contracts, reset generation handoff and surviving antirollback authority are proposed changes. Their names are not evidence that production implements them.
- The tracer must assign each fact once, especially where a detached effect owner reports evidence/settlement into a session continuation guard. Two independently writable copies of effect outcome or receipt custody would violate this design.
- Every enabled non-session owner must supply the same durable custody and evidence-pending contract. The session mapping cannot silently stand in for arbitrary HTTP/blob/resource operations.
- External reconciliation must be exercised against the actual destination contract. A local receipt, a timeout or an idempotency-key-shaped string does not establish exactly-once behavior.
- The surviving antirollback mechanism and receipt integrity retention must be independent of the rollback/clone boundary under test. Merely restoring matching numbers from a second copied file fails.
- Performance optimization must preserve logical entry semantics. Group commit, batched authority queries and incremental cursors reduce overhead; none permits late audit or unbounded stale authority.

The acceptance gates already require actual sink assertions, stale-worker races, receipt acknowledgement loss, reset late-result isolation, owner-store loss before/after effects, reserve exhaustion, relationship revocation, and snapshot restoration. Those gates must run through production owners and the existing generated-machine/model pipeline. Design review alone cannot discharge them.

No mandatory wording revision is required before project-owner re-review.
