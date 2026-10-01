# ADR-001 amendment: local permissions and explicit disclosure limits

## Status and scope

Candidate r8, 2026-10-01. Luka has explicitly clarified the default: cheap local
checks, normal refusal feedback to the model, full execution-mode coverage,
mechanical tool/source and peering permissions, information-handling instructions,
and an optional gate agent. Automatic semantic provenance tracking and
whole-context information-flow confinement are outside this decision.

This candidate supersedes conflicting requirements in earlier candidates,
[ADR-001](adr-001-runtime-security.md), the [profile companion](governed-deployment-profiles.md)
and the [acceptance plan](adr-001-implementation/acceptance-plan.md). Earlier
frozen reviews and source remain historical evidence for their exact scope.
This is a design candidate, not implementation or performance acceptance.

### Supersession of r4-r6

| Earlier requirement | Disposition in this candidate |
| --- | --- |
| Authenticated requester/actor/executor, exact work association and narrowing grants | Keep as authority for observable operations, including peer and schedule hops. |
| Exact tool, source, account, provider and peer checks; local invalidation | Keep at the existing operation owners. |
| Semantic envelopes, transitive dependency propagation and whole-context confinement | Remove. They cannot establish semantic non-disclosure and are not default infrastructure. |
| Memory restriction propagation, compaction partitions and legacy source adoption | Remove. Actual stored-resource ACLs remain with the resource owner. |
| Source leases and trusted-adapter retained-copy contracts for semantic propagation | Remove. The source's own enforcement and stored copies' actual ACL/retention policy remain. |
| Source-to-model projection, `PolicyWithheld` rewriting and semantic cache purging | Remove. Check the actual configured provider/model/account; use ordinary context. |
| Per-chunk subscriber gating and physical-room audience binding | Replace with authenticated session read/subscribe admission and actual communication/tool destination checks. |
| Gate approval as evidence of confidentiality | Replace with mechanically mandatory gate placement and explicitly fallible model judgment. |
| Audit as complete provenance or pre-effect crash proof | Replace with observable operation records in existing commits and explicit uncertainty. |
| External witnesses, authenticated time and human-gated security recovery | Remain parked as separately selected future assurance work. |
| Local feedback, last controller route, full coverage and measured cost | Keep, with no semantic-information-flow acceptance test. |

## What the system guarantees

Meerkat mechanically enforces who may execute an operation, access a particular
tool or information source, and communicate with a particular peer or destination.
It attributes those observable operations to authenticated actors and retained
work. These guarantees are enforced at the shared owners, across all surfaces.

It does not claim to track the meaning of information through an LLM. An agent
can read an allowed Elephant document and reproduce its meaning in a limerick,
a summary, a tool argument or another message without retaining any reference.
Document IDs, citations, labels and dependency metadata do not prove that such
an output contains no protected information. Once information enters a model's
context, this design provides no mechanical guarantee against its later semantic
disclosure to an otherwise permitted destination. Luka explicitly accepts this
limitation.

Applications use information-handling instructions and may require a gate agent
to review particular outgoing actions. The runtime can enforce that the gate
was consulted for the exact action and that its decision was honored. Whether
the content is appropriate remains model judgment. Neither instructions nor a
gate are described as complete confidentiality enforcement.

The host process, OS, configured identity issuers, policy owners, storage and
registered tool/provider adapters are trusted. This is not a sandbox against
malicious native plugins or a compromised administrator. A deployment needing
stronger isolation must keep information out of an untrusted agent/context or
separate agents, stores and permitted routes at actual boundaries; attaching
metadata after disclosure does not create isolation.

## One mechanical authorization path

`local-governed-v1` is the first governed profile. `trusted-embedded` remains an
explicit host choice. A governed refusal never falls back to trusted execution.
Request bodies cannot select the profile or install authority. All providers,
streaming, live/voice, compaction, memory, comms, schedules and supported native
and WASM surfaces remain in scope. A slim fixture is an integration milestone,
not the delivered feature boundary.

The existing native work/input owner retains one immutable association: qualified
requester, authenticated ingress actor, logical executor and target, original
work/authentication references, authority basis, restriction ceiling and selected
host contract. Recorded source references, when available, are audit observations,
not semantic taint or permission. Authentication and original provenance cannot
be rewritten by restart, reauthentication, replay or a console projection.

Shared ingress adapters authenticate the real actor. An agent, service, schedule
or connector can act under an explicitly issued scoped mandate; no human identity
is invented. The existing schedule/connector owner retains the mandate and its
commissioning actor and checks current authority for each occurrence. Provider
API credentials establish access to the provider account, not the authority of
the user or agent to perform an application operation.

Keep the acting agent, actual requester, represented user when different,
external account/credential binding and explicit delegation scope distinct.
Tool configuration selects available connections and baseline agent capabilities;
possession of a connection never activates every permission of its human owner.
Each work item retains its actual delegation, and each operation must satisfy
the acting agent's permissions, that delegation and the actual source/account
policy. A shared agent does not acquire Luka's authority merely because he owns
it when someone else asks it to act. For example, Luka's calendar assistant may
read availability in selected calendars while event deletion remains forbidden,
even when Luka and the OAuth token can delete those events.

For Elephant and other services we control, the receiving service authenticates
the acting agent and represented subject together with their scoped delegation
and enforces the requested operation there. For an external API that recognizes
only the user's OAuth account, the trusted connector retains the credential and
enforces the narrower action/resource/account grant locally before use. Raw
credentials stay outside model access. Audit retains agent, requester,
represented user, delegation and actual external account; it does not claim that
an external API distinguishes identities absent from its protocol.

[RFC 8693's delegation vocabulary](https://www.rfc-editor.org/rfc/rfc8693.html#section-1.1)
distinguishes actor from subject. It is a vocabulary and interoperability option,
not a requirement to add token exchange or a network round trip to each action.
Existing `ActingOnBehalfOf` and pre-materialization auth-binding checks are
foundations; permission to materialize a credential is not an operation grant.

Bind the association to the actual `InputId`, including its qualified idempotency
scope and exact content/association comparison, in the existing transaction.
In-memory work has process-lifetime custody unless its host retains it normally.
Restore/reseed preserves the association or performs fresh authorized admission;
a message replay cannot impersonate missing admitted work. No side ledger,
requester registry or mutable permission mirror is introduced.

Native batching retains every contributing association and conjoins their
ceilings. It cannot union permissions, choose the first contributor's identity
or mix incompatible qualified requesters. Different event/authentication
references alone need not prevent otherwise compatible work. This preserves
mechanical attribution and authority; it does not prove semantic independence
between content that shares an agent context.

The feature-owned policy composition reads the canonical identity, grant,
resource and destination owners. A resolved operation includes the actual tool,
arguments/resource target, executor, requester, provider/account where relevant,
and recipient/channel. A host may declare a conservative whole-tool/server/account
contract when no finer resource mapping is available. A request requiring finer
controls needs trusted metadata or enforcement by that actual source/provider;
model-authored arguments do not establish resource attributes or authority.

Delegation only narrows the parent's effective permissions, including mob spawn,
helpers/subagents, delegate, `fork_off`, temporary councils and session fork.
At issue and use,
the canonical grant owner checks the complete current ancestor chain, issuer,
grantee, scope, lifetime and delegation depth. Historical validation is not a
reusable permit. Correlated ABAC rules stay correlated; separate indexes must
not invent an action/resource/recipient combination that no rule permitted.
An administrator's or tool host's credential never substitutes for the requester.
Peer requests retain the actual originating requester and conjoin its authority
with the receiving agent's permissions. A receiver cannot turn an otherwise
denied request into its own more privileged operation. A scheduled occurrence
uses its retained commissioning authority, not whichever agent happens to run it.
Connector-mandated routing chains retain the scoped service mandate across their
hops; deployments must explicitly permit their intended routing and notifications.

Prepare an immutable private decision for the exact resolved operation. Retain
it with that operation's real request/plan. The final local check verifies the
same binding, the current coherent policy generation and its deadline after
relevant preparation waits and immediately before the protected action. A
changed target, argument binding, account, peer or route requires fresh evaluation.
The decision is disposable, nonserializable and never a bearer token.

Canonical owners publish relevant local identity, grant, resource and route
changes under one coherent invalidation ordering. The stamp owns no permissions.
No policy lock is held across network or tool execution. Work already authorized
before a concurrent revoke may be in flight; later checks observe the new local
generation. An established session subscription is authorized at subscribe/read;
this decision does not introduce a separate permission check on every chunk.
When a relevant owner invalidation changes that permission, the subscription
owner rechecks and closes the affected subscription. This is event-driven and
does not terminate the agent's run or session. Already released bytes cannot be
recalled; no per-chunk atomic revocation guarantee is claimed.
Restart discards prepared decisions. Remote systems retain their own enforcement
and propagation guarantees; a local generation does not prove remote currentness.

## Shared enforcement boundaries

| Boundary | Mechanical contract |
| --- | --- |
| Ingress/native admission | Authenticate the actual actor; authorize the target and operation; retain exact work, mandate and replay identity. |
| Tool/source access | Check exact action and resolved resource/server/account at invocation or hydration. Visibility is not permission. Preserve source-owned ACL checks. |
| Model/provider operations | Check the configured model/backend/account/destination and hosted capability. A permitted model receives its ordinary agent context; no semantic source-to-model taint system is added. |
| Peer communications and delegation | Check the actual authenticated sender/requester, logical executor, recipient and action. Apply the same rule to helpers, delegated agents, replies, handoffs and monitoring copies. |
| Session observation | Authenticate and authorize session read, history/export and subscription admission, including monitoring copies. Enforce current owner policy at the next operation check; do not add per-chunk gates. |
| Communications and publication | Check the actual sender/requester, destination, account and action at each send or publishing tool invocation, including live paths. A route label is not a semantic confidentiality guarantee. |
| Stored history, memory, blobs, export and audit | Enforce the owning store/resource's reader and writer permissions. An ID or shared blob hash grants no access; search and hydration must not bypass the declared resource ACL. |
| Schedules/connectors | Check the retained service mandate and current target permissions for each real occurrence. Timers and credentials grant no application authority. |

These checks apply at existing shared seams, including provider-native tools,
image/search executors, compaction/curator calls and live paths. Native tools
include shell, apply_patch, wire/unwire/spawn/retire member and `meerkat_schedule_*`;
none escapes through a path outside the callback bridge. Adapters provide
actual typed facts; they do not own policy. Delegation through another client
must retain the operation's authority context. Redirects, retries or queued
commands cannot silently change the permitted destination or bypass a final
check. Use existing transport controls and provider contracts; there is no
replacement security transport.

For opaque hosted capabilities, authorize the declared conservative unit before
enabling it. A required finer rule that the provider cannot enforce refuses that
capability, while eligible model work continues. Do not claim that inspecting a
returned result prevented a server-side action that already occurred.

Peering is a real permission relation, not a console filter, display name or
unsigned sender string. Each send and spawn checks current permissions. Wiring
and topology mutation are authorized owner operations; an agent cannot grant
itself a peer or publishing route. The source application or channel owner
retains membership and history access rules. Once a permitted peer receives content,
its later behavior is governed by its own permissions and instructions; this
ADR does not claim to prevent semantic relaying through permitted peers.

## Instructions and optional gate agents

Hosts provide information-handling instructions appropriate to the agent's role
and connected sources. Prompts and model decisions cannot widen a mechanical
permission. Instructions can guide discretion but are not a security proof.

A host may require a gate for selected sends, publications or tool actions.
Prefer ordinary permission topology: only the designated gate agent may reach
the declared external destination or publishing tool. Other agents can submit
candidates to it but cannot publish directly, rewire their own permissions, or
delegate to an unguarded publisher. Its identity and permission to inspect a
candidate are explicit. If a separate executor consumes a gate decision, bind
that decision to the exact candidate action and recipient; changing either
requires a new decision. A missing, refusing or unavailable required gate refuses
only that action and returns normal feedback to the originating agent. Optional
means the deployment chooses whether to install it; once required for an action,
it cannot be skipped.

Declared gate topology also covers indirect routes through shared stores. An
ungated agent cannot write a memory, task, blackboard or WorkGraph resource that
an unguarded publisher uses as an authorized publication queue. Such a route must
pass through the gate or be unavailable under the deployment's mechanical ACLs.
This is configuration and operation enforcement, not automatic discovery of
semantic flows between arbitrary data. The publishing operation independently
checks the exact agent, original requester and destination; a gate verdict grants
no extra authority.

A gate-requiring deployment declares a closed inventory of egress capabilities:
network-capable shell/HTTP/browser/code tools, outbound MCP servers, hosted
search/code/image capabilities, schedule `HostRunnable` targets, external peers
and live output, together with model/provider connections. Each entry is either
gate-only or explicitly accepted as ungated. Unclassified capabilities are
unavailable to non-gate agents. General execution tools require a trusted bounded
host contract or are classified as a whole capability; inspecting an arbitrary
script cannot establish its eventual destinations. The mandatory-gate guarantee
is relative to this declared inventory. It does not discover covert semantic
channels or certify content sent over explicitly accepted ungated connections.

Retain the original requester and work on an explicitly declared publication
queue item. A publisher refuses a queue item missing that required association;
it cannot adopt the writer's request as its own. This is attribution of a queued
operation, not semantic provenance of arbitrary stored text. Keep requester
authority separate from each executor's permissions: agent A may be allowed to
submit a candidate, gate G may independently be allowed to publish it for
requester R, and A may still be forbidden to publish directly. The gate cannot
override R's restrictions, but A's direct-executor ACL is not incorrectly
substituted for G's own authority on the declared gated route.

The gate's decision and observed action are auditable. No gate reply grants new
tool/source/peer permissions or overrides the policy owner. Gate review may add
model calls, latency and cost; report that separately as the explicitly selected
application workflow, never hide it inside the cheap default authorization cost.

## Elephant and existing applications

Elephant remains authoritative for its resource attributes, ABAC and query/fetch
permissions. Its existing authenticated request path must receive the actual
requester/delegation contract or a deliberately scoped service mandate, and keep
its own enforcement before returning protected data. Meerkat separately checks
permission to invoke that source/tool and to communicate over its permitted routes.
Shared qualified identities, operation/resource vocabulary and conformance cases
connect the two systems; neither imports the other's policy database.

A successful Elephant read is an observable authorized access, not a proof that
later model output is safe. The default does not propagate document labels through
summaries, memory, compaction or arbitrary generated text. Revoking source access
prevents later governed reads under the source's actual contract; it cannot make
an agent forget content already read. Existing stored copies have their own
resource access/retention policy, without pretending that semantic recall was
revoked. Existing histories need no fabricated provenance or blanket adoption
process merely to remain usable.

MobKit supplies domain policy and projects authorized state through its console.
It does not maintain a parallel runtime permission truth. HomeCore-style gate
workflows remain application compositions over the common enforced route.
HomeCore's current instruction-only gate is not evidence that mandatory routing
already exists. Its callback approval path and native tools need the same shared
checks, including topology mutation and selected-account binding. A human
approval, when an application requests one, must derive from the approver's
authenticated input rather than an agent's claim that the person approved.
The existing approval owner binds it to the exact action/recipient, with expiry
and single-use consumption; an edited or replayed action needs new approval.

## Refusals, configuration and audit

An ordinary denied, expired or unresolved operation returns typed audience-safe
feedback to the agent, like a tool result. The agent can choose another action;
allowed siblings and the turn continue. No `FatalFailure`, `RunErrored`, session
hold, teardown or security-specific parked state is synthesized from that denial.
Affected resources are released and affected waiters receive their ordinary
operation result. A completed physical effect is never relabeled failed merely
because its bookkeeping or audit settlement later failed; retain both facts.

Refused model/route changes leave the existing permitted controller route usable
and give feedback through the ordinary loop. A policy route change is evaluated
before any forbidden send and applies to that request; it does not accidentally
inherit sticky session fallback behavior or failed-attempt thresholds. There is
no per-document context filtering, `PolicyWithheld` rewriting or semantic cache
purging in this default.

Setup requires a usable authorized controller route for admitted work. Ordinary
policy changes preserve or atomically replace that route while queued or in-flight
admitted work depends on it; this is not an indefinite lease for idle sessions.
A change that would remove the last one is a typed administrative refusal.
Controller authority must cover the admitted work's lifetime independently of
narrower tool/source grants. Deliberately removing all execution capability
requires an explicit attributed administrative stop through the existing owner,
not a stop inferred from an operation refusal. Actual provider/process/storage
failures retain their existing behavior and must not be used to disguise denial.

Refusals consume existing tool-call, turn, token and time budgets. Identical
runtime-denied bindings are not automatically retried without a relevant change.
Uncertain mutations retain their ordinary idempotency/reconciliation contract;
no blind repeat of effects or separate security retry machine is introduced.

Audit records observable authentication, requester/actor/executor, work and
operation, resolved resource/recipient, policy version, decision, start and
observed outcome. Recorded references aid investigation; they are not complete
semantic provenance or proof of what a model learned or disclosed. Audit data
itself has reader permissions and excludes raw protected fields by default.

Authoritative records join the next existing native/host commit, without an extra
per-operation fsync. An in-memory host declares process-lifetime audit unless
it joins an actual existing durable host commit. A crash may lose the newest
uncommitted details; recovery records uncertainty rather than claiming that
missing audit proves no effect. Required staging failure refuses only the new
affected operation; it does not rewrite completed physical outcomes.

Optional exporters fail observably and do not block ordinary execution. MobKit's
lossy event-log ingress must not be the only audit authority; test its overflow
and verify that native records survive and export loss is visible. External
witnesses, rollback-resistant storage and complete pre-effect evidence remain
separately selected future assurance work, not default dependencies.

## Cost, implementation and acceptance

No added network round trips or fsyncs on ordinary admission, model, tool or
publication hot paths. Existing authentication handshakes, requested source/tool
calls and ordinary storage commits remain. Optional gate-agent calls are explicit
application work, measured separately. Host time with bounded declared leeway is
sufficient; no authenticated clock or witness service is required.

Added authorization cost must be below 1 ms p99 per tool/model operation and at
most 10 percent per representative turn. Count evaluation, attribute resolution,
cache misses, recording and allocation. The final warm check is constant time;
preparation is not free merely because it precedes dispatch. Measure matched
trusted/governed runs, warm and cold paths, invalidation, 1/16 concurrency,
1/100/1,000/10,000 relevant policy/resource entries, streaming and audio. Report
p50/p95/p99, CPU, allocations, lock time and actual network/storage counts.
Reserve a quiet benchmark window after compilation.

1. Complete one real governed path: authenticated native input and immutable
   association, actual policy/grant owner, exact tool/source/peer dispatch,
   ordinary refusal feedback, permitted sibling and continued model turn,
   native audit and restart. Test revocation after an await, qualified replay,
   mixed-requester rejection, correlated permissions and completed-effect
   settlement failure. Preserve existing generated lifecycle owners.
2. Extend those same checks across every built-in execution mode and surface,
   including live/audio, WASM, memory/history, schedules, hosted tools and
   Elephant. Test actual sink entry and bypass attempts, not just policy helpers.
   Test optional mandatory gate placement and edited-action rejection separately
   from its model judgment, including a legitimate gated publication that the
   originating agent cannot perform directly and a queue item missing its
   requester/work association. No semantic non-disclosure claim is an acceptance gate.
   Test each declared egress category for gate bypass and unclassified capability
   refusal, and revoke an active subscription to verify owner-driven closure
   while its agent run remains usable.
   Also exercise the same broad external credential with an allowed selected-
   calendar read, refused deletion, refused account switch and a different caller
   who has no delegation from the credential's owner. Verify receiving-service
   enforcement for Elephant and connector-local enforcement for opaque APIs.
3. Run focused adversarial races/fault cases and cost measurements, obtain the
   four independent bus implementation reviews, and create the PR with green CI.
   Review changes from accepted checkpoints; preserve shared build reservations.

Preserve but remove from the default path the unfinished witness/time/recovery
stack and the semantic envelope, transitive-dependency, legacy-adoption and
context-partition machinery. Do not introduce storage-format migrations solely
to carry semantic labels that this contract no longer requires. Existing real
resource ACLs and observable work/audit identity remain in scope.
