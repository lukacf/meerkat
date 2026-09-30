# ADR-001 r2 independent Toolkit integration review

Reviewed candidate: `candidate-r2.md`, SHA-256 `44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca`, all 568 lines. Reviewer: `/root/c1_prototype_independent_review`. Scope: setup coalescing, independent waiter grants, secrets and human actions, custom native extensions, service commissioning, durable recovery and revocation. Root separately reviews profile negotiation and SDK/tool fidelity.

## Disposition

**No blocking design defect found in this assigned scope.** The candidate can express Toolkit revision 8's requirements without a second security-owned setup lifecycle, a general privilege-elevation flag, or merging waiter authority. This is bounded design review, not an implementation acceptance, endorsement of all referenced current-source claims, or selection of C1's concrete architecture.

No proposed design-defect finding survived the clauses already present. The cases below are concrete implementation acceptance obligations, not requests to expand the ADR with a Toolkit state machine or to add new upstream APIs. No minimum architectural text fix is required for them. An implementation claiming coverage must supply the stated evidence; the ADR itself supplies no executed proof.

## 1. Shared preparation is not shared waiter authority

**ADR:** section 3, lines 150-181; section 5, lines 316-323. **Toolkit:** revision 8, lines 368-380.

Counterexample attempted: two acquisitions under one authenticated owner share an exact connector installation. Waiter A is revoked or cancelled while B still needs the preparation. The installer completes, and A receives B's successful activation or retained access because the setup key was treated as an authorization key. Another variant cancels A by revoking the shared account credential, breaking B.

Why this is not an uncovered design defect: the original authority, native contributors and disclosure mode remain associated with each admitted work item; conflicting authority is not a duplicate entitled to the original result; ancestor validity remains live. The independent service-mandate rule permits a genuinely separately authorized preparation owner to finish without manufacturing surviving authority for A. Toolkit already assigns shared effects and waiter outcomes separate owners. Nothing requires duplicating the physical installation or conflating independent acquisitions with one security operation. First-profile multi-principal context batching is expressly unavailable, so it cannot justify pooling differently authorized request text in one model context.

Minimum evidence: two real waiters, one shared exact installation, separate activation and disclosure receipts; cancel/revoke A before its role entry; B can still complete if its own authority and preparation mandate permit it; A gains no uncommitted role, result body or borrowed credential-administration right. Repeat with restart, a late callback, a new waiter after preparation completed, and a different requester attempting the same key. Assert the actual role and delivery sinks. If a future batching profile is offered, additionally prove contributor-safe model context and fan-out, not merely scheduler deduplication.

## 2. Human connection steps cannot become bearer or grant forwarding

**ADR:** section 2, lines 122-140; section 3, lines 150-163 and 186-201; section 4, lines 205-250; section 5, lines 254-278 and 300-314; section 7, lines 377-385. **Toolkit:** lines 400-435.

Counterexample attempted: setup requests an OAuth ceremony, then treats the person who happens to finish the browser step as the original requester; a callback for an old account/attempt activates a new role. Alternatively an auth page, PKCE verifier, token, password or callback code reaches model context through browser observation, an elicitation result, a transcript or an audit record.

Why this is not an uncovered design defect: authenticated requester, actor, binding and exact operation cannot be substituted; human approval is scoped and expiring; changes to binding/arguments require a new decision; protected reads and disclosures have resource owners and credential plaintext is excluded from receipts. Toolkit's domain contract supplies the stricter browser isolation, exact ceremony binding and secret-entry rules. The ADR is extensible to these mandatory obligations and refuses unsupported enforcement. It does not prescribe an OAuth card as an authority or require secrets in a common runtime envelope. The general information-flow contract is not a substitute for Toolkit's explicit no-secret-in-model rule.

Minimum evidence: a real secret canary never reaches prompt construction, tool-visible JSON, SSE, history, screenshots/DOM capture, ordinary logs or security receipts; exact user/provider/credential-owner/account/redirect/scope/attempt binding; denied account substitution; callback replay and supersession; restart with an owned challenge or explicit invalidation. For a non-repeatable domain ceremony, show the domain owner consumes the attempt budget before the sensitive effect and cannot be bypassed by a new setup ticket or automatic retry. These are feature-owned obligations and tests, not missing generic ADR states.

## 3. Connector access and frontend exposure remain separate operations

**ADR:** section 1, lines 106-111; section 3, lines 150-163; section 4, lines 205-219; section 5, lines 310-323. **Toolkit:** lines 15-20, 256-266 and 449-451.

Counterexample attempted: installing a Slack package or authorizing its bot account silently grants channel participants the right to address a private agent; the connector's account grant is then reused as authorization for an audience-wide reply.

Why this is not an uncovered design defect: feature-owned action/resource declarations, exact bound destinations and independently authorized disclosure allow separate service access, frontend admission and frontend delivery owners. Neither package installation nor provider credentials are defined as requester identity. Toolkit already declares these separate roles. A common evaluator need not erase those distinctions.

Minimum evidence: a package exporting both roles; connector success with frontend exposure still denied; a verified provider callback that does not admit arbitrary channel participants; exact conversation/requester/audience preserved on reply and retry; monitoring/export copies separately authorized. No architecture text change required.

## 4. Custom native extensions must identify the real deferred-effect owner

**ADR:** section 1, lines 106-111; section 2, lines 122-140; section 4, lines 205-250; section 5, lines 300-308; section 7, lines 394-407. **Toolkit:** lines 445-451.

Counterexample attempted: a Reachy-style tool is permitted once, queues work in a daemon and reports success; later physical actions use the daemon's broader ambient credential after the request grant is revoked. A generic `write` classification hides the extension's required physical-control or safety-monotone contract.

Why this is not an uncovered design defect: the candidate explicitly requires declarations for governed entry points, refuses unsupported obligations, separates launch from terminal execution, and requires each subsequent effect to carry its actual owner's correlations. Long-running operations must declare checkpoints or a bounded indivisible entered effect. Native hooks and sinks must participate or remain unavailable. Trusted native code is honestly part of the TCB; the ADR does not claim a tool-name ACL confines malicious extension code.

Minimum evidence: a real extension/daemon handoff with its domain declaration; separate launch and effect receipts; revoke before a subsequent new effect and observe the physical/control sink remains untouched; after-entry revocation preserves truthful outcome and attempts supported cancellation. Demonstrate required domain semantics cannot fall back to ordinary write. If hostile-code containment is claimed, show process/network/credential confinement separately. No new generic execution owner follows from this requirement.

## 5. Privileged service commissioning is not delegated elevation

**ADR:** section 3, lines 165-201; section 5, lines 280-314.

Counterexample attempted: a setup helper lacking the user's private read rights labels its work a service, takes its own broader host credentials, processes unrelated data, and returns an unrestricted summary to the requester. Another variant detaches a child grant from its revoked ancestor by relabeling it a service mandate.

Why this is not an uncovered design defect: the independently issued service mandate and typed commissioning contract are explicit, bounded and separately authorized; service parameters, recipients and revocation dependencies are recorded; requester and service actor remain distinct; internal credentials and permits cannot be exported; publication is a separate decision. Ancestor validity applies to derived grants, while intentionally independent service authority must have its own issuer. Derived data does not declassify itself. These provisions directly block both variants.

Minimum evidence: an authorized commissioning case succeeds without conferring internal read rights; the same request under ordinary delegation fails to gain those rights; changed resources, processor, destination or purpose refuses; unauthorized service-mandate creation refuses; an output still bound to restricted sources remains withheld unless a separately authorized release transformation permits it. Revocation dependencies must be exercised at the actual service and release owners. No architectural fix identified.

## 6. Restart and audit failure must not turn a setup completion into a new effect

**ADR:** section 3, lines 158-163 and 177-189; section 4, lines 221-250; section 7, lines 387-426. **Toolkit:** lines 368-384 and 451.

Counterexample attempted: an installation, identity order or downstream mutation applied, but its reply/mandatory receipt was lost. Restart replays the operation from an intent row, or an observer cancellation is treated as proof the old attempt stopped. A saved ready receipt resurrects a revoked role on a restored session.

Why this is not an uncovered design defect: durable effect-owner custody precedes entry, intent alone is not execution authority, known outcome and evidence-pending are separate, uncertain recovery cannot infer safe retry, and grants/bindings must be current on new use. The ADR retains existing effect ownership and does not create a security-owned retry queue. A requester's cancellation does not erase owner custody. Toolkit's exact activation-incarnation receipts and browser ceremony restrictions fit these requirements.

Minimum evidence: kill the owner at admission/entry/effect/receipt boundaries; recover the same native attempt and the correct Known/Unknown/evidence-pending state; no second install, email send, identity order or PIN submission from receipt-only retry; cancellation-resistant settlement; actual grant/policy/resource authorities determine a revocation race. An in-memory common lock or source-only tracer is insufficient proof of participating production fences. Test stale activation callbacks and restore against current role/grant/binding revisions.

## Review limits

- No code, builds, tests, prototype execution, live edits, commits, bus messages, provider traffic or deployed behavior was exercised. This review cannot establish real coverage or safety.
- Read the entire candidate and relevant frozen Toolkit revision 8 requirements. Existing source baselines cited by the ADR were not independently re-audited for this design pass.
- The first governed profile is narrower than Toolkit's complete end state. Its explicit lack of mixed-history, dynamic-audience and multi-principal batching support is a compatibility limit to expose honestly, not evidence those Toolkit requirements can be discarded. Root is reviewing profile negotiation in detail.
- Do not turn these acceptance cases into a parallel Toolkit policy engine, shared grant, shadow effect journal or mandatory new upstream API. Concrete ownership and composition choices still need the existing-owner tracer and implementation evidence.
