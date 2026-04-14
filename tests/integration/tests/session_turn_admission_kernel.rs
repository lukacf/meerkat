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

fn attached_meerkat_state() -> meerkat_machine_kernels::KernelState {
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
fn session_turn_admission_kernel_gracefully_drains_running_shutdown() {
    let state = attached_meerkat_state();
    let running = meerkat::transition_signal(
        &state,
        &signal(
            "SubmitMobWork",
            vec![
                ("agent_runtime_id", string("runtime-7")),
                ("fence_token", KernelValue::U64(3)),
                ("work_id", string("work-1")),
            ],
        ),
    )
    .expect("submit work")
    .next_state;
    let shutdown = meerkat::transition(&running, &input("CancelAfterBoundary"))
        .expect("request boundary cancel")
        .next_state;
    assert_eq!(shutdown.phase, "Running");
    assert_eq!(
        shutdown.fields.get("shutdown_pending"),
        Some(&KernelValue::Bool(true))
    );

    let finalized = meerkat::transition_signal(
        &shutdown,
        &signal("RunCompleted", vec![("work_id", string("work-1"))]),
    )
    .expect("complete run")
    .next_state;
    assert_eq!(finalized.phase, "Attached");
    assert_eq!(
        finalized.fields.get("active_work_id"),
        Some(&KernelValue::None)
    );
    assert_eq!(
        finalized.fields.get("shutdown_pending"),
        Some(&KernelValue::Bool(false))
    );
}

#[test]
fn session_turn_admission_kernel_interrupt_only_wakes_running_turns() {
    let state = attached_meerkat_state();
    assert!(
        meerkat::transition(&state, &input("InterruptCurrentRun")).is_err(),
        "attached sessions without active work must reject interrupts"
    );

    let running = meerkat::transition_signal(
        &state,
        &signal(
            "SubmitMobWork",
            vec![
                ("agent_runtime_id", string("runtime-7")),
                ("fence_token", KernelValue::U64(3)),
                ("work_id", string("work-2")),
            ],
        ),
    )
    .expect("submit work")
    .next_state;
    let interrupted =
        meerkat::transition(&running, &input("InterruptCurrentRun")).expect("running interrupt");
    assert_eq!(interrupted.next_state.phase, "Running");
    assert_eq!(
        interrupted.next_state.fields.get("interrupt_pending"),
        Some(&KernelValue::Bool(true))
    );
    let effect_names = interrupted
        .effects
        .iter()
        .map(|effect| effect.variant.as_str())
        .collect::<Vec<_>>();
    assert_eq!(interrupted.effects.len(), 2);
    assert!(effect_names.iter().any(|name| name == &"WakeInterrupt"));
    assert!(
        effect_names
            .iter()
            .any(|name| name == &"RequestCancellationAtBoundary")
    );
}
