# GrantAuthorityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `GrantAuthorityMachine`

### Code Anchors
- `grant_authority` (machine `GrantAuthorityMachine`): `crates/meerkat-authorization/src/grants/mod.rs` — generated local grant issuance, revocation, full-lineage resolution and the custody end at the host's close or after an uncertain append; no persistence or caller authentication claim

### Scenarios
- `grant-three-level-attenuation` — grants/tests.rs::three_levels_use_exact_attenuation_and_zero_depth_only_forbids_children configures the real owner, issues root/child/leaf, resolves exact narrowed use and rejects another child at exhausted depth
- `grant-custody-release-at-close` — grants/durable_tests.rs::a_released_owner_frees_its_lease_while_a_reference_is_still_held releases the owner's custody at its host's close while another reference is held: the store restores at once, the straggler's change, revocation, resolution and lookup are refused, a second release changes nothing, and the straggler's last drop leaves the successor's lease intact
- `grant-uncertain-append-then-close` — grants/durable_tests.rs::an_uncertain_owner_still_releases_its_lease_at_close makes an append uncertain, refuses every later change and use, then releases the lease at the host's close so a successor restores what the store retained
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
- `ReleaseCustody`
  - anchors: `grant_authority`
  - scenarios: `grant-custody-release-at-close`
- `RecordUncertainAppend`
  - anchors: `grant_authority`
  - scenarios: `grant-uncertain-append-then-close`
- `ReleaseUncertainCustody`
  - anchors: `grant_authority`
  - scenarios: `grant-uncertain-append-then-close`
- `ReleaseCustodyAgain`
  - anchors: `grant_authority`
  - scenarios: `grant-custody-release-at-close`

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
- `CustodyReleased`
  - anchors: `grant_authority`
  - scenarios: `grant-custody-release-at-close`, `grant-uncertain-append-then-close`
- `CustodyUncertain`
  - anchors: `grant_authority`
  - scenarios: `grant-uncertain-append-then-close`

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
