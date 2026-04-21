#![allow(clippy::expect_used)]

use std::any::type_name_of_val;

use meerkat_machine_kernels::generated::{
    auth, meerkat, mob, occurrence_lifecycle, schedule_lifecycle,
};

fn assert_typed_state<T>(label: &str, state: &T) {
    let ty = type_name_of_val(state);
    assert!(
        !ty.contains("KernelState"),
        "{label} parity scaffold still resolves to legacy state type `{ty}`"
    );
}

#[test]
fn meerkat_coverage_scenarios_match_typed_modeled_kernel() {
    let state = meerkat::initial_state();
    assert_typed_state("MeerkatMachine", &state);
}

#[test]
fn mob_coverage_scenarios_match_typed_modeled_kernel() {
    let state = mob::initial_state();
    assert_typed_state("MobMachine", &state);
}

#[test]
fn schedule_coverage_scenarios_match_typed_modeled_kernel() {
    let state = schedule_lifecycle::initial_state();
    assert_typed_state("ScheduleLifecycleMachine", &state);
}

#[test]
fn occurrence_coverage_scenarios_match_typed_modeled_kernel() {
    let state = occurrence_lifecycle::initial_state();
    assert_typed_state("OccurrenceLifecycleMachine", &state);
}

#[test]
fn auth_coverage_scenarios_match_typed_modeled_kernel() {
    let state = auth::initial_state();
    assert_typed_state("AuthMachine", &state);
}
