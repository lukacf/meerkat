# GCP Meerkat/MobKit lead review of r2

Received via agent bus from `claude-gcp-lead`, host `meerkat-dev`, at 2026-09-30T20:01:15+00:00.
Message: `20260930T200115.851335-claude-gcp-lead-00bf5c.json`. The text below preserves the review as received.

ADR-001 r2 adversarial review from claude-gcp-lead (Meerkat release/MobKit lead, meerkat-dev).

1. CANDIDATE AND VERDICT
Reviewed: r2, 568 lines, sha256 44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca (recomputed locally from the bus body).
Source checked at Meerkat 54d14e91 (types and seams below). I did not re-check the MobKit or Elephant line anchors; the MobKit findings are about role, not line facts.

VERDICT: RED.
The direction is right and should survive:
- one decision contract;
- Meerkat enforces at its existing owners;
- conjunctive, never-widening delegation;
- trusted-embedded vs governed profiles with no silent downgrade;
- the Elephant empty-allowlist and relabel fixes.
It is RED for three material problems:
- The entry/receipt contract cannot be built without the per-effect durable journal the ADR itself forbids, and its generated ownership is deferred (F2, F4).
- The information-flow rules make long sessions permanently unusable after one source revocation (F3).
- Shared evaluator code couples Elephant to Meerkat's release cadence (F1).
There is a smaller coherent design (section 4 below) that keeps the good parts.

2. FINDINGS

F1 HIGH. Shared evaluator code between Meerkat and Elephant. [Decision; section 6; Consequences "coordinated versioning"]
Counterexample:
- Meerkat ships "Breaking (Rust source)" changes in most patch releases (0.8.4x CHANGELOG).
- Elephant pinning a Meerkat-owned evaluator crate must either upgrade in lockstep or keep an older evaluator.
- If it keeps the older one, Meerkat and Elephant give different decisions for the same wire request.
- That divergence is exactly the "no permanent double evaluation with divergent policy" the ADR forbids.
Why not covered: the cost is named but no ownership or stability rule is given, and "share evaluation/composition machinery" is stated as the decision.
Minimum change:
- Share the wire contract (request/decision/receipt schema, versioned), plus a golden conformance vector set run in both repos.
- Do not share evaluator code in the first profile.
- If a shared Rust crate is wanted later, it has no meerkat-core dependency, an independent version and semver policy, and lives outside the Meerkat release train.
Acceptance: the same vector file drives a differential test in Meerkat and Elephant CI; a vector change fails both until both conform.

F2 HIGH. Fenced multi-authority entry is a new per-effect durable journal, and its generated owner is unnamed. [section 4, "fenced entry"; section 7, "entry commit"]
Current code:
- `ToolDispatchAdmission::await_dispatch_admission` (meerkat-core tool_execution_policy.rs:74) is process-local, and its settle is "intentionally inert for ordinary process-local gates".
- Only the generated live-bridge gate consumes one-use authority.
- Per tool call there is no durable entry, attempt identity, or pre-effect row today.
The ADR requires, per tool dispatch, model attempt and disclosure:
- every grant/policy/resource/binding authority holds its generation through one durable commit;
- that commit also records the attempt identity, owner state and a pre-effect receipt.
This is a new durable per-effect journal (the "second recovery journal" section 7 forbids), unless existing turn/tool machines gain those states. Dogma sections 1 and 2 require that composition to be a named generated machine/composition/handoff. The ADR defers "generated state/handoff changes" to Open implementation decisions. That deferral is the central feasibility question.
Counterexample:
- A turn with 20 parallel tool calls needs 20 durable multi-authority fenced commits plus receipts before any dispatch.
- With Elephant as the resource authority, it falls back to leases anyway (section 4, across hosts).
- So the expensive local protocol buys little over an epoch check for the common remote case.
Minimum change for the first profile:
- (a) Validate against authority epochs at the existing dispatch admission seam, in process. Name ToolDispatchAdmission as the enforcement owner; its admit+settle shape already matches permit entry/settlement.
- (b) Record durable decision evidence at the existing turn/run commit boundary.
- (c) Require a durable pre-effect intent only for tools whose policy-owned consequence class (CompiledToolConsequence R0..R3, tool_consequence_policy.rs:218) is declared irreversible or external.
- (d) Name the owner of grant and policy epochs (machine or typed authority contract, from the dogma section 1 list).
- (e) Drop "explicit bounded lease profile, even within one process". In-process authorities participate in the fence or push epoch-change events; timers are not a local correctness mechanism.
Acceptance: a machine-schema/TLA+ model of the entry composition proving the revoke-before-entry race, written before implementation, plus a per-tool-call overhead measurement.

F3 HIGH. Whole-context dependency, current-epoch recheck and delete-revokes together poison sessions permanently. [section 5]
Counterexample:
- A 200-turn personal-agent session reads document D at turn 3. D is later tombstoned.
- Under these rules, every later model attempt in that session refuses forever, including unrelated questions:
  - "every transitive dependency must satisfy ... current ... tombstone epoch" before each use;
  - "summarization, compaction ... do not declassify";
  - the initial profile attaches "the complete observed context dependency domain";
  - "absent an explicit surviving archival/release grant, deletion does revoke".
- There is no defined remedy.
- Separately, each model attempt must check the current epoch of every dependency ever observed: O(session lifetime) authority lookups per call, remote for Elephant.
Why not covered: "mixed-history" is deferred to a later profile, but the first profile has no escape hatch and no cost bound.
Minimum change:
- Define an authorized context rebase: a new session revision built only from currently authorized sources, owned by the session owner, typed and audited. It is the first-profile remedy.
- Bound rechecks with epoch-change notifications from resource authorities (event-driven invalidation of the session's dependency set), not per-use polling of the whole set.
Acceptance:
- tombstoning one source in a 200-turn session makes the next turn refuse with a typed dependency-revoked outcome;
- a rebase produces a working session without that source;
- per-attempt authority traffic is O(changed dependencies).

F4 MEDIUM-HIGH. Evidence-pending withholds the tool result from the agent loop, leaving the turn in an unnamed state. [section 7, custody table row 3]
Counterexample:
- The main consumer of a tool result is the next model call in the same turn, not a human recipient.
- The receipt append fails after a known outcome, so the body is withheld.
- The turn cannot continue, and there is no timer (correctly), so it is suspended until the store recovers.
- That is a lifecycle state of the existing turn/run machine, but the ADR names no state, transition, or recovery trigger.
- "No second lifecycle reducer" is asserted, not designed.
Minimum change: either
- add an EvidencePending turn state with a store-recovered transition to the owning generated machine; or
- have the governed profile refuse new protected dispatch while the receipt store is not healthy (admission-time check), so post-effect append failure is only the crash path, which already maps to Unknown.
Acceptance: the machine model plus a fault test (receipt store fails after tool success; the turn ends in the named state; recovery resumes without a second effect).

F5 MEDIUM. Principal/grant sprawl is not reconciled, and the canonical requester type is not chosen. [Decision last paragraph; Implementation step 2]
At 54d14e91 there are at least:
- meerkat-core auth/principal.rs: PrincipalRef, PrincipalKind, ActingOnBehalfOf, AuthGrant;
- approval.rs: ApprovalPrincipalId;
- service/mod.rs: OpaquePrincipalToken, MobToolAuthorityContext;
- meerkat-mob control_policy.rs: MobControlPrincipal, OperatorGrant;
- machine-schema: PrincipalId;
- CompiledMemberToolGrant, WorkGraphNamespaceGrant, ForkedParticipantGrant.
`AuthGrant::allows` compares acting_on_behalf_of by equality: there is no chain and no attenuation. The monotonic restriction algebra is therefore a new type regardless.
Why not covered: "concepts below are not an instruction to create parallel versions" is a caution, not a mapping. Dogma section 4 says identity is canonical.
Minimum change:
- Name the canonical requester/actor type now. PrincipalRef plus an ActingOnBehalfOf extended into a delegation chain is the obvious candidate.
- List which types map onto it or are retired, as an entry gate for step 2.
Acceptance: the tracer's inventory shows one requester type at every admission path.

F6 MEDIUM. The carrier placement risks a duplicate "who sent this" fact. [section 3 "Persist ... atomically with existing input/work admission"]
- InputHeader.source is `InputOrigin { Operator, Peer { peer_id, display_identity }, ... }`, with no authenticated principal.
- Adding a requester authority beside InputOrigin::Operator creates two answers to one question.
Minimum change:
- Requester authority refines InputOrigin: Operator carries the authenticated principal; Peer carries the peer principal binding. It is not a sibling field.
- Legacy persisted inputs deserialize as an explicit unattributed variant, never as an authenticated Operator (this matches the section 5 migration rule).
Acceptance: an old-schema input round-trips as unattributed and is refused in the governed profile.

F7 MEDIUM. MobKit's role contradicts the ADR's own ownership table. [Decision "MobKit becomes a policy administration and observation client"; Implementation step 4]
- The table says operation owners are authoritative for entry.
- MobKit owns its own protected operations: console HTTP/SSE, blob download, and the memory coordinator (the ADR cites mk-blob and mk-operator as defects to fix).
- Counterexample: blob download involves no Meerkat operation. If MobKit is only a client, nothing enforces it.
Minimum change:
- MobKit is the enforcement point for MobKit-owned operations under the shared contract, plus the policy administration UI.
- Its current ABAC engine becomes one implementation of the decision contract, or is retired.
- "Replace console authority with projections" applies to Meerkat-owned facts only.

F8 MEDIUM. Realtime/live voice is an ungoverned disclosure channel. [section 5 model-attempt and output-release rules; first profile "buffers output"]
- GPT Live sessions stream audio continuously and hold provider-side state (seeded summaries and context carriers).
- Buffered output is impossible there, and the provider-held context is exactly what section 5 wants inventoried.
- Unless named, a governed session with a live bridge discloses the same history through voice.
Minimum change: add realtime/live sessions to the section 5 list of sinks that "must join the same contract or be unavailable in the governed composition", with governed bootstrap refusing a live bridge.
Acceptance: governed bootstrap with a live bridge refuses before any provider connect.

F9 LOW. #1333 migration is ambiguous.
- caller-context.md offers two options. The ADR picks a hybrid (a Meerkat-owned capability, host policy providers behind it) without saying which option it replaces or what happens to the "coarse peer-wiring" note.
- Minimum change: one sentence in Status: which option it adopts and how peer wiring relates (it stays a transport/topology control, not the containment model).

3. NONBLOCKING IMPLEMENTATION REQUIREMENTS
- wasm32: Meerkat must compile for wasm32 (push-to-main gate). The authorization feature, receipt store and fences are cargo-feature gated, and trusted-embedded builds carry no cost.
- Semver: new pub fields on InputHeader/ToolDispatchContext and new enum variants are Rust-source breaking. Land them in a declared breaking release with CHANGELOG entries.
- Generated catalogs: extend the existing machine-schema catalog and codegen drift gate. Do not add a second generator.
- TLC/model checks for any new composition must fit the existing CI budget. Pre-push to green is held to about 20 minutes; state-space growth is a design constraint.
- Performance: measure the added latency per tool dispatch and per model attempt in the vertical slice. The slice acceptance should include a bound.
- Elephant empty-allowlist and relabel fixes: file them as Elephant issues now; the ADR says they are independent.

4. SMALLER COHERENT DESIGN (proposed)
- (1) Placement. Meerkat owns the enforcement points and a typed decision contract at its existing seams:
  - ToolDispatchAdmission;
  - the tool-policy provider;
  - model-route composition;
  - output release;
  - input admission.
  The host supplies the decision implementation. MobKit's ABAC becomes one implementation plus the admin UI and remains the enforcement point for MobKit-owned operations. Elephant stays a separate resource authority, reached with a delegated-authority wire token. No shared evaluator code.
- (2) Carrier. The requester, actor and grant chain refine InputOrigin and are persisted with admission. Delegation is conjunction-only attenuation. The model can request a grant but never mint one.
- (3) Enforcement. An in-process epoch check at admission/dispatch. Durable decision evidence at existing turn commits. A durable pre-effect intent only for irreversible external effects. Grant/policy epoch ownership is a named authority. Revocation is event-driven locally; leases only across hosts.
- (4) Information flow. Session-granularity labels with an explicit authorized rebase. Per-message IFC, derived declassification, and provider-cache inventory move to a separate ADR-002 profile.
- (5) Elephant. Its domain fixes proceed independently.
This keeps every good property of r2 in the first profile and removes the three RED items. It also makes step 3's vertical slice buildable on current owners.
