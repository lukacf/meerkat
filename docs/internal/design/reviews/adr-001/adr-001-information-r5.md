# ADR-001 information and identity review, bounded r5 delta

Verdict: GREEN. The delta from frozen r4 preserves the established runtime invariants. It clarifies one stable-recipient destination class and strengthens adoption, classifier-isolation, and quarantined-notification requirements. No material widening or contradiction was found.

Reviewed on 2026-09-30 against `reviews/adr-001/candidate-r4.md` and `reviews/adr-001/candidate-r4-profiles.md`. No source or document edits were made. No builds, security tests, or live operations were performed.

## Exact reviewed pair

| Artifact | Lines | SHA-256 |
| --- | --- | --- |
| `/Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md` | 847 | `dae4e6a4fab44b103ced096cd730a54668a4d617a78b72dd10c2ed30f88dd0b5` |
| `/Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/governed-deployment-profiles.md` | 218 | `4a745ab008ed8876a213ab311010bbe5be400f03b8876b87f8424ebe8249b174` |

## Material findings

None. No I-series RED findings.

## Delta adjudication

### Individually controlled external accounts

Companion lines 72-81 permit the initial stable-recipient profile to release to an authenticated principal's individually controlled external account under an explicit release contract. This is a coherent clarification of stable recipient identity, not an exception for arbitrary channels or addresses.

The account-binding authority must verify the principal/account relationship; policy must expressly allow provider retention and the principal's authorized devices; binding revocation blocks new sends; shared/group accounts and compromised or unknowable control do not qualify merely through credential possession. The existing later-read and dynamic-audience rules remain at companion lines 83-104, and the runtime still requires actual recipient and source-constraint authorization.

Counterexamples checked: sending to an unverified address because the API accepted it; classifying a household-shared account as a personal account because one parent owns its credential; and treating this rule as authorization for a later group member. None satisfies the new conditions. Retention by the authorized principal's own account is an explicitly permitted irreversible release, not a promise that previously delivered copies can be revoked. New test coverage is at companion line 205.

### Legacy adoption and participant rights

Companion lines 142-151 explicitly reject custody, operator/admin status, generic guardianship, participant union, and guessed membership as adoption/release authority. A proven per-person corpus retains its privacy floor absent applicable release authority; multi-person release must satisfy all applicable rights authorities, including properly scoped representation where policy allows it. Unknown participants still require explicit corpus-wide authority covering that uncertainty or the bundle remains refused.

This strengthens, rather than contradicts, the existing exceptional-adoption contract at lines 131-141. Unknown provenance remains unknown; the adoption grant explicitly covers it within its declared bounds. Known source restrictions remain binding, new bytes require authorization, derivatives retain the bundle dependency, revocation blocks later use, and no historical execution authority is created.

Counterexamples checked: a storage administrator adopts a child's corpus; a parent invokes generic guardianship for every participant's private messages; a union of conversation participants is declared entitled to every constituent message. The additions explicitly reject all three shortcuts. They do not pretend that an authorization assumption reconstructs historical identity. New test coverage is at companion line 206.

### Per-item classifier isolation

Companion lines 124-130 require a fresh per-item context with fixed authorized configuration and no prior transcript, memory, or provider cache. Labels are derived outputs and keep all observed and control dependencies. A future batch must carry the complete batch dependencies, and common source principal/class is not evidence of independence.

This closes the subtle case where classification of item B is influenced by item A even if the classifier returns only B's label. It is consistent with the runtime's pre-inference partitioning and conservative dependency rules. It does not claim that isolation solves semantic classification error: lines 116-120 still explicitly identify broad outcomes as preauthorized release/trust assumptions and deny injection-proof correctness claims. New test coverage is at companion line 207.

### Quarantined notifications

Companion lines 167-174 require a notification owner independent of reverted state and a surviving rollback-resistant authority or monotonic witness proving the current mandate and all required revocation dependencies. Same-domain proof must participate in the fence; any independent-domain lease remains subject to ADR section 4. A stale copy, missed invalidation, remote outage, or missing durable capacity cannot authorize an alarm.

This is compatible with both the main quarantine rule and the existing reserved-backend contract. It allows an independently valid alarm path to keep operating without pretending that the quarantined snapshot is valid. It expressly makes alarms unavailable when the necessary proof or evidence capacity is absent. New test coverage is at companion line 208.

Counterexample checked: a restored host sends using a copied pre-revocation alarm mandate while the current authority is unreachable. The new text directly forbids it; there is no availability-driven fail-open exception.

### Main-document limitations and deferred carry-forward

The added intent-sensitive-information limitation is consistent with the existing distinction between authorization, source truth, and prompt-injection resistance. It does not waive configured purpose, recipient, or resource constraints; it accurately refuses to claim that generic authorization infers motives such as gift probing or partner surveillance.

Adding selective carry-forward to the deferred-profile list is consistent with the existing first-profile reset. That reset already excludes inherited content and defers selective carry-forward. This edit does not enable old user messages, summaries, metadata, or provider state to cross into a fresh segment.

## Conclusion

The exact r5 pair is suitable for continued project-owner review. Its new destination allowance is explicitly bound to verified individual control and a retention/release policy; its other substantive changes narrow unsafe interpretations. Optional grants and destination profiles still require the declared authority and implementation conformance evidence. This review does not supply those deployment-specific grants or implementation proofs.
