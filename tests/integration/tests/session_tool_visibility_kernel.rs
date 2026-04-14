#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::{KernelInput, KernelSignal, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn tool_filter_all() -> KernelValue {
    string("All")
}

fn verified_witness() -> KernelValue {
    string("verified")
}

fn input(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(field, value)| (field.to_string(), value))
            .collect(),
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

fn attached_meerkat_state() -> meerkat_machine_kernels::KernelState {
    let initialized = meerkat::transition_signal(
        &meerkat::initial_state().expect("initial state"),
        &signal("Initialize", vec![]),
    )
    .expect("initialize")
    .next_state;
    let registered = meerkat::transition(
        &initialized,
        &input("RegisterSession", vec![("session_id", string("session-1"))]),
    )
    .expect("register session")
    .next_state;
    meerkat::transition(
        &registered,
        &input(
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
fn session_tool_visibility_kernel_promotes_staged_filter_at_boundary() {
    let attached = attached_meerkat_state();
    let running = meerkat::transition_signal(
        &attached,
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
    let staged = meerkat::transition_signal(
        &running,
        &signal(
            "StagePersistentFilter",
            vec![
                ("filter", tool_filter_all()),
                (
                    "witnesses",
                    KernelValue::Map(BTreeMap::from([(string("search"), verified_witness())])),
                ),
            ],
        ),
    )
    .expect("stage filter")
    .next_state;

    assert_eq!(
        staged.fields.get("staged_visibility_revision"),
        Some(&KernelValue::U64(1))
    );
    assert_eq!(
        staged.fields.get("active_visibility_revision"),
        Some(&KernelValue::U64(0))
    );

    let promoted = meerkat::transition_signal(
        &staged,
        &signal("BoundaryApplied", vec![("revision", KernelValue::U64(1))]),
    )
    .expect("promote staged visibility");

    assert_eq!(promoted.next_state.phase, "Running");
    assert_eq!(
        promoted.next_state.fields.get("active_visibility_revision"),
        Some(&KernelValue::U64(1))
    );
    assert_eq!(
        promoted
            .next_state
            .fields
            .get("committed_visibility_revision"),
        Some(&KernelValue::U64(1))
    );
    assert_eq!(promoted.effects.len(), 1);
    assert_eq!(promoted.effects[0].variant, "CommittedVisibleSetPublished");
}

#[test]
fn session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state() {
    let attached = attached_meerkat_state();
    let requested = meerkat::transition_signal(
        &attached,
        &signal(
            "RequestDeferredTools",
            vec![
                (
                    "names",
                    KernelValue::Set(
                        vec![string("search"), string("view_image")]
                            .into_iter()
                            .collect(),
                    ),
                ),
                (
                    "witnesses",
                    KernelValue::Map(BTreeMap::from([
                        (string("search"), verified_witness()),
                        (string("view_image"), verified_witness()),
                    ])),
                ),
            ],
        ),
    )
    .expect("request deferred tools")
    .next_state;

    assert_eq!(
        requested.fields.get("staged_visibility_revision"),
        Some(&KernelValue::U64(1))
    );
    assert_eq!(
        requested.fields.get("active_visibility_revision"),
        Some(&KernelValue::U64(0))
    );
    assert_eq!(
        requested.fields.get("active_requested_deferred_names"),
        Some(&KernelValue::Set(Default::default()))
    );

    match requested.fields.get("staged_requested_deferred_names") {
        Some(KernelValue::Set(names)) => {
            assert!(names.contains(&string("search")));
            assert!(names.contains(&string("view_image")));
        }
        other => panic!("unexpected staged names shape: {other:?}"),
    }
}
