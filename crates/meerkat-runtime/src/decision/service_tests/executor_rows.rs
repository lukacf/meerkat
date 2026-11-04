//! Rows of the decision executor loop: one executor per store and kind, in
//! this process and across opens of a SQLite store, and never beside an
//! executor of 0.9.10; settle first after a restart; the approver's
//! currentness at execution; retries, refusals and notices; the deadline;
//! and a candidate launch that starts none.

use std::collections::{BTreeMap, VecDeque};

use async_trait::async_trait;
use futures::FutureExt;

use super::*;
use crate::decision::{
    DecisionApproverCurrentness, DecisionDeferral, DecisionExecution, DecisionExecutionPermit,
    DecisionExecutor, DecisionExecutorLoop, DecisionExecutorLoopError, DecisionExecutorPass,
    DecisionExecutorRecordUnavailable, DecisionPassOutcome,
};

/// Where an interrupted execution waits until its pass is dropped.
#[derive(Debug, Clone, Copy)]
enum Interruption {
    /// Before it commits anything.
    BeforeTheCommit,
    /// After its commit, which the owner's record reports.
    AfterTheCommit,
}

/// The owner of one decision kind's held requests: answers each execution
/// from its script for the decision (a commit when none is scripted), keeps
/// its own record of the commits it made, and delivers notices while it
/// can. An interrupted execution never returns.
struct ScriptedOwner {
    kind: DecisionKind,
    scripts: Mutex<BTreeMap<DecisionId, VecDeque<DecisionExecution>>>,
    interruptions: Mutex<BTreeMap<DecisionId, Interruption>>,
    commits: Mutex<BTreeSet<DecisionId>>,
    record_readable: AtomicBool,
    delivers: AtomicBool,
    calls: Mutex<Vec<(&'static str, DecisionId)>>,
}

impl ScriptedOwner {
    fn of_kind(kind: &str) -> Arc<Self> {
        Arc::new(Self {
            kind: DecisionKind::new(kind).expect("kind"),
            scripts: Mutex::default(),
            interruptions: Mutex::default(),
            commits: Mutex::default(),
            record_readable: AtomicBool::new(true),
            delivers: AtomicBool::new(true),
            calls: Mutex::default(),
        })
    }

    /// The owner of the rows' `release` decisions.
    fn release() -> Arc<Self> {
        Self::of_kind("release")
    }

    fn script(
        &self,
        decision_id: &DecisionId,
        executions: impl IntoIterator<Item = DecisionExecution>,
    ) {
        self.scripts
            .lock()
            .expect("scripts")
            .entry(decision_id.clone())
            .or_default()
            .extend(executions);
    }

    /// The next execution of `decision_id` waits at `at` and never returns.
    fn interrupt(&self, decision_id: &DecisionId, at: Interruption) {
        self.interruptions
            .lock()
            .expect("interruptions")
            .insert(decision_id.clone(), at);
    }

    /// A commit this owner made under the decision's permit, recorded in its
    /// own record before it stopped.
    fn committed_before_the_stop(&self, decision_id: &DecisionId) {
        self.commits
            .lock()
            .expect("commits")
            .insert(decision_id.clone());
    }

    fn set_record_readable(&self, readable: bool) {
        self.record_readable.store(readable, Ordering::SeqCst);
    }

    fn set_delivers(&self, delivers: bool) {
        self.delivers.store(delivers, Ordering::SeqCst);
    }

    fn record(&self, call: &'static str, decision_id: &DecisionId) {
        self.calls
            .lock()
            .expect("calls")
            .push((call, decision_id.clone()));
    }

    fn calls(&self) -> Vec<(&'static str, DecisionId)> {
        self.calls.lock().expect("calls").clone()
    }

    /// The decisions this owner was asked to execute, in order.
    fn executions(&self) -> Vec<DecisionId> {
        self.calls()
            .into_iter()
            .filter(|(call, _)| *call == "execute")
            .map(|(_, decision_id)| decision_id)
            .collect()
    }
}

#[async_trait]
impl DecisionExecutor for ScriptedOwner {
    fn kind(&self) -> &DecisionKind {
        &self.kind
    }

    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        self.record("execute", &decision.decision_id);
        assert!(
            permit.binds(
                &decision.descriptor.operation_id,
                &decision.descriptor.request_digest
            ),
            "the permit binds the held request"
        );
        let interruption = self
            .interruptions
            .lock()
            .expect("interruptions")
            .remove(&decision.decision_id);
        if let Some(at) = interruption {
            if matches!(at, Interruption::AfterTheCommit) {
                self.committed_before_the_stop(&decision.decision_id);
            }
            return std::future::pending().await;
        }
        let execution = self
            .scripts
            .lock()
            .expect("scripts")
            .get_mut(&decision.decision_id)
            .and_then(VecDeque::pop_front)
            .unwrap_or(DecisionExecution::Committed);
        if execution == DecisionExecution::Committed {
            self.committed_before_the_stop(&decision.decision_id);
        }
        execution
    }

    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        self.record("committed", &decision.decision_id);
        if !self.record_readable.load(Ordering::SeqCst) {
            return Err(DecisionExecutorRecordUnavailable(
                "the record is offline".to_owned(),
            ));
        }
        Ok(self
            .commits
            .lock()
            .expect("commits")
            .contains(&decision.decision_id))
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.record("deliver_notice", &decision.decision_id);
        self.delivers
            .load(Ordering::SeqCst)
            .then(|| ApprovalConsentDeliveryState::Admitted {
                session_id: SessionId::new(),
                input_id: format!("notice-{}", decision.decision_id),
            })
    }
}

/// How the host stands on every approver.
#[derive(Debug, Clone, Copy)]
enum Standing {
    Current,
    NotCurrent,
    Unsettled,
    /// Evidence an owner change makes stale before the authorization
    /// commits.
    Stale,
}

/// The host's answers for the approvers who decided.
struct Approvers(Mutex<Standing>);

impl Approvers {
    fn current() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Standing::Current)))
    }

    fn set(&self, standing: Standing) {
        *self.0.lock().expect("standing") = standing;
    }
}

impl DecisionApproverCurrentness for Approvers {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        match *self.0.lock().expect("standing") {
            Standing::Current => current(),
            Standing::NotCurrent => ConsentEligibilityAnswer::Refused,
            Standing::Unsettled => ConsentEligibilityAnswer::Unsettled,
            Standing::Stale => {
                let evidence = Arc::new(Eligibility::current());
                let answer = ConsentEligibilityAnswer::Eligible(evidence.clone());
                evidence.revoke();
                answer
            }
        }
    }
}

fn start(
    service: &DurableDecisionService,
    owners: &[&Arc<ScriptedOwner>],
    approvers: &Arc<Approvers>,
) -> Result<DecisionExecutorLoop, DecisionExecutorLoopError> {
    let executors: Vec<Arc<dyn DecisionExecutor>> = owners
        .iter()
        .map(|owner| -> Arc<dyn DecisionExecutor> { Arc::<ScriptedOwner>::clone(owner) })
        .collect();
    let approver: Arc<dyn DecisionApproverCurrentness> = approvers.clone();
    DecisionExecutorLoop::start(service.clone(), executors, approver)
}

/// What a pass did to each decision, in the order it did it.
fn outcomes_of(pass: &DecisionExecutorPass) -> BTreeMap<DecisionId, Vec<DecisionPassOutcome>> {
    let mut outcomes: BTreeMap<DecisionId, Vec<DecisionPassOutcome>> = BTreeMap::new();
    for (decision_id, outcome) in &pass.outcomes {
        outcomes
            .entry(decision_id.clone())
            .or_default()
            .push(outcome.clone());
    }
    outcomes
}

#[tokio::test]
async fn the_loop_executes_an_approved_decision_once_then_delivers_its_notice() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let owner = ScriptedOwner::release();
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

    let first = executor.pass().await;
    assert_eq!(
        first.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::Executed)]
    );
    let executed = store.stored(&decision_id);
    assert_eq!(executed.state.phase, DecisionPhase::Executed);
    assert!(!executed.state.permit_issued);
    assert_eq!(executed.state.notice, DecisionNotice::Owed);

    owner.set_delivers(false);
    let undelivered = executor.pass().await;
    assert_eq!(
        undelivered.outcomes,
        vec![(
            decision_id.clone(),
            DecisionPassOutcome::Deferred(DecisionDeferral::NoticeUndelivered)
        )]
    );
    assert_eq!(
        store.stored(&decision_id).state.notice,
        DecisionNotice::Owed
    );

    owner.set_delivers(true);
    let delivered = executor.pass().await;
    assert_eq!(
        delivered.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::NoticeSettled)]
    );
    assert_eq!(
        store.stored(&decision_id).state.notice,
        DecisionNotice::Delivered
    );

    store.clear();
    let idle = executor.pass().await;
    assert!(idle.outcomes.is_empty(), "{:?}", idle.outcomes);
    assert_eq!(
        store.calls(),
        vec!["due", "owed"],
        "a pass reads its two pages, never every row"
    );
    assert_eq!(owner.executions(), vec![decision_id.clone()], "once");
    assert_eq!(
        owner.calls(),
        vec![
            ("execute", decision_id.clone()),
            ("deliver_notice", decision_id.clone()),
            ("deliver_notice", decision_id),
        ]
    );
}

#[test]
fn a_pass_can_run_on_a_spawned_task() {
    fn assert_send<T: Send>(_: &T) {}
    let (_store, _clock, service) = fixture();
    let mut executor = start(
        &service,
        &[&ScriptedOwner::release()],
        &Approvers::current(),
    )
    .expect("start");
    let pass = executor.pass();
    assert_send(&pass);
}

fn release_kind() -> DecisionKind {
    DecisionKind::new("release").expect("kind")
}

/// A service over the SQLite decision store under `store_path`, opened as
/// another process opens it.
fn sqlite_service(store_path: &std::path::Path) -> DurableDecisionService {
    let store: Arc<dyn DecisionStore> = Arc::new(SqliteDecisionStore::in_store_path(store_path));
    let clock: Arc<dyn DecisionClock> = ManualClock::at(HELD_AT);
    DurableDecisionService::with_clock(store, clock)
}

#[tokio::test]
async fn a_kind_held_elsewhere_is_reported_and_retried() {
    let (store, clock, service) = fixture();
    let other_owner = service_over(&store, &clock);
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let first_owner = ScriptedOwner::release();
    let second_owner = ScriptedOwner::release();
    let approvers = Approvers::current();

    let first = start(&service, &[&first_owner], &approvers).expect("the first executor");
    let mut second = start(&other_owner, &[&second_owner], &approvers)
        .expect("a second executor of the kind starts without it");
    let mut same_owner = start(&service, &[&second_owner], &approvers)
        .expect("and so does a second loop of the same owner");
    for executor in [&mut second, &mut same_owner] {
        let held = executor.pass().await;
        assert_eq!(
            held.unclaimed,
            vec![(release_kind(), DecisionError::ExecutorHeld)]
        );
        assert_eq!(
            held.outcomes,
            vec![(decision_id.clone(), DecisionPassOutcome::NoExecutor)]
        );
    }
    assert!(
        second_owner.calls().is_empty(),
        "a kind held elsewhere is not executed here"
    );
    let waiting = store.stored(&decision_id);
    assert_eq!(waiting.state.phase, DecisionPhase::Approved);
    assert!(!waiting.state.permit_issued);

    drop(first);
    let claimed = second.pass().await;
    assert!(claimed.unclaimed.is_empty(), "{:?}", claimed.unclaimed);
    assert_eq!(
        claimed.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::Executed)],
        "the next pass claims the kind once the first executor ends"
    );
    assert_eq!(second_owner.executions(), vec![decision_id]);
    assert_eq!(
        same_owner.pass().await.unclaimed,
        vec![(release_kind(), DecisionError::ExecutorHeld)],
        "and holds it from then on"
    );
    assert!(first_owner.calls().is_empty());
}

#[tokio::test]
async fn two_runtimes_execute_their_own_kinds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rendering = rendering();
    let holder = sqlite_service(dir.path());
    let release_id = approved(&holder, &rendering);
    let deploy_id = approved_of_kind(&holder, &rendering, "deploy", "artifact:build-7");
    let release_owner = ScriptedOwner::release();
    let deploy_owner = ScriptedOwner::of_kind("deploy");
    let approvers = Approvers::current();

    // Two runtimes over one store, each with its own open of it, as two
    // processes are.
    let mut releases = start(&sqlite_service(dir.path()), &[&release_owner], &approvers)
        .expect("the executor of releases");
    let mut deploys = start(&sqlite_service(dir.path()), &[&deploy_owner], &approvers)
        .expect("the executor of deploys starts beside it");
    for kind in ["release", "deploy"] {
        assert!(
            dir.path()
                .join(format!("approval_decisions.executor.{kind}.lock"))
                .is_file(),
            "each kind's claim holds a lock file of its own"
        );
    }

    let first = releases.pass().await;
    assert!(first.unclaimed.is_empty(), "{:?}", first.unclaimed);
    assert_eq!(
        outcomes_of(&first),
        BTreeMap::from([
            (release_id.clone(), vec![DecisionPassOutcome::Executed]),
            (deploy_id.clone(), vec![DecisionPassOutcome::NoExecutor]),
        ])
    );
    let second = deploys.pass().await;
    assert!(second.unclaimed.is_empty(), "{:?}", second.unclaimed);
    assert_eq!(
        outcomes_of(&second).get(&deploy_id),
        Some(&vec![DecisionPassOutcome::Executed])
    );
    for decision_id in [&release_id, &deploy_id] {
        assert_eq!(
            holder.get(decision_id).expect("read").state.phase,
            DecisionPhase::Executed
        );
    }
    assert_eq!(release_owner.executions(), vec![release_id]);
    assert_eq!(deploy_owner.executions(), vec![deploy_id]);

    let mut late = start(&sqlite_service(dir.path()), &[&deploy_owner], &approvers)
        .expect("a second executor of deploys starts without it");
    assert_eq!(
        late.pass().await.unclaimed,
        vec![(
            DecisionKind::new("deploy").expect("kind"),
            DecisionError::ExecutorHeld
        )],
        "another open's executor holds the kind"
    );
    drop(deploys);
    assert!(
        late.pass().await.unclaimed.is_empty(),
        "it claims the kind at its next pass once that executor ends"
    );
}

#[tokio::test]
async fn a_0910_loop_and_a_0911_loop_never_share_a_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rendering = rendering();
    let decision_id = approved(&sqlite_service(dir.path()), &rendering);
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();
    let store_lock = dir.path().join("approval_decisions.executor.lock");
    // The claim of a 0.9.10 executor: an exclusive OS lock on the store's
    // lock file.
    let lock_as_0910 = || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&store_lock)
            .expect("open the store's lock file")
    };

    let old_executor = lock_as_0910();
    old_executor
        .try_lock()
        .expect("the 0.9.10 executor's claim");
    let mut executor = start(&sqlite_service(dir.path()), &[&owner], &approvers)
        .expect("a 0.9.11 loop starts beside a 0.9.10 executor");
    let held = executor.pass().await;
    assert_eq!(
        held.unclaimed,
        vec![(release_kind(), DecisionError::ExecutorHeld)]
    );
    assert_eq!(
        held.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::NoExecutor)]
    );
    assert!(
        owner.calls().is_empty(),
        "nothing executes beside a 0.9.10 executor"
    );
    drop(old_executor);

    let claimed = executor.pass().await;
    assert!(claimed.unclaimed.is_empty(), "{:?}", claimed.unclaimed);
    assert_eq!(
        claimed.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::Executed)]
    );

    // While the 0.9.11 loop runs, a 0.9.10 executor is refused at its start.
    let late_old_executor = lock_as_0910();
    assert!(matches!(
        late_old_executor.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    #[allow(deprecated)]
    let whole_store = sqlite_service(dir.path()).claim_executor().map(|_| ());
    assert_eq!(whole_store, Err(DecisionError::ExecutorHeld));
    drop(executor);
    late_old_executor
        .try_lock()
        .expect("free once the 0.9.11 loop ends");
}

#[tokio::test]
async fn after_a_restart_the_loop_settles_outstanding_permits_first() {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let rendering = rendering();
    let previous = service_over(&store, &clock);
    let landed = approved_subject(&previous, &rendering, "subject:landed");
    let lost = approved_subject(&previous, &rendering, "subject:lost");
    // The previous executor was authorized for both and stopped: it had
    // committed under one permit, and not under the other.
    for decision_id in [&landed, &lost] {
        let _permit = previous
            .authorize_execution(decision_id, &operation_id(), &request_digest(), &current())
            .expect("permit");
    }
    drop(previous);
    let owner = ScriptedOwner::release();
    owner.committed_before_the_stop(&landed);

    let service = service_over(&store, &clock);
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

    owner.set_record_readable(false);
    let unreadable = executor.pass().await;
    let deferred = vec![DecisionPassOutcome::Deferred(
        DecisionDeferral::RecordUnavailable,
    )];
    assert_eq!(
        outcomes_of(&unreadable),
        BTreeMap::from([(landed.clone(), deferred.clone()), (lost.clone(), deferred)])
    );
    for decision_id in [&landed, &lost] {
        let waiting = store.stored(decision_id);
        assert_eq!(waiting.state.phase, DecisionPhase::Approved);
        assert!(waiting.state.permit_issued, "nothing is settled blind");
    }

    owner.set_record_readable(true);
    let settled = executor.pass().await;
    assert_eq!(
        outcomes_of(&settled),
        BTreeMap::from([
            (landed.clone(), vec![DecisionPassOutcome::SettledExecuted]),
            (
                lost.clone(),
                vec![
                    DecisionPassOutcome::SettledReleased,
                    DecisionPassOutcome::Executed
                ]
            ),
        ])
    );
    for decision_id in [&landed, &lost] {
        assert_eq!(
            store.stored(decision_id).state.phase,
            DecisionPhase::Executed
        );
    }
    assert_eq!(
        owner.executions(),
        vec![lost],
        "a landed commit is never made again"
    );
}

#[tokio::test]
async fn a_permit_outstanding_past_the_deadline_settles_into_its_end() {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let rendering = rendering();
    let previous = service_over(&store, &clock);
    let landed = approved_subject(&previous, &rendering, "subject:landed");
    let lost = approved_subject(&previous, &rendering, "subject:lost");
    for decision_id in [&landed, &lost] {
        let _permit = previous
            .authorize_execution(decision_id, &operation_id(), &request_digest(), &current())
            .expect("permit");
    }
    drop(previous);
    let owner = ScriptedOwner::release();
    owner.committed_before_the_stop(&landed);

    clock.set(DEADLINE + 5);
    let service = service_over(&store, &clock);
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");
    let settled = executor.pass().await;
    assert_eq!(
        outcomes_of(&settled),
        BTreeMap::from([
            (landed.clone(), vec![DecisionPassOutcome::SettledExecuted]),
            (lost.clone(), vec![DecisionPassOutcome::SettledReleased]),
        ])
    );
    assert_eq!(
        store.stored(&landed).state.phase,
        DecisionPhase::Executed,
        "a commit made before the deadline settles after it"
    );
    let expired = store.stored(&lost);
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert_eq!(expired.state.notice, DecisionNotice::Owed);
    assert!(owner.executions().is_empty(), "nothing new runs");

    let notices = executor.pass().await;
    assert_eq!(
        outcomes_of(&notices),
        BTreeMap::from([
            (landed, vec![DecisionPassOutcome::NoticeSettled]),
            (lost, vec![DecisionPassOutcome::NoticeSettled]),
        ])
    );
}

/// A pass may be dropped at any await, and its execution with it: the
/// permit stays outstanding, and the next pass settles it first from the
/// owner's own record. A commit made before the drop is recorded executed
/// and never made again; an execution dropped before its commit is released
/// and runs once more.
#[tokio::test]
async fn a_dropped_pass_leaves_its_permit_to_the_next_pass() {
    for (at, settled, runs) in [
        (
            Interruption::AfterTheCommit,
            vec![DecisionPassOutcome::SettledExecuted],
            1,
        ),
        (
            Interruption::BeforeTheCommit,
            vec![
                DecisionPassOutcome::SettledReleased,
                DecisionPassOutcome::Executed,
            ],
            2,
        ),
    ] {
        let (store, _clock, service) = fixture();
        let rendering = rendering();
        let decision_id = approved(&service, &rendering);
        let owner = ScriptedOwner::release();
        owner.interrupt(&decision_id, at);
        let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

        assert!(
            executor.pass().now_or_never().is_none(),
            "{at:?}: the pass is dropped while its execution waits"
        );
        let outstanding = store.stored(&decision_id);
        assert_eq!(outstanding.state.phase, DecisionPhase::Approved, "{at:?}");
        assert!(
            outstanding.state.permit_issued,
            "{at:?}: nothing settled it"
        );

        let next = executor.pass().await;
        let expected: Vec<_> = settled
            .into_iter()
            .map(|outcome| (decision_id.clone(), outcome))
            .collect();
        assert_eq!(next.outcomes, expected, "{at:?}");
        assert_eq!(
            store.stored(&decision_id).state.phase,
            DecisionPhase::Executed,
            "{at:?}"
        );
        assert_eq!(owner.executions().len(), runs, "{at:?}");
    }
}

#[tokio::test]
async fn the_owner_answers_settle_each_attempt() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let retried = approved_subject(&service, &rendering, "subject:retried");
    let refused = approved_subject(&service, &rendering, "subject:refused");
    let owner = ScriptedOwner::release();
    owner.script(&retried, [DecisionExecution::Released]);
    owner.script(
        &refused,
        [DecisionExecution::Refused(reference("quota_exceeded"))],
    );
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

    let first = executor.pass().await;
    assert_eq!(
        outcomes_of(&first),
        BTreeMap::from([
            (retried.clone(), vec![DecisionPassOutcome::Released]),
            (refused.clone(), vec![DecisionPassOutcome::ExecutionRefused]),
        ])
    );
    let waiting = store.stored(&retried);
    assert_eq!(waiting.state.phase, DecisionPhase::Approved);
    assert!(!waiting.state.permit_issued);
    let closed = store.stored(&refused);
    assert_eq!(closed.state.phase, DecisionPhase::ExecutionRefused);
    assert_eq!(closed.execution_refusal, Some(reference("quota_exceeded")));

    let second = executor.pass().await;
    assert_eq!(
        outcomes_of(&second),
        BTreeMap::from([
            (retried.clone(), vec![DecisionPassOutcome::Executed]),
            (refused.clone(), vec![DecisionPassOutcome::NoticeSettled]),
        ])
    );
    assert_eq!(store.stored(&retried).state.phase, DecisionPhase::Executed);
    let executions = owner.executions();
    let runs_of = |decision_id: &DecisionId| {
        executions
            .iter()
            .filter(|executed| *executed == decision_id)
            .count()
    };
    assert_eq!(
        (runs_of(&retried), runs_of(&refused)),
        (2, 1),
        "the retried request runs again, the refused one never"
    );
}

#[tokio::test]
async fn an_approver_who_is_no_longer_current_ends_the_decision_with_no_execution() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();
    approvers.set(Standing::NotCurrent);
    let mut executor = start(&service, &[&owner], &approvers).expect("start");

    let first = executor.pass().await;
    assert_eq!(
        first.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::ApproverNotCurrent)]
    );
    let ended = store.stored(&decision_id);
    assert_eq!(ended.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!ended.state.permit_issued);
    assert_eq!(
        ended.execution_refusal,
        Some(reference(APPROVER_NOT_CURRENT_REFUSAL))
    );
    assert!(owner.executions().is_empty(), "no permit, no execution");

    let second = executor.pass().await;
    assert_eq!(
        second.outcomes,
        vec![(decision_id, DecisionPassOutcome::NoticeSettled)]
    );
}

#[tokio::test]
async fn an_unsettled_approver_answer_defers_the_decision() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let before = store.stored(&decision_id);
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();
    approvers.set(Standing::Unsettled);
    let mut executor = start(&service, &[&owner], &approvers).expect("start");

    let deferred = executor.pass().await;
    assert_eq!(
        deferred.outcomes,
        vec![(
            decision_id.clone(),
            DecisionPassOutcome::Deferred(DecisionDeferral::ApproverUnsettled)
        )]
    );
    assert_eq!(store.stored(&decision_id), before, "nothing applied");
    assert!(owner.executions().is_empty());

    approvers.set(Standing::Current);
    let executed = executor.pass().await;
    assert_eq!(
        executed.outcomes,
        vec![(decision_id, DecisionPassOutcome::Executed)]
    );
}

/// Evidence an owner change made stale before the authorization committed
/// (related or not) says nothing about the approver: the decision is
/// deferred as for an unsettled answer, and a later pass with a current
/// answer executes it.
#[tokio::test]
async fn evidence_no_longer_current_at_the_commit_defers_the_decision() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let before = store.stored(&decision_id);
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();
    approvers.set(Standing::Stale);
    let mut executor = start(&service, &[&owner], &approvers).expect("start");

    let deferred = executor.pass().await;
    assert_eq!(
        deferred.outcomes,
        vec![(
            decision_id.clone(),
            DecisionPassOutcome::Deferred(DecisionDeferral::ApproverUnsettled)
        )]
    );
    assert_eq!(store.stored(&decision_id), before, "nothing applied");
    assert!(owner.executions().is_empty());

    approvers.set(Standing::Current);
    let executed = executor.pass().await;
    assert_eq!(
        executed.outcomes,
        vec![(decision_id, DecisionPassOutcome::Executed)]
    );
}

#[tokio::test]
async fn the_loop_records_a_due_expiry_then_delivers_the_notice() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let owner = ScriptedOwner::release();
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

    clock.set(DEADLINE);
    let pass = executor.pass().await;
    assert_eq!(
        pass.outcomes,
        vec![
            (decision_id.clone(), DecisionPassOutcome::Expired),
            (decision_id.clone(), DecisionPassOutcome::NoticeSettled),
        ]
    );
    let expired = store.stored(&decision_id);
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert_eq!(expired.state.notice, DecisionNotice::Delivered);
    assert!(
        owner.executions().is_empty(),
        "nothing runs at the deadline"
    );
}

#[tokio::test]
async fn a_decision_of_a_kind_with_no_executor_is_left_as_stored() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let before = store.stored(&decision_id);
    let owner = ScriptedOwner::of_kind("deploy");
    let mut executor = start(&service, &[&owner], &Approvers::current()).expect("start");

    let pass = executor.pass().await;
    assert_eq!(
        pass.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::NoExecutor)]
    );
    clock.set(DEADLINE);
    let due = executor.pass().await;
    assert_eq!(
        due.outcomes,
        vec![
            (decision_id.clone(), DecisionPassOutcome::NoExecutor),
            (decision_id.clone(), DecisionPassOutcome::NoExecutor),
        ]
    );
    assert_eq!(store.stored(&decision_id), before);
    assert!(owner.calls().is_empty());
}

#[test]
fn a_candidate_launch_or_a_duplicate_kind_starts_no_executor_and_claims_nothing() {
    let (store, _clock, service) = fixture();
    let owner = ScriptedOwner::release();
    let twin = ScriptedOwner::release();
    let approvers = Approvers::current();

    store.clear();
    assert_eq!(
        start(&service.for_candidate_launch(), &[&owner], &approvers)
            .expect_err("a candidate launch"),
        DecisionExecutorLoopError::Decision(DecisionError::CandidateLaunch)
    );
    assert_eq!(
        start(&service, &[&owner, &twin], &approvers).expect_err("two owners of one kind"),
        DecisionExecutorLoopError::DuplicateKind(DecisionKind::new("release").expect("kind"))
    );
    assert!(store.calls().is_empty(), "{:?}", store.calls());

    let _executor = start(&service, &[&owner], &approvers).expect("nothing was claimed");
    assert_eq!(store.calls(), vec!["claim_executor_kind"]);
}

/// A store wrapper written before claims of a kind: it forwards no claim of
/// a kind, and the whole-store claim only when `forwards_whole_store` is
/// set, as a wrapper written for 0.9.10 does. Every other call is the
/// in-memory store's.
#[derive(Default)]
struct WrapperWithoutKindClaims {
    inner: Arc<InMemoryDecisionStore>,
    forwards_whole_store: bool,
}

impl DecisionStore for WrapperWithoutKindClaims {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        self.inner.get(decision_id)
    }

    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        self.inner.insert_if_absent(row)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        self.inner.replace_if_revision(expected_revision, row)
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.inner.list(filter, after, limit)
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        self.inner.owed(after, limit)
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.inner.due(now_ms, after, limit)
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        self.inner.count(filter)
    }

    fn is_persistent(&self) -> bool {
        false
    }

    #[allow(deprecated)]
    fn claim_executor(&self) -> Result<DecisionExecutorClaim, ApprovalStoreError> {
        if self.forwards_whole_store {
            self.inner.claim_executor()
        } else {
            Err(ApprovalStoreError::NotCommitted(
                "this decision store keeps no executor claim".to_owned(),
            ))
        }
    }
}

#[test]
fn a_store_that_keeps_no_executor_claim_starts_no_executor() {
    let store: Arc<dyn DecisionStore> = Arc::new(WrapperWithoutKindClaims::default());
    let clock: Arc<dyn DecisionClock> = ManualClock::at(HELD_AT);
    let service = DurableDecisionService::with_clock(store, clock);
    let refused = start(
        &service,
        &[&ScriptedOwner::release()],
        &Approvers::current(),
    )
    .expect_err("no claim, no executor");
    assert!(
        matches!(
            refused,
            DecisionExecutorLoopError::Decision(DecisionError::Unavailable(_))
        ),
        "{refused:?}"
    );
}

#[test]
fn a_store_wrapper_forwards_claim_executor_kind() {
    // A wrapper that forwards the claim of a kind: the loop claims each kind
    // it registers through it, and nothing else.
    let (store, _clock, service) = fixture();
    store.clear();
    let release_owner = ScriptedOwner::release();
    let deploy_owner = ScriptedOwner::of_kind("deploy");
    let _executor = start(
        &service,
        &[&release_owner, &deploy_owner],
        &Approvers::current(),
    )
    .expect("start");
    assert_eq!(
        store.calls(),
        vec!["claim_executor_kind", "claim_executor_kind"]
    );

    // A wrapper written for 0.9.10 forwards only the whole-store claim. The
    // default claim of a kind refuses, so no executor starts over it, and
    // nothing is claimed through it.
    let inner = Arc::new(InMemoryDecisionStore::new());
    let wrapper: Arc<dyn DecisionStore> = Arc::new(WrapperWithoutKindClaims {
        inner: Arc::clone(&inner),
        forwards_whole_store: true,
    });
    let clock: Arc<dyn DecisionClock> = ManualClock::at(HELD_AT);
    let refused = start(
        &DurableDecisionService::with_clock(wrapper, clock),
        &[&ScriptedOwner::release()],
        &Approvers::current(),
    )
    .expect_err("no claim of a kind, no executor");
    assert!(
        matches!(
            refused,
            DecisionExecutorLoopError::Decision(DecisionError::Unavailable(_))
        ),
        "{refused:?}"
    );
    let _claim = inner
        .claim_executor_kind(&release_kind())
        .expect("nothing was claimed");
}
