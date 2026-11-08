//! G admission only: host-authored bytes, never waiter disclosure or effects.
//! Actual native source and contributor checks decide the negative deliveries.
use super::*;
use crate::original_ingress::{
    PendingToolkitOriginal, RetainedToolkitOriginal, ToolkitOriginalOwner,
};
use meerkat_authorization_contracts::evidence::HistoricalEvidenceRef;
use meerkat_core::{ControllerModelClient, SessionId, lifecycle::InputId};
use meerkat_mobkit::identity_first::{GovernedAdmission, GovernedSendOutcome};
use meerkat_runtime::{SessionServiceRuntimeExt, identifiers::IdempotencyKey};

// These are test observations and custody for this actual native delivery.
// They cannot produce an association, a current grant or an admission receipt.
#[derive(Default)]
struct FollowupCustody {
    pending: Mutex<Option<PendingToolkitOriginal>>,
    accepted: OnceLock<GovernedAdmission>,
}
struct FollowupSettlement {
    input: InputId,
    session: SessionId,
    custody: Arc<FollowupCustody>,
}
impl GovernedSettlement for FollowupSettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        if admission.deduplicated
            || admission.input_id != self.input
            || admission.session_id != self.session
        {
            return Err("followup acknowledgement did not match; fixture retains custody".into());
        }
        let pending = self
            .custody
            .pending
            .lock()
            .unwrap()
            .take()
            .ok_or("followup source was already settled")?;
        pending
            .admit()
            .map_err(|_| "followup source settlement failed")?;
        self.custody
            .accepted
            .set(admission.clone())
            .map_err(|_| "followup acceptance was already observed".into())
    }
}

struct FollowupBinder {
    owner: Arc<ToolkitOriginalOwner>,
    original: Arc<RetainedToolkitOriginal>,
    session: SessionId,
    controller: ControllerModelClient,
    authentication: HistoricalEvidenceRef,
    event: IdempotencyKey,
    bound: OnceLock<Input>,
    custody: Arc<FollowupCustody>,
    unresolved: Mutex<Vec<Box<dyn GovernedSettlement>>>,
}
impl FollowupBinder {
    fn new(c: &DedicatedFixture, held: &Arc<HeldPrivateOriginal>, event: &str) -> Arc<Self> {
        let route = c.f.source.route(&c.lead.original.route).unwrap();
        Arc::new(Self {
            owner: c.f.source.owner.clone(),
            original: held.original.clone(),
            session: c.member.session().unwrap(),
            controller: c
                .f
                .source
                .owner
                .workflow_controller(&held.original)
                .unwrap(),
            authentication: evidence(
                &route.authentication_domain,
                &c.lead.transport_authentication,
            )
            .unwrap(),
            event: IdempotencyKey::new(event),
            bound: OnceLock::new(),
            custody: Arc::new(FollowupCustody::default()),
            unresolved: Mutex::new(Vec::new()),
        })
    }
}
#[async_trait::async_trait]
impl GovernedInputBinder for FollowupBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let incoming_route = context
            .controller
            .plain_facts()
            .map_err(|_| "followup incoming controller facts unavailable")?
            .route_key();
        let retained_route = self
            .controller
            .plain_facts()
            .map_err(|_| "followup retained controller facts unavailable")?
            .route_key();
        if context.session_id != self.session || incoming_route != retained_route {
            return Err("followup destination changed".into());
        }
        // No authority precheck or alternate source is supplied here. The
        // public owner binds the retained G to the actual new native input.
        let (input, pending) = self
            .owner
            .bind_authenticated_input(
                &self.original,
                &context.session_id,
                input,
                self.event.clone(),
                self.authentication.clone(),
                context.controller.clone(),
            )
            .map_err(|_| "followup binding refused")?;
        self.bound
            .set(input.clone())
            .map_err(|_| "followup binder called twice")?;
        assert!(
            self.custody
                .pending
                .lock()
                .unwrap()
                .replace(pending)
                .is_none()
        );
        Ok(BoundGovernedInput {
            settlement: Box::new(FollowupSettlement {
                input: input.id().clone(),
                session: context.session_id.clone(),
                custody: self.custody.clone(),
            }),
            input,
            entry_fence: None,
        })
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, _: &str) {
        self.unresolved.lock().unwrap().push(settlement);
    }
}

fn denied(result: &Result<GovernedSendOutcome, IdentityRuntimeError>, session: &SessionId) {
    assert!(
        matches!(result,
        Err(IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(error)))
        if matches!(error.as_ref(), meerkat_mob::MobError::MemberInputRefused {
            session_id, error: meerkat_runtime::RuntimeDriverError::InputRefused { refusal },
        } if session_id == session && refusal.kind() == meerkat_core::OperationRefusalKind::Denied)),
        "actual exact-session native admission must refuse Denied"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_host_input_keeps_dedicated_g_and_refuses_foreign_and_cancelled_parent() {
    let c = DedicatedFixture::new().await;
    // Retain test settlement custody until the actual fixture shutdown joins,
    // including if an assertion or an unresolved native observation fails.
    let mut binders = Vec::new();
    let checked = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let f = &c.f;
        let a = f.submit().await;
        f.finish(&a).await.unwrap();
        let dispatch = f.probe.as_ref().unwrap().captured();
        let original = f.source.acquisition_original(&dispatch).await.unwrap();
        let p = f
            .source
            .submit_authenticated_original(c.parent.clone())
            .await
            .unwrap();
        finish(f, "planning_parent", &p).await;
        let planning = f
            .source
            .prepare_workflow_planning(
                &c.parent.original,
                c.lead.clone(),
                c.terms.clone(),
                c.lineage.clone(),
            )
            .await
            .unwrap();
        let (membership, snapshot) = join_waiter(
            f, &planning, &original, &dispatch, &c.backend, &c.work, "waiter-a",
        )
        .await;
        let first = native(planning.launch(&snapshot).await.unwrap());
        finish(f, "planning_lead", &first).await;
        custody_promoted(f, &c.lead.original).await;
        let PrivateOriginalSubmission::Admitted {
            session_id,
            input_id,
            ..
        } = first
        else {
            panic!("actual G admission")
        };
        assert_eq!(session_id, c.lead_session.to_string());
        let first_id = InputId::from_uuid(input_id.parse().unwrap());
        let held = f
            .source
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|held| held.reference == c.lead.original)
            .cloned()
            .unwrap();
        assert_eq!(
            held.original.association().candidate().contributing_work,
            vec![
                planning
                    .parent
                    .association()
                    .candidate()
                    .original_work
                    .clone()
            ]
        );
        let workflow = held.setup_workflow.get().unwrap().clone();
        assert!(workflow.current_source().is_ok());
        let initial = f
            .machine
            .input_state(&c.lead_session, &first_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(initial.state.authority_contributors.len(), 1);
        assert!(
            initial.state.authority_contributors[0].association() == held.original.association()
        );
        assert!(matches!(
            &initial.state.scoped_destination,
            meerkat_runtime::input_state::ScopedDestinationWitness::Bound { .. }
        ));
        let a_history = history(f, &f.session).await;
        let p_history = history(f, &c.parent_session).await;
        let first_history = history(f, &c.lead_session).await;
        let source_row = f
            .store
            .get(&c.terms.planning_source)
            .await
            .unwrap()
            .unwrap();
        let delivery = f
            .store
            .get_effect(&membership.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(delivery.status, EffectStatus::Pending);
        assert_eq!(delivery.attempt, 0);
        let runtime = f.source.bound_identity_runtime().unwrap();
        let identity = AgentIdentity::parse("planning_lead").unwrap();
        let input = Input::Prompt(PromptInput::new("host-authored planning followup", None));
        let second_id = input.id().clone();
        assert_ne!(second_id, first_id);
        let binder = FollowupBinder::new(&c, &held, "dedicated-host-followup");
        binders.push(binder.clone());
        let accepted = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some("planning_lead"),
                input.clone(),
                binder.clone(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.admission.session_id, c.lead_session);
        assert_eq!(accepted.admission.input_id, second_id);
        assert!(!accepted.admission.deduplicated);
        assert!(accepted.settlement.is_ok());
        assert_eq!(binder.custody.accepted.get(), Some(&accepted.admission));
        assert!(binder.custody.pending.lock().unwrap().is_none());
        let TurnTracking::Tracked(ticket) = accepted.ticketed.turn else {
            panic!("actual second G ticket")
        };
        assert!(matches!(
            runtime.wait_for_turn(&identity, ticket, GUARD).await,
            TurnOutcome::Completed {
                output: TurnOutput::Text { .. }
            }
        ));
        let second = f
            .machine
            .input_state(&c.lead_session, &second_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.state.input_id, second_id);
        assert_eq!(second.state.authority_contributors.len(), 1);
        assert_eq!(
            second.state.authority_contributors[0].input_id(),
            &second_id
        );
        assert!(
            second.state.authority_contributors[0].association() == held.original.association()
        );
        assert_eq!(
            second.state.scoped_destination,
            initial.state.scoped_destination
        );
        assert!(Arc::ptr_eq(held.setup_workflow.get().unwrap(), &workflow));
        assert!(workflow.current_source().is_ok());
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        let second_history = history(f, &c.lead_session).await;
        assert_ne!(second_history, first_history);
        assert!(
            second_history
                .to_string()
                .contains("host-authored planning followup")
        );
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);

        let replay = FollowupBinder::new(&c, &held, "dedicated-host-followup");
        binders.push(replay.clone());
        let observed = runtime
            .dispatch_governed_with_turn_ticket(
                &identity,
                Some("planning_lead"),
                input,
                replay.clone(),
            )
            .await
            .unwrap();
        assert!(observed.admission.deduplicated && observed.settlement.is_ok());
        assert_eq!(observed.admission.session_id, c.lead_session);
        assert_eq!(observed.admission.input_id, second_id);
        assert!(
            replay.custody.accepted.get().is_none(),
            "replay cannot promote a new source"
        );
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        assert_eq!(history(f, &c.lead_session).await, second_history);

        // An independently valid foreign source is actually bound and then
        // refused by the native dedicated-member conjunction, not the binder.
        let mut event = private_event("foreign-after-second-g");
        event.route = c.lead.original.route.clone();
        event.content_sha256 = digest(b"foreign after second G");
        let authentication = event.original_authentication.clone();
        let foreign = f.source.retain_event(event).await.unwrap();
        let route = f.source.route(&foreign.route).unwrap();
        let foreign_binder = Arc::new(OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: f.source.clone(),
            grant: None,
            batch: None,
            original: foreign.clone(),
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication: evidence(&route.authentication_domain, &authentication).unwrap(),
            event_key: f.source.original_event_key(&foreign).await.unwrap(),
            console: None,
            approved_login: None,
        });
        let foreign_input = Input::Prompt(PromptInput::new("foreign after second G", None));
        let foreign_id = foreign_input.id().clone();
        denied(
            &runtime
                .dispatch_governed_with_turn_ticket(
                    &identity,
                    Some("planning_lead"),
                    foreign_input,
                    foreign_binder.clone(),
                )
                .await,
            &c.lead_session,
        );
        let actual_foreign = foreign_binder
            .held
            .get()
            .expect("actual foreign binder completed");
        let claims = actual_foreign.original.association().candidate();
        assert!(claims.original_work.work == foreign.original);
        f.source
            .owner
            .grants()
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .unwrap();
        assert!(
            f.machine
                .input_state(&c.lead_session, &foreign_id)
                .await
                .unwrap()
                .is_none()
        );

        let cancelled = FollowupBinder::new(&c, &held, "dedicated-after-parent-cancel");
        binders.push(cancelled.clone());
        assert!(matches!(
            f.source
                .cancel_original_operation(&c.parent.original)
                .await
                .unwrap(),
            PrivateOriginalCancellation::OperationsCancelled
        ));
        let parent_key = f
            .source
            .source_record_key(&c.parent.original)
            .await
            .unwrap();
        let parent_row = f.store.get(&parent_key).await.unwrap().unwrap();
        assert!(
            f.source
                .decode_stored(&parent_row)
                .unwrap()
                .operation_cancelled
        );
        // P is the exact contributor of G. G's own source is still Open, so
        // the public binder succeeds and actual native contributor admission
        // must observe P's durable cancellation and refuse this new input.
        let blocked = Input::Prompt(PromptInput::new(
            "host planning after parent cancellation",
            None,
        ));
        let blocked_id = blocked.id().clone();
        denied(
            &runtime
                .dispatch_governed_with_turn_ticket(
                    &identity,
                    Some("planning_lead"),
                    blocked,
                    cancelled.clone(),
                )
                .await,
            &c.lead_session,
        );
        assert_eq!(cancelled.bound.get().unwrap().id(), &blocked_id);
        assert!(cancelled.custody.accepted.get().is_none());
        assert!(
            f.machine
                .input_state(&c.lead_session, &blocked_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(input_count(f, &c.lead_session).await, 2);
        assert_eq!(history(f, &c.lead_session).await, second_history);
        assert_eq!(history(f, &f.session).await, a_history);
        assert_eq!(history(f, &c.parent_session).await, p_history);
        assert_eq!(
            f.store.get(&source_row.key).await.unwrap().unwrap(),
            source_row
        );
        assert_eq!(
            f.store.get_effect(&delivery.key).await.unwrap().unwrap(),
            delivery,
            "host-authored G input cannot acknowledge waiter disclosure"
        );
        assert!(
            binders
                .iter()
                .all(|binder| binder.unresolved.lock().unwrap().is_empty())
        );
    }))
    .catch_unwind()
    .await;
    let cleanup = c.f.shutdown().await;
    drop(binders);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(result) => result.expect("bounded second G input control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
