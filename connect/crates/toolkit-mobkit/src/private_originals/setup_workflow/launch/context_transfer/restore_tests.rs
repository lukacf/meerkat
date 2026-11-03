//! Controlled committed SQLite envelopes with real P/G native owners. Store
//! admission here is data setup, not a claim of a Find-fenced disclosure. The
//! existing public SDK controls separately retain actual Find commit coverage.
use super::*;
use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::DedicatedFixture;
use futures::FutureExt;
use meerkat_core::{SessionServiceHistoryExt, service::SessionHistoryQuery};
use meerkat_mobkit::identity_first::{AgentIdentity, TurnOutcome, TurnOutput, TurnTicket};
use meerkat_workgraph::UpdateWorkItemRequest;
use std::{panic::AssertUnwindSafe, time::Duration};
use toolkit_store::ToolkitStore;

pub(super) type Fixture =
    DedicatedFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;
pub(super) const WAIT: Duration = Duration::from_secs(30);
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

async fn finish(c: &Fixture, member: &str, input: PrivateOriginalSubmission) {
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = input
    else {
        panic!("actual native input")
    };
    assert!(matches!(
        c.f.source
            .bound_identity_runtime()
            .unwrap()
            .wait_for_turn(
                &AgentIdentity::parse(member).unwrap(),
                TurnTicket::parse(&ticket).unwrap(),
                WAIT,
            )
            .await,
        TurnOutcome::Completed {
            output: TurnOutput::Text { .. }
        }
    ));
}
pub(super) async fn native_rows(c: &Fixture) -> serde_json::Value {
    let mut rows = Vec::new();
    for session in [&c.f.session, &c.parent_session, &c.lead_session] {
        let states = c
            .history
            .as_ref()
            .unwrap()
            .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
            .await
            .unwrap();
        let history =
            c.f.service
                .read_history(
                    session,
                    SessionHistoryQuery {
                        offset: 0,
                        limit: Some(100),
                    },
                )
                .await
                .unwrap();
        assert!(!history.has_more);
        rows.push(serde_json::json!({"ids":states.iter().map(|row| &row.state.input_id).collect::<Vec<_>>(),
            "history":history.messages}));
    }
    serde_json::json!(rows)
}

pub(super) struct Data {
    pub(super) planning: Arc<InstalledWorkflowPlanning>,
    pub(super) envelope: TransferEnvelope,
    pub(super) original: crate::private_originals::acquisition::AcquisitionOriginal,
    pub(super) recipient: WorkGraphService,
    pub(super) sibling: SetupWorkflowWaiter,
}
pub(super) async fn prepare(c: &Fixture) -> Data {
    c.member
        .prepare_waiter_source(c.f.original_reference())
        .await
        .unwrap();
    c.f.source
        .prepare_workflow_parent_source(&c.parent.original)
        .await
        .unwrap();
    let parent =
        c.f.source
            .submit_authenticated_original(c.parent.clone())
            .await
            .unwrap();
    finish(c, "planning_parent", parent).await;
    let probe = c.f.probe.as_ref().unwrap();
    probe.arm_next_input();
    let a = c.f.submit().await;
    c.f.finish(&a).await.unwrap();
    let dispatch = probe.captured();
    assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
    let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
    let planning =
        c.f.source
            .prepare_workflow_planning(
                &c.parent.original,
                c.lead.clone(),
                c.terms.clone(),
                c.lineage.clone(),
            )
            .await
            .unwrap();
    let scope =
        serde_json::to_value(dispatch.work_authorization().unwrap().execution_scope()).unwrap();
    let call = "controlled-committed-envelope";
    let correlation = crate::installed_host::acquisition::installed_invocation_correlation(
        &original.reference,
        &scope,
        call,
    )
    .unwrap();
    let (membership, _, acquisition) = super::super::tests::join_waiter_context(
        &c.f,
        &planning,
        &original,
        &dispatch,
        &c.backend,
        &c.work,
        "donor",
        None,
        Some(correlation.clone()),
    )
    .await;
    // A second canonical interest keeps G/P open after this donor membership
    // departs. It is not a second grant or an A-to-G effect delegation.
    let (sibling, snapshot, _) = super::super::tests::join_waiter_context(
        &c.f, &planning, &original, &dispatch, &c.backend, &c.work, "sibling", None, None,
    )
    .await;
    let g = match planning.launch(&snapshot).await.unwrap() {
        WorkflowLaunchObservation::Native { observation } => observation,
        _ => panic!("actual G admission"),
    };
    finish(c, "planning_lead", g).await;
    let recipient = WorkGraphService::with_scope(
        c.backend.service().store().clone(),
        c.terms.realm.to_string(),
        planning.context_namespace().unwrap(),
    );
    let selected = TransferSelection {
        original: original.reference.clone(),
        association: original.original.association().clone(),
        correlation,
        workflow: membership.workflow().clone(),
        planning_source: c.lead.original.clone(),
        parent: planning.parent.association().clone(),
        lead: c.terms.lead.clone(),
        backend: c.backend.pin().clone(),
        recipient_realm: recipient.default_realm_id().into(),
        recipient_namespace: recipient.default_namespace().clone(),
        native_audience: original
            .original
            .association()
            .candidate()
            .target
            .audience
            .clone(),
        output_audiences: [id("donor")].into(),
    };
    let envelope = TransferEnvelope {
        purpose: PURPOSE.into(),
        recipient: selected,
        membership,
        input_digest: acquisition.request().input_digest().clone(),
        find_call: call.into(),
        find_scope: scope,
        request: acquisition.request().request().clone(),
        deadline_ms: acquisition.request().host().limits.deadline_ms,
    };
    Data {
        planning,
        envelope,
        recipient,
        sibling,
        original,
    }
}
pub(super) async fn restore(
    c: &Fixture,
    data: &Data,
    lifecycle: &AcquisitionRecovery,
) -> Result<WorkflowContextObservation, PrivateOriginalError> {
    // This exercises internal observation/registration mechanics. It does not
    // assert native Ready or publish a recovery worker; the public host facade
    // additionally requires its already published exact recovery control.
    data.planning
        .restore_committed_context(
            &c.f.source,
            &data.envelope.membership,
            data.recipient.clone(),
            &c.backend,
            data.envelope.recipient.output_audiences.clone(),
            &lifecycle.transfer_control(),
        )
        .await
}
pub(super) async fn store_envelope(data: &Data) -> WorkItem {
    match data
        .recipient
        .create_idempotent(
            admission_key(&data.envelope.membership).unwrap(),
            creation_request(&data.envelope).unwrap(),
        )
        .await
        .unwrap()
    {
        WorkAdmissionOutcome::Created(item) => item,
        _ => panic!("new controlled store admission"),
    }
}
pub(super) async fn close(
    c: &Fixture,
    lifecycle: &AcquisitionRecovery,
    body: Result<(), Box<dyn std::any::Any + Send>>,
) {
    lifecycle.stop();
    let joined = c.f.source.join_workflow_context_transfers(false).await;
    let cleanup = c.close().await;
    assert!(
        joined.is_ok() && cleanup.is_ok(),
        "all actual source/native owners joined: {joined:?}/{cleanup:?}"
    );
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_store_restore_has_one_recipient_owner_and_never_dispatches_unknown_input() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let data = prepare(&c).await;
        let before = native_rows(&c).await;
        let model_requests = c.f.model_requests();
        let effect_key = data.envelope.membership.delivery_effect().unwrap();
        let effect = c.f.store.get_effect(&effect_key).await.unwrap().unwrap();
        assert!(matches!(
            restore(&c, &data, &lifecycle).await.unwrap(),
            WorkflowContextObservation::Pending
        ));
        assert!(
            data.planning.context_transfers.lock().unwrap().is_empty(),
            "absence installs no owner"
        );
        let item = store_envelope(&data).await;
        // Preserve the existing warm per-source bound. Dormant choices exceed
        // this workflow's waiter limit without consuming canonical membership.
        for index in 0..128 {
            let mut selection = data.envelope.recipient.clone();
            selection.correlation = id(&format!("dormant-{index}"));
            let candidate = Arc::new(InstalledWorkflowContextTransfer {
                source: Arc::downgrade(&c.f.source),
                original: Arc::downgrade(&data.original.original),
                planning: Arc::downgrade(&data.planning),
                service: data.recipient.clone(),
                selected: selection.clone(),
                attempt: Mutex::new(None),
                checkpoint: std::sync::OnceLock::new(),
            });
            c.f.source
                .retain_workflow_binding(
                    &data.original,
                    &selection.correlation,
                    Some(&data.planning),
                    Some(&candidate),
                    Some(&c.work),
                )
                .unwrap();
        }
        let mut selection = data.envelope.recipient.clone();
        selection.correlation = id("dormant-over-capacity");
        let overflow = Arc::new(InstalledWorkflowContextTransfer {
            source: Arc::downgrade(&c.f.source),
            original: Arc::downgrade(&data.original.original),
            planning: Arc::downgrade(&data.planning),
            service: data.recipient.clone(),
            selected: selection.clone(),
            attempt: Mutex::new(None),
            checkpoint: std::sync::OnceLock::new(),
        });
        assert!(matches!(
            c.f.source.retain_workflow_binding(
                &data.original,
                &selection.correlation,
                Some(&data.planning),
                Some(&overflow),
                Some(&c.work)
            ),
            Err(PrivateOriginalError::Unavailable)
        ));
        assert_eq!(data.planning.context_transfers.lock().unwrap().len(), 128);
        data.planning
            .depart_waiter(&data.envelope.membership)
            .await
            .unwrap();
        assert!(
            !c.f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(data.sibling.workflow())
                .await
                .unwrap()
                .departed(&data.sibling)
                .unwrap()
        );
        let WorkflowContextObservation::Committed(context) =
            restore(&c, &data, &lifecycle).await.unwrap()
        else {
            panic!("exact committed store observation")
        };
        assert!(!context.attempt.locally_entered());
        assert!(
            context.transfer.original.upgrade().is_none(),
            "donor native custody was not required"
        );
        let (_, stamp) = c.f.source.owner.publication().observe(|| ()).unwrap();
        let WorkflowContextObservation::Committed(replay) =
            restore(&c, &data, &lifecycle).await.unwrap()
        else {
            panic!("exact replay")
        };
        stamp.check_current().unwrap();
        assert!(
            Arc::ptr_eq(&context.transfer, &replay.transfer)
                && Arc::ptr_eq(&context.attempt, &replay.attempt)
        );
        assert_eq!(
            data.planning.context_transfers.lock().unwrap().len(),
            129,
            "one exact committed owner remains independently bounded by canonical membership"
        );
        assert_eq!(c.f.source.workflow_context_transfers().unwrap().len(), 1);
        assert_eq!(
            context.deliver().await.unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert_eq!(
            context.transfer.advance_delivery().await.unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert_eq!(
            context.transfer.reconcile_delivery().await.unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert!(context.attempt.delivery.lock().unwrap().is_none());
        assert!(
            context
                .attempt
                .advance
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .finished(),
            "historical reads use the existing joined advance owner without native dispatch custody"
        );
        assert_eq!(
            c.f.store.get_effect(&effect_key).await.unwrap().unwrap(),
            effect
        );
        assert_eq!(native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), model_requests);
        let observed = data
            .recipient
            .store()
            .observe_item_admission(
                data.recipient.default_realm_id(),
                data.recipient.default_namespace(),
                &admission_key(&data.envelope.membership).unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.0, item);
        data.planning.depart_waiter(&data.sibling).await.unwrap();
        assert!(matches!(
            restore(&c, &data, &lifecycle).await,
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            context.transfer.observe().await,
            Err(PrivateOriginalError::Denied)
        ));
    })
    .catch_unwind()
    .await;
    close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_restore_refuses_changed_private_selection_and_stopped_registration() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let data = prepare(&c).await;
        let before = native_rows(&c).await;
        let item = store_envelope(&data).await;
        let control = lifecycle.transfer_control();
        assert!(matches!(
            data.planning
                .restore_committed_context(
                    &c.f.source,
                    &data.envelope.membership,
                    data.recipient.clone(),
                    &c.backend,
                    [id("foreign")].into(),
                    &control
                )
                .await,
            Err(PrivateOriginalError::Conflict)
        ));
        let foreign = WorkGraphService::with_scope(
            data.recipient.store().clone(),
            c.terms.realm.to_string(),
            WorkNamespace::new("foreign-recipient").unwrap(),
        );
        assert!(matches!(
            data.planning
                .restore_committed_context(
                    &c.f.source,
                    &data.envelope.membership,
                    foreign,
                    &c.backend,
                    [id("donor")].into(),
                    &control
                )
                .await,
            Err(PrivateOriginalError::Denied)
        ));
        let mut changed = serde_json::to_value(&data.envelope.membership).unwrap();
        changed["identity"]["deadline_ms"] =
            serde_json::json!(data.envelope.membership.deadline_ms() - 1);
        let changed: SetupWorkflowWaiter = serde_json::from_value(changed).unwrap();
        assert!(matches!(
            data.planning
                .restore_committed_context(
                    &c.f.source,
                    &changed,
                    data.recipient.clone(),
                    &c.backend,
                    [id("donor")].into(),
                    &control
                )
                .await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());
        // An edited WorkGraph item cannot replace its immutable admission.
        let update: UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "id":item.id,"realm_id":item.realm_id,"namespace":item.namespace,
            "expected_revision":item.revision,"description":"{}"
        }))
        .unwrap();
        data.recipient.update(update).await.unwrap();
        assert!(matches!(
            restore(&c, &data, &lifecycle).await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());
        lifecycle.stop();
        assert!(matches!(
            restore(&c, &data, &lifecycle).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());
        assert_eq!(native_rows(&c).await, before);
    })
    .catch_unwind()
    .await;
    close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_read_cannot_register_after_stop_or_recipient_close() {
    for stop in [true, false] {
        let c = Fixture::persistent_configured_waiter().await;
        let lifecycle = AcquisitionRecovery::default();
        let body = AssertUnwindSafe(async {
            let data = prepare(&c).await;
            store_envelope(&data).await;
            let before = native_rows(&c).await;
            let pause = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
            let source = c.f.source.clone();
            let planning = data.planning.clone();
            let membership = data.envelope.membership.clone();
            let recipient = data.recipient.clone();
            let backend = c.backend.clone();
            let audiences = data.envelope.recipient.output_audiences.clone();
            let control = lifecycle.transfer_control();
            let task_pause = pause.clone();
            let task = tokio::spawn(async move {
                restore::AFTER_COMMITTED_READ
                    .scope(
                        task_pause,
                        planning.restore_committed_context(
                            &source,
                            &membership,
                            recipient,
                            &backend,
                            audiences,
                            &control,
                        ),
                    )
                    .await
            });
            // Retain and release the only entered caller on every assertion exit.
            struct Release(Arc<(tokio::sync::Notify, tokio::sync::Notify)>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0.1.notify_one();
                }
            }
            let release = Release(pause.clone());
            let checked = AssertUnwindSafe(async {
                tokio::time::timeout(WAIT, pause.0.notified())
                    .await
                    .unwrap();
                assert!(data.planning.context_transfers.lock().unwrap().is_empty());
                if stop {
                    lifecycle.stop();
                } else {
                    data.planning
                        .depart_waiter(&data.envelope.membership)
                        .await
                        .unwrap();
                    data.planning.depart_waiter(&data.sibling).await.unwrap();
                }
            })
            .catch_unwind()
            .await;
            drop(release);
            let result = task.await.unwrap();
            if let Err(panic) = checked {
                std::panic::resume_unwind(panic);
            }
            if stop {
                assert!(matches!(result, Err(PrivateOriginalError::Unavailable)));
            } else {
                assert!(matches!(result, Err(PrivateOriginalError::Denied)));
            }
            assert!(data.planning.context_transfers.lock().unwrap().is_empty());
            assert_eq!(native_rows(&c).await, before);
        })
        .catch_unwind()
        .await;
        close(&c, &lifecycle, body).await;
    }
}

#[path = "publication_wait_tests.rs"]
mod publication_wait_tests;
