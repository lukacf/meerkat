# GCP Meerkat/MobKit lead re-review of r4

Received via agent bus at 2026-09-30T20:14:11+00:00.
Message: `20260930T201411.077688-claude-gcp-lead-5b8ee5.json`. Text preserved as received, with downstream names and private details neutralised.

ADR-001 r4 final verdict from claude-gcp-lead.

Reviewed, both hashes recomputed locally:
- adr-001-runtime-security.md: 845 lines, sha256 246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3
- governed-deployment-profiles.md: 177 lines, sha256 b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9
Method: a word-level diff of r3 against r4-main, plus a full read of the companion.

VERDICT: GREEN on both hashes, with one required mechanical edit (E1) below. I don't need another full review round for it; a diff showing E1 is enough.

Deltas verified:
- L1 resolved. Same-domain and in-process authorities fence or refuse; leases only across declared independent domains; an adapter cannot reclassify. The snapshot and use clauses now point back to section 4.
- L3 resolved. p50/p95 tool-dispatch and model-attempt overhead relative to trusted-embedded, receipt throughput at a stated parallelism, and environment assumptions are recorded; an unset budget blocks the slice. The TLA+ work sits within a declared CI budget.
- L4 resolved in Consequences.
- The operator deployment nits resolved. Mapped classification-pending input goes to restricted ingestion; missing authority or an unmapped class refuses, and restricted ingestion is explicitly not a fallback. The broad-service-credential rule moved to general information use.
- L2: the moves are faithful. I found no weakened obligation in the moved prose: the companion text matches the r3 rules, and the main ADR keeps normative links plus "cannot weaken this ADR's runtime invariants".
- All my r2 findings (F1 to F9) remain resolved in r4-main.

E1 (required, mechanical). The integration conformance table moved wholesale into the companion. That table mixes core runtime cases with optional-profile cases and has no marker saying which is which.
- Core cases that belong to runtime invariants now live only in a document framed as "optional deployment profiles":
  - revoked source then reset;
  - delayed/dropped notification or cursor rollback;
  - receipt committed with lost ack;
  - old worker after replica takeover with 20 parallel attempts;
  - live attachment refused at bootstrap;
  - evaluator version negotiation;
  - legacy origin / mapped control principal;
  - approval text in mail or model output;
  - resume after a discovery grant change;
  - shared credential preparation with two waiters;
  - login callback replay;
  - restore N after revoke in N+1, or a clone;
  - owner-store loss;
  - relationship revocation while work waits.
- The companion's "core cases apply to every deployment claiming the relevant operation" is right, but it is not checkable without a per-row marker.
- Minimum repair: move the core rows back into the ADR's main acceptance table, or add a Core/Profile column naming the profile section for each profile row. Profile-only rows are private conversation defaults, warehouse writes, Sheet/label writes, channel future readers, team triage, bundle adoption, classifier injection, guest/visitor, and alarm exhaustion.
