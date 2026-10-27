//! Real A/B admission with existing canonical-row failures. The fixture scripts
//! the model and protected approval; physical MCP readiness remains covered by
//! attachment_native_tests. Failed-attachment controls also exercise the actual
//! retained A result owner; ordinary B/cancel controls cannot submit another input.
use super::*;
use meerkat_toolkit::continuation::{ContinuationStep, ContinuationStepOptions, SetupContinuationDelivery, SetupTerminalOutcome};
use toolkit_catalog::AcquisitionPhase;
use meerkat_runtime::SessionServiceRuntimeExt;

async fn prepared(fixture: &ContinuationFixture) -> (SetupSnapshot, PrivateOriginalRef) {
    let a = fixture.submit().await;
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(
        &fixture.session, dispatch.retained_work().unwrap().run_id())).await.unwrap();
    let snapshot = acquisition(fixture, &dispatch).await;
    let b = approve_b(fixture, snapshot.clone()).await;
    (snapshot, b)
}

async fn settled(fixture: &ContinuationFixture, input: &str) {
    let input = InputId::from_uuid(input.parse().unwrap());
    let mut settlements = fixture.machine.subscribe_run_settlements();
    tokio::time::timeout(WAIT, async {
        loop {
            let actual = fixture.machine.input_state(&fixture.session, &input).await.unwrap().unwrap();
            if let Some(run) = &actual.seed.last_run_id {
                if fixture.machine.run_turn_terminal(&fixture.session, run).await == Some(TurnTerminalOutcome::Completed)
                    && fixture.machine.current_run(&fixture.session).await.as_ref() != Some(run) { break; }
            }
            settlements.changed().await.unwrap();
        }
    }).await.expect("actual B input settles");
}

fn continuation_owner(fixture: &ContinuationFixture) -> (
    Arc<crate::installed_host::acquisition::InstalledNativeContinuation>, Arc<meerkat_mob_mcp::MobMcpState>,
    Arc<meerkat::ContinuationHostBindings>,
) {
    let adapter = crate::installed_host::acquisition::InstalledNativeContinuation::for_service_continuation_test(
        fixture.store.clone(), fixture.scope.clone(), fixture.source.clone());
    let state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(fixture.service.clone(),
        Some(fixture.machine.clone()), meerkat_mob::MobControlPrincipal::Owner).unwrap()
        .with_host_job_source(meerkat_mob_mcp::HostJobSource::new([adapter.namespace()], adapter.clone()).unwrap())
        .into_shared();
    let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()));
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    state.bind_continuations(inbox, &bindings).unwrap();
    adapter.bind_fixture_native(state.continuation_service().unwrap(), state.clone()).unwrap();
    (adapter, state, bindings)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_b_repairs_lost_a_result_without_another_native_input() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        source.reserve_service_continuation(&row).await.unwrap();
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_service_result BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.service_ready') IS NOT NULL \
             BEGIN SELECT RAISE(FAIL, 'lost setup result'); END;", snapshot.key().id)).unwrap();
        assert!(source.step_service_input(&row).await.is_err());
        let admitted = fixture.store.get(&row.key).await.unwrap().unwrap();
        let record = source.decode_record(&admitted, &b).unwrap();
        assert!(record.service_admission.is_some());
        assert_eq!(fixture.inputs().await, Ok(2));
        settled(&fixture, &record.service_admission.as_ref().unwrap().input_id).await;
        let count = fixture.model_requests();
        sql.execute_batch("DROP TRIGGER fail_service_result;").unwrap();
        source.step_service_input(&admitted).await.unwrap();
        let saved = source.service_task_snapshot(&record).await.unwrap();
        assert_eq!(saved.state().lifecycle_phase, AcquisitionPhase::Ready);
        assert!(saved.approved_service().is_some());
        let obligation = source.service_setup_store().unwrap().committed_continuation(
            &Id::new(snapshot.key().id.clone()).unwrap()).await.unwrap().unwrap();
        assert!(matches!(obligation.outcome(), SetupTerminalOutcome::ApprovedServiceAdmitted { .. }));
        let evidence = source.observe_service_continuation(&obligation).await.unwrap().unwrap();
        assert_eq!(evidence.session.as_str(), fixture.session.to_string());
        assert_eq!(evidence.durable_input.id.as_str(), record.service_admission.as_ref().unwrap().input_id);
        source.step_service_input(&admitted).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(fixture.model_requests(), count);
        assert_eq!(source.service_task_snapshot(&record).await.unwrap().condition(), saved.condition());
        assert_eq!(saved.request().authority_digest().unwrap(), snapshot.request().authority_digest().unwrap());

        // Use the actual installed continuation owner and native service. There
        // is deliberately no RuntimeDeliveryOwner to deliver a second input.
        let (adapter, _state, _bindings) = continuation_owner(&fixture);
        assert!(matches!(meerkat::RetainedJobSource::retained_job(adapter.as_ref(),
            &meerkat::ContinuationProducer::Host { namespace: adapter.namespace().into() },
            obligation.operation().as_str()).await, meerkat::RetainedJobLookup::Absent));
        assert!(adapter.admit(&obligation).await.unwrap().durable_input == evidence.durable_input);
        let options = ContinuationStepOptions { worker: id("b-receipt-recovery"), lease_ms: 30_000,
            owner_timeout_ms: 1_000, settlement_budget_ms: 1_000 };
        let setup = source.service_setup_store().unwrap();
        // Lose the outbox write after the same canonical B-backed receipt is
        // saved. Recovery must use that receipt and never another submission.
        assert!(setup.acknowledge_continuation(obligation.operation(), &evidence.receipt,
            adapter.as_ref()).await.unwrap().is_some());
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_b_outbox BEFORE UPDATE OF data ON toolkit_effects \
             WHEN NEW.record_id = '{}' BEGIN SELECT RAISE(FAIL, 'lost outbox write'); END;",
            snapshot.key().id)).unwrap();
        assert!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.is_err());
        sql.execute_batch("DROP TRIGGER fail_b_outbox;").unwrap();
        assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.unwrap(),
            ContinuationStep::Acknowledged(_)));
        assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.unwrap(),
            ContinuationStep::Acknowledged(_)));
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(fixture.model_requests(), count);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_approved_b_sources_cannot_both_reserve_a() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let other = approve_b_call(&fixture, snapshot, [Id::new("read").unwrap()].into(), None,
            "competing-protected-approval").await;
        let source = &fixture.source;
        let first = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let second = fixture.store.get(&source.key(&other).unwrap()).await.unwrap().unwrap();
        let (left, right) = tokio::join!(source.reserve_service_continuation(&first),
            source.reserve_service_continuation(&second));
        assert_ne!(left.is_ok(), right.is_ok());
        let winner = if left.is_ok() { &first } else { &second };
        let winner_record = source.decode_stored(winner).unwrap();
        let reserved = source.service_task_snapshot(&winner_record).await.unwrap();
        source.reserve_service_continuation(winner).await.unwrap();
        assert_eq!(source.service_task_snapshot(&winner_record).await.unwrap().condition(), reserved.condition(),
            "discarding the first reservation reply does not advance or replace the selection");
        let loser = if left.is_ok() { &second } else { &first };
        assert!(source.step_service_input(loser).await.is_err());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

struct ControlDetail(ContextReference);
impl ContextReferenceOwner for ControlDetail {
    fn verify_access(&self, value: &ContextReference, evidence: &SetupRequestEvidence,
        context: &SetupUseContext<'_>) -> Result<(), RequestError> {
        evidence.check_context(context)?;
        if value == &self.0 { Ok(()) } else { Err(RequestError::ContextUnavailable) }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_reservation_fences_failure_writer_but_preserves_actual_cancel() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (_, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        source.reserve_service_continuation(&row).await.unwrap();
        let record = source.decode_stored(&row).unwrap();
        let snapshot = source.service_task_snapshot(&record).await.unwrap();
        let evidence = snapshot.request().evidence();
        let constraints = snapshot.request().constraints();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let context = SetupUseContext { tenant: &constraints.tenant, deployment: &constraints.deployment,
            realm: &evidence.association().candidate().ingress_namespace.realm,
            target: &evidence.association().candidate().target, policy_revision: &constraints.policy_revision,
            now_ms: u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap(),
            authorization: dispatch.work_authorization().unwrap() };
        let detail = ContextReference { owner: id("service-admission-test"), id: id("cancel-detail"),
            revision: revision("1"), content_digest: toolkit_package::ContentDigest::of_bytes(b"cancel") };
        let references = ControlDetail(detail.clone());
        let setup = source.service_setup_store().unwrap();
        let lease = fixture.store.claim_record(snapshot.key(), snapshot.state().revision, "failure-writer", 10_000).await.unwrap();
        assert!(setup.resolve(&snapshot, meerkat_toolkit::setup::Resolution::Failed(detail.clone()), Some(&lease),
            meerkat_toolkit::setup::SetupAccess { evidence, context: &context, references: &references }).await.is_err());
        fixture.store.release_record_claim(&lease).await.unwrap();
        setup.cancel(&id(&snapshot.key().id), detail,
            meerkat_toolkit::setup::SetupAccess { evidence, context: &context, references: &references }).await.unwrap();
        source.step_service_input(&row).await.unwrap();
        let terminal = source.service_task_snapshot(&record).await.unwrap();
        assert_eq!(terminal.state().lifecycle_phase, AcquisitionPhase::Cancelled);
        assert!(terminal.service_attachment_failure().is_none());
        assert!(terminal.approved_service() == snapshot.approved_service());
        let obligation = setup.committed_continuation(&id(&snapshot.key().id)).await.unwrap().unwrap();
        assert!(obligation.approved_service().is_some());
        assert!(source.observe_service_continuation(&obligation).await.unwrap().is_none());
        assert!(source.observe_service_rejection(&obligation).await.unwrap().is_some());
        let (adapter, _state, _bindings) = continuation_owner(&fixture);
        let options = ContinuationStepOptions { worker: id("cancelled-b-outbox"), lease_ms: 30_000,
            owner_timeout_ms: 1_000, settlement_budget_ms: 1_000 };
        // First dispatch records uncertainty from the absence of an admitted
        // input. Existing reconciliation then authenticates permanent refusal.
        assert!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.is_err());
        for _ in 0..2 {
            assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.unwrap(),
                ContinuationStep::OwnerRejected));
        }
        let row = fixture.store.get(&row.key).await.unwrap().unwrap();
        assert!(source.decode_stored(&row).unwrap().operation_cancelled);
        assert!(source.step_service_input(&row).await.is_ok());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

struct ChangedInstallationReadback;
#[async_trait::async_trait]
impl ServiceInstallationReadback for ChangedInstallationReadback {
    async fn readback(&self, _: &ApprovalRecord) -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        let mut changed = reviewed_config("another-realm");
        changed.name = "another-installation".into();
        Ok(changed)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_pre_entry_readback_refusal_terminalizes_without_an_input() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let source = &fixture.source;
        source.bind_service_installation_readback(Arc::new(ChangedInstallationReadback)).unwrap();
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        // The real production preparation compares the configured descriptor
        // with the reviewed one and refuses before any native attachment/input.
        assert_eq!(source.step_service_input_with_attachment(&fixture.store, &row).await,
            Err(PrivateOriginalError::Denied));
        let closed = fixture.store.get(&row.key).await.unwrap().unwrap();
        let record = source.decode_stored(&closed).unwrap();
        assert!(record.operation_cancelled && record.final_association.is_none() && record.service_admission.is_none());
        let terminal = source.service_task_snapshot(&record).await.unwrap();
        assert_eq!(terminal.state().lifecycle_phase, AcquisitionPhase::Unavailable);
        assert!(terminal.service_attachment_failure().is_none(), "generic denial is not a native failure cause");
        assert!(!terminal.approved_service_was_admitted());
        assert_eq!(terminal.request().authority_digest().unwrap(), snapshot.request().authority_digest().unwrap());
        let setup = source.service_setup_store().unwrap();
        let obligation = setup.committed_continuation(&id(&snapshot.key().id)).await.unwrap().unwrap();
        let (adapter, _state, _bindings) = continuation_owner(&fixture);
        let options = ContinuationStepOptions { worker: id("refused-b-outbox"), lease_ms: 30_000,
            owner_timeout_ms: 1_000, settlement_budget_ms: 1_000 };
        assert!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.is_err());
        assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.unwrap(),
            ContinuationStep::OwnerRejected));
        source.step_service_input_with_attachment(&fixture.store, &closed).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_receipt_refuses_changed_native_identity_and_closed_host() {
    for change in ["session", "input", "association", "host"] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (snapshot, b) = prepared(&fixture).await;
            let source = &fixture.source;
            let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
            source.step_service_input(&row).await.unwrap();
            let row = fixture.store.get(&row.key).await.unwrap().unwrap();
            let mut record = source.decode_stored(&row).unwrap();
            settled(&fixture, &record.service_admission.as_ref().unwrap().input_id).await;
            let setup = source.service_setup_store().unwrap();
            let obligation = setup.committed_continuation(&id(&snapshot.key().id)).await.unwrap().unwrap();
            assert!(source.observe_service_continuation(&obligation).await.unwrap().is_some());
            let count = fixture.model_requests();
            match change {
                "session" => record.service_admission.as_mut().unwrap().session_id = meerkat_core::SessionId::new().to_string(),
                "input" => record.service_admission.as_mut().unwrap().input_id = meerkat_core::SessionId::new().to_string(),
                "association" => record.final_association = fixture.binding().await.unwrap(),
                "host" => {
                    use meerkat_toolkit::host_lifetime::control::{PrivateHostLifetimeControl, HostCloseReason};
                    fixture.lifetime().begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
                },
                _ => unreachable!(),
            }
            if change != "host" {
                fixture.store.compare_and_swap(Transition { key: row.key, expected_revision: row.revision,
                    expected_digest: Some(row.content_digest), claim: None, data: encode(record).unwrap(),
                    events: vec![], effects: vec![] }).await.unwrap();
            }
            assert!(source.observe_service_continuation(&obligation).await.is_err());
            assert_eq!(fixture.inputs().await, Ok(2));
            assert_eq!(fixture.model_requests(), count);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_accepted_b_with_failed_source_publication_never_commits_a_success() {
    use crate::private_originals::governed_submit::tests::DeliveryMode;
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::PoisonPublicationAfterSecondAcceptance).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        assert!(source.step_service_input(&row).await.is_err());
        assert_eq!(fixture.inputs().await, Ok(2), "native accepted B before its source publication failed");
        let current = fixture.store.get(&row.key).await.unwrap().unwrap();
        let record = source.decode_stored(&current).unwrap();
        assert!(record.final_association.is_some() && record.service_admission.is_none());
        assert!(source.service_setup_store().unwrap().committed_continuation(&id(&snapshot.key().id)).await.unwrap().is_none());
        // Losing the actual pending holder cannot convert native history into
        // fresh source custody, nor authorize another native input.
        source.retained.lock().unwrap().retain(|held| held.reference != b);
        assert!(source.step_service_input(&current).await.is_err());
        assert_eq!(fixture.store.get(&row.key).await.unwrap().unwrap(), current);
        assert_eq!(fixture.inputs().await, Ok(2));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_source_custody_without_durable_b_admission_is_unavailable() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (_, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_b_receipt BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.service_admission') IS NOT NULL \
             BEGIN SELECT RAISE(FAIL, 'lost B receipt'); END;", row.key.id)).unwrap();
        assert!(source.step_service_input(&row).await.is_err());
        assert_eq!(fixture.inputs().await, Ok(2));
        let retained = source.retained.lock().unwrap().iter().find(|held| held.reference == b).cloned().unwrap();
        let Some(PrivateOriginalSubmission::Admitted { input_id, .. }) = source.reconcile_original_submission(&b).await.unwrap()
            else { panic!("the actual retained source guard reconciles before its loss") };
        settled(&fixture, &input_id).await;
        // Drop the actual process-local custody while preserving both native
        // input and canonical source bytes. This is the cold-loss boundary,
        // not a fabricated native absence or a replacement acceptance guard.
        source.owner.release_original(&retained.original).unwrap();
        retained.unresolved.lock().unwrap().clear();
        source.retained.lock().unwrap().retain(|held| held.reference != b);
        drop(retained);
        sql.execute_batch("DROP TRIGGER fail_b_receipt;").unwrap();
        let current = fixture.store.get(&row.key).await.unwrap().unwrap();
        let before = current.content_digest.clone();
        let count = fixture.model_requests();
        assert!(matches!(source.step_service_input(&current).await, Err(PrivateOriginalError::Unavailable)));
        assert_eq!(fixture.store.get(&row.key).await.unwrap().unwrap().content_digest, before);
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(fixture.model_requests(), count);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_attachment_result_resumes_only_original_authority_once() {
    attachment_failure_delivery(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_attachment_observation_reaches_original_delivery_once() {
    attachment_failure_delivery(Some(serde_json::from_value(serde_json::json!({
        "phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 1}},
    })).unwrap())).await;
}

// Exercise the actual source writer, outbox and native A delivery. Optional
// host observations are synthetic fixture data, not native process capture.
async fn attachment_failure_delivery(
    host_observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
) {
    use meerkat::{ContinuationHostBindings, ContinuationKey, ContinuationOwner, ContinuationStatus,
        DetachedJobService, MemoryDetachedJobStore, RuntimeDeliveryOwner};
    use meerkat::surface::SessionServiceDeliveryHost;
    use meerkat_runtime::{RuntimeDeliveryInbox, InMemoryRuntimeStore};
    use meerkat_toolkit::setup::ApprovedServiceAttachmentFailure;
    let mut fixture = ContinuationFixture::for_service_input().await;
    let native_handle = fixture.native_for_console_mut().mob_handle();
    let mut owner_handle = None;
    let mut owner_passes = None;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        source.reserve_service_continuation(&row).await.unwrap();
        if let Some(observation) = host_observation {
            let mut invalid = row.clone();
            invalid.data.body["service_attachment_observation"] = serde_json::to_value(observation).unwrap();
            assert!(source.decode_stored(&invalid).is_err(), "observation alone is not a terminal failure");
        }
        // The other control covers exact native observation. This calls the
        // same source-owned terminal settlement and tests its real A delivery.
        if host_observation.is_some() {
            // Explicit synthetic host observation exercises the real writer;
            // it is not capture from the unchanged native attachment API.
            source.reject_failed_service_attachment_with_observation(
                &b, ApprovedServiceAttachmentFailure::Failed, host_observation).await.unwrap();
        } else {
            source.reject_failed_service_attachment(&b, ApprovedServiceAttachmentFailure::Failed).await.unwrap();
        }
        let observation = serde_json::json!({
            "phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 1}},
        });
        let setup = source.service_setup_store().unwrap();
        let obligation = setup.committed_continuation(&id(&snapshot.key().id)).await.unwrap().unwrap();
        assert!(source.failed_service_allows_original_result(&obligation).await.unwrap());
        let exact_digest = obligation.digest().unwrap();
        let adapter = crate::installed_host::acquisition::InstalledNativeContinuation::for_service_continuation_test(
            fixture.store.clone(), fixture.scope.clone(), source.clone());
        let state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(fixture.service.clone(),
            Some(fixture.machine.clone()), meerkat_mob::MobControlPrincipal::Owner).unwrap()
            .with_host_job_source(meerkat_mob_mcp::HostJobSource::new([adapter.namespace()], adapter.clone()).unwrap())
            .into_shared();
        let binding = obligation.continuation().native_original.as_ref().unwrap();
        let member = binding.member.as_ref().unwrap();
        state.try_mob_insert_handle(meerkat_mob::MobId::from(member.mob_id.as_str()), native_handle).await.unwrap();
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let bindings = Arc::new(ContinuationHostBindings::default());
        state.bind_continuations(inbox.clone(), &bindings).unwrap();
        let service = state.continuation_service().unwrap();
        adapter.bind_fixture_native(service.clone(), state.clone()).unwrap();
        let options = ContinuationStepOptions { worker: id("failed-service-result"), lease_ms: 30_000,
            owner_timeout_ms: 1_000, settlement_budget_ms: 1_000 };
        // The first step durably submits the existing result; no owner is
        // armed yet, so it cannot claim native admission.
        assert!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.is_err());
        let owner = ContinuationOwner::Member { mob_id: member.mob_id.clone(), identity: member.member.clone() };
        let key = ContinuationKey::new(obligation.continuation().delivery_key.to_string()).unwrap();
        let ContinuationStatus::Pending { receipt, .. } = service.continuation_status(&owner, &key).await.unwrap()
            else { panic!("failure result must be committed before the native owner starts"); };
        let pending = inbox.list_pending(&meerkat_runtime::LogicalRuntimeId(receipt.address), 2).await.unwrap();
        assert_eq!(pending.len(), 1);
        let payload: serde_json::Value = serde_json::from_slice(pending[0].submission.payload()).unwrap();
        let delivery: meerkat::ContinuationDelivery = serde_json::from_value(payload).unwrap();
        let mut expected = serde_json::json!({
            "outcome": obligation.outcome(), "service_attachment_failure": "failed",
            "explanation": "The selected service attachment attempt failed or was rejected. Its replacement instruction was permanently fenced before admission. This is a setup failure result under your original authority, not service readiness or permission to retry. Research any corrected choice and request a separately reviewed setup.",
        });
        if host_observation.is_some() {
            expected["service_attachment_observation"] = observation.clone();
            assert_eq!(serde_json::to_value(&obligation).unwrap()["service_attachment_observation"], observation);
        }
        let expected = String::from_utf8(toolkit_package::canonical_json(&expected).unwrap()).unwrap();
        assert_eq!(delivery.body, expected.into(), "the delivered body carries only the bounded failure facts, never native detail or readiness/cleanup claims");
        let jobs = Arc::new(MemoryDetachedJobStore::new());
        let host = SessionServiceDeliveryHost::new(&fixture.service, &fixture.machine,
            DetachedJobService::new(jobs.clone()), None).with_continuation_bindings(bindings.clone());
        let handle = RuntimeDeliveryOwner::new(jobs, inbox)
            .with_source_changes(bindings.subscribe_source_changes()).arm(Arc::new(host)).unwrap();
        owner_passes = Some(handle.subscribe_passes());
        owner_handle = Some(handle);
        let status = tokio::time::timeout(WAIT, async {
            loop {
                let status = service.continuation_status(&owner, &key).await.unwrap();
                if matches!(status, ContinuationStatus::Applied { .. }) { break status; }
                owner_passes.as_mut().unwrap().changed().await.unwrap();
            }
        }).await.expect("failed setup result is admitted under original retained work");
        let ContinuationStatus::Applied { session, input, .. } = status else { unreachable!() };
        assert_eq!(session, fixture.session);
        let stored_input = fixture.machine.input_state(&session, &input).await.unwrap().unwrap();
        assert_eq!(stored_input.state.retained_resume.as_ref().unwrap().identity, binding.retained_work);
        assert_eq!(fixture.inputs().await, Ok(2));
        for _ in 0..2 {
            assert!(matches!(setup.step_continuation(obligation.operation(), adapter.as_ref(), &options).await.unwrap(),
                ContinuationStep::Acknowledged(_)));
        }
        let b_record = source.decode_stored(&fixture.store.get(&row.key).await.unwrap().unwrap()).unwrap();
        assert!(b_record.operation_cancelled && b_record.final_association.is_none() && b_record.service_admission.is_none());
        if host_observation.is_some() {
            let mut invalid = fixture.store.get(&row.key).await.unwrap().unwrap();
            invalid.data.body["operation_cancelled"] = false.into();
            assert!(source.decode_stored(&invalid).is_err(), "observation cannot bypass cancellation");
        }
        assert_eq!(setup.committed_continuation(obligation.operation()).await.unwrap().unwrap().digest().unwrap(), exact_digest);
        assert_eq!(fixture.inputs().await, Ok(2), "neither reconciliation nor replay creates B or a second A result");
        if host_observation.is_some() {
            // A different valid observation is not interchangeable with the
            // exact source/obligation pair, even under the same failure enum.
            let current = fixture.store.get(&row.key).await.unwrap().unwrap();
            let original = current.data.clone();
            let mut changed = original.clone();
            changed.body["service_attachment_observation"]["phase"] = "launch".into();
            let changed = fixture.store.compare_and_swap(Transition {
                key: current.key, expected_revision: current.revision,
                expected_digest: Some(current.content_digest), claim: None,
                data: changed, events: vec![], effects: vec![],
            }).await.unwrap();
            assert!(matches!(source.failed_service_allows_original_result(&obligation).await,
                Err(meerkat_toolkit::continuation::ContinuationError::InvalidReceipt)));
            fixture.store.compare_and_swap(Transition {
                key: changed.key, expected_revision: changed.revision,
                expected_digest: Some(changed.content_digest), claim: None,
                data: original, events: vec![], effects: vec![],
            }).await.unwrap();
            // Restoring bytes does not restore the source revision retained
            // by the actual cancellation owner. No new authority is published.
            assert!(matches!(source.failed_service_allows_original_result(&obligation).await,
                Err(meerkat_toolkit::continuation::ContinuationError::OwnerUnavailable)));
        }
    })).catch_unwind().await;
    drop(owner_handle);
    if let Some(mut passes) = owner_passes {
        tokio::time::timeout(WAIT, async { while passes.changed().await.is_ok() {} }).await.unwrap();
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_b_wins_over_a_late_failed_attachment_settlement() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (snapshot, b) = prepared(&fixture).await;
        let source = &fixture.source;
        let row = fixture.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        source.step_service_input(&row).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(2));
        source.reject_failed_service_attachment(&b,
            meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Failed).await.unwrap();
        source
            .reject_failed_service_attachment_with_observation(
                &b,
                meerkat_toolkit::setup::ApprovedServiceAttachmentFailure::Failed,
                Some(
                    serde_json::from_value(serde_json::json!({
                        "phase": "initialize", "class": {"kind": "timed_out"},
                    }))
                    .unwrap(),
                ),
            )
            .await
            .unwrap();
        let obligation = source.service_setup_store().unwrap().committed_continuation(&id(&snapshot.key().id))
            .await.unwrap().unwrap();
        assert!(matches!(obligation.outcome(), SetupTerminalOutcome::ApprovedServiceAdmitted { .. }));
        assert!(obligation.service_attachment_observation().is_none());
        assert!(obligation.service_attachment_failure().is_none());
        assert!(!source.failed_service_allows_original_result(&obligation).await.unwrap());
        assert!(source.observe_service_continuation(&obligation).await.unwrap().is_some());
        assert_eq!(fixture.inputs().await, Ok(2));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}
