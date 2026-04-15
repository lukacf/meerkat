#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::{KernelInput, KernelSignal, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn verified_witness() -> KernelValue {
    string("verified")
}

fn tool_filter_all() -> KernelValue {
    KernelValue::String(
        serde_json::to_string(&meerkat_core::ToolFilter::All)
            .expect("tool filter should serialize"),
    )
}

fn empty_string_set() -> KernelValue {
    KernelValue::Set(Default::default())
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

fn prepared_meerkat_state() -> meerkat_machine_kernels::KernelState {
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
fn session_tool_visibility_kernel_publishes_committed_set_from_attached() {
    let attached = prepared_meerkat_state();

    // PublishCommittedVisibleSet from Attached carries the active/staged owner
    // revisions in the input, but the top-level machine no longer stores a
    // separate active visibility mirror.
    let published = meerkat::transition(
        &attached,
        &input(
            "PublishCommittedVisibleSet",
            vec![
                ("active_filter", tool_filter_all()),
                ("staged_filter", tool_filter_all()),
                ("active_requested_deferred_names", empty_string_set()),
                ("staged_requested_deferred_names", empty_string_set()),
                ("active_visibility_revision", KernelValue::U64(0)),
                ("staged_visibility_revision", KernelValue::U64(0)),
            ],
        ),
    )
    .expect("publish committed visible set");

    // PrepareBindings promotes an idle registered session to Attached.
    assert_eq!(published.next_state.phase, "Attached");
    assert_eq!(published.effects.len(), 1);
    assert_eq!(published.effects[0].variant, "CommittedVisibleSetPublished");
}

#[test]
fn session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state() {
    let attached = prepared_meerkat_state();
    let requested = meerkat::transition(
        &attached,
        &input(
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

    assert!(
        !requested.fields.contains_key("staged_visibility_revision"),
        "top-level machine should no longer mirror staged visibility revision",
    );
    assert!(
        !requested.fields.contains_key("active_visibility_revision"),
        "top-level machine should no longer mirror active visibility revision",
    );
    assert!(
        !requested
            .fields
            .contains_key("active_requested_deferred_names"),
        "top-level machine should no longer mirror active requested names",
    );

    assert!(
        !requested
            .fields
            .contains_key("staged_requested_deferred_names"),
        "top-level machine should no longer mirror staged requested names",
    );
}
