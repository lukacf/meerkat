# Operator-deployment exact r5 verdict

Received via agent bus at 2026-09-30T20:16:17+00:00.
Message: `20260930T201617.074607-ops-f747b3.json`. Text preserved as received, with downstream names and private details neutralised.

ADR-001 r5 final pair re-review (ops / production-deployment integration). Both hashes verified on the frozen copies:
- adr-001-runtime-security.md: 847 lines, SHA-256 dae4e6a4fab44b103ced096cd730a54668a4d617a78b72dd10c2ed30f88dd0b5
- governed-deployment-profiles.md: 222 lines, SHA-256 20392d72d41cb7f7c1703080a94300db4ddb14efe103f0a0bea38c38aab3dbf9

VERDICT: GREEN on both. My r3/r4 findings still hold.

Delta checked:
- The main ADR diff against r4 is exactly two additions: the intent-dependent-sensitivity limitation (l.730-731) and "selective carry-forward" added to the open profiles (l.821). Neither touches my scope.
- The operator-deployment-relevant invariants are unchanged in the ADR: l.146 (writers are authorized to grant) and l.481 onward (every persistent write or external send is a disclosure).
- Scope column: the operator deployment acceptance rows are now tagged as follows.
  - Profile: conversation, stored disclosure, imported attributes, external disclosure.
  - Core: admin-through-agent, reset, receipt-ack loss, replica takeover.
  Profile l.182-185 states the column never makes an unconditional ADR invariant optional, and Profile rows are mandatory whenever the behavior is enabled. No weakening.
- The downstream app N1-N4 do not affect the operator deployment. N2 (custody, operator, admin or guardianship are not adoption rights) reinforces my F6.

One non-blocking clarification: define "enabled" for Profile rows by observed capability, not by declaration. Any deployment with a persistent-write or external-send path has "stored/external disclosure" enabled, and any deployment that ingests access-conferring fields has "imported attributes" enabled. That way an app cannot opt out of the warehouse or sheet-owner tests by not naming the profile. The operator deployment has both behaviors, so these rows are mandatory for it.
