//! Contract ratchets for the experimental channel-scoped GPT Live authority.
//!
//! These tests pin the generated semantic owner, not a provider codec. All
//! private provider identifiers remain opaque strings at the machine seam.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::{dsl_meerkat_machine, dsl_session_document_machine};
use meerkat_machine_schema::meerkat_mob_seam_composition;

fn variant_names(variants: &[meerkat_machine_schema::VariantSchema]) -> Vec<&str> {
    variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect()
}

#[test]
fn meerkat_machine_owns_live_fence_delegation_and_delivery_authority() {
    let machine = dsl_meerkat_machine();
    let inputs = variant_names(&machine.inputs.variants);
    let effects = variant_names(&machine.effects.variants);

    for required in [
        "BindLiveExecutionChannel",
        "ObserveLiveAssistantTurnStarted",
        "AdmitLiveInteraction",
        "AdmitLiveDelegation",
        "AdmitLiveInteractionDelegation",
        "ReconcileLiveDelegationTranscript",
        "AuthorizeLiveDelegationWorkerStart",
        "ResolveLiveDelegationWorkerStart",
        "AuthorizeLiveDelegationTranscriptTerminalCancellation",
        "SupersedeLiveInteraction",
        "ResolveLiveDelegationCancellation",
        "RecordLiveDelegationWorkerTerminal",
        "AuthorizeLiveDelegationWorkerRetirement",
        "ResolveLiveDelegationWorkerRetirement",
        "AbandonLiveInteraction",
        "CompleteLiveInteraction",
        "AuthorizeLiveConsequentialEffect",
        "AuthorizeLiveDelegationResultRelease",
        "AuthorizeLiveDelegationResultDelivery",
        "ResolveLiveDelegationResultDelivery",
        "BindLiveDelegationResultRecoveryChannel",
        "AuthorizeLiveContextAppend",
        "EnqueueLiveContextRow",
        "AdvanceLiveContextCanonicalCoverage",
        "ResolveLiveContextAppend",
        "BindLiveContextRecoveryChannel",
        "RecordLiveWebrtcAnswerAcceptedAndBindExecution",
    ] {
        assert!(
            inputs.contains(&required),
            "missing live authority input {required}"
        );
    }

    let reconciliation = machine
        .inputs
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "ReconcileLiveDelegationTranscript")
        .expect("missing live reconciliation input");
    let reconciliation_fields = reconciliation
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        reconciliation_fields.contains(&"final_transcript_committed")
            && reconciliation_fields.contains(&"normalized_digest_matches"),
        "live reconciliation must enter as canonical evidence facts"
    );
    assert!(
        !reconciliation_fields.contains(&"reconciliation"),
        "callers must not select the machine-owned reconciliation class"
    );
    let result_release = machine
        .inputs
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "AuthorizeLiveDelegationResultRelease")
        .expect("missing result-release input");
    assert!(
        !result_release
            .fields
            .iter()
            .any(|field| field.name.as_str() == "provider_turn_open"),
        "callers must not select the machine-owned result-release disposition"
    );
    let result_delivery = machine
        .inputs
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "AuthorizeLiveDelegationResultDelivery")
        .expect("missing result-delivery input");
    let result_delivery_fields = result_delivery
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    assert!(result_delivery_fields.contains(&"result_digest"));
    assert!(!result_delivery_fields.contains(&"previous_cursor"));
    assert!(!result_delivery_fields.contains(&"next_cursor"));

    for required in [
        "LiveExecutionChannelBound",
        "LiveAssistantTurnStarted",
        "LiveInteractionAdmitted",
        "LiveDelegationAdmitted",
        "LiveInteractionDelegationAdmitted",
        "LiveDelegationTranscriptReconciled",
        "LiveDelegationWorkerStartAuthorized",
        "LiveDelegationWorkerStartResolved",
        "LiveDelegationCancellationAuthorized",
        "LiveDelegationCancellationResolved",
        "LiveDelegationWorkerTerminalRecorded",
        "LiveDelegationWorkerRetirementAuthorized",
        "LiveDelegationWorkerRetirementResolved",
        "LiveInteractionSupersededWithoutCancellation",
        "LiveInteractionAbandoned",
        "LiveInteractionCompleted",
        "LiveConsequentialEffectAuthorized",
        "LiveDelegationResultReleaseAuthorized",
        "LiveDelegationResultDeliveryAuthorized",
        "LiveDelegationResultDeliveryResolved",
        "LiveDelegationResultAmbiguityRecoveryAuthorized",
        "LiveDelegationResultRecoveryChannelBound",
        "LiveContextAppendAuthorized",
        "LiveContextRowQueued",
        "LiveContextCanonicalCoverageAdvanced",
        "LiveContextAppendResolved",
        "LiveContextAmbiguityRecoveryAuthorized",
        "LiveContextRecoveryChannelBound",
        "LiveWebrtcAnswerAcceptedAndExecutionBound",
    ] {
        assert!(
            effects.contains(&required),
            "missing live authority effect {required}"
        );
    }

    let invariants = machine
        .invariants
        .iter()
        .map(|invariant| invariant.name.as_str())
        .collect::<Vec<_>>();
    for required in [
        "live_execution_binding_is_complete_and_channel_scoped",
        "live_active_interaction_is_exactly_channel_bound",
        "live_assistant_turn_is_frozen_to_exact_foreground_interaction",
        "live_pending_delegation_is_serialized_and_complete",
        "live_delegation_operation_has_exact_join_identity",
        "live_delegation_worker_binding_is_exact",
        "live_delegation_terminal_is_worker_bound",
        "live_delegation_result_eligibility_is_terminal_and_confirmed",
        "live_delegation_late_terminal_never_eligible",
        "live_released_result_requires_confirmed_transcript",
        "live_result_delivery_is_exact_and_terminal_once",
        "live_result_recovery_is_exact_and_channel_scoped",
        "live_consequential_authority_requires_confirmed_transcript",
        "live_pending_context_append_is_exact_and_channel_scoped",
        "live_context_outbox_is_exact_and_session_scoped",
        "live_context_recovery_is_exact_and_channel_scoped",
    ] {
        assert!(
            invariants.contains(&required),
            "missing live invariant {required}"
        );
    }
}

#[test]
fn session_document_owns_live_transcript_reconciliation_and_playback_terminal() {
    let machine = dsl_session_document_machine();
    let inputs = variant_names(&machine.inputs.variants);
    let effects = variant_names(&machine.effects.variants);

    for required in [
        "AdmitLiveInteractionTranscript",
        "StageLiveProvisionalUserTranscript",
        "ReconcileLiveFinalUserTranscript",
        "CompleteLiveInteractionTranscript",
        "AdmitLiveAssistantPlaybackTarget",
        "RecoverLiveAssistantPlaybackTarget",
        "ResolveLiveAssistantPlaybackOnChannelClose",
        "ResolveLiveAssistantPlaybackTerminal",
        "ClassifyLiveContextCommittedRow",
    ] {
        assert!(
            inputs.contains(&required),
            "missing transcript input {required}"
        );
    }

    for required in [
        "LiveInteractionTranscriptAdmitted",
        "LiveProvisionalUserTranscriptStaged",
        "LiveFinalUserTranscriptReconciled",
        "LiveInteractionTranscriptCompleted",
        "LiveAssistantPlaybackTargetAdmitted",
        "LiveAssistantPlaybackTargetRecovered",
        "LiveAssistantPlaybackTerminalResolved",
        "LiveContextCommittedRowClassified",
    ] {
        assert!(
            effects.contains(&required),
            "missing transcript effect {required}"
        );
    }

    let terminal_arms = machine
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("ResolveLiveAssistantPlayback")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminal_arms.len(),
        4,
        "playback terminal evidence must include exact channel-close abandonment"
    );
    assert!(terminal_arms.iter().all(|transition| {
        format!("{:?}", transition.emit).contains("biological_hearing_claimed")
    }));

    let invariants = machine
        .invariants
        .iter()
        .map(|invariant| invariant.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        invariants.contains(&"live_assistant_playback_target_is_complete_and_interaction_bound"),
        "playback target must remain exact and interaction-bound until terminal"
    );
}

#[test]
fn live_delegation_enters_through_existing_meerkat_mob_seam() {
    let seam = meerkat_mob_seam_composition();
    let entries = seam
        .entry_inputs
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();

    for required in [
        "bind_live_execution_channel",
        "stage_experimental_live_execution",
        "observe_live_provider_turn_started",
        "observe_live_assistant_turn_started",
        "admit_live_interaction",
        "admit_live_delegation",
        "admit_live_interaction_delegation",
        "reconcile_live_delegation_transcript",
        "authorize_live_delegation_worker_start",
        "resolve_live_delegation_worker_start",
        "authorize_live_delegation_transcript_cancellation",
        "supersede_live_interaction",
        "resolve_live_delegation_cancellation",
        "record_live_delegation_worker_terminal",
        "authorize_live_delegation_worker_retirement",
        "resolve_live_delegation_worker_retirement",
        "authorize_live_consequential_effect",
        "authorize_live_delegation_result_release",
        "authorize_live_delegation_result_delivery",
        "resolve_live_delegation_result_delivery",
        "bind_live_delegation_result_recovery_channel",
        "authorize_live_context_append",
        "enqueue_live_context_row",
        "advance_live_context_canonical_coverage",
        "resolve_live_context_append",
        "bind_live_context_recovery_channel",
    ] {
        assert!(
            entries.contains(&required),
            "missing mob seam entry {required}"
        );
    }
}
