// Machine DSL integration tests.
//
// This crate depends on both the DSL macro and the schema crate, so it can
// verify both directions: runtime dispatch AND schema generation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cmp_owned,
    clippy::panic,
    unreachable_code,
    unused_variables,
    dead_code
)]

// Re-export for test modules
pub use meerkat_machine_schema;

mod meerkat_machine;
mod mob_machine;
mod occurrence_lifecycle;
mod schedule_lifecycle;
#[cfg(test)]
mod tests;
