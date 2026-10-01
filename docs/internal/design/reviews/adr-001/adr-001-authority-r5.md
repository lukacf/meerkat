# ADR-001 r5 bounded authority review

Verdict: GREEN. No material authority escape or regression found in the r5 delta. No mandatory wording change requested.

Reviewed files:

- `docs/internal/design/adr-001-runtime-security.md`: 847 lines, SHA-256 `dae4e6a4fab44b103ced096cd730a54668a4d617a78b72dd10c2ed30f88dd0b5`.
- `docs/internal/design/governed-deployment-profiles.md`: 218 lines, SHA-256 `4a745ab008ed8876a213ab311010bbe5be400f03b8876b87f8424ebe8249b174`.

Compared against frozen `candidate-r4.md` (`246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3`) and `candidate-r4-profiles.md` (`b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9`). This is the requested bounded delta review, not a new full architecture review or implementation proof. No source or ADR edits were made.

## Findings

1. Quarantine does not grant notification authority. Profiles lines 167-174 require a notification owner independent of reverted state, a surviving authority/witness proving its current mandate, and coverage of every required revocation dependency. The witness participates in the same-domain fence. Stale snapshots, missing invalidations and external-authority outages explicitly fail to establish permission. Cross-domain leases remain bound to main section 4. The unchanged notification contract at profiles 155-165 still requires bounded authenticated event sources, fixed recipients/payload, pre-dispatch evidence and ordinary owner settlement. Main lines 679-682 preserve the full owner-custody contract.

   Counterexample checked: restore generation N after a revocation at N+1, lose contact with the original authority, then send an alarm using the copied mandate and reserved receipt store. The new text rejects it. A monotonic number copied from N does not prove current mandate validity or all revocation dependencies. A surviving independent notification owner can operate only with its own proved current authority; it does not release the restored application from main lines 188-198 quarantine/fresh-incarnation requirements. The reserved backend remains evidence storage, not execution authority.

2. Adoption is not an administrator or participant-union bypass. Profiles lines 142-151 expressly deny adoption/release authority from store custody, generic administration or generic guardianship. A known person's privacy floor persists without an applicable release grant. Multi-person release must satisfy all applicable rights authorities, with representation requiring a specifically scoped grant. Unknown participants require explicit corpus-wide authority covering the uncertainty or refusal.

   Counterexample checked: an operator imports a backup, declares every known participant an audience member, and treats missing participants as absent. The new rules reject each inference. The unchanged lines 131-141 still bind exact immutable bytes, preserve known source restrictions, require new authorization for additions, retain bundle revocation and deny historical replay authority. Main lines 160-166 prevent an authenticated but unrelated authority from granting rights outside its registered domain. The exceptional adoption route remains an explicit authorized release transformation, not reconstructed historical provenance.

3. Stable-principal account release preserves audience checks. Profiles lines 72-81 require an account-binding authority and an explicit policy covering provider retention and the principal's account-authorized devices. An address or successful API call is insufficient. Revoked/lost binding blocks new sends, and shared/group accounts cannot inherit this class from one credential owner. This is compatible with the unchanged rule requiring all plaintext readers to enforce retained restrictions or belong to an authorized effective audience. It does not silently enable dynamic-group or physical-exposure profiles. Destination conformance remains required.

4. Classifier isolation prevents cross-item dependency erasure. Profiles lines 124-130 require a fresh per-item context without previous transcript, memory or provider cache, and retain every observed/control dependency on labels. A common sender or class does not establish independent context. Counterexample checked: item A controls a classifier's decision for B, but A is omitted from the output. The new text expressly denies an independent B label in that case. Existing bounded-outcome and explicit-release assumptions remain unchanged.

5. The two main-document changes narrow claims rather than permissions. Main lines 730-731 disclaim automatic inference of intent-dependent sensitivity; they do not waive declared resource, disclosure or product-policy requirements. Naming selective carry-forward as a later unsupported profile at lines 821-822 is consistent with the existing fresh-reset contract, which already defers selective imports.

All 24 r4 integration conformance rows remain unchanged. Four additional rows cover the new account, adoption, classifier and quarantine cases. No owner, fence, audit, recovery or unsupported-profile requirement was removed by this delta.

The GREEN verdict applies to the two r5 hashes above. Proving a surviving mandate witness, account binding, corpus-wide release authority or isolated classifier attempt remains production implementation and conformance work; this review supplies no such runtime evidence.

## Final-hash addendum: mechanical E1 scope column

Verdict remains GREEN after the bounded mechanical E1 check. Final main ADR SHA-256 remains `dae4e6a4fab44b103ced096cd730a54668a4d617a78b72dd10c2ed30f88dd0b5` (847 lines). Final profiles SHA-256 is `20392d72d41cb7f7c1703080a94300db4ddb14efe103f0a0bea38c38aab3dbf9` (222 lines).

Verified 28 integration data rows, excluding the header and separator: 15 Core rows and 13 Profile rows. Every Profile scope links to an existing section. Profiles lines 182-185 make applicability depend on enabled behavior regardless of profile name and expressly prohibit making an unconditional ADR invariant optional.

For an exact preservation check, I removed only the new four-line scope explanation and first table column from the final profiles document. The reconstructed document hashes to `4a745ab008ed8876a213ab311010bbe5be400f03b8876b87f8424ebe8249b174`, exactly the previously reviewed r5 profiles file. This proves the prior row payloads and all other content were unchanged. No full re-review was needed or performed.
