#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::TransitionSchema;
use meerkat_machine_schema::catalog::dsl::dsl_mob_machine;

fn has_guard(transition: &TransitionSchema, name: &str) -> bool {
    transition.guards.iter().any(|guard| guard.name == name)
}

fn has_effect(transition: &TransitionSchema, variant: &str) -> bool {
    transition
        .emit
        .iter()
        .any(|effect| effect.variant.as_str() == variant)
}

#[test]
fn fresh_work_origins_are_fenced_by_the_typed_lifecycle_intent() {
    let schema = dsl_mob_machine();

    for name in [
        "AuthorizeSpawnProfileRunning",
        "RecordAdaptivePlanningDecisionActiveRunning",
        "RecordAdaptivePlanRejectedActiveRunning",
        "ResolveAdaptiveLayerAdmissionRejectedRunning",
        "EnsureMemberRunningExisting",
        "EnsureMemberRunningMissing",
        "BeginHostBindRequested",
        "BeginHostBindReplacementRequested",
        "InitializeOrchestratorRunning",
        "BindCoordinatorRunning",
        "BindObjectiveOwnerRunning",
        "ClassifyMemberLiveMaterializationRevivable",
        "RecordCoordinationWorkIntent",
        "RecordCoordinationResourceClaim",
        "UpdateCoordinationWorkIntentPlanned",
        "UpdateCoordinationWorkIntentActive",
        "UpdateCoordinationWorkIntentBlocked",
        "UpdateCoordinationResourceClaimActive",
        "AuthorizeFlowRunReducerCommandFailStepEscalating",
        "AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalating",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(
            has_guard(transition, "lifecycle_origin_open"),
            "{name} can originate work and must reject while lifecycle cleanup owns the mob"
        );
    }

    let escalation_arms = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("EscalateToSupervisorTargetFound")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        escalation_arms.len(),
        4,
        "per-phase escalation arms changed"
    );
    assert!(
        escalation_arms
            .iter()
            .all(|transition| has_guard(transition, "lifecycle_origin_open")),
        "every supervisor-turn origin must be lifecycle-fenced"
    );
}

#[test]
fn terminal_failure_folding_survives_quiesce_without_escalating() {
    let schema = dsl_mob_machine();
    for name in [
        "AuthorizeFlowRunReducerCommandFailStepEscalationSuppressedByLifecycle",
        "AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalationSuppressedByLifecycle",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(has_guard(transition, "lifecycle_origin_closed"));
        assert!(has_effect(transition, "AppendFailureLedger"));
        assert!(
            !has_effect(transition, "EscalateSupervisor"),
            "{name} must fold a terminal failure without opening a supervisor turn"
        );
    }
}

#[test]
fn recovery_watermarks_preserve_the_recovered_phase() {
    let schema = dsl_mob_machine();
    for prefix in [
        "RecoverPlacedCompletionDispatchSequenceAdvance",
        "RecoverPlacedCompletionDispatchSequenceReplay",
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.name.as_str().starts_with(prefix))
            .collect::<Vec<_>>();
        assert_eq!(
            transitions.len(),
            4,
            "{prefix} must be total over Running/Stopped/Completed/Destroyed"
        );
        for transition in transitions {
            assert_eq!(transition.from.len(), 1);
            assert_eq!(
                transition.to, transition.from[0],
                "{} must never revive or demote a recovered phase",
                transition.name
            );
        }
    }
}

#[test]
fn retire_all_restores_the_stopped_or_completed_baseline() {
    let schema = dsl_mob_machine();
    for (name, phase, baseline) in [
        (
            "EndPlacedCompletionLifecycleQuiesceStoppedRetireAll",
            "Stopped",
            "Stop",
        ),
        (
            "EndPlacedCompletionLifecycleQuiesceCompletedRetireAll",
            "Completed",
            "Complete",
        ),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert_eq!(transition.to.as_str(), phase);
        let persistence = transition
            .emit
            .iter()
            .find(|effect| effect.variant.as_str() == "PersistPlacedCompletionLifecycleIntent")
            .unwrap_or_else(|| panic!("{name} must persist the restored baseline"));
        let rendered = format!("{:?}", persistence.fields);
        assert!(rendered.contains(baseline), "{name}: {rendered}");
        assert!(rendered.contains("true"), "{name}: {rendered}");
    }

    let resume = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "ResumeStopped")
        .expect("MobMachine must declare ResumeStopped");
    assert!(has_guard(resume, "placed_completion_stop_intent"));
    assert!(has_effect(resume, "PersistPlacedCompletionLifecycleIntent"));
    assert!(has_effect(resume, "AppendLifecycleJournal"));
}

#[test]
fn lifecycle_intent_invariant_excludes_adaptive_custody() {
    let schema = dsl_mob_machine();
    let invariant = schema
        .invariants
        .iter()
        .find(|invariant| {
            invariant.name == "placed_completion_lifecycle_intent_owns_no_adaptive_custody"
        })
        .expect("lifecycle intent must own no adaptive custody");
    assert!(format!("{:?}", invariant.expr).contains("mob_machine_adaptive_lifecycle_drained"));
}

#[test]
fn shutdown_cannot_orphan_adaptive_child_custody() {
    let schema = dsl_mob_machine();
    for name in ["ShutdownRunning", "ShutdownStopped", "ShutdownCompleted"] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(
            has_guard(transition, "adaptive_lifecycle_drained"),
            "{name} must not shut down while detached adaptive child custody survives"
        );
        assert!(
            format!("{:?}", transition.guards).contains("mob_machine_adaptive_lifecycle_drained"),
            "{name} must use the machine-owned adaptive drain predicate"
        );
    }
}

#[test]
fn evidence_missing_terminalizes_only_after_exact_adaptive_custody_drains() {
    let schema = dsl_mob_machine();
    let transition = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "RecordAdaptiveBodyEvidenceMissingRunning")
        .expect("MobMachine must declare RecordAdaptiveBodyEvidenceMissingRunning");
    assert!(
        has_guard(transition, "adaptive_run_custody_drained"),
        "EvidenceMissing must not discard an owned adaptive child"
    );
    let debug = format!("{transition:?}");
    assert!(debug.contains("mob_machine_adaptive_run_custody_drained"));
    assert!(debug.contains("adaptive_active_run"));
    assert!(
        debug.contains("None"),
        "EvidenceMissing must release the single active-run slot after custody drains"
    );
}

#[test]
fn adaptive_cleanup_disposition_is_owner_exact_and_idempotent() {
    let schema = dsl_mob_machine();
    for name in [
        "RecordAdaptiveLayerMobDestroyedRunning",
        "RecordAdaptiveLayerMobRetainedRunning",
        "RecordAdaptiveLayerMobRetainedCleanupRequiredRunning",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(has_guard(transition, "layer_owner_matches"));
        assert!(has_guard(transition, "disposition_absent"));
        if name.contains("Retained") {
            assert!(has_guard(transition, "retained_disposition"));
        }
    }
    for name in [
        "RecordAdaptiveLayerMobDestroyedReplayRunning",
        "RecordAdaptiveLayerMobRetainedReplayRunning",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(has_guard(transition, "layer_owner_matches"));
        assert!(has_guard(transition, "exact_disposition_replay"));
        if name.contains("Retained") {
            assert!(has_guard(transition, "retained_disposition"));
        }
        assert!(transition.updates.is_empty(), "{name} must not debit twice");
    }
}

#[test]
fn a_new_adaptive_run_waits_for_prior_child_custody() {
    let schema = dsl_mob_machine();
    let transition = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "InitializeAdaptiveRunRunningRunning")
        .expect("MobMachine must declare InitializeAdaptiveRunRunningRunning");
    assert!(has_guard(transition, "prior_adaptive_custody_drained"));
    assert!(has_guard(transition, "adaptive_run_id_unused"));
    assert!(format!("{:?}", transition.guards).contains("mob_machine_adaptive_lifecycle_drained"));
}

#[test]
fn adaptive_initialization_reply_loss_has_one_exact_replay() {
    let schema = dsl_mob_machine();
    let transition = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "InitializeAdaptiveRunReplayRunning")
        .expect("MobMachine must declare InitializeAdaptiveRunReplayRunning");
    assert!(has_guard(transition, "same_active_run"));
    assert!(has_guard(transition, "run_still_unstarted"));
    assert!(has_guard(transition, "exact_initialization_replay"));
    assert!(transition.updates.is_empty());
    assert!(has_effect(transition, "AdaptiveRunInitialized"));
}

#[test]
fn adaptive_layer_progress_is_bound_to_its_exact_run_owner() {
    let schema = dsl_mob_machine();
    for name in [
        "RecordAdaptiveLayerProvisionedRunning",
        "RecordAdaptiveLayerRunStartedRunning",
        "IngestAdaptiveLayerTerminalSuccessRunning",
        "IngestAdaptiveLayerTerminalFailureRunning",
        "RecordAdaptiveLayerSetupFaultRunning",
        "RecordAdaptiveLayerResultValidatedRunning",
        "RecordAdaptiveLayerResultInvalidRunning",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(has_guard(transition, "layer_owner_matches"), "{name}");
        assert!(has_guard(transition, "active_layer_matches"), "{name}");
    }
}

#[test]
fn adaptive_terminal_inputs_cannot_abandon_child_custody() {
    let schema = dsl_mob_machine();
    for name in [
        "RecordAdaptivePlanningDecisionPlanLimitRunning",
        "RecordAdaptivePlanRejectedRepairLimitRunning",
        "ResolveAdaptiveFinishRunningRunning",
        "RecordAdaptiveBodyEvidenceMissingRunning",
        "RecordAdaptiveDeadlineObservedExpiredRunning",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("MobMachine must declare {name}"));
        assert!(
            has_guard(transition, "adaptive_run_custody_drained"),
            "{name}"
        );
    }
}
