#![allow(clippy::expect_used, dead_code)]

use meerkat_machine_kernels::{compat_generated, test_oracle};
use meerkat_mob::runtime::flow_kernels::{flow_frame, flow_run, loop_iteration};
use serde::{Serialize, de::DeserializeOwned};

fn compat_to_local<TLocal, TCompat>(value: TCompat) -> TLocal
where
    TLocal: DeserializeOwned,
    TCompat: Serialize,
{
    serde_json::from_value(serde_json::to_value(value).expect("serialize compat state"))
        .expect("deserialize local state")
}

fn local_to_compat<TCompat, TLocal>(value: &TLocal) -> TCompat
where
    TCompat: DeserializeOwned,
    TLocal: Serialize,
{
    serde_json::from_value(serde_json::to_value(value).expect("serialize local state"))
        .expect("deserialize compat state")
}

pub fn flow_run_state_from_raw(mut state: test_oracle::KernelState) -> flow_run::State {
    let mut seeded =
        test_oracle::legacy_generated::flow_run::initial_state().expect("raw flow_run init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    let compat_state = test_oracle::compat_generated::flow_run::from_test_oracle_state(state)
        .expect("typed flow_run state");
    compat_to_local(compat_state)
}

pub fn flow_run_state_to_raw(state: &flow_run::State) -> test_oracle::KernelState {
    let compat_state: compat_generated::flow_run::State = local_to_compat(state);
    test_oracle::compat_generated::flow_run::to_test_oracle_state(&compat_state)
}

pub fn flow_frame_state_from_raw(mut state: test_oracle::KernelState) -> flow_frame::State {
    let mut seeded =
        test_oracle::legacy_generated::flow_frame::initial_state().expect("raw flow_frame init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    let compat_state = test_oracle::compat_generated::flow_frame::from_test_oracle_state(state)
        .expect("typed flow_frame state");
    compat_to_local(compat_state)
}

pub fn flow_frame_state_to_raw(state: &flow_frame::State) -> test_oracle::KernelState {
    let compat_state: compat_generated::flow_frame::State = local_to_compat(state);
    test_oracle::compat_generated::flow_frame::to_test_oracle_state(&compat_state)
}

pub fn loop_iteration_state_from_raw(mut state: test_oracle::KernelState) -> loop_iteration::State {
    let mut seeded = test_oracle::legacy_generated::loop_iteration::initial_state()
        .expect("raw loop_iteration init");
    seeded.phase = state.phase;
    seeded.fields.extend(state.fields);
    state = seeded;
    let compat_state = test_oracle::compat_generated::loop_iteration::from_test_oracle_state(state)
        .expect("typed loop_iteration state");
    compat_to_local(compat_state)
}

pub fn loop_iteration_state_to_raw(state: &loop_iteration::State) -> test_oracle::KernelState {
    let compat_state: compat_generated::loop_iteration::State = local_to_compat(state);
    test_oracle::compat_generated::loop_iteration::to_test_oracle_state(&compat_state)
}
