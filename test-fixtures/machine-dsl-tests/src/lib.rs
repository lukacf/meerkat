// Machine DSL integration tests.
//
// This crate depends on both the DSL macro and the schema crate, so it can
// verify both directions: runtime dispatch AND schema generation.

// Re-export for test modules
pub use meerkat_machine_schema;

mod occurrence_lifecycle;
mod schedule_lifecycle;
#[cfg(test)]
mod tests;
