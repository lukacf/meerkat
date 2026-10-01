# Meerkat Toolkit: final r6 review

Source: agent bus message `20260930T202520.951568-toolkit-codex-local-a25fd9.json`.
From: `toolkit-codex-local` on `Lukas-BigMac` at `2026-09-30T20:25:20+00:00`.

The message below is preserved verbatim.

Toolkit final ADR-001 r6 bundle review: GREEN on both exact hashes.

# ADR-001 r6: Toolkit final bundle review

Verdict: GREEN_DESIGN_ONLY on the two exact documents together. No material architectural finding remains in the reviewed Toolkit scope.

- Main: 2c3744f703ea0f58f3cbd6cd02103e0539eadb5eafac4cd6a48de34f0a38c6fe
- Profiles: 05ed1bd4202f4d53d5b7a5f83af052c6806a1809a6b8e152a060df74e294cc53

Root reviewed the full r3 baseline, complete successive main-document deltas through r6, the complete r4 companion and its complete deltas through r6. The separate adversarial reviewer covered r3-to-r5 changes and the full r5 companion, building on root's r3 baseline. The final r5-to-r6 delta is root-reviewed; it is not claimed as independently reviewed by that subagent. Both final file hashes were reverified before this verdict.

The final domain rule resolves the remaining ambiguity without creating a new owner: classification follows each canonical authority/state contract. Co-hosting or consuming a parent grant does not merge an independent resource authority into Meerkat. A handle for Meerkat-owned grant or epoch state cannot escape the same-domain entry fence by declaring itself independent. The trusted composition owner persists the negotiated relationship; adapters cannot select a weaker lease for same-domain state. Rolling replicas and in-process tool-policy providers remain covered.

The companion is normative with the ADR. Reachable governed behavior triggers the relevant conformance obligations regardless of profile naming. Explicit refusal for missing identity/audience support and the four added Core cases close label-omission and missing-support escapes. The performance budgets now include the lowest-resource advertised supported host class without relaxing entry or evidence requirements.

Previously reviewed Toolkit obligations remain intact: independent setup-waiter authority; exact credential and approval ceremonies; service credentials distinct from requester and recipient entitlement; release at the actual sink; complete plaintext audience and retention contracts; bounded classifier provenance; explicit legacy-corpus adoption rights; context reset without old-data or side-effect laundering; durable evidence and uncertain-append reconciliation; and current revocation proof for quarantine notifications. The complete Toolkit end state remains in scope. Limits of the initial governed profile are limits to enablement, not removal of later requirements.

This is an architecture review, not evidence of implementation, performance, destination conformance, profile enablement or release readiness. No source changes, builds or tests were performed for this review. It selects no caller-context API and authorizes no ABAC implementation. Luka retains the architecture decision, with Meerkat/MobKit upstream acceptance owned by the lead below Luka.

Evidence: /Users/luka/src/meerkat-toolkit/docs/coordination/20260930-upstream-review/adr-001-r6-toolkit-review/root-review.md
Independent review through r5: /Users/luka/src/meerkat-toolkit/docs/coordination/20260930-upstream-review/adr-001-r5-toolkit-review/independent-delta-review.md
