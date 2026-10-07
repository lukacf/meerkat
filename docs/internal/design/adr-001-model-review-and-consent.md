# ADR-001 amendment: model review and scoped human consent

## Status and scope

0.9 design amendment candidate, 2026-10-05. The 0.9 coordinator accepted the
direction and parallel preparation against review packet
`52436e2fe2ad35b9fff0ce3b27af45a0989c05678869141f45504ed41549dfcc`.
This document integrates that baseline with the explicitly identified HomeCore
and autonomous-host deltas below. Acceptance of direction is not implementation,
pilot, performance or release acceptance.

This extends the existing [consent contract](adr-001-confinement-and-consent.md)
and [local default](adr-001-local-governed-default.md). It does not create a second
permission system, approval ledger or workflow engine. Full coverage includes
native and host tools, MCP, information sources, communication, schedules,
delegation and every supported SDK/API surface. Early host-ingress coverage is
an implementation checkpoint, not the final profile.

## Decision

Use a restricted model reviewer to judge a retained action when owner policy
requires review. The existing native authorities still decide permissions,
currentness, consent validity and physical entry. Model judgment cannot grant
missing tool, account, source, recipient, processor or sandbox permissions.
Denial, unavailable review and pending human action settle the affected operation
with model-visible feedback. Permitted siblings and queued work continue.

Keep R1 authorization local, with no reviewer call. Measure optional reviewer
latency, token use, extra model turns and human delay separately from the native
dispatch p99 <1 ms and representative native overhead <=10% requirements. This
amendment provides no new benchmark evidence and adds no default policy network
round trip or per-operation fsync.

## What the Codex source establishes

Inspected `/Users/luka/src/cc/codex` at
`d6c3b448a41311ece3255c52ec3dbfd9ff36f154`, read-only. No Codex tests, UI exercise
or latency measurement were performed. Paths below are relative to `codex-rs`.

| Finding | Source | Meerkat disposition |
|---|---|---|
| "Approve for me" selects AutoReview rather than disabling approvals. | `tui/src/chatwidget/permission_popups.rs:120`; `utils/cli/src/shared_options.rs:79` | Explicit owner-selected review policy. |
| Host action, trusted request context and effective permissions are assembled separately from tool evidence; the reviewer has restricted tools. | `core/src/guardian/prompt.rs:106`; `ext/guardian-v2/src/sync_reviewer/reviewer_config.rs:25` | Retained native candidate, authenticated context and governed reviewer access. |
| The model returns allow/deny. Only outcome is mandatory; risk, authorization and rationale are optional. Model escalation is not in that schema. | `ext/guardian-reviewer/src/assessment.rs:80`; `ext/guardian-reviewer/src/completion.rs:147` | Define allow, deny and escalate explicitly; unavailable is a host outcome. |
| Authorization and review-context currentness are checked again after review. | `core/src/guardian/review_request.rs:206` | Reuse current native owners at entry, without a new global revision mirror. |
| Some failures appear publicly as denial; human fallback is limited. | `ext/guardian-reviewer/src/completion.rs:68`; `core/src/tools/approvals.rs:554` | Preserve unavailable versus refusal and explicit fallback policy. |
| Ordinary denials give tool feedback, but the denial circuit breaker can interrupt the parent turn, including after one completed denial for cyber-specialty models. | `core/src/tools/approvals.rs:465`; `ext/guardian-reviewer/src/circuit_breaker.rs:3`; `ext/guardian-reviewer/src/review.rs:140` | No permission-triggered turn/session interruption. |
| Adaptive thread-risk reuse and session approval caching are separate mechanisms. | `ext/guardian-v2/src/async_scorer/approval.rs:191`; `core/src/tools/sandboxing.rs:65` | Neither is a substitute for scoped native consent. No broad risk-score reuse in the first slice. |
| The reviewer has a 90-second budget with up to three attempts; assessment/warning events are transient. | `ext/guardian-reviewer/src/lib.rs:40`; `rollout/src/policy.rs:141` | Start with one bounded review attempt. Visible events alone do not establish durable audit. |

## Policy contract

Every tier retains current native permission, delegation, account/resource and
confinement ceilings. An eligible owner can choose R1 across a fully autonomous
deployment. No tool is hardcoded to human-only review by this amendment.

| Tier or outcome | Required behavior |
|---|---|
| R1 | No additional review. Native permission and entry checks suffice. |
| R2 allow | Satisfies this candidate's configured review requirement only. |
| R2 deny | Local refusal with an audience-safe reason; no entry. |
| R2 escalate | Review remains unsatisfied. Route to qualified human review of the exact candidate/closed batch if configured, or settle locally without a human request in an autonomous profile. |
| Reviewer unavailable | Distinct infrastructure outcome. HomeCore selects qualified human review as an alternative. An unattended owner can select local unavailable feedback. Neither executes unchecked. |
| R3 | Fresh qualified human decision for the exact candidate or displayed closed batch. Generic standing consent cannot waive it. |

Policy coverage is explicit at boot/activation and when dynamic operations are
introduced. Unknown policy never implies R1. A human route is required only
where configured policy requires one. Reject an invalid update without
terminating already admitted work. Apply tier enforcement at the shared native
operation boundary so direct SDK entry cannot bypass gateway configuration.

The reviewer receives authenticated requester, executor, represented subject,
selected account, immediate ingress, causal work association, applicable
mandates and scoped policy edits. Model statements and forwarded text do not
create those facts. The reviewer must itself be authorized for its model,
processor, investigation tools and information sources. Missing necessary access
is unavailable review. This is operational attribution, not semantic provenance
or a guarantee against information leaking from an LLM context.

Review one retained candidate or submitted closed manifest once, initially with
one attempt and a finite owner-selected deadline. Deadline expiry retires that
review reference through its owner before choosing fallback. Cancel owned work
through the existing task lifecycle; late results from the retired attempt
cannot authorize entry. Do not reuse a result for a merely similar action.

## Initial native, Meta and host interface

These are semantic contracts, not claims that particular Rust/API names ship.
The native owner selects typed representations and generated inputs through the
normal repository checks. Surfaces project the same meanings.

| Record or operation | Owner and required meaning |
|---|---|
| Retained action reference | The operation owner retains exact tool/action, canonical arguments, requester/executor/subject, account, resource/recipient scope, work association and applicable policy binding. A reference is not bearer authority; matching JSON cannot recreate a lost binding. |
| Review request/result | Native authority binds the candidate and review attempt. Meta invokes the reviewer and returns allow/deny/escalate through authenticated admission. Native code verifies binding, currentness, deadline and retirement. Host failures remain unavailable. |
| Human decision | The approval owner records authenticated approver, exact displayed candidate/batch, decision and expiry. Host adapters map the actual channel event to a principal and eligibility; agent-relayed claims cannot decide. |
| Action-required feedback | A settled local tool result with an audience-safe owner reference and unsatisfied requirement. It does not park the tool batch or run. |
| Resolution/expiry notice | Existing delivery/session owners reliably admit the native result to the owning session and wake it without a manual nudge. Duplicate delivery adds no authority. Delivery itself never executes the retained action. |
| Fresh attempt | The agent or host explicitly attempts the retained action after notice. Recheck current authority, candidate, review, expiry and remaining consent immediately before entry. A current exact decision does not require another review round trip. |
| Consent consumption | Existing approval lifecycle conditionally commits use against the shared approval/budget owner. One budget has one commit domain across sessions and processes; nested checks do not consume again. |
| Policy edit | Existing policy authority authenticates administrator, typed scope and requested tier/duration. It emits an attributable current policy result. Console and channel adapters do not keep an authoritative override map. |

The same settled native outcome is available outside the model transcript as a
reader-authorized typed host/SDK event identifying outcome kind (unsatisfied
review, reviewer unavailable or consent required), the existing action owner
reference and tool/action. Hosts can derive requested versus completed counts.
Events and counters are projections; they cannot authorize entry or expose
protected reviewer rationale to an ineligible audience.

An owning session that cannot receive a result has an explicit delivery failure
or terminal disposition, visible to the host/person. It is not silently treated
as delivered or used to recreate a deleted session. Reuse normal admission and
delivery recovery. The promise is reliable wake/visible failure, not immediate
execution or an exactly-once external effect.

Decision status, current spendability and execution disposition are different
facts. Approved can be expired, revoked or consumed. Used does not establish a
successful physical effect. Preserve unknown outcome after an accepted effect
whose response is lost.

## Consent, batches and temporary policy changes

Approve once covers one retained candidate. Approve a batch covers one submitted
closed manifest: for example, one calendar/account/event/content with twenty
named recipients. Count constituent recipient effects, not API calls. Each
constituent is usable at most once under that authorization. A host must submit
the manifest explicitly; twenty independent ungrouped calls do not implicitly
become a batch. The batch needs one review and at most one human prompt.

Reusable consent is useful only for a separate requirement that owner policy
explicitly makes reusable. Give it a deterministic typed scope, count unit,
remaining uses where bounded, expiry or an explicitly selected until-revoked
lifetime, and revocation. It never satisfies an unresolved R2 escalation or
R3 fresh-review requirement. Do not expose a misleading "don't ask" control when
there is no reusable requirement to satisfy.

For HomeCore's requested "don't ask for two hours" behavior, offer an eligible
administrator an explicit scoped policy edit, such as "approve this action and
use R2 for this actor/tool/argument scope for two hours". Display and authorize
the exact-action decision and policy change separately; report each result.
An approver is not automatically a policy administrator. The edit cannot exceed
that administrator's policy authority or weaken independent permission ceilings.
The existing policy owner expires the temporary rule and resolves the then-current
underlying policy. Auto-revert must not overwrite intervening edits with an old
snapshot. Applicable mandates and edits appear in trusted reviewer context.

HomeCore can present this through authenticated Telegram events as well as its
console. A private readable Telegram prompt showing the required recipients,
content and account is a full-detail surface. If a surface cannot show the
necessary details to an eligible reader, use another qualified surface; a
redacted placeholder cannot establish informed approval. Eligibility belongs to
host policy: a child's hold routes to parent-1; there is no universal self-approval
ban. Native safe feedback is separate from reviewer rationale, which is not
automatically forwarded to every model or viewer.

## Entry, persistence and restart

Complete host argument validation, preparation and cancellation checks before
consumption. Immediately at the physical-entry boundary, native owners recheck
current requirements and conditionally consume the needed use before invoking
the effect. A failure known to precede the consumption commit leaves otherwise-valid
approval spendable. An ambiguous commit result must be resolved through the same
owner before reuse. Once that transition commits, a crash, cancellation or failed response does not refund it;
the small commit-to-effect crash window has uncertain execution disposition.
Reconciliation or applicable operation idempotency is required before repeating
an uncertain effect. Fresh consent alone cannot establish non-delivery.

At inspected native commit `afb79ea29d8cdf30689345d884b43bc6c5baa428`, RPC runtimes
with a store path already open `FileApprovalStore` at `<store>/approvals.json`
and restore decisions (`crates/meerkat-rpc/src/session_runtime.rs:2558-2592`).
The current `ApprovalStore` load/put contract and instance-local file mutex do
not provide conditional consumption across processes. Extend the existing
generated approval lifecycle and native/realm store mechanics; do not add a
parallel consent ledger. The public custom-store contract must require the same
atomic owner commitment. A temporary single-owner backend must enforce its
limited ownership and cannot claim shared-process coverage.

Recover actual candidate bindings, decisions and consumption disposition under
the declared durability guarantee. Do not recover execution authority from an
audit row, matching arguments or historical Approved status. A memory-only
restart invalidates old references; it does not prove earlier effects never
happened. Unrelated newly admitted work proceeds under current authority.
Only the affected uncertain retry awaits disposition; there is no global
uncertainty quarantine.

The host chooses the human-decision deadline and validity rules; HomeCore's pilot
uses four hours. Expiry prevents later decision/use as applicable and produces
person and agent notices through the existing delivery owner. Offline recovery
observes expiry before entry and resumes pending notification delivery. Required
store/audit staging failures affect the new operation locally with distinct
infrastructure outcomes. Optional exporter failure is observable.

HomeCore's separate bounded resend policy for ordinary conversational Telegram
replies is outside consent- or approval-governed effects. It creates no native
consent exception, no refund and no relaxation of uncertain-effect rules.

## Parallel ownership and dependencies

| Owner | Work that can start against this contract | Integration dependency |
|---|---|---|
| GCP native lead (`claude-gcp-lead`) | Test-first generated lifecycle, candidate retention, conditional store contract, typed results, entry/currentness and restart. One owner edits shared machine declarations. | #1730 authorization/envelope foundation; #1741 tracks existing file-store conflict separately. #1730 does not implement consent step 6. |
| Meta/MobKit (`event-replay-gcp`) | Governed reviewer adapter, human routing, SDK/console projections and contract fixtures. | Native API for authority and entry; remove competing gateway authority when native enforcement is integrated. |
| HomeCore (`homecore`) | Authenticated Telegram events, person/approver/admin mapping, full-detail prompts, four-hour expiry and real pilot fixtures. | Native/Meta integration before replacing its live hold. |
| Toolkit (`toolkit-codex-local`) | Adapter parity and host-visible typed result fixtures. | Same native owner references; transport retry never resets consent or changes a candidate. |
| OB3 (`ob3`) | Autonomous R1/R2 policy and independent fresh-work/restart cases. | Current native entry; no mandatory human channel for a profile that does not require one. |
| ADR owner | Integrate design, focused adversarial review and documentation. | Review only changed semantics and failures, not unchanged accepted machinery. |

An initial authenticated host-ingress slice can use existing input/work-association
primitives. Broader peer/callback propagation and recovery claims require #1646
carry integration. Build jobs use isolated writable targets within the
coordinator's live resource budget; same-target work and performance measurements
remain exclusive where necessary. Use normal repository commands and CI.

Slice A delivers native retained action, exact decision/use, local results,
reviewer/test-human adapters, restart and one SDK path. Slice B adds closed
batches, shared count budgets, revocation and owner policy/consent preferences.
Slice C verifies HomeCore's real adapter and closes the remaining surface matrix.
Full A/B/C scope remains; each slice needs its own reviewable PR and executed
acceptance evidence. No production migration is authorized by source review.

## Acceptance and focused review delta

Write critical owner tests first with controlled clocks, completion barriers and
existing effect sinks. Condense them into the existing smoke suite, without a
new runner or receipt framework.

| Scenario | Required evidence |
|---|---|
| Routine appointment/dinner, plus repeated denied siblings | Controlled reviewer results preserve the model-tool-model loop. Separately, the real reviewer permits the HomeCore owner cases with authenticated ingress and calendar evidence, without an unnecessary human prompt. |
| Escalation, unavailable review and late human decision | Distinct typed outcomes; human alternative versus autonomous local feedback; authenticated notice wakes the owner; fresh attempt executes or fails visibly. Wrong approver, copied ID and agent-relayed decision cannot enter. |
| Currentness and review deadline | Pause review/entry, change relevant authority/account/policy, then release old allow: zero entry, siblings preserved. Retired review cannot overtake human fallback. Unchanged paired control enters. |
| Pre-entry and post-entry failure | Invalid/preparation failure leaves consent spendable. Conditional commit failure gives infrastructure feedback without entry. After consumption, failure/cancellation/UNKNOWN never refunds or automatically resends. |
| Contention and twenty-recipient batch | One closed-manifest review, at most one prompt; one final use shared across sessions/processes; each constituent enters at most once. This does not claim exactly-once physical effects. |
| Restart, expiry and notification | Recover open pending and decisions under the declared store profile; four-hour HomeCore expiry notifies person and agent; old memory-only references refuse while unrelated new work progresses. |
| Temporary owner policy edit | Authenticated administrator, actor/tool/typed-argument scope, current reviewer context, expiry/auto-revert without clobbering later edits; unknown tool stays denied. |
| Delivery/store/audit failure | Distinct infrastructure outcomes, no unchecked entry, settled sibling results retained, notice recovery or visible terminal failure. |
| Host observability | Typed host/SDK outcomes outside the model transcript agree with settled tool feedback and identify the existing owner reference and tool/action. Requested/completed projections add no authority or unauthorized rationale disclosure. |

The changed HomeCore seams are C1 real-reviewer evidence; C2 explicit closed
manifest; C3-C5 expiring policy edit and authenticated channel administration;
C6-C8 host eligibility and actual channel/full-detail evidence; C9 reliable wake
without callback execution; C10 pre-entry spendability versus committed-use crash
window; C11 expiry/notices; C12 real Telegram migration acceptance. These receive
one focused delta review. The accepted baseline is not reopened.

HomeCore retains `approvals.py` and its current human hold until the replacement
passes its seven owner cases plus concurrent re-issue, restart/open-pending,
expiry/notices, wrong/copied/agent-relayed decisions, pre-entry spendability,
parent-1 routing and the four-hour deadline through the real adapter. Stub tests
establish enforcement, not reviewer judgment quality or live migration readiness.
