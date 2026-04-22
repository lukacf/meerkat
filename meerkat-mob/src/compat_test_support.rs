#![allow(clippy::expect_used)]

use meerkat_machine_kernels::{compat_generated, test_oracle};

pub fn flow_run_state_from_raw(
    mut state: test_oracle::KernelState,
) -> compat_generated::flow_run::State {
    let mut seeded = meerkat_machine_kernels::legacy_generated::flow_run::initial_state()
        .expect("raw flow_run init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    compat_generated::flow_run::from_test_oracle_state(state).expect("typed flow_run state")
}

pub fn flow_run_state_to_raw(
    state: &compat_generated::flow_run::State,
) -> test_oracle::KernelState {
    compat_generated::flow_run::to_test_oracle_state(state)
}

pub fn flow_frame_state_from_raw(
    mut state: test_oracle::KernelState,
) -> compat_generated::flow_frame::State {
    let mut seeded = meerkat_machine_kernels::legacy_generated::flow_frame::initial_state()
        .expect("raw flow_frame init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    compat_generated::flow_frame::from_test_oracle_state(state).expect("typed flow_frame state")
}

pub fn flow_frame_state_to_raw(
    state: &compat_generated::flow_frame::State,
) -> test_oracle::KernelState {
    compat_generated::flow_frame::to_test_oracle_state(state)
}

pub fn loop_iteration_state_from_raw(
    mut state: test_oracle::KernelState,
) -> compat_generated::loop_iteration::State {
    let mut seeded = meerkat_machine_kernels::legacy_generated::loop_iteration::initial_state()
        .expect("raw loop_iteration init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    compat_generated::loop_iteration::from_test_oracle_state(state)
        .expect("typed loop_iteration state")
}

pub fn loop_iteration_state_to_raw(
    state: &compat_generated::loop_iteration::State,
) -> test_oracle::KernelState {
    compat_generated::loop_iteration::to_test_oracle_state(state)
}
