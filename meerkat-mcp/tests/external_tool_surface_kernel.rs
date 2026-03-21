#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::external_tool_surface;
use meerkat_machine_kernels::{KernelInput, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn input(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn map_value<'a>(
    state: &'a meerkat_machine_kernels::KernelState,
    field: &str,
    key: &str,
) -> Option<&'a KernelValue> {
    match state.fields.get(field) {
        Some(KernelValue::Map(values)) => values.get(&string(key)),
        _ => None,
    }
}

fn set_contains(state: &meerkat_machine_kernels::KernelState, name: &str, value: &str) -> bool {
    match state.fields.get(name) {
        Some(KernelValue::Set(values)) => values.contains(&string(value)),
        _ => false,
    }
}

fn effect_with_variant<'a>(
    outcome: &'a meerkat_machine_kernels::TransitionOutcome,
    variant: &str,
) -> &'a meerkat_machine_kernels::KernelEffect {
    outcome
        .effects
        .iter()
        .find(|effect| effect.variant == variant)
        .expect("expected effect variant")
}

#[test]
fn external_tool_surface_kernel_add_and_reload_emit_canonical_deltas() {
    let state = external_tool_surface::initial_state().expect("initial state");
    let staged_add = external_tool_surface::transition(
        &state,
        &input("StageAdd", vec![("surface_id", string("alpha"))]),
    )
    .expect("stage add");
    let apply_add = external_tool_surface::transition(
        &staged_add.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("alpha")),
                ("applied_at_turn", KernelValue::U64(7)),
            ],
        ),
    )
    .expect("apply add");
    let pending_add = effect_with_variant(&apply_add, "EmitExternalToolDelta");
    let scheduled_add = effect_with_variant(&apply_add, "ScheduleSurfaceCompletion");
    assert_eq!(apply_add.transition, "ApplyBoundaryAdd");
    assert_eq!(pending_add.fields.get("operation"), Some(&string("Add")));
    assert_eq!(pending_add.fields.get("phase"), Some(&string("Pending")));
    assert_eq!(
        pending_add.fields.get("persisted"),
        Some(&KernelValue::Bool(false))
    );
    assert_eq!(
        pending_add.fields.get("applied_at_turn"),
        Some(&KernelValue::U64(7))
    );

    let added = external_tool_surface::transition(
        &apply_add.next_state,
        &input(
            "PendingSucceeded",
            vec![
                ("surface_id", string("alpha")),
                ("operation", string("Add")),
                (
                    "pending_task_sequence",
                    scheduled_add
                        .fields
                        .get("pending_task_sequence")
                        .expect("pending add task sequence")
                        .clone(),
                ),
                (
                    "staged_intent_sequence",
                    scheduled_add
                        .fields
                        .get("staged_intent_sequence")
                        .expect("pending add intent sequence")
                        .clone(),
                ),
                ("applied_at_turn", KernelValue::U64(7)),
            ],
        ),
    )
    .expect("pending add succeeds");
    let applied_add = effect_with_variant(&added, "EmitExternalToolDelta");
    assert!(set_contains(&added.next_state, "visible_surfaces", "alpha"));
    assert_eq!(applied_add.fields.get("phase"), Some(&string("Applied")));
    assert_eq!(
        applied_add.fields.get("persisted"),
        Some(&KernelValue::Bool(true))
    );

    let staged_reload = external_tool_surface::transition(
        &added.next_state,
        &input("StageReload", vec![("surface_id", string("alpha"))]),
    )
    .expect("stage reload");
    let apply_reload = external_tool_surface::transition(
        &staged_reload.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("alpha")),
                ("applied_at_turn", KernelValue::U64(8)),
            ],
        ),
    )
    .expect("apply reload");
    let pending_reload = effect_with_variant(&apply_reload, "EmitExternalToolDelta");
    let scheduled_reload = effect_with_variant(&apply_reload, "ScheduleSurfaceCompletion");
    assert_eq!(
        pending_reload.fields.get("operation"),
        Some(&string("Reload"))
    );
    assert_eq!(pending_reload.fields.get("phase"), Some(&string("Pending")));

    let failed_reload = external_tool_surface::transition(
        &apply_reload.next_state,
        &input(
            "PendingFailed",
            vec![
                ("surface_id", string("alpha")),
                ("operation", string("Reload")),
                (
                    "pending_task_sequence",
                    scheduled_reload
                        .fields
                        .get("pending_task_sequence")
                        .expect("pending reload task sequence")
                        .clone(),
                ),
                (
                    "staged_intent_sequence",
                    scheduled_reload
                        .fields
                        .get("staged_intent_sequence")
                        .expect("pending reload intent sequence")
                        .clone(),
                ),
                ("applied_at_turn", KernelValue::U64(8)),
            ],
        ),
    )
    .expect("pending reload fails");
    let failed_delta = effect_with_variant(&failed_reload, "EmitExternalToolDelta");
    assert_eq!(
        failed_delta.fields.get("operation"),
        Some(&string("Reload"))
    );
    assert_eq!(failed_delta.fields.get("phase"), Some(&string("Failed")));
    assert_eq!(
        failed_delta.fields.get("persisted"),
        Some(&KernelValue::Bool(true))
    );
}

#[test]
fn external_tool_surface_kernel_remove_drain_completion_and_forced_finalize_emit_deltas() {
    let state = external_tool_surface::initial_state().expect("initial state");
    let staged_add = external_tool_surface::transition(
        &state,
        &input("StageAdd", vec![("surface_id", string("beta"))]),
    )
    .expect("stage add");
    let apply_add = external_tool_surface::transition(
        &staged_add.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("beta")),
                ("applied_at_turn", KernelValue::U64(10)),
            ],
        ),
    )
    .expect("apply add");
    let scheduled_active = effect_with_variant(&apply_add, "ScheduleSurfaceCompletion");
    let active = external_tool_surface::transition(
        &apply_add.next_state,
        &input(
            "PendingSucceeded",
            vec![
                ("surface_id", string("beta")),
                ("operation", string("Add")),
                (
                    "pending_task_sequence",
                    scheduled_active
                        .fields
                        .get("pending_task_sequence")
                        .expect("pending active task sequence")
                        .clone(),
                ),
                (
                    "staged_intent_sequence",
                    scheduled_active
                        .fields
                        .get("staged_intent_sequence")
                        .expect("pending active intent sequence")
                        .clone(),
                ),
                ("applied_at_turn", KernelValue::U64(10)),
            ],
        ),
    )
    .expect("pending succeeds");

    let call_started = external_tool_surface::transition(
        &active.next_state,
        &input("CallStarted", vec![("surface_id", string("beta"))]),
    )
    .expect("call started");
    assert_eq!(
        map_value(&call_started.next_state, "inflight_calls", "beta"),
        Some(&KernelValue::U64(1))
    );

    let staged_remove = external_tool_surface::transition(
        &call_started.next_state,
        &input("StageRemove", vec![("surface_id", string("beta"))]),
    )
    .expect("stage remove");
    let draining = external_tool_surface::transition(
        &staged_remove.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("beta")),
                ("applied_at_turn", KernelValue::U64(11)),
            ],
        ),
    )
    .expect("apply remove");
    let draining_delta = effect_with_variant(&draining, "EmitExternalToolDelta");
    assert_eq!(draining.transition, "ApplyBoundaryRemoveDraining");
    assert_eq!(
        draining_delta.fields.get("phase"),
        Some(&string("Draining"))
    );
    assert!(!set_contains(
        &draining.next_state,
        "visible_surfaces",
        "beta"
    ));

    let finished = external_tool_surface::transition(
        &draining.next_state,
        &input("CallFinished", vec![("surface_id", string("beta"))]),
    )
    .expect("call finished");
    assert_eq!(
        map_value(&finished.next_state, "inflight_calls", "beta"),
        Some(&KernelValue::U64(0))
    );
    let clean = external_tool_surface::transition(
        &finished.next_state,
        &input(
            "FinalizeRemovalClean",
            vec![
                ("surface_id", string("beta")),
                ("applied_at_turn", KernelValue::U64(11)),
            ],
        ),
    )
    .expect("finalize clean removal");
    let clean_delta = effect_with_variant(&clean, "EmitExternalToolDelta");
    assert_eq!(clean_delta.fields.get("phase"), Some(&string("Applied")));
    assert_eq!(
        effect_with_variant(&clean, "CloseSurfaceConnection")
            .fields
            .get("surface_id"),
        Some(&string("beta"))
    );

    let forced_state = external_tool_surface::transition(
        &state,
        &input("StageAdd", vec![("surface_id", string("gamma"))]),
    )
    .expect("stage add gamma");
    let forced_state = external_tool_surface::transition(
        &forced_state.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("gamma")),
                ("applied_at_turn", KernelValue::U64(12)),
            ],
        ),
    )
    .expect("apply add gamma");
    let scheduled_forced = effect_with_variant(&forced_state, "ScheduleSurfaceCompletion");
    let forced_state = external_tool_surface::transition(
        &forced_state.next_state,
        &input(
            "PendingSucceeded",
            vec![
                ("surface_id", string("gamma")),
                ("operation", string("Add")),
                (
                    "pending_task_sequence",
                    scheduled_forced
                        .fields
                        .get("pending_task_sequence")
                        .expect("pending forced task sequence")
                        .clone(),
                ),
                (
                    "staged_intent_sequence",
                    scheduled_forced
                        .fields
                        .get("staged_intent_sequence")
                        .expect("pending forced intent sequence")
                        .clone(),
                ),
                ("applied_at_turn", KernelValue::U64(12)),
            ],
        ),
    )
    .expect("gamma active");
    let forced_state = external_tool_surface::transition(
        &forced_state.next_state,
        &input("CallStarted", vec![("surface_id", string("gamma"))]),
    )
    .expect("gamma call started");
    let forced_state = external_tool_surface::transition(
        &forced_state.next_state,
        &input("StageRemove", vec![("surface_id", string("gamma"))]),
    )
    .expect("stage gamma remove");
    let forced_state = external_tool_surface::transition(
        &forced_state.next_state,
        &input(
            "ApplyBoundary",
            vec![
                ("surface_id", string("gamma")),
                ("applied_at_turn", KernelValue::U64(13)),
            ],
        ),
    )
    .expect("apply gamma remove");
    let forced = external_tool_surface::transition(
        &forced_state.next_state,
        &input(
            "FinalizeRemovalForced",
            vec![
                ("surface_id", string("gamma")),
                ("applied_at_turn", KernelValue::U64(13)),
            ],
        ),
    )
    .expect("forced finalize");
    let forced_delta = effect_with_variant(&forced, "EmitExternalToolDelta");
    assert_eq!(forced_delta.fields.get("phase"), Some(&string("Forced")));
    assert_eq!(
        forced_delta.fields.get("persisted"),
        Some(&KernelValue::Bool(true))
    );
}
