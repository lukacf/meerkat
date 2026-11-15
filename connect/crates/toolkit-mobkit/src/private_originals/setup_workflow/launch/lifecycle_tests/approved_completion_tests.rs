//! Real native A/P/G and an approved service continuation. Model IO and the
//! protected decision use the existing fixture. No physical MCP installation,
//! second caller's native dispatch or installed acceptance is claimed here.
use super::super::idle_restore_tests::finish;
use super::super::waiter_restore_tests::PersistentDedicated;
use super::*;
use meerkat_core::ToolCallView;
use meerkat_runtime::SessionServiceRuntimeExt;

async fn completion_case(cancel: bool) {
    let c = PersistentDedicated::persistent_effect_waiter().await;
    let f = &c.f;
    let result = AssertUnwindSafe(Box::pin(tokio::time::timeout(GUARD, async {
        let source = &f.source;
        c.member
            .prepare_effect_waiter_source(f.original_reference())
            .await
            .unwrap();
        source
            .prepare_workflow_parent_source(&c.parent.original)
            .await
            .unwrap();
        let p = submit_planning_parent(source, &c.parent).await;
        finish(&c, "planning_parent", &p).await;
        f.probe.as_ref().unwrap().arm_next_input();
        let admitted_a = f.submit().await;
        f.finish(&admitted_a).await.unwrap();
        let dispatch = f.probe.as_ref().unwrap().captured();
        let a = source.acquisition_original(&dispatch).await.unwrap();
        let planning = prepare_planning(source, &c.parent.original, &c.lead, &c.terms, &c.lineage)
            .await
            .unwrap();
        let (_, selected_member, _) = source.selected_source_route(&a.reference.route).unwrap();
        let (membership, snapshot, acquisition) = join_waiter_context_with_member(
            f,
            &planning,
            &a,
            &dispatch,
            &c.backend,
            &c.work,
            "approved-cohort",
            None,
            None,
            Some(selected_member.member.clone()),
        )
        .await;
        source
            .retain_workflow_selection(
                &a,
                &acquisition.request().host().correlation,
                Some(&planning),
                Some(&c.work),
            )
            .unwrap();
        let g = native(planning.launch(&snapshot).await.unwrap());
        finish(&c, "planning_lead", &g).await;
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .unwrap()
            .clone();
        let workflow = held.setup_workflow.get().unwrap();
        let waiter = workflow.waiters.lock().unwrap().first().unwrap().clone();
        // The existing shared profile selected and issued this exact target
        // before A admission. This staged call claims no native tool effect.
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = waiter
            .stage_call(ToolCallView {
                id: "before-ready",
                name: "read",
                args: &args,
            })
            .unwrap();
        call.settle_unentered().unwrap();
        call.current_relationship().unwrap();

        let service =
            crate::private_originals::service_approved_input::approve_workflow_service_for_test(
                f,
                acquisition.clone(),
            )
            .await;
        let row = f
            .store
            .get(&source.key(&service).unwrap())
            .await
            .unwrap()
            .unwrap();
        let hold = f.hold_next_response();
        source.step_service_input(&row).await.unwrap();
        hold.entered().await;
        let admitted = f.store.get(&row.key).await.unwrap().unwrap();
        let record = source.decode_record(&admitted, &service).unwrap();
        let terminal = source.service_task_snapshot(&record).await.unwrap();
        let terminal_row = f.store.get(terminal.key()).await.unwrap().unwrap();
        let setup = source.workflow_store().unwrap();
        let obligation = setup
            .committed_continuation(&id(&terminal.key().id))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            obligation.outcome(),
            SetupTerminalOutcome::ApprovedServiceAdmitted { .. }
        ));
        assert!(
            !source
                .approved_service_continuation_finalized(&terminal, &obligation)
                .await
                .unwrap()
        );
        let candidate = terminal.continuation_recovery_candidate().unwrap();
        assert!(matches!(
            source
                .reconcile_terminal_workflow_acquisition(&candidate)
                .await
                .unwrap(),
            WorkflowTerminalRecovery::ContinuationPending
        ));
        assert!(
            call.current_relationship().is_err(),
            "terminal setup authority is retired while continuation is live"
        );
        assert!(
            waiter
                .stage_call(ToolCallView {
                    id: "after-ready",
                    name: "read",
                    args: &args
                })
                .is_err()
        );
        let pending = setup
            .load_setup_workflow(membership.workflow())
            .await
            .unwrap();
        assert!(
            !pending.sealed()
                && pending.active_waiters() == 1
                && !pending.departed(&membership).unwrap()
        );
        assert!(
            f.store
                .get_effect(&membership.workflow().close_effect())
                .await
                .unwrap()
                .is_none()
        );

        // A separate canonical acquisition can join while the actual service
        // input is held. Its native caller independence remains the BOTH test.
        let child = c
            .backend
            .service()
            .create(CreateWorkItemRequest {
                title: "late-during-service".into(),
                labels: [c.terms.cycle_label.clone()].into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let child = c
            .backend
            .service()
            .claim(ClaimWorkItemRequest {
                id: child.id,
                realm_id: None,
                namespace: None,
                expected_revision: child.revision,
                owner: WorkOwner {
                    key: WorkOwnerKey::agent("late-during-service").unwrap(),
                    display_name: None,
                },
                lease_seconds: Some(300),
                lease_expires_at: None,
            })
            .await
            .unwrap();
        c.backend
            .service()
            .link(LinkWorkItemsRequest {
                realm_id: None,
                namespace: None,
                kind: WorkEdgeKind::Parent,
                from_id: child.id.clone(),
                to_id: c.work.parent().item_id.clone(),
            })
            .await
            .unwrap();
        let work = SetupWorkContext::from_owner(
            c.backend.pin().clone(),
            c.work.parent().clone(),
            work_ref(&child),
            c.terms.parent_owner.clone(),
            child.owner.as_ref().unwrap().key.clone(),
            c.terms.cycle_label.clone(),
        )
        .unwrap();
        let (late, joined) = join_waiter(
            f,
            &planning,
            &a,
            &dispatch,
            &c.backend,
            &work,
            "late-during-service",
        )
        .await;
        assert_eq!(joined.active_waiters(), 2);
        assert_ne!(late.acquisition(), membership.acquisition());
        let after_late = planning.depart_waiter(&late).await.unwrap();
        assert!(!after_late.sealed() && after_late.active_waiters() == 1);

        let mut unacknowledged = record.clone();
        unacknowledged.service_admission = None;
        assert!(
            !source
                .service_input_finalized(&service, unacknowledged)
                .await
                .unwrap(),
            "absent canonical admission is not completion"
        );
        let mut wrong = record.clone();
        wrong.service_admission.as_mut().unwrap().input_id = acquisition
            .request()
            .host()
            .continuation
            .revision
            .to_string();
        assert!(
            matches!(
                source.service_input_finalized(&service, wrong).await,
                Err(PrivateOriginalError::Conflict)
            ),
            "a different actual native input cannot discharge this service"
        );
        let bound = record
            .bound_service_input
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        let native = f
            .machine
            .input_state(&f.session, bound.id())
            .await
            .unwrap()
            .unwrap();
        crate::private_originals::service_input_receipt::verify_service_input_state(
            &bound,
            &native.state,
        )
        .unwrap();
        let mut wrong_native = native.state.clone();
        wrong_native.input_id = meerkat_core::lifecycle::InputId::from_uuid(
            acquisition
                .request()
                .host()
                .continuation
                .revision
                .as_str()
                .parse()
                .unwrap(),
        );
        assert!(
            matches!(
                crate::private_originals::service_input_receipt::verify_service_input_state(
                    &bound,
                    &wrong_native
                ),
                Err(PrivateOriginalError::Conflict)
            ),
            "same prompt metadata under another native input ID must refuse"
        );
        let mut foreign = service.clone();
        foreign.original = EvidenceId::new("missing-service-source").unwrap();
        assert!(
            source
                .service_input_finalized(&foreign, record.clone())
                .await
                .is_err(),
            "an unavailable immutable source recheck cannot become completion"
        );
        assert!(
            !setup
                .load_setup_workflow(membership.workflow())
                .await
                .unwrap()
                .sealed()
        );

        if cancel {
            source
                .cancel_original_operation(&a.reference)
                .await
                .unwrap();
            let cancelled = setup
                .load_setup_workflow(membership.workflow())
                .await
                .unwrap();
            assert!(cancelled.sealed() && cancelled.active_waiters() == 0);
            assert!(
                matches!(
                    source
                        .reconcile_terminal_workflow_acquisition(&candidate)
                        .await
                        .unwrap(),
                    WorkflowTerminalRecovery::Departed | WorkflowTerminalRecovery::CleanupPending
                ),
                "explicit durable departure is not blocked by the service completion gate"
            );
        }
        hold.release();
        loop {
            if source
                .approved_service_continuation_finalized(&terminal, &obligation)
                .await
                .unwrap()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            source
                .reconcile_terminal_workflow_acquisition(&candidate)
                .await
                .unwrap(),
            WorkflowTerminalRecovery::Departed | WorkflowTerminalRecovery::CleanupPending
        ));
        let completed = setup
            .load_setup_workflow(membership.workflow())
            .await
            .unwrap();
        assert!(
            completed.sealed()
                && completed.active_waiters() == 0
                && completed.departed(&membership).unwrap()
        );
        assert!(
            f.store
                .get_effect(&membership.workflow().close_effect())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            call.current_relationship().is_err(),
            "finalization never reopens setup authority"
        );
        assert_eq!(
            f.store.get(terminal.key()).await.unwrap().unwrap(),
            terminal_row
        );
        let ids = super::super::waiter_restore_tests::all_ids(&c).await;
        assert_eq!(
            ids.iter().map(Vec::len).sum::<usize>(),
            4,
            "only actual A/P/G and the one approved service input"
        );
    })))
    .catch_unwind()
    .await;
    let cleanup = c.close().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded service cohort completion"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approved_service_keeps_cohort_until_exact_native_finalization() {
    completion_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_cancellation_departs_approved_service_without_waiting_for_completion() {
    completion_case(true).await;
}
