//! Behavioral tests for the catalog-owned GPT Live machine region.

#![allow(clippy::expect_used, clippy::panic)]

#[cfg(feature = "live")]
use meerkat_live::{LiveChannelId, LiveSidebandTurnRef};
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
        mm::MeerkatMachineInput::ResolveLiveExecutionModeAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            profile_id: "test-function-bridge".to_string(),
            requested_mode: mm::LiveExecutionMode::FunctionBridge,
            function_bridge_available: true,
            client_context_available: false,
        },
    )
    .expect("function bridge mode is independently qualified");
    apply(
        authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor,
            pending_receipt: "pending-receipt".to_string(),
        },
    )
    .expect("strict experimental execution is staged before provider answer");
    apply(
        authority,
        mm::MeerkatMachineInput::RegisterLivePlaybackOwner {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            owner_id: "playback-owner".to_string(),
            readiness_id: "playback-readiness".to_string(),
            pending_receipt: "pending-receipt".to_string(),
        },
    )
    .expect("playback owner is ready while execution remains pending");
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
            activation_receipt: "activation-receipt".to_string(),
        },
    )
    .expect("provider answer and acknowledged seed bind experimental execution atomically");
}

fn bind_and_admit(authority: &mut mm::MeerkatMachineAuthority) {
    bind_only(authority);
    admit_provider_turn_delegation(authority);
}

fn confirm_delegation_transcript(authority: &mut mm::MeerkatMachineAuthority) {
    apply(
        authority,
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
    .expect("canonical final transcript confirms the exact operation before worker start");
}

fn prepare_confirmed_completed_worker(authority: &mut mm::MeerkatMachineAuthority) {
    confirm_delegation_transcript(authority);
    apply(
        authority,
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
        authority,
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
        authority,
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
    .expect("completed worker terminal is recorded after transcript confirmation");
    apply(
        authority,
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
        authority,
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
}

#[cfg(feature = "live")]
#[test]
fn replacement_channel_accepts_reset_provider_local_turn_ref_and_fences_stale_source() {
    const REPLACEMENT: &str = "channel-live-authority-replacement";
    const INTERACTION_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const INTERACTION_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    let mut authority = opened_authority();
    bind_only(&mut authority);
    let channel_a = LiveChannelId::new(CHANNEL);
    let turn_a = LiveSidebandTurnRef::__from_provider_observation(
        &channel_a,
        "turn:1".to_string(),
        "private-provider-turn-a".to_string(),
    )
    .expect("channel A provider turn");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION_A.to_string(),
            provider_turn_ref: turn_a.adapter_key().to_string(),
        },
    )
    .expect("channel A first provider turn is admitted");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::CompleteLiveInteraction {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            provider_turn_ref: turn_a.adapter_key().to_string(),
        },
    )
    .expect("channel A provider turn completes");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AbandonLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
        },
    )
    .expect("channel A closes before replacement admission");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            llm_identity: identity(),
        },
    )
    .expect("channel B open admission");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::BindLiveExecutionChannel {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
        },
    )
    .expect("channel B binds the same runtime incarnation");

    let channel_b = LiveChannelId::new(REPLACEMENT);
    let turn_b = LiveSidebandTurnRef::__from_provider_observation(
        &channel_b,
        "turn:1".to_string(),
        "private-provider-turn-b".to_string(),
    )
    .expect("channel B provider turn");
    assert_ne!(
        turn_a.adapter_key(),
        turn_b.adapter_key(),
        "the authoritative channel incarnation namespaces a reset provider-local ref"
    );
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: REPLACEMENT.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION_B.to_string(),
            provider_turn_ref: turn_b.adapter_key().to_string(),
        },
    )
    .expect("channel B first provider turn is admitted despite its reset local ref");

    let stale_a_turn = LiveSidebandTurnRef::__from_provider_observation(
        &channel_a,
        "turn:2".to_string(),
        "private-stale-provider-turn-a".to_string(),
    )
    .expect("stale channel A provider turn");
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc".to_string(),
                provider_turn_ref: stale_a_turn.adapter_key().to_string(),
            },
        )
        .is_err(),
        "closed channel A cannot publish after B becomes active"
    );
    assert_eq!(
        authority
            .state()
            .live_active_interaction_by_channel
            .get(REPLACEMENT)
            .map(String::as_str),
        Some(INTERACTION_B),
        "stale A rejection cannot mutate B's admitted interaction"
    );
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
            activation_receipt: "activation-receipt-running".to_string(),
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
            activation_receipt: "activation-receipt-context".to_string(),
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
fn open_turn_result_delivery_terminalizes_delivered_and_provider_rejected() {
    for observation in [
        mm::LiveDelegationResultDeliveryObservation::Delivered,
        mm::LiveDelegationResultDeliveryObservation::Rejected,
    ] {
        let mut authority = opened_authority();
        bind_experimental(&mut authority, 0);
        admit_provider_turn_delegation(&mut authority);
        prepare_confirmed_completed_worker(&mut authority);

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
        .expect("an active provider turn admits immediate result context");
        assert!(release.effects().iter().any(|effect| matches!(
            effect,
            mm::MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
                disposition: mm::LiveDelegationResultDisposition::OpenTurn,
                ..
            }
        )));

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
                result_digest: "open-turn-result-digest".to_string(),
                disposition: mm::LiveDelegationResultDisposition::OpenTurn,
            },
        )
        .expect("open-turn result receives distinct provider delivery authority");
        let resolved = apply(
            &mut authority,
            mm::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                operation_id: operation_id(),
                result_digest: "open-turn-result-digest".to_string(),
                replacement_channel_id: String::new(),
                observation,
            },
        )
        .expect("delivered and provider-rejected observations are generated terminals");
        assert!(resolved.effects().iter().any(|effect| matches!(
            effect,
            mm::MeerkatMachineEffect::LiveDelegationResultDeliveryResolved {
                disposition: mm::LiveDelegationResultDisposition::OpenTurn,
                observation: resolved_observation,
                retry_allowed: false,
                recovery_required: false,
                ..
            } if resolved_observation == &observation
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
                    result_digest: "open-turn-result-digest".to_string(),
                    disposition: mm::LiveDelegationResultDisposition::OpenTurn,
                },
            )
            .is_err(),
            "terminal provider evidence cannot be replayed"
        );
    }
}

#[test]
fn newer_user_turn_suppresses_old_result_while_worker_is_still_running() {
    const NEW_INTERACTION: &str = "33333333-3333-4333-8333-333333333333";
    const NEW_PROVIDER_TURN: &str = "opaque-provider-turn-newer-before-result";
    const RESULT_DIGEST: &str = "worker-pending-old-result-digest";

    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    admit_provider_turn_delegation(&mut authority);
    confirm_delegation_transcript(&mut authority);
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
    .expect("old worker start is authorized");
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
    .expect("old worker is still running when the user turn completes");
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
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: NEW_INTERACTION.to_string(),
            provider_turn_ref: NEW_PROVIDER_TURN.to_string(),
        },
    )
    .expect("newer user turn suppresses speech for the still-running old operation");
    assert!(
        authority
            .state()
            .live_result_speech_suppressed_operations
            .contains(&operation_id()),
        "suppression is machine-owned before worker completion or result release"
    );

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
    .expect("old worker completion remains durable after the newer user turn");
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
    .expect("old completed result is truthfully released as deferred context");
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
            result_digest: RESULT_DIGEST.to_string(),
            disposition: mm::LiveDelegationResultDisposition::DeferredContext,
        },
    )
    .expect("old released result receives exact provider delivery authority");
    let resolution = apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: operation_id(),
            result_digest: RESULT_DIGEST.to_string(),
            replacement_channel_id: String::new(),
            observation: mm::LiveDelegationResultDeliveryObservation::Delivered,
        },
    )
    .expect("old result acknowledgement remains a truthful delivered terminal");

    assert!(resolution.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultDeliveryResolved {
            observation: mm::LiveDelegationResultDeliveryObservation::Delivered,
            speech_disposition:
                mm::LiveDelegationResultSpeechDisposition::SuppressedByNewerUserTurn,
            retry_allowed: false,
            recovery_required: false,
            ..
        }
    )));
    assert_eq!(
        authority
            .state()
            .live_delegation_worker_terminal_by_operation
            .get(&operation_id()),
        Some(&mm::LiveDelegationWorkerTerminalKind::Completed),
        "speech suppression cannot rewrite durable executor completion"
    );
    assert_eq!(
        authority
            .state()
            .live_result_delivery_observation_by_operation
            .get(&operation_id()),
        Some(&mm::LiveDelegationResultDeliveryObservation::Delivered),
        "suppressed speech still projects truthful provider delivery"
    );
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                assistant_turn_ref: "stale-worker-result-assistant".to_string(),
            },
        )
        .is_err(),
        "old delivered result cannot admit spoken output after the newer user turn"
    );
}

#[test]
fn newer_user_turn_suppresses_late_old_result_speech_without_cancelling_completion() {
    const NEW_INTERACTION: &str = "22222222-2222-4222-8222-222222222222";
    const NEW_PROVIDER_TURN: &str = "opaque-provider-turn-newer";
    const RESULT_DIGEST: &str = "late-old-result-digest";

    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    admit_provider_turn_delegation(&mut authority);
    prepare_confirmed_completed_worker(&mut authority);
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
    .expect("old result is released while its provider turn is still open");
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
            result_digest: RESULT_DIGEST.to_string(),
            disposition: mm::LiveDelegationResultDisposition::OpenTurn,
        },
    )
    .expect("old result delivery is authorized before the newer user turn");
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
    .expect("old provider turn completes");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: NEW_INTERACTION.to_string(),
            provider_turn_ref: NEW_PROVIDER_TURN.to_string(),
        },
    )
    .expect("newer user turn supersedes the old result speech window");

    let resolution = apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: operation_id(),
            result_digest: RESULT_DIGEST.to_string(),
            replacement_channel_id: String::new(),
            observation: mm::LiveDelegationResultDeliveryObservation::Delivered,
        },
    )
    .expect("late old append acknowledgement remains a truthful delivered terminal");
    assert!(resolution.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveDelegationResultDeliveryResolved {
            observation: mm::LiveDelegationResultDeliveryObservation::Delivered,
            speech_disposition:
                mm::LiveDelegationResultSpeechDisposition::SuppressedByNewerUserTurn,
            retry_allowed: false,
            recovery_required: false,
            ..
        }
    )));
    assert_eq!(
        authority
            .state()
            .live_delegation_worker_terminal_by_operation
            .get(&operation_id()),
        Some(&mm::LiveDelegationWorkerTerminalKind::Completed),
        "speech suppression cannot rewrite durable executor completion"
    );
    assert_eq!(
        authority
            .state()
            .live_result_delivery_observation_by_operation
            .get(&operation_id()),
        Some(&mm::LiveDelegationResultDeliveryObservation::Delivered),
        "result delivery remains projected as delivered"
    );
    assert_eq!(
        authority
            .state()
            .live_active_interaction_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some(NEW_INTERACTION),
        "late acknowledgement cannot replace the newer foreground interaction"
    );
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                assistant_turn_ref: "stale-old-result-assistant".to_string(),
            },
        )
        .is_err(),
        "late old result acknowledgement cannot reopen spoken output"
    );
}

#[test]
fn delivered_deferred_result_authorizes_one_exact_resumed_assistant_turn() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    admit_provider_turn_delegation(&mut authority);
    prepare_confirmed_completed_worker(&mut authority);

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
    .expect("provider user turn opens the immediate assistant acknowledgement slot");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            assistant_turn_ref: "assistant-acknowledgement".to_string(),
        },
    )
    .expect("assistant acknowledgement consumes the user-turn response slot");

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
    .expect("late bounded result is released as deferred context");
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
            result_digest: "deferred-result-digest".to_string(),
            disposition: mm::LiveDelegationResultDisposition::DeferredContext,
        },
    )
    .expect("exact deferred result receives provider delivery authority");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: operation_id(),
            result_digest: "deferred-result-digest".to_string(),
            replacement_channel_id: String::new(),
            observation: mm::LiveDelegationResultDeliveryObservation::Delivered,
        },
    )
    .expect("provider acknowledgement reopens one result-response slot");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            assistant_turn_ref: "assistant-result-response".to_string(),
        },
    )
    .expect("resumed speech is frozen to the exact delegated interaction");
    assert_eq!(
        authority
            .state()
            .live_assistant_interaction_by_turn
            .get("assistant-result-response")
            .map(String::as_str),
        Some(INTERACTION)
    );
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                assistant_turn_ref: "unsolicited-after-result".to_string(),
            },
        )
        .is_err(),
        "one delivered result cannot authorize an unbounded stream of assistant turns"
    );
}

#[test]
fn confirmed_delegation_mints_distinct_effect_and_deferred_result_authorities() {
    let mut authority = opened_authority();
    bind_experimental(&mut authority, 0);
    admit_provider_turn_delegation(&mut authority);
    prepare_confirmed_completed_worker(&mut authority);

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
        mm::MeerkatMachineInput::ResolveLiveExecutionModeAdmission {
            session_id: SESSION.to_string(),
            channel_id: "channel-result-recovery".to_string(),
            profile_id: "test-function-bridge".to_string(),
            requested_mode: mm::LiveExecutionMode::FunctionBridge,
            function_bridge_available: true,
            client_context_available: false,
        },
    )
    .expect("result recovery mode is independently qualified");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: "channel-result-recovery".to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
            pending_receipt: "result-recovery-pending".to_string(),
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
    confirm_delegation_transcript(&mut authority);
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
    confirm_delegation_transcript(&mut authority);
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
    confirm_delegation_transcript(&mut authority);
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
    confirm_delegation_transcript(&mut authority);
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
        mm::MeerkatMachineInput::ResolveLiveExecutionModeAdmission {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            profile_id: "test-function-bridge".to_string(),
            requested_mode: mm::LiveExecutionMode::FunctionBridge,
            function_bridge_available: true,
            client_context_available: false,
        },
    )
    .expect("context recovery mode is independently qualified");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: REPLACEMENT.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 1,
            pending_receipt: "context-recovery-pending".to_string(),
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

fn bridge_operation_id() -> mm::OperationId {
    mm::OperationId("bridge-operation-live-authority".to_string())
}

fn activate_experimental_channel(
    authority: &mut mm::MeerkatMachineAuthority,
    channel_id: &str,
    pending_receipt: &str,
    owner_id: &str,
    readiness_id: &str,
    activation_receipt: &str,
    answer_observation_sequence: u64,
) {
    apply(
        authority,
        mm::MeerkatMachineInput::ResolveLiveExecutionModeAdmission {
            session_id: SESSION.to_string(),
            channel_id: channel_id.to_string(),
            profile_id: "test-function-bridge".to_string(),
            requested_mode: mm::LiveExecutionMode::FunctionBridge,
            function_bridge_available: true,
            client_context_available: false,
        },
    )
    .expect("function bridge mode is qualified for the exact channel");
    apply(
        authority,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: channel_id.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
            pending_receipt: pending_receipt.to_string(),
        },
    )
    .expect("exact channel execution is staged");
    apply(
        authority,
        mm::MeerkatMachineInput::RegisterLivePlaybackOwner {
            session_id: SESSION.to_string(),
            channel_id: channel_id.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            owner_id: owner_id.to_string(),
            readiness_id: readiness_id.to_string(),
            pending_receipt: pending_receipt.to_string(),
        },
    )
    .expect("exact channel playback owner is ready");
    apply(
        authority,
        mm::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
            session_id: SESSION.to_string(),
            channel_id: channel_id.to_string(),
            answer_observation_sequence,
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
            activation_receipt: activation_receipt.to_string(),
        },
    )
    .expect("exact channel execution becomes active");
}

fn observe_live_bridge_lineage(
    authority: &mut mm::MeerkatMachineAuthority,
    channel_id: &str,
    interaction_id: &str,
    provider_turn_ref: &str,
) {
    apply(
        authority,
        mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
            channel_id: channel_id.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: interaction_id.to_string(),
            provider_turn_ref: provider_turn_ref.to_string(),
        },
    )
    .expect("exact channel provider turn establishes structural lineage");
}

#[allow(clippy::too_many_arguments)]
fn admit_live_bridge_on_channel(
    authority: &mut mm::MeerkatMachineAuthority,
    channel_id: &str,
    interaction_id: &str,
    operation_id: mm::OperationId,
    provider_turn_ref: &str,
    provider_delegation_ref: &str,
    provider_call_ref: &str,
    request_digest: &str,
) -> Result<mm::MeerkatMachineTransition, mm::MeerkatMachineTransitionError> {
    apply(
        authority,
        mm::MeerkatMachineInput::AdmitLiveBridgeOperation {
            session_id: SESSION.to_string(),
            channel_id: channel_id.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: interaction_id.to_string(),
            operation_id,
            provider_turn_ref: provider_turn_ref.to_string(),
            provider_delegation_ref: provider_delegation_ref.to_string(),
            provider_call_ref: provider_call_ref.to_string(),
            agent_identity: mm::AgentIdentity("durable-agent".to_string()),
            canonical_context_revision: "canonical-revision".to_string(),
            request_digest: request_digest.to_string(),
            structural_lineage_proven: true,
        },
    )
}

fn prepare_live_bridge_lineage(authority: &mut mm::MeerkatMachineAuthority) {
    bind_experimental(authority, 0);
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
    .expect("provider turn establishes structural lineage");
}

fn admit_live_bridge(authority: &mut mm::MeerkatMachineAuthority) {
    apply(
        authority,
        mm::MeerkatMachineInput::AdmitLiveBridgeOperation {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
            provider_delegation_ref: "opaque-delegation".to_string(),
            provider_call_ref: "opaque-call".to_string(),
            agent_identity: mm::AgentIdentity("durable-agent".to_string()),
            canonical_context_revision: "canonical-revision".to_string(),
            request_digest: "request-digest-without-text-equivalence".to_string(),
            structural_lineage_proven: true,
        },
    )
    .expect("structurally correlated durable-member bridge is admitted");
}

fn issue_and_consume_live_bridge_effect(
    authority: &mut mm::MeerkatMachineAuthority,
    authority_id: &str,
    kind: mm::LiveBridgeEffectKind,
) {
    apply(
        authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeEffect {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: authority_id.to_string(),
            kind,
        },
    )
    .expect("effect authority is issued");
    apply(
        authority,
        mm::MeerkatMachineInput::ConsumeLiveBridgeEffectAuthority {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: bridge_operation_id(),
            authority_id: authority_id.to_string(),
            kind,
        },
    )
    .expect("effect authority is consumed into in-flight custody");
}

#[test]
fn live_bridge_provider_refs_are_channel_scoped_and_stale_callbacks_are_fenced() {
    const CHANNEL_B: &str = "channel-live-authority-b";
    const INTERACTION_A: &str = "interaction-live-authority-a";
    const INTERACTION_B: &str = "interaction-live-authority-b";
    const PROVIDER_TURN_A: &str = "opaque-provider-turn-a";
    const PROVIDER_TURN_B: &str = "opaque-provider-turn-b";
    const SHARED_DELEGATION_REF: &str = "provider-local-delegation-1";
    const SHARED_CALL_REF: &str = "provider-local-call-1";

    let operation_a = mm::OperationId("bridge-operation-channel-a".to_string());
    let operation_b = mm::OperationId("bridge-operation-channel-b".to_string());
    let mut authority = opened_authority();

    activate_experimental_channel(
        &mut authority,
        CHANNEL,
        "pending-a",
        "owner-a",
        "readiness-a",
        "activation-a",
        1,
    );
    observe_live_bridge_lineage(&mut authority, CHANNEL, INTERACTION_A, PROVIDER_TURN_A);
    admit_live_bridge_on_channel(
        &mut authority,
        CHANNEL,
        INTERACTION_A,
        operation_a.clone(),
        PROVIDER_TURN_A,
        SHARED_DELEGATION_REF,
        SHARED_CALL_REF,
        "request-a",
    )
    .expect("channel A accepts its provider-local refs");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::AbandonLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
        },
    )
    .expect("channel A is revoked before replacement");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ResolveLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL_B.to_string(),
            llm_identity: identity(),
        },
    )
    .expect("channel B becomes the current fenced channel");
    activate_experimental_channel(
        &mut authority,
        CHANNEL_B,
        "pending-b",
        "owner-b",
        "readiness-b",
        "activation-b",
        1,
    );
    observe_live_bridge_lineage(&mut authority, CHANNEL_B, INTERACTION_B, PROVIDER_TURN_B);
    admit_live_bridge_on_channel(
        &mut authority,
        CHANNEL_B,
        INTERACTION_B,
        operation_b.clone(),
        PROVIDER_TURN_B,
        SHARED_DELEGATION_REF,
        SHARED_CALL_REF,
        "request-b",
    )
    .expect("channel B may reuse opaque provider-local refs from revoked channel A");

    assert_eq!(
        authority
            .state()
            .live_bridge_operation_by_channel
            .get(CHANNEL_B),
        Some(&operation_b),
        "current bridge custody is keyed by the fenced channel"
    );
    assert_eq!(
        authority
            .state()
            .live_bridge_provider_delegation_by_operation
            .get(&operation_b)
            .map(String::as_str),
        Some(SHARED_DELEGATION_REF)
    );
    assert_eq!(
        authority
            .state()
            .live_bridge_provider_call_by_operation
            .get(&operation_b)
            .map(String::as_str),
        Some(SHARED_CALL_REF)
    );

    assert!(
        admit_live_bridge_on_channel(
            &mut authority,
            CHANNEL,
            INTERACTION_A,
            operation_a,
            PROVIDER_TURN_A,
            SHARED_DELEGATION_REF,
            SHARED_CALL_REF,
            "request-a",
        )
        .is_err(),
        "a stale channel A callback is rejected after channel B takes custody"
    );
    assert_eq!(
        authority
            .state()
            .live_bridge_operation_by_channel
            .get(CHANNEL_B),
        Some(&operation_b),
        "stale channel A input cannot alter channel B custody"
    );
}

#[test]
fn live_bridge_effect_outcomes_are_terminal_exact_and_cancellation_stable() {
    let mut authority = opened_authority();
    prepare_live_bridge_lineage(&mut authority);
    admit_live_bridge(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ConfirmLiveBridgeFinalInput {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("final input unlocks consequential effect classes");

    for authority_id in [
        "effect-committed",
        "effect-failed",
        "effect-unknown",
        "effect-cancelled-in-flight",
    ] {
        issue_and_consume_live_bridge_effect(
            &mut authority,
            authority_id,
            mm::LiveBridgeEffectKind::ToolDispatch,
        );
        assert!(
            authority
                .state()
                .live_bridge_in_flight_effect_authorities
                .contains(authority_id),
            "every consumed authority is explicitly in flight"
        );
    }

    let committed = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
            channel_id: CHANNEL.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "effect-committed".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
            outcome: mm::LiveBridgeEffectOutcome::Committed,
        },
    )
    .expect("successful dispatch records committed exactly once");
    assert!(committed.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeEffectOutcomeRecorded {
            outcome: mm::LiveBridgeEffectOutcome::Committed,
            replay: false,
            ..
        }
    )));
    let replay = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
            channel_id: CHANNEL.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "effect-committed".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
            outcome: mm::LiveBridgeEffectOutcome::Committed,
        },
    )
    .expect("exact same-outcome replay is idempotent");
    assert!(replay.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeEffectOutcomeRecorded {
            outcome: mm::LiveBridgeEffectOutcome::Committed,
            replay: true,
            ..
        }
    )));
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
                channel_id: CHANNEL.to_string(),
                operation_id: bridge_operation_id(),
                authority_id: "effect-committed".to_string(),
                kind: mm::LiveBridgeEffectKind::ToolDispatch,
                outcome: mm::LiveBridgeEffectOutcome::Failed,
            },
        )
        .is_err(),
        "a terminal outcome cannot be relabeled"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
            channel_id: CHANNEL.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "effect-failed".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
            outcome: mm::LiveBridgeEffectOutcome::Failed,
        },
    )
    .expect("definite dispatch failure is terminally recorded");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
            channel_id: CHANNEL.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "effect-unknown".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
            outcome: mm::LiveBridgeEffectOutcome::Unknown,
        },
    )
    .expect("dropped or unprovable dispatch records terminal unknown");

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CancelLiveBridgeOperation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            reason: mm::LiveBridgeCancellationReason::BargeIn,
        },
    )
    .expect("operation cancellation is independent of effect settlement");
    assert_eq!(
        authority
            .state()
            .live_bridge_effect_outcome_by_authority
            .get("effect-committed"),
        Some(&mm::LiveBridgeEffectOutcome::Committed),
        "cancellation does not rewrite a recorded outcome"
    );
    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeEffectOutcome {
            channel_id: CHANNEL.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "effect-cancelled-in-flight".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
            outcome: mm::LiveBridgeEffectOutcome::Unknown,
        },
    )
    .expect("an effect already dispatched before cancellation still settles exactly");
}

#[test]
fn live_bridge_is_prefinal_fail_closed_and_submission_recovery_never_resends() {
    let mut authority = opened_authority();
    prepare_live_bridge_lineage(&mut authority);

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AdmitLiveBridgeOperation {
                session_id: SESSION.to_string(),
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: bridge_operation_id(),
                provider_turn_ref: PROVIDER_TURN.to_string(),
                provider_delegation_ref: "opaque-delegation".to_string(),
                provider_call_ref: "opaque-call".to_string(),
                agent_identity: mm::AgentIdentity("durable-agent".to_string()),
                canonical_context_revision: "canonical-revision".to_string(),
                request_digest: "request-digest-without-text-equivalence".to_string(),
                structural_lineage_proven: false,
            },
        )
        .is_err(),
        "caller assertion cannot substitute for structural lineage"
    );
    admit_live_bridge(&mut authority);
    assert_eq!(
        authority
            .state()
            .live_bridge_agent_identity_by_operation
            .get(&bridge_operation_id()),
        Some(&mm::AgentIdentity("durable-agent".to_string()))
    );
    assert_eq!(
        authority
            .state()
            .live_bridge_context_revision_by_operation
            .get(&bridge_operation_id())
            .map(String::as_str),
        Some("canonical-revision")
    );

    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveBridgeEffect {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: bridge_operation_id(),
                authority_id: "prefinal-tool".to_string(),
                kind: mm::LiveBridgeEffectKind::ToolDispatch,
            },
        )
        .is_err(),
        "pre-final inference denies consequential effects"
    );
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeEffect {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "prefinal-model".to_string(),
            kind: mm::LiveBridgeEffectKind::ModelComputation,
        },
    )
    .expect("one restricted model computation is allowed pre-final");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ConsumeLiveBridgeEffectAuthority {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: bridge_operation_id(),
            authority_id: "prefinal-model".to_string(),
            kind: mm::LiveBridgeEffectKind::ModelComputation,
        },
    )
    .expect("effect authority is consumed exactly once");
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ConsumeLiveBridgeEffectAuthority {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                operation_id: bridge_operation_id(),
                authority_id: "prefinal-model".to_string(),
                kind: mm::LiveBridgeEffectKind::ModelComputation,
            },
        )
        .is_err(),
        "consumed effect authority cannot be replayed"
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::ConfirmLiveBridgeFinalInput {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("canonical evidence authorizes final input structurally");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeEffect {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            authority_id: "final-tool".to_string(),
            kind: mm::LiveBridgeEffectKind::ToolDispatch,
        },
    )
    .expect("final-input authority unlocks consequential effect selection");

    let start = apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeExecutionStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            request_digest: "request-digest-without-text-equivalence".to_string(),
        },
    )
    .expect("generated authority marks exact durable execution start");
    assert!(start.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeExecutionStartAuthorized {
            phase: mm::LiveBridgeOperationPhase::ExecutionRunning,
            ..
        }
    )));

    let terminal = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeExecutionTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            terminal: mm::MeerkatExecutionTerminal::Completed,
            result_digest: Some("result-digest".to_string()),
        },
    )
    .expect("Meerkat execution terminal settles independently");
    assert!(terminal.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeExecutionTerminalRecorded { replay: false, .. }
    )));
    let terminal_replay = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveBridgeExecutionTerminal {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            terminal: mm::MeerkatExecutionTerminal::Completed,
            result_digest: Some("result-digest".to_string()),
        },
    )
    .expect("exact terminal replay reconciles without mutation");
    assert!(terminal_replay.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeExecutionTerminalRecorded { replay: true, .. }
    )));
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::RecordLiveBridgeExecutionTerminal {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: bridge_operation_id(),
                terminal: mm::MeerkatExecutionTerminal::Completed,
                result_digest: Some("different-result-digest".to_string()),
            },
        )
        .is_err(),
        "terminal replay with a different digest is rejected"
    );
    assert!(
        !authority
            .state()
            .live_bridge_submission_state_by_operation
            .contains_key(&bridge_operation_id()),
        "execution terminal does not pretend provider output settled"
    );
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeSubmission {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            provider_call_ref: "opaque-call".to_string(),
            output_kind: mm::LiveBridgeOutputKind::Success,
            output_digest: "result-digest".to_string(),
        },
    )
    .expect("exact terminal result authorizes one provider submission");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ClaimLiveBridgeSubmissionAttempt {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            operation_id: bridge_operation_id(),
            provider_call_ref: "opaque-call".to_string(),
            output_digest: "result-digest".to_string(),
        },
    )
    .expect("durable send claim commits before transport IO");
    let recovered = apply(
        &mut authority,
        mm::MeerkatMachineInput::RecoverLiveBridgeSubmission {
            operation_id: bridge_operation_id(),
        },
    )
    .expect("claimed submission recovers as ambiguous");
    assert!(recovered.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeSubmissionRecoveredAmbiguous {
            state: mm::LiveBridgeSubmissionState::SubmissionAmbiguous,
            retry_allowed: false,
            ..
        }
    )));
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::ClaimLiveBridgeSubmissionAttempt {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                operation_id: bridge_operation_id(),
                provider_call_ref: "opaque-call".to_string(),
                output_digest: "result-digest".to_string(),
            },
        )
        .is_err(),
        "recovery never reissues transport authority"
    );
}

#[test]
fn revoked_running_bridge_reconciles_physical_terminal_without_submission_authority() {
    let mut authority = opened_authority();
    prepare_live_bridge_lineage(&mut authority);
    admit_live_bridge(&mut authority);
    apply(
        &mut authority,
        mm::MeerkatMachineInput::ConfirmLiveBridgeFinalInput {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            provider_turn_ref: PROVIDER_TURN.to_string(),
        },
    )
    .expect("confirm exact final input");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AuthorizeLiveBridgeExecutionStart {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            request_digest: "request-digest-without-text-equivalence".to_string(),
        },
    )
    .expect("cross exact durable execution start boundary");
    apply(
        &mut authority,
        mm::MeerkatMachineInput::AbandonLiveOpenAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
        },
    )
    .expect("revoke provider channel while physical execution remains running");
    assert_eq!(
        authority
            .state()
            .live_bridge_phase_by_operation
            .get(&bridge_operation_id()),
        Some(&mm::LiveBridgeOperationPhase::ExecutionRunning)
    );
    assert_eq!(
        authority
            .state()
            .live_bridge_cancellation_reason_by_operation
            .get(&bridge_operation_id()),
        Some(&mm::LiveBridgeCancellationReason::ChannelClose)
    );
    assert!(
        !authority
            .state()
            .live_bridge_execution_terminal_by_operation
            .contains_key(&bridge_operation_id()),
        "source cancellation must not fabricate a physical executor terminal"
    );

    let recovered = apply(
        &mut authority,
        mm::MeerkatMachineInput::ReconcileRevokedLiveBridgeExecutionTerminal {
            channel_id: CHANNEL.to_string(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            request_digest: "request-digest-without-text-equivalence".to_string(),
            terminal: mm::MeerkatExecutionTerminal::Completed,
            result_digest: Some("recovered-result-digest".to_string()),
        },
    )
    .expect("reconcile exact late physical terminal");
    assert!(recovered.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeExecutionTerminalRecorded { replay: false, .. }
    )));
    let replay = apply(
        &mut authority,
        mm::MeerkatMachineInput::ReconcileRevokedLiveBridgeExecutionTerminal {
            channel_id: CHANNEL.to_string(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            request_digest: "request-digest-without-text-equivalence".to_string(),
            terminal: mm::MeerkatExecutionTerminal::Completed,
            result_digest: Some("recovered-result-digest".to_string()),
        },
    )
    .expect("exact late terminal replay is idempotent");
    assert!(replay.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveBridgeExecutionTerminalRecorded { replay: true, .. }
    )));
    assert!(
        apply(
            &mut authority,
            mm::MeerkatMachineInput::AuthorizeLiveBridgeSubmission {
                channel_id: CHANNEL.to_string(),
                runtime_id: runtime_id(),
                fence_token: fence(),
                generation: generation(),
                interaction_id: INTERACTION.to_string(),
                operation_id: bridge_operation_id(),
                provider_call_ref: "opaque-call".to_string(),
                output_kind: mm::LiveBridgeOutputKind::Success,
                output_digest: "recovered-result-digest".to_string(),
            },
        )
        .is_err(),
        "late terminal reconciliation cannot mint provider submission authority"
    );
}

#[test]
fn bridge_cancellation_and_owner_revocation_never_retire_the_durable_member() {
    let mut authority = opened_authority();
    prepare_live_bridge_lineage(&mut authority);
    admit_live_bridge(&mut authority);

    apply(
        &mut authority,
        mm::MeerkatMachineInput::CancelLiveBridgeOperation {
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            interaction_id: INTERACTION.to_string(),
            operation_id: bridge_operation_id(),
            reason: mm::LiveBridgeCancellationReason::BargeIn,
        },
    )
    .expect("bridge cancellation addresses only the exact operation");
    assert_eq!(
        authority.state().lifecycle_phase,
        mm::MeerkatPhase::Idle,
        "bridge cancellation cannot retire the durable member runtime"
    );
    assert_eq!(
        authority
            .state()
            .live_channel_identity_by_channel
            .get(CHANNEL),
        Some(&identity())
    );

    apply(
        &mut authority,
        mm::MeerkatMachineInput::RevokeLivePlaybackOwner {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            owner_id: "playback-owner".to_string(),
            readiness_id: "playback-readiness".to_string(),
        },
    )
    .expect("owner loss revokes channel authority");
    assert_eq!(
        authority
            .state()
            .live_experimental_pending_receipt_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some("pending-receipt"),
        "opaque pending custody survives revocation for stateless status"
    );
    assert_eq!(authority.state().lifecycle_phase, mm::MeerkatPhase::Idle);

    apply(
        &mut authority,
        mm::MeerkatMachineInput::RecordLiveCloseClosed {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            close_observation_sequence: 1,
        },
    )
    .expect("closed status settles channel custody without member retirement");
    assert_eq!(
        authority
            .state()
            .live_experimental_pending_receipt_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some("pending-receipt"),
        "opaque pending custody survives close for stateless status"
    );
    assert_eq!(
        authority
            .state()
            .live_activation_receipt_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some("activation-receipt"),
        "inactive activation custody survives close for stateless status"
    );
    let replay = apply(
        &mut authority,
        mm::MeerkatMachineInput::RevokeLiveChannelCloseCustody {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            pending_receipt: None,
            activation_receipt: Some("activation-receipt".to_string()),
        },
    )
    .expect("lost close response replays from exact closed tombstone");
    assert!(replay.effects().iter().any(|effect| matches!(
        effect,
        mm::MeerkatMachineEffect::LiveChannelCloseCustodyRevoked {
            already_closed: true,
            ..
        }
    )));
    assert_eq!(
        replay.effects().len(),
        1,
        "closed replay mints no new provider effect"
    );
    assert_eq!(authority.state().lifecycle_phase, mm::MeerkatPhase::Idle);
}

#[test]
fn close_custody_accepts_pending_or_activation_receipt_without_owner_echo() {
    let mut pending = opened_authority();
    apply(
        &mut pending,
        mm::MeerkatMachineInput::ResolveLiveExecutionModeAdmission {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            profile_id: "test-function-bridge".to_string(),
            requested_mode: mm::LiveExecutionMode::FunctionBridge,
            function_bridge_available: true,
            client_context_available: false,
        },
    )
    .expect("mode resolves before pending close");
    apply(
        &mut pending,
        mm::MeerkatMachineInput::StageExperimentalLiveExecution {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            runtime_id: runtime_id(),
            fence_token: fence(),
            generation: generation(),
            canonical_seed_cursor: 0,
            pending_receipt: "pending-receipt".to_string(),
        },
    )
    .expect("pending close can occur before owner registration");
    apply(
        &mut pending,
        mm::MeerkatMachineInput::RevokeLiveChannelCloseCustody {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            pending_receipt: Some("pending-receipt".to_string()),
            activation_receipt: None,
        },
    )
    .expect("exact pending receipt revokes close custody without owner echo");
    assert_eq!(
        pending.state().live_execution_phase_by_channel.get(CHANNEL),
        Some(&mm::LiveExecutionChannelPhase::Revoked)
    );
    assert_eq!(
        pending
            .state()
            .live_experimental_pending_receipt_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some("pending-receipt")
    );

    let mut active = opened_authority();
    prepare_live_bridge_lineage(&mut active);
    admit_live_bridge(&mut active);
    apply(
        &mut active,
        mm::MeerkatMachineInput::RevokeLiveChannelCloseCustody {
            session_id: SESSION.to_string(),
            channel_id: CHANNEL.to_string(),
            pending_receipt: None,
            activation_receipt: Some("activation-receipt".to_string()),
        },
    )
    .expect("exact activation receipt revokes active close custody");
    assert!(
        !active
            .state()
            .live_playback_owner_by_channel
            .contains_key(CHANNEL)
    );
    assert!(
        !active
            .state()
            .live_playback_readiness_by_channel
            .contains_key(CHANNEL)
    );
    assert_eq!(
        active
            .state()
            .live_activation_receipt_by_channel
            .get(CHANNEL)
            .map(String::as_str),
        Some("activation-receipt"),
        "inactive activation tombstone remains valid for close and status"
    );
    assert_eq!(
        active
            .state()
            .live_bridge_execution_terminal_by_operation
            .get(&bridge_operation_id()),
        Some(&mm::MeerkatExecutionTerminal::Cancelled)
    );
    assert_eq!(
        active
            .state()
            .live_bridge_cancellation_reason_by_operation
            .get(&bridge_operation_id()),
        Some(&mm::LiveBridgeCancellationReason::ChannelClose)
    );
    assert_eq!(active.state().lifecycle_phase, mm::MeerkatPhase::Idle);
    assert_eq!(
        active.state().live_channel_identity_by_channel.get(CHANNEL),
        Some(&identity()),
        "channel close custody never retires the durable member"
    );
}
