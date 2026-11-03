//! The launch submission's one replay after a typed pre-admission answer, on
//! the actual native owner. The answer comes from the real system: an owner
//! change of the source publication held in progress after the real bind
//! (native input admission then answers PolicyChanged). No native outcome is
//! synthesized here.
use super::*;
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::SessionServiceRuntimeExt;
use std::sync::atomic::AtomicUsize;

/// How long an injected owner change stays in progress.
const CHANGE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy)]
enum Inject {
    /// Hold one owner change of the source publication in progress, this
    /// long, from another task after the bind: native input admission then
    /// refuses with PolicyChanged.
    PolicyChange(Duration),
}

/// What every injected binder of one launch shares across its attempts.
#[derive(Default)]
struct Injected {
    calls: AtomicUsize,
    inputs: Mutex<Vec<serde_json::Value>>,
    bound: Mutex<Vec<InputId>>,
    unresolved: AtomicUsize,
    holds: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

/// One attempt's real workflow binder with one pre-admission answer injected
/// after each of the launch's first `inject` real binds. It records every
/// input it is given and every bound input identity.
struct InjectAfterBind {
    inner: Arc<WorkflowBinder>,
    source: Arc<PrivateOriginals>,
    mode: Inject,
    inject: usize,
    shared: Arc<Injected>,
}

#[async_trait::async_trait]
impl GovernedInputBinder for InjectAfterBind {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let call = self.shared.calls.fetch_add(1, Ordering::AcqRel);
        self.shared
            .inputs
            .lock()
            .unwrap()
            .push(serde_json::to_value(&input).unwrap());
        let bound = self.inner.bind(context, input).await?;
        self.shared
            .bound
            .lock()
            .unwrap()
            .push(bound.input.id().clone());
        if call < self.inject {
            match self.mode {
                Inject::PolicyChange(hold) => {
                    let publication = self.source.owner.publication();
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
                    if !holding.await.map_err(|_| "fixture change hold failed")? {
                        return Err("fixture publication change unavailable".into());
                    }
                }
            }
        }
        Ok(bound)
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.shared.unresolved.fetch_add(1, Ordering::AcqRel);
        self.inner.retain_unresolved(settlement, detail);
    }
}

struct Launch {
    prepared: Arc<PreparedWorkflowLaunch>,
    route: PrivateOriginalRoute,
    mode: Inject,
    inject: usize,
    shared: Arc<Injected>,
    workflows: Arc<Mutex<Vec<Arc<WorkflowBinder>>>>,
    lease: EffectLease,
    inputs: usize,
}

/// Each attempt's fresh real workflow binder, wrapped to inject; `record`
/// hands that binder's record to the submission.
struct InjectedBinders {
    prepared: Arc<PreparedWorkflowLaunch>,
    source: Arc<PrivateOriginals>,
    route: PrivateOriginalRoute,
    mode: Inject,
    inject: usize,
    shared: Arc<Injected>,
    workflows: Arc<Mutex<Vec<Arc<WorkflowBinder>>>>,
    record: bool,
}
#[async_trait::async_trait]
impl LaunchBinders for InjectedBinders {
    async fn next(&mut self) -> Result<LaunchBinding, PrivateOriginalError> {
        let inner = self.prepared.binder(&self.source, &self.route).await?;
        self.workflows.lock().unwrap().push(inner.clone());
        Ok(LaunchBinding {
            binder: Arc::new(InjectAfterBind {
                inner: inner.clone(),
                source: self.source.clone(),
                mode: self.mode,
                inject: self.inject,
                shared: self.shared.clone(),
            }),
            workflow: self.record.then_some(inner),
        })
    }
}

async fn claimed_launch(f: &FirstFixture, c: &Case, mode: Inject, inject: usize) -> Launch {
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
        mode,
        inject,
        shared: Arc::default(),
        workflows: Arc::default(),
        lease: claimed.lease.unwrap(),
        inputs,
    }
}

/// A dispatch of A's own admitted work, outside any earlier call's tool time.
fn dispatch(c: &Case) -> ToolDispatchContext {
    ToolDispatchContext::default()
        .with_work_authorization(c.a_dispatch.work_authorization().cloned())
}

impl Launch {
    /// The product's submission, each attempt through a fresh real workflow
    /// binder (wrapped to inject); `record` hands that binder's record to it.
    async fn submit(
        &self,
        f: &FirstFixture,
        dispatch: Option<(&ToolDispatchContext, tokio::time::Instant)>,
        record: bool,
    ) -> WorkflowLaunchObservation {
        let mut binders = InjectedBinders {
            prepared: self.prepared.clone(),
            source: f.source.clone(),
            route: self.route.clone(),
            mode: self.mode,
            inject: self.inject,
            shared: self.shared.clone(),
            workflows: self.workflows.clone(),
            record,
        };
        self.prepared
            .submit_launch(&f.source, dispatch, &mut binders)
            .await
            .unwrap()
    }
    fn calls(&self) -> usize {
        self.shared.calls.load(Ordering::Acquire)
    }
    async fn join_holds(&self) {
        let holds = std::mem::take(&mut *self.shared.holds.lock().unwrap());
        for hold in holds {
            hold.await.unwrap();
        }
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
        assert_eq!(self.calls(), 2, "exactly one replay");
        let workflows = self.workflows.lock().unwrap().clone();
        assert_eq!(workflows.len(), 2);
        assert!(
            !Arc::ptr_eq(&workflows[0], &workflows[1]),
            "the single-use binder is never bound twice"
        );
        let inputs = self.shared.inputs.lock().unwrap().clone();
        assert_eq!(inputs[0], inputs[1], "the replay submits the exact input");
        let bound = self.shared.bound.lock().unwrap().clone();
        assert_eq!(bound.len(), 2);
        assert_eq!(
            bound[0], bound[1],
            "the fresh binder rebinds the same first custody and input"
        );
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
    /// Definitely not admitted, after `calls` binds: no native input, no
    /// unresolved custody, and the failed launch is recorded as before.
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
        assert_eq!(self.calls(), calls);
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
}

async fn control(name: &str, body: impl AsyncFnOnce(&FirstFixture, Case)) {
    let mut f = FirstFixture::for_continuation().await;
    // As the completed host does, select the source's custody recovery: the
    // replay rebinds a refused attempt's retained first custody through it.
    let announcer = f
        .native_for_console_mut()
        .mob_runtime()
        .continuation_source_announcer();
    f.source.start_custody_recovery(announcer).unwrap();
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        body(&f, c).await;
    }))
    .catch_unwind()
    .await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.unwrap_or_else(|_| panic!("bounded {name}")),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_changed_launch_replays_its_exact_input_once() {
    control("PolicyChanged replay", async |f, c| {
        let launch = claimed_launch(f, &c, Inject::PolicyChange(CHANGE), 1).await;
        let context = dispatch(&c);
        let deadline = tokio::time::Instant::now() + GUARD;
        let observation = launch.submit(f, Some((&context, deadline)), true).await;
        launch.join_holds().await;
        launch.assert_replayed_admission(f, &c, observation).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_policy_change_stops_after_one_replay() {
    control("repeated PolicyChanged", async |f, c| {
        let launch = claimed_launch(f, &c, Inject::PolicyChange(CHANGE), usize::MAX).await;
        let context = dispatch(&c);
        let deadline = tokio::time::Instant::now() + GUARD;
        let observation = launch.submit(f, Some((&context, deadline)), true).await;
        launch.join_holds().await;
        launch.assert_not_admitted(f, &c, observation, 2).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_changed_outside_a_dispatch_or_its_budget_is_never_replayed() {
    control("PolicyChanged without a dispatch", async |f, c| {
        let launch = claimed_launch(f, &c, Inject::PolicyChange(CHANGE), usize::MAX).await;
        let observation = launch.submit(f, None, true).await;
        launch.join_holds().await;
        launch.assert_not_admitted(f, &c, observation, 1).await;
    })
    .await;
    control("PolicyChanged without the binder record", async |f, c| {
        let launch = claimed_launch(f, &c, Inject::PolicyChange(CHANGE), usize::MAX).await;
        let context = dispatch(&c);
        let deadline = tokio::time::Instant::now() + GUARD;
        let observation = launch.submit(f, Some((&context, deadline)), false).await;
        launch.join_holds().await;
        launch.assert_not_admitted(f, &c, observation, 1).await;
    })
    .await;
    control("PolicyChanged after the dispatch deadline", async |f, c| {
        let launch = claimed_launch(f, &c, Inject::PolicyChange(CHANGE), usize::MAX).await;
        let context = dispatch(&c);
        let deadline = tokio::time::Instant::now();
        let observation = launch.submit(f, Some((&context, deadline)), true).await;
        launch.join_holds().await;
        launch.assert_not_admitted(f, &c, observation, 1).await;
    })
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
        }),
        IdentityRuntimeError::GovernedInput(GovernedInputError::Bind("refused".into())),
        IdentityRuntimeError::GovernedInput(GovernedInputError::Unbound),
    ] {
        assert_eq!(launch_replay(&error, &attempt), None);
    }
}
