//! Actual SQLite admission commit with its native acknowledgement lost.
//! Core supplies the store fault and uncertain admission settlement; unchanged
//! Meta invokes the real binder's retain_unresolved. No bridge error or source
//! settlement callback is fabricated. The local R2 review is not Find authority.
use super::*;

fn unresolved_native_run(
    c: &Fixture,
    context: &InstalledWorkflowContext,
    slot: &Arc<DeliverySlot>,
    review: ReviewCustody,
) -> Arc<DeliveryRun> {
    let source = c.f.source.clone();
    let transfer = context.transfer.clone();
    let attempt = context.attempt.clone();
    let selected = slot.clone();
    let control = attempt
        .recovery
        .upgrade()
        .expect("same real transfer recovery owner");
    control.enter_transfer(|| DeliveryRun::retain_or_start(&mut slot.run.lock().unwrap(), move |task_native| async move {
        review.retain_io(async {
            let planning = transfer.planning()?;
            let effect = selected.effect(&source, &attempt).await?;
            assert!(delivery_claim_ready(&selected.native, &effect)?);
            transfer.with_recipient(&source, |workflow| {
                if !Arc::ptr_eq(&workflow, &selected.workflow) { return Err(PrivateOriginalError::Conflict); }
                Ok(())
            }).await?;
            let snapshot = source.workflow_store()?.load_setup_workflow(&transfer.selected.workflow)
                .await.map_err(|_| PrivateOriginalError::Unavailable)?;
            assert!(snapshot.terms() == &planning.terms);
            let _ = snapshot.departed(&attempt.envelope.membership)
                .map_err(|_| PrivateOriginalError::Conflict)?;
            let (native, lease) = claim_delivery_effect_with_review(source.store.as_ref(), &effect,
                snapshot.revision(), &selected.native, &task_native, review.clone()).await?.unwrap();
            let route = source.route(&transfer.selected.planning_source.route)?;
            let binder = Arc::new(DeliveryBinder { owner: source.owner.clone(),
                original: selected.workflow.planning.clone(), slot: selected.clone(), native: native.clone(),
                controller: source.owner.workflow_controller(&selected.workflow.planning).map_err(custody_error)?,
                authentication: evidence(&route.authentication_domain, &planning.submission.transport_authentication)?,
                planning });
            let runtime = source.bound_identity_runtime()?;
            let identity = runtime.owned_identity_for_member_alias(&selected.workflow.member.member).await.unwrap();
            *native.decision.lock().unwrap() = NativeDecision::Submitted;
            let actual = runtime.dispatch_governed_with_turn_ticket(&identity,
                Some(&selected.workflow.member.member), selected.input.clone(), review.bind(binder)).await;
            assert!(matches!(actual, Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::AdmissionUnresolved { .. }))),
                "unchanged Core/Meta must return actual uncertain admission, not accepted callback failure");
            assert!(native.custody.pending.lock().unwrap().is_some());
            assert_eq!(native.custody.unresolved.lock().unwrap().len(), 1,
                "Meta must transfer its original settlement to the real DeliveryBinder");
            assert!(native.custody.accepted.get().is_none());
            assert!(!native.custody.entry.get().unwrap().settled());
            // Same owned uncertainty acknowledgment as DeliverySlot::drive's
            // actual AdmissionUnresolved arm; no retry or negative witness.
            let acknowledged = source.store.acknowledge_effect(&lease,
                EffectOutcome::Uncertain { evidence: selected.receipt(&attempt)? }).await.map_err(store_error)?;
            assert_eq!(acknowledged.status, EffectStatus::Uncertain);
            assert_eq!(acknowledged.attempt, 1);
            Ok(WorkflowDeliveryObservation::Pending)
        }).await
    })).expect("actual registration gate accepted this task")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_sqlite_ack_loss_retains_workflow_review_through_uncertain_default_reconciliation() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let history = c.history.as_ref().expect("actual persistent runtime store");
        let fault = history.input_persist_ack_loss_fault_for_test()
            .expect("only the actual SqliteRuntimeStore may issue this fault");
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&slot.workflow.session);
        let rows_before = history.load_input_states_strict(&runtime_id).await.unwrap();
        assert!(rows_before.iter().all(|row| row.state.input_id != *slot.input.id()));
        let native_before = fixture::native_rows(&c).await;
        let requests_before = c.f.model_requests();
        let (review, observations) = consumed("workflow-native-lost-ack").await;
        fault.lose_next_commit_acknowledgement(runtime_id.clone());
        assert!(fault.is_armed());
        assert!(fault.consumed_commit().is_none());

        let run = unresolved_native_run(&c, &context, &slot, review);
        assert_eq!(tokio::time::timeout(WAIT, run.join()).await.unwrap().unwrap(), WorkflowDeliveryObservation::Pending);
        assert!(!fault.is_armed(), "a wrong runtime or a refused pre-commit path cannot pass");
        assert_eq!(fault.consumed_commit(), Some((runtime_id.clone(), vec![slot.input.id().clone()])),
            "the original SQLite instance really committed this exact input batch before losing its reply");
        let rows_after = history.load_input_states_strict(&runtime_id).await.unwrap();
        assert_eq!(rows_after.len(), rows_before.len() + 1);
        let persisted = rows_after.iter().find(|row| row.state.input_id == *slot.input.id()).unwrap();
        assert!(persisted.state.persisted_input.is_some());
        verify_exact_replay(&persisted.state, slot.bound.get().unwrap()).unwrap();
        assert_eq!(c.f.model_requests(), requests_before, "ambiguous native admission does not wake an executor");

        let native = slot.native.lock().unwrap().clone().unwrap();
        assert!(native.joined.load(Ordering::Acquire));
        assert!(matches!(*native.decision.lock().unwrap(), NativeDecision::Submitted));
        assert!(native.custody.review.lock().unwrap().is_some());
        assert!(native.custody.pending.lock().unwrap().is_some());
        assert_eq!(native.custody.unresolved.lock().unwrap().len(), 1);
        assert!(native.custody.accepted.get().is_none());
        assert!(native.not_applied.get().is_none());
        let uncertain = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert_eq!(uncertain.status, EffectStatus::Uncertain);
        assert_eq!(uncertain.outcome, Some(EffectOutcome::Uncertain { evidence: slot.receipt(&context.attempt).unwrap() }));
        assert_eq!(observations.used_count(), 0,
            "joining genuine uncertain native admission must not finish the consumed review");

        let native_before_reconciliation = fixture::native_rows(&c).await;
        let mut previous_rows = native_before_reconciliation.clone();
        let selected_id = serde_json::to_value(slot.input.id()).unwrap();
        let mut added = 0;
        for row in previous_rows.as_array_mut().unwrap() {
            let ids = row.get_mut("ids").unwrap().as_array_mut().unwrap();
            let count = ids.len();
            ids.retain(|id| id != &selected_id);
            added += count - ids.len();
        }
        assert_eq!(added, 1, "exactly the committed delivery input was added across A/P/G");
        assert_eq!(previous_rows, native_before, "all previous native input IDs and histories are unchanged");
        let observed = slot.execute(c.f.source.clone(), context.transfer.clone(), context.attempt.clone(),
            false, ReviewCustody::default()).await.unwrap();
        assert_eq!(observed, slot.admitted());
        observations.exactly_used("workflow-native-lost-ack").await;
        assert!(native.custody.pending.lock().unwrap().is_none());
        assert!(native.custody.unresolved.lock().unwrap().is_empty());
        assert!(native.custody.entry.get().unwrap().settled());
        assert!(native.custody.review.lock().unwrap().is_none());
        assert!(native.custody.accepted.get().is_none(), "readback is not a fabricated live admission reply");
        assert!(native.not_applied.get().is_none());
        let applied = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert_eq!(applied.status, EffectStatus::Succeeded);
        assert_eq!(applied.attempt, 1);
        assert!(applied.lease.is_none());
        assert_eq!(applied.outcome, Some(EffectOutcome::Succeeded { receipt: slot.receipt(&context.attempt).unwrap() }));
        assert_eq!(fixture::native_rows(&c).await, native_before_reconciliation);
        assert_eq!(c.f.model_requests(), requests_before);
        assert_eq!(fault.consumed_commit(), Some((runtime_id, vec![slot.input.id().clone()])));
        assert_eq!(slot.execute(c.f.source.clone(), context.transfer.clone(), context.attempt.clone(),
            false, ReviewCustody::default()).await.unwrap(), observed);
        assert_eq!(observations.used_count(), 1);
        assert_eq!(fixture::native_rows(&c).await, native_before_reconciliation);
        assert_eq!(c.f.model_requests(), requests_before);
    }).catch_unwind().await;
    fixture::close(&c, &lifecycle, body).await;
}
