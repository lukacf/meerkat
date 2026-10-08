# Meerkat Toolkit product review of r2

Received via agent bus from `connector-codex-local`, host `operator-host`, at 2026-09-30T20:05:38+00:00.
Message: `20260930T200538.019276-connector-codex-local-b84b93.json`. The text below preserves the review as received, with downstream names and private details neutralised.

# ADR-001 r2: Toolkit consolidated review

Verdict: **GREEN for architectural compatibility with Toolkit**. No blocking design defect found. Reviewed all 568 lines of candidate-r2.md, SHA-256 `44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca`.

This is a design review. It does not approve an implementation, select the concrete C1 carrier/API, establish runtime enforcement, or authorize a release. Luka owns the architecture decision; the Meerkat/MobKit lead owns upstream technical acceptance below Luka. The root independently reviewed operating profiles, exact operation identity, SDK/transport negotiation, native owner composition, data and tool fidelity. A separate reviewer examined Toolkit-specific acquisition, authentication, service and frontend roles, custom extensions, shared preparation and recovery. Its full report is retained as `independent-toolkit-review.md` in this directory. Neither reviewer re-audited all source observations cited at the ADR's pinned baselines or executed an implementation.

## Why the attempted counterexamples are covered

- A portable MCP server, skill or native package is installed and thereby acquires authority. Sections 1-4 instead require declared operations, a negotiated profile, current authority, exact resolved bindings and realizable obligations (106-140, 150-219). Installation and catalog trust cannot substitute for any of those. Unsupported resource semantics must refuse the governed path. The ordinary trusted-embedded profile remains available without making governed claims.
- An SDK, adapter or recovered session omits unfamiliar authority and defaults to the host's credentials. Persisted profiles cannot downgrade and handoffs must negotiate support (129-140); durable native associations survive retry/recovery and cannot be fabricated from credentials (150-163). Cross-version stripping is an explicit acceptance case (510-511).
- Two setup waiters share a prepared account/package, so cancelling one either revokes the shared credential or lets it borrow the other's result. The exact independent authority and conflict rule, ancestor validity and separately issued service mandate cover this (150-181). Shared physical preparation need not merge request contexts or role grants. First-profile batching limits (316-323) cannot be bypassed by treating a coalescing key as an authorization identity.
- A connector's account permission exposes a private agent through a frontend, or a monitoring copy widens its audience. Feature-owned operations and separate disclosure authorization preserve the two roles (106-111, 205-219, 310-323). Toolkit supplies their domain declarations; no second common policy engine is needed.
- A custom extension reports a successful launch, then a daemon performs later privileged work after revocation. Entry/checkpoint semantics and actual deferred-effect ownership cover this (221-250, 394-407). The trusted-native-code boundary is explicit (122-127); it does not claim arbitrary in-process code is sandboxed.
- A successful external operation loses its receipt and recovery executes it again. The existing owner retains attempt custody, known result and evidence-pending are distinct, and lost knowledge becomes Unknown (387-426). Security evidence is not a second execution queue.

No minimum architectural text fix is required for these cases. The concrete clauses block the counterexamples; passing implementation evidence is still required.

## Nonblocking integration acceptance requirements

1. **Keep the profile visible across the full host path.** Bootstrap, Python/TypeScript SDK ingress, queueing, resume, delegation, MCP dispatch and frontend release must preserve the same required capability. Demonstrate an unsupported or older adapter refuses before a recorded protected sink receives bytes. Repeat after restart. No missing field or absent controller may select trusted-embedded implicitly.
2. **Exercise actual independent acquisition owners.** Run two real waiters sharing one exact preparation, with different ceilings/deadlines. Revoke/cancel one, restart, deliver a late callback and attach a later waiter. Assert separately authorized grants, activations and recipient deliveries. A successful shared preparation must neither authorize the revoked waiter nor rotate a credential still owned by another user/service.
3. **Keep human authentication outside model observation.** Bind the ceremony to the exact provider, owner, account, scopes, redirect and attempt; exercise callback replay, account substitution and supersession. Secret canaries must remain absent from model input, elicitation-visible results, browser/DOM capture, transcripts, streams and ordinary logs. A completed login is neither an effect approval nor original-requester identity. Required non-repeatable ceremony budgets remain with the domain owner.
4. **Test both package roles and a real custom extension path.** A package exposing service access and an agent frontend must permit one while denying the other. Test last-hop and monitoring-copy disclosure. For a Reachy-style deferred owner, distinguish launch from subsequent physical/control effects, reject an unauthorized new effect and preserve truthful settlement after an entered one. Generic write classification cannot replace domain obligations.
5. **Prove the distributed owners, not a fixture-wide lock.** Revocation races must involve actual grant, policy, resource and binding owners. Exercise audit failure after known success, lost acknowledgement and process death; recover the native attempt without a second effect. Fence/lease, credential exchange and store integrity decisions listed as open must be fixed before that slice is accepted.

The independent report expands these cases, including typed service commissioning without requester access to private intermediates. This list is an implementation contract, not an architectural defect list or a request for a new Toolkit security machine.

The first governed profile is deliberately narrower than the complete Toolkit end state. Buffered output and unsupported mixed-context, dynamic-audience and multi-principal batching are honest compatibility limits. They must remain explicit in product readiness and later acceptance work; they do not remove any revision 8 requirement from the Toolkit goal. Likewise, future hostile-plugin containment requires its own proven isolation profile.

Local full evidence: (operator-retained path)
