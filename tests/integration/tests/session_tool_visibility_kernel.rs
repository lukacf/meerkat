#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_kernels::generated::meerkat;

fn tool_filter_all() -> String {
    serde_json::to_string(&meerkat_core::ToolFilter::All).expect("tool filter should serialize")
}

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
            fence_token: 3,
            generation: 1,
        }),
        &meerkat::EmptyContext,
    )
    .expect("prepare bindings")
    .next_state
}

#[test]
fn session_tool_visibility_kernel_publishes_committed_set_from_attached() {
    let attached = prepared_meerkat_state();

    let published = meerkat::transition(
        &attached,
        meerkat::Input::PublishCommittedVisibleSet(meerkat::inputs::PublishCommittedVisibleSet {
            active_filter: meerkat::ToolFilter::from(tool_filter_all()),
            staged_filter: meerkat::ToolFilter::from(tool_filter_all()),
            active_requested_deferred_names: BTreeSet::new(),
            staged_requested_deferred_names: BTreeSet::new(),
            active_visibility_revision: 0,
            staged_visibility_revision: 0,
        }),
        &meerkat::EmptyContext,
    )
    .expect("publish committed visible set");

    assert_eq!(published.next_state.phase, meerkat::Phase::Attached);
    assert_eq!(published.effects.len(), 1);
    assert!(matches!(
        published.effects[0],
        meerkat::Effect::CommittedVisibleSetPublished(_)
    ));
}

#[test]
fn session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state() {
    let attached = prepared_meerkat_state();
    let requested = meerkat::transition(
        &attached,
        meerkat::Input::RequestDeferredTools(meerkat::inputs::RequestDeferredTools {
            names: BTreeSet::from(["search".to_string(), "view_image".to_string()]),
            witnesses: BTreeMap::from([
                (
                    "search".to_string(),
                    meerkat::ToolVisibilityWitness::from("verified"),
                ),
                (
                    "view_image".to_string(),
                    meerkat::ToolVisibilityWitness::from("verified"),
                ),
            ]),
        }),
        &meerkat::EmptyContext,
    )
    .expect("request deferred tools")
    .next_state;

    let value = serde_json::to_value(&requested).expect("serialize typed state");
    let object = value.as_object().expect("typed state serializes as object");

    assert!(
        !object.contains_key("staged_visibility_revision"),
        "top-level machine should no longer mirror staged visibility revision",
    );
    assert!(
        !object.contains_key("active_visibility_revision"),
        "top-level machine should no longer mirror active visibility revision",
    );
    assert!(
        !object.contains_key("active_requested_deferred_names"),
        "top-level machine should no longer mirror active requested names",
    );
    assert!(
        !object.contains_key("staged_requested_deferred_names"),
        "top-level machine should no longer mirror staged requested names",
    );
}
