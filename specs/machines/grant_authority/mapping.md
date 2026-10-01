# GrantAuthorityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `GrantAuthorityMachine`

### Code Anchors
- `grant_authority` (machine `GrantAuthorityMachine`): `crates/meerkat-authorization/src/grants/mod.rs` — generated local grant issuance, revocation and full-lineage resolution; no persistence or caller authentication claim

### Scenarios
- `grant-three-level-attenuation` — grants/tests.rs::three_levels_use_exact_attenuation_and_zero_depth_only_forbids_children configures the real owner, issues root/child/leaf, resolves exact narrowed use and rejects another child at exhausted depth
- `grant-revoked-ancestor` — grants/tests.rs::revoked_ancestor_refuses_descendant_and_invalidates_existing_publication revokes an actual root and refuses descendant use and further child issuance
- `grant-immutable-id-and-idempotent-revoke` — grants/tests.rs::issuance_and_revocation_are_immutable_with_no_id_reuse rejects duplicate issuance before and after revocation and verifies repeated revoke does not advance revision
- `grant-fresh-chain-lifetime` — grants/tests.rs::full_chain_is_checked_at_fresh_time_without_cached_valid_state tests actual root/child lifetime boundaries with a changed owner clock and no cached valid state

### Transitions
- `Configure`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`
- `IssueRoot`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`, `grant-immutable-id-and-idempotent-revoke`
- `IssueChild`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`, `grant-revoked-ancestor`, `grant-fresh-chain-lifetime`
- `RevokeNew`
  - anchors: `grant_authority`
  - scenarios: `grant-revoked-ancestor`, `grant-immutable-id-and-idempotent-revoke`
- `RevokeAlready`
  - anchors: `grant_authority`
  - scenarios: `grant-immutable-id-and-idempotent-revoke`
- `ResolveUse`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`, `grant-revoked-ancestor`, `grant-fresh-chain-lifetime`

### Effects
- `Configured`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`
- `Issued`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`, `grant-immutable-id-and-idempotent-revoke`, `grant-fresh-chain-lifetime`
- `Revoked`
  - anchors: `grant_authority`
  - scenarios: `grant-revoked-ancestor`, `grant-immutable-id-and-idempotent-revoke`
- `UseResolved`
  - anchors: `grant_authority`
  - scenarios: `grant-three-level-attenuation`, `grant-fresh-chain-lifetime`

### Invariants
- `configured_identity_is_present`
  - anchors: `grant_authority`
  - scenarios: (unclaimed)
- `unconfigured_state_is_empty`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `revision_accounts_for_retained_mutations`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `issued_records_have_exact_identity_and_revision`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `issued_records_belong_to_this_incarnation`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `revoked_records_remain_present`
  - anchors: `grant_authority`
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
