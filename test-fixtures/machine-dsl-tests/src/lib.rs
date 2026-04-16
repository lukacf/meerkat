// Machine DSL integration tests.
//
// This crate depends on both the DSL macro and the schema crate, so it can
// verify both directions: runtime dispatch AND schema generation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cmp_owned,
    clippy::panic,
    clippy::assign_op_pattern,
    clippy::iter_cloned_collect,
    unreachable_code,
    unused_variables,
    unused_assignments,
    dead_code
)]

// Re-export for test modules
pub use meerkat_machine_schema;

pub mod map_keyed_authority;
pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;
#[cfg(test)]
mod tests;

/// Returns all 4 DSL-generated machine schemas.
///
/// This is the DSL equivalent of `canonical_machine_schemas()` from the
/// hand-written catalog. Used for aggregate equivalence testing and as the
/// future replacement source.
pub fn dsl_machine_schemas() -> Vec<meerkat_machine_schema::MachineSchema> {
    vec![
        meerkat_machine::MeerkatMachineState::schema(),
        mob_machine::MobMachineState::schema(),
        schedule_lifecycle::ScheduleLifecycleMachineState::schema(),
        occurrence_lifecycle::OccurrenceLifecycleMachineState::schema(),
    ]
}
