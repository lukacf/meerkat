#![allow(clippy::expect_used)]

use meerkat_machine_schema::catalog::dsl::dsl_mob_machine as canonical_mob_machine;
use meerkat_mob::machines::mob_machine::MobMachineState as RuntimeMobMachineState;

#[test]
fn canonical_catalog_mob_machine_matches_runtime_owner_schema() {
    let mut runtime_schema = RuntimeMobMachineState::schema();
    let canonical_schema = canonical_mob_machine();

    // The owner DSL and the catalog copy intentionally bind different Rust
    // modules, but the semantic machine schema must stay identical.
    runtime_schema.rust = canonical_schema.rust.clone();

    assert_eq!(runtime_schema, canonical_schema);
}
