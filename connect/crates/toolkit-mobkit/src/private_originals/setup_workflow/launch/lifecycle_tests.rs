//! Actual native A/P/G and joined recovery owner. The readiness/delivery cases
//! use controlled protocol ports; approved_completion_tests instead observes a
//! real admitted service input through native finalization. Neither establishes
//! physical service installation, waiter disclosure or restart qualification.
use super::*;
use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
use meerkat_toolkit::{
    authority::{LifetimeScope, RoleLifetime, ServicePermission},
    binding::*,
    continuation::*,
    setup::{AgentSelection, ReadinessResult, SetupSnapshot},
};
use std::sync::atomic::AtomicUsize;

fn lifetime(deadline: u64) -> RoleLifetime {
    RoleLifetime {
        scope: LifetimeScope::Member {
            member: id("agent"),
        },
        expires_at_ms: Some(deadline),
    }
}
pub(super) fn readiness_ceiling(receiver: &Id, audience: &BTreeSet<Id>, deadline: u64) -> Ceiling {
    Ceiling {
        actions: [Action::FindAndInstall, Action::GrantService]
            .into_iter()
            .map(|action| ActionPermission {
                action,
                resource: receiver.clone(),
            })
            .collect(),
        services: [ServicePermission {
            target: receiver.clone(),
            connection: id("fixture-account"),
            operation: id("read"),
            resource: id("fixture-data"),
            lifetime: lifetime(deadline),
        }]
        .into(),
        frontends: BTreeSet::new(),
        output_audiences: audience.clone(),
    }
}
fn reference(label: &str) -> ContextReference {
    ContextReference {
        owner: id("lifecycle-fixture"),
        id: id(label),
        revision: revision("1"),
        content_digest: ContentDigest::of_bytes(label.as_bytes()),
    }
}
struct Readiness;
impl ContextReferenceOwner for Readiness {
    fn verify_access(
        &self,
        value: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        if *value == reference("decision") {
            Ok(())
        } else {
            Err(RequestError::ContextUnavailable)
        }
    }
}
impl GrantOwner for Readiness {
    fn current_grant(
        &self,
        intent: &BindingIntent,
        _: u64,
    ) -> Result<Option<GrantReceipt>, BindingError> {
        Ok(Some(GrantReceipt {
            id: id("fixture-grant"),
            owner: id("fixture-policy"),
            intent: intent.id.clone(),
            intent_revision: intent.revision.clone(),
            tenant: intent.tenant.clone(),
            deployment: intent.deployment.clone(),
            subject: intent.target.clone(),
            policy_revision: intent.policy_revision.clone(),
            manifest_digest: intent.manifest_digest.clone(),
            original_work: intent.original_work.clone(),
            intent_digest: intent.digest()?,
            role: intent.role.clone(),
            lifetime: intent.lifetime.clone(),
        }))
    }
}
impl BindingOwner for Readiness {
    fn current_binding(
        &self,
        intent: &BindingIntent,
        grant: &GrantReceipt,
        now: u64,
    ) -> Result<Option<BindingReceipt>, BindingError> {
        let RoleRequirement::Service {
            runtime,
            connection_revision,
            ..
        } = &intent.role
        else {
            return Err(BindingError::InvalidBinding);
        };
        Ok(Some(BindingReceipt {
            id: id("fixture-binding"),
            owner: id("fixture-runtime"),
            intent: intent.id.clone(),
            intent_revision: intent.revision.clone(),
            grant: grant.id.clone(),
            intent_digest: intent.digest()?,
            grant_digest: grant.digest()?,
            manifest_digest: intent.manifest_digest.clone(),
            policy_revision: intent.policy_revision.clone(),
            observed_at_ms: now,
            expires_at_ms: intent.request_valid_until_ms,
            evidence: RoleEvidence::Service {
                runtime: runtime.clone(),
                incarnation: id("fixture-incarnation"),
                catalog_revision: revision("1"),
                connection_revision: connection_revision.clone(),
            },
        }))
    }
}
async fn ready(f: &FirstFixture, c: &Case) -> SetupSnapshot {
    ready_operation(f, c, &id(&c.membership.acquisition().id)).await
}
async fn ready_operation(f: &FirstFixture, c: &Case, operation: &Id) -> SetupSnapshot {
    let task = NativeOriginalTask {
        origin_session_id: f.session.clone(),
        retained_work: c.a_dispatch.retained_work().unwrap().clone(),
        member: None,
    };
    let store = f.source.workflow_store().unwrap();
    let snapshot = store.service_task_snapshot(operation, &task).await.unwrap();
    let evidence = snapshot.request().evidence();
    let claims = evidence.association().candidate();
    let now = u64::try_from(f.store.now_ms().await.unwrap()).unwrap();
    let context = SetupUseContext {
        tenant: &evidence.constraints().tenant,
        deployment: &evidence.constraints().deployment,
        realm: &claims.ingress_namespace.realm,
        target: &claims.target,
        policy_revision: &evidence.constraints().policy_revision,
        now_ms: now,
        authorization: c.a_dispatch.work_authorization().unwrap(),
    };
    let deadline = snapshot.request().host().limits.deadline_ms;
    let intent = BindingIntent {
        id: id("fixture-service"),
        tenant: evidence.constraints().tenant.clone(),
        deployment: evidence.constraints().deployment.clone(),
        requester: claims.requester.clone(),
        target: snapshot.request().host().target.clone(),
        original_work: snapshot.request().original_work().clone(),
        manifest_digest: ContentDigest::of_bytes(b"fixture-service"),
        revision: revision("1"),
        policy_revision: evidence.constraints().policy_revision.clone(),
        request_valid_until_ms: deadline,
        lifetime: lifetime(deadline),
        role: RoleRequirement::Service {
            connection: id("fixture-account"),
            connection_revision: revision("1"),
            access: [ServiceAccess {
                operation: id("read"),
                resource: id("fixture-data"),
            }]
            .into(),
            runtime: id("fixture-runtime"),
        },
    };
    let lease = f
        .store
        .claim_record(
            snapshot.key(),
            snapshot.state().revision,
            "lifecycle-ready",
            60_000,
        )
        .await
        .unwrap();
    let selected = store
        .select(
            &snapshot,
            AgentSelection {
                decision: reference("decision"),
                requirements: vec![intent],
            },
            &lease,
            SetupAccess {
                evidence,
                context: &context,
                references: &Readiness,
            },
        )
        .await
        .unwrap();
    let ReadinessResult::Ready(terminal) = store
        .observe_ready(
            &selected,
            &lease,
            SetupAccess {
                evidence,
                context: &context,
                references: &Readiness,
            },
            &Readiness,
            &Readiness,
        )
        .await
        .unwrap()
    else {
        panic!("canonical Ready requires both controlled owner observations")
    };
    *terminal
}

// This controlled delivery owner retains a real protocol admission decision,
// not a native runtime input. A lost answer remains Pending until commit_late;
// reconciliation then observes that exact decision without another dispatch.
struct Delivery {
    automatic: bool,
    pending: Mutex<Option<ContinuationAdmissionEvidence>>,
    accepted: Mutex<Option<ContinuationAdmissionEvidence>>,
    dispatches: AtomicUsize,
}
impl Delivery {
    fn new(automatic: bool) -> Arc<Self> {
        Arc::new(Self {
            automatic,
            pending: Mutex::new(None),
            accepted: Mutex::new(None),
            dispatches: 0.into(),
        })
    }
    fn evidence(obligation: &ContinuationObligation) -> ContinuationAdmissionEvidence {
        let mut receipt = reference("result-receipt");
        receipt.owner = obligation.continuation().owner.clone();
        ContinuationAdmissionEvidence {
            receipt,
            obligation_digest: obligation.digest().unwrap(),
            delivery_identity: obligation.delivery_identity().unwrap(),
            runtime: id("fixture-delivery"),
            session: id("fixture-result-session"),
            durable_input: reference("fixture-result-input"),
            observed_at_ms: obligation.terminal_at_ms() + 1,
        }
    }
    fn commit_late(&self) {
        let value = self
            .pending
            .lock()
            .unwrap()
            .take()
            .expect("one pending result");
        assert!(self.accepted.lock().unwrap().replace(value).is_none());
    }
}
#[async_trait::async_trait]
impl SetupContinuationOwner for Delivery {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        let saved = self.accepted.lock().unwrap().clone();
        if let Some(value) = &saved {
            assert!(value.receipt == *receipt);
            assert_eq!(value.obligation_digest, obligation.digest()?);
            assert_eq!(value.delivery_identity, obligation.delivery_identity()?);
        }
        Ok(saved)
    }
}
#[async_trait::async_trait]
impl SetupContinuationDelivery for Delivery {
    async fn authorize_delivery(
        &self,
        _: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        Ok(())
    }
    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        assert_eq!(self.dispatches.fetch_add(1, Ordering::SeqCst), 0);
        let value = Self::evidence(obligation);
        if self.automatic {
            *self.accepted.lock().unwrap() = Some(value.clone());
            Ok(value)
        } else {
            *self.pending.lock().unwrap() = Some(value);
            Err(ContinuationError::OwnerUnavailable)
        }
    }
    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        let saved = self.accepted.lock().unwrap().clone();
        Ok(match saved {
            Some(value) => {
                assert_eq!(value.obligation_digest, obligation.digest()?);
                ContinuationReconciliation::Applied(value)
            }
            None => ContinuationReconciliation::Pending {
                evidence: reference("result-pending"),
            },
        })
    }
}
#[async_trait::async_trait]
impl crate::installed_host::acquisition::InstalledOriginalTaskOwner for Delivery {
    async fn original_task(
        &self,
        _: crate::installed_host::acquisition::InstalledOriginalInvocation<'_>,
    ) -> Result<TaskContinuation, crate::ToolAdmissionError> {
        panic!("lifecycle recovery must not request a new original task")
    }
}
fn options() -> ContinuationStepOptions {
    ContinuationStepOptions {
        worker: id("fixture-result"),
        lease_ms: 1000,
        owner_timeout_ms: 200,
        settlement_budget_ms: 200,
    }
}
async fn child_work(c: &Case, label: &str) -> SetupWorkContext {
    let child = claim(c._backend.service(), label).await;
    c._backend
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
    SetupWorkContext::from_owner(
        c._backend.pin().clone(),
        c.work.parent().clone(),
        work_ref(&child),
        c.snapshot.terms().parent_owner.clone(),
        child.owner.as_ref().unwrap().key.clone(),
        CYCLE.into(),
    )
    .unwrap()
}
async fn exercise_lifecycle(success: bool) {
    let f = FirstFixture::for_continuation().await;
    let recovery = AcquisitionRecovery::default();
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let deadline = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 60_000;
        let c = prepare_with_waiter_options(&f, success.then_some((deadline, true))).await;
        let admitted = native(c.planning.launch(&c.snapshot).await.unwrap());
        let input = admitted_input(&f, &admitted);
        f.finish(&Ok::<_, String>(admitted)).await.unwrap();
        let delivery = Delivery::new(success);
        let (membership, canonical_before, settled_before) = if success {
            f.source
                .retain_workflow_selection(
                    &c.a,
                    &id("waiter-a-call"),
                    Some(&c.planning),
                    Some(&c.work),
                )
                .unwrap();
            let terminal = ready(&f, &c).await;
            assert!(matches!(
                f.source
                    .workflow_store()
                    .unwrap()
                    .step_continuation(&id(&terminal.key().id), delivery.as_ref(), &options())
                    .await
                    .unwrap(),
                ContinuationStep::Acknowledged(_)
            ));
            let before = f.store.get(terminal.key()).await.unwrap().unwrap();
            assert!(!before.data.body["continuation_receipt"].is_null());
            let effects = f
                .store
                .list_effects(terminal.key(), None, 16)
                .await
                .unwrap();
            assert!(
                !f.source
                    .workflow_store()
                    .unwrap()
                    .load_setup_workflow(c.snapshot.reference())
                    .await
                    .unwrap()
                    .departed(&c.membership)
                    .unwrap(),
                "settled result still has stale durable membership before recovery"
            );
            (c.membership.clone(), before, Some(effects))
        } else {
            // Two acquisitions share actual A, but own different WorkGraph
            // children/deadlines. Only this second acquisition is due.
            let work = child_work(&c, "waiter-expiring").await;
            let due = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 1000;
            let (membership, _) = join_waiter_options(
                &f,
                &c.planning,
                &c.a,
                &c.a_dispatch,
                &c._backend,
                &work,
                "waiter-expiring",
                Some((due, false)),
            )
            .await;
            f.source
                .retain_workflow_selection(
                    &c.a,
                    &id("waiter-expiring-call"),
                    Some(&c.planning),
                    Some(&work),
                )
                .unwrap();
            let before = f
                .store
                .get(membership.acquisition())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(before.data.body["state"]["terminal_revision"], 0);
            (membership, before, None)
        };
        let context_delivery = f
            .store
            .get_effect(&membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        let owner =
            crate::installed_host::controller_first_input_fixture::bind_workflow_lifecycle_recovery(
                &f.store,
                &f.scope,
                &f.source,
                f.lifetime(),
                &f.source_path,
                delivery.clone(),
            );
        recovery
            .start(recovery.prepare_start().unwrap(), owner)
            .unwrap();
        loop {
            let workflow = f
                .source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(membership.workflow())
                .await
                .unwrap();
            if workflow.departed(&membership).unwrap() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        f.source.owner.workflow_controller(&c.a.original).unwrap();
        f.source
            .owner
            .workflow_controller(&c.planning.parent)
            .unwrap();
        if success {
            let close = loop {
                let close = f
                    .store
                    .get_effect(&membership.workflow().close_effect())
                    .await
                    .unwrap()
                    .unwrap();
                if close.status == EffectStatus::Succeeded {
                    break close;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            let Some(EffectOutcome::Succeeded { receipt }) = close.outcome else {
                panic!("close receipt")
            };
            assert_eq!(
                serde_json::from_value::<WorkflowCloseObservation>(receipt.body).unwrap(),
                WorkflowCloseObservation::SourceFencedAndFinalized {
                    session_id: f.session.to_string(),
                    input_id: input.to_string()
                }
            );
            assert_eq!(
                f.store
                    .get(membership.acquisition())
                    .await
                    .unwrap()
                    .unwrap(),
                canonical_before
            );
            assert_eq!(
                f.store
                    .list_effects(membership.acquisition(), None, 16)
                    .await
                    .unwrap(),
                settled_before.unwrap()
            );
        } else {
            let pending = loop {
                let row = f
                    .store
                    .get(membership.acquisition())
                    .await
                    .unwrap()
                    .unwrap();
                if delivery.pending.lock().unwrap().is_some() {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            assert_eq!(pending.data.body["state"]["lifecycle_phase"], "expired");
            assert_eq!(
                pending.data.body["request"],
                canonical_before.data.body["request"]
            );
            assert!(pending.data.body["continuation_receipt"].is_null());
            let obligation = f
                .source
                .workflow_store()
                .unwrap()
                .committed_continuation(&id(&membership.acquisition().id))
                .await
                .unwrap()
                .unwrap();
            let saved_digest = obligation.digest().unwrap();
            let workflow = f
                .source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(membership.workflow())
                .await
                .unwrap();
            assert_eq!(workflow.active_waiters(), 1);
            assert!(!workflow.departed(&c.membership).unwrap());
            assert!(
                f.store
                    .get_effect(&membership.workflow().close_effect())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                c.planning
                    .interests
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|i| i.membership == c.membership)
                    .is_some_and(|i| !i.closed)
            );
            // Late C remains independent and starts with an undisclosed input;
            // expiry of another acquisition on A cannot seal the live workflow.
            let work = child_work(&c, "waiter-late-c").await;
            let (late, joined) = join_waiter(
                &f,
                &c.planning,
                &c.a,
                &c.a_dispatch,
                &c._backend,
                &work,
                "waiter-late-c",
            )
            .await;
            assert_eq!(joined.active_waiters(), 2);
            assert_ne!(late.acquisition(), membership.acquisition());
            let late_effect = f
                .store
                .get_effect(&late.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(late_effect.status, EffectStatus::Pending);
            assert_eq!(late_effect.attempt, 0);
            // Complete the protocol owner's actual held decision after waiter
            // departure. The same recovery owner must acknowledge that exact
            // legitimate pending result, without any second delivery dispatch.
            delivery.commit_late();
            let continued = loop {
                let row = f
                    .store
                    .get(membership.acquisition())
                    .await
                    .unwrap()
                    .unwrap();
                if !row.data.body["continuation_receipt"].is_null() {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            assert_eq!(
                continued.data.body["continuation_receipt"]["obligation_digest"],
                saved_digest.to_string()
            );
            assert_eq!(continued.data.body["state"]["lifecycle_phase"], "expired");
            assert_eq!(
                continued.data.body["request"],
                canonical_before.data.body["request"]
            );
            f.source.owner.workflow_controller(&c.a.original).unwrap();
            assert_eq!(
                f.store
                    .get_effect(&late.delivery_effect().unwrap())
                    .await
                    .unwrap()
                    .unwrap(),
                late_effect
            );
        }
        assert_eq!(delivery.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.store
                .get_effect(&membership.delivery_effect().unwrap())
                .await
                .unwrap()
                .unwrap(),
            context_delivery,
            "terminal departure cannot acknowledge undisclosed waiter context"
        );
        assert_eq!(
            f.inputs().await.unwrap(),
            3,
            "A, P and one G only; result owner is controlled"
        );
    }))
    .catch_unwind()
    .await;
    recovery.stop();
    let joined = recovery.join().await;
    let cleanup = f.shutdown().await;
    assert!(joined.is_ok(), "{joined:?}");
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded lifecycle recovery"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_worker_departs_exact_acquisition_and_preserves_pending_result_and_late_c() {
    exercise_lifecycle(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ready_result_already_acknowledged_is_recovered_without_replay_or_original_cancellation() {
    exercise_lifecycle(true).await;
}

#[path = "lifecycle_tests/selection_tests.rs"]
mod selection_tests;

#[path = "lifecycle_tests/approved_completion_tests.rs"]
mod approved_completion_tests;
