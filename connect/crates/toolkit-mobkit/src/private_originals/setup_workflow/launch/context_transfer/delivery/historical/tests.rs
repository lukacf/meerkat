//! Controlled committed envelope, actual governed G admission, then actual
//! native retirement. This is the metadata step on an already restored owner;
//! it does not qualify fresh-process host selection or the native Ready worker.
use super::super::super::restore_tests::{self as fixture, Data, Fixture, WAIT};
use super::*;
use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
use futures::FutureExt;
use meerkat_mobkit::identity_first::{TurnOutcome, TurnOutput, TurnTracking};
use meerkat_runtime::meerkat_machine::MeerkatMachine;
use std::panic::AssertUnwindSafe;
use toolkit_store::ToolkitStore;

pub(in crate::private_originals::setup_workflow::launch::context_transfer) async fn restored(
    c: &Fixture,
    data: &Data,
    lifecycle: &AcquisitionRecovery,
) -> InstalledWorkflowContext {
    fixture::store_envelope(data).await;
    match fixture::restore(c, data, lifecycle).await.unwrap() {
        WorkflowContextObservation::Committed(context) => context,
        _ => panic!("actual committed store observation"),
    }
}

/// Enter the real binder/native input with a real Dispatch lease. The controlled
/// WorkGraph envelope supplies test data, not a claim of an actual Find commit.
/// After admission we deliberately record Uncertain, not a lost SQLite reply.
pub(in crate::private_originals::setup_workflow::launch::context_transfer) async fn admitted_but_unacknowledged(
    c: &Fixture,
    context: &InstalledWorkflowContext,
) -> (Input, GovernedAdmission) {
    let source = &c.f.source;
    let attempt = &context.attempt;
    let (input, event) = delivery_input(attempt).unwrap();
    let slot = Arc::new(DeliverySlot {
        input,
        event,
        workflow: context.workflow.clone(),
        bound: OnceLock::new(),
        run: Mutex::new(None),
        native: Mutex::new(None),
    });
    // Keep actual pending/entry custody in the existing slot until joined.
    *attempt.delivery.lock().unwrap() = Some(slot.clone());
    let effect = delivery_effect(source, attempt).await.unwrap();
    let snapshot = source
        .workflow_store()
        .unwrap()
        .load_setup_workflow(&context.transfer.selected.workflow)
        .await
        .unwrap();
    let task_native = Mutex::new(None);
    let (native, lease) = claim_delivery_effect(
        source.store.as_ref(),
        &effect,
        snapshot.revision(),
        &slot.native,
        &task_native,
    )
    .await
    .unwrap()
    .unwrap();
    let planning = context.transfer.planning().unwrap();
    let route = source
        .route(&context.transfer.selected.planning_source.route)
        .unwrap();
    let binder = Arc::new(DeliveryBinder {
        owner: source.owner.clone(),
        original: slot.workflow.planning.clone(),
        slot: slot.clone(),
        native: native.clone(),
        controller: source
            .owner
            .workflow_controller(&slot.workflow.planning)
            .unwrap(),
        authentication: evidence(
            &route.authentication_domain,
            &planning.submission.transport_authentication,
        )
        .unwrap(),
        planning,
    });
    let runtime = source.bound_identity_runtime().unwrap();
    let identity = runtime
        .owned_identity_for_member_alias(&slot.workflow.member.member)
        .await
        .unwrap();
    *native.decision.lock().unwrap() = NativeDecision::Submitted;
    let actual = runtime
        .dispatch_governed_with_turn_ticket(
            &identity,
            Some(&slot.workflow.member.member),
            slot.input.clone(),
            binder,
        )
        .await
        .unwrap();
    assert!(actual.settlement.is_ok() && !actual.admission.deduplicated);
    assert_eq!(actual.admission.session_id, c.lead_session);
    assert_eq!(actual.admission.input_id, *slot.input.id());
    assert_eq!(native.custody.accepted.get(), Some(&actual.admission));
    *native.decision.lock().unwrap() = NativeDecision::Accepted;
    native.joined.store(true, Ordering::Release);
    let TurnTracking::Tracked(ticket) = actual.ticketed.turn else {
        panic!("actual native turn ticket")
    };
    assert!(matches!(
        runtime.wait_for_turn(&identity, ticket, WAIT).await,
        TurnOutcome::Completed {
            output: TurnOutput::Text { .. }
        }
    ));
    source
        .store
        .acknowledge_effect(
            &lease,
            EffectOutcome::Uncertain {
                evidence: slot.receipt(attempt).unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(native.custody.pending.lock().unwrap().is_none());
    assert!(native.custody.unresolved.lock().unwrap().is_empty());
    assert!(native.custody.entry.get().unwrap().settled());
    let expected = slot.bound.get().unwrap().clone();
    // This owner actually joined; dropping its process evidence creates no
    // fabricated Joined commit or negative witness on the cold origin.
    assert!(Arc::ptr_eq(
        &attempt.delivery.lock().unwrap().take().unwrap(),
        &slot
    ));
    (expected, actual.admission)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_then_retired_delivery_is_acknowledged_without_live_actor_or_another_input() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let context = restored(&c, &data, &lifecycle).await;
        let (bound, actual) = admitted_but_unacknowledged(&c, &context).await;
        let effect_key = data.envelope.membership.delivery_effect().unwrap();
        let uncertain = c.f.store.get_effect(&effect_key).await.unwrap().unwrap();
        assert_eq!(uncertain.status, EffectStatus::Uncertain);
        assert_eq!(uncertain.attempt, 1);
        let expected = expected_delivery(&c.f.source, &context.transfer, &context.attempt)
            .await
            .unwrap();
        assert!(expected.input.header().ingress_context.is_none());
        let native_before =
            c.f.machine
                .durable_admitted_original(&c.lead_session, &expected.input)
                .await
                .unwrap();
        assert_eq!(native_before.input_id(), &actual.input_id);
        assert!(Some(native_before.association()) == bound.header().authority_association.as_ref());
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
        assert!(
            c.f.machine
                .current_executor_attachment_witness(&c.lead_session)
                .await
                .is_none()
        );
        let native_retired =
            c.f.machine
                .durable_admitted_original(&c.lead_session, &expected.input)
                .await
                .unwrap();
        assert_eq!(native_retired.input_id(), &actual.input_id);
        assert!(native_retired.association() == native_before.association());
        let before = fixture::native_rows(&c).await;
        let requests = c.f.model_requests();
        // This reaches an actually admitted row under its unchanged canonical
        // event/association, then proves semantic replay conflicts on content.
        let Input::Prompt(mut changed) = expected.input.clone() else {
            panic!("delivery prompt")
        };
        changed.content =
            meerkat_core::types::ContentInput::Text("changed transferred context".into());
        assert!(matches!(
            accepted_only(
                c.f.machine
                    .durable_admitted_original(&c.lead_session, &Input::Prompt(changed))
                    .await
            ),
            Err(PrivateOriginalError::Conflict)
        ));
        let Input::Prompt(mut rekeyed) = expected.input.clone() else {
            panic!("delivery prompt")
        };
        rekeyed.header.idempotency_key = Some(IdempotencyKey::new("unadmitted-context-event"));
        assert!(matches!(
            accepted_only(
                c.f.machine
                    .durable_admitted_original(&c.lead_session, &Input::Prompt(rekeyed))
                    .await
            ),
            Ok(None)
        ));
        assert_eq!(
            c.f.store.get_effect(&effect_key).await.unwrap().unwrap(),
            uncertain
        );
        assert_eq!(fixture::native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), requests);
        let (_, stamp) = c.f.source.owner.publication().observe(|| ()).unwrap();
        let observed = context.transfer.advance_delivery().await.unwrap();
        assert_eq!(
            observed,
            WorkflowDeliveryObservation::Admitted {
                session_id: c.lead_session.to_string(),
                input_id: actual.input_id.to_string(),
            }
        );
        let applied = c.f.store.get_effect(&effect_key).await.unwrap().unwrap();
        assert_eq!(applied.status, EffectStatus::Succeeded);
        assert_eq!(applied.attempt, uncertain.attempt);
        assert_eq!(applied.intent, uncertain.intent);
        assert!(applied.lease.is_none());
        let Some(EffectOutcome::Succeeded { receipt }) = &applied.outcome else {
            panic!("durable Applied receipt")
        };
        let receipt: DeliveryReceipt = serde_json::from_value(receipt.body.clone()).unwrap();
        assert_eq!(receipt.input, actual.input_id);
        assert_eq!(receipt.session, actual.session_id);
        assert!(receipt.association == *native_retired.association());
        assert_eq!(
            context.deliver().await.unwrap(),
            observed,
            "public deliver is historical-only for this origin"
        );
        assert_eq!(
            context.transfer.reconcile_delivery().await.unwrap(),
            observed
        );
        assert_eq!(
            c.f.store.get_effect(&effect_key).await.unwrap().unwrap(),
            applied
        );
        stamp.check_current().unwrap();
        assert!(
            context.attempt.delivery.lock().unwrap().is_none(),
            "no new native custody"
        );
        assert_eq!(fixture::native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), requests);
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_absence_inexact_and_unavailable_evidence_never_enable_delivery_retry() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let context = restored(&c, &data, &lifecycle).await;
        let effect = delivery_effect(&c.f.source, &context.attempt)
            .await
            .unwrap();
        let snapshot =
            c.f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(&data.envelope.recipient.workflow)
                .await
                .unwrap();
        let EffectClaimOutcome::Claimed(claim) =
            c.f.store
                .claim_effect(
                    &effect.key,
                    snapshot.revision(),
                    "historical-unknown-native",
                    60_000,
                )
                .await
                .unwrap()
        else {
            panic!("actual claimed effect")
        };
        c.f.store
            .acknowledge_effect(
                claim.lease.as_ref().unwrap(),
                EffectOutcome::Uncertain {
                    evidence: RecordData {
                        schema: "test.no-native-outcome".into(),
                        version: 1,
                        body: serde_json::json!({}),
                    },
                },
            )
            .await
            .unwrap();
        let uncertain = c.f.store.get_effect(&effect.key).await.unwrap().unwrap();
        let before = fixture::native_rows(&c).await;
        let requests = c.f.model_requests();
        let expected = expected_delivery(&c.f.source, &context.transfer, &context.attempt)
            .await
            .unwrap();
        assert!(matches!(
            c.f.machine
                .durable_admitted_original(&expected.session, &expected.input)
                .await,
            Err(DurableAdmittedOriginalRefusal::NotAdmitted)
        ));
        for result in [
            context.transfer.advance_delivery().await,
            context.deliver().await,
            context.transfer.reconcile_delivery().await,
        ] {
            assert_eq!(result.unwrap(), WorkflowDeliveryObservation::Pending);
        }
        assert!(matches!(
            accepted_only(
                c.f.machine
                    .durable_admitted_original(&SessionId::new(), &expected.input)
                    .await
            ),
            Ok(None)
        ));
        assert!(matches!(
            accepted_only(
                MeerkatMachine::ephemeral()
                    .durable_admitted_original(&expected.session, &expected.input)
                    .await
            ),
            Err(PrivateOriginalError::Unavailable)
        ));
        let Input::Prompt(mut ungoverned) = expected.input.clone() else {
            panic!("delivery prompt")
        };
        ungoverned.header.authority_association = None;
        assert!(matches!(
            accepted_only(
                c.f.machine
                    .durable_admitted_original(&expected.session, &Input::Prompt(ungoverned))
                    .await
            ),
            Err(PrivateOriginalError::Conflict)
        ));
        assert_eq!(
            c.f.store.get_effect(&effect.key).await.unwrap().unwrap(),
            uncertain
        );
        assert!(context.attempt.delivery.lock().unwrap().is_none());
        assert_eq!(fixture::native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), requests);
        lifecycle.stop();
        assert!(matches!(
            context.transfer.advance_delivery().await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert_eq!(
            c.f.store.get_effect(&effect.key).await.unwrap().unwrap(),
            uncertain
        );
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}
