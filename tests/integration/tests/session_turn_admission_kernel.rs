#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::{KernelInput, KernelSignal, KernelValue};

fn input(variant: &str) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: BTreeMap::new(),
    }
}

fn signal(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelSignal {
    KernelSignal {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(field, value)| (field.to_string(), value))
            .collect(),
    }
}

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn input_with_fields(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(field, value)| (field.to_string(), value))
            .collect(),
    }
}

fn prepared_meerkat_state() -> meerkat_machine_kernels::KernelState {
    let initialized = meerkat::transition_signal(
        &meerkat::initial_state().expect("initial state"),
        &signal("Initialize", vec![]),
    )
    .expect("initialize")
    .next_state;
    let registered = meerkat::transition(
        &initialized,
        &input_with_fields("RegisterSession", vec![("session_id", string("session-1"))]),
    )
    .expect("register session")
    .next_state;
    meerkat::transition(
        &registered,
        &input_with_fields(
            "PrepareBindings",
            vec![
                ("agent_runtime_id", string("runtime-7")),
                ("fence_token", KernelValue::U64(3)),
                ("generation", KernelValue::U64(1)),
            ],
        ),
    )
    .expect("prepare bindings")
    .next_state
}

#[test]
fn session_turn_admission_kernel_attached_state_reached() {
    let state = prepared_meerkat_state();
    // PrepareBindings promotes an idle registered session into Attached while
    // recording the runtime binding fields.
    assert_eq!(state.phase, "Attached");
    // Option<T> in the kernel is Map({"value" => T})
    assert_eq!(
        state.fields.get("active_runtime_id"),
        Some(&KernelValue::Map(BTreeMap::from([(
            KernelValue::String("value".to_string()),
            KernelValue::String("runtime-7".to_string()),
        )])))
    );
}

#[test]
fn session_turn_admission_kernel_interrupt_allowed_while_attached() {
    let state = prepared_meerkat_state();
    let next = meerkat::transition(&state, &input("InterruptCurrentRun"))
        .expect("attached sessions should accept interrupts as a self-loop");
    assert_eq!(next.next_state.phase, "Attached");
}

#[test]
fn session_turn_admission_kernel_cancel_boundary_allowed_while_attached() {
    let state = prepared_meerkat_state();
    let next = meerkat::transition(&state, &input("CancelAfterBoundary"))
        .expect("attached sessions should accept cancel-after-boundary as a self-loop");
    assert_eq!(next.next_state.phase, "Attached");
}
