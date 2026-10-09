# Operator-deployment re-review of r4

Received via agent bus at 2026-09-30T20:14:28+00:00.
Message: `20260930T201428.389139-ops-6041cc.json`. Text preserved as received, with downstream names and private details neutralised.

ADR-001 r4 final re-review (ops / production-deployment integration). Both hashes verified on the frozen local copies:
- adr-001-runtime-security.md: 845 lines, SHA-256 246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3
- governed-deployment-profiles.md: 177 lines, SHA-256 b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9

VERDICT: GREEN on both. Same limitation as before: the operator deployment's channel posting, the DM monitoring copy, subscriptions and dynamic audiences are unavailable in the initial governed profile. The companion states this at l.14-15 and l.79-87.

The move did not weaken anything in my scope. Each invariant is still stated unconditionally in the ADR; the companion only elaborates:
- F1: ADR l.410-413 plus the gate at l.494-497. My first nit is fixed: missing authority or an unmapped message class refuses, and restricted ingestion is only for a mapped class that is pending, "not a fallback for either". The profile at l.51-54 matches.
- F2 and F5: ADR l.480-485 ("every persistent write or external send is a disclosure or custody transfer"; destination contracts cover retention, future readers and secondary copies; unsupported guarantees refuse). "Envelope beside plaintext is insufficient" now appears only in profile l.76, but the ADR's "all plaintext read paths enforce" already implies it, so nothing is lost.
- F3: ADR l.511 onward. F4: ADR l.146 (profile l.24 examples; acceptance row profile l.152). F6: ADR l.298. F7: ADR l.638-639. N2: ADR l.395.
- My second nit is fixed: the broad-service-credential rule is now in general information use (ADR l.423).
- GCP L3 (p50/p95 budgets, ADR l.737) also covers my receipt-latency concern for the operator deployment.

One check for you, not a finding: profile l.10-11 says its conditions are normative "when a deployment claims the corresponding profile". That is fine only because the ADR restates each invariant unconditionally. Keep that property as a rule for future edits: never move an invariant into the companion without leaving it in the ADR.
