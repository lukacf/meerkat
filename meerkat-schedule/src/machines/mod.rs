//! DSL-generated machine state types for schedule lifecycle machines.
//!
//! These modules contain `machine!` invocations that generate the canonical
//! machine state structs, input/effect enums, and `apply()` dispatch for
//! both the OccurrenceLifecycle and ScheduleLifecycle machines.
//!
//! The generated state structs are the single source of truth for machine
//! state. Domain types (`Occurrence`, `Schedule`) compose these structs
//! alongside non-machine metadata.
#![allow(
    dead_code,
    unused_variables,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;
