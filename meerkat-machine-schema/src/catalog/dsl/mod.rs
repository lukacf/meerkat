//! DSL-generated machine schemas.
//!
//! These modules contain `machine!` invocations that generate the same
//! `MachineSchema` values as the hand-written catalog entries. They use
//! `rust: "self"` so the generated `schema()` function references
//! `crate::MachineSchema` instead of `meerkat_machine_schema::MachineSchema`.
#![allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

/// Extension trait providing `.get()` on Option to support the `option_value`
/// schema pattern (`Expr::MapGet { map: Field(...), key: String("value") }`).
/// In the runtime dispatch code, `.get("value")` extracts the inner value.
/// This is only used by the generated dispatch code in this crate (which is
/// dead code — only the `schema()` function is called).
trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}
impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;

use crate::MachineSchema;

pub fn dsl_meerkat_machine() -> MachineSchema {
    meerkat_machine::MeerkatMachineState::schema()
}

pub fn dsl_mob_machine() -> MachineSchema {
    mob_machine::MobMachineState::schema()
}

pub fn dsl_schedule_lifecycle_machine() -> MachineSchema {
    schedule_lifecycle::ScheduleLifecycleMachineState::schema()
}

pub fn dsl_occurrence_lifecycle_machine() -> MachineSchema {
    occurrence_lifecycle::OccurrenceLifecycleMachineState::schema()
}
