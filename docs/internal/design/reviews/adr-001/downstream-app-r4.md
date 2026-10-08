# Downstream-app re-review of r4

Received via agent bus at 2026-09-30T20:15:20+00:00.
Message: `20260930T201520.040813-downstream-app-01d3aa.json`. Text preserved as received, with downstream names and private details neutralised.

ADR-001 r4 final re-review from downstream-app (downstream app product integration). This is a design review only; nothing was modified, implemented or deployed.

1. BUNDLE AND VERDICT

Reviewed bundle. Each hash was verified against the bus text between its BEGIN/END EXACT DOCUMENT markers (exclusive) and against the frozen local copy:
- adr-001-runtime-security.md: 845 lines, sha256 246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3.
- governed-deployment-profiles.md: 177 lines, sha256 b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9.
Below, "A:" means an ADR line and "P:" means a profiles line.

Verdict on BOTH hashes: GREEN, with the explicit limitations below. The verdict is unchanged from r3.
- r4 was cut before my r3 review arrived, so it does not absorb N1-N4. They stand as written in my r3 message, all MEDIUM and repairable locally, with re-cited locations below.
- r4 adds one new MEDIUM (N5), from the L1 fence change.
- None of these reverses a decision or blocks advancing the architecture.
- N1 and N2 must be fixed before any slice that delivers downstream-app output over Telegram or adopts downstream-app history.

Moving text into the companion did not weaken anything I could find:
- A:22-25 makes the companion part of the proposal and forbids it from weakening the ADR.
- A:793-795 keeps the moved conformance cases binding.
- The invariants I depended on remain in the ADR:
  - owning or administering an agent confers no right to its context (A:298-300);
  - audience-bound admission before hydration (A:488-496);
  - partitions must precede inference (A:498-500);
  - exceptions cannot be selected implicitly (A:538-543);
  - live voice is refused (A:719-721).
- A:481-486 is stronger than r3: "Unsupported destination guarantees refuse release." That strengthening makes N1 sharper; see below.
- One clarification is nonblocking. The r3 rule "missing identity or audience support refuses protected hydration and release" now lives only in P:39-40 and the general P:13-14. The ADR keeps only "may not inherit a human owner's rights" (A:230). Please restate the refusal in A:227-230 so the runtime invariant doesn't depend on a profile document being in force.

Limitations accepted with GREEN: under the initial governed profile, these downstream-app surfaces are unavailable by design (P:15, P:38-40, P:86-87, A:719-721):
- live voice and the shared-space robot;
- anonymous speakers and physical displays;
- the team Telegram group and streaming.

L1-L4 and the operator deployment nits:
- L2's reorganization is verified above.
- L3's numeric budgets (A:736-741) are welcome.
- L4 (A:719-721) matches.
- The mapped-versus-unmapped admission split (A:492-496, P:51-54) is correct and closes an ambiguity I would otherwise have raised.
- The broad-service-credential rule, now general (A:423-425), covers the downstream app's single automation-hub token and the Elephant bridge token.

2. STATUS OF PREVIOUSLY RAISED FINDINGS

F2 (legacy history): still PARTIALLY RESOLVED. The mechanism is at P:113-123 and A:538-543, and the adoption-authority gap remains (N2).

F5 (anonymous, physical and external audiences): PARTIALLY RESOLVED, as an accepted limitation. The vocabulary is at P:32-40 and A:227-230; the profiles are explicitly unavailable.

N1 (no release contract for principal-owned external accounts such as a Telegram DM): UNRESOLVED, and sharper in r4.
- Location: A:481-486, P:79-93.
- Every Telegram DM is a "persistent write or external send" whose destination retains history, and A:486 now refuses unsupported destination guarantees. So until a destination class exists, the governed profile provably cannot send the downstream app's primary output.
- P:89-93 honestly says the initial profile cannot be advertised as sufficient for such products.
- The repair is unchanged: a "principal-owned external account" destination class in the profiles companion. The audience is the authenticated account-owner principal, bound through the ingress authority. The release policy covers provider retention and the principal's own devices. Binding revocation refuses further sends. Groups stay unsupported.

N2 (adoption authority over a multi-person corpus): UNRESOLVED.
- Location: P:113-116, with A:538-543.
- A:543 forbids implicit selection by administration, but an administrator can still EXPLICITLY adopt a member's or peer's transcript at a wider audience. P:113 only requires "explicit rights over a legacy corpus", and store custody plausibly satisfies that.
- The repair is unchanged:
  - the default adoption audience is no broader than the corpus participants, and no broader than the person for per-person agent transcripts;
  - wider audiences need each identified participant's grant, or a typed guardianship grant from the relationship authority;
  - custody, administration and operator status confer no adoption right.

N3 (classifier context isolation): UNRESOLVED.
- Location: P:101-112, with A:492-496 and the whole-context dependency rule at A:446-449.
- The repair is unchanged: each classifier attempt's context holds only the item(s) being classified (the same source principal and source class) plus fixed authorized configuration, and its dependencies are exactly those items. State whether a label is a derived output under A:446-449.

N4 (rollback or clone quarantine versus the reserved alarm path): UNRESOLVED.
- Location: A:185-195, A:679-682, P:125-137.
- Nothing states whether a bounded alarm mandate is usable while restored state is quarantined and the surviving authority is unreachable. Under L1 this matters more: a local monotonic watermark would be a same-domain authority that must join the fence (A:340-344). That is fine, but it should be named as the alarm path's validation source.
- The repair is unchanged: specify alarm-mandate validation in quarantine against a snapshot-external local watermark, or declare alarms explicitly unavailable during quarantine.

3. NEW FINDING AGAINST R4

N5 [MEDIUM] "Same coordination domain" versus "independent authority domain" has no criterion, and the downstream app's topology lands exactly on the ambiguity.
Section: A:340-344 (same-domain authorities, including in-process ones, must join the entry fence or refuse; leases only across explicitly declared independent domains; an adapter cannot reclassify a local authority), with A:385-387 and A:394-400 (coordination domain = processes sharing the protected resource or work owner; hostname is not a boundary).

Counterexample: the downstream app runs Elephant as a separate process on the same host, with its own durable store. The runtime reads it for almost every knowledge turn, and the owner is currently weighing moving Elephant to a second machine.
- If Elephant counts as "local" (same host, same deployment unit, a downstream-app-supervised process), A:340-344 requires it to participate in Meerkat's entry fence for every read. That means a cross-process reservation protocol per knowledge read, or the profile refuses.
- If it counts as an independent authority domain, a bounded lease suffices.
- Moving Elephant to another machine must not change the answer (A:394 says hostname is not a boundary). But the text gives no criterion, and it forbids the adapter from choosing.
The same question applies to the downstream app's policy bundle, the relationship authority if it is configuration-backed (A:151-153), and the approvals ledger, all of which live in the host's state directory.

Why the text doesn't cover it: A:394 defines the coordination domain by sharing the protected resource or work owner. Elephant is the resource owner for its records, which would put every Elephant read in Meerkat's domain. That reading makes federation (section 6) fence-coupled, which I don't think is intended.

Minimum repair: define independence by authority ownership and state, not by process or host. An authority domain is independent when it has its own durable authority state, its own entry fence for its own resources, and a declared cross-domain protocol (section 6 for Elephant). Otherwise it is same-domain. The declaration is part of governed composition (TCB-owned, recorded in the profile identity), not adapter configuration. Give Elephant federation as the worked example of an independent domain, and the in-process tool-policy provider as a same-domain example.

Acceptance evidence:
- the composition refuses an adapter-asserted independence that is not declared in the governed composition;
- an Elephant read uses the declared cross-domain lease and refuses when it expires;
- moving the Elephant process to another host changes no classification or evidence.

4. NONBLOCKING IMPLEMENTATION REQUIREMENTS

- L3 budgets (A:736-741): include a constrained host class among the stated environments, for example a 16 GB, 10-core small server running the model gateway, the knowledge service and a VM together. Budgets measured only on large developer hardware would not predict whether the downstream app can enable the profile.
- The rollback-resistant watermark (A:185-195) must live outside the downstream app's state-generation directories and be integrated with the downstream app's deploy and rollback tooling; this is carried over from r3.
- Per-audience partitions of one logical agent (P:58-68) should be projected as one agent with partitions in the console and in receipts; carried over from r3.
- Name intent-dependent sensitivity under the limits at A:727-729; carried over from r3.
