# auth_lease_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `auth_lease_bundle`

### Code Anchors
- `auth_lease_handle` (machine `AuthMachine`): `meerkat-runtime/src/handles/auth_lease.rs` — runtime auth lease owner consumes canonical AuthMachine lifecycle acquire, refresh, reauth, release, wake, and publication events
- `auth_lease_bundle_schema` (machine `AuthMachine`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal AuthMachine lifecycle publication handoff composition

### Scenarios
- `auth-lease-lifecycle-publication` — AuthMachine acquire, refresh, reauth, release, wake, and lifecycle transitions publish through the explicit auth lease handoff protocol

### Routes
- `(none)`

### Scheduler Rules
- `(none)`

### Invariants
- `auth_release_oauth_flow_drain_protocol_covered`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `auth_lease_lifecycle_publication_protocol_covered`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
