//! Real Core R2 consumption, real G admission and real SQLite effect custody.
//! The local probe's reviewed effect hands custody to these delivery owners.
//! The committed WorkGraph envelope is controlled fixture data, not proof of
//! a Find-fenced commit. Deferred acceptance below is a controlled settlement
//! callback failure, not a native transport AdmissionUnresolved result.
use super::super::restore_tests::{self as fixture, Fixture, WAIT};
use super::*;
use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
use futures::FutureExt;
use meerkat_core::{
    AgentToolDispatcher, ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolDef,
    ToolError, ToolExecutionResolutionContext, ToolResult,
    approval::review::{
        BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
        ReviewCandidate, ReviewEntrySupport, ReviewObservation, ReviewVerdict, ReviewerFailure,
    },
    authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationReviewTier,
        PreparedAuthorizationBinding, PreparedOperationAuthorization,
        ReviewOperationContextResolution, WorkAuthorization, WorkAuthorizationContext,
    },
};
use meerkat_mobkit::identity_first::{TurnOutcome, TurnOutput, TurnTracking};
use std::{panic::AssertUnwindSafe, time::Duration};
use toolkit_store::ToolkitStore;

mod native_ack_loss;
mod sqlite_worker;

const PROBE: &str = "workflow_delivery_custody_probe";

// This narrow local policy selects its own effect. It grants no Find,
// workflow, G/P or Console authority and never substitutes a native owner.
struct ProbePolicy;
struct ProbeDecision(PreparedAuthorizationBinding);
impl WorkAuthorization for ProbePolicy {
    fn review_operation_context(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        if tool.name != PROBE || tool.arguments.get() != "{}" {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(Arc::new(ProbeDecision(binding.clone())))
    }
}
impl PreparedOperationAuthorization for ProbeDecision {
    fn spend_at_entry(&self) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R2
    }
    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if !binding.same_operation(&self.0) {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(())
    }
}
struct Allow(WorkAuthorizationContext);
#[async_trait::async_trait]
impl OperationReviewer for Allow {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        assert!(candidate.work_authorization().same_context(&self.0));
        let AuthorizationOperation::Tool(tool) = &candidate.binding().facts().operation else {
            panic!("only the exact local probe is reviewed")
        };
        assert_eq!(tool.name, PROBE);
        assert_eq!(tool.arguments.get(), "{}");
        Ok(ReviewVerdict::Allow)
    }
}
#[derive(Default)]
struct Observations {
    rows: Mutex<Vec<ReviewObservation>>,
    used: tokio::sync::Notify,
}
impl OperationReviewObserver for Observations {
    fn observe(&self, observation: ReviewObservation) {
        let used = observation.status == ReviewAttemptStatus::Used;
        self.rows.lock().unwrap().push(observation);
        if used {
            self.used.notify_one();
        }
    }
}
impl Observations {
    fn used_count(&self) -> usize {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.status == ReviewAttemptStatus::Used)
            .count()
    }
    async fn exactly_used(&self, id: &str) {
        tokio::time::timeout(WAIT, self.used.notified())
            .await
            .unwrap();
        let rows = self.rows.lock().unwrap();
        let used: Vec<_> = rows
            .iter()
            .filter(|row| row.status == ReviewAttemptStatus::Used)
            .collect();
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].tool_call_id.as_ref(), id);
    }
}
#[derive(Default)]
struct Probe(Mutex<Option<ReviewCustody>>);
#[async_trait::async_trait]
impl AgentToolDispatcher for Probe {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(
            ToolDef::new(
                PROBE,
                "retain the local reviewed effect in a delivery owner",
                serde_json::json!({"type":"object"}),
            )
            .with_provenance(meerkat_core::ToolProvenance::new(
                meerkat_core::ToolSourceKind::Builtin,
                "workflow-delivery-custody-control",
            )),
        )]
        .into()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let (_, held) = context.enter_reviewed_effect_held(call, Some(plan))?;
        assert!(
            self.0
                .lock()
                .unwrap()
                .replace(ReviewCustody::entered(held))
                .is_none()
        );
        Ok(ToolResult::new(
            call.id.to_owned(),
            "actual review custody handed to the local owner".into(),
            false,
        )
        .into())
    }
}
async fn consumed(id: &str) -> (ReviewCustody, Arc<Observations>) {
    let work = WorkAuthorizationContext::new(
        Arc::new(ProbePolicy),
        meerkat_core::OperationExecutionScope::Domain,
    );
    let observed = Arc::new(Observations::default());
    let review = Arc::new(
        BoundOperationReview::new(
            Arc::new(Allow(work.clone())),
            meerkat_core::ApprovalService::new(),
            WAIT,
        )
        .with_observer(observed.clone()),
    );
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(work))
        .with_operation_review(Some(review));
    let probe = Arc::new(Probe::default());
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id,
        name: PROBE,
        args: &args,
    };
    let resolution = ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            WAIT,
        )])
        .unwrap(),
    );
    let plan =
        meerkat_core::resolve_tool_execution_plan_fenced(&probe, call, &context, &resolution)
            .unwrap();
    meerkat_core::dispatch_tool_execution_plan_fenced(&probe, call, &context, &plan)
        .await
        .unwrap();
    let custody = probe
        .0
        .lock()
        .unwrap()
        .take()
        .expect("actual consumed native entry");
    drop(probe);
    drop(context);
    assert_eq!(
        observed.used_count(),
        0,
        "native dispatch returned but the actual effect still owns custody"
    );
    (custody, observed)
}

// The real accepted callback transfers the original settlement to the actual
// production unresolved owner. The native input itself is still admitted.
struct DeferredBinder(Arc<DeliveryBinder>);
struct DeferredSettlement {
    inner: Box<dyn GovernedSettlement>,
    binder: Arc<DeliveryBinder>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for DeferredBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        let BoundGovernedInput {
            input,
            settlement,
            entry_fence,
        } = self.0.try_bind(context, input).await?;
        Ok(BoundGovernedInput {
            input,
            entry_fence,
            settlement: Box::new(DeferredSettlement {
                inner: settlement,
                binder: self.0.clone(),
            }),
        })
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.0.retain_unresolved(settlement, detail);
    }
}
impl GovernedSettlement for DeferredSettlement {
    fn accepted(self: Box<Self>, _: &GovernedAdmission) -> Result<(), String> {
        self.binder
            .retain_unresolved(self.inner, "controlled accepted-settlement deferral");
        Err("controlled accepted-settlement deferral".into())
    }
}
struct RefusedBinder(Arc<DeliveryBinder>);
struct RefusedFence;
impl GovernedEntryFence for RefusedFence {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        Err(GovernedEntryRefusal::Retired)
    }
}
#[async_trait::async_trait]
impl GovernedInputBinder for RefusedBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        let mut bound = self.0.try_bind(context, input).await?;
        bound.entry_fence = Some(Arc::new(RefusedFence));
        Ok(bound)
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.0.retain_unresolved(settlement, detail);
    }
}

fn slot(context: &InstalledWorkflowContext) -> Arc<DeliverySlot> {
    let (input, event) = delivery_input(&context.attempt).unwrap();
    let slot = Arc::new(DeliverySlot {
        input,
        event,
        workflow: context.workflow.clone(),
        bound: OnceLock::new(),
        run: Mutex::new(None),
        native: Mutex::new(None),
    });
    assert!(
        context
            .attempt
            .delivery
            .lock()
            .unwrap()
            .replace(slot.clone())
            .is_none()
    );
    slot
}
async fn context(c: &Fixture, lifecycle: &AcquisitionRecovery) -> InstalledWorkflowContext {
    let data = fixture::prepare(c).await;
    fixture::store_envelope(&data).await;
    match fixture::restore(c, &data, lifecycle).await.unwrap() {
        WorkflowContextObservation::Committed(context) => context,
        _ => panic!("actual committed envelope observation"),
    }
}
struct Pause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Pause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }
}
struct Release(Arc<Pause>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release.notify_one();
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Defer,
    Refuse,
    FailAfterClaim,
}

fn entered_run(
    c: &Fixture,
    context: &InstalledWorkflowContext,
    slot: &Arc<DeliverySlot>,
    review: ReviewCustody,
    mode: Mode,
    pause: Option<Arc<Pause>>,
) -> Arc<DeliveryRun> {
    let source = c.f.source.clone();
    let transfer = context.transfer.clone();
    let attempt = context.attempt.clone();
    let selected = slot.clone();
    let control = context
        .attempt
        .recovery
        .upgrade()
        .expect("same actual transfer recovery owner");
    control
        .enter_transfer(|| {
            DeliveryRun::retain_or_start(
                &mut slot.run.lock().unwrap(),
                move |task_native| async move {
                    review
                        .retain_io(async {
                            let effect = selected.effect(&source, &attempt).await?;
                            let snapshot = source
                                .workflow_store()?
                                .load_setup_workflow(&transfer.selected.workflow)
                                .await
                                .map_err(|_| PrivateOriginalError::Unavailable)?;
                            let (native, lease) = claim_delivery_effect_with_review(
                                source.store.as_ref(),
                                &effect,
                                snapshot.revision(),
                                &selected.native,
                                &task_native,
                                review.clone(),
                            )
                            .await?
                            .unwrap();
                            if let Some(pause) = pause {
                                pause.entered.notify_one();
                                pause.release.notified().await;
                            }
                            if matches!(mode, Mode::FailAfterClaim) {
                                return Err(PrivateOriginalError::Unavailable);
                            }
                            let planning = transfer.planning()?;
                            let route = source.route(&transfer.selected.planning_source.route)?;
                            let inner = Arc::new(DeliveryBinder {
                                owner: source.owner.clone(),
                                original: selected.workflow.planning.clone(),
                                slot: selected.clone(),
                                native: native.clone(),
                                controller: source
                                    .owner
                                    .workflow_controller(&selected.workflow.planning)
                                    .map_err(custody_error)?,
                                authentication: evidence(
                                    &route.authentication_domain,
                                    &planning.submission.transport_authentication,
                                )?,
                                planning,
                            });
                            let binder: Arc<dyn GovernedInputBinder> = match mode {
                                Mode::Defer => Arc::new(DeferredBinder(inner)),
                                Mode::Refuse => Arc::new(RefusedBinder(inner)),
                                Mode::FailAfterClaim => unreachable!(),
                            };
                            let runtime = source.bound_identity_runtime()?;
                            let identity = runtime
                                .owned_identity_for_member_alias(&selected.workflow.member.member)
                                .await
                                .unwrap();
                            *native.decision.lock().unwrap() = NativeDecision::Submitted;
                            let actual = runtime
                                .dispatch_governed_with_turn_ticket(
                                    &identity,
                                    Some(&selected.workflow.member.member),
                                    selected.input.clone(),
                                    review.bind(binder),
                                )
                                .await;
                            match mode {
                                Mode::Defer => {
                                    let actual = actual.unwrap();
                                    assert!(
                                        actual.settlement.is_err()
                                            && !actual.admission.deduplicated
                                    );
                                    assert_eq!(
                                        actual.admission.session_id,
                                        selected.workflow.session
                                    );
                                    assert_eq!(actual.admission.input_id, *selected.input.id());
                                    assert!(native.custody.pending.lock().unwrap().is_some());
                                    assert_eq!(native.custody.unresolved.lock().unwrap().len(), 1);
                                    assert!(native.custody.accepted.get().is_none());
                                    *native.decision.lock().unwrap() = NativeDecision::Accepted;
                                    let TurnTracking::Tracked(ticket) = actual.ticketed.turn else {
                                        panic!("actual admitted turn ticket")
                                    };
                                    assert!(matches!(
                                        runtime.wait_for_turn(&identity, ticket, WAIT).await,
                                        TurnOutcome::Completed {
                                            output: TurnOutput::Text { .. }
                                        }
                                    ));
                                }
                                Mode::Refuse => {
                                    assert!(matches!(
                                        actual,
                                        Err(IdentityRuntimeError::GovernedInput(
                                            GovernedInputError::EntryRefused(
                                                GovernedEntryRefusal::Retired
                                            )
                                        ))
                                    ));
                                    *native.decision.lock().unwrap() = NativeDecision::NotAdmitted;
                                    // Same concrete cleanup as the production definite refusal arm.
                                    native.custody.pending.lock().unwrap().take();
                                    assert!(native.custody.unresolved.lock().unwrap().is_empty());
                                }
                                Mode::FailAfterClaim => unreachable!(),
                            }
                            let reply = source
                                .store
                                .acknowledge_effect(
                                    &lease,
                                    EffectOutcome::Uncertain {
                                        evidence: selected.receipt(&attempt)?,
                                    },
                                )
                                .await
                                .map_err(store_error)?;
                            assert_eq!(reply.status, EffectStatus::Uncertain);
                            Ok(WorkflowDeliveryObservation::Pending)
                        })
                        .await
                },
            )
        })
        .expect("actual registration gate accepted this task")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deferred_actual_delivery_settlement_keeps_review_through_default_reconciliation() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let (review, observations) = consumed("deferred-delivery").await;
        let run = entered_run(&c, &context, &slot, review, Mode::Defer, None);
        assert_eq!(
            tokio::time::timeout(WAIT, run.join())
                .await
                .unwrap()
                .unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert_eq!(observations.used_count(), 0);
        let native = slot.native.lock().unwrap().clone().unwrap();
        assert!(native.joined.load(Ordering::Acquire));
        assert!(native.custody.review.lock().unwrap().is_some());
        let before = fixture::native_rows(&c).await;
        let requests = c.f.model_requests();
        let observed = slot
            .execute(
                c.f.source.clone(),
                context.transfer.clone(),
                context.attempt.clone(),
                false,
                ReviewCustody::default(),
            )
            .await
            .unwrap();
        assert_eq!(observed, slot.admitted());
        observations.exactly_used("deferred-delivery").await;
        assert!(native.custody.pending.lock().unwrap().is_none());
        assert!(native.custody.unresolved.lock().unwrap().is_empty());
        assert!(native.custody.entry.get().unwrap().settled());
        assert!(native.custody.review.lock().unwrap().is_none());
        let applied = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert_eq!(applied.status, EffectStatus::Succeeded);
        assert_eq!(applied.attempt, 1);
        assert_eq!(fixture::native_rows(&c).await, before);
        assert_eq!(c.f.model_requests(), requests);
        assert_eq!(
            slot.execute(
                c.f.source.clone(),
                context.transfer.clone(),
                context.attempt.clone(),
                false,
                ReviewCustody::default()
            )
            .await
            .unwrap(),
            observed
        );
        assert_eq!(observations.used_count(), 1);
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_native_refusal_and_not_applied_writer_release_delivery_review() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let before = fixture::native_rows(&c).await;
        let (review, observations) = consumed("refused-delivery").await;
        let run = entered_run(&c, &context, &slot, review, Mode::Refuse, None);
        assert_eq!(
            tokio::time::timeout(WAIT, run.join())
                .await
                .unwrap()
                .unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert_eq!(
            observations.used_count(),
            0,
            "definite native refusal alone did not acknowledge the effect"
        );
        assert_eq!(
            slot.execute(
                c.f.source.clone(),
                context.transfer.clone(),
                context.attempt.clone(),
                false,
                ReviewCustody::default()
            )
            .await
            .unwrap(),
            WorkflowDeliveryObservation::NotApplied
        );
        observations.exactly_used("refused-delivery").await;
        let native = slot.native.lock().unwrap().clone().unwrap();
        let pending = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert!(native.can_retry(&pending).unwrap());
        assert!(native.not_applied.get().is_some());
        assert!(native.custody.review.lock().unwrap().is_none());
        assert_eq!(fixture::native_rows(&c).await, before);
        assert_eq!(pending.attempt, 1);
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_delivery_success_releases_review_after_actual_effect_acknowledgement() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let (review, observations) = consumed("successful-delivery").await;
        let observed = slot
            .execute(
                c.f.source.clone(),
                context.transfer.clone(),
                context.attempt.clone(),
                true,
                review,
            )
            .await
            .unwrap();
        assert_eq!(observed, slot.admitted());
        observations.exactly_used("successful-delivery").await;
        let native = slot.native.lock().unwrap().clone().unwrap();
        assert!(native.joined.load(Ordering::Acquire));
        assert!(native.custody.accepted.get().is_some());
        assert!(native.custody.review.lock().unwrap().is_none());
        let effect = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Succeeded);
        assert_eq!(
            effect.outcome,
            Some(EffectOutcome::Succeeded {
                receipt: slot.receipt(&context.attempt).unwrap()
            })
        );
        assert!(effect.lease.is_none());
    })
    .catch_unwind()
    .await;
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_delivery_observer_cannot_replace_the_entered_review() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let pause = Pause::new();
    let release = Release(pause.clone());
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let (first, observations) = consumed("first-delivery").await;
        let run = entered_run(&c, &context, &slot, first, Mode::Defer, Some(pause.clone()));
        tokio::time::timeout(WAIT, pause.entered.notified())
            .await
            .unwrap();
        let (later, discarded) = consumed("later-delivery-observer").await;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                slot.execute(
                    c.f.source.clone(),
                    context.transfer.clone(),
                    context.attempt.clone(),
                    true,
                    later
                )
            )
            .await
            .is_err()
        );
        discarded.exactly_used("later-delivery-observer").await;
        assert_eq!(observations.used_count(), 0);
        assert!(Arc::ptr_eq(
            &slot.run.lock().unwrap().clone().unwrap(),
            &run
        ));
        assert_eq!(
            slot.effect(&c.f.source, &context.attempt)
                .await
                .unwrap()
                .attempt,
            1
        );
        pause.release.notify_one();
        assert_eq!(
            tokio::time::timeout(WAIT, run.join())
                .await
                .unwrap()
                .unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert_eq!(observations.used_count(), 0);
        assert_eq!(
            slot.execute(
                c.f.source.clone(),
                context.transfer.clone(),
                context.attempt.clone(),
                false,
                ReviewCustody::default()
            )
            .await
            .unwrap(),
            slot.admitted()
        );
        observations.exactly_used("first-delivery").await;
        assert_eq!(discarded.used_count(), 1);
    })
    .catch_unwind()
    .await;
    drop(release);
    fixture::close(&c, &lifecycle, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopped_full_join_waits_for_other_owner_before_finalizing_failed_delivery_review() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let pause = Pause::new();
    let release = Release(pause.clone());
    let body = AssertUnwindSafe(async {
        let context = context(&c, &lifecycle).await;
        let slot = slot(&context);
        let (review, observations) = consumed("failed-delivery").await;
        let failed = entered_run(&c, &context, &slot, review, Mode::FailAfterClaim, None);
        assert_eq!(
            tokio::time::timeout(WAIT, failed.join()).await.unwrap(),
            Err(PrivateOriginalError::Unavailable)
        );
        assert_eq!(observations.used_count(), 0);
        c.f.source
            .join_workflow_context_transfers(true)
            .await
            .unwrap();
        c.f.source
            .join_workflow_context_transfers(false)
            .await
            .unwrap();
        assert_eq!(
            observations.used_count(),
            0,
            "ordinary joins while registration is open retain uncertainty"
        );
        let original = slot.effect(&c.f.source, &context.attempt).await.unwrap();
        assert_eq!(original.status, EffectStatus::InFlight);
        let source = c.f.source.clone();
        let selected = slot.clone();
        let attempt = context.attempt.clone();
        let task_pause = pause.clone();
        let advance = DeliveryRun::register_advance(
            &lifecycle.transfer_control(),
            &context.attempt.advance,
            move || async move {
                let review = selected.review_custody()?;
                review
                    .retain_io(async {
                        let observed = selected.effect(&source, &attempt).await?;
                        assert_eq!(observed.status, EffectStatus::InFlight);
                        task_pause.entered.notify_one();
                        task_pause.release.notified().await;
                        Err(PrivateOriginalError::Conflict)
                    })
                    .await
            },
        )
        .unwrap();
        tokio::time::timeout(WAIT, pause.entered.notified())
            .await
            .unwrap();
        lifecycle.stop();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                c.f.source.join_workflow_context_transfers(false)
            )
            .await
            .is_err()
        );
        assert_eq!(
            observations.used_count(),
            0,
            "stop and the other cached failure did not join this live owner"
        );
        pause.release.notify_one();
        c.f.source
            .join_workflow_context_transfers(false)
            .await
            .unwrap();
        observations.exactly_used("failed-delivery").await;
        assert_eq!(advance.join().await, Err(PrivateOriginalError::Conflict));
        assert_eq!(failed.join().await, Err(PrivateOriginalError::Unavailable));
        assert_eq!(
            slot.effect(&c.f.source, &context.attempt).await.unwrap(),
            original,
            "failed-owner finalization preserved actual uncertain custody/effect evidence"
        );
        assert!(
            slot.native
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .custody
                .review
                .lock()
                .unwrap()
                .is_none()
        );
        assert!(
            lifecycle
                .transfer_control()
                .enter_transfer(|| panic!("no registration after stop"))
                .is_none()
        );
    })
    .catch_unwind()
    .await;
    drop(release);
    fixture::close(&c, &lifecycle, body).await;
}
