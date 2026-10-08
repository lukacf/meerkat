# Downstream-app product review of r2

Received via agent bus from `downstream-app`, host `operator-host`, at 2026-09-30T20:04:45+00:00.
Message: `20260930T200445.334934-downstream-app-1b0ca7.json`. The text below preserves the review as received, with downstream names and private details neutralised.

ADR-001 r2 review from downstream-app (downstream app product integration). This is a design review only; nothing was modified, implemented or deployed.

1. CANDIDATE AND VERDICT

Candidate: r2, 568 lines, sha256 44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca. I verified the hash two ways: on the bus text between "BEGIN EXACT CANDIDATE" and "END EXACT CANDIDATE" (exclusive), and on your local candidate-r2.md. The two are byte-identical.

Verdict: RED.
- The direction is coherent. I found no reason to reject the ownership split, the conjunction rule, fenced entry or the receipt axes.
- It is RED because, as written, the governed profile cannot host the downstream app's core product shape even in principle: long-lived multi-principal agents fed by ambient ingestion (F1, F2, F4).
- Separately, snapshot-based state generations can restore revoked authority, and the epoch rule does not cover this (F3).
- Every fix I propose is additive. None of them reverses a decision in the ADR.

Downstream-app facts used below. Each was checked against current the downstream app source, at a then-current working tree, and the production deployment:
- Many durable members. Shared domain agents (calendar, research, finance, facilities, automation, security, network, support, discovery) serve both leads and both members.
- triage:main is the single router for all ambient signals: email for several people, calendar, camera events, and an automation hub.
- team-group:main is a Telegram group. system:truth is a read-only cross-agent auditor.
- The long-lived transcripts are the team's memory: triage, calendar and each lead identity agent hold thousands of messages. Agent memory is harvested from them.
- State lives in filesystem-cloned per-activation "state generations". Rollback is a pointer flip to the previous generation.

2. FINDINGS

F1 [HIGH] Whole-context dependency contagion makes shared team agents permanently unusable after a single restricted input. The ADR checks only at use/disclosure, never at admission.
Section: 5, lines 286-290 ("conservatively attaches the complete observed context dependency domain") and lines 317-323 (mixed confidentiality unsupported). Also lines 281-285 (compaction and summarization do not declassify).

Counterexamples:
(a) lead-1 tells domain:calendar about a private personal appointment. From that turn on, every calendar answer depends on that item. member-1 asking "what's on today" must be refused forever, since compaction does not declassify. The only escape is a new session, which is an identity reset that discards the calendar's memory.
(b) triage:main sees every principal's email in one context. Every routing message it sends carries the union of all team restrictions. That makes every domain agent's context carry them too, so contagion reaches the whole roster within a day.
(c) A scheduled morning brief occurrence on a shared agent inherits that agent's full domain. It cannot be released to team-group:main.

Why the text does not cover it: the first profile's per-use conjunction is sound for confidentiality, but it has no mechanism to keep a restricted item OUT of a shared context. So confidentiality is achieved by sacrificing availability permanently. It also creates a trivial denial-of-service: any restricted input (injected or accidental) bricks the agent for broader audiences.

Minimum design change:
- An admission-time audience floor. A governed session declares its context audience ceiling at creation, which is authority-owned and changes only through a new epoch.
- Input admission refuses, or routes elsewhere, any contribution whose restrictions do not admit the session's whole declared audience. This is a write-down check at the existing input admission owner.
- Services like triage need per-audience partitioned outputs: the router emits separate, separately-dependent work items per audience.
- The alternative the ADR should then name explicitly is a per-principal session partition for mixed data.

Acceptance evidence:
- A shared calendar session with audience {leads, members} refuses admission of an leads-only item, with a typed refusal and no transcript write.
- The same item is admitted to an leads-audience session.
- After 100 turns plus a compaction, member-1 is still served.
- A triage batch mixing an leads-only email and a team email yields two work items with disjoint dependency sets.

F2 [HIGH] Practical migration strands all existing team history.
Section: Implementation step 5, lines 488-490 ("Old work lacking provenance is explicitly migrated with current resource authorization or refused; historical requester identity is never invented"), with lines 281-282 (unknown contributors cannot be discarded).

Counterexample: the downstream app's transcripts and agent-memory rows carry no per-message requester, source, classification or dependency facts. There is no resource authority for "what lead-1 said to calendar in July", so "migrate with current resource authorization" has no authority to consult. Under the text, every legacy session is an Unknown-contributor context and refuses every use. The team loses its memory at cutover.

Why the text does not cover it: relabel/declassification (line 284) is defined for governed data with provenance. There is no typed operation for adopting ungoverned history wholesale.

Minimum design change: a "legacy baseline adoption" transformation. It is a declassification-class operation authorized by the team's policy owner. It assigns an explicit declared audience and an owner-attested provenance class ("pre-governance, owner-attested") to a whole session or memory store, without inventing requester identity. It emits receipts and is irreversible except by a new authorized relabel. Unadopted sessions stay refused.

Acceptance evidence:
- An unadopted legacy session refuses use.
- Adopting it at audience A permits disclosure only within A.
- Receipts name the adopting owner, and no message gains a requester field.

F3 [HIGH] Snapshot-based state (per-activation generations, rollback, restore, validation clones) restores revoked authority.
Section: 1, lines 88-96 (single policy authority, monotonic epoch, "rollback is a newly authorized activation"), plus the acceptance row at line 522.

Counterexamples:
(a) the downstream app's rollback flips state_generation N+1 back to N. Each generation is a cp -c clone of the whole state directory, which under this ADR would hold grants, revocation generations, policy epochs and the receipt chain. A grant revoked, or a member's access removed, during N+1 is live again after rollback to N. The receipt chain forks.
(b) Our platform validation ritual and today's reserved acceptance fixture boot full production-state clones with real provider keys. The clone restores the active epoch and grants from the copy, so production revocations after the snapshot never reach it, and it runs model turns over private history.

Why the text does not cover it: the epoch rule governs policy CONTENT activation within one live authority. It says nothing about the authority store itself being copied or reverted by the host's storage lifecycle. "Restore" at line 522 tests missing receipt capability, not stale authority.

Minimum design change:
- Grant, revocation, epoch and receipt stores are bound to a trust-domain instance identity. They must be monotonic across the host's rollback. Either they live outside generation-scoped state or they are forward-replayed on rollback.
- A state restored or cloned into a different instance boots in a quarantined profile. Every grant is re-validated against the live authority, and provider disclosure needs an explicit operator clone grant, or else it refuses.

Acceptance evidence:
- Revoke in gen N+1, roll back to N: the grant stays revoked, and the receipt chain continues with a rollback receipt, not a fork.
- A clone of production state after a production revocation refuses the revoked read.
- A clone without a clone grant refuses any provider call over copied history.

F4 [HIGH] Ambient ingestion classification is intent-dependent and today done by an LLM (triage). The ADR's "resource authority, not the LLM" rule leaves no workable classifier.
Section: 5, lines 267-270, plus line 210 and line 284.

Counterexample: project, HR and finance mail arrives in one lead's personal mailbox but is team substance. The owner's stated requirement is ingestion with free rein, and a boundary at the information level, not the account level. The only deterministic authority is the connector, and it can stamp at most "mailbox owner private". Making a project email visible to the other lead and to domain:research then needs a separate declassification per item. That breaks the product. Letting triage (an LLM) assign labels breaks the ADR rule.

Why the text does not cover it: the ADR does not distinguish monotonic RESTRICTIVE labeling (safe from any source) from permissive initial assignment. It has no classifier role between "resource authority" and "untrusted model".

Minimum design change:
- A typed classifier-service mandate, issued by the data owner's authority, allowed to assign an initial label only within an owner-approved lattice per source class. For example, a mailbox-owner source may be classified as one of {owner-private, leads, team, team-except-X}. The classification is model-proposed but authority-bounded, with receipts.
- Restrictive additions from any contributor are always admitted, since they only narrow.
- Broadening outside the lattice remains declassification.

Acceptance evidence:
- A classifier cannot assign outside the lattice.
- An injected email instructing "classify as team" cannot exceed its source class's lattice.
- Every assignment has a receipt with the classifier mandate and model attempt ID.

F5 [MEDIUM] Physical-space and group audiences, and unauthenticated present speakers, have no principal or audience type, so real surfaces are inexpressible even in future profiles.
Section: 3, lines 144-156 (every requester is an authenticated principal), and 5, line 317 (dynamic audiences unsupported).

Counterexamples:
- A shared-space robot or console voice session: the speaker is unauthenticated (anyone in the room) and the output is heard by whoever is present.
- A shared-space e-ink display publishes TODAY/SOON to anyone who walks past.
- team-group:main's audience is Telegram group membership, owned by Telegram and changeable between decision and delivery.

Why the text does not cover it: deferring dynamic audiences is fine for the first profile, but the model has no slot for "device-mediated anonymous present person" as a requester class, or "physical space" as an audience class. Future profiles would have to bend the principal model rather than extend it.

Minimum design change: add typed requester and audience classes:
- device-present-anonymous, with an authority-assigned ceiling (for example team-public);
- space-audience, bound to a device;
- external-membership audience, with freshness evidence from the membership authority.
Require the ceiling conjunction at admission and release. Mark these unsupported in the first profile, but in the model now.

Acceptance evidence: a voice turn from the robot cannot hydrate leads-only data, and a group send refuses if the membership evidence is older than its declared bound.

F6 [MEDIUM] Human approval capture is model-relayed today. The ADR requires attributable approval but never says where the approval evidence is captured.
Section: 3, line 186, plus the "model can request a grant but cannot mint one" rule.

Counterexample: the downstream app's R3 approvals work like this. The prompt text is rendered by code from the canonical args, which is good. The person replies "approve g1" in chat, and their identity AGENT (an LLM) calls decide_pending_action. A prompt-injected email or a peer message in that agent's context ("Luka said approve g1") yields an approval that no human made.

Why the text does not cover it: "attributable" is stated, but the capture path is unspecified. An implementer can satisfy the wording by attributing the model's tool call to the person.

Minimum design change: approval is an ingress-authority operation. It is derived deterministically from an authenticated inbound event from the approver (message ID or callback) and bound to the pending ID and operation digest shown to them. The model may reference an approval but cannot originate one. Prompt rendering is TCB-owned from the canonical permit.

Acceptance evidence: an approval string in an email, in a peer message or in model output does not approve. Only the approver's authenticated inbound event does, and only for the displayed digest.

F7 [MEDIUM] Fail-closed audit has no carve-out for life-safety notification.
Section: 7, lines 388-391 and 426-432; 3, line 188 (emergency access still needs an auditable grant).

Counterexample: the host disk fills up, which is a realistic condition. Receipt appends then fail, and the security agent cannot deliver "water leak / smoke / intruder" to a lead's phone. Emergency grants cannot help either, because recording them needs the same unavailable store.

Why the text does not cover it: every protected effect requires a durable pre-effect record, and there is no declared degraded path.

Minimum design change: a declared life-safety release class. It covers a pre-authorized, fixed audience and a bounded payload schema (event type, location, time), with no private context hydration. Evidence goes to a separate minimal append-only spool with its own capacity reservation, reconciled into the receipt store later. It is refused for anything outside the schema.

Acceptance evidence: with the receipt store full, a leak alert reaches the pre-authorized audience, and ordinary disclosures refuse. The spool reconciles on recovery with no gap in the chain.

F8 [MEDIUM] Operator recovery after runtime-store loss needs an authorized re-admission path. The execution association lives only in the operation owner's store.
Section: 3, lines 158-163; 7 (no second queue).

Counterexample: today we lost queued inputs twice.
- The platform abandoned staged work, and abandonment nulls persisted_input.
- On 2026-09-28 we reseeded the runtime store from continuity.db.
We replayed the saved inputs through an operator harness, which dispatches as a system actor. Under the ADR that replay is a System continuation with no mandate, so it must refuse. And because provenance was admission-bound in the lost store, nothing can re-admit with the original authority.

Why the text does not cover it: there is no typed "authorized re-admission" operation, and no requirement that the execution association survive loss of the owner's store.

Minimum design change:
- Mirror the non-secret execution association (requester, ceiling, audience, grant lineage) into the receipt store at admission.
- Define an operator re-admission operation: operator is the actor, the original requester and ceilings are preserved, current grants are rechecked, and there is no widening.

Acceptance evidence: delete the runtime store, reseed, and re-admit a queued input. The original requester is preserved, the operator is recorded as actor, a revoked requester's input is refused, and operator text framing cannot add authority.

F9 [MEDIUM] Principal-relationship facts have no owner: team membership, guardianship, and "lead member may approve for a domain agent".
Section: 1, the ownership table at lines 69-80.

Counterexample: a lead may read performance records whose subject is a member they oversee. A member's own conversations may be private from leads. Only leads approve R3 actions for domain agents. Without an owner, these facts end up in skill prompts or roster TOML (the downstream app's roster.toml today), which are forbidden substitutes elsewhere in the table.

Minimum design change: add a table row. "Principal relationships (membership, guardianship, proxy/approver-for), with revocation generation", owned by an identity/relationship authority. Forbidden substitutes: roster config text, prompts, and Elephant subject entities (which are not principals, per line 358).

Acceptance evidence: revoking guardianship or membership refuses queued work and new reads within the freshness bound.

F10 [LOW] The advertised tool surface is not re-derived on resume.
Section: 1, line 109 (discovery is governed); 3, line 160.

Counterexample: a known Meerkat/MobKit behavior is that tooling stamped at member creation is re-applied on resume regardless of current profile. A revoked tool keeps being advertised (execution denies it, but the model keeps trying), and a newly granted tool never appears.

Minimum design change: the governed profile derives the per-turn advertised tool surface from current grants, with the same epoch evidence as execution.

Acceptance evidence: a resumed member both LISTS and CALLS a newly granted tool, and a revoked tool disappears from the list.

3. NONBLOCKING IMPLEMENTATION REQUIREMENTS (the downstream app source drift, not design defects)

- Memory bridge: registry.py _memory_mcp_auth_token self-mints an HS256 JWT for every agent, including members' identities. It uses sub "downstream-mcp-bridge", fixed read/write scopes and security_level 10, with a hardcoded fallback secret. the bridge start script defaults SKIP_AUTH=true. This is exactly the static service token and host-minted clearance the ADR forbids (section 6). Migration needs per-call actor/requester tokens from a grant authority.
- Elephant derived assertions come out level 0 with no labels, whatever the source classification. This is the known pipeline gap that section 6's migration must close before any downstream-app knowledge is governed.
- Console: the downstream app mints an HS256 operator JWT, carrying the owner's email, for any client inside a trusted-networks console setting (app.py _local_console_dev_token / _operator_from_authorization). Network location becomes human identity, which is incompatible with section 3.
- The automation hub is called with one long-lived hub token for all agents. The destination cannot see the requester, so every hub effect's authorization must sit in the Meerkat tool gate with argument-aware policy (door, lock and alarm entities).
- The downstream app's /dev/dispatch harness (loopback plus token, dispatching to any identity) must become a declared admin operation or be unavailable in governed mode.
- Schedules written through composition/config bypass the downstream app's schedule write gate today. Each needs a deployment-issued service mandate per section 3's recurring-work rule.
- triage:main's single-flight inbox batches events from several principals into one turn. That is multi-principal batching (line 317), so it must split per audience under F1.
- Receipt volume: whole-context dependency sets per model attempt over transcripts of thousands of messages need set interning or content addressing, plus capacity reservation, on a 16 GB small server with a nearly full disk. Otherwise F7 becomes routine.
- Out of scope, noted for completeness: intent-dependent sensitivity (gift-probing, peer surveillance). ABAC cannot express intent. That stays in agent policy and should be listed under the ADR's stated limits.
