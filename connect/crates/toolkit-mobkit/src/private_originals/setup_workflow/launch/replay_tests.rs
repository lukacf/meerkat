//! The launch submission's one replay after a typed pre-admission answer, on
//! the actual native owner. Each answer comes from the real system: an owner
//! change of the source publication held in progress after the real bind
//! (native input admission then answers PolicyChanged), or the real source
//! writer held from another task (the launch fence then finds it busy). No
//! native outcome is synthesized here. The controls keep only identity
//! witnesses (Weak) of the production binders and custody: the submission and
//! native own every binder, so dropping the submission runs the real binder
//! drops, and custody settles through the real recovery.
use super::*;
use crate::private_originals::HeldPrivateOriginal;
use crate::private_originals::custody_recovery::HandoffPhase;
use crate::private_originals::governed_submit::{PendingOriginalAdmission, SourcePublication};
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::SessionServiceRuntimeExt;
use std::sync::{Weak, atomic::AtomicUsize};

/// How long an injected owner change stays in progress.
const CHANGE: Duration = Duration::from_millis(150);
/// A cause held well past the point where a control acts during its wait.
const HELD: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum Inject {
    /// Hold one owner change of the source publication in progress, this
    /// long, from another thread after the bind: native input admission then
    /// refuses with PolicyChanged.
    PolicyChange(Duration),
    /// Hold the source writer from another task, this long, after the bind:
    /// the launch fence then finds it busy.
    HeldWriter(Duration),
}

/// What every injected binder of one launch shares across its attempts.
#[derive(Default)]
struct Injected {
    calls: AtomicUsize,
    inputs: Mutex<Vec<serde_json::Value>>,
    bound: Mutex<Vec<InputId>>,
    unresolved: AtomicUsize,
    holds: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Each attempt's production workflow binder, by identity only.
    workflows: Mutex<Vec<Weak<WorkflowBinder>>>,
    /// Each bound attempt's source custody, by identity only, shared with
    /// the control, which checks it once the fixture's owners joined.
    custody: Arc<Mutex<Vec<Weak<PendingOriginalAdmission>>>>,
}

/// One attempt's real workflow binder, with the launch's injection for that
/// attempt (if any) after its real bind. It records every input it is given
/// and every bound input and custody identity.
struct InjectAfterBind {
    inner: Arc<WorkflowBinder>,
    source: Arc<PrivateOriginals>,
    plan: Arc<[Inject]>,
    shared: Arc<Injected>,
}

#[async_trait::async_trait]
impl GovernedInputBinder for InjectAfterBind {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        use meerkat_mobkit::identity_first::GovernedBindRefusal;
        let call = self.shared.calls.fetch_add(1, Ordering::AcqRel);
        self.shared
            .inputs
            .lock()
            .unwrap()
            .push(serde_json::to_value(&input).unwrap());
        let bound = self.inner.try_bind(context, input).await?;
        self.shared
            .bound
            .lock()
            .unwrap()
            .push(bound.input.id().clone());
        let custody = self.inner.delegate.pending.get().expect("bound custody");
        self.shared
            .custody
            .lock()
            .unwrap()
            .push(Arc::downgrade(custody));
        match self.plan.get(call) {
            Some(Inject::PolicyChange(hold)) => {
                let (hold, publication) = (*hold, self.source.owner.publication());
                let (held, holding) = tokio::sync::oneshot::channel();
                self.shared
                    .holds
                    .lock()
                    .unwrap()
                    .push(tokio::task::spawn_blocking(move || {
                        // The publication guard stays on this one thread.
                        let change = publication.begin_owner_change();
                        let _ = held.send(change.is_ok());
                        std::thread::sleep(hold);
                        drop(change);
                    }));
                if !holding
                    .await
                    .map_err(|_| GovernedBindRefusal::unavailable("fixture change hold failed"))?
                {
                    return Err(GovernedBindRefusal::unavailable(
                        "fixture publication change unavailable",
                    )
                    .into());
                }
            }
            Some(Inject::HeldWriter(hold)) => {
                let (hold, source) = (*hold, self.source.clone());
                let (held, holding) = tokio::sync::oneshot::channel();
                self.shared
                    .holds
                    .lock()
                    .unwrap()
                    .push(tokio::spawn(async move {
                        let _writer = source.owner.source_writer().await;
                        let _ = held.send(());
                        tokio::time::sleep(hold).await;
                    }));
                holding
                    .await
                    .map_err(|_| GovernedBindRefusal::unavailable("fixture writer hold failed"))?;
            }
            None => {}
        }
        Ok(bound)
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.shared.unresolved.fetch_add(1, Ordering::AcqRel);
        self.inner.retain_unresolved(settlement, detail);
    }
}

/// Each attempt's fresh real workflow binder, under the launch's review
/// custody as the product's binders are, wrapped to inject; `record` hands
/// that binder's record to the submission.
struct InjectedBinders {
    prepared: Arc<PreparedWorkflowLaunch>,
    source: Arc<PrivateOriginals>,
    route: PrivateOriginalRoute,
    review: ReviewCustody,
    plan: Arc<[Inject]>,
    shared: Arc<Injected>,
    waiting: tokio::sync::mpsc::UnboundedSender<LaunchReplay>,
    record: bool,
    dispatch_deadline: Option<tokio::time::Instant>,
}
#[async_trait::async_trait]
impl LaunchBinders for InjectedBinders {
    async fn next(
        &mut self,
        replay_of: Option<Weak<PendingOriginalAdmission>>,
    ) -> Result<LaunchBinding, PrivateOriginalError> {
        let inner = self
            .prepared
            .binder_with_review(
                &self.source,
                &self.route,
                self.review.clone(),
                replay_of,
                self.dispatch_deadline,
            )
            .await?;
        inner
            .attempt
            .observe_replay_wait_for_test(self.waiting.clone());
        self.shared
            .workflows
            .lock()
            .unwrap()
            .push(Arc::downgrade(&inner));
        Ok(LaunchBinding {
            binder: self.review.bind(Arc::new(InjectAfterBind {
                inner: inner.clone(),
                source: self.source.clone(),
                plan: self.plan.clone(),
                shared: self.shared.clone(),
            })),
            workflow: self.record.then_some(inner),
        })
    }
}

struct Launch {
    prepared: Arc<PreparedWorkflowLaunch>,
    route: PrivateOriginalRoute,
    review: ReviewCustody,
    plan: Arc<[Inject]>,
    shared: Arc<Injected>,
    waiting: tokio::sync::mpsc::UnboundedSender<LaunchReplay>,
    waits: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<LaunchReplay>>,
    lease: EffectLease,
    inputs: usize,
}

async fn claimed_launch(f: &FirstFixture, c: &Case, plan: &[Inject], witness: &Witness) -> Launch {
    let inputs = f.inputs().await.unwrap();
    assert_eq!(inputs, 2, "A and independent P are admitted");
    let EffectClaimOutcome::Claimed(claimed) = f
        .store
        .claim_effect(
            &c.snapshot.reference().launch_effect(),
            c.snapshot.revision(),
            "launch-replay-control",
            60_000,
        )
        .await
        .unwrap()
    else {
        panic!("first launch effect claimed")
    };
    let (waiting, waits) = tokio::sync::mpsc::unbounded_channel();
    Launch {
        prepared: Arc::new(PreparedWorkflowLaunch {
            selection: c.planning.clone(),
            reference: c.snapshot.reference().clone(),
        }),
        route: f
            .source
            .route(&c.planning.submission.original.route)
            .unwrap()
            .into_owned(),
        review: witness.review.clone(),
        plan: plan.into(),
        shared: Arc::new(Injected {
            custody: witness.custody.clone(),
            ..Injected::default()
        }),
        waiting,
        waits: tokio::sync::Mutex::new(waits),
        lease: claimed.lease.unwrap(),
        inputs,
    }
}

/// A dispatch of A's own admitted work, outside any earlier call's tool time.
fn dispatch(c: &Case) -> ToolDispatchContext {
    ToolDispatchContext::default()
        .with_work_authorization(c.a_dispatch.work_authorization().cloned())
}

/// The retained planning source the launch input binds.
fn planning_source(f: &FirstFixture, c: &Case) -> Arc<HeldPrivateOriginal> {
    f.source
        .retained
        .lock()
        .unwrap()
        .iter()
        .find(|held| held.reference == c.planning.submission.original)
        .cloned()
        .unwrap()
}

impl Launch {
    /// The product's submission under the launch's review custody, each
    /// attempt through a fresh real workflow binder (wrapped to inject);
    /// `record` hands that binder's record to it.
    async fn submit(
        &self,
        f: &FirstFixture,
        dispatch: Option<(&ToolDispatchContext, tokio::time::Instant)>,
        record: bool,
    ) -> WorkflowLaunchObservation {
        self.submit_planned(f, dispatch, record, self.plan.clone())
            .await
    }
    async fn submit_planned(
        &self,
        f: &FirstFixture,
        dispatch: Option<(&ToolDispatchContext, tokio::time::Instant)>,
        record: bool,
        plan: Arc<[Inject]>,
    ) -> WorkflowLaunchObservation {
        let mut binders = InjectedBinders {
            prepared: self.prepared.clone(),
            source: f.source.clone(),
            route: self.route.clone(),
            review: self.review.clone(),
            plan,
            shared: self.shared.clone(),
            waiting: self.waiting.clone(),
            record,
            dispatch_deadline: dispatch_deadline(dispatch),
        };
        self.review
            .retain_io(
                self.prepared
                    .submit_launch(&f.source, dispatch, &mut binders),
            )
            .await
            .unwrap()
    }
    fn calls(&self) -> usize {
        self.shared.calls.load(Ordering::Acquire)
    }
    fn workflows(&self) -> Vec<Weak<WorkflowBinder>> {
        self.shared.workflows.lock().unwrap().clone()
    }
    fn custody(&self) -> Vec<Weak<PendingOriginalAdmission>> {
        self.shared.custody.lock().unwrap().clone()
    }
    /// The cause the launch's dispatch starts to wait out, observed from
    /// inside that wait's poll while `submit` stays pending in it.
    async fn waiting_in<F: std::future::Future>(
        &self,
        submit: &mut std::pin::Pin<Box<F>>,
    ) -> LaunchReplay {
        let mut waits = self.waits.lock().await;
        tokio::select! {
            _ = submit.as_mut() => panic!("the launch finished before its wait"),
            cause = waits.recv() => cause.unwrap(),
        }
    }
    /// No dispatch of this launch ever started a replay wait.
    fn never_waited(&self) {
        assert!(
            self.waits.try_lock().unwrap().try_recv().is_err(),
            "no replay wait started"
        );
    }
    async fn join_holds(&self) {
        let holds = std::mem::take(&mut *self.shared.holds.lock().unwrap());
        for hold in holds {
            hold.await.unwrap();
        }
    }
    /// Every attempt's production binder was dropped by the submission (or
    /// native): each foreground release ran.
    fn assert_binders_released(&self) {
        for (attempt, workflow) in self.workflows().iter().enumerate() {
            assert!(
                workflow.upgrade().is_none(),
                "attempt {attempt}'s binder was released"
            );
        }
    }
    /// Every bound attempt bound the exact same input value and input
    /// identity: one first custody throughout.
    fn assert_same_input(&self, calls: usize) {
        assert_eq!(self.calls(), calls);
        let inputs = self.shared.inputs.lock().unwrap().clone();
        assert_eq!(inputs.len(), calls);
        assert!(
            inputs.iter().all(|input| input == &inputs[0]),
            "every attempt submits the exact input"
        );
        let bound = self.shared.bound.lock().unwrap().clone();
        assert!(
            bound.iter().all(|input| input == &bound[0]),
            "every attempt binds the same first custody and input identity"
        );
    }
    /// The submission released each attempt's custody: a replayed attempt's
    /// custody moved to its replay, and the last attempt's foreground release
    /// ran, handing its custody to the custody recovery (or, unenrolled,
    /// dropping it). Recovery withdraws it; the control checks that once the
    /// owners joined.
    fn assert_released(&self) {
        let custody = self.custody();
        let (last, earlier) = custody.split_last().expect("one bound attempt");
        for transferred in earlier {
            if let Some(pending) = transferred.upgrade() {
                assert!(pending.handoff.phase() == HandoffPhase::Done);
                assert!(
                    matches!(
                        &*pending.pending.lock().unwrap(),
                        SourcePublication::Settled { source_acknowledged: false, result: Err(moved) }
                            if moved == "The same source candidate moved to its exact native reprepare"
                    ),
                    "a replayed attempt's custody moved to its replay"
                );
            }
        }
        assert!(
            last.upgrade()
                .is_none_or(|pending| pending.handoff.phase() != HandoffPhase::Foreground),
            "the refused attempt's foreground released its custody"
        );
    }
    /// One exact replay admitted once: the same input value, a fresh binder
    /// rebinding the same first custody and input identity, one native
    /// input, its ticket and its source custody.
    async fn assert_replayed_admission(
        &self,
        f: &FirstFixture,
        c: &Case,
        observation: WorkflowLaunchObservation,
    ) {
        self.assert_same_input(2);
        let workflows = self.workflows();
        assert_eq!(workflows.len(), 2);
        assert!(
            !Weak::ptr_eq(&workflows[0], &workflows[1]),
            "the single-use binder is never bound twice"
        );
        assert!(
            workflows[0].upgrade().is_none(),
            "the refused attempt's binder was released"
        );
        let custody = self.custody();
        assert_eq!(custody.len(), 2);
        assert!(
            custody[0]
                .upgrade()
                .is_none_or(|refused| refused.handoff.phase() == HandoffPhase::Done),
            "the refused attempt's custody was transferred, not settled"
        );
        let replayed = custody[1].upgrade().expect("the replay's custody");
        assert!(
            planning_source(f, c)
                .first_custody
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|first| Arc::ptr_eq(first, &replayed)),
            "the replay rebound the retained first custody"
        );
        drop(replayed);
        let bound = self.shared.bound.lock().unwrap().clone();
        let admitted = native(observation.clone());
        let PrivateOriginalSubmission::Admitted {
            input_id,
            deduplicated,
            turn_ticket,
            source_settlement,
            ..
        } = &admitted
        else {
            panic!("the replay is admitted")
        };
        assert_eq!(input_id, &bound[1].to_string());
        assert!(!deduplicated);
        assert!(
            turn_ticket.is_some(),
            "the admitted replay keeps its ticket"
        );
        assert!(matches!(
            source_settlement,
            PrivateSourceSettlement::FreshAcceptancePromoted
        ));
        assert_eq!(f.inputs().await.unwrap(), self.inputs + 1, "one delivery");
        assert!(
            f.machine
                .input_state(&f.session, &bound[1])
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(self.shared.unresolved.load(Ordering::Acquire), 0);
        f.finish(&Ok::<_, String>(admitted)).await.unwrap();
        c.planning
            .record_launch_observation(&f.source, &self.lease, &observation)
            .await
            .unwrap();
        let effect = f
            .store
            .get_effect(&c.snapshot.reference().launch_effect())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(effect.status, EffectStatus::Succeeded);
        assert_eq!(effect.attempt, 1);
    }
    /// Definitely not admitted, after `calls` binds of the same input: no
    /// native input, no unresolved custody, every binder released, and the
    /// failed launch is recorded as before.
    async fn assert_not_admitted(
        &self,
        f: &FirstFixture,
        c: &Case,
        observation: WorkflowLaunchObservation,
        calls: usize,
    ) {
        assert!(matches!(
            observation,
            WorkflowLaunchObservation::NotAdmitted
        ));
        self.assert_same_input(calls);
        self.assert_binders_released();
        assert_eq!(f.inputs().await.unwrap(), self.inputs, "nothing admitted");
        assert_eq!(self.shared.unresolved.load(Ordering::Acquire), 0);
        c.planning
            .record_launch_observation(&f.source, &self.lease, &observation)
            .await
            .unwrap();
        let effect = f
            .store
            .get_effect(&c.snapshot.reference().launch_effect())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
        assert_eq!(effect.attempt, 1);
    }
    /// A dispatch dropped while it waits out its cause: one attempt, its
    /// binder released at the drop, nothing admitted or unresolved.
    async fn assert_cancelled_in_wait(&self, f: &FirstFixture) {
        self.assert_same_input(1);
        self.assert_binders_released();
        self.join_holds().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(self.calls(), 1, "a cancelled wait never replays");
        assert_eq!(f.inputs().await.unwrap(), self.inputs, "nothing admitted");
        assert_eq!(self.shared.unresolved.load(Ordering::Acquire), 0);
    }
}

/// Whether the source's custody recovery is selected, as the completed host
/// selects it, or left unselected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recovery {
    Selected,
    Unselected,
}

/// What a control's launch reports to the control beyond its own body: its
/// review custody (a lifetime witness) and each bound attempt's custody.
struct Witness {
    review: ReviewCustody,
    custody: Arc<Mutex<Vec<Weak<PendingOriginalAdmission>>>>,
}

/// One bounded control on a fresh fixture. On every exit a selected recovery
/// is stopped, native is shut down and the recovery is joined, in that order.
/// Once those owners joined, every bound attempt's source custody is settled
/// (withdrawn, moved to its replay, or promoted), and no hold of the launch's
/// review custody remains.
async fn control(
    name: &str,
    recovery: Recovery,
    body: impl AsyncFnOnce(&FirstFixture, Case, &Witness),
) {
    let mut f = FirstFixture::for_continuation().await;
    if recovery == Recovery::Selected {
        // As the completed host does, select the source's custody recovery:
        // a replay rebinds a refused attempt's retained first custody.
        let announcer = f
            .native_for_console_mut()
            .mob_runtime()
            .continuation_source_announcer();
        f.source.start_custody_recovery(announcer).unwrap();
    }
    let held: Arc<()> = Arc::new(());
    let witness = Witness {
        review: ReviewCustody::witness_for_test(held.clone()),
        custody: Arc::default(),
    };
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        body(&f, c, &witness).await;
    }))
    .catch_unwind()
    .await;
    // Native shutdown neither stops nor joins the source's recovery: stop it,
    // shut native down, then join it, before the body's panic resumes or any
    // settlement check runs.
    if recovery == Recovery::Selected {
        f.source.stop_custody_recovery();
    }
    let cleanup = f.shutdown().await;
    let joined = f.source.join_custody_recovery().await;
    assert!(cleanup.is_ok(), "{name}: {cleanup:?}");
    assert!(joined.is_ok(), "{name}: recovery join {joined:?}");
    match result {
        Ok(result) => result.unwrap_or_else(|_| panic!("bounded {name}")),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    let settled = |pending: &PendingOriginalAdmission| {
        pending.handoff.phase() == HandoffPhase::Done
            && matches!(
                &*pending.pending.lock().unwrap(),
                SourcePublication::Settled { .. }
            )
    };
    // An attempt the recovery owned (enrolled) is checked while the source
    // still retains it, so it cannot pass only because it was dropped with
    // the fixture. An unenrolled attempt has no recovery phase to reach.
    for (attempt, custody) in witness.custody.lock().unwrap().iter().enumerate() {
        assert!(
            custody
                .upgrade()
                .is_none_or(|pending| !pending.handoff.enrolled() || settled(&pending)),
            "{name}: attempt {attempt}'s enrolled source custody settled once its owners joined"
        );
    }
    drop(f);
    for (attempt, custody) in witness.custody.lock().unwrap().iter().enumerate() {
        assert!(
            custody.upgrade().is_none_or(|pending| settled(&pending)),
            "{name}: attempt {attempt}'s source custody settled once its owners joined"
        );
    }
    drop(witness);
    assert_eq!(
        Arc::strong_count(&held),
        1,
        "{name}: every hold of the launch's review custody was released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_changed_launch_replays_its_exact_input_once() {
    control(
        "PolicyChanged replay",
        Recovery::Selected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &[Inject::PolicyChange(CHANGE)], witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.assert_replayed_admission(f, &c, observation).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_policy_change_stops_after_one_replay() {
    control(
        "repeated PolicyChanged",
        Recovery::Selected,
        async |f, c, witness| {
            let plan = [Inject::PolicyChange(CHANGE); 2];
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.assert_not_admitted(f, &c, observation, 2).await;
            launch.assert_released();
        },
    )
    .await;
}

/// One replay in total: a second refusal of the other cause stops too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_causes_stop_after_one_replay_in_either_order() {
    let writer = Inject::HeldWriter(Duration::from_millis(200));
    let change = Inject::PolicyChange(CHANGE);
    for (name, plan) in [
        ("PolicyChanged then a held writer", [change, writer]),
        ("a held writer then PolicyChanged", [writer, change]),
    ] {
        control(name, Recovery::Selected, async |f, c, witness| {
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.assert_not_admitted(f, &c, observation, 2).await;
            launch.assert_released();
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_changed_outside_a_dispatch_or_its_budget_is_never_replayed() {
    let plan = [Inject::PolicyChange(CHANGE); 2];
    control(
        "PolicyChanged without a dispatch",
        Recovery::Selected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let observation = launch.submit(f, None, true).await;
            launch.join_holds().await;
            launch.never_waited();
            launch.assert_not_admitted(f, &c, observation, 1).await;
            launch.assert_released();
        },
    )
    .await;
    control(
        "PolicyChanged without the binder record",
        Recovery::Selected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), false).await;
            launch.join_holds().await;
            launch.never_waited();
            launch.assert_not_admitted(f, &c, observation, 1).await;
            launch.assert_released();
        },
    )
    .await;
    control(
        "PolicyChanged after the dispatch deadline",
        Recovery::Selected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now();
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.never_waited();
            launch.assert_not_admitted(f, &c, observation, 1).await;
            launch.assert_released();
        },
    )
    .await;
}

#[test]
fn only_the_exact_policy_changed_answer_is_a_replay() {
    use meerkat_runtime::{RuntimeDriverError, traits::ControllerReadinessFailure};
    let session = meerkat_core::SessionId::new();
    let refused = |session_id: &meerkat_core::SessionId, reason| {
        IdentityRuntimeError::GovernedInput(GovernedInputError::Mob(Box::new(
            meerkat_mob::MobError::MemberInputRefused {
                session_id: session_id.clone(),
                error: RuntimeDriverError::ControllerReadinessUnavailable { reason },
            },
        )))
    };
    let attempt = LaunchAttempt::default();
    let changed = refused(&session, ControllerReadinessFailure::PolicyChanged);
    // Nothing bound yet: no session is this input's.
    assert_eq!(launch_replay(&changed, &attempt), None);
    attempt.bind(&session);
    assert_eq!(
        launch_replay(&changed, &attempt),
        Some(LaunchReplay::PolicyChanged)
    );
    let other = meerkat_core::SessionId::new();
    assert_eq!(
        launch_replay(
            &refused(&other, ControllerReadinessFailure::PolicyChanged),
            &attempt
        ),
        None,
        "another session's refusal is not this input's"
    );
    assert_eq!(
        launch_replay(
            &refused(&session, ControllerReadinessFailure::PolicyUnavailable),
            &attempt
        ),
        None
    );
    for error in [
        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
            GovernedEntryRefusal::Retired,
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
            GovernedEntryRefusal::Expired,
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
            GovernedEntryRefusal::Unavailable("The waiter entry owner is unavailable".into()),
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved {
            detail: "native admission settled None".into(),
            receipt: None,
        }),
        IdentityRuntimeError::GovernedInput(GovernedInputError::Refused(
            meerkat_mobkit::identity_first::GovernedBindRefusal::denied("refused"),
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::Unbound),
    ] {
        assert_eq!(launch_replay(&error, &attempt), None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_source_writer_launch_waits_for_the_writer_and_replays_once() {
    control(
        "held writer replay",
        Recovery::Selected,
        async |f, c, witness| {
            let hold = Duration::from_millis(200);
            let launch = claimed_launch(f, &c, &[Inject::HeldWriter(hold)], witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let started = tokio::time::Instant::now();
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            assert!(
                started.elapsed() >= hold,
                "the replay waited for the actual writer"
            );
            launch.join_holds().await;
            launch.assert_replayed_admission(f, &c, observation).await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistently_busy_source_writer_stops_after_one_replay() {
    control(
        "repeated held writer",
        Recovery::Selected,
        async |f, c, witness| {
            let plan = [Inject::HeldWriter(Duration::from_millis(100)); 2];
            let launch = claimed_launch(f, &c, &plan, witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.assert_not_admitted(f, &c, observation, 2).await;
            launch.assert_released();
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_during_the_writer_wait_never_replays() {
    control(
        "deadline during the writer wait",
        Recovery::Selected,
        async |f, c, witness| {
            let hold = Duration::from_secs(2);
            let launch = claimed_launch(f, &c, &[Inject::HeldWriter(hold)], witness).await;
            let context = dispatch(&c);
            let started = tokio::time::Instant::now();
            let deadline = started + Duration::from_millis(300);
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            assert!(
                started.elapsed() < hold,
                "the wait ended at the dispatch deadline, not the writer"
            );
            launch.join_holds().await;
            launch.assert_not_admitted(f, &c, observation, 1).await;
            launch.assert_released();
        },
    )
    .await;
}

/// The dispatch dropped while it waits out either cause: the production
/// binder drops with it (its foreground release), nothing replays, and the
/// refused attempt's custody settles through recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_either_wait_releases_custody_and_never_replays() {
    for (name, inject, cause) in [
        (
            "cancellation during the policy wait",
            Inject::PolicyChange(HELD),
            LaunchReplay::PolicyChanged,
        ),
        (
            "cancellation during the writer wait",
            Inject::HeldWriter(HELD),
            LaunchReplay::SourceWriterBusy,
        ),
    ] {
        control(name, Recovery::Selected, async |f, c, witness| {
            let launch = claimed_launch(f, &c, &[inject], witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let mut submit = Box::pin(launch.submit(f, Some((&context, deadline)), true));
            assert_eq!(launch.waiting_in(&mut submit).await, cause);
            assert!(
                launch.workflows()[0].upgrade().is_some(),
                "the refused binder is alive while its dispatch waits"
            );
            drop(submit);
            launch.assert_cancelled_in_wait(f).await;
            launch.assert_released();
        })
        .await;
    }
}

/// What changes while the dispatch waits out its cause.
#[derive(Clone, Copy, Debug)]
enum Drift {
    /// The source owner stops admitting work (its host begins to close).
    OwnerClose,
    /// The planning lineage's actual leaf grant is revoked.
    SelectionRevoked,
}

/// The source owner closing, or the selection's authority revoked, while the
/// dispatch waits out either cause: the wait settles, the checks after it
/// refuse, and the first definite refusal stands, with no second call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_close_or_selection_drift_during_either_wait_never_replays() {
    for inject in [Inject::PolicyChange(HELD), Inject::HeldWriter(HELD)] {
        for drift in [Drift::OwnerClose, Drift::SelectionRevoked] {
            let name = format!(
                "{drift:?} during the {} wait",
                match inject {
                    Inject::PolicyChange(_) => "policy",
                    Inject::HeldWriter(_) => "writer",
                }
            );
            control(&name, Recovery::Selected, async |f, c, witness| {
                let launch = claimed_launch(f, &c, &[inject], witness).await;
                let context = dispatch(&c);
                let deadline = tokio::time::Instant::now() + GUARD;
                let mut submit = Box::pin(launch.submit(f, Some((&context, deadline)), true));
                launch.waiting_in(&mut submit).await;
                match drift {
                    Drift::OwnerClose => f.source.stop_admitting_work().unwrap(),
                    Drift::SelectionRevoked => {
                        let grants = f.source.owner.grants();
                        let administrator = f
                            .lifetime()
                            .selected_policy()
                            .native_host
                            .as_ref()
                            .unwrap()
                            .grant_administrator
                            .clone();
                        let leaf = c.planning.lineage[1].clone();
                        let machine = f.machine.clone();
                        let revoke = move || {
                            let mut custody = machine.try_controller_grant_mutation().unwrap();
                            grants
                                .revoke(
                                    &meerkat_core::AdmissionGuard::none(),
                                    &administrator,
                                    &leaf,
                                    &mut custody,
                                )
                                .unwrap();
                        };
                        match inject {
                            // The held change owns the publication writer the
                            // revocation takes: revoke at the check after the
                            // wait, inside its read of the selection.
                            Inject::PolicyChange(_) => {
                                *c.planning.binding_observed_for_test.lock().unwrap() =
                                    Some(Box::new(revoke));
                            }
                            Inject::HeldWriter(_) => revoke(),
                        }
                    }
                }
                let observation = submit.await;
                launch.join_holds().await;
                launch.assert_not_admitted(f, &c, observation, 1).await;
                assert!(
                    c.planning
                        .binding_observed_for_test
                        .lock()
                        .unwrap()
                        .is_none(),
                    "the check after the wait read the selection"
                );
                launch.assert_released();
            })
            .await;
        }
    }
}

/// Without the selected custody recovery a first candidate is not retained
/// for a replay: the launch never waits or replays, and a binder built to
/// replay it refuses at bind rather than stage fresh custody, where an
/// ordinary binder for the same input does stage and is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unselected_recovery_never_replays_or_stages_replay_custody() {
    control(
        "unselected recovery",
        Recovery::Unselected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &[Inject::PolicyChange(CHANGE)], witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let observation = launch.submit(f, Some((&context, deadline)), true).await;
            launch.join_holds().await;
            launch.never_waited();
            launch.assert_not_admitted(f, &c, observation, 1).await;
            let refused = launch.custody()[0].clone();
            assert!(
                refused.upgrade().is_none(),
                "the unenrolled first candidate was withdrawn with its binder"
            );
            assert!(
                planning_source(f, &c)
                    .first_custody
                    .lock()
                    .unwrap()
                    .is_none()
            );
            // A binder built to replay that attempt refuses at bind.
            let replay = launch
                .prepared
                .binder_with_review(
                    &f.source,
                    &launch.route,
                    witness.review.clone(),
                    Some(refused),
                    dispatch_deadline(Some((&context, deadline))),
                )
                .await
                .unwrap();
            let replay_binder = Arc::downgrade(&replay);
            let observed = launch
                .prepared
                .submit_with_binder(
                    &f.source,
                    Arc::new(InjectAfterBind {
                        inner: replay,
                        source: f.source.clone(),
                        plan: Arc::new([]),
                        shared: launch.shared.clone(),
                    }),
                    Some((&context, deadline)),
                )
                .await
                .unwrap();
            assert!(matches!(observed, WorkflowLaunchObservation::NotAdmitted));
            assert!(replay_binder.upgrade().is_none());
            assert_eq!(launch.calls(), 2, "the replay binder was asked to bind");
            assert_eq!(
                launch.custody().len(),
                1,
                "the replay binder bound no custody"
            );
            assert!(
                planning_source(f, &c)
                    .first_custody
                    .lock()
                    .unwrap()
                    .is_none()
            );
            assert_eq!(f.inputs().await.unwrap(), launch.inputs, "nothing admitted");
            // An ordinary binder for the same input stages custody and is
            // admitted: the refusal above was the replay's alone.
            let admitted = launch
                .submit_planned(f, Some((&context, deadline)), true, Arc::new([]))
                .await;
            let PrivateOriginalSubmission::Admitted { deduplicated, .. } = native(admitted.clone())
            else {
                panic!("the ordinary binder is admitted")
            };
            assert!(!deduplicated);
            assert_eq!(launch.custody().len(), 2);
            assert_eq!(f.inputs().await.unwrap(), launch.inputs + 1);
            f.finish(&Ok::<_, String>(native(admitted))).await.unwrap();
        },
    )
    .await;
}

/// Custody recovery stopped while the dispatch waits: the source no longer
/// admits work, so nothing replays and no binding is attempted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_recovery_during_the_wait_never_replays() {
    control(
        "stopped recovery",
        Recovery::Selected,
        async |f, c, witness| {
            let launch = claimed_launch(f, &c, &[Inject::PolicyChange(HELD)], witness).await;
            let context = dispatch(&c);
            let deadline = tokio::time::Instant::now() + GUARD;
            let mut submit = Box::pin(launch.submit(f, Some((&context, deadline)), true));
            launch.waiting_in(&mut submit).await;
            f.source.stop_custody_recovery();
            let observation = submit.await;
            launch.join_holds().await;
            launch.assert_not_admitted(f, &c, observation, 1).await;
            assert_eq!(launch.workflows().len(), 1, "no replay binder was built");
            launch.assert_released();
        },
    )
    .await;
}

#[test]
fn only_the_fence_recorded_busy_writer_is_a_replay() {
    let busy = IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
        GovernedEntryRefusal::Unavailable("The waiter entry owner is unavailable".into()),
    ));
    let attempt = LaunchAttempt::default();
    // The same refusal without the fence's typed cause is no replay.
    assert_eq!(launch_replay(&busy, &attempt), None);
    attempt.fence_refused(Some(LaunchReplay::SourceWriterBusy));
    assert_eq!(
        launch_replay(&busy, &attempt),
        Some(LaunchReplay::SourceWriterBusy)
    );
    // Only the Unavailable refusal carries that cause.
    for error in [
        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
            GovernedEntryRefusal::Retired,
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
            GovernedEntryRefusal::Expired,
        )),
        IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved {
            detail: "native admission settled None".into(),
            receipt: None,
        }),
    ] {
        assert_eq!(launch_replay(&error, &attempt), None);
    }
    attempt.fence_refused(None);
    assert_eq!(launch_replay(&busy, &attempt), None);
}
