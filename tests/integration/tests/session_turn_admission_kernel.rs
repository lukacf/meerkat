#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_kernels::generated::meerkat;

fn prepared_meerkat_state() -> meerkat::State {
    let initialized = meerkat::transition_signal(
        &meerkat::initial_state(),
        meerkat::Signal::Initialize(meerkat::signals::Initialize {}),
        &meerkat::EmptyContext,
    )
    .expect("initialize")
    .next_state;
    let registered = meerkat::transition(
        &initialized,
        meerkat::Input::RegisterSession(meerkat::inputs::RegisterSession {
            session_id: "session-1".into(),
        }),
        &meerkat::EmptyContext,
    )
    .expect("register session")
    .next_state;
    meerkat::transition(
        &registered,
        meerkat::Input::PrepareBindings(meerkat::inputs::PrepareBindings {
            agent_runtime_id: "runtime-7".into(),
            fence_token: meerkat::FenceToken(3),
            generation: meerkat::Generation(1),
        }),
        &meerkat::EmptyContext,
    )
    .expect("prepare bindings")
    .next_state
}

#[test]
fn session_turn_admission_kernel_attached_state_reached() {
    let state = prepared_meerkat_state();
    assert_eq!(state.phase, meerkat::Phase::Attached);
    assert_eq!(state.active_runtime_id, Some("runtime-7".into()));
}

#[test]
fn session_turn_admission_kernel_interrupt_allowed_while_attached() {
    let state = prepared_meerkat_state();
    let next = meerkat::transition(
        &state,
        meerkat::Input::InterruptCurrentRun(meerkat::inputs::InterruptCurrentRun {}),
        &meerkat::EmptyContext,
    )
    .expect("attached sessions should accept interrupts as a self-loop");
    assert_eq!(next.next_state.phase, meerkat::Phase::Attached);
}

#[test]
fn session_turn_admission_kernel_cancel_boundary_allowed_while_attached() {
    let state = prepared_meerkat_state();
    let next = meerkat::transition(
        &state,
        meerkat::Input::CancelAfterBoundary(meerkat::inputs::CancelAfterBoundary {}),
        &meerkat::EmptyContext,
    )
    .expect("attached sessions should accept cancel-after-boundary as a self-loop");
    assert_eq!(next.next_state.phase, meerkat::Phase::Attached);
}
