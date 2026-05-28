#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::test_oracle::{
    GeneratedMachineKernel, KernelInput, KernelSignal, KernelState, KernelValue,
};
use meerkat_machine_schema::identity::{
    FieldId, InputVariantId, NamedTypeId, PhaseId, SignalVariantId,
};

fn field(slug: &str) -> FieldId {
    FieldId::parse(slug).expect("field id")
}

fn input(slug: &str) -> InputVariantId {
    InputVariantId::parse(slug).expect("input id")
}

fn signal(slug: &str) -> SignalVariantId {
    SignalVariantId::parse(slug).expect("signal id")
}

fn phase(slug: &str) -> PhaseId {
    PhaseId::parse(slug).expect("phase id")
}

fn named_string(type_name: &str, value: &str) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("type id"),
        value: Box::new(KernelValue::String(value.into())),
    }
}

fn named_u64(type_name: &str, value: u64) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("type id"),
        value: Box::new(KernelValue::U64(value)),
    }
}

fn prepared_meerkat_state() -> KernelState {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let initialized = kernel
        .transition_signal(
            &kernel.initial_state().expect("initial state"),
            &KernelSignal {
                variant: signal("Initialize"),
                fields: BTreeMap::new(),
            },
        )
        .expect("initialize")
        .next_state;
    let registered = kernel
        .transition(
            &initialized,
            &KernelInput {
                variant: input("RegisterSession"),
                fields: BTreeMap::from([(
                    field("session_id"),
                    named_string("SessionId", "session-1"),
                )]),
            },
        )
        .expect("register session")
        .next_state;
    kernel
        .transition(
            &registered,
            &KernelInput {
                variant: input("PrepareBindings"),
                fields: BTreeMap::from([
                    (
                        field("agent_runtime_id"),
                        named_string("AgentRuntimeId", "runtime-7"),
                    ),
                    (field("fence_token"), named_u64("FenceToken", 3)),
                    (field("generation"), named_u64("Generation", 1)),
                    (field("runtime_epoch_id"), KernelValue::None),
                    (field("session_id"), named_string("SessionId", "session-1")),
                ]),
            },
        )
        .expect("prepare bindings")
        .next_state
}

#[test]
fn session_turn_admission_kernel_attached_state_reached() {
    let state = prepared_meerkat_state();
    assert_eq!(state.phase, phase("Attached"));
    assert_eq!(
        state.fields.get(&field("active_runtime_id")),
        Some(&KernelValue::Map(BTreeMap::from([(
            KernelValue::String("value".into()),
            named_string("AgentRuntimeId", "runtime-7"),
        )])))
    );
}

#[test]
fn session_turn_admission_kernel_interrupt_allowed_while_attached() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let state = prepared_meerkat_state();
    let next = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("InterruptCurrentRun"),
                fields: BTreeMap::new(),
            },
        )
        .expect("attached sessions should accept interrupts as a self-loop");
    assert_eq!(next.next_state.phase, phase("Attached"));
}

#[test]
fn session_turn_admission_kernel_cancel_boundary_allowed_while_attached() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let state = prepared_meerkat_state();
    let next = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("CancelAfterBoundary"),
                fields: BTreeMap::from([(field("reason"), KernelValue::String("test".into()))]),
            },
        )
        .expect("attached sessions should accept cancel-after-boundary as a self-loop");
    assert_eq!(next.next_state.phase, phase("Attached"));
}
