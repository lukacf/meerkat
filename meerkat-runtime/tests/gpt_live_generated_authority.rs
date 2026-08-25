//! Behavioral tests for the catalog-owned GPT Live machine region.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_runtime::meerkat_machine::dsl as mm;

const SESSION: &str = "session-live-authority";
const CHANNEL: &str = "channel-live-authority";
const INTERACTION: &str = "interaction-live-authority";
const PROVIDER_TURN: &str = "opaque-provider-turn";
const WORKER: &str = "live-worker-authority";

fn runtime_id() -> mm::AgentRuntimeId {
    mm::AgentRuntimeId("runtime-live-authority".to_string())
}

fn fence() -> mm::FenceToken {
    mm::FenceToken(41)
}

fn generation() -> mm::Generation {
    mm::Generation(7)
}

fn operation_id() -> mm::OperationId {
    mm::OperationId("operation-live-authority".to_string())
}

fn identity() -> mm::SessionLlmIdentity {
    mm::SessionLlmIdentity {
        model: "experimental-live".to_string(),
        provider: mm::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params_repr: None,
        auth_binding: None,
    }
}

fn opened_authority() -> mm::MeerkatMachineAuthority {
    let mut state = mm::MeerkatMachineState {
        lifecycle_phase: mm::MeerkatPhase::Idle,
        session_id: Some(mm::SessionId(SESSION.to_string())),
        active_runtime_id: Some(runtime_id()),
        active_fence_token: Some(fence()),
        active_runtime_generation: Some(generation()),
        ..Default::default()
    };
    state
        .live_active_channel_by_session
        .insert(SESSION.to_string(), CHANNEL.to_string());
    state
        .live_channel_session_by_channel
        .insert(CHANNEL.to_string(), SESSION.to_string());
    state
        .live_channel_identity_by_channel
        .insert(CHANNEL.to_string(), identity());
    mm::MeerkatMachineAuthority::recover_from_state(state)
        .expect("seed state satisfies generated invariants")
}

fn apply(
    authority: &mut mm::MeerkatMachineAuthority,
    input: mm::MeerkatMachineInput,
) -> Result<mm::MeerkatMachineTransition, mm::MeerkatMachineTransitionError> {
    mm::MeerkatMachineMutator::apply(authority, input)
}

fn bind_only(authority: &mut mm::MeerkatMachineAuthority) {
    apply(
        authority,
        mm::MeerkatMachineInput::BindLiveExecutionChannel {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
        },
    )
    .expect("exact runtime binding is admitted");
}

fn stage_experimental(authority: &mut mm::MeerkatMachineAuthority, canonical_seed_cursor: u64) {
    apply(
        authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor,
        },
    )
    .expect("strict experimental execution is staged before provider answer");
}

fn bind_experimental(authority: &mut mm::MeerkatMachineAuthority, canonical_seed_cursor: u64) {
    stage_experimental(authority, canonical_seed_cursor);
    apply(
        authority,
        mm::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            answer_observation_sequence: 1,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor,
        },
    )
    .expect("provider answer and acknowledged seed bind experimental execution atomically");
}

fn bind_and_admit(authority: &mut mm::MeerkatMachineAuthority) {
    bind_only(authority);
    admit_provider_turn_delegation(authority);
}

#[test]
fn assistant_turn_freezes_completed_user_interaction_before_next_user_turn() {
    let mut authority = opened_authority();
    bind_only(&mut authority);
    let user_one = "11111111-1111-4111-8111-111111111111";
    let user_two = "22222222-2222-4222-8222-222222222222";
    let user_turn_one = "provider-user-turn-one";
    let assistant_turn_one = "provider-assistant-turn-one";

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: user_one.to_string(),
            provider_turn_ref: user_turn_one.to_string(),
        },
    )
    .expect("first foreground user interaction starts");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                assistant_turn_ref: assistant_turn_one.to_string(),
            },
        )
        .is_err(),
        "fork-inverted assistant start cannot bypass the user-finish authority boundary"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: user_turn_one.to_string(),
        },
    )
    .expect("exact user finish opens one awaiting-assistant slot");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            assistant_turn_ref: assistant_turn_one.to_string(),
        },
    )
    .expect("assistant start consumes the exact awaiting response slot");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                assistant_turn_ref: "unsolicited-assistant-turn".to_string(),
            },
        )
        .is_err(),
        "a prior user interaction does not authorize unsolicited later assistant output"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: user_two.to_string(),
            provider_turn_ref: "provider-user-turn-two".to_string(),
        },
    )
    .expect("second user interaction starts after assistant correlation freezes");

    assert_eq!(
        authority
            .state()
            .live_assistant_interaction_by_turn
            .get(assistant_turn_one)
            .map(String::as_str),
        Some(user_one),
        "User2 cannot overwrite Assistant1 playback attribution"
    );
    assert_eq!(
        authority
            .state()
            .live_active_interaction_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some(user_two),
        "the next foreground turn remains independently active"
    );
}

fn admit_provider_turn_delegation(authority: &mut mm::MeerkatMachineAuthority) {
    apply(
        authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("typed provider turn is joined to one interaction before delegation");
    apply(
        authority,
        mm::MeerkatMachineInput::AdmitLiveDelegation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            delegation_identity_present: true,
            actionable_input_present: true,
            exact_join: true,
        },
    )
    .expect("exact delegation and actionable input join is admitted");
}

fn bind_and_legacy_atomic_admit(authority: &mut mm::MeerkatMachineAuthority) {
    bind_only(authority);
    apply(
        authority,
        mm::MeerkatMachineInput::AdmitLiveInteractionDelegation {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            delegation_identity_present: true,
            actionable_input_present: true,
            exact_join: true,
        },
    )
    .expect("legacy atomic interaction and delegation fixture is admitted");
}

fn enqueue_mirror_row(
    authority: &mut mm::MeerkatMachineAuthority,
    append_id: &str,
    canonical_cursor: u64,
) {
    apply(
        authority,
        mm::MeerkatMachineInput::EnqueueLiveContextRow {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: append_id.to_string(),
            canonical_cursor,
            content_digest: format!("digest-{append_id}"),
            commit_authority_token: format!("commit-{append_id}"),
            disposition: mm::LiveContextRowDisposition::MirrorParentText,
        },
    )
    .expect("canonical committed row enters generated outbox");
}

#[test]
fn atomic_webrtc_answer_binding_preserves_running_phase() {
    let mut authority = opened_authority();
    let mut running = authority.state().clone();
    running.lifecycle_phase = mm::MeerkatPhase::Running;
    running.current_run_id = Some(mm::RunId("run-live-authority".to_string()));
    running.pre_run_phase = Some(mm::PreRunPhase::Idle);
    authority = mm::MeerkatMachineAuthority::recover_from_state(running)
        .expect("running live-open state satisfies generated invariants");

    stage_experimental(&mut authority, 4);

    let transition = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            answer_observation_sequence: 1,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 4,
        },
    )
    .expect("answered WebRTC transport and acknowledged seed bind atomically");

    assert_eq!(authority.state().lifecycle_phase, mm::MeerkatPhase::Running);
    assert!(transition.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveWebrtcAnswerAcceptedAndExecutionBound {
            canonical_seed_cursor: 4,
            answered: true,
            ..
        }
    )));
}

#[test]
fn pre_bind_committed_row_remains_queued_until_acknowledged_seed_installs_cursor() {
    let mut authority = opened_authority();
    stage_experimental(&mut authority, 3);
    enqueue_mirror_row(&mut authority, "context-before-answer", 4);

    assert_eq!(
        authority
            .state()
            .live_context_queued_append_by_cursor
            .get(&4)
            .map(String::as_str),
        Some("context-before-answer")
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            answer_observation_sequence: 1,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 3,
        },
    )
    .expect("acknowledged seed K binds after pre-bind K+1 entered generated custody");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-before-answer".to_string(),
            previous_cursor: 3,
            next_cursor: 4,
        },
    )
    .expect("pre-bind K+1 is delivered only after acknowledged K becomes authoritative");
}

#[test]
fn provisional_delegation_cannot_release_effect_or_result() {
    let mut authority = opened_authority();
    bind_and_admit(&mut authority);

    let effect = mm::MeerkatMachineInput::AuthorizeLiveConsequentialEffect {
        channel_id: CHANNEL.to_string(),
        runtime_id: runtime_id(),
        fence_token: fence(),
        generation: generation(),
        interaction_id: INTERACTION.to_string(),
        operation_id: operation_id(),
        authority_id: "effect-authority".to_string(),
    };
    assert!(apply(&mut authority, effect).is_err());

    let result = mm::MeerkatMachineInput::AuthorizeLiveDelegationResultRelease {
        channel_id: CHANNEL.to_string(),
        runtime_id: runtime_id(),
        fence_token: fence(),
        generation: generation(),
        interaction_id: INTERACTION.to_string(),
        operation_id: operation_id(),
        provider_turn_correlation: PROVIDER_TURN.to_string(),
    };
    assert!(apply(&mut authority, result).is_err());
}

#[test]
fn live_reconciliation_is_derived_from_canonical_evidence_facts() {
    let mut impossible = opened_authority();
    bind_and_admit(&mut impossible);
    assert!(
        apply(
            &mut impossible,
            mm::MeerkatMachineInput::ReconcileLiveDelegationTranscript {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: operation_id(),
                provider_turn_correlation: PROVIDER_TURN.to_string(),
                final_transcript_committed: false,
                normalized_digest_matches: true,
            },
        )
        .is_err(),
        "a missing canonical transcript cannot claim digest equality"
    );

    let mut conflict = opened_authority();
    bind_and_admit(&mut conflict);
    let transition = apply(
        &mut conflict,
        mm::MeerkatMachineInput::ReconcileLiveDelegationTranscript {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            final_transcript_committed: true,
            normalized_digest_matches: false,
        },
    )
    .expect("committed digest conflict is machine-classified");
    assert!(transition.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationTranscriptReconciled {
            reconciliation: mm::LiveDelegationReconciliation::MaterialConflict,
            ..
        }
    )));
}

#[test]
fn confirmed_delegation_mints_distinct_effect_and_deferred_result_authorities() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    admit_provider_turn_delegation(&mut authority);

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("exact worker start is authorized");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            started: true,
        },
    )
    .expect("worker start is recorded");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveDelegationWorkerTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            terminal: mm::LiveDelegationWorkerTerminalKind::Completed,
        },
    )
    .expect("completed worker terminal is recorded before transcript evidence");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("terminal worker retirement is authorized");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            retired: true,
        },
    )
    .expect("worker retirement clears only channel serialization state");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ReconcileLiveDelegationTranscript {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            final_transcript_committed: true,
            normalized_digest_matches: true,
        },
    )
    .expect("delayed final transcript confirms the retired operation");

    let effect_transition = apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveConsequentialEffect {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            authority_id: "effect-authority".to_string(),
        },
    )
    .expect("confirmed transcript unlocks consequential authority");
    assert!(effect_transition.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveConsequentialEffectAuthorized { authority_id, .. }
            if authority_id == "effect-authority"
    )));

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("provider turn can complete while the executor result remains pending");

    let release = apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationResultRelease {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
        },
    )
    .expect("late confirmed result is admitted as deferred context");
    assert!(release.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
            disposition: mm::LiveDelegationResultDisposition::DeferredContext,
            ..
        }
    )));

    let delivery = apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationResultDelivery {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            result_digest: "bounded-result-digest".to_string(),
            disposition: mm::LiveDelegationResultDisposition::DeferredContext,
        },
    )
    .expect("released result receives distinct digest-bound provider delivery authority");
    assert!(delivery.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultDeliveryAuthorized {
            result_digest,
            disposition: mm::LiveDelegationResultDisposition::DeferredContext,
            ..
        } if result_digest == "bounded-result-digest"
    )));

    let resolved = apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: operation_id(),
            result_digest: "bounded-result-digest".to_string(),
            replacement_channel_id: "channel-result-recovery".to_string(),
            observation: mm::LiveDelegationResultDeliveryObservation::Ambiguous,
        },
    )
    .expect("ambiguous provider result delivery terminalizes without replay");
    assert!(resolved.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultAmbiguityRecoveryAuthorized {
            replacement_channel_id,
            canonical_seed_cursor: 0,
            ..
        } if replacement_channel_id == "channel-result-recovery"
    )));
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveDelegationResultDelivery {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: operation_id(),
                provider_turn_correlation: PROVIDER_TURN.to_string(),
                result_digest: "bounded-result-digest".to_string(),
                disposition: mm::LiveDelegationResultDisposition::DeferredContext,
            },
        )
        .is_err(),
        "an ambiguous result delivery cannot be authorized again"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AbandonLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
        },
    )
    .expect("ambiguous result transport closes under generated recovery debt");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: "channel-result-recovery".to_string(),
            llm_identity: identity(),
        },
    )
    .expect("fresh result-recovery channel passes ordinary open admission");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: "channel-result-recovery".to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
        },
    )
    .expect("result-recovery replacement stages the exact carried seed");
    let rebound = apply(
        &mut authority,
        mm::MeerkatMachineInput::BindLiveDelegationResultRecoveryChannel {
            session_id: SESSION.to_string(),
            closing_channel_id: CHANNEL.to_string(),
            replacement_channel_id: "channel-result-recovery".to_string(),
            answer_observation_sequence: 12,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: operation_id(),
            result_digest: "bounded-result-digest".to_string(),
            canonical_seed_cursor: 0,
        },
    )
    .expect("provider answer and acknowledged seed bind result recovery atomically");
    assert!(rebound.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultRecoveryChannelBound {
            replacement_channel_id,
            answered: true,
            canonical_seed_cursor: 0,
            ..
        } if replacement_channel_id == "channel-result-recovery"
    )));
}

#[test]
fn terminal_worker_supersession_requires_no_cancellation_and_allows_fresh_atomic_admission() {
    let mut authority = opened_authority();
    bind_and_legacy_atomic_admit(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("worker start authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            started: true,
        },
    )
    .expect("worker running");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveDelegationWorkerTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            terminal: mm::LiveDelegationWorkerTerminalKind::Completed,
        },
    )
    .expect("terminal recorded");

    let next_interaction = "interaction-live-authority-next";
    let superseded = apply(
        &mut authority,
        mm::MeerkatMachineInput::SupersedeLiveInteraction {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            superseding_interaction_id: next_interaction.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("terminal worker supersession is totally classified");
    assert!(superseded.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveInteractionSupersededWithoutCancellation {
            superseding_interaction_id,
            ..
        } if superseding_interaction_id == next_interaction
    )));

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("terminal worker retirement authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            retired: true,
        },
    )
    .expect("old worker retired");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AdmitLiveInteractionDelegation {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: next_interaction.to_string(),
            operation_id: mm::OperationId("operation-live-authority-next".to_string()),
            provider_turn_correlation: "opaque-provider-turn-next".to_string(),
            delegation_identity_present: true,
            actionable_input_present: true,
            exact_join: true,
        },
    )
    .expect("fresh operation is atomically admitted only after old retirement");
}

#[test]
fn running_worker_supersession_authorizes_cancellation_and_suppresses_late_terminal_result() {
    let mut authority = opened_authority();
    bind_and_legacy_atomic_admit(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("worker start authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            started: true,
        },
    )
    .expect("worker running");

    let next_interaction = "interaction-live-authority-barge";
    let superseded = apply(
        &mut authority,
        mm::MeerkatMachineInput::SupersedeLiveInteraction {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            superseding_interaction_id: next_interaction.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("running worker supersession is machine-classified as cancellation");
    assert!(superseded.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationCancellationAuthorized {
            operation_id: authorized_operation,
            reason: mm::LiveDelegationCancellationReason::Superseded,
            superseding_interaction_id: Some(superseding_interaction_id),
            ..
        } if authorized_operation == &operation_id()
            && superseding_interaction_id == next_interaction
    )));

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationCancellation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            outcome: mm::LiveDelegationCancellationOutcome::Cancelled,
        },
    )
    .expect("authorized cancellation outcome is recorded");

    let terminal = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveDelegationWorkerTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            terminal: mm::LiveDelegationWorkerTerminalKind::Completed,
        },
    )
    .expect("late terminal is recorded without reopening result eligibility");
    assert!(terminal.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationWorkerTerminalRecorded {
            operation_id: terminal_operation,
            late: true,
            result_eligible: false,
            ..
        } if terminal_operation == &operation_id()
    )));

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("late terminal worker retirement is machine-authorized");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            retired: true,
        },
    )
    .expect("late terminal worker is retired under exact authority");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveDelegationResultRelease {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: operation_id(),
                provider_turn_correlation: PROVIDER_TURN.to_string(),
            },
        )
        .is_err(),
        "a cancelled operation's late completion never becomes releasable"
    );
}

#[test]
fn completed_turn_pending_worker_can_be_superseded_without_abandoning_new_active_turn() {
    let mut authority = opened_authority();
    bind_and_admit(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("worker start authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            started: true,
        },
    )
    .expect("old worker running");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("old provider turn completes while its worker remains pending");

    let next_interaction = "interaction-live-authority-next-active";
    let next_provider_turn = "opaque-provider-turn-next-active";
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: next_interaction.to_string(),
            provider_turn_ref: next_provider_turn.to_string(),
        },
    )
    .expect("next provider turn is independently admitted");

    let superseded = apply(
        &mut authority,
        mm::MeerkatMachineInput::SupersedeLiveInteraction {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            superseding_interaction_id: next_interaction.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("completed-turn pending worker receives exact supersession cancellation");
    assert!(superseded.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationCancellationAuthorized {
            interaction_id,
            operation_id: authorized_operation,
            reason: mm::LiveDelegationCancellationReason::Superseded,
            superseding_interaction_id: Some(superseding_interaction_id),
            ..
        } if interaction_id == INTERACTION
            && authorized_operation == &operation_id()
            && superseding_interaction_id == next_interaction
    )));
    assert_eq!(
        authority
            .state()
            .live_active_interaction_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some(next_interaction),
        "old worker cancellation must not abandon the new provider turn"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationCancellation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            outcome: mm::LiveDelegationCancellationOutcome::Cancelled,
        },
    )
    .expect("old worker cancellation resolves");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveDelegationWorkerTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            terminal: mm::LiveDelegationWorkerTerminalKind::Failed,
        },
    )
    .expect("old worker terminal is recorded late");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("old worker retirement authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            retired: true,
        },
    )
    .expect("old worker retirement clears the serialized delegation slot");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AdmitLiveDelegation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: next_interaction.to_string(),
            operation_id: mm::OperationId("operation-next-active".to_string()),
            provider_turn_correlation: next_provider_turn.to_string(),
            delegation_identity_present: true,
            actionable_input_present: true,
            exact_join: true,
        },
    )
    .expect("new active turn can attach its delegation after old retirement");
}

#[test]
fn failed_start_retirement_clears_the_active_channel_fail_closed() {
    let mut authority = opened_authority();
    bind_and_admit(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            provider_turn_correlation: PROVIDER_TURN.to_string(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("worker start authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            started: false,
        },
    )
    .expect("failed start is recorded");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
        },
    )
    .expect("failed start retirement authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: operation_id(),
            worker_identity: WORKER.to_string(),
            retired: true,
        },
    )
    .expect("failed start is retired and its interaction abandoned");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("the exact failed-start provider turn still owns occupancy until finish");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: "interaction-after-failed-start".to_string(),
            provider_turn_ref: "provider-after-failed-start".to_string(),
        },
    )
    .expect("the next exact provider turn is admitted after the prior finish");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AdmitLiveDelegation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: "interaction-after-failed-start".to_string(),
            operation_id: mm::OperationId("operation-after-failed-start".to_string()),
            provider_turn_correlation: "provider-after-failed-start".to_string(),
            delegation_identity_present: true,
            actionable_input_present: true,
            exact_join: true,
        },
    )
    .expect("failed start cannot wedge the channel");
}

#[test]
fn stale_fence_and_ambiguous_context_retry_are_rejected() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    enqueue_mirror_row(&mut authority, "context-1", 1);

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: mm::FenceToken(fence().0 + 1),
                generation: generation(),
                append_id: "context-1".to_string(),
                previous_cursor: 0,
                next_cursor: 1,
            },
        )
        .is_err()
    );

    let authorized = apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-1".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
        },
    )
    .expect("exact cursor edge receives pre-send authority");
    assert!(authorized.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveContextAppendAuthorized {
            append_id,
            previous_cursor: 0,
            next_cursor: 1,
            ..
        } if append_id == "context-1"
    )));

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-1".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
            replacement_channel_id: "channel-live-recovery".to_string(),
            observation: mm::LiveContextAppendObservation::Ambiguous,
        },
    )
    .expect("first ambiguous observation records a no-retry fence");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ResolveLiveContextAppend {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                append_id: "context-1".to_string(),
                previous_cursor: 0,
                next_cursor: 1,
                replacement_channel_id: String::new(),
                observation: mm::LiveContextAppendObservation::Delivered,
            },
        )
        .is_err()
    );
}

#[test]
fn ambiguity_recovery_answer_and_seed_binding_commit_atomically() {
    const REPLACEMENT: &str = "channel-live-recovery-atomic";
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    enqueue_mirror_row(&mut authority, "context-recovery-atomic", 1);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-recovery-atomic".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
        },
    )
    .expect("exact append is authorized before ambiguous delivery");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-recovery-atomic".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
            replacement_channel_id: REPLACEMENT.to_string(),
            observation: mm::LiveContextAppendObservation::Ambiguous,
        },
    )
    .expect("ambiguity creates an exact replacement obligation");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AbandonLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
        },
    )
    .expect("ambiguous transport is closed before replacement open");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            llm_identity: identity(),
        },
    )
    .expect("exact replacement receives ordinary open admission");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 1,
        },
    )
    .expect("exact recovery replacement is staged before provider answer");

    let transition = apply(
        &mut authority,
        mm::MeerkatMachineInput::BindLiveContextRecoveryChannel {
            session_id: SESSION.to_string(),
            closing_channel_id: CHANNEL.to_string(),
            replacement_channel_id: REPLACEMENT.to_string(),
            answer_observation_sequence: 9,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-recovery-atomic".to_string(),
            canonical_seed_cursor: 1,
        },
    )
    .expect("provider answer truth and acknowledged recovery seed bind atomically");

    assert!(transition.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveContextRecoveryChannelBound {
            replacement_channel_id,
            status: mm::LiveWebrtcAnswerPublicStatus::Answered,
            answered: true,
            answer_observation_sequence: 9,
            canonical_seed_cursor: 1,
            ..
        } if replacement_channel_id == REPLACEMENT
    )));
    assert_eq!(
        authority
            .state()
            .live_context_cursor_by_channel
            .get(REPLACEMENT),
        Some(&1)
    );
    assert_eq!(
        authority
            .state()
            .live_webrtc_answer_status_by_channel
            .get(REPLACEMENT),
        Some(&mm::LiveWebrtcAnswerPublicStatus::Answered)
    );
}

#[test]
fn context_resolution_without_pre_send_authority_is_rejected() {
    let mut authority = opened_authority();
    bind_only(&mut authority);

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ResolveLiveContextAppend {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                append_id: "never-authorized".to_string(),
                previous_cursor: 0,
                next_cursor: 1,
                replacement_channel_id: String::new(),
                observation: mm::LiveContextAppendObservation::Delivered,
            },
        )
        .is_err()
    );
}

#[test]
fn rejected_context_append_clears_pending_edge_without_advancing_cursor() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    enqueue_mirror_row(&mut authority, "context-rejected", 1);

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-rejected".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
        },
    )
    .expect("pre-send authority");
    let rejected = apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-rejected".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
            replacement_channel_id: String::new(),
            observation: mm::LiveContextAppendObservation::Rejected,
        },
    )
    .expect("known rejection resolves without advancing");
    assert!(rejected.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveContextAppendResolved {
            cursor: 0,
            observation: mm::LiveContextAppendObservation::Rejected,
            retry_allowed: true,
            ..
        }
    )));

    enqueue_mirror_row(&mut authority, "context-retry", 1);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-retry".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
        },
    )
    .expect("new append identity can retry the unadvanced edge");
}

#[test]
fn active_turn_defers_context_without_loss_and_rows_send_in_canonical_order() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    enqueue_mirror_row(&mut authority, "context-first", 1);
    enqueue_mirror_row(&mut authority, "context-second", 2);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("provider turn is active");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                append_id: "context-first".to_string(),
                previous_cursor: 0,
                next_cursor: 1,
            },
        )
        .is_err(),
        "active provider turn is not a safe append boundary"
    );
    assert_eq!(
        authority
            .state()
            .live_context_queued_append_by_cursor
            .get(&1)
            .map(String::as_str),
        Some("context-first"),
        "deferred row remains in generated custody"
    );

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::CompleteLiveInteraction {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                provider_turn_ref: "unrelated-provider-turn".to_string(),
            },
        )
        .is_err(),
        "an unrelated provider turn cannot open the context boundary"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("provider turn reaches safe boundary");

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                append_id: "context-second".to_string(),
                previous_cursor: 1,
                next_cursor: 2,
            },
        )
        .is_err(),
        "row two cannot overtake the canonical outbox head"
    );
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-first".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
        },
    )
    .expect("first row authorizes once at safe boundary");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-first".to_string(),
            previous_cursor: 0,
            next_cursor: 1,
            replacement_channel_id: String::new(),
            observation: mm::LiveContextAppendObservation::Delivered,
        },
    )
    .expect("first row acknowledgement advances cursor exactly once");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveContextAppend {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            append_id: "context-second".to_string(),
            previous_cursor: 1,
            next_cursor: 2,
        },
    )
    .expect("second row authorizes only after first acknowledgement");
}
