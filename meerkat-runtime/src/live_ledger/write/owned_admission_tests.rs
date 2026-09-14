use super::*;
use crate::MeerkatMachine;
use crate::live_grant::{LiveExecutionGrant, LiveExecutionGrantIssuer};
use crate::live_ledger::authority::dsl;
use crate::live_ledger::source::LiveSourceRow;
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use crate::service_ext::SessionServiceRuntimeExt;
use meerkat_core::live_execution::request::LiveSourceKey;

#[path = "restore_tests.rs"]
mod restore_tests;

#[path = "recovery_tests.rs"]
mod recovery_tests;

#[path = "claim_tests.rs"]
mod claim_tests;

#[path = "physical_dispatch_tests.rs"]
mod physical_dispatch_tests;

#[path = "settlement_tests.rs"]
mod settlement_tests;

#[path = "request_completion_tests.rs"]
mod request_completion_tests;

#[path = "source_cancellation_tests.rs"]
mod source_cancellation_tests;

#[path = "callback_suspension_tests.rs"]
mod callback_suspension_tests;

#[derive(Default)]
struct LegacyScopeExecutor {
    ordinary_calls: usize,
}

#[tokio::test]
async fn native_client_delegation_refusal_is_source_first_after_text_and_close() -> TestResult {
    use crate::live_ledger::transcript_authority::{
        LiveClientDelegationOutcome, LiveSourceReservationOutcome,
    };
    use meerkat_core::live_execution::request::LiveProviderReference;
    use meerkat_core::live_observation::{
        LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    for backend in backends() {
        let mut owned = OwnedFixture::new(backend).await?;
        let id = LiveProviderReference::new("permissionless-client-source")?;
        let outcome = owned
            .channel
            .reserve_and_admit_client_source(&owned.machine, id.clone(), 0.0, None)
            .await?;
        let LiveClientDelegationOutcome::Source(LiveSourceReservationOutcome::Retained(original)) =
            outcome
        else {
            return Err("permissionless source did not retain its refusal".into());
        };
        assert!(
            matches!(original.as_ref(), LiveSourceEntryRecord::Reservation { record }
            if matches!(record.disposition(), LiveSourceDisposition::Refused { .. }))
        );
        owned
            .channel
            .append(LiveTranscriptObservation::new(
                LiveTranscriptDirection::Input,
                LiveTranscriptRange::new(1.0, 2.0)?,
                "later text",
            ))
            .await?;
        owned.channel.close().await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let replay = owned
            .channel
            .reserve_and_admit_client_source(&owned.machine, id, 0.0, Some(&owned.grant))
            .await?;
        let LiveClientDelegationOutcome::Source(LiveSourceReservationOutcome::Retained(replayed)) =
            replay
        else {
            return Err("old refusal minted new work".into());
        };
        assert_eq!(replayed, original);
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        assert!(
            owned
                .fixture
                .store
                .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()),)
                .await?
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn ordinary_runtime_retire_does_not_invent_archive_source_cancellation() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let fixture = &owned.fixture;
        let before = fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        owned
            .machine
            .retire_runtime_control_plane(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?;
        let after = fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        assert_eq!(after.bytes(), before.bytes());
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(state.source_cancellations.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn full_capacity_ingress_fence_uses_reserved_snapshot_bytes() -> TestResult {
    use crate::live_ledger::transcript_authority::dsl as transcript;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let fixture = &owned.fixture;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let quota = before.payload.used.checked_add(before.payload.reserved)?;
        let request = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&before.payload.request_snapshot)?,
        )?;
        let mut request = request.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut request, dsl::LiveRequestInput::CloseIngress)?;
        let transcript = transcript::LiveTranscriptMachineAuthority::recover_from_state(
            crate::generated::live_transcript_state::decode(&before.payload.transcript_snapshot)?,
        )?;
        let mut transcript = transcript.prepare_authority();
        transcript::LiveTranscriptMachineMutator::apply(
            &mut transcript,
            transcript::LiveTranscriptInput::CloseCurrentIngress,
        )?;
        let mut commit = PreparedLiveLedgerCommit::from_request_transition(
            fixture.session.id(),
            Some(&before),
            &request,
        )?
        .with_transcript_transition(&transcript)?;
        commit.quota = quota;
        assert!(
            commit
                .successor
                .payload
                .used
                .checked_add(commit.successor.payload.reserved)?
                .fits_within(quota)
        );
        assert!(commit.successor.payload.used.encoded_bytes > before.payload.used.encoded_bytes);
        assert!(
            commit.successor.payload.reserved.encoded_bytes < before.payload.reserved.encoded_bytes
        );
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(commit, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn native_archive_fence_closes_ingress_without_fabricating_run_completion() -> TestResult {
    use crate::live_ledger::transcript_authority::LiveTranscriptStoreOwner;
    use meerkat_core::live_execution::LiveChannelId;
    use meerkat_core::live_observation::{
        LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    for backend in backends() {
        for staged in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = if staged {
                Some(owned.staged_scope().await?)
            } else {
                None
            };
            let fixture = &owned.fixture;
            let actor = fixture.actor().await?;
            let lifecycle = fixture
                .store
                .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
                .await?
                .version()
                .ok_or("lifecycle")?
                .clone();
            let mut channel = LiveTranscriptStoreOwner::new(
                Arc::clone(&fixture.store),
                fixture.session.id().clone(),
                current_fence(),
            )
            .activate_channel(LiveChannelId::new("archive-channel"), lifecycle)
            .await?;
            let observation = || -> TestResult<_> {
                Ok(LiveTranscriptObservation::new(
                    LiveTranscriptDirection::Input,
                    LiveTranscriptRange::new(0.0, 1.0)?,
                    "observed",
                ))
            };
            channel.append(observation()?).await?;
            let before = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            let prior =
                crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
            let lease = owned
                .machine
                .prepare_session_archive_lease(fixture.session.id())
                .await?
                .ok_or("archive lease")?;
            drop(lease);
            let after = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            let request =
                crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
            let transcript = crate::generated::live_transcript_state::decode(
                &after.payload.transcript_snapshot,
            )?;
            assert!(!request.ingress_open && !transcript.ingress_open);
            assert_eq!(
                after.payload.ingress_generation,
                transcript.ingress_generation
            );
            assert_eq!(after.reference.event_count, before.reference.event_count);
            assert_eq!(request.request_inputs, prior.request_inputs);
            assert_eq!(request.run_requests, prior.run_requests);
            assert_eq!(request.claim_phases, prior.claim_phases);
            assert_eq!(
                request.request_completion_obligations.len(),
                usize::from(staged)
            );
            assert_eq!(fixture.actor().await?, actor);
            if let Some(scope) = scope {
                assert_eq!(
                    request.request_phases[&scope.record().request_id.to_string()],
                    dsl::LiveRequestPhase::Running
                );
            }
            assert!(channel.append(observation()?).await.is_err());
            let lease = owned
                .machine
                .prepare_session_archive_lease(fixture.session.id())
                .await?
                .ok_or("archive lease")?;
            drop(lease);
            let repeated = fixture
                .ops()?
                .load_live_head(fixture.session.id())
                .await?
                .ok_or("head")?;
            assert_eq!(
                repeated.payload.ingress_generation,
                after.payload.ingress_generation
            );
            assert_eq!(
                repeated.payload.request_snapshot,
                after.payload.request_snapshot
            );
            channel.close().await?;
            assert_eq!(fixture.actor().await?, actor);
        }
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_stage_retains_actual_input_and_admission_commit_by_run() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let snapshot =
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let run = scope.record().run_id.to_string();
        assert_eq!(
            snapshot.run_inputs.get(&run),
            Some(&scope.record().input_id.to_string()),
            "actual run must retain its own immutable ordinary input"
        );
        assert_eq!(
            snapshot.run_admission_commits.get(&run),
            Some(&serde_json::to_string(&scope.record().admission_commit)?),
            "historical scope must not borrow a replaceable current admission"
        );
    }
    Ok(())
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for LegacyScopeExecutor {
    async fn apply(
        &mut self,
        _run_id: meerkat_core::lifecycle::RunId,
        _primitive: meerkat_core::lifecycle::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::CoreExecutorError,
    > {
        self.ordinary_calls += 1;
        Err(meerkat_core::lifecycle::CoreExecutorError::Internal(
            "ordinary executor reached".into(),
        ))
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
        Ok(())
    }
}

#[tokio::test]
async fn owned_live_stage_seals_exact_core_scope_and_never_falls_back_to_ordinary_executor()
-> TestResult {
    use meerkat_core::execution_scope::RunExecutionAuthority;
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, StagedRunInput};
    use meerkat_core::lifecycle::{CoreExecutor, RunId, RunPrimitive};

    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let (run_id, authority) = owned
            .machine
            .prepare_next_batch_for_live_scope_authority_test(
                owned.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await?;
        let RunExecutionAuthority::Scoped(scope) = &authority else {
            return Err("joint Live stage lost its scoped authority".into());
        };
        assert_eq!(&scope.record().input_id, admitted.record().input_id());
        assert_eq!(scope.record().run_id, run_id);
        assert_eq!(
            &scope.record().admission_commit,
            admitted.record().admission_commit()
        );
        assert_eq!(
            scope.native_tools(),
            meerkat_core::ProviderNativeToolPolicy::DisableAll
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!(
                "__meerkat_core_runtime_generated_live_request_scope_build_v1_",
                env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
            )]
            fn build_scope_with_token(
                token: &'static (dyn std::any::Any + Send + Sync),
                scope_id: meerkat_core::execution_scope::RunEffectScopeId,
                record: meerkat_core::execution_scope::RunEffectScopeRecord,
                scope_revision: std::num::NonZeroU64,
            ) -> Result<meerkat_core::execution_scope::ScopedRunAuthority, String>;
        }
        #[allow(unsafe_code)]
        let forged = unsafe {
            build_scope_with_token(
                &(),
                scope.scope_id(),
                scope.record().clone(),
                std::num::NonZeroU64::new(head.reference.revision).ok_or("scope revision")?,
            )
        };
        assert!(
            forged
                .unwrap_err()
                .contains("generated LiveRequest owner bridge")
        );
        let run = scope.record().run_id.to_string();
        assert_eq!(
            state.run_scopes.get(&run),
            Some(&scope.scope_id().as_uuid().to_string())
        );
        assert_eq!(
            serde_json::from_str::<meerkat_core::execution_scope::RunEffectScopeRecord>(
                state.run_scope_records.get(&run).ok_or("scope record")?
            )?,
            *scope.record(),
        );
        let ordinary = RunPrimitive::StagedInput(StagedRunInput {
            execution_authority: RunExecutionAuthority::SessionPolicy,
            boundary: RunApplyBoundary::RunStart,
            appends: Vec::new(),
            contributing_input_ids: vec![admitted.record().input_id().clone()],
            turn_metadata: None,
        });
        let old_shape = serde_json::to_value(&ordinary)?;
        assert!(old_shape.get("execution_authority").is_none());
        assert_eq!(serde_json::from_value::<RunPrimitive>(old_shape)?, ordinary);
        let primitive = ordinary
            .clone()
            .with_execution_authority(authority.clone())?;
        primitive.validate_execution_authority(&run_id)?;
        assert!(
            primitive
                .validate_execution_authority(&RunId::new())
                .is_err()
        );
        let encoded = serde_json::to_value(&primitive)?;
        assert_eq!(encoded["execution_authority"]["kind"], "scoped");
        let error = serde_json::from_value::<RunPrimitive>(encoded).unwrap_err();
        assert!(error.to_string().contains("generated restoration"));
        let mut invalid = serde_json::to_value(&ordinary)?;
        invalid["execution_authority"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<RunPrimitive>(invalid).is_err());
        let mut wrong = ordinary.clone();
        if let RunPrimitive::StagedInput(staged) = &mut wrong {
            staged.contributing_input_ids.clear();
        }
        assert!(wrong.with_execution_authority(authority).is_err());

        let mut executor = LegacyScopeExecutor::default();
        let error = executor
            .apply_with_execution_authority(run_id.clone(), primitive)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not support scoped"));
        assert_eq!(executor.ordinary_calls, 0);
        let error = executor
            .apply_with_execution_authority(run_id, ordinary)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ordinary executor reached"));
        assert_eq!(executor.ordinary_calls, 1);
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_index_does_not_authorize_unsealed_kind_or_origin() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (_authority, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let rows = owned
            .fixture
            .store
            .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
            .await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("ordinary input missing".into());
        };
        let Some(crate::input::Input::LiveRequest(original)) = &row.state.persisted_input else {
            return Err("typed Live input missing".into());
        };
        for origin_only in [false, true] {
            let mut input = original.clone();
            input.header.id = meerkat_core::lifecycle::InputId::new();
            let input = if origin_only {
                crate::input::Input::Prompt(crate::input::PromptInput {
                    header: input.header,
                    content: meerkat_core::types::ContentInput::Text("unsealed".into()),
                    typed_turn_appends: Vec::new(),
                    injected_context: Vec::new(),
                    turn_metadata: None,
                })
            } else {
                crate::input::Input::LiveRequest(input)
            };
            assert!(matches!(
                owned
                    .machine
                    .accept_input(owned.fixture.session.id(), input)
                    .await,
                Err(crate::traits::RuntimeDriverError::ValidationFailed { reason })
                    if reason == crate::accept::RejectReason::LiveRequestRequiresGrant.to_string()
            ));
        }
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_replay_preserves_terminal_retired_input() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (original, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let rows = owned.fixture.store.load_input_states(&runtime).await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("ordinary input missing".into());
        };
        let key = row.state.idempotency_key.as_ref().ok_or("source key")?;
        assert!(
            owned
                .machine
                .cancel_input_if_present(
                    owned.fixture.session.id(),
                    original.record().input_id(),
                    "cancel queued Live input",
                )
                .await?
        );
        let (terminal, terminal_digest) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime, key)
            .await?
            .ok_or("terminal index")?
            .into_parts();
        assert!(terminal.state.persisted_input.is_none());
        assert!(
            crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
                original.record().input_id(),
                &terminal.seed,
            )?
        );
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let (joined, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        assert_eq!(joined.record(), original.record());
        assert!(completion.is_none());
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference, before.reference);
        let (_still_terminal, still_digest) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime, key)
            .await?
            .ok_or("terminal index")?
            .into_parts();
        assert_eq!(still_digest, terminal_digest);
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_run_start_persists_scope_and_ordinary_run_binding_together() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let run_id = owned
            .machine
            .prepare_next_batch_for_live_scope_test(
                owned.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let source = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        let LiveSourceEntryRecord::Reservation { record } = source.record()? else {
            return Err("reservation".into());
        };
        let request = record.request_id().to_string();
        assert_eq!(state.request_runs.get(&request), Some(&run_id.to_string()));
        let scope: meerkat_core::execution_scope::RunEffectScopeRecord = serde_json::from_str(
            state
                .run_scope_records
                .get(&run_id.to_string())
                .ok_or("run scope")?,
        )?;
        assert_eq!(&scope.input_id, admitted.record().input_id());
        assert_eq!(scope.run_id, run_id);
        assert_eq!(
            &scope.admission_commit,
            admitted.record().admission_commit()
        );
        assert_eq!(&scope.executor, admitted.record().executor());
        assert_eq!(&scope.grant, admitted.record().grant());
        let rows = owned
            .fixture
            .store
            .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
            .await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("ordinary input missing".into());
        };
        assert_eq!(row.seed.last_run_id.as_ref(), Some(&run_id));
        let lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
            .await?;
        let crate::store::MachineLifecycleObservation::Decoded { record, .. } = lifecycle else {
            return Err("staged lifecycle missing".into());
        };
        assert_eq!(record.runtime_state(), Some(crate::RuntimeState::Running));
        assert_eq!(record.run().current_run_id(), Some(&run_id));
        assert_eq!(
            row.seed.phase,
            crate::input_state::InputLifecycleState::Staged
        );
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_stage_registration_refusal_keeps_all_durable_predecessors() -> TestResult {
    for backend in backends() {
        let fence = Arc::new(AdmissionFenceControl::default());
        let owned = OwnedFixture::with_registration_fence(backend, Some(fence.clone())).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let rows = owned.fixture.store.load_input_states(&runtime_id).await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("input missing".into());
        };
        let key = row.state.idempotency_key.as_ref().ok_or("key")?;
        let (_, before_input) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime_id, key)
            .await?
            .ok_or("index")?
            .into_parts();
        let before_lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        fence
            .refuse
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            owned
                .machine
                .prepare_next_batch_for_live_scope_test(
                    owned.fixture.session.id(),
                    admitted.record().input_id(),
                )
                .await
                .is_err()
        );
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after, before);
        let (_, after_input) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime_id, key)
            .await?
            .ok_or("index")?
            .into_parts();
        assert_eq!(after_input, before_input);
        assert_eq!(
            owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?,
            before_lifecycle,
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_live_stage_outlives_caller_drop_inside_physical_commit() -> TestResult {
    for backend in backends() {
        let fence = Arc::new(AdmissionFenceControl::default());
        let owned = OwnedFixture::with_registration_fence(backend, Some(fence.clone())).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let machine = Arc::clone(&owned.machine);
        let session = owned.fixture.session.id().clone();
        let input = admitted.record().input_id().clone();
        fence.pause.store(true, std::sync::atomic::Ordering::SeqCst);
        let release = ReleaseAdmissionFence(&fence);
        let caller = tokio::spawn(async move {
            machine
                .prepare_next_batch_for_live_scope_test(&session, &input)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), fence.entered.notified()).await?;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(release);
        let after = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let head = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                if head.reference != before.reference {
                    return TestResult::Ok(head);
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        let rows = owned
            .fixture
            .store
            .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
            .await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("input missing".into());
        };
        let run_id = row.seed.last_run_id.as_ref().ok_or("staged run")?;
        assert_eq!(state.request_runs.len(), 1);
        assert_eq!(
            state.request_runs.values().next(),
            Some(&run_id.to_string())
        );
        assert_eq!(after.reference.revision, before.reference.revision + 1);
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn owned_live_stage_late_lifecycle_failure_rolls_back_input_and_scope() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let rows = owned.fixture.store.load_input_states(&runtime_id).await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("input missing".into());
        };
        let key = row.state.idempotency_key.as_ref().ok_or("key")?;
        let (_, before_input) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime_id, key)
            .await?
            .ok_or("index")?
            .into_parts();
        let before_lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let connection = rusqlite::Connection::open(&owned.fixture.path)?;
        connection.execute_batch(
            "CREATE TRIGGER reject_live_staged_lifecycle
             BEFORE UPDATE ON runtime_states
             BEGIN SELECT RAISE(ABORT, 'synthetic late Live stage failure'); END;",
        )?;
        drop(connection);
        let error = owned
            .machine
            .prepare_next_batch_for_live_scope_test(
                owned.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await
            .expect_err("late stage failure must refuse run start");
        assert!(
            error
                .to_string()
                .contains("synthetic late Live stage failure"),
            "{error}"
        );
        assert!(
            !owned
                .machine
                .is_durability_ready(owned.fixture.session.id())
                .await
        );
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after, before);
        let (_, after_input) = owned
            .fixture
            .store
            .load_input_state_by_idempotency_key(&runtime_id, key)
            .await?
            .ok_or("index")?
            .into_parts();
        assert_eq!(after_input, before_input);
        assert_eq!(
            owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?,
            before_lifecycle,
        );
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_run_start_survives_committed_ingress_close() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
            Arc::clone(&owned.fixture.store),
            owned.fixture.session.id().clone(),
        )
        .commit(dsl::LiveRequestInput::CloseIngress, current_fence())
        .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let run = owned
            .machine
            .prepare_next_batch_for_live_scope_test(
                owned.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await?;
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        assert!(!state.ingress_open);
        assert_eq!(state.request_runs.values().next(), Some(&run.to_string()));
        assert_eq!(state.request_runs.len(), 1);
        assert_eq!(state.run_scope_records.len(), 1);
        assert_eq!(after.reference.revision, before.reference.revision + 1);
    }
    Ok(())
}

struct OwnedFixture {
    fixture: Fixture,
    machine: Arc<MeerkatMachine>,
    grant: LiveExecutionGrant<()>,
    source: LiveSourceKey,
    channel: crate::live_ledger::transcript_authority::LiveTranscriptChannelIngress,
}

#[tokio::test]
async fn source_drain_fence_keeps_observation_ingress_without_admitting_new_work() -> TestResult {
    use meerkat_core::live_observation::{
        LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
    };
    for backend in backends() {
        for admitted_first in [false, true] {
            let mut owned = OwnedFixture::new(backend).await?;
            let admitted = if admitted_first {
                Some(
                    owned
                        .machine
                        .commit_live_input_admission(owned.source.clone(), &owned.grant)
                        .await?
                        .0,
                )
            } else {
                None
            };
            let before = owned
                .fixture
                .ops()?
                .lookup_live_source(&owned.source)
                .await?
                .ok_or("source")?;
            let fence = owned.channel.begin_drain().await?;
            assert_eq!(owned.channel.begin_drain().await?, fence);
            owned
                .channel
                .append(LiveTranscriptObservation::new(
                    LiveTranscriptDirection::Input,
                    LiveTranscriptRange::new(1.0, 2.0)?,
                    "received while draining",
                ))
                .await?;
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .lookup_live_source(&owned.source)
                    .await?
                    .ok_or("source")?
                    .bytes(),
                before.bytes()
            );
            if let Some(admitted) = admitted {
                let (_, scope) = owned
                    .machine
                    .prepare_next_batch_for_live_scope_authority_test(
                        owned.fixture.session.id(),
                        admitted.record().input_id(),
                    )
                    .await?;
                assert!(matches!(
                    scope,
                    meerkat_core::execution_scope::RunExecutionAuthority::Scoped(_)
                ));
            } else {
                assert!(
                    owned
                        .machine
                        .commit_live_input_admission(owned.source.clone(), &owned.grant)
                        .await
                        .is_err()
                );
                assert!(
                    owned
                        .fixture
                        .store
                        .load_input_states(&LogicalRuntimeId::for_session(
                            owned.fixture.session.id()
                        ),)
                        .await?
                        .is_empty()
                );
            }
            owned.channel.close().await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn channel_close_orders_initial_admission_without_revoking_already_admitted_work()
-> TestResult {
    for backend in backends() {
        for close_before_admission in [true, false] {
            let mut owned = OwnedFixture::new(backend).await?;
            if close_before_admission {
                owned.channel.close().await?;
                let before = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                assert!(
                    owned
                        .machine
                        .commit_live_input_admission(owned.source.clone(), &owned.grant)
                        .await
                        .is_err()
                );
                assert_eq!(
                    owned
                        .fixture
                        .ops()?
                        .load_live_head(owned.fixture.session.id())
                        .await?,
                    Some(before)
                );
                let state = crate::generated::live_request_state::decode(
                    &owned
                        .fixture
                        .ops()?
                        .load_live_head(owned.fixture.session.id())
                        .await?
                        .ok_or("head")?
                        .payload
                        .request_snapshot,
                )?;
                assert!(state.request_inputs.is_empty());
                assert!(state.run_requests.is_empty());
                assert!(
                    owned
                        .fixture
                        .store
                        .load_input_states(&LogicalRuntimeId::for_session(
                            owned.fixture.session.id()
                        ),)
                        .await?
                        .is_empty(),
                    "close-winning admission must not mint an ordinary input row",
                );
            } else {
                let (admitted, _) = owned
                    .machine
                    .commit_live_input_admission(owned.source.clone(), &owned.grant)
                    .await?;
                owned.channel.close().await?;
                let (repeated, _) = owned
                    .machine
                    .commit_live_input_admission(owned.source.clone(), &owned.grant)
                    .await?;
                assert_eq!(admitted.record(), repeated.record());
                let (_, authority) = owned
                    .machine
                    .prepare_next_batch_for_live_scope_authority_test(
                        owned.fixture.session.id(),
                        admitted.record().input_id(),
                    )
                    .await?;
                assert!(matches!(
                    authority,
                    meerkat_core::execution_scope::RunExecutionAuthority::Scoped(_)
                ));
            }
        }
    }
    Ok(())
}

impl OwnedFixture {
    async fn staged_scope(&self) -> TestResult<meerkat_core::execution_scope::ScopedRunAuthority> {
        let (admitted, _completion) = self
            .machine
            .commit_live_input_admission(self.source.clone(), &self.grant)
            .await?;
        let (_, authority) = self
            .machine
            .prepare_next_batch_for_live_scope_authority_test(
                self.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await?;
        match authority {
            meerkat_core::execution_scope::RunExecutionAuthority::Scoped(scope) => Ok(scope),
            meerkat_core::execution_scope::RunExecutionAuthority::SessionPolicy => {
                Err("lost scoped authority".into())
            }
        }
    }

    async fn new(backend: Backend) -> TestResult<Self> {
        Self::with_registration_fence(backend, None).await
    }

    async fn with_registration_fence(
        backend: Backend,
        fence: Option<Arc<dyn RuntimeStoreWriteFence>>,
    ) -> TestResult<Self> {
        let fixture = Fixture::new(backend).await?;
        let machine = Arc::new(MeerkatMachine::persistent(
            Arc::clone(&fixture.store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        if let Some(fence) = fence {
            let observed = machine
                .observe_cold_runtime_lifecycle(fixture.session.id())
                .await?;
            let outcome = machine
                .register_session_if_runtime_lifecycle_current(observed, fence)
                .await;
            assert!(
                matches!(
                    outcome,
                    crate::RuntimeSessionRegistrationOutcome::Applied { .. }
                        | crate::RuntimeSessionRegistrationOutcome::AlreadyExact { .. }
                ),
                "actual conditional registration failed: {outcome:?}"
            );
        }
        let bindings = machine
            .prepare_bindings(fixture.session.id().clone())
            .await?;
        let mut request = grant_activation_request(&fixture, bindings.epoch_id())?;
        request.executor.binding.binding_generation = 0;
        let observed = fixture
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
            .await?;
        assert!(
            matches!(&observed, crate::store::MachineLifecycleObservation::Decoded { record, .. }
                if record.binding().runtime_generation() == Some(0)
                    && record.binding().runtime_epoch_id() == Some(bindings.epoch_id().to_string().as_str())),
            "real prepared lifecycle: {observed:?}"
        );
        let grant = LiveExecutionGrantIssuer::new(Arc::clone(&fixture.store))
            .activate(request, current_fence())
            .await
            .map_err(|error| {
                format!("issuer rejected real prepared lifecycle {observed:?}: {error}")
            })?;
        use crate::live_ledger::transcript_authority::{
            LiveSourceReservationOutcome, LiveTranscriptStoreOwner,
        };
        use meerkat_core::live_observation::{
            LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
        };
        let version = observed.version().ok_or("current lifecycle")?.clone();
        let mut channel = LiveTranscriptStoreOwner::new(
            Arc::clone(&fixture.store),
            fixture.session.id().clone(),
            current_fence(),
        )
        .activate_channel(
            meerkat_core::live_execution::LiveChannelId::new("voice"),
            version,
        )
        .await?;
        channel
            .append(LiveTranscriptObservation::new(
                LiveTranscriptDirection::Input,
                LiveTranscriptRange::new(0.0, 1.0)?,
                "source evidence",
            ))
            .await?;
        let LiveSourceReservationOutcome::Retained(entry) = channel
            .reserve_client_source(
                meerkat_core::live_execution::request::LiveProviderReference::new(
                    "owned-admission",
                )?,
                0.0,
                Some(&grant),
            )
            .await?
        else {
            return Err("native fixture source did not commit".into());
        };
        let crate::live_source::LiveSourceEntryRecord::Reservation { record } = *entry else {
            return Err("native fixture source was cancelled".into());
        };
        assert!(matches!(
            record.disposition(),
            crate::live_source::LiveSourceDisposition::Reserved {}
        ));
        Ok(Self {
            fixture,
            machine,
            grant,
            source: record.source().clone(),
            channel,
        })
    }
}

#[derive(Default)]
struct AdmissionFenceControl {
    pause: std::sync::atomic::AtomicBool,
    refuse: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    released: std::sync::Mutex<bool>,
    release_signal: std::sync::Condvar,
}

impl AdmissionFenceControl {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release_signal.notify_all();
    }
}

struct ReleaseAdmissionFence<'a>(&'a AdmissionFenceControl);

impl Drop for ReleaseAdmissionFence<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl RuntimeStoreWriteFence for AdmissionFenceControl {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        use std::sync::atomic::Ordering;
        if self.refuse.load(Ordering::SeqCst) {
            return Ok(RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "external registration revoked".into(),
            });
        }
        if self.pause.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            let released = self.released.lock().unwrap();
            let (released, _) = self
                .release_signal
                .wait_timeout_while(released, std::time::Duration::from_secs(5), |released| {
                    !*released
                })
                .unwrap();
            if !*released {
                return Err(RuntimeStoreError::WriteFailed(
                    "admission test did not release its publication barrier".into(),
                ));
            }
        }
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_live_admission_outlives_caller_drop_inside_physical_commit() -> TestResult {
    use std::sync::atomic::Ordering;
    for backend in backends() {
        let fence = Arc::new(AdmissionFenceControl::default());
        let owned = OwnedFixture::with_registration_fence(backend, Some(fence.clone())).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let machine = owned.machine.clone();
        let source = owned.source.clone();
        let grant = owned.grant;
        fence.pause.store(true, Ordering::SeqCst);
        let release = ReleaseAdmissionFence(&fence);
        let caller =
            tokio::spawn(async move { machine.commit_live_input_admission(source, &grant).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), fence.entered.notified()).await?;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(release);
        let receipt = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let source = owned
                    .fixture
                    .ops()?
                    .lookup_live_source(&owned.source)
                    .await?
                    .ok_or("source")?;
                if let LiveSourceEntryRecord::Reservation { record } = source.record()?
                    && let LiveSourceDisposition::Admitted { receipt } = record.disposition()
                {
                    return TestResult::Ok(receipt.clone());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let rows = owned
            .fixture
            .store
            .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
            .await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err("caller drop lost or duplicated the ordinary input".into());
        };
        assert_eq!(&row.state.input_id, receipt.input_id());
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference.revision, before.reference.revision + 1);
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_preserves_external_registration_refusal() -> TestResult {
    use std::sync::atomic::Ordering;
    for backend in backends() {
        let fence = Arc::new(AdmissionFenceControl::default());
        let owned = OwnedFixture::with_registration_fence(backend, Some(fence.clone())).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let source_before = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        fence.refuse.store(true, Ordering::SeqCst);
        owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await
            .expect_err("native binding checks must not drop the external registration fence");
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference, before.reference);
        let source_after = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        assert_eq!(source_after.bytes(), source_before.bytes());
        assert!(
            owned
                .fixture
                .store
                .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
                .await?
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_replay_joins_original_input_even_after_grant_revocation() -> TestResult
{
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (original, _original_completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        for revoke in [false, true] {
            if revoke {
                crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
                    owned.fixture.store.clone(),
                    owned.fixture.session.id().clone(),
                )
                .commit(
                    dsl::LiveRequestInput::Revoke {
                        grant_id: owned.grant.record().grant_ref().id.as_uuid().to_string(),
                        generation: owned.grant.record().grant_ref().generation.get(),
                    },
                    current_fence(),
                )
                .await?;
            }
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let (replayed, completion) = owned
                .machine
                .commit_live_input_admission(owned.source.clone(), &owned.grant)
                .await?;
            assert_eq!(replayed.record(), original.record());
            assert!(completion.is_some(), "join the original nonterminal input");
            let after = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            assert_eq!(after.reference, before.reference);
            assert_eq!(
                owned
                    .fixture
                    .store
                    .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
                    .await?
                    .len(),
                1
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_concurrent_callers_collapse_to_one_input() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let (left, right) = tokio::join!(
            owned
                .machine
                .commit_live_input_admission(owned.source.clone(), &owned.grant),
            owned
                .machine
                .commit_live_input_admission(owned.source.clone(), &owned.grant)
        );
        let (left, left_completion) = left?;
        let (right, right_completion) = right?;
        assert_eq!(left.record(), right.record());
        assert!(left_completion.is_some() && right_completion.is_some());
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference.revision, before.reference.revision + 1);
        assert_eq!(
            owned
                .fixture
                .store
                .load_input_states(&LogicalRuntimeId::for_session(owned.fixture.session.id()))
                .await?
                .len(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn owned_live_admission_uses_real_zero_generation_binding_and_one_joint_commit() -> TestResult
{
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let fixture = &owned.fixture;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        assert!(
            fixture
                .store
                .load_input_states(&runtime_id)
                .await?
                .is_empty()
        );
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let (authority, handle) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        assert!(
            handle.is_some(),
            "ordinary completion registration must be reused"
        );
        assert_eq!(authority.record().executor().binding_generation, 0);
        let rows = fixture.store.load_input_states(&runtime_id).await?;
        let [crate::store::InputStateRow::Decoded(row)] = rows.as_slice() else {
            return Err(format!("expected one ordinary input, got {rows:?}").into());
        };
        assert_eq!(&row.state.input_id, authority.record().input_id());
        let Some(crate::input::Input::LiveRequest(input)) = &row.state.persisted_input else {
            return Err("ordinary persisted Live input missing".into());
        };
        assert_eq!(input.header.source, crate::input::InputOrigin::LiveRequest);
        let append = input
            .request
            .materialize(fixture.store.as_ref(), &input.header.id)
            .await?
            .ok_or("original request must materialize an append")?;
        assert!(matches!(
            append.role,
            meerkat_core::lifecycle::run_primitive::ConversationAppendRole::DelegatedRequest { .. }
        ));
        let after = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference.revision, before.reference.revision + 1);
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        let source = fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        let LiveSourceEntryRecord::Reservation { record } = source.record()? else {
            return Err("reservation".into());
        };
        let LiveSourceDisposition::Admitted { receipt } = record.disposition() else {
            return Err("source admission missing".into());
        };
        assert_eq!(receipt.as_ref(), authority.record());
        assert_eq!(
            state.request_inputs.get(&record.request_id().to_string()),
            Some(&authority.record().input_id().to_string())
        );
        assert_eq!(state.admitted_requests.len(), 1);
        assert!(
            state.bound_requests.is_empty(),
            "admission is not joint run staging"
        );
        assert!(
            state.claim_ids.is_empty(),
            "admission is not physical permission"
        );
    }
    Ok(())
}
