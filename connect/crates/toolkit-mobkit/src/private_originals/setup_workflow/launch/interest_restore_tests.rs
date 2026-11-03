//! Real ordinary resume and exact canonical interest, without new native work.
use super::idle_restore_tests::{assert_unattached_policy_closed, close};
use super::waiter_restore_tests::{
    PersistentDedicated, all_ids, histories, join_recovery_for_idle_restore, selected_then_close,
};
use super::*;
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationFacts, PreparedAuthorizationBinding,
    ToolAuthorizationFacts, ToolAuthorizationTarget,
};
use meerkat_mobkit::identity_first::ControllerPinSource;

// A negative policy-component input only, following the existing MCP policy
// fixture. No router, native operation or MCP attachment is manufactured.
pub(super) fn unstaged_mcp() -> PreparedAuthorizationBinding {
    let fingerprint = meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
        &meerkat_core::ToolCatalogEntry::session_inline(
            Arc::new(meerkat_core::ToolDef::new(
                "unstaged",
                "negative policy input",
                serde_json::json!({"type":"object"}),
            )),
            true,
        ),
    );
    let witness = meerkat_core::ToolExecutionOwnerWitness::new(
        "negative-interest-fixture",
        "unstaged",
        fingerprint,
    )
    .unwrap()
    .with_operation_target(meerkat_core::ToolOperationTarget::McpServerOperation {
        server: "unstaged".into(),
        connection_generation: 1,
        raw_operation: "write".into(),
        connector: meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
        account: Arc::new(meerkat_core::OperationAccount::Unavailable),
    });
    let deadlines =
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(60),
        )])
        .unwrap();
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(deadlines)
        .unwrap()
        .with_owner_witness(witness)
        .unwrap();
    PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000007\"").unwrap(),
        execution_scope: meerkat_core::exact_operation::OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
            call_id: Arc::from("unstaged"),
            name: "unstaged".into(),
            arguments: serde_json::value::RawValue::from_string("{}".into())
                .unwrap()
                .into(),
            target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
        }),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_interest_reuses_native_sources_without_input_or_effect_delegation() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let source = &c.f.source;
        let grants_path = c.f.source_path.parent().unwrap().join("grants.jsonl");
        let grants = std::fs::read(&grants_path).unwrap();
        let effect =
            c.f.store
                .get_effect(&saved.membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
        let workflow = source
            .workflow_store()
            .unwrap()
            .load_setup_workflow(saved.membership.workflow())
            .await
            .unwrap();
        assert!(
            c.member
                .restore_waiter_interest(c.f.original_reference(), &saved.membership)
                .await
                .is_err(),
            "canonical membership alone supplies no native source custody"
        );
        c.member
            .restore_waiter_source(c.f.original_reference())
            .await
            .unwrap();
        c.member.restore_admitted_source().await.unwrap();
        let g = source
            .binding_evidence(&c.lead.original)
            .await
            .unwrap()
            .unwrap();
        assert_unattached_policy_closed(&c, &g);
        let publication = source.owner.publication();
        let (_, before_attach) = publication.observe(|| ()).unwrap();
        let plan = c
            .member
            .restore_waiter_interest(c.f.original_reference(), &saved.membership)
            .await
            .unwrap();
        assert!(
            before_attach.check_current().is_err(),
            "first attachment publishes the changed gate and interest"
        );
        let (_, before_replay) = publication.observe(|| ()).unwrap();
        let replay = c
            .member
            .restore_waiter_interest(c.f.original_reference(), &saved.membership)
            .await
            .unwrap();
        before_replay
            .check_current()
            .expect("exact successful restore replay leaves current publication unchanged");
        assert!(
            Arc::ptr_eq(&plan, &replay),
            "the actual P owner interns one planning selection"
        );
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        let (a, held_g) = {
            let held = source.retained.lock().unwrap();
            (
                held.iter()
                    .find(|row| &row.reference == c.f.original_reference())
                    .unwrap()
                    .clone(),
                held.iter()
                    .find(|row| row.reference == c.lead.original)
                    .unwrap()
                    .clone(),
            )
        };
        assert!(Arc::ptr_eq(
            &source
                .retained_workflow_selection(&a.original, &id("selected-a-restore-call"))
                .unwrap()
                .unwrap(),
            &plan
        ));
        let attached = held_g.setup_workflow.get().unwrap();
        assert!(attached.accepted.load(Ordering::Acquire));
        assert_eq!(attached.session, c.lead_session);
        assert!(
            attached.waiters.lock().unwrap().is_empty(),
            "no per-waiter effect custody was reconstructed"
        );
        assert!(matches!(
            source.workflow_call_for_policy(&g, &unstaged_mcp()),
            Err(PrivateOriginalError::Denied)
        ));
        let WorkAuthorityBasis::GrantLineage { lineage } =
            &saved.association.candidate().authority_basis
        else {
            panic!("actual A task")
        };
        assert!(
            source
                .owner
                .grants()
                .resolve_lineage(
                    lineage,
                    &g.candidate().logical_executor,
                    g.candidate().represented_subject.as_ref()
                )
                .is_err(),
            "A's task was not delegated to G"
        );
        {
            let interests = plan.interests.lock().unwrap();
            assert_eq!(interests.len(), 1);
            assert!(interests[0].membership == saved.membership);
            assert!(!interests[0].closed && interests[0].entries.is_empty());
            assert!(Arc::ptr_eq(
                interests[0].original.as_ref().unwrap(),
                &a.original
            ));
        }
        // Exercise the existing actual A currentness/entry owner without
        // submitting native work. Its finite unentered reservation is settled.
        LaunchFence {
            planning: plan.clone(),
        }
        .enter()
        .unwrap()
        .finish(GovernedEntryOutcome::Unentered);
        plan.settle_launch_interest().unwrap();
        assert_eq!(
            source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(saved.membership.workflow())
                .await
                .unwrap()
                .revision(),
            workflow.revision()
        );
        assert_eq!(
            c.f.store.get_effect(&effect.key).await.unwrap().unwrap(),
            effect
        );
        assert_eq!(std::fs::read(grants_path).unwrap(), grants);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumed_interest_rejects_changed_membership_and_departure_during_native_observation() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        let source = &c.f.source;
        c.member
            .restore_waiter_source(c.f.original_reference())
            .await
            .unwrap();
        c.member.restore_admitted_source().await.unwrap();
        let g = source
            .binding_evidence(&c.lead.original)
            .await
            .unwrap()
            .unwrap();
        assert!(
            source
                .workflow_store()
                .unwrap()
                .workflow_waiter_recovery_snapshot(&saved.membership, &g)
                .await
                .is_err(),
            "another actual native source is not this acquisition's A"
        );
        let mut changed = serde_json::to_value(&saved.membership).unwrap();
        changed["identity"]["input_digest"] =
            serde_json::to_value(ContentDigest::of_bytes(b"changed request")).unwrap();
        let changed: SetupWorkflowWaiter = serde_json::from_value(changed).unwrap();
        assert!(
            c.member
                .restore_waiter_interest(c.f.original_reference(), &changed)
                .await
                .is_err()
        );
        let plan = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.reference == c.parent.original)
            .unwrap()
            .workflow_plans
            .lock()
            .unwrap()[0]
            .clone();
        assert!(plan.interests.lock().unwrap().is_empty());
        let publication = source.owner.publication();
        let (_, rejected_departure) = publication.observe(|| ()).unwrap();
        assert!(matches!(
            plan.depart_waiter(&changed).await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(
            plan.interests.lock().unwrap().is_empty(),
            "invalid portable membership leaves no tombstone or capacity use"
        );
        rejected_departure
            .check_current()
            .expect("rejected membership does not change publication");
        assert_unattached_policy_closed(&c, &g);
        let pause = Arc::new((tokio::sync::Notify::new(), tokio::sync::Notify::new()));
        let pending = super::super::restore::AFTER_MEMBERSHIP_READ.scope(
            pause.clone(),
            c.member
                .restore_waiter_interest(c.f.original_reference(), &saved.membership),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = pause.0.notified() => (),
            _ = &mut pending => panic!("the real canonical membership read must be reached"),
        }
        // The same interned owner exists, but no interest/controller has been
        // published. Departure wins the source writer and leaves a tombstone.
        let departed = plan.depart_waiter(&saved.membership).await.unwrap();
        assert!(departed.departed(&saved.membership).unwrap());
        pause.1.notify_one();
        assert!(matches!(
            pending.await,
            Err(PrivateOriginalError::Unavailable)
        ));
        {
            let interests = plan.interests.lock().unwrap();
            assert_eq!(interests.len(), 1);
            assert!(interests[0].closed && interests[0].original.is_none());
        }
        let claims = saved.association.candidate();
        let unchanged_a = source
            .owner
            .grants()
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .unwrap();
        let (_, repeated_departure) = publication.observe(|| ()).unwrap();
        let replay = plan.depart_waiter(&saved.membership).await.unwrap();
        assert_eq!(replay.revision(), departed.revision());
        repeated_departure
            .check_current()
            .expect("closed tombstone replay does not invalidate unrelated work");
        unchanged_a
            .check_current()
            .expect("A controller remains current after departure replay");
        assert_unattached_policy_closed(&c, &g);
        assert!(
            c.member
                .restore_waiter_interest(c.f.original_reference(), &saved.membership)
                .await
                .is_err()
        );
        assert!(matches!(
            LaunchFence {
                planning: plan.clone()
            }
            .enter(),
            Err(GovernedEntryRefusal::Retired)
        ));
        source
            .record_operation_cancelled(c.f.original_reference())
            .await
            .unwrap();
        assert!(
            c.member
                .restore_waiter_interest(c.f.original_reference(), &saved.membership)
                .await
                .is_err()
        );
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_interest_pin_refresh_adopts_exact_callback_and_refuses_changed_route() {
    let saved = selected_then_close().await;
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        join_recovery_for_idle_restore(&c).await;
        let source = &c.f.source;
        source
            .with_synthetic_admission_diagnostics(
                c.member.restore_waiter_source(c.f.original_reference()),
            )
            .await
            .unwrap();
        c.member.restore_admitted_source().await.unwrap();
        let plan = c
            .member
            .restore_waiter_interest(c.f.original_reference(), &saved.membership)
            .await
            .unwrap();
        let held = source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|row| row.reference == c.lead.original)
            .unwrap()
            .clone();
        let attachment = held.setup_workflow.get().unwrap();
        let row =
            c.f.store
                .get(&source.key(&c.lead.original).unwrap())
                .await
                .unwrap()
                .unwrap();
        let input = source
            .decode_record(&row, &c.lead.original)
            .unwrap()
            .bound_service_input
            .unwrap()
            .decode()
            .unwrap();
        let machine = &c.f.machine;
        let registration = machine
            .current_session_registration_witness(&c.lead_session)
            .await
            .unwrap();
        let executor = machine
            .current_executor_attachment_witness(&c.lead_session)
            .await
            .unwrap();
        let actor = c.f.service.live_actor(&c.lead_session).await.unwrap();
        let identity = actor.executor_actor_identity();
        let before = source.owner.workflow_controller(&held.original).unwrap();
        let facts = before.plain_facts().unwrap();
        let route = facts.route_key();
        assert!(attachment.controller_route == route);
        // Only the expected host-policy route is changed. The runnable pin
        // still comes from this real actor's protected native callback.
        let changed_route = meerkat_core::ControllerModelFacts::new(
            facts.selection().clone(),
            Arc::from("https://changed-route.invalid/v1"),
            Arc::from(facts.wire_model()),
        )
        .route_key();
        assert!(changed_route != route);
        let publication = source.owner.publication();
        let (_, rejected) = publication.observe(|| ()).unwrap();
        let result = machine
            .with_admitted_original(&registration, &executor, &identity, &input, |admitted| {
                let _writer = source.owner.try_source_writer()?;
                source.owner.complete_restored_workflow_member(
                    saved.membership.workflow(),
                    &c.terms.lead,
                    held.original.association(),
                    admitted,
                    &changed_route,
                    |_| panic!("a mismatched actual route cannot reach attachment publication"),
                )
            })
            .await;
        assert!(matches!(result, Ok(Err(OriginalWriterError::Conflict))));
        rejected
            .check_current()
            .expect("route refusal must leave policy unchanged");
        assert!(
            Arc::ptr_eq(
                before.client(),
                source
                    .owner
                    .workflow_controller(&held.original)
                    .unwrap()
                    .client()
            ),
            "route refusal cannot replace the retained runnable pin"
        );
        for _ in 0..2 {
            let (_, stamp) = publication.observe(|| ()).unwrap();
            let returned = machine
                .with_admitted_original(&registration, &executor, &identity, &input, |admitted| {
                    let _writer = source.owner.try_source_writer()?;
                    assert_eq!(admitted.session_id(), &c.lead_session);
                    assert_eq!(admitted.input_id(), input.id());
                    assert!(admitted.association() == held.original.association());
                    source.owner.complete_restored_workflow_member(
                        saved.membership.workflow(),
                        &c.terms.lead,
                        held.original.association(),
                        admitted,
                        &route,
                        |apply| {
                            assert!(!apply, "an exact existing attachment changes no policy");
                            Ok(false)
                        },
                    )?;
                    Ok::<_, OriginalWriterError>(admitted.controller().clone())
                })
                .await
                .unwrap()
                .unwrap();
            let retained = source.owner.workflow_controller(&held.original).unwrap();
            assert!(
                Arc::ptr_eq(retained.client(), returned.client()),
                "retain this callback's exact runnable pin, not an earlier equal-route wrapper"
            );
            assert!(retained.plain_facts().unwrap().route_key() == attachment.controller_route);
            stamp
                .check_current()
                .expect("native custody refresh is publication-free");
            let current =
                c.f.service
                    .live_actor(&c.lead_session)
                    .await
                    .unwrap()
                    .executor_actor_identity();
            assert!(identity.is_same_actor(&current) && current.is_live());
        }
        let prior = source.owner.workflow_controller(&held.original).unwrap();
        source
            .bound_identity_runtime()
            .unwrap()
            .respawn_tracked(
                &meerkat_mobkit::identity_first::AgentIdentity::parse("planning_lead").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(c.member.session().unwrap(), c.lead_session);
        let successor =
            c.f.service
                .live_actor(&c.lead_session)
                .await
                .unwrap()
                .executor_actor_identity();
        assert!(!identity.is_live() && successor.is_live() && !identity.is_same_actor(&successor));
        let replay = c
            .member
            .restore_waiter_interest(c.f.original_reference(), &saved.membership)
            .await
            .unwrap();
        let registration = machine
            .current_session_registration_witness(&c.lead_session)
            .await
            .unwrap();
        let executor = machine
            .current_executor_attachment_witness(&c.lead_session)
            .await
            .unwrap();
        let retained = source.owner.workflow_controller(&held.original).unwrap();
        assert!(
            !Arc::ptr_eq(prior.client(), retained.client()),
            "retired actor custody cannot survive a real respawn"
        );
        assert!(retained.plain_facts().unwrap().route_key() == route);
        assert!(
            machine
                .with_admitted_original(
                    &registration,
                    &executor,
                    &identity,
                    &input,
                    |_| -> Result<(), OriginalWriterError> {
                        panic!("the retired actor cannot publish custody")
                    }
                )
                .await
                .is_err()
        );
        let (_, stamp) = publication.observe(|| ()).unwrap();
        let actual = machine
            .with_admitted_original(&registration, &executor, &successor, &input, |admitted| {
                let _writer = source.owner.try_source_writer()?;
                source.owner.complete_restored_workflow_member(
                    saved.membership.workflow(),
                    &c.terms.lead,
                    held.original.association(),
                    admitted,
                    &route,
                    |apply| {
                        assert!(!apply, "successor custody changes no canonical policy");
                        Ok(false)
                    },
                )?;
                Ok::<_, OriginalWriterError>(admitted.controller().clone())
            })
            .await
            .unwrap()
            .unwrap();
        assert!(
            Arc::ptr_eq(
                actual.client(),
                source
                    .owner
                    .workflow_controller(&held.original)
                    .unwrap()
                    .client()
            ),
            "the existing source adopts this actual successor callback's exact pin"
        );
        stamp.check_current().unwrap();
        assert!(Arc::ptr_eq(&plan, &replay));
        assert_eq!(plan.interests.lock().unwrap().len(), 1);
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_interest_restore_refuses_retired_actor_after_same_route_reopen() {
    let saved = selected_then_close().await;
    let old = PersistentDedicated::reopen(saved.durable.clone()).await;
    let observed = AssertUnwindSafe(Box::pin(async {
        let actor = old.f.service.live_actor(&old.lead_session).await.unwrap();
        let identity = actor.executor_actor_identity();
        let pin = old.f.service.pin_actor(&actor).await.unwrap();
        assert!(identity.is_live());
        (identity, pin.plain_facts().unwrap().route_key())
    }))
    .catch_unwind()
    .await;
    let cleanup = AssertUnwindSafe(async {
        let result = old.close().await;
        assert!(result.is_ok(), "{result:?}");
    })
    .catch_unwind()
    .await;
    drop(old);
    let (old_actor, old_route) = match observed {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(value) => {
            if let Err(panic) = cleanup {
                std::panic::resume_unwind(panic);
            }
            value
        }
    };
    let c = PersistentDedicated::reopen(saved.durable.clone()).await;
    let body = AssertUnwindSafe(Box::pin(async {
        assert!(
            !old_actor.is_live(),
            "joined ordinary close revokes the old actor"
        );
        let actor = c.f.service.live_actor(&c.lead_session).await.unwrap();
        let current = actor.executor_actor_identity();
        assert!(current.is_live() && !old_actor.is_same_actor(&current));
        let pin = c.f.service.pin_actor(&actor).await.unwrap();
        assert!(
            pin.plain_facts().unwrap().route_key() == old_route,
            "the stale-actor refusal must not depend on a route change"
        );
        let source = &c.f.source;
        let row =
            c.f.store
                .get(&source.key(&c.lead.original).unwrap())
                .await
                .unwrap()
                .unwrap();
        let record = source.decode_record(&row, &c.lead.original).unwrap();
        let association = record.final_association.unwrap();
        let input = record.bound_service_input.unwrap().decode().unwrap();
        let registration =
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .unwrap();
        let executor =
            c.f.machine
                .current_executor_attachment_witness(&c.lead_session)
                .await
                .unwrap();
        let called = AtomicBool::new(false);
        let publication = source.owner.publication();
        let (_, stamp) = publication.observe(|| ()).unwrap();
        let refused = c
            .f
            .machine
            .with_admitted_original::<(), ()>(&registration, &executor, &old_actor, &input, |_| {
                called.store(true, Ordering::Release);
                Ok(())
            })
            .await;
        assert!(matches!(
            refused,
            Err(meerkat_runtime::meerkat_machine::AdmittedOriginalRefusal::Unavailable)
        ));
        assert!(
            !called.load(Ordering::Acquire),
            "stale actor must never reach source adoption"
        );
        stamp
            .check_current()
            .expect("stale observation changes no source publication");
        assert!(!source.owner.has_admitted_source(&association).unwrap());
        assert!(c.member.session().is_err());
        c.f.machine
            .with_admitted_original::<(), ()>(
                &registration,
                &executor,
                &current,
                &input,
                |admitted| {
                    assert!(admitted.association() == &association);
                    assert_eq!(admitted.input_id(), input.id());
                    assert!(admitted.controller().plain_facts().unwrap().route_key() == old_route);
                    Ok(())
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            !source.owner.has_admitted_source(&association).unwrap(),
            "observation alone grants no source custody"
        );
        assert_eq!(all_ids(&c).await, saved.ids);
        assert_eq!(histories(&c).await, saved.histories);
        assert_eq!(c.f.model_requests(), 0);
    }))
    .catch_unwind()
    .await;
    close(&c, body).await;
}
