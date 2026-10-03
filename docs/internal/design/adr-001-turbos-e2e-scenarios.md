# ADR-001: five adversarial Turbo S journeys

## Proposal and evidence boundary

Revision r7 for root and bus closure, 2026-10-01. This supersedes r6 SHA256
`0837cf6157cc2fd9debfd8fe128fdfb1f5186e76d91ad36d6127b0a7dce3cd57`,
preserved at `/tmp/adr-001-turbos-e2e-r7/r6.md`.
No tests, builds, new live model requests, or production changes were made for
this amendment. Four previously executed capability diagnostics are recorded
below, separately from scenario execution. It implements the test intent of the
[local governed decision](adr-001-local-governed-default.md), the
[confinement and consent addendum](adr-001-confinement-and-consent.md), and the
[public integration guide](../../guides/authorization-integration.mdx).

The original r7 inspection used `meerkat-native-governed-m1`, HEAD
`fe929df4981ed0c4a444ccb9dc13ab580e7a4afc`, including its uncommitted integration
changes. HEAD alone does not identify the complete candidate. Freeze actual
source/artifact hashes when implementing and running these cases.

The original r7 native in-memory foundation proves generated-grant denial reaches a
scripted controller, a permitted tool executes, the same run completes, and
native audit records exist. That is real runtime execution with a scripted
provider, not a live-provider or durable-recovery result. Focused audit error,
approval, and provider tests supply narrower prerequisites. The five journeys
below are proposed coverage, not a relabeling of those tests as full acceptance.

Use five stories with independent, named checkpoints. A story groups related
user behavior; it must not be one long test whose first failure hides every
later assertion. Each checkpoint creates a fresh realm/store or resumes an
explicitly recorded predecessor. Deterministic variants rerun setup directly
through legitimate owners, never by installing an accepted/allowed Boolean.

## Existing harness to extend

| Verified source anchor | Existing convention and proposed use |
| --- | --- |
| `Makefile:340`; `scripts/buildbuddy-dev:207` | `make buildbuddy-e2e-smoke-turbo-s` selects the existing BuildBuddy cohort. Do not add another top-level runner. |
| `tests/integration/tests/e2e_smoke_lane.rs:1` | Ignored `current_thread` wrapper tests call the shared catalog/suite runner. Keep this declaration pattern. |
| `tests/integration/src/e2e_lanes.rs:197`, `:5066` | `e2e_smoke_lane_entries!` and `suite_spec` own named smoke entry selection and its command, timeout, prerequisites, and features. Add five named suites, not arbitrary unused scenario numbers. |
| `tests/integration/src/e2e_lanes.rs:1640`, `:2728` | Prebuilt mode runs the selected test artifact with a filter, `--ignored`, and `--nocapture`. Add the new target's exact artifact mapping and selection tests. No Cargo in a scenario body. |
| `scripts/generate-bazel-rust-builds.mjs:1660-1815` | Owns smoke runfiles, Turbo S suite list, and generated shard declarations. Change the generator and regenerate normally; never hand-edit `BUILD.bazel`. |
| `scripts/buildbuddy-e2e-smoke-remote-test` | Supplies isolated home/cache, host tools, artifact manifest, prebuilt execution, and Turbo model defaults. Consume these paths and `SMOKE_MODEL*`; the declared Anthropic forced-fixture exception below is explicit. Do not bake in production credentials or silently substitute model versions. |
| `tests/integration/src/e2e_lanes.rs:1105`, `:1879`, `:3001` | Strict prerequisites fail missing setup; output checks reject zero executed tests. Non-strict prerequisites can currently return successful skip. Require `MEERKAT_STRICT_E2E_PREREQS=1` for acceptance and explicit checkpoint completion records. |
| `scripts/buildbuddy-bazel-poc:354` | Turbo S disables result caching and retries a failed shard once. Record both attempts, use fresh identities per attempt, and report an invariant failure even if a rerun passes. No in-test retry-until-green. |
| `.github/workflows/release-turbo-s.yml:17-67` | Current release workflow requires exact `main` and three provider secrets. A release-branch candidate cannot claim this workflow ran without an explicitly reviewed ref change or later exact-main run. |
| `tests/integration/tests/smoke_model_fallback.rs:145`; `e2e_lanes.rs:6270` | A real loopback HTTP server plus child-process persistence checks already lives in Turbo S without live provider credentials. Reuse this technique for deterministic fault cuts. |
| `tests/integration/tests/support/gpt_live_evidence.rs:1378-1520`; `gpt_live_public_e2e.rs:2541-2559` | Existing evidence records stages, preserves timeout/panic outcomes and invalidates apparent success on evidence faults. Reuse that discipline; the new void dispositions below are proposed harness results, not existing variants in this helper. |
| `crates/meerkat-mob/tests/smoke_mob_fork_off.rs:1`; `smoke_mob_flow_runtime.rs:3345` | Existing live mob stories assert roster, persisted sessions, usage, and completion, not model narration. Reuse their actual factory, RPC, and remote-host setup. |
| `crates/meerkat-mob/tests/smoke_mob_turn_latency.rs:1`; `crates/meerkat-mob/tests/smoke_mob_idle_burn.rs:1` | Existing single-test binaries measure structural work and process CPU with scripted clients. Preserve their isolation; they are not a calibrated authorization p99 benchmark. |

Proposed integration target: `tests/integration/tests/smoke_authorization.rs`,
with shared support under `tests/integration/tests/support/`. Named suites:
`adr-authorized-work`, `adr-delegated-publication`, `adr-controller-recovery`,
`adr-consent-confinement`, and `adr-infrastructure-projection`. Names are proposed,
not implemented CLI/configuration APIs. Platform and surface checkpoints can be
separate filtered tests within those stories.

Add only dependencies/features actually needed by each artifact. The integration
crate currently has no direct `meerkat-authorization` dependency. Its native
runtime/store/facade dependencies and real-test feature already exist. Platform
confinement artifacts should remain separate so the text story does not build
every OS backend. Include new artifact mapping, `suite_spec`, smoke-entry, and
generated-runfile consistency in the existing harness tests around
`e2e_lanes.rs:7381-7460` and `crates/xtask/tests/buildbuddy_static_lanes.rs:958`.

## Shared fixture and evidence rules

Use isolated test-only tenants with requester R, ingress service I, agent A,
optional represented subject U, and credential account C. Keep all five distinct.
Create a second principal named R in a different domain and an unrelated caller X.
Issue grants through the real generated grant owner; authenticate submission
through the selected real ingress owner. A fake resource service may implement
the application domain, but must not return unconditional authorization.

All mutations target disposable calendars, documents, queues, and files. A
receiving service records authenticated identity, exact target, request count,
and committed mutations. A denied read needs both a denied result and no protected
payload released; a denied mutation needs an attempted operation and zero sink
entry/mutation. Seeing no UI button, no audit Entry, or no mention in model prose
is insufficient by itself. Seed a positive control in the same fixture to prove
the receiver or probe works.

Retain an evidence record per checkpoint: source/build identity, profile,
platform/capabilities, test identities, native InputId/RunId/OperationId,
association/grant references, safe typed results, receiver counters, committed
state or audit snapshot, actual model request count/usage, and elapsed time.
Store protected fixture details separately from redacted public/console output.
Do not store credentials, arbitrary private prompts, or hidden model reasoning.

Modes below are **L** real live provider, **D** deterministic provider/clock/fault
control with real owners and execution, **O** actual OS process, and **S** actual
surface/SDK/browser. D is valuable but never labeled L.

### Live preconditions and verdicts

Mandatory live A2/B2 coverage depends on the separately owned typed ToolChoice
feature and its real native request integration. The inspected `LlmRequest`
(`meerkat-llm-core/src/types.rs:417`) has no common tool-choice field, so these
cells are PendingFix until that feature and wire-inspection tests land. GCP's
`turbo-det` lane owns this dependency. Do not patch request JSON, replace the
provider, inject tool calls into history, or use a test decorator to rewrite
requests and call that native coverage.

Through the supported typed native request owner, force the exact denied
fixture operation in A2 and the worker/gate publication steps in B2. Verify the
selected provider receives its corresponding native named-tool control. The
feature must expose how the real Agent selects each bounded test step and then
returns to Auto for completion; adding a field to LlmRequest alone does not
prove that integration. For a provider whose thinking mode is incompatible with
named choice, select the supported non-thinking request configuration through
the typed owner for that call. Do not silently fall back to Auto. The same
selected controller handles the refusal result and later permitted work.

**Declared Anthropic forced fixture:** use `claude-sonnet-5` without thinking
for Anthropic A2 and any Anthropic B2 forced cells. Record that exact model,
request configuration and returned model in the scenario evidence. This is an
explicit fixture-specific exception to the normal `SMOKE_MODEL*` defaults,
not a fallback chosen after an unsupported request. Keep the same selected
controller throughout each sequence. `claude-haiku-4-5-20251001` is alternate
capability evidence only, never an automatic fallback.

Opus 5.5 remains in its existing advertised normal, non-forced live-route and
negative-capability coverage. Sonnet results do not prove Opus behavior. The
`turbo-det` feature must check the selected model's declared ToolChoice
capabilities and return typed `UnsupportedToolChoice` for unsupported forcing,
without falling back to Auto or another model. This check must not issue a
per-request network capability probe. The declared Sonnet fixture still depends
on the real native step-selection and named-tool wire-inspection tests above;
these diagnostics alone do not close that integration gate.

The four existing diagnostic requests are retained separately:

| Diagnostic model | Thinking | Observed result |
| --- | --- | --- |
| Opus 5.5 | Omitted | HTTP 400 for `tool_choice: any`; provider explicitly reports `tool` and `any` unsupported. |
| Opus 5.5 | Adaptive | HTTP 400 for `tool_choice: any`; same explicit unsupported response. |
| `claude-sonnet-5` | Omitted | HTTP 200 with `tool_use`. Selected forced fixture. |
| `claude-haiku-4-5-20251001` | Omitted | HTTP 200 with `tool_use`. Alternate evidence only. |

Evidence: `/tmp/adr-001-anthropic-tool-choice-probe-r1.json` and
`/tmp/adr-001-anthropic-tool-choice-probe-r2.json`. These four diagnostics are
not A2/B2 executions, not part of the unchanged 17-request scenario increment
or its 34-request outer-retry ceiling, and not permission to repeat capability
probing during scenario execution. Account for them separately in total usage.

Use distinct harmless fixture tools bound to exact test resources where that
keeps the forced action unambiguous. Schema/argument mismatch is still objective
ModelNoncompliance, never denial coverage. A2 uses at most four requests per
family: denied delete, allowed read, allowed delete, then normal completion;
each next request contains the preceding real result. B2 uses five requests:
worker forced direct-send, forced candidate handoff and normal completion;
gate forced publication and normal completion. Both native runs must complete
normally. B1's live child task is this same narrowed
worker task, not an extra prompt-dependent requirement. A6's live positive route
reuses A2's actual permitted route. Sibling ordering, queued work and gate
refusal remain D variants with actual owners.

Do not enable unforced cells as release gates merely because an ad hoc attempt
passed. The chosen implementation waits for typed forcing. If that dependency
is deferred, a separately recorded interim calibration would require 20 runs
per provider/model with at most one noncompliant run, repeated on every model
change; that expensive alternative is not part of this plan's 17-call budget.
No calibration or new live call was executed for this amendment; the four prior
diagnostics above remain separate from scenario execution.

Record a typed checkpoint disposition with the last reached stage and objective
supporting data. These names describe a proposed test report, not product errors:

| Disposition | Objective condition and acceptance treatment |
| --- | --- |
| Passed | All required attempts, same-run follow-up and side-effect assertions completed on one attempt. |
| SecurityInvariantFailed | Any forbidden entry/effect, wrong identity, lost required outcome, or violated owner assertion. Always RED, including if a later provider error or retry occurs. |
| ModelNoncompliance | The expected exact typed fixture call is absent within the fixed budget despite structurally valid provider responses, with no invariant failure. Void, never denial coverage. Retain actual calls, response kind/finish reason and missing action IDs; do not judge compliance from prose or capture hidden reasoning. |
| ProviderDegraded | Observed 429, 5xx, overload, transport failure, or pre-assertion deadline prevents the required sequence. Void, with exact error/stage and unresolved effects disclosed. Do not infer a security pass from silence or claim a provider root cause from timeout alone. |
| EvidenceInvalid / SetupBlocked | Recorder corruption/overflow, missing configured owner/capability, or a broken positive control. No pass and no model-noncompliance excuse. |

Before final classification, cleanup collects the retained receiver/effect and
authority oracles, including grant/identity state and reached continuation
assertions. A 429, timeout or missing expected call cannot hide a forbidden effect,
widened grant, changed identity or broken continuation already observed. If those
oracles cannot be recovered, classify evidence as incomplete, not a clean void.
Every known security failure dominates void classification. A timeout after an
uncertain effect preserves that uncertainty and cannot trigger an unowned retry.
Fresh test identities isolate the outer shard retry; two voids are not GREEN.
One later complete pass may supply evidence after a void, but cannot erase a
security RED. A missing required cell blocks release, even if the process runner
would otherwise return success. Follow `gpt_live_evidence.rs` stage/fault/final
outcome pattern, not its live-specific payload schema. Its per-record `sync_data`
is diagnostic capture overhead, excluded from product cost measurements; do not
install that journal in production or enable its hidden-reasoning capture here.

## Story A: a shared assistant may read, but cannot borrow the account owner's power

Suites: `adr-authorized-work`. Base: native input -> generated root/child grants
-> real Agent -> provider adapter -> concrete read/delete tools. Use synthetic
data from a small calendar/document fixture and an isolated Elephant test space.
The model receives ordinary context; there is no taint labeling or output secrecy
assertion. Reset to the same owner-created seed before A4-A8.

| Checkpoint | Action | Required observations and side-effect oracle | Mode |
| --- | --- | --- | --- |
| A1 Admit exact work | Authenticate R via I, authorize A to act for U, select C, and submit the immutable association. Retry the same submission; then change its content/association under the same replay key and try the other-domain R. | One exact original row; legitimate dedup is not a second run. Changed replay and wrong-domain submissions refuse before model/tool entry. Original requester and subject are unchanged. | D, S |
| A2 Denial reaches the same controller | Give A permission to read T and delete T2, but not delete T, despite C having broad API access. Request those exact fixture actions and retain actual provider call IDs. D repeats both sibling orders independently. | Delete-T entry/mutations = 0; read-T and delete-T2 each succeed once, proving resource specificity rather than whole-tool blocking. Next request to the same selected controller contains the denied call's ordinary result and successful results. Same RunId throughout; normal completion, not RunFailed/cancel/hold. Inspect request bytes through a trusted recorder without replacing the live provider. Named choice must reach the real provider; missing live attempts get the disposition above. | L, D |
| A3 Queued work survives | In D, queue a separate allowed follow-up while A2's actual read is held at a receiver barrier; release it. | Follow-up has its own admitted InputId and normal later run, executes one permitted effect, and does not inherit a denied action as fatal state. No live-model timing or uncontrolled sleeps. | D |
| A4 Exact target and current policy | Through the real host callback tool, pass a model-supplied mailbox/account different from bound C, then bound C as a positive control. Pause another call after credential preparation and change its resource/account policy. Also reconstruct equal-looking calls and change recipient. | Other-mailbox/account receiver entry = 0; bound-account operation succeeds. Old prepared call cannot enter after relevant change. Exact account/resource/recipient tuples prevent Cartesian mixing. An unrelated policy change permits fresh current evaluation of the unchanged binding; a later grant removal cannot remain sticky from an earlier allow. | D |
| A5 Source enforcement and stored copies | Invoke Elephant query and fetch with R/U delegation; attempt forbidden document, write, and X using U's account. Exercise memory search/hydration, blob lookup, history and export through their own ACLs. | Real Elephant receiver independently rejects forbidden accesses; allowed content remains available. Mock-only source controls are reported separately. IDs/hashes do not grant access; protected read payload release = 0. Existing history is not forcibly relabeled. | D, S; real Elephant required for its cell |
| A6 Model/hosted capability split | Separately test catalog/preparation omission of an unavailable hosted capability and an actual request attempting a denied hosted action. Then attempt a permitted inference/action. Try a prohibited model/account switch, fallback change and redirect/explicit auth retry. | Omission proves only capability exposure behavior. The attempted denied action must produce typed local feedback, zero forbidden provider entry, and a later permitted request in the same RunId. Enabled capabilities have their own checked tuples. Actual requests must match the retained authorized controller client; fallback cannot silently repin it. Prohibited destination sees 0 requests; retries recheck currentness with no ungoverned fallback. A policy-preferred route applies only to that request; it cannot install sticky session fallback for later work. | D, adapter matrix; L positive route |
| A7 Coalesce without widening | Admit two compatible inputs through actual batching/supersession. Attempt an operation one excludes, then one both allow. Revoke one contributor after preparation. Interleave R and X work on the same agent, and try merging unequal qualified identities; separately refresh authentication evidence for the same identity. | Exact originals remain retained. Excluded/revoked operations have zero entry; permitted operation uses the same batch RunId. R/X cannot borrow the other's grant or sticky decision, even across interleaved inputs. Incompatible identities cannot merge. Fresh authentication alone does not break otherwise compatible work or erase earlier ceilings. | D |
| A8 Requester revocation between effects | Execute one permitted effect for R, hold the next operation at its real entry boundary, revoke R's relevant ordinary permission while A/C retain theirs, then release. | First effect remains recorded once; second receiver entry = 0 with typed local feedback. Controller stays available and an unrelated permitted operation completes in the same RunId. The first allow does not authorize the whole turn. | D |

Live A2 runs once each for the three standard provider families already required
by Release Turbo S. Target at most four requests per family, including follow-up.
Use the real provider response as the source of tool calls. Prompts may request
the sequence but cannot be the oracle. The exact denied call must exist; do not
inject it into a live transcript and claim the model attempted it.

Missing prerequisites: full receiving-source/Elephant identity integration,
all source/store ACL adapters, and every governed provider/hosted route. The
current `native_governed_loop.rs:327` is A2's D foundation, not A1-A8 acceptance.

## Story B: a delegated team routes publication through the gate without gaining authority

Suite: `adr-delegated-publication`. Use an actual mob with worker A, narrowed
helper B, and gate/publisher G. R may request publication to destination P;
A may submit a candidate to G but has no direct P route. G has its own publishing
grant, still bounded by R's authority. Each negative variant gets a fresh mob.

| Checkpoint | Action | Required observations and side-effect oracle | Mode |
| --- | --- | --- | --- |
| B1 Delegate narrowly | A creates B through the actual member tool, gives it a narrowed child grant, and B delegates to a third level. Attempt wider resources, changed U, excess depth, and expired/revoked ancestor use. | Real roster/grant chain records exact parent links; invalid issues create no usable grant/member effect. Existing child use refuses after ancestor revoke. A permitted child operation is observed, not inferred from grant serialization. | D; one L child task |
| B2 Gate route | Worker proposes publication, attempts direct send, then sends the same candidate to G. G publishes for original R. In a separate-executor variant, change candidate/recipient after gate review while reusing its old decision. | Direct and stale-decision P entry = 0; exact permitted G publication = 1. Candidate queue retains original work/requester. G's independent executor grant is used without incorrectly requiring A's direct-publish permission. No claim that G's judgment proves confidentiality. | D, L |
| B3 No bypass through tools or queues | Attempt self-wire, unwire/replace gate, indirect publication queue write, network-capable tool, or unclassified hosted/MCP capability. | Actual topology/queue/sink unchanged for denied attempts. Every declared route has an owner check; missing queue association refuses. Legitimate permitted queue route is a separate positive control. | D |
| B4 Broader orchestration | Repeat the narrowed task through delegate/helper, `fork_off`, session fork, temporary council, remote host and connector hop; test both turn-driven and autonomous modes. | Each actual path retains R/U and conjoins its own executor ceiling. No child/receiver substitutes its human owner's identity. A permitted result returns through the real peer completion path; forbidden peer receives nothing. | D variants; extend existing S92/S93/S96 live positives |
| B5 Schedule and observe | Create a schedule through actual configuration/setup under commissioner R, whose rights differ from the agent's owner O. Run an allowed occurrence, revoke R's relevant permission while O remains authorized, then run the next occurrence. Separately invalidate an observer and attempt history/export as X. | Configuration creates and retains the real service mandate; a missing mandate is not silently replaced by O. First scheduled effect = 1; second refused effect = 0 with local feedback. Subscription owner closes/rechecks only affected access without per-chunk polling or stopping another run; later read/export releases no protected payload. | D, S |
| B6 Admit inbound peers | Send an actual authenticated peer request through local and remote inbound admission. Attempt forged origin, replayed/altered work, and a legitimate peer carrying a requester who lacks the target right. Then send a permitted peer request. | Zero accepted work/sink effects for forged or unauthorized cases. Legitimate request retains original requester/subject and receiving executor, admits once, and produces a permitted response through the actual peer path. Outbound send permission alone cannot satisfy this checkpoint. | D, S |
| B7 Deliver to the actual audience | Queue a frontend/live publication, change destination membership or freshness before the actual send, and attempt a separately unauthorized monitoring copy. Attempt proactive contact with reply-only permission, then with the proper retained mandate. | Stale/unauthorized destination and monitor sink entry = 0; authorized recipient still receives once. Reply permission does not authorize proactive outreach. Rechecks belong to the actual queued-send owner, not each stream chunk or semantic analysis. | D, S |

A maximum five new live requests covers the narrow worker/gate exchange: create
the mob and child grant through real owners in deterministic setup, then give
worker B three requests and G two requests. B attempts direct publication,
submits the harmless candidate to G, then receives its real tool result and
completes normally. G performs the permitted publication and completes normally.
Do not omit either completion request or terminate a worker to meet the budget.
This does not
claim live-model coverage of A's member creation or third-level delegation;
those are B1's deterministic variants. Test a refusing/unavailable gate and exact
edited-candidate binding deterministically; model judgment is not expected to
be a deterministic safety oracle. Existing orchestration tests keep their own
live budgets; governance assertions should reuse those requests rather than
add a parallel swarm of expensive agents.

B3 uses a closed egress inventory: direct HTTP/network, hosted provider tools,
email/calendar/chat connectors, MCP, shell/subprocess/browser, peer/delegate and
remote completion forwarding, scheduled/background sends, and shared memory,
task, blackboard or WorkGraph publication queues. For every enabled category,
name its actual owner and classify it as gate-only or explicitly accepted
ungated access. Attempt each prohibited bypass and record zero destination entry,
with a positive permitted control. An unclassified opaque capability is not a
passing negative case: it remains unavailable or blocks the declared profile.
This tests mechanical routes, not hidden semantic channels on allowed routes.
The `find_and_install` setup flow is a separate executor variant of B2/B3/D6:
the business agent may submit a bounded request, while the independently
authorized setup executor performs installation. Test direct installation and
shell attempts by the business agent with zero entry, then one permitted setup
execution. Returning its result must not grant installer/shell privileges to the
business agent. If this flow is external to this checkout or unwired, its cell
is PendingFix rather than an invented built-in tool API.

Missing prerequisites: governed peering, remote/delegate/schedule propagation,
receiving service policy, mandatory gate topology, and owner-driven subscription
invalidation. The ability to construct a mob or hide a tool proves none of these.

## Story C: account maintenance and restart cannot strand or impersonate accepted work

Suite: `adr-controller-recovery`. Start from a real selected client and the
actual native input, AuthMachine, token store and account policy owners. Use a
loopback provider with exact request counters; a live provider adds no value to
the required interleavings. C1-C3 are separate cases, C4-C7 separate child-process
recovery fixtures. Do not import the old witness/time stack.

| Checkpoint | Action | Required observations and side-effect oracle | Mode |
| --- | --- | --- | --- |
| C1 Last controller maintenance | Accept queued and running work, then use actual public grant revoke, account-policy mutation, route replacement, and direct/coordinated credential-clear APIs. | Invalid last-route removal refuses before changing vault bytes, AuthMachine state, or account policy; alternatively an explicitly authorized atomic replacement keeps work runnable. Subsequent permitted effect and same running RunId prove continuity. Complete unrelated work is unaffected. | D |
| C2 Clear/admission races | Gate the actual token-store clear after generated Release and race admission. Separately race status rehydration, acquisition/save, rollback and cancellation. | Admission cannot capture stale Ready credentials; byte-for-byte vault and full generated state reflect the winning owner outcome. No ready producer revives Released state before storage settles. Assert the invariant, not a required new async permit type. | D |
| C3 End references correctly | Abandon one contributing input mid-run and try archive/revoke; then drive actual RunCompleted, RunFailed or RunCancelled. | The unfinished reference remains until real terminality; direct archive is GuardRejected. After settlement, normal cleanup can archive and ordinary maintenance succeeds. Use actual owner queries, not a mirror counter. Brief native lock contention alone does not become permission denial. | D |
| C4 Commit and restart | Before killing a real SQLite-backed child process, record requester/executor/subject, original InputId/RunId/operation correlation, audit prefix and first effect. Reopen through actual registration/recovery. Separately rebuild runtime storage from supported session continuity, and test missing/revoked/forged custody. | Positive recovery obtains current qualified custody and executes a permitted resumed effect with exact association and prior evidence retained. Preserve the same logical operation correlation when resuming that effect; new physical attempt IDs require the owner's explicit relation, not a fresh ID hiding replay. Negative custody has zero effect. Rebuilt storage must recover exact association or require explicit fresh authorized admission; replay cannot impersonate it. Fresh admission is a separate result, not positive retained recovery. Missing producers remain PendingFix. | D, process boundary |
| C5 Pending audit and uncertain effect | Hold a native commit while appending a suffix; fail commit, retry through owner recovery. Separately lose acknowledgment after a receiver mutation, retaining its original logical operation identity across restart. | Committed prefix is immutable, live suffix not lost/duplicated, same row sink remains attached. Prior effect remains observable and its count stays 1 under applicable idempotency; absent idempotency leaves explicit uncertainty and no automatic repeat. A new OperationId cannot conceal repetition of the same logical effect. Missing newest audit is not proof of no effect. | D |
| C6 Cross-surface continuation | Create through RPC, reopen/read through REST, and continue from CLI or a packaged SDK using the same realm and actual auth. Try X and altered replay data at every handoff. | Exact shared durable association, authorized history, no new requester, one admitted replay identity. Cold registration settles stale Running before archive/nonrehydration; no surviving active controller reference is reconstructed from discarded rows. | S, D |
| C7 Exclusive host ownership | Start two actual processes against the same selected native store. Hold the first owner's real custody, attempt admission/mutation from the second, then release the first normally and let the second recover. | Second process cannot become a concurrent authority or enter the sink. Store bytes/committed rows show no second-owner mutation. After legitimate release and qualified recovery, the second process performs one allowed effect. A fake in-process mutex or different store path is not this test. | D, process boundary |

Operation correlation across C4/C5/D8 applies to the exact resumed logical effect.
Genuinely new effects receive new OperationIds; the test must not force every
post-recovery operation to reuse an earlier identity.

Existing anchors include the local-authorization and controller-custody tests,
the native input audit buffer, and the `smoke_model_fallback.rs` child-process
pattern. The four REDs recorded by the original r7 inspection are historical;
selected repaired controls have passed, as qualified below. They do not establish
all C1-C3 removal and race variants. C4 durable authority restoration and C7
pre-open physical SQLite custody remain unimplemented. The prepared C7 tests are
unexecuted; C4 still lacks its owner-backed restoration API. Copying a MemoryStore snapshot is not process recovery.

## Story D: approve one exact action, then confine its actual process

Suite: `adr-consent-confinement`. An otherwise-authorized operation writes a
disposable report through the real shell/process launch owner. It requires
human consent and an explicit Required confinement profile. A separate ordinary
read remains allowed. D1-D3/D8-D9 use the real approval owner with controlled time;
D4-D7 use actual platform processes, each with its own positive control.

| Checkpoint | Action | Required observations and side-effect oracle | Mode |
| --- | --- | --- | --- |
| D1 Request and deliver consent | Request the exact action; submit a model claim, legacy actor string, and ineligible shared-channel click before an authenticated decision. Deliver the genuine decision after the initiating run completes. Repeat with a host restart while consent is pending. | No write before consumption. Forged decisions leave state unchanged. Presentation identifies action/account/recipient/expiry. The owning session receives typed decision, candidate and InputId references with original R/work; its next actual controller request contains the safe decision feedback. No auto-execution. After restart, lost/stale consent produces explicit fresh-consent feedback and a next request, not reconstructed live authority or silent loss. | D, S |
| D2 Exact fresh attempt | Approve an otherwise-authorized action and explicitly retry the exact retained candidate through its owner with a new attempt. Separately change account, target, recipient, executable artifact or equal-looking reconstructed JSON, and revoke a nonconsent grant. | Positive fresh attempt reaches the exact physical sink once with expected arguments and current permissions. Every changed/lost binding or grant-negative variant has zero entry and typed feedback. Approval cannot widen ceilings or replace a gate. Neither only-negative tests nor a consumed record without a sink effect pass the positive case. | D |
| D3 One-use and failure | Attempt entry while pending, declined, cancelled or expired. In a fresh authorized case, release two valid consumers concurrently. Separately race cancel vs consume and inject observation failures before/after consumption. | Ordinary refusals have zero body entry, safe feedback and a later allowed read in the same RunId. Two-consumer positive: exactly one successful consumption and exactly one physical sink effect; the loser refuses. Cancel race: either cancel wins with zero effect or consume wins with one, with matching owner state. Pre-consume infrastructure failure preserves unconsumed approval; post-consume stays consumed. Preserve completed siblings and follow existing engine infrastructure policy. No automatic retry. | D |
| D4 Basic OS boundary | Launch allowed shell command, then actual out-of-root read/write, symlink/rename/hardlink escape, sparse inherited FD, secret environment, forbidden socket/TCP/UDP and listener probes. | Positive command and permitted network control succeed. Forbidden receiver/file effects = 0 and real OS denial is observed. Child exit/output is retained, not classified by stderr keywords. Helper preserves required process custody/PID and no extra supervisor appears. | O |
| D5 Shared bootstrap and agent isolation | Concurrently launch agents with disjoint roots. From helper B's profile try replacing helper A or an ancestor, using existing/new hardlink or symlink; then execute A's already-bound and next launches. For a detached/background worker, observe one permitted effect, revoke/narrow before its next effect, then release the test barrier. | Cross-agent and cross-helper writes = 0; subsequent A remains confined. Detached first effect remains recorded; second prohibited effect = 0. Actual owner stops/replaces the affected worker or mechanically blocks further operations, without cancelling unrelated work. In-memory invalidation alone cannot revoke a running child's OS rights. | O |
| D6 Launch families and unsupported profile | Repeat via foreground/background shell, recovered monitor, local MCP, command hook, skill source and package-install child. Exercise declared broker/device/browser endpoints. Request unavailable backend or unsupported exact endpoint. | Same boundary at actual child entry, no ambient credentials/control FD, no unconfined fallback. Setup reports required unsupported capability explicitly; a supported later launch succeeds. TrustedHost remains a separate explicit positive path. | O, S |
| D7 Preserve Required through configuration | Round-trip a Required configuration through its actual stored format, reject unknown nested fields/reserved payloads and invalid paths, then reopen a job under a different current host configuration. | Required never becomes TrustedHost or an empty policy by decoding. Malformed or mismatched recovery setup refuses before spawn, with zero child entry. The matching configuration still launches its allowed command and denies its forbidden write. | D, O |
| D8 Consent is not effect reconciliation | Starting from C5's receiver mutation with lost acknowledgment, obtain fresh legitimate consent and explicitly try the same logical effect again. | Consent alone cannot settle prior uncertainty or authorize a blind duplicate. The real operation owner must reconcile or apply its actual idempotency contract; receiver mutation count stays 1. Otherwise the new attempt refuses with explicit uncertainty. A separate unrelated permitted effect still works. | D |
| D9 Eligibility and relevant change | After requesting/approving, revoke the human's eligibility and attempt consumption. Test self-approval under a policy that forbids it and an explicitly permitting policy. Separately change relevant policy, then unrelated policy, while consent is pending. | Revoked/forbidden approver and invalidated exact binding produce zero effect with feedback. Explicitly permitted self-approval and unrelated changes retain a valid owner-proven candidate path and execute once after current checks. Relevant change needs fresh consent when the binding no longer holds. Use actual owner observations, not a new epoch or a blanket all-change invalidation rule. | D |

Owner tests currently exist in `approval/action_tests.rs`, and actual macOS
probes in `meerkat-sandbox/tests/{process_confinement,bootstrap_integrity}.rs`.
They are prerequisites, not proof of an end-to-end shell consent workflow.
Late authenticated delivery, exact later-attempt relation and sink consumption
are still needed. D9's unrelated-change positive is a required UX/test outcome,
not a claim that current consent survives coarse publication invalidation. If
the owner cannot prove unchanged exact candidate and current approver eligibility,
that positive cell stays PendingFix. Repair the actual owner integration, not a
browser exception or a new scoped-generation registry; a relevant candidate edit
still requires fresh approval. Bootstrap cross-profile protection and some network cases
remain open. Linux must test both its non-userns subset and namespace/bwrap mode;
an explicitly selected container boundary needs its own declared guarantees.
Windows unsupported behavior is a testable contract, not a claim of Windows
confinement. Do not coerce exact IP endpoints into wider localhost permissions.

Run real OS variants in a required release companion job, separately from
unqualified BuildBuddy workers and live-provider shards. The job name and wiring
are implementation work; its pass for the same source/artifacts must be required
alongside Turbo S. Preparation capability failure is not a passed blocked-escape
test: the corresponding allowed command must first execute successfully.

| Release companion cell | Executor qualification and required evidence |
| --- | --- |
| Linux common host | Intended GitHub Ubuntu ABI 7 executor: probe actual kernel, Landlock ABI/access bits, seccomp and userns/bwrap availability. The dedicated disposable job configures `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` for its namespace/bwrap cell, then verifies actual launch; never mutate a developer host for this test. Exercise the supported non-userns subset separately. A runner label is not capability evidence. |
| Linux expanded profile | Dedicated VM expecting ABI 8: independently probe actual ABI/features and run the requirements ABI 7 cannot satisfy. Missing ABI/access bits block this advertised cell, rather than silently weakening restrictions. |
| Linux container | Explicit container boundary, image/runtime configuration and permitted userns mode. Prove the declared boundary and child ownership; host Linux tests do not stand in for it. |
| macOS Seatbelt | Qualified native Mac with working helper, Seatbelt, same-user installation protection and positive command/network controls. Generic macOS BuildBuddy availability alone does not qualify the sandbox. |
| Unsupported OS/profile | Explicit negative setup tests on Windows or another unimplemented profile; they cannot satisfy any positive advertised isolation cell. |

Record which executor proves each D4-D7 capability: filesystem/exclusion/link and
ancestor safety, network/Unix socket policy, environment, descriptors, process
custody/termination, compiled-profile reuse, bootstrap protection and configuration
recovery. A capability needing a stronger executor is assigned there explicitly;
no catch-all skipped test converts that gap into a release pass.

## Story E: failed observation stays infrastructure, without losing sibling effects

Suite: `adr-infrastructure-projection`. Use the real native loop and real HTTP
provider adapters against deterministic recording servers. Independent reset
points select the failing observation and sibling order. Feed resulting typed
events through actual SDK decoders and the console.

| Checkpoint | Action | Required observations and side-effect oracle | Mode |
| --- | --- | --- | --- |
| E1 Policy control | Refuse one action normally, permit a sibling, and let the controller continue. | Typed Refused becomes ordinary feedback in the same RunId; forbidden body = 0, permitted effect = 1, next controller call occurs. This distinguishes the E2 infrastructure case. | D |
| E2 Before-entry audit failure | Fail required Prepared/Entry append, or Refused append during a final current check after an auth await. Repeat both synchronous sibling orders and callback/barrier variants. | Typed ObservationUnavailable survives. Blocked HTTP/tool body = 0; healthy completed sibling retained exactly once. Do not convert it into permission feedback or a policy-driven retry/fallback. Pin the expected typed infrastructure disposition from the configured engine contract before running and assert that exact value; do not infer the expected result from what the implementation happened to return. This design adds neither global shutdown nor policy-driven retry. A separately admitted concurrent session or work item still executes its permitted operation, proving the staging failure does not refuse unrelated work. | D |
| E3 After physical result | Receiver mutates once and returns success (or definite failure), then outcome recording/settlement fails. | Preserve exact physical result plus ordered settlement diagnostic; no result relabeling, no automatic duplicate, no claim that dispatch acceptance proves final physical completion for a deferred operation. | D |
| E4 All error consumers | Route E2 through provider-native web search, built-in dispatcher, member upcall, external runner, live host and fallback. Serialize contradictory Retryable metadata with the infrastructure kind. | Primary infrastructure stays nonretryable and distinct; no tool-feedback laundering. Companion order survives where supported. Explicitly record the deferred post-effect live companion gap rather than claiming full E3 live coverage. | D |
| E5 SDK and console | Consume the same real event through Python, TypeScript, Web SDK/raw WASM projection and console rendering; attempt unauthenticated observer/read. Feed B7's queued audience change and unauthorized monitor-copy variants through the actual frontend/live host. | Wire discriminant, safe reason and retryability agree. UI distinguishes Permission denied from Unable to record audit event; neither grants authority. Private fields are absent from public output. Actual destination/monitor receiver evidence proves enforcement, not merely absence in UI. | S, D |
| E6 Export loss | Overfill or fail the optional MobKit event exporter while a native operation/audit commit succeeds. | Native authoritative record remains; export loss is visible. Export failure does not stop the operation, become required-audit failure, or fabricate native persistence. | D, S |

Existing foundations include `native_governed_loop/observation_deferred_siblings.rs`,
the three provider `authorization_tests.rs` fixtures, `core/event.rs`, and the
reviewed SDK generator and console projection tests. An interrupted callback
whose persisted post-tool suffix hits existing NoPendingBoundary is a recorded
recovery gap; it must get an explicit acceptance disposition, not disappear into
the ordinary callback success count.

## Feature and surface coverage matrix

The cells below name variants that must exist and execute. They do not claim
current coverage. Implementation records should use the existing
`tests/integration/src/coverage_matrix.rs` vocabulary:
Covered only with an exact executed test, PendingFix with a blocker,
Gap with a reason, and Impossible only for a justified contract exclusion.

Recorded evidence as of 2026-10-03 remains prerequisite coverage. The repair
later committed as native `141aad15` passed 49 selected integration tests and
six library controls: `oauth_resolve`, machine/metadata/ordinal contracts,
`core_apply_terminal_truth`, selected `cross_host_flows` and `fork_off_surface`
cases, `b1_live_join_*`, and `detached_delivery_follows_runtime_presence`.
The earlier 13-test acceptance batch included
`governed_jsonl_refusal_retains_actor_seed_and_context_then_same_run_read_finishes`,
bundle ownership, RPC credential continuity and wire readiness. Its JSONL wire
flow uses memory stores and a local HTTP model fixture; it proves neither disk
JSONL persistence nor restart or live-provider coverage. Test-body durations are
not authorization overhead measurements. Native `12fe6077` includes the repairs
and regenerated outputs; its seventh normal push stopped on a test-helper
`clippy::implicit_clone` lint, with no successful publication, remote CI or cost
acceptance for this candidate.

Console `103540e8` includes passing `authorization-feedback` and three
`checked-save-*` mock-browser contracts. `real-checked-save-recovery` passed on
Console `9762e89b` using a recorded earlier `console_acceptance_fixture` binary:
its build log and matching Rust/Cargo inputs exist, but its historical binary
digest does not. This is Console access-owner evidence, not current native
readiness or C4 recovery evidence. The unfiltered Console browser suite remains
unexecuted because three required native reference binaries were unavailable.

All 37 checkpoints (A1-A8, B1-B7, C1-C7, D1-D9 and E1-E6) and their advertised
surface/platform variants remain required. These focused results do not execute
the five proposed Turbo S suites. Full implementation review, OS/helper launch
coverage, durable recovery and the measured cost gate remain outstanding.

| Required feature or surface | Exact checkpoints/variant | Current prerequisite or gap |
| --- | --- | --- |
| Qualified identities, auth vs account, represented subject, idempotency | A1/A4/A7-A8, C2/C6 | Real ingress/authenticated association, actual mailbox callback and interleaved R/X work; native batch conjoins every original. |
| Root/child/third-level narrowing, expiry, revocation, correlated rules | A2/A4, B1 | Generated grant owner exists; cross-agent propagation required. |
| Same-model refusal, healthy sibling, same-run continuation, queued work | A2/A3, E1 | Native scripted and governed memory-backed JSONL wire controls passed; full A2/A3/E1 variants remain required. |
| Anthropic, OpenAI, Gemini text | A2 once per family; A6/E2/E4 adapter faults | Real standard credentials plus actual governed adapter selected. |
| Compatible/self-hosted, Copilot and cloud backend/account variants | A4/A6 route-specific variant, existing active provider smoke overlay | Every advertised backend needs its own real positive route and deterministic negative send oracle; common adapter alone is insufficient. Missing credentials block that advertised cell. |
| Hosted search/code/image, image-generation executors | A6/E4; governed overlays on existing S74/S76/S77/S79/S80/S82/S90 | Conservative capability checks and actual hosted request inspection; provider action may be opaque. No finer effect claim than provider contract supports. |
| Compaction, memory curator, history/blob hydration/export | A5/A6 request-usage variants | Actual non-main-loop clients and resource ACL paths, not main Agent-only tests. |
| Tools including shell, apply_patch, network/browser and MCP | A2/A4; D4/D6 | Enumerate each actual dispatch family and observe its physical target. |
| Elephant query/fetch/write and external broad OAuth connector | A5 plus A2 calendar read/delete | Real isolated Elephant service and connector receiving-policy integration required; mocks do not close it. |
| Comms, delegate/helpers, fork_off, fork, council, remote placement | B1-B4/B6; existing S21/S92/S93/S96 | Both autonomous and turn-driven variants, inbound authentication as well as outbound permission, exact original work retained. |
| Wire/unwire/spawn/retire and publication queues | B3 | Actual topology/store mutation guards; no UI-only denial. |
| Schedules/connectors including HostRunnable | B5/B7/D6 | Config-created commissioning mandate, commissioner distinct from agent owner, per-occurrence check and proactive-vs-reply permission. |
| Session read/subscribe/monitoring and live publication | B5/B7/E5 | Actual observer auth and queued-send audience freshness, independent monitor-copy denial, no per-chunk or physical-room claims. |
| Independent service activation and agent frontends | B7 service-lifecycle variant | PendingFix: pausing/removing an agent frontend leaves an independently active shared service available to its authorized audience; removing the service does not widen another service or release shared credential/package restrictions. Exercise actual separate owners and authorized positive controls. |
| Separately authorized setup executor | B2/B3/D6 `find_and_install` variant | Business requester never inherits installer/shell permission; actual integration location and recipient owner are PendingFix until wired. |
| Credential/controller admission and all removal APIs | C1-C3 | Selected controller/custody controls passed; full removal-API and clear/status/acquire/storage race acceptance remains pending. |
| Persistent/detached operation and actual process recovery | C4-C7, D5/D6 monitor | PendingFix: C4 durable authority restoration and C7 pre-open physical SQLite custody are unimplemented; prepared C7 tests are unexecuted. Positive restart and runtime-store reconstruction must retain logical effect correlation and audit prefix. Detached revocation still needs actual entry/worker enforcement. |
| Approval freshness, authenticated decision, late delivery, one-use | D1-D3/D8-D9 | Positive approved sink and concurrent consumption, pending host restart, late typed input/next request, current eligibility and relevant/unrelated change; physical integration remains outstanding. |
| macOS/Linux confinement, non-userns/container modes | D4-D7 platform/configuration variants | Actual capabilities, bootstrap integrity and all launch families must pass; Required cannot silently downgrade. |
| Windows/remote/browser unsupported OS requirements | D6 capability-negative variant | Explicit unsupported is correct only for unadvertised/unsupported profile; never a positive isolation claim. |
| Required audit vs optional exporter, outcome/uncertainty | C5, E2-E6 | Native audited row and error foundations exist; durable/live recovery gaps explicit. |
| Rust embedding, CLI, REST, RPC, MCP protocol | A1/A2/C6/E5 surface variants | Reuse S16/S23/S25/S26/S27/S31/S49-S53 and Rust SDK suites; assert governance is actually installed. |
| Python and TypeScript packaged SDKs | C6/E5 | `sdks/python/tests/test_e2e_smoke.py` S38/S39 and `sdks/typescript/tests/e2e_smoke.test.mjs` S43/S44. Fresh generated wire artifacts required. |
| Browser/raw WASM vs hosted browser client | A1/A6/B5/B7/E5 | Separate S47/S48 raw exports and packaged SDK variants; server-only enforcement does not prove browser execution. Browser fetch controls/capabilities remain explicit. |
| Live text/audio, interruption, backend switch and playback | A6/B5/B7/E4/E5 overlays on S71/S72 and S97-S107 | Reuse prerecorded audio and existing calls; observe real live send/tool boundary and typed lifecycle. Text fixture is not audio evidence. |
| MobKit console and domain policy | E5/E6, B3/D1 controls | Mock-browser feedback/checked-save controls and qualified real Console checked-save evidence exist; full E5/E6/B3/D1 decoded-event and authenticated-action variants remain pending. Rendering is projection only. |
| Default/trusted mode, mandatory gate, cheap checks | B2/B3, D6, cost gate below | Explicit profile inventory and measured costs remain pending; no accepted overhead benchmark. No semantic provenance tests. |

Before running, enumerate the advertised profile from real provider capabilities,
enabled tool/launch inventory and registered surfaces. Map every entry to a row
and exact test variant. Unknown/unmapped active entries fail acceptance. Existing
Turbo S scenarios passing without governed context count only as compatibility
controls. Do not silently limit the inventory to the five new test functions.

## Cost and scheduling budget

These are proposed limits, not measured results. Separate build/materialization,
test setup, operation work, and optional gate-model costs in evidence.

| Group | Added live-call ceiling | Warm execution target / hard timeout | Setup policy |
| --- | --- | --- | --- |
| A, three standard families | 4 per family, 12 total; <=2048 output tokens/request | <=120 s/family / 240 s | Small synthetic context, <=8 KiB per request excluding schema overhead; no 650k-token fixture. |
| B gate/team core | 5 total, <=2048 output tokens/request | <=120 s / 240 s | One small mob; no repeated live calls for every negative delegation variant. |
| C recovery/races | 0 | <=60 s / 180 s | Local HTTP fixture, bounded child process and small real SQLite DB. |
| D consent/OS | 0 | <=60 s/platform / 180 s | Prebuilt helper and approved disposable roots; isolated Mac/Linux executor. Browser/device setup cost separate. |
| E infrastructure/projection | 0 | <=60 s/native / 180 s; browser <=120 s / 240 s | Reuse prebuilt RPC/browser artifacts and local server; no provider API needed for faults. |

New live budget is at most 17 provider requests per complete attempt, or 34 if
the existing outer shard retry reruns every live checkpoint. Count transport/auth
retries in the budget; do not hide them as a single logical call. Exceeding it is
not permission for more calls or weaker assertions: stop and classify the
unreached checkpoint from its actual provider/model evidence above. A broken
budget limiter is itself a harness failure.
Existing live/image/audio scenarios keep their own budget and reuse requests for
governed overlays. Those overlays inherit the host scenario's void rule and add
no new requirement for the model to choose a particular action; their added
assertions concern the actual governance boundaries already exercised. Report total suite consumption separately from this increment.
Do not substitute a cheaper model if that changes the route under test. The
declared Sonnet forced-fixture exception above names its own route in advance;
it does not replace Opus or other advertised model coverage.

Use the lowest reasoning effort supported by the actual selected route through
its existing typed knobs. For example, the inspected OpenAI tag carries
`ReasoningEffort`, with supported values checked by request lowering; do not send
a made-up `minimal` value. A known nonreasoning path may use a 512-token cap, but
reasoning-capable routes get up to 2048 so the test does not manufacture missing
tool calls by truncation. Record selected effort, token cap, finish reason and
actual billed usage. No reasoning-content capture is required.

Build shared Rust artifacts once through the existing selected-plan materializer,
use the generated runfiles/manifest, and build the browser WASM bundle once.
No per-case Cargo, npm install, fresh audio synthesis, external witness service,
or repeated source hashing on warm entry. The existing 12/6/2 smoke scheduler
limits are aggregate defaults, not a license to run 17 live calls concurrently;
cap the new live fixtures at two active requests per provider, locally within
their test. Use barriers instead of timing sleeps for races. Cleanup owns child
processes, listeners and fixture tenants even on assertion failure.

Keep heavy OS/device compatibility in the required qualified release companion
job and long media runs in their existing platform shards. Their result must
still be required for an advertised capability before release; moving them out
of the short shard does not turn absence into success.
If setup lacks a selected required capability, fail the named checkpoint as
blocked with the exact reason. An explicitly unsupported capability gets its
own successful refusal test and remains excluded from advertised support.

The ADR budget is **<1 ms p99 added work per model/tool operation and <=10 percent
per representative turn**, including preparation, attribute lookup, invalidation,
recording, allocation and lock time. A short smoke cannot establish this tail.
Add a zero-live-call structural guard to the existing cost suite: no extra network
requests/fsync, no per-chunk policy evaluation, no whole-history scan/hash/clone,
no repeated success logging on each unchanged warm check, and no busy polling
while idle. Preserve the required operation Prepared, Entry and outcome
observations at their real boundaries; this budget does not remove those audit
records. Measure matched
trusted/governed modes separately on a quiet host with 1/16 concurrency,
1/100/1,000/10,000 relevant entries, warm/cold/invalidation cases and audio/stream
paths. Preserve p50/p95/p99, CPU, allocation, lock and I/O counters. A recorded
cost-gate pass on the exact candidate is required; unrelated provider latency
must not mask authorization overhead.

## Implementation and review order

1. Close the already-executed critical REDs and audit checkpoint first. Add A2 D
   and E controls to the new prebuilt integration artifact without changing their
   owner semantics. Verify the harness fails missing prerequisites, zero tests,
   missing checkpoint records and a deliberately disabled sink guard.
2. Integrate the separately landed typed ToolChoice through the real native
   request owner and verify named control on the provider wire, then implement
   A's first real provider path and its other standard families. Require exact
   tool request, same-controller feedback and receiver oracles;
   distinguish objective model/provider voids from invariant failures and require
   at least one complete valid run for every mandatory live cell.
   An enabled allow-all host or absent WorkAuthorizationContext must fail setup.
3. Wire C only after real controller and persistent owners are ready. Wire B,
   D and surface variants as their production paths land; each remains PendingFix
   until its exact end-to-end checkpoint executes. No test-only authority adapter
   may stand in for a missing production ingress, grant, vault or consent owner.
4. Run the advertised capability/surface inventory in the active governed profile,
   compatibility controls in explicit trusted mode, required qualified OS companions and
   the separate measured-cost gate. Review failures checkpoint-by-checkpoint and
   preserve both attempts under Turbo S's retry policy.

Root reviews this bounded proposal, then sequences implementation through the
existing source ownership and build windows above. No additional user approval
step is proposed. Coverage claims require the actual checkpoint evidence;
implementation must not introduce a new policy registry, lifecycle, transport
stack, semantic provenance mechanism, or security-specific parked run.
