//! Actual idle close/ordinary resume, historical source custody and a new B.
//! The static G fixture does not qualify dynamic class or physical MCP recovery.
use super::waiter_restore_tests::join_recovery_for_idle_restore;
use super::*;
use crate::private_originals::governed_submit::tests::first_input_tests::dedicated::{
    DedicatedFixture, DurableDedicated,
};
use meerkat_core::SessionServiceHistoryExt;
use meerkat_core::service::SessionHistoryQuery;
use meerkat_core::{
    authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, PreparedAuthorizationBinding,
        SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
    },
    exact_operation::OperationExecutionScope,
    lifecycle::InputId,
    memory::MemorySearchScope,
};
use meerkat_mobkit::identity_first::{AgentIdentity, TurnOutcome, TurnOutput, TurnTicket};

type PersistentDedicated =
    DedicatedFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

pub(super) async fn finish(
    c: &PersistentDedicated,
    member: &str,
    input: &PrivateOriginalSubmission,
) {
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = input
    else {
        panic!("actual admission required")
    };
    assert!(matches!(
        c.f.source
            .bound_identity_runtime()
            .unwrap()
            .wait_for_turn(
                &AgentIdentity::parse(member).unwrap(),
                TurnTicket::parse(ticket).unwrap(),
                GUARD,
            )
            .await,
        TurnOutcome::Completed {
            output: TurnOutput::Text { .. }
        }
    ));
}
pub(super) async fn ids(
    c: &PersistentDedicated,
    session: &meerkat_core::SessionId,
) -> Vec<InputId> {
    let rows = c
        .history
        .as_ref()
        .unwrap()
        .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
        .await
        .unwrap();
    assert!(rows.len() <= 3, "bounded exact fixture input history");
    let mut ids: Vec<_> = rows.into_iter().map(|row| row.state.input_id).collect();
    ids.sort_by_key(|id| id.to_string());
    assert!(ids.windows(2).all(|pair| pair[0] != pair[1]));
    ids
}
async fn history(c: &PersistentDedicated) -> serde_json::Value {
    let rows =
        c.f.service
            .read_history(
                &c.lead_session,
                SessionHistoryQuery {
                    offset: 0,
                    limit: Some(100),
                },
            )
            .await
            .unwrap();
    assert!(!rows.has_more);
    serde_json::to_value(rows.messages).unwrap()
}
pub(super) async fn close(
    c: &PersistentDedicated,
    body: Result<(), Box<dyn std::any::Any + Send>>,
) {
    let cleanup = AssertUnwindSafe(async {
        let result = c.close().await;
        assert!(result.is_ok(), "{result:?}");
    })
    .catch_unwind()
    .await;
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}
struct Saved {
    durable: DurableDedicated,
    association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    row: toolkit_store::StoredRecord,
    membership: SetupWorkflowWaiter,
    ids: Vec<InputId>,
    history: serde_json::Value,
}
async fn admit_then_close() -> Saved {
    let c = PersistentDedicated::persistent().await;
    let body = AssertUnwindSafe(Box::pin(async {
        c.f.source.prepare_workflow_parent_source(&c.parent.original).await.unwrap();
        let p = c.f.source.submit_authenticated_original(c.parent.clone()).await.unwrap();
        finish(&c, "planning_parent", &p).await;
        let probe = c.f.probe.as_ref().unwrap();
        assert_eq!(probe.captured().origin_session_id(), Some(&c.parent_session));
        // The shared provider captured P. Its joined turn permits the existing
        // probe to capture the next actual input, A, without changing authority.
        probe.arm_next_input();
        let a = c.f.submit().await;
        c.f.finish(&a).await.unwrap();
        let dispatch = probe.captured();
        assert_eq!(dispatch.origin_session_id(), Some(&c.f.session));
        let original = c.f.source.acquisition_original(&dispatch).await.unwrap();
        assert_eq!(original.source_record.key, c.f.source.key(c.f.original_reference()).unwrap());
        assert_eq!(original.original.association(), &c.f.binding().await.unwrap().unwrap());
        let planning = c.f.source.prepare_workflow_planning(&c.parent.original,
            c.lead.clone(), c.terms.clone(), c.lineage.clone()).await.unwrap();
        let (membership, snapshot) = join_waiter(&c.f, &planning, &original, &dispatch, &c.backend, &c.work, "restore-waiter").await;
        let admitted = native(planning.launch(&snapshot).await.unwrap());
        finish(&c, "planning_lead", &admitted).await;
        let PrivateOriginalSubmission::Admitted { input_id, .. } = admitted else { panic!("G admission") };
        let actual_id = InputId::from_uuid(input_id.parse().unwrap());
        let association = c.f.source.binding_evidence(&c.lead.original).await.unwrap().unwrap();
        let row = c.f.store.get(&c.f.source.key(&c.lead.original).unwrap()).await.unwrap().unwrap();
        let record = c.f.source.decode_record(&row, &c.lead.original).unwrap();
        assert!(matches!(&record.planning_source_evidence,
            Some(crate::private_originals::planning_source_evidence::PlanningSourceEvidence::Lead { parent }) if parent == &c.parent.original));
        let input = record.bound_service_input.as_ref().unwrap().decode().unwrap();
        tokio::time::timeout(GUARD, async {
            loop {
                let rows = c.history.as_ref().unwrap().load_input_states_strict(
                    &meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session)).await.unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].state.input_id, actual_id);
                meerkat_runtime::input_state::verify_exact_replay(&rows[0].state, &input).unwrap();
                if rows[0].state.persisted_input.is_none() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("terminal G input body retires before restore");
        let ids = ids(&c, &c.lead_session).await;
        assert_eq!(ids, vec![actual_id]);
        Saved { durable: c.durable_state(), association, row, membership, ids, history: history(&c).await }
    })).catch_unwind().await;
    let cleanup = AssertUnwindSafe(async {
        let result = c.close().await;
        assert!(result.is_ok(), "{result:?}");
    })
    .catch_unwind()
    .await;
    // All first activation owners are dropped. Only paths, immutable selection
    // and evidence survive, as in the existing persistent first-input fixture.
    drop(c);
    match body {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(saved) => {
            if let Err(panic) = cleanup {
                std::panic::resume_unwind(panic);
            }
            saved
        }
    }
}

pub(super) fn assert_unattached_policy_closed(
    c: &PersistentDedicated,
    association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
) {
    // An ungranted policy candidate exercises the actual singleton fallback
    // boundary. It neither enters native work nor creates another input.
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000004\"").unwrap(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            usage: SourceAuthorizationUse::Read,
            target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                c.lead_session.clone(),
            )),
        }),
    });
    assert!(
        matches!(
            c.f.source.workflow_call_for_policy(association, &binding),
            Err(PrivateOriginalError::Unavailable)
        ),
        "unattached G must not select singleton policy"
    );
    assert!(
        matches!(
            c.f.source.owner.check_workflow_member(
                &meerkat_runtime::LogicalRuntimeId::for_session(&c.lead_session),
                association,
                None
            ),
            Err(
                meerkat_runtime::input_authority::NativeAdmissionError::Readiness(
                    meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable
                )
            )
        ),
        "the same incomplete controller keeps native input admission unavailable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_resumed_g_restores_exact_native_custody_without_input() {
    let saved = admit_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        assert_eq!(
            c.f.model_requests(),
            0,
            "idle resume cannot replay the model"
        );
        assert!(c.member.session().is_err());
        assert!(
            !c.f.source
                .owner
                .has_admitted_source(&saved.association)
                .unwrap()
        );
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        c.member.restore_admitted_source().await.unwrap();
        assert!(
            c.member.session().is_err(),
            "historical source is not a completed workflow attachment"
        );
        assert!(
            c.f.source
                .owner
                .has_admitted_source(&saved.association)
                .unwrap()
        );
        assert_eq!(
            c.f.source
                .owner
                .historical_task_custody_for_test(&saved.association),
            Some((true, true))
        );
        assert_unattached_policy_closed(&c, &saved.association);
        let p =
            c.f.source
                .binding_evidence(&c.parent.original)
                .await
                .unwrap()
                .unwrap();
        assert!(c.f.source.owner.has_admitted_source(&p).unwrap());
        assert!(crate::original_ingress::controller_only_basis(
            &p.candidate().authority_basis
        ));
        assert_eq!(
            saved.association.candidate().contributing_work,
            vec![p.candidate().original_work.clone()]
        );
        // Canonical departure does not share the source publication. Historical
        // custody can replay after closure because neither policy gate opens.
        let closed =
            c.f.source
                .workflow_store()
                .unwrap()
                .depart_setup_workflow(&saved.membership)
                .await
                .unwrap();
        assert_eq!(closed.active_waiters(), 0);
        c.member.restore_admitted_source().await.unwrap();
        assert_unattached_policy_closed(&c, &saved.association);
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(c.f.model_requests(), 0);
        assert_eq!(
            c.f.store.get(&saved.row.key).await.unwrap().unwrap(),
            saved.row
        );
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_resumed_g_refuses_foreign_missing_changed_and_cancelled_evidence() {
    let saved = admit_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let workflow =
            c.f.source
                .workflow_store()
                .unwrap()
                .workflow_ref(&c.terms)
                .unwrap();
        let mut foreign = c.lead.original.clone();
        foreign.event_sha256 = "f".repeat(64);
        assert_eq!(
            c.f.source
                .restore_workflow_lead_source(&workflow, &foreign, &c.terms.lead)
                .await,
            Err(PrivateOriginalError::Conflict)
        );
        for changed in [false, true] {
            let row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
            let mut data = saved.row.data.clone();
            if changed {
                data.body["bound_service_input"]["content"] =
                    serde_json::json!("changed G request");
            } else {
                data.body
                    .as_object_mut()
                    .unwrap()
                    .remove("bound_service_input");
            }
            c.f.store
                .compare_and_swap(toolkit_store::Transition {
                    key: row.key,
                    expected_revision: row.revision,
                    expected_digest: Some(row.content_digest),
                    claim: None,
                    data,
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
            assert_eq!(
                c.member.restore_admitted_source().await,
                Err(if changed {
                    PrivateOriginalError::Conflict
                } else {
                    PrivateOriginalError::Unavailable
                })
            );
            assert!(c.member.session().is_err());
            assert!(
                !c.f.source
                    .owner
                    .has_admitted_source(&saved.association)
                    .unwrap()
            );
            let p =
                c.f.source
                    .binding_evidence(&c.parent.original)
                    .await
                    .unwrap()
                    .unwrap();
            assert!(
                !c.f.source.owner.has_admitted_source(&p).unwrap(),
                "invalid G evidence must refuse before promoting P"
            );
        }
        let row = c.f.store.get(&saved.row.key).await.unwrap().unwrap();
        c.f.store
            .compare_and_swap(toolkit_store::Transition {
                key: row.key,
                expected_revision: row.revision,
                expected_digest: Some(row.content_digest),
                claim: None,
                data: saved.row.data.clone(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        c.f.source
            .record_operation_cancelled(&c.lead.original)
            .await
            .unwrap();
        assert_eq!(
            c.member.restore_admitted_source().await,
            Err(PrivateOriginalError::Denied)
        );
        assert!(c.member.session().is_err());
        assert!(
            c.f.source
                .owner
                .historical_task_custody_for_test(&saved.association)
                .is_none()
        );
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settled_lead_restores_empty_attachment_before_independent_b_join() {
    let saved = admit_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let source = &c.f.source;
        // Observe the actual finite cold scan after its reads finish. Unlike
        // stopping the worker, this leaves genuine new B admission available.
        let mut settled = Vec::new();
        for reference in [c.f.original_reference(), &c.parent.original, &c.lead.original] {
            let row = c.f.store.get(&source.key(reference).unwrap()).await.unwrap().unwrap();
            let record = source.decode_record(&row, reference).unwrap();
            assert!(record.custody_attempt.as_ref().is_some_and(|attempt|
                attempt.decision() == crate::original_ingress::custody_attempt::CustodyDecision::Promoted));
            settled.push(row);
        }
        let probe = Arc::new(crate::private_originals::recovery_probe::MetadataRecoveryProbe::for_interest(saved.row.key.clone()));
        probe.release(); // Observe only; never pause native or interest recovery.
        probe.ready.store(true, Ordering::SeqCst);
        assert!(source.metadata_probe.set(probe.clone()).is_ok());
        tokio::time::timeout(GUARD, probe.wait_for(|probe|
            probe.custody_idle_rounds.load(Ordering::SeqCst) > 0)).await
            .expect("actual custody scan must finish without pending or unavailable work");
        for row in settled {
            assert_eq!(c.f.store.get(&row.key).await.unwrap().unwrap(), row,
                "idle observation cannot change already-promoted canonical custody");
        }
        // A keeps the canonical workflow open across Restore. The retained
        // attachment below has no waiters; last departure was never crossed.
        let open = source.workflow_store().unwrap().load_setup_workflow(saved.membership.workflow()).await.unwrap();
        assert!(!open.sealed() && open.active_waiters() == 1);
        assert!(!open.departed(&saved.membership).unwrap());
        assert_eq!(source.recover_workflow_interest_source(&saved.row, || true).await.unwrap(),
            super::recovery::WorkflowInterestRecovery::NotSelected,
            "a legacy Lead without helper provenance cannot infer a cold late-helper selection");
        c.member.restore_admitted_source().await.unwrap();
        assert_unattached_policy_closed(&c, &saved.association);
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&grants_path).unwrap();
        let publication = source.owner.publication();
        let plan = source.restore_workflow_planning_source(saved.membership.workflow(),
            &c.lead.original, &c.terms.lead).await.unwrap();
        let g = source.retained.lock().unwrap().iter()
            .find(|held| held.reference == c.lead.original).unwrap().clone();
        let attached = g.setup_workflow.get().unwrap();
        assert!(attached.waiters.lock().unwrap().is_empty());
        assert!(plan.interests.lock().unwrap().is_empty());
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        assert!(matches!(source.workflow_call_for_policy(&saved.association,
            &super::interest_restore_tests::unstaged_mcp()), Err(PrivateOriginalError::Denied)),
            "empty actual G attachment must refuse the ordinary MCP fallback");
        let (_, stamp) = publication.observe(|| ()).unwrap();
        let replay = source.restore_workflow_planning_source(saved.membership.workflow(),
            &c.lead.original, &c.terms.lead).await.unwrap();
        assert!(Arc::ptr_eq(&plan, &replay));
        stamp.check_current().expect("exact native observation preserves the existing attachment");
        assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
        assert_eq!(source.store.get(&saved.row.key).await.unwrap().unwrap(), saved.row);
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(c.f.model_requests(), 0);

        // A saved successful launch cannot replace the current native input
        // witness, even after the actual G attachment was restored.
        let launch = source.store.get_effect(&open.reference().launch_effect()).await.unwrap().unwrap();
        assert_eq!(launch.status, EffectStatus::Succeeded);
        for changed_input_id in [true, false] {
            let row = source.store.get(&saved.row.key).await.unwrap().unwrap();
            let mut data = row.data.clone();
            let Input::Prompt(mut input) = source.decode_record(&row, &c.lead.original).unwrap()
                .bound_service_input.unwrap().decode().unwrap() else { panic!("retained G prompt") };
            if changed_input_id { input.header.id = InputId::new(); }
            else { input.content = meerkat_core::ContentInput::Text("changed historical G input".into()); }
            data.body["bound_service_input"] = serde_json::to_value(Input::Prompt(input)).unwrap();
            source.store.compare_and_swap(toolkit_store::Transition {
                key: row.key, expected_revision: row.revision, expected_digest: Some(row.content_digest),
                claim: None, data, events: vec![], effects: vec![],
            }).await.unwrap();
            match plan.launch(&open).await {
                Err(error) => assert_eq!(error, PrivateOriginalError::Conflict),
                Ok(observed) => panic!("saved success cannot establish admission for a changed input ID or body: {}",
                    serde_json::to_string(&observed).unwrap()),
            }
            let row = source.store.get(&saved.row.key).await.unwrap().unwrap();
            source.store.compare_and_swap(toolkit_store::Transition {
                key: row.key, expected_revision: row.revision, expected_digest: Some(row.content_digest),
                claim: None, data: saved.row.data.clone(), events: vec![], effects: vec![],
            }).await.unwrap();
            assert_eq!(source.store.get_effect(&launch.key).await.unwrap().unwrap(), launch);
            assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
            assert_eq!(history(&c).await, saved.history);
            assert_eq!(c.f.model_requests(), 0);
        }

        // B is independently admitted on its actual existing session before A
        // departs. No A request, local interest or grant is reconstructed.
        let mut waiter_inputs = ids(&c, &c.f.session).await;
        assert_eq!(waiter_inputs.len(), 1, "only the original A input precedes B");
        let mut event = crate::original_ingress::tests::private_event("settled-restore-b");
        event.route = c.f.original_reference().route.clone();
        event.content_sha256 = digest(b"new B after restored G");
        let authentication = event.original_authentication.clone();
        let original = source.retain_event(event).await.unwrap();
        // The fresh post-reopen provider is already armed for its first input, B.
        let b = source.submit_authenticated_original(PrivateOriginalSubmit { original,
            content: "new B after restored G".into(), transport_authentication: authentication }).await.unwrap();
        let PrivateOriginalSubmission::Admitted { session_id, input_id, deduplicated: false, .. } = &b
            else { panic!("B requires its own new native input") };
        assert_eq!(session_id, &c.f.session.to_string());
        let b_input = InputId::from_uuid(input_id.parse().unwrap());
        assert!(!waiter_inputs.contains(&b_input));
        waiter_inputs.push(b_input);
        waiter_inputs.sort_by_key(|id| id.to_string());
        c.f.finish(&Ok::<_, String>(b)).await.unwrap();
        // This one joined B input contains the probe tool call and then
        // the final answer. Both are counted model requests, not new inputs.
        let settled_b_requests = c.f.model_requests();
        assert_eq!(settled_b_requests, 2);
        assert_eq!(ids(&c, &c.f.session).await, waiter_inputs);
        let dispatch = c.f.probe.as_ref().unwrap().captured();
        let b = source.acquisition_original(&dispatch).await.unwrap();
        let graph = c.backend.service();
        let child = graph.create(CreateWorkItemRequest { title: "restored B".into(),
            labels: [c.terms.cycle_label.clone()].into(), ..Default::default() }).await.unwrap();
        let child = graph.claim(ClaimWorkItemRequest { id: child.id, realm_id: None, namespace: None,
            expected_revision: child.revision, owner: WorkOwner { key: WorkOwnerKey::agent("restored-b").unwrap(), display_name: None },
            lease_seconds: Some(300), lease_expires_at: None }).await.unwrap();
        graph.link(LinkWorkItemsRequest { realm_id: None, namespace: None, kind: WorkEdgeKind::Parent,
            from_id: child.id.clone(), to_id: c.terms.parent.item_id.clone() }).await.unwrap();
        let work = SetupWorkContext::from_owner(c.backend.pin().clone(), c.terms.parent.clone(), work_ref(&child),
            c.terms.parent_owner.clone(), child.owner.as_ref().unwrap().key.clone(), c.terms.cycle_label.clone()).unwrap();
        let (membership, snapshot) = join_waiter(&c.f, &plan, &b, &dispatch, &c.backend, &work, "restored-b").await;
        assert_ne!(membership.acquisition(), saved.membership.acquisition());
        assert!(!snapshot.sealed() && snapshot.active_waiters() == 2);
        assert!(!snapshot.departed(&saved.membership).unwrap());
        let snapshot = source.workflow_store().unwrap().depart_setup_workflow(&saved.membership).await.unwrap();
        assert!(!snapshot.sealed());
        assert_eq!(snapshot.active_waiters(), 1);
        assert!(snapshot.departed(&saved.membership).unwrap());
        assert!(g.unresolved.lock().unwrap().is_empty(), "cold G has no pending binder to reconcile");
        let launch = source.store.get_effect(&snapshot.reference().launch_effect()).await.unwrap().unwrap();
        assert_eq!(launch.status, EffectStatus::Succeeded);
        let g_row = source.store.get(&saved.row.key).await.unwrap().unwrap();
        let grants = std::fs::read(&grants_path).unwrap();
        for _ in 0..2 {
            let observed = plan.launch(&snapshot).await.unwrap();
            assert!(matches!(&observed, WorkflowLaunchObservation::HistoricalAdmitted { session_id, input_id }
                if session_id == &c.lead_session.to_string() && input_id == &saved.ids[0].to_string()),
                "restored launch must observe the exact original G admission without a new submission: {}",
                serde_json::to_string(&observed).unwrap());
            assert_eq!(source.store.get_effect(&launch.key).await.unwrap().unwrap(), launch);
            assert_eq!(source.store.get(&saved.row.key).await.unwrap().unwrap(), g_row);
            assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
            assert!(g.unresolved.lock().unwrap().is_empty());
        }
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids, "B observes the original G launch without resubmission");
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(plan.interests.lock().unwrap().len(), 1);
        assert!(plan.interests.lock().unwrap()[0].membership == membership,
            "restored interest retains the exact B membership");
        assert!(matches!(source.workflow_call_for_policy(&saved.association,
            &super::interest_restore_tests::unstaged_mcp()), Err(PrivateOriginalError::Denied)),
            "Find membership does not grant an unrelated MCP operation");
        let closed = plan.depart_waiter(&membership).await.unwrap();
        assert!(closed.sealed() && closed.active_waiters() == 0);
        source.record_operation_cancelled(&c.lead.original).await.unwrap();
        let cancelled = source.store.get(&saved.row.key).await.unwrap().unwrap();
        assert!(source.decode_record(&cancelled, &c.lead.original).unwrap().operation_cancelled);
        // Canonical sealing does not prove native completion. The actual old
        // input receipt, not a reconstructed pending binder, must close G.
        let terminal = c.f.machine.input_terminal_receipt(&c.lead_session,
            meerkat_runtime::terminal_status::InteractionSelector::InputId(saved.ids[0].clone()))
            .await.unwrap().unwrap();
        assert!(matches!(terminal.report, meerkat_runtime::terminal_status::InputTerminalReceiptRead::Finalized(_)));
        let mut missing = cancelled.data.clone();
        missing.body.as_object_mut().unwrap().remove("bound_service_input");
        source.store.compare_and_swap(toolkit_store::Transition {
            key: cancelled.key.clone(), expected_revision: cancelled.revision,
            expected_digest: Some(cancelled.content_digest.clone()), claim: None,
            data: missing, events: vec![], effects: vec![],
        }).await.unwrap();
        assert_eq!(plan.observe_close(closed.reference()).await.unwrap(), WorkflowCloseObservation::Pending,
            "a real finalized receipt cannot replace missing canonical input selection");
        assert_eq!(source.store.get_effect(&closed.reference().close_effect()).await.unwrap().unwrap().status,
            EffectStatus::Uncertain);
        let row = source.store.get(&saved.row.key).await.unwrap().unwrap();
        let mut corrupt = cancelled.data.clone();
        corrupt.body["bound_service_input"]["content"] = serde_json::json!("different G completion");
        source.store.compare_and_swap(toolkit_store::Transition {
            key: row.key, expected_revision: row.revision, expected_digest: Some(row.content_digest),
            claim: None, data: corrupt, events: vec![], effects: vec![],
        }).await.unwrap();
        assert_eq!(plan.close_observation(source, closed.reference()).await,
            Err(PrivateOriginalError::Conflict));
        let row = source.store.get(&saved.row.key).await.unwrap().unwrap();
        let restored = source.store.compare_and_swap(toolkit_store::Transition {
            key: row.key, expected_revision: row.revision, expected_digest: Some(row.content_digest),
            claim: None, data: cancelled.data.clone(), events: vec![], effects: vec![],
        }).await.unwrap();
        let expected = WorkflowCloseObservation::SourceFencedAndFinalized {
            session_id: c.lead_session.to_string(), input_id: saved.ids[0].to_string(),
        };
        assert_eq!(plan.observe_close(closed.reference()).await.unwrap(), expected);
        let close_effect = source.store.get_effect(&closed.reference().close_effect()).await.unwrap().unwrap();
        assert_eq!(close_effect.status, EffectStatus::Succeeded);
        assert_eq!(plan.observe_close(closed.reference()).await.unwrap(), expected);
        assert_eq!(source.store.get_effect(&close_effect.key).await.unwrap().unwrap(), close_effect);
        assert_eq!(source.store.get(&saved.row.key).await.unwrap().unwrap(), restored);
        assert_eq!(source.store.get_effect(&launch.key).await.unwrap().unwrap(), launch);
        assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
        assert!(g.unresolved.lock().unwrap().is_empty());
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(ids(&c, &c.f.session).await, waiter_inputs);
        assert_eq!(c.f.model_requests(), settled_b_requests, "close observation cannot invoke G or replay B");
        assert!(matches!(source.restore_workflow_planning_source(saved.membership.workflow(),
            &c.lead.original, &c.terms.lead).await, Err(PrivateOriginalError::Denied)));
        // Closure preserves P and the selected grant envelope. The actual G
        // source and closed workflow, not that pure projection, forbid reuse.
        plan.check_for_binding(source, &c.lead.original).unwrap();
        let workflow = g.setup_workflow.get().unwrap();
        assert!(matches!(workflow.current_operation_source(), Err(PrivateOriginalError::Denied)));
        assert!(matches!(workflow.current_source(), Err(PrivateOriginalError::Denied)));
        assert!(matches!(plan.launch(&closed).await, Err(PrivateOriginalError::Denied)));
        assert_eq!(source.store.get(&saved.row.key).await.unwrap().unwrap(), restored);
        assert_eq!(source.store.get_effect(&launch.key).await.unwrap().unwrap(), launch);
        assert_eq!(source.store.get_effect(&close_effect.key).await.unwrap().unwrap(), close_effect);
        assert_eq!(std::fs::read(&grants_path).unwrap(), grants);
        assert_eq!(ids(&c, &c.lead_session).await, saved.ids);
        assert_eq!(history(&c).await, saved.history);
        assert_eq!(ids(&c, &c.f.session).await, waiter_inputs);
        assert_eq!(c.f.model_requests(), settled_b_requests);
    })).catch_unwind().await;
    close(&c, body).await;
}
