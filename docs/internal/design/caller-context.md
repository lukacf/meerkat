# Caller context through existing work owners

Status: design direction accepted by the Meerkat/MobKit development lead on 2026-09-30. The first supported profile and investigation sequence below are agreed; Rust APIs and wire types remain undecided. This is a docs-only record, with no implementation or executed acceptance evidence, and is outside Meerkat 0.8.50. The observed gaps are source evidence, not demonstrated vulnerabilities in the existing trusted-host API.

Source baseline: Meerkat `fe7fc72952da961c2586fdd9385b674a43a3d95b` and MobKit `e2795b5dbbabe224e2e912256d7b99e480b5c9c9`. Source links below are pinned to those commits; they describe that baseline, not the future implementation. Earlier experimental Toolkit contracts are not prerequisites or merge candidates.

The intended behavior is: a trusted host can bind an admitted request to its authenticated requester, original provenance and disclosure limits; runtime-derived work preserves those facts; the owners of private data, execution and delivery consult current host policy at the actual use. A saved admission decision is never perpetual permission. Meerkat remains a general runtime, MobKit forwarding is optional, and application authentication and policy remain application-owned.

## Current owners to preserve

| Boundary | Current evidence and design implication |
| --- | --- |
| Input and persistence | [InputHeader:36](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/input.rs#L36) has origin, visibility, dedup and correlation, but no general authenticated requester or audience. [Persistent admission:1770](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/driver/persistent.rs#L1770) and [InputState:1692](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/input_state.rs#L1692) already own admission, recovery and payload retirement. Associate new facts here instead of creating another work journal. |
| Runtime to Core | [for_input:42](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/runtime_loop.rs#L42), [batch merge:150](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/runtime_loop.rs#L150) and [StagedRunInput:1892](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/lifecycle/run_primitive.rs#L1892) preserve contributor identities and turn semantics. Core should receive the execution-relevant association through this boundary, not understand runtime Input or maintain a parallel contributor map. |
| Delegation and repair | [Delegation:908](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-mob/src/runtime/delegation.rs#L908) creates internal WorkSpec. [Continuation:763](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/input.rs#L763) can have System origin. [MobKit delivery:2572](https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/identity_first/bridge.rs#L2572) and [repair classification:3510](https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/identity_first/bridge.rs#L3510) already own propagation and carry. None of those classifications authenticates an external requester. |
| Private history and model use | [SessionService:2766](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/service/mod.rs#L2766) owns lifecycle and [read/events:2949](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/service/mod.rs#L2949) expose session data. [Model assembly:1667](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/agent/runner.rs#L1667) reads the session projection; [request preparation:5354](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/agent/state.rs#L5354) then hydrates messages and constructs an [actual provider attempt:140](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/agent.rs#L140). Authentication only at tool dispatch is too late for these disclosures. |
| Tools | [ToolDispatchAdmission:74](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/tool_execution_policy.rs#L74), existing access policy and [consequence policy:537](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-core/src/tool_consequence_policy.rs#L537) are real gates. Extend or compose their context rather than replace them. Provider-native tools explicitly bypass this dispatcher. |
| Results and events | [Terminal batch publication:1189](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-runtime/src/runtime_loop.rs#L1189) already owns exact completion and cancellation-resistant handoff. [Session event publication:697](https://github.com/lukacf/meerkat/blob/fe7fc72952da961c2586fdd9385b674a43a3d95b/crates/meerkat-session/src/ephemeral.rs#L697) includes replay and raw subscribers. Execution terminality and permission to disclose its body are distinct. |

Existing `TurnRequestContext` is provider text; `MobToolCallerProvenance` is projection-only; mob operator authority is capability-oriented; provider `auth_binding` selects provider credentials. None should be reinterpreted as the proposed requester model. An injected service/model/tool wrapper can already enforce a bounded single-tenant composition. C1 is needed only where native recovered, derived or batched work must preserve and enforce the same caller semantics.

## Meaning and trust boundary

Keep these concepts distinct, regardless of eventual Rust names or wire shape:

- **Requester:** the principal on whose behalf this work was accepted. This can be a person or service, qualified by the host's trust domain. It is not an agent identity, session ID, model-selected name or provider account.
- **Original origin:** the authenticated ingress provenance, such as a particular account/channel and occurrence. **Immediate origin** records the current hop, such as peer, flow or continuation. Delegation changes the hop, not the original requester.
- **Executor:** the actual agent/session/member performing work under existing runtime and mob authority. Possession of a requester association does not grant permission to spawn, attach, resume or use that executor.
- **Audience:** the permitted recipients and destinations for disclosure, including final replies. The requester is not automatically the audience. A group message may name one requester but a group audience; a private reply must not become a group reply because a callback uses the channel's default destination.
- **Use constraints:** the upper bounds on data domains, operations, purpose and permitted processing destinations. A model provider, an MCP server and a final chat recipient are different destinations. Permission to send a final answer does not imply permission to send private history to every configured model or tool.
- **Derivation:** which existing admitted inputs or operations produced the work, plus any narrowing. Keep associations to exact native identities; correlation and display labels remain diagnostic facts.

The authenticated host ingress attests these facts after resolving the actual transport identity and target resource. Public request fields are requests to be checked, not self-attestations. A prompt saying "I am the owner", an arbitrary serialized header, or a callback returning success cannot mint trusted provenance. In-process trusted embedders are inside the host boundary; this design does not sandbox a malicious host with direct store access.

Durable facts are bounded inline general facts: opaque, non-secret host principal, provenance and audience identifiers plus the original constraint ceiling. Persist them through the existing admission transaction. Do not store access credentials or a durable boolean Allow. The first supported profile does not use an external descriptor reference or add a policy engine to Core. Cross-host forwarding must be authenticated by the existing trusted transport/composition and validated by the receiving host. Serialization alone does not establish that trust, and C1 does not require a new credential issuer or token registry.

## Propagation, replay and batching

Admission binds the association to the exact accepted input and existing replay identity. Dedup must not let a caller with a colliding key obtain or inherit another requester's work or completion. The existing dedup owner must either namespace the claim by the relevant trust/requester domain or validate the exact original association before reporting a duplicate. Same body with a conflicting requester or broader audience is not an exact replay. Lost acknowledgement and crash recovery retain the original association while rechecking the retrier's right to observe or continue that work.

Delegation, flow retries, keep-alive work and background completions inherit the original ceiling through their existing owners. Narrowing may remove data, actions, processors or recipients; it cannot add them, replace the requester, or erase a restrictive contributor. An independently authenticated request can authorize different work, but must be represented as such, not silently replace the original request during a retry. Preserve the necessary association in existing completion/outbox records for as long as publication can be retried; payload retirement must not erase the only remaining audience evidence.

The first supported batching rule is: require equal host trust domain, requester, original constraint ceiling, selected confidentiality domain and intended audience semantics. Different occurrence IDs may batch, but every contributor remains identifiable. Apply this rule before irreversible dequeue/staging; incompatible inputs remain independently schedulable under the existing queue owner. The current scalar merge rules are useful infrastructure, not permission to pick the first requester's authority. Do not collapse a conflict to missing context and later reseed it with a third input. Rules that change admission, batching or dequeue decisions must be typed guards in the generated machine that owns that transition. Handwritten runtime checks cannot become a second decision owner.

Broader batching can follow only with an explicit compatibility rule: every selected datum must be authorized for the joint processing context, each effect must satisfy all applicable contributors, and each recipient must be entitled to all information used in its output. Intersecting tool names or unioning audiences does not prove this. One model answer is not safely separable into per-requester outputs without an independently justified information-flow rule. Multi-principal batching is deliberately deferred from the first slice.

`System` origin alone grants nothing. Requester-derived maintenance retains its lineage. Independently authorized scheduled/admin/system work has an explicit service principal and its own host-attested scope. Compaction, summarization and indexing consume private data and need the applicable data-use authority even if no user reply is produced. Mechanical cancellation, reaping and removal of already-owned temporary state must remain possible after revocation, but that cleanup authority does not authorize new model calls, private-body interpretation or external replies.

## Confidential history, computation and release

Caller context does not by itself label the existing transcript. The first supported profile authorizes the entire selected conversation/history domain, including retained summaries and memory injected from other stores. The application's existing resource/policy owner supplies that domain binding for the exact session/history identity; agent identity or a session label is not proof. Session selection and resume must enforce it before private loading or publication. Mixed-confidentiality history or memory receives a typed refusal as unsupported by this profile. A product needing that behavior must specify persistent message, summary and memory provenance and prove the information-flow rule before enabling it. The first profile does not redefine the complete Toolkit end state or remove its multi-user obligations.

For each model attempt, the existing Core owner must establish that the current execution may assemble the selected transcript, injected context, retrieved memory, artifacts and media; then authorize disclosure of that concrete request to the actual selected provider route. Hydration, compaction, fallback, retry, extraction, hooks, tracing and caches are part of this inventory. Authorizing only the final network call cannot undo an earlier unauthorized private read, and an authorization for provider A does not cover fallback to provider B. Existing provider credential acquisition remains separate from requester data-use permission.

Tool dispatch uses the same current execution association alongside existing access/consequence policy and exact tool binding. Returned tool data is not automatically authorized for later model or audience use. Provider-native tools, custom hooks and custom sinks that cannot satisfy the supported profile must be explicitly unavailable in that composition, not covered by a general "tool policy enabled" claim.

A computed answer may reflect all authorized inputs and history. Final output, streaming deltas, tool arguments/results in events, transcript reads, event replay, artifacts and peer replies each require an authorized destination. Check actual recipients/subscribers, not only the original reply destination. The first context-enforcing profile buffers model output until authorized final release. Supporting streaming later requires the same current audience discipline for every emitted chunk because each is an irreversible disclosure. Do not publish unrestricted early deltas and describe a later final-response check as protection. Execution may be durably terminal while body delivery is denied; retain truthful execution/side-effect status without leaking the withheld body or inviting duplicate execution.

## Current policy, waits and cancellation

The durable association is a ceiling and provenance, not a current verdict. At each protected operation the existing owner checks the original ceiling together with current host policy, exact executor/resource, destination and relevant contributor set. Revocation or an unavailable policy owner refuses new protected uses. Already assembled data and cached earlier approvals cannot justify a later provider attempt, callback, retry or delivery.

The contract must define the instant an operation enters its authorized effect. A loose check followed by an unrelated await and then dispatch is insufficient. The operation owner and host policy owner must coordinate entry and release consistently with their existing ownership mechanisms; this design does not select a lock order, permission-token type or universal work wrapper. Do not hold a policy/store lock across an unbounded model or network wait. The actual entered attempt stays with the existing execution/cleanup owner when a caller abandons its waiter, and relevant policy is checked again before each subsequent effect or protected result release.

Revocation cannot retract bytes already sent to a provider or reverse a completed external mutation. Revocation semantics are prospective: stop new protected work and unreleased disclosure, request cancellation of entered work where supported, and settle/report its real outcome. A cancelled observation is not proof of physical cancellation. Any stronger revocation guarantee needs an explicit supported-provider contract and a demonstrated owner; it cannot be obtained by dropping a future.

## Existing ownership boundaries to investigate

| Existing owner | Minimum behavioral capability to prove |
| --- | --- |
| Trusted host ingress and existing admission | Accept an attested association separately from untrusted content, validate its relation to the selected destination, and persist it with the exact input/replay identity. No new admission journal. |
| Runtime queue, recovery and primitive construction | Preserve association through all input variants, recovery and exact contributors; refuse incompatible batching; deliver only execution-relevant facts to Core. Admission, batching and dequeue changes use typed guards in the generated owning machine. |
| Mob delegation/continuation and optional MobKit bridge | Forward validated lineage and narrowing at existing creation/carry seams. Refuse a context-bearing request when a bridge cannot preserve it. No shadow member or session runtime. |
| Session/private-data, Core attempt and actual effect owners | Consult host policy for the concrete protected operation using the bound association; preserve existing cancellation and settlement ownership. Reuse present model/tool/service injection where it suffices; extend only the proven missing boundary. |
| Completion, outbox, event and transport owners | Preserve execution terminality while authorizing actual body publication and replay to each audience. Keep publication retries and caller loss under existing owners. |

These are behavioral seams, not five new trait families. A first implementation should trace one public request through durable admission, restart, delegated execution, history/model assembly and final delivery before freezing API shape. No generic callback should acquire blanket authority to execute arbitrary work merely because it returned Ready or Allow.

## Compatibility and accepted first profile

Existing trusted-host Meerkat compositions remain available when the host has not enabled this contract. A context-enforcing composition returns typed Unavailable for missing, invalid or legacy context on protected work, and typed Conflict for a dedup/replay association mismatch. Neither case acquires administrator or System authority. It can explicitly attest a new independent host request, but cannot invent historical provenance. Reopening old private history requires a real current resource authorization and an explicit migration/selection decision. Bridges and adapters advertise an explicit context-preservation capability. Context-bearing requests refuse peers that would discard it; an optional serialized field alone is not sufficient capability negotiation.

The accepted first supported profile is deliberately bounded so that each owner can be validated before API shape is frozen:

| Concern | Accepted decision |
| --- | --- |
| Representation | Bounded inline general facts and opaque host identifiers, persisted in existing admission; no credentials or external descriptor. |
| History | Whole-conversation authorization; mixed-confidentiality history and memory are typed unsupported. |
| Audience | Explicit stable recipients and destinations; dynamic group membership is unsupported. A frozen audience cannot silently become the latest channel membership. |
| Batching | Exact compatibility only; no multi-principal batch. Independently authorized system and scheduled work have explicit service principals. |
| Revocation and output | Prospective checks at operation entry, buffered model output and withheld late results with truthful execution status. |
| Compatibility | Opt-in per host; typed Unavailable or Conflict as specified above; explicit bridge/adapter capability. Prove that existing injection or wrappers cannot satisfy a boundary before expanding its upstream contract. |

These choices define implementation order and the supported first profile. They do not weaken the wider Toolkit end-state requirements or authorize unsupported profiles through a compatibility fallback.

## Open decision: where attribute-based access control lives

This design currently assumes Meerkat carries typed caller facts and asks a
host-owned decision point at each protected use, while MobKit and Elephant keep
their existing attribute-based access control (ABAC) as the policy owners. That
assumption is not settled. Before any implementation starts, decide between:

- **Host decision point (current assumption).** Meerkat is the attribute carrier
  and the policy enforcement point at every internal use (history load, model
  attempt, tool dispatch, output release, replay, delegation). One general host
  decision interface (operation, subject attributes, resource attributes,
  context) is answered by MobKit's or Elephant's ABAC. Meerkat owns no policy
  language.
- **ABAC generalized into Meerkat.** MobKit's ABAC moves down into Meerkat as a
  general capability, so containment of information between agents, sessions
  and peers is expressed as attribute policy inside the runtime rather than
  through coarse peering controls. MobKit and Elephant become policy authors and
  hosts instead of separate engines.

Either way the caller facts become a set of typed attributes rather than a fixed
field list, so existing ABAC policies can consume them. Today's peer-wiring
controls are a coarse way to bound information flow and should not be treated as
the containment model this design needs.

Ownership: this decision and the implementation that follows are owned by Luka
in a separate development environment. It is not part of 0.8.50.

## Investigation and implementation sequence

1. Version this design as a docs-only change in Meerkat's internal design area.
2. Build a nonmergeable tracer spike on a branch. Trace one public request through durable admission, a cold restart, delegated execution, history/model assembly and final delivery, using a deterministic local provider and recorded sinks. At every boundary record its existing owner, whether present injection/wrappers suffice, and the exact missing capability if they do not. The spike supplies ownership evidence; its provisional API is not a merge candidate.
3. Use that evidence for small first-profile changes, one owning subsystem per PR, each with typed refusal tests and existing-owner settlement evidence. The development lead reviews the threat model before the first code PR merges. API names and storage/wire schema follow that review, not the exploratory spike.

This work is outside the Meerkat 0.8.50 release. The design's acceptance does not imply implementation approval for an unreviewed API or a completed isolation guarantee.

## Required evidence for a later implementation

Use actual current admission, store, queue, Core and delegation owners with a deterministic local provider and recorded effect/sink calls. A locally authenticated fixture is not live-account authentication evidence.

- Same durable input survives a real cold restart with exact requester/origin/audience; conflicting dedup or lost-ack replay cannot disclose another request or duplicate the helper/effect.
- Compatible occurrences batch; different audiences/requesters do not, including a three-input conflict sequence and an active-turn continuation.
- A delegated request and derived background completion retain limits; reclassification as Internal/System cannot widen them. An independently attested system request is tested separately.
- A confidential sentinel in history, compacted summary, injected memory and hydrated media cannot reach an unauthorized model route, hook, tool, event subscriber or final destination. Verify actual sink bytes and callback counts, not only returned error strings.
- Deterministically revoke at queue wait, history-load wait, provider entry, model completion and final publication. Assert the exact entered work settles, no new unauthorized effect begins, and withheld output is absent from live and replay sinks.
- Missing legacy context, unavailable policy, unsupported bridge/client and changed provider route fail through the supported profile without silently becoming unrestricted. Existing trusted-host compatibility remains covered separately.

None of these acceptance cases has been executed or implemented by this design record. The next evidence is the end-to-end ownership tracer, followed by the reviewed first-profile changes described above.
