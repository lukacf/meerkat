use super::dsl::command_capabilities::{
    AuthorizedRuntimeLoopBatch, AuthorizedStageForRun, RuntimeLoopBatchSource,
};
use super::dsl::{
    AdmissionContinuationKind, AdmissionInputKind, InputLane, InputPhase, MeerkatMachineAuthority,
    MeerkatMachineInput, MeerkatMachineMutator, MeerkatMachineState, MeerkatPhase, PreRunPhase,
    RecoveredInputKind, RecoveredRunApplyBoundary, RecoveredRuntimeExecutionKind, RunId,
};

fn authority() -> MeerkatMachineAuthority {
    MeerkatMachineAuthority::recover_from_state(MeerkatMachineState {
        lifecycle_phase: MeerkatPhase::Running,
        current_run_id: Some(RunId("run".into())),
        pre_run_phase: Some(PreRunPhase::Idle),
        ..Default::default()
    })
    .unwrap()
}

// These fixtures exercise generated queue/stage authority, not the separate
// sealed ingress or physical execution permission.
fn queue(owner: &mut MeerkatMachineAuthority, id: &str, live: bool) {
    queue_kind(
        owner,
        id,
        if live {
            AdmissionInputKind::LiveRequest
        } else {
            AdmissionInputKind::PeerMessage
        },
    );
}

fn queue_kind(owner: &mut MeerkatMachineAuthority, id: &str, kind: AdmissionInputKind) {
    let transition = MeerkatMachineMutator::apply(
        owner,
        MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: id.into(),
            input_kind: kind,
            requested_lane: Some(InputLane::Queue),
            continuation_kind: AdmissionContinuationKind::Ordinary,
            silent_intent_match: false,
            existing_superseded_input_id: None,
            runtime_running: true,
            active_turn_boundary_available: false,
            without_wake: false,
        },
    )
    .unwrap();
    if kind == AdmissionInputKind::LiveCallbackContinuation {
        assert!(matches!(
            transition.effects(),
            [super::dsl::MeerkatMachineEffect::AdmissionResolved {
                runtime_execution_kind: super::dsl::AdmissionRuntimeExecutionKind::ResumePending,
                record_transcript: false,
                ..
            }]
        ));
    }
    MeerkatMachineMutator::apply(
        owner,
        MeerkatMachineInput::QueueAccepted {
            input_id: id.into(),
        },
    )
    .unwrap();
}

#[test]
fn live_callback_queue_is_exclusive_resume_not_another_content_turn() {
    let mut owner = authority();
    queue_kind(
        &mut owner,
        "callback",
        AdmissionInputKind::LiveCallbackContinuation,
    );
    queue(&mut owner, "peer", false);
    assert_eq!(
        owner.state().input_runtime_execution_kind.get("callback"),
        Some(&RecoveredRuntimeExecutionKind::ResumePending)
    );
    assert_eq!(owner.state().input_is_prompt.get("callback"), Some(&false));
    let (_, selected, source) =
        AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(owner.state())
            .unwrap()
            .into_parts();
    assert_eq!(selected, ["callback"]);
    assert_eq!(source, RuntimeLoopBatchSource::Queue);
    assert!(
        AuthorizedStageForRun::authorize_stage_for_run_from_state(
            owner.state(),
            &["callback".into(), "peer".into()],
            &RunId("run".into()),
            RuntimeLoopBatchSource::Queue,
        )
        .is_none()
    );
}

#[test]
fn live_requests_and_callback_continuations_cannot_select_steer() {
    for kind in [
        AdmissionInputKind::LiveRequest,
        AdmissionInputKind::LiveCallbackContinuation,
    ] {
        let mut owner = authority();
        assert!(
            MeerkatMachineMutator::apply(
                &mut owner,
                MeerkatMachineInput::ResolveAdmissionPlan {
                    input_id: "live".into(),
                    input_kind: kind,
                    requested_lane: Some(InputLane::Steer),
                    continuation_kind: AdmissionContinuationKind::Ordinary,
                    silent_intent_match: false,
                    existing_superseded_input_id: None,
                    runtime_running: true,
                    active_turn_boundary_available: false,
                    without_wake: false,
                },
            )
            .is_err()
        );
        assert!(
            !owner
                .state()
                .admission_authorized_lanes
                .contains_key("live")
        );
    }
}

#[test]
fn live_callback_recovery_requires_exact_resume_queue_run_start() {
    for kind in [
        RecoveredInputKind::LiveRequest,
        RecoveredInputKind::LiveCallbackContinuation,
    ] {
        for execution in [
            RecoveredRuntimeExecutionKind::ContentTurn,
            RecoveredRuntimeExecutionKind::ResumePending,
        ] {
            for lane in [InputLane::Queue, InputLane::Steer] {
                for boundary in [
                    RecoveredRunApplyBoundary::RunStart,
                    RecoveredRunApplyBoundary::Immediate,
                ] {
                    let mut owner = authority();
                    let expected = lane == InputLane::Queue
                        && boundary == RecoveredRunApplyBoundary::RunStart
                        && ((kind == RecoveredInputKind::LiveRequest
                            && execution == RecoveredRuntimeExecutionKind::ContentTurn)
                            || (kind == RecoveredInputKind::LiveCallbackContinuation
                                && execution == RecoveredRuntimeExecutionKind::ResumePending));
                    let result = MeerkatMachineMutator::apply(
                        &mut owner,
                        MeerkatMachineInput::RecoverAdmittedInput {
                            input_id: "live".into(),
                            input_kind: kind,
                            runtime_boundary: boundary,
                            runtime_execution_kind: execution,
                            runtime_peer_response_terminal_apply_intent: None,
                            lane,
                        },
                    );
                    assert_eq!(
                        result.is_ok(),
                        expected,
                        "{kind:?} {execution:?} {lane:?} {boundary:?}"
                    );
                    assert_eq!(
                        owner.state().input_exclusive_live_requests.contains("live"),
                        expected
                    );
                }
            }
        }
    }
}

#[test]
fn live_queue_selection_preserves_fifo_without_co_batching() {
    for (inputs, expected) in [
        (vec![("peer", false), ("live", true)], vec!["peer"]),
        (
            vec![("peer", false), ("peer2", false), ("live", true)],
            vec!["peer", "peer2"],
        ),
        (vec![("live", true), ("peer", false)], vec!["live"]),
        (
            vec![("live", true), ("live2", true), ("peer", false)],
            vec!["live"],
        ),
    ] {
        let mut owner = authority();
        for (id, live) in inputs {
            queue(&mut owner, id, live);
        }
        let (_, selected, source) =
            AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(owner.state())
                .unwrap()
                .into_parts();
        assert_eq!(selected, expected);
        assert_eq!(source, RuntimeLoopBatchSource::Queue);
        assert!(owner.state().input_exclusive_live_requests.contains("live"));
        assert_eq!(owner.state().input_is_prompt.get("live"), Some(&false));
    }
}

#[test]
fn live_stage_plan_independently_refuses_mixed_duplicate_and_steer_batches() {
    let mut owner = authority();
    queue(&mut owner, "live", true);
    queue(&mut owner, "peer", false);
    queue(&mut owner, "live2", true);
    let run = RunId("run".into());
    for ids in [
        vec!["live", "peer"],
        vec!["peer", "live"],
        vec!["live", "live2"],
        vec!["live", "live"],
    ] {
        let ids = ids.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                owner.state(),
                &ids,
                &run,
                RuntimeLoopBatchSource::Queue,
            )
            .is_none()
        );
    }
    assert!(
        AuthorizedStageForRun::authorize_stage_for_run_from_state(
            owner.state(),
            &["live".into()],
            &run,
            RuntimeLoopBatchSource::Queue,
        )
        .is_some()
    );
    assert!(
        AuthorizedStageForRun::authorize_stage_for_run_from_state(
            owner.state(),
            &["live".into()],
            &run,
            RuntimeLoopBatchSource::Steer,
        )
        .is_none()
    );
}

#[test]
fn live_stage_transition_refuses_sequential_mixing_in_either_order() {
    for (first, second) in [("live", "peer"), ("peer", "live"), ("live", "live2")] {
        let mut owner = authority();
        queue(&mut owner, "live", true);
        queue(&mut owner, "peer", false);
        queue(&mut owner, "live2", true);
        let stage = |id: &str| MeerkatMachineInput::StageForRun {
            input_id: id.into(),
            run_id: RunId("run".into()),
        };
        MeerkatMachineMutator::apply(&mut owner, stage(first)).unwrap();
        assert!(
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                owner.state(),
                &[second.into()],
                &RunId("run".into()),
                RuntimeLoopBatchSource::Queue,
            )
            .is_none()
        );
        assert!(MeerkatMachineMutator::apply(&mut owner, stage(second)).is_err());
        assert_eq!(
            owner.state().input_phases.get(second),
            Some(&InputPhase::Queued)
        );
        assert_eq!(owner.state().input_run_associations.len(), 1);
    }
}

#[test]
fn live_stage_isolation_does_not_reintroduce_prior_run_attribution_wedge() {
    let mut owner = authority();
    queue(&mut owner, "live", true);
    queue(&mut owner, "peer", false);
    MeerkatMachineMutator::apply(
        &mut owner,
        MeerkatMachineInput::StageForRun {
            input_id: "peer".into(),
            run_id: RunId("run".into()),
        },
    )
    .unwrap();
    let mut restarted = owner.state().clone();
    restarted.current_run_id = Some(RunId("next-run".into()));
    let mut owner = MeerkatMachineAuthority::recover_from_state(restarted).unwrap();
    assert!(
        AuthorizedStageForRun::authorize_stage_for_run_from_state(
            owner.state(),
            &["live".into()],
            &RunId("next-run".into()),
            RuntimeLoopBatchSource::Queue,
        )
        .is_some()
    );
    MeerkatMachineMutator::apply(
        &mut owner,
        MeerkatMachineInput::StageForRun {
            input_id: "live".into(),
            run_id: RunId("next-run".into()),
        },
    )
    .unwrap();
}

#[test]
fn live_recovery_requires_queue_run_start_and_restores_isolation() {
    for (lane, boundary, accepted) in [
        (InputLane::Queue, RecoveredRunApplyBoundary::RunStart, true),
        (InputLane::Steer, RecoveredRunApplyBoundary::RunStart, false),
        (
            InputLane::Steer,
            RecoveredRunApplyBoundary::Immediate,
            false,
        ),
    ] {
        let mut owner = authority();
        let result = MeerkatMachineMutator::apply(
            &mut owner,
            MeerkatMachineInput::RecoverAdmittedInput {
                input_id: "live".into(),
                input_kind: RecoveredInputKind::LiveRequest,
                runtime_boundary: boundary,
                runtime_execution_kind: RecoveredRuntimeExecutionKind::ContentTurn,
                runtime_peer_response_terminal_apply_intent: None,
                lane,
            },
        );
        assert_eq!(result.is_ok(), accepted);
        assert_eq!(
            owner.state().input_exclusive_live_requests.contains("live"),
            accepted
        );
    }
}

#[test]
fn live_rollback_cannot_replay_and_mixed_run_recovery_fails_closed() {
    let mut owner = authority();
    queue(&mut owner, "live", true);
    queue(&mut owner, "peer", false);
    MeerkatMachineMutator::apply(
        &mut owner,
        MeerkatMachineInput::StageForRun {
            input_id: "live".into(),
            run_id: RunId("run".into()),
        },
    )
    .unwrap();
    assert!(
        MeerkatMachineMutator::apply(
            &mut owner,
            MeerkatMachineInput::RollbackStaged {
                input_id: "live".into(),
                lane: InputLane::Steer,
            },
        )
        .is_err()
    );
    assert_eq!(
        owner.state().input_phases.get("live"),
        Some(&InputPhase::Staged)
    );
    let mut corrupted = owner.state().clone();
    corrupted
        .input_run_associations
        .insert("peer".into(), RunId("run".into()));
    assert!(MeerkatMachineAuthority::recover_from_state(corrupted).is_err());
    for command in [
        MeerkatMachineInput::RollbackStaged {
            input_id: "live".into(),
            lane: InputLane::Queue,
        },
        MeerkatMachineInput::ResolveStagedRollback {
            input_id: "live".into(),
            lane: InputLane::Queue,
        },
    ] {
        assert!(MeerkatMachineMutator::apply(&mut owner, command).is_err());
    }
    assert_eq!(
        owner.state().input_phases.get("live"),
        Some(&InputPhase::Staged)
    );
    assert_eq!(
        owner.state().input_run_associations.get("live"),
        Some(&RunId("run".into()))
    );
    assert!(owner.state().input_exclusive_live_requests.contains("live"));
}

#[test]
fn failed_run_recovery_classifies_exact_scoped_contributors_without_replay() {
    use super::dsl::{FailedRunRecoveryDisposition, MeerkatMachineEffect};
    for kind in [
        AdmissionInputKind::PeerMessage,
        AdmissionInputKind::LiveRequest,
        AdmissionInputKind::LiveCallbackContinuation,
    ] {
        let mut owner = authority();
        queue_kind(&mut owner, "input", kind);
        MeerkatMachineMutator::apply(
            &mut owner,
            MeerkatMachineInput::StageForRun {
                input_id: "input".into(),
                run_id: RunId("run".into()),
            },
        )
        .unwrap();
        let input_ids = std::collections::BTreeSet::from(["input".to_string()]);
        for (run_id, inputs) in [
            (RunId("foreign".into()), input_ids.clone()),
            (RunId("run".into()), Default::default()),
            (
                RunId("run".into()),
                std::collections::BTreeSet::from(["foreign".into()]),
            ),
        ] {
            assert!(
                MeerkatMachineMutator::apply(
                    &mut owner,
                    MeerkatMachineInput::ResolveFailedRunRecovery {
                        run_id,
                        input_ids: inputs
                    },
                )
                .is_err()
            );
        }
        let transition = MeerkatMachineMutator::apply(
            &mut owner,
            MeerkatMachineInput::ResolveFailedRunRecovery {
                run_id: RunId("run".into()),
                input_ids: input_ids.clone(),
            },
        )
        .unwrap();
        let expected = if kind == AdmissionInputKind::PeerMessage {
            FailedRunRecoveryDisposition::Ordinary
        } else {
            FailedRunRecoveryDisposition::HoldScoped
        };
        assert!(matches!(transition.effects(),
            [MeerkatMachineEffect::FailedRunRecoveryResolved {
                run_id, input_ids: observed, disposition,
            }] if run_id == &RunId("run".into()) && observed == &input_ids
                && *disposition == expected
        ));
        assert_eq!(
            owner.state().input_phases.get("input"),
            Some(&InputPhase::Staged)
        );
        assert_eq!(
            owner.state().input_run_associations.get("input"),
            Some(&RunId("run".into()))
        );
        assert!(!owner.state().input_lane.contains_key("input"));
        assert!(!owner.state().input_terminal_kind.contains_key("input"));
    }
}
