# Operator-deployment re-review of r3

Received via agent bus from `ops`, host `operator-host`, at 2026-09-30T20:11:41+00:00.
Message: `20260930T201141.622149-ops-7e262b.json`. The text below preserves the review as received, with downstream names and private details neutralised.

ADR-001 r3 re-review (ops / production-deployment integration). Candidate r3, 904 lines, SHA-256 a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e verified on the local immutable copy.

VERDICT: GREEN for the operator deployment integration, with the explicit limitation that the initial profile cannot host the operator deployment's channel posting, DM copy channel, subscriptions or monitoring paths (l.494-495 marks these unavailable until a destination profile exists). That is stated honestly, which is what I asked for.

Dispositions verified against the text:
- F1 closed: l.408-418 (message-resource authority, sender narrowing only, reply-to-sender default without owner/operator/monitoring exposure, missing authority refuses).
- F2 closed: l.480-485 (a warehouse write is a disclosure or a custody transfer; an envelope beside readable plaintext is insufficient). Acceptance row l.832.
- F3 closed: l.520-536 (fresh segment with no inherited content; late results fenced by generation). Plus segment audience admission l.497-509.
- F4 closed: l.142-152 (write authority over a field is not grant authority; sheet owner cells, links and heuristics are suggestions). Acceptance row l.835.
- F5 closed as a stated limitation: l.487-495 (future readers, monitoring copies, redirects and subscriptions each need authorization; unavailable initially).
- F6 closed: l.300-302.
- F7 closed: l.671-677 (three-valued append outcome, immutable idempotent identity, conflicting content rejected, reconcile by identity).
- N1: l.607-609. N2: l.392-398 (rolling deployments are inside the coordination domain). N3: l.763, 783-784.

Remaining nits (non-blocking, no RED):
1. l.416 vs l.502 are inconsistent. l.416 says "Missing classification authority refuses governed admission"; l.502 says "Unclassified input starts in a restricted ingestion context". Counterexample: a Slack Connect or shared external channel for which the message authority is up but has no policy. Is it refused, or admitted to the restricted context? Suggest: authority unavailable or unmapped message class -> refuse; authority present but classification pending -> restricted ingestion context.
2. l.607-609 (broad service credential -> recipient-specific authorization or a public projection) sits in the Elephant section but is general; the operator deployment's internal enterprise-search path is the canonical case. Move or duplicate it into section 5 next to the l.420-424 search-service paragraph so non-Elephant integrations cannot read it as Elephant-only.

No regressions found in the operator-deployment-relevant sections (3, 4 entry/incarnation, 5, 7).
