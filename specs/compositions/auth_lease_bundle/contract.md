# auth_lease_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `auth_machine`: `AuthMachine` @ actor `auth_machine_authority`

## Routes

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `(none)`

## Scheduler Rules
- `(none)`

## Structural Requirements
- `auth_lease_lifecycle_publication_protocol_covered` — every AuthMachine lifecycle-phase transition's external publication crosses into the runtime auth-lease owner through the explicit `auth_lease_lifecycle_publication` protocol rather than ad-hoc shell observation

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `meerkat-runtime/src/handles/auth_lease.rs` — runtime auth lease owner consumes canonical AuthMachine lifecycle publications
- `meerkat-machine-schema/src/catalog/compositions.rs` — formal AuthMachine lifecycle publication handoff composition

### Scenarios
- `auth-lease-lifecycle-publication` — AuthMachine lifecycle transitions publish through the explicit auth lease handoff protocol
