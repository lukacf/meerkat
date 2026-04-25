# auth_lease_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `auth_lease_bundle`

### Code Anchors
- `auth_lease_handle`: `meerkat-runtime/src/handles/auth_lease.rs` — runtime auth lease owner consumes canonical AuthMachine lifecycle publications
- `auth_lease_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal AuthMachine lifecycle publication handoff composition

### Scenarios
- `auth-lease-lifecycle-publication` — AuthMachine lifecycle transitions publish through the explicit auth lease handoff protocol

### Routes
- `(none)`

### Scheduler Rules
- `(none)`

### Invariants
- `auth_lease_lifecycle_publication_protocol_covered`
  - anchors: `auth_lease_handle`, `auth_lease_bundle_schema`
  - scenarios: `auth-lease-lifecycle-publication`


<!-- GENERATED_COVERAGE_END -->
