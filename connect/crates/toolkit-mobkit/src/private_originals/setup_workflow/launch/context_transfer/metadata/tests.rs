//! Actual persisted native admission followed by retirement and a new host/store
//! composition with no roster. The committed envelope is controlled setup;
//! this does not claim a Find commit, native Ready, or automatic SDK acceptance.
use super::super::{delivery::historical::tests as history_fixture, restore_tests as fixture};
use super::*;
use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::MetadataDedicated;
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use meerkat_core::{SessionServiceHistoryExt, service::SessionHistoryQuery};
use std::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use toolkit_store::ToolkitStore;

fn selector(calls: Arc<AtomicUsize>) -> InstalledWorkflowContextSelector {
    Arc::new(move |workflow, _, backend| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(InstalledWorkflowContextRead::for_workflow(
            backend,
            workflow,
            [Id::new("donor").unwrap()].into(),
        )?))
    })
}
async fn rows(c: &MetadataDedicated) -> serde_json::Value {
    let mut rows = Vec::new();
    for session in &c.sessions {
        let states = c
            .history
            .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
            .await
            .unwrap();
        let history = c
            .service
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
        rows.push(serde_json::json!({"ids":states.iter().map(|row| &row.state.input_id).collect::<Vec<_>>(), "history":history.messages}));
    }
    serde_json::json!(rows)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_host_page_acknowledges_retired_g_without_revival_or_a_retained_transfer() {
    // No per-workflow list is captured before bootstrap. Canonical terms arise
    // later and the same selected callback supplies their exact private reader.
    let calls = Arc::new(AtomicUsize::new(0));
    let selected = selector(calls.clone());
    let c = fixture::Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let old_source = Arc::downgrade(&c.f.source);
    let old_machine = Arc::downgrade(&c.f.machine);
    let saved = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let context = history_fixture::restored(&c, &data, &lifecycle).await;
        let (_, actual) = history_fixture::admitted_but_unacknowledged(&c, &context).await;
        data.planning
            .depart_waiter(&data.envelope.membership)
            .await
            .unwrap();
        data.planning.depart_waiter(&data.sibling).await.unwrap();
        let runtime = c.f.source.bound_identity_runtime().unwrap();
        let identity = runtime
            .owned_identity_for_member_alias(&c.terms.lead.member)
            .await
            .unwrap();
        runtime.retire(&identity).await.unwrap();
        assert!(
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .is_none()
        );
        let key = data.envelope.membership.delivery_effect().unwrap();
        let uncertain = c.f.store.get_effect(&key).await.unwrap().unwrap();
        assert_eq!(uncertain.status, EffectStatus::Uncertain);
        (
            c.durable_state(),
            data.envelope.membership.source().clone(),
            key,
            actual,
            uncertain,
            fixture::native_rows(&c).await,
        )
    })
    .catch_unwind()
    .await;
    lifecycle.stop();
    let transfer_join = c.f.source.join_workflow_context_transfers(false).await;
    let closed = c.close().await;
    let cleanup = std::panic::catch_unwind(|| {
        assert!(
            transfer_join.is_ok() && closed.is_ok(),
            "joined original owners: {transfer_join:?}/{closed:?}"
        );
    });
    drop(c);
    let (saved, source_key, key, actual, uncertain, before) = match saved {
        Ok(saved) => {
            if let Err(panic) = cleanup {
                std::panic::resume_unwind(panic);
            }
            saved
        }
        Err(panic) => std::panic::resume_unwind(panic),
    };
    assert!(old_source.upgrade().is_none() && old_machine.upgrade().is_none());
    let fresh = saved.reopen_metadata().await;
    let body = AssertUnwindSafe(async {
        assert!(fresh.source.bound_identity_runtime().is_err());
        assert!(
            fresh
                .source
                .workflow_context_transfers()
                .unwrap()
                .is_empty()
        );
        for session in &fresh.sessions {
            assert!(
                fresh
                    .machine
                    .current_executor_attachment_witness(session)
                    .await
                    .is_none()
            );
        }
        assert_eq!(rows(&fresh).await, before);
        assert_eq!(fresh.model_requests(), 0);
        let mut after = None;
        let mut selected_row = None;
        loop {
            let page = fresh
                .source
                .workflow_close_recovery_page(after.as_deref(), 1)
                .await
                .unwrap();
            for row in page.sources {
                if row.key == source_key {
                    selected_row = Some(row);
                }
            }
            after = page.next_after_id;
            if after.is_none() {
                break;
            }
        }
        let row = selected_row.expect("same canonical source page");
        let declined: InstalledWorkflowContextSelector = Arc::new(|_, _, _| Ok(None));
        let unavailable: InstalledWorkflowContextSelector =
            Arc::new(|_, _, _| Err(HostLifetimeError::Unavailable));
        assert_eq!(
            fresh
                .source
                .recover_workflow_context_source(&row, &fresh.backend, &declined, || true)
                .await
                .unwrap(),
            WorkflowContextRecovery::NotSelected
        );
        assert!(matches!(
            fresh
                .source
                .recover_workflow_context_source(&row, &fresh.backend, &unavailable, || true)
                .await,
            Err(PrivateOriginalError::Unavailable)
        ));
        let foreign: InstalledWorkflowContextSelector = Arc::new(|_, _, backend| {
            Ok(Some(InstalledWorkflowContextRead {
                recipient: backend.service().clone(),
                output_audiences: [Id::new("donor").unwrap()].into(),
            }))
        });
        assert!(matches!(
            fresh
                .source
                .recover_workflow_context_source(&row, &fresh.backend, &foreign, || true)
                .await,
            Err(PrivateOriginalError::Denied)
        ));
        assert_eq!(
            fresh.source.store.get_effect(&key).await.unwrap().unwrap(),
            uncertain
        );
        let (_, stamp) = fresh.source.owner.publication().observe(|| ()).unwrap();
        assert_eq!(
            fresh
                .source
                .recover_workflow_context_source(&row, &fresh.backend, &selected, || true)
                .await
                .unwrap(),
            WorkflowContextRecovery::Observed {
                admitted: 1,
                pending: 1
            }
        );
        let applied = fresh.source.store.get_effect(&key).await.unwrap().unwrap();
        assert_eq!(applied.status, EffectStatus::Succeeded);
        assert_eq!(applied.attempt, uncertain.attempt);
        assert_eq!(applied.intent, uncertain.intent);
        let Some(EffectOutcome::Succeeded { receipt }) = &applied.outcome else {
            panic!("Applied metadata")
        };
        assert_eq!(
            receipt.body["input"],
            serde_json::to_value(&actual.input_id).unwrap()
        );
        assert_eq!(
            receipt.body["session"],
            serde_json::to_value(&actual.session_id).unwrap()
        );
        assert_eq!(
            fresh
                .source
                .recover_workflow_context_source(&row, &fresh.backend, &selected, || true)
                .await
                .unwrap(),
            WorkflowContextRecovery::Observed {
                admitted: 1,
                pending: 1
            }
        );
        assert_eq!(
            fresh.source.store.get_effect(&key).await.unwrap().unwrap(),
            applied
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        stamp.check_current().unwrap();
        assert!(
            fresh
                .source
                .workflow_context_transfers()
                .unwrap()
                .is_empty()
        );
        assert!(fresh.source.retained.lock().unwrap().is_empty());
        assert_eq!(rows(&fresh).await, before);
        assert_eq!(fresh.model_requests(), 0);
    })
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(fresh.close()).catch_unwind().await;
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}

// A second actual configured source on the same retained A session. It uses
// the existing submission/grant owner and canonical join, not copied rows or
// synthetic input admission. Both donors select the same fixed output audience.
pub(super) async fn second_source(
    c: &fixture::Fixture,
    planning: &Arc<InstalledWorkflowPlanning>,
) -> fixture::Data {
    let content = "second metadata source";
    let mut event = crate::original_ingress::tests::private_event("metadata-second-source");
    event.route = c.f.original_reference().route.clone();
    event.content_sha256 = digest(content.as_bytes());
    let authentication = event.original_authentication.clone();
    let reference = c.f.source.retain_event(event).await.unwrap();
    c.member.prepare_waiter_source(&reference).await.unwrap();
    let probe = c.f.probe.as_ref().unwrap();
    probe.arm_next_input();
    let submitted =
        c.f.source
            .submit_authenticated_original(crate::private_originals::PrivateOriginalSubmit {
                original: reference.clone(),
                content: content.into(),
                transport_authentication: authentication,
            })
            .await
            .map_err(|error| error.to_string());
    c.f.finish(&submitted).await.unwrap();
    let dispatch = probe.captured();
    assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
    let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
    assert!(original.reference == reference);
    let scope =
        serde_json::to_value(dispatch.work_authorization().unwrap().execution_scope()).unwrap();
    let call = "controlled-second-metadata-envelope";
    let correlation = crate::installed_host::acquisition::installed_invocation_correlation(
        &reference, &scope, call,
    )
    .unwrap();
    let (membership, _, acquisition) = super::super::super::tests::join_waiter_context(
        &c.f,
        planning,
        &original,
        &dispatch,
        &c.backend,
        &c.work,
        "donor",
        None,
        Some(correlation.clone()),
    )
    .await;
    let recipient = WorkGraphService::with_scope(
        c.backend.service().store().clone(),
        c.terms.realm.to_string(),
        planning.context_namespace().unwrap(),
    );
    let selected = TransferSelection {
        original: reference,
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
        output_audiences: [Id::new("donor").unwrap()].into(),
    };
    let envelope = TransferEnvelope {
        purpose: PURPOSE.into(),
        recipient: selected,
        membership: membership.clone(),
        input_digest: acquisition.request().input_digest().clone(),
        find_call: call.into(),
        find_scope: scope,
        request: acquisition.request().request().clone(),
        deadline_ms: acquisition.request().host().limits.deadline_ms,
    };
    fixture::Data {
        planning: planning.clone(),
        envelope,
        original,
        recipient,
        sibling: membership,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_page_read_survives_dropped_polls_and_stop_blocks_acknowledgment() {
    let c = fixture::Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let pause = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        fixture::store_envelope(&data).await;
        let other = second_source(&c, &data.planning).await;
        let other_context = history_fixture::restored(&c, &other, &lifecycle).await;
        let (_, actual) = history_fixture::admitted_but_unacknowledged(&c, &other_context).await;
        let a =
            c.f.store
                .get(data.envelope.membership.source())
                .await
                .unwrap()
                .unwrap();
        let queued = a.clone();
        let other_row =
            c.f.store
                .get(other.envelope.membership.source())
                .await
                .unwrap()
                .unwrap();
        assert!(a.key != other_row.key);
        let other_key = other.envelope.membership.delivery_effect().unwrap();
        let other_before = c.f.store.get_effect(&other_key).await.unwrap().unwrap();
        assert_eq!(other_before.status, EffectStatus::Uncertain);
        let owners = c.f.source.workflow_context_transfers().unwrap();
        assert_eq!(owners.len(), 1);
        let key = data.envelope.membership.delivery_effect().unwrap();
        let before = c.f.store.get_effect(&key).await.unwrap().unwrap();
        let native_before = fixture::native_rows(&c).await;
        let requests = c.f.model_requests();
        let calls = Arc::new(AtomicUsize::new(0));
        let selected = selector(calls.clone());
        let open = Arc::new(AtomicBool::new(true));
        type RecoveryFuture = std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<WorkflowContextRecovery, PrivateOriginalError>,
                    > + Send,
            >,
        >;
        let mut pending: FuturesUnordered<RecoveryFuture> = FuturesUnordered::new();
        let (source, backend, selection, task_pause, task_open) = (
            c.f.source.clone(),
            c.backend.clone(),
            selected.clone(),
            pause.clone(),
            open.clone(),
        );
        pending.push(Box::pin(async move {
            AFTER_PRIVATE_READ
                .scope(
                    task_pause,
                    source.recover_workflow_context_source(&a, &backend, &selection, || {
                        task_open.load(Ordering::SeqCst)
                    }),
                )
                .await
        }));
        let (source, backend, selection) =
            (c.f.source.clone(), c.backend.clone(), selected.clone());
        pending.push(Box::pin(async move {
            source
                .recover_workflow_context_source(&other_row, &backend, &selection, || true)
                .await
        }));
        assert_eq!(
            tokio::time::timeout(fixture::WAIT, pending.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WorkflowContextRecovery::Observed {
                admitted: 1,
                pending: 0
            },
            "another selected source reaches actual native history and Applied while A read is held"
        );
        let other_applied = c.f.store.get_effect(&other_key).await.unwrap().unwrap();
        assert_eq!(other_applied.status, EffectStatus::Succeeded);
        assert_eq!(other_applied.attempt, other_before.attempt);
        let Some(EffectOutcome::Succeeded { receipt }) = &other_applied.outcome else {
            panic!("Applied metadata")
        };
        assert_eq!(
            receipt.body["input"],
            serde_json::to_value(&actual.input_id).unwrap()
        );
        tokio::time::timeout(fixture::WAIT, async {
            tokio::select! {
                _ = pause.0.notified() => (),
                result = pending.next() => panic!("held source returned early: {result:?}"),
            }
        })
        .await
        .unwrap();
        for _ in 0..2 {
            assert!(
                tokio::time::timeout(Duration::from_millis(25), pending.next())
                    .await
                    .is_err()
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                2,
                "same two selected reads, no reconstructed read"
            );
        }
        open.store(false, Ordering::SeqCst);
        // A not-yet-polled source in the final drain must see stop before any
        // selector or private read, even though its future was already queued.
        let (source, backend, selection, task_open) = (
            c.f.source.clone(),
            c.backend.clone(),
            selected.clone(),
            open.clone(),
        );
        pending.push(Box::pin(async move {
            source
                .recover_workflow_context_source(&queued, &backend, &selection, || {
                    task_open.load(Ordering::SeqCst)
                })
                .await
        }));
        pause.1.notify_one();
        assert_eq!(
            tokio::time::timeout(fixture::WAIT, pending.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WorkflowContextRecovery::Stopped
        );
        assert_eq!(
            tokio::time::timeout(fixture::WAIT, pending.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WorkflowContextRecovery::Stopped
        );
        assert!(pending.is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "stopped queued source never enters a read"
        );
        assert_eq!(c.f.store.get_effect(&key).await.unwrap().unwrap(), before);
        assert_eq!(
            c.f.store.get_effect(&other_key).await.unwrap().unwrap(),
            other_applied
        );
        assert_eq!(fixture::native_rows(&c).await, native_before);
        assert_eq!(c.f.model_requests(), requests);
        let after = c.f.source.workflow_context_transfers().unwrap();
        assert_eq!(after.len(), owners.len());
        assert!(after.iter().zip(&owners).all(|(a, b)| Arc::ptr_eq(a, b)));
    })
    .catch_unwind()
    .await;
    pause.1.notify_one();
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_page_rereads_opt_in_and_refuses_changed_envelopes_without_publication() {
    let c = fixture::Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let item = fixture::store_envelope(&data).await;
        let row =
            c.f.store
                .get(data.envelope.membership.source())
                .await
                .unwrap()
                .unwrap();
        let effect =
            c.f.store
                .get_effect(&data.envelope.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
        let before = fixture::native_rows(&c).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let selected = selector(calls.clone());
        let publication = c.f.source.owner.publication();
        for malformed in [false, true] {
            let (_, before_mutation) = publication.observe(|| ()).unwrap();
            let current = c.f.store.get(&row.key).await.unwrap().unwrap();
            let mut changed = row.data.clone();
            if malformed {
                changed.body["planning_source_evidence"] = serde_json::json!({"unrecognized":true});
            } else {
                changed
                    .body
                    .as_object_mut()
                    .unwrap()
                    .remove("planning_source_evidence");
            }
            c.f.store
                .compare_and_swap(toolkit_store::Transition {
                    key: current.key,
                    expected_revision: current.revision,
                    expected_digest: Some(current.content_digest),
                    claim: None,
                    data: changed,
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
            assert!(
                matches!(
                    before_mutation.check_current(),
                    Err(meerkat_authorization::publication::PublicationError::Changed)
                ),
                "controlled source mutation publishes its change"
            );
            let (_, stamp) = publication.observe(|| ()).unwrap();
            let result =
                c.f.source
                    .recover_workflow_context_source(&row, &c.backend, &selected, || true)
                    .await;
            if malformed {
                assert_eq!(result, Err(PrivateOriginalError::Conflict));
            } else {
                assert_eq!(result, Ok(WorkflowContextRecovery::NotSelected));
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "invalid current row does not reach selector"
            );
            stamp
                .check_current()
                .expect("recovery refusal does not publish");
        }
        let (_, before_restore) = publication.observe(|| ()).unwrap();
        let current = c.f.store.get(&row.key).await.unwrap().unwrap();
        c.f.store
            .compare_and_swap(toolkit_store::Transition {
                key: current.key,
                expected_revision: current.revision,
                expected_digest: Some(current.content_digest),
                claim: None,
                data: row.data.clone(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        assert!(
            matches!(
                before_restore.check_current(),
                Err(meerkat_authorization::publication::PublicationError::Changed)
            ),
            "controlled source restoration publishes its change"
        );
        let changed: meerkat_workgraph::UpdateWorkItemRequest =
            serde_json::from_value(serde_json::json!({
                "id":item.id,"realm_id":item.realm_id,"namespace":item.namespace,
                "expected_revision":item.revision,"description":"{}"
            }))
            .unwrap();
        data.recipient.update(changed).await.unwrap();
        let (_, stamp) = publication.observe(|| ()).unwrap();
        assert!(matches!(
            c.f.source
                .recover_workflow_context_source(&row, &c.backend, &selected, || true)
                .await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert_eq!(
            c.f.store.get_effect(&effect.key).await.unwrap().unwrap(),
            effect
        );
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());
        stamp.check_current().unwrap();
        assert_eq!(fixture::native_rows(&c).await, before);
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}
