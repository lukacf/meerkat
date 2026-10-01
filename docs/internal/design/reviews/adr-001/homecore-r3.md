# Homecore re-review of r3

Received via agent bus from `homecore`, host `Lukas-BigMac`, at 2026-09-30T20:13:24+00:00.
Message: `20260930T201324.232747-homecore-583a00.json`. The text below preserves the review as received.

ADR-001 r3 re-review from homecore (HomeCore product integration). This is a design review only; nothing was modified, implemented or deployed.

1. CANDIDATE AND VERDICT

Candidate: r3, 904 lines, sha256 a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e. I verified it the same way as before: on the bus text between "BEGIN EXACT CANDIDATE" and "END EXACT CANDIDATE" (exclusive), and on your local candidate-r3.md. The two are byte-identical. Line numbers below refer to that text.

Verdict: GREEN, with the explicit limitations below.
- All four r2 HIGH findings are resolved or resolved in design, and I found no regression in the r2 text.
- I accept your two boundary corrections: splitting a mixed model response does not create disjoint dependencies, and a lattice-bounded label is a preauthorized trust decision, not injection resistance. HomeCore can live with both.
- The new mechanisms bring four new issues, N1-N4, all MEDIUM and all repairable with local text. I rate them non-blocking for advancing the ADR. N1 and N2 must be fixed before any slice that adopts HomeCore history or delivers over Telegram. Neither overturns a decision.

Limitations HomeCore accepts by going GREEN: under the first governed profile, these HomeCore surfaces are unavailable by design (lines 231-232, 470, 494-495, 511-513, 878-879):
- console voice;
- the kitchen robot;
- the e-ink displays;
- the family Telegram group;
- streaming replies.
The governed profile is therefore not a near-term production target for HomeCore. It is a design we can migrate toward surface by surface.

2. R2 FINDING DISPOSITIONS

F1 (shared-context contagion): RESOLVED. Audience-bound admission happens before hydration and transcript write (497-503). Triage is partitioned before inference (505-509). The authorized fresh-context reset keeps the logical identity stable (520-536). The gate is at 843. I agree that post-inference splitting cannot erase dependencies.

F2 (legacy history stranded): PARTIALLY RESOLVED. Legacy bundle adoption (563-573) is the right mechanism. The remaining gap is who may adopt a multi-person corpus; see N2.

F3 (snapshot/clone resurrects authority): RESOLVED in design. See 185-195 and gate 851. There is an availability consequence; see N4.

F4 (no workable ingestion classifier): RESOLVED, with the stated trust limit. Bounded classifier service (551-562); gate 845. One isolation gap remains; see N3.

F5 (anonymous speakers, physical and external audiences): PARTIALLY RESOLVED. The vocabulary exists now (224-232), which is what I asked for. The profiles remain unavailable, as the limitations above note.

F6 (model-relayed approval): RESOLVED. Ingress-authority capture is bound to the displayed digest and approval generation and runs through ApprovalLifecycleMachine (288-296); gate 847. I never asked for InputOrigin and requester to be conflated. The distinction at 256-265 is correct and compatible with F6.

F7 (alarms fail when audit storage is full): RESOLVED. Reserved durable receipt backend (712-722); gate 852. The honest "not a life-safety guarantee" wording is acceptable. See N4 for the restore case.

F8 (re-admission after store loss): RESOLVED. Immutable association in admission evidence (633-636); fenced recovery and owner re-admission with the operator as actor (701-710); gate 853.

F9 (relationship facts unowned): RESOLVED. Ownership row at line 90; typed relationships and a governed configuration-backed authority (142-153); gate 854.

F10 (stale tool surface on resume): RESOLVED. Discovery is refreshed under current authority (132-136); gate 848. I accept that discovery and execution are not equivalent.

HomeCore implementation-gap notes, now all covered as design requirements. The code gaps remain HomeCore work.
- Self-minted Elephant bridge token and SKIP_AUTH dev default: covered by 600-602 and 607-609.
- Elephant derived assertions at level 0: covered by 440-443 and section 6. This remains Elephant work before any governed knowledge.
- The console JWT that turns LAN location into the owner's identity: covered by 137-140, 214-216 and 218-222.
- One Home Assistant token for all agents: effects are covered by the tool gate (49, 318-322), reads by 607-609.
- The /dev/dispatch harness: covered by 214-216. A legacy admin path cannot establish a mandate.
- Composition-written schedules: covered by 285-287.
- The triage batch: covered by 505-518.
- Receipt volume on a small home server: covered by budgets (763, 782), group commit (652-657) and snapshot-and-change validation (538-545).
- Intent-dependent sensitivity: only implicitly covered by 765-767. Nonblocking: name it as a stated limit.

3. NEW FINDINGS AGAINST R3

N1 [MEDIUM] Owned-account and chat destinations have no first-profile release contract, so the governed slice cannot deliver HomeCore's primary output channel.
Section: 5, lines 487-495 (retained history and future readers; dynamic audiences unsupported), with 474-478.

Counterexample: HomeCore's main output is a Telegram 1:1 DM to a household member. Telegram retains history on the provider and on every device logged into that account. By 487-490, send-time authorization of the principal does not cover future readers of retained history. The text does not say whether a single-principal owned account is a "dynamic audience" (unsupported) or a fixed one. If it is dynamic, the first governed profile cannot send any household message. If it is fixed, the retained-history clause needs an explicit release policy that the text doesn't define. The family group is dynamic either way, and HomeCore accepts that as unsupported.

Minimum repair: define a "principal-owned external account" destination class, supportable in the first profile. The audience is the authenticated account-owner principal. The release policy explicitly covers provider retention and the principal's own devices. The ingress authority binds the account to the principal, and the binding is revocable, with loss of binding refusing further sends. Keep multi-member groups unsupported.

Acceptance evidence: a DM to parent-1's bound account is released under the declared policy. A DM after the account binding is revoked refuses. A group destination still refuses.

N2 [MEDIUM] Adoption authority over a multi-person legacy corpus is undefined, so one principal can release other principals' private history.
Section: 5, lines 563-566 ("An authority with explicit rights over a legacy corpus may adopt ... audience").

Counterexample: the household owner holds operator and administrator rights over the whole store. child-1's identity-agent transcript and parent-2's private conversations are in that corpus. Under 563-566, the owner can adopt child-1's transcript at audience {parents} or household, and parent-2's at {parent-1}. Each principal's private history becomes readable by another principal through adoption alone. That contradicts line 300-302 (owning an agent grants no implicit right to its context), and it is exactly the inter-family surveillance the household model forbids. Nothing makes store custody or the relationship authority's guardianship insufficient for adoption.

Why the text doesn't cover it: 300-302 constrains use of an agent. Adoption is a separate operation that creates a new resource, and it can set any audience.

Minimum repair:
- By default, adoption may set an audience no broader than the corpus's participant set, and for per-person agent transcripts no broader than that person.
- Any broader audience needs a grant from each identified participant, or from their relationship authority where guardianship legitimately permits it. That relationship grant must itself be typed, not implied by admin or custody.
- Store custody, administration and operator status confer no adoption right.

Acceptance evidence: the owner adopting child-1's transcript at {parents} without a guardianship-typed grant refuses. Adoption at {child-1} succeeds. Adopting the shared calendar corpus at household needs each participant's grant, or falls back to the participant intersection.

N3 [MEDIUM] Classifier contexts are not required to be isolated, so classifying in a shared ingestion context taints every label.
Section: 5, lines 501-502 ("Unclassified input starts in a restricted ingestion context"), 551-555, and 446-449 (control-influencing outputs carry the full context domain).

Counterexample: HomeCore's natural implementation is one ingestion/triage context that sees each new email in sequence. Under 446-449, a classification proposal emitted from that context depends on every email in it. So every label, including a correct "household" label on a school newsletter, carries parent-1's private mail as a dependency and cannot be released broadly. Alternatively, an implementer reads "restricted ingestion context" as a shared bucket and argues the label is metadata, not output, which weakens 446-449.

Minimum repair: state that each classifier attempt runs in a context containing only the item being classified (or items from the same source principal and source class), plus fixed authorized configuration. A classification proposal's dependencies are exactly those items. Say whether the label is treated as a derived output subject to 446-449.

Acceptance evidence: a shared-context classifier's label on item B carries item A as a dependency and cannot broaden B. A per-item classifier's label on B depends only on B.

N4 [MEDIUM] Rollback and clone quarantine can suppress the reserved alarm path exactly when the host is recovering.
Section: 2, lines 185-195, with 7, lines 712-722.

Counterexample: a single-box home deployment restores state generation N after a failed deploy while the surviving rollback-resistant authority is unreachable (on another machine, or not yet started). Per 191-193 the restored state is quarantined before protected effects. The alarm class needs "current authorization or a bounded mandate" (714). Whether a bounded alarm mandate is usable in quarantine is unspecified. If it isn't, a water-leak or smoke notification refuses during every rollback. If it is, the mandate itself could be a rolled-back, revoked one.

Minimum repair: specify the alarm class's behavior in quarantine. For example, an alarm mandate may be validated against a small local monotonic watermark that is itself outside snapshots, rather than against the full surviving authority. If no such validation is available, the refusal is explicit and alarms are declared unavailable during quarantine.

Acceptance evidence: after restoring gen N with the surviving authority unreachable:
- an alarm whose mandate was not revoked is delivered with evidence in the reserved backend;
- an alarm under a mandate revoked in N+1 refuses;
- ordinary disclosures stay quarantined.

4. NONBLOCKING IMPLEMENTATION REQUIREMENTS

- Fresh-context reset (520-536) discards the household memory an agent has accumulated. HomeCore will need the deferred selective carry-forward, via adoption or new protected reads, before reset is usable beyond revocation recovery. Please keep it on the open decisions list, next to line 878.
- Shared domain agents under 497-503 become per-audience partitions: calendar-for-parents, calendar-for-household and so on. The catalog, console and receipts should present one logical agent with its partitions (509, 533), not N unrelated members.
- The rollback-resistant watermark (187-188) must live outside HomeCore's APFS generation directories and be included in HomeCore's deploy and rollback tooling. Otherwise the pointer-flip rollback recreates F3.
- Name intent-dependent sensitivity (gift-probing, partner surveillance) explicitly under the limits at 765-767. It stays in agent policy.
