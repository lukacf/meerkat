# GrantAuthorityMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::grant_authority`

## State
- Phase enum: `Unconfigured | Active`
- `root`: `Option<GrantPrincipal>`
- `namespace`: `Option<EvidenceId>`
- `generation`: `u64`
- `revision`: `u64`
- `records`: `Map<EvidenceId, GrantRecord>`
- `revoked`: `Set<EvidenceId>`

## Inputs
- `Configure`(root: GrantPrincipal, namespace: EvidenceId, generation: u64)
- `IssueRoot`(actor: GrantPrincipal, record: GrantRecord)
- `IssueChild`(actor: GrantPrincipal, record: GrantRecord, derived: DerivedChildRestrictions, chain: Seq<GrantRecord>, now_ms: u64)
- `Revoke`(actor: GrantPrincipal, record: GrantRecord)
- `ResolveUse`(namespace: EvidenceId, generation: u64, executor: GrantPrincipal, represented_subject: Option<GrantPrincipal>, leaf: GrantRecord, chain: Seq<GrantRecord>, now_ms: u64)

## Signals

## Effects
- `Configured`
- `Issued`(record: GrantRecord)
- `Revoked`(grant_id: EvidenceId)
- `UseResolved`(leaf: GrantRecord)

## Helpers
- `lifetime_current`(restrictions: ExecutionRestrictions, now_ms: u64) -> `Bool`
- `child_rank`(parent: ExecutionRestrictions, child: ExecutionRestrictions) -> `Bool`
- `chain_links`(chain: Seq<GrantRecord>, root: Option<GrantPrincipal>, now_ms: u64) -> `Bool`

## Invariants
- `configured_identity_is_present`
- `revoked_records_remain_present`

## Transitions
### `Configure`
- From: `Unconfigured`
- On: `Configure`(root, namespace, generation)
- Guards:
  - ``
- Emits: `Configured`
- To: `Active`

### `IssueRoot`
- From: `Active`
- On: `IssueRoot`(actor, record)
- Guards:
  - ``
  - ``
  - ``
  - ``
- Emits: `Issued`
- To: `Active`

### `IssueChild`
- From: `Active`
- On: `IssueChild`(actor, record, derived, chain, now_ms)
- Guards:
  - ``
  - ``
  - ``
  - ``
  - ``
- Emits: `Issued`
- To: `Active`

### `RevokeNew`
- From: `Active`
- On: `Revoke`(actor, record)
- Guards:
  - ``
  - ``
  - ``
  - ``
- Emits: `Revoked`
- To: `Active`

### `RevokeAlready`
- From: `Active`
- On: `Revoke`(actor, record)
- Guards:
  - ``
  - ``
  - ``
- Emits: `Revoked`
- To: `Active`

### `ResolveUse`
- From: `Active`
- On: `ResolveUse`(namespace, generation, executor, represented_subject, leaf, chain, now_ms)
- Guards:
  - ``
  - ``
  - ``
  - ``
- Emits: `UseResolved`
- To: `Active`

## Coverage
### Code Anchors
- `grant_authority` (machine `GrantAuthorityMachine`): `crates/meerkat-authorization/src/grants/mod.rs` — generated local grant issuance, revocation and full-lineage resolution; no persistence or caller authentication claim

### Scenarios
- `grant-three-level-attenuation` — grants/tests.rs::three_levels_use_exact_attenuation_and_zero_depth_only_forbids_children configures the real owner, issues root/child/leaf, resolves exact narrowed use and rejects another child at exhausted depth
- `grant-revoked-ancestor` — grants/tests.rs::revoked_ancestor_refuses_descendant_and_invalidates_existing_publication revokes an actual root and refuses descendant use and further child issuance
- `grant-immutable-id-and-idempotent-revoke` — grants/tests.rs::issuance_and_revocation_are_immutable_with_no_id_reuse rejects duplicate issuance before and after revocation and verifies repeated revoke does not advance revision
- `grant-fresh-chain-lifetime` — grants/tests.rs::full_chain_is_checked_at_fresh_time_without_cached_valid_state tests actual root/child lifetime boundaries with a changed owner clock and no cached valid state
