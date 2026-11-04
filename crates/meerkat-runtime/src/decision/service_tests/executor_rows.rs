//! Rows of the decision executor loop: one executor per store, in this
//! process and across opens of a SQLite store; settle first after a
//! restart; the approver's currentness at execution; retries, refusals and
//! notices; the deadline; and a candidate launch that starts none.

use std::collections::{BTreeMap, VecDeque};

use async_trait::async_trait;
use meerkat_store::approval_decision_store::APPROVAL_DECISIONS_FILE;

use super::*;
use crate::decision::{
    DecisionApproverCurrentness, DecisionDeferral, DecisionExecution, DecisionExecutionPermit,
    DecisionExecutor, DecisionExecutorLoop, DecisionExecutorLoopError, DecisionExecutorPass,
    DecisionExecutorRecordUnavailable, DecisionPassOutcome,
};

/// The owner of one decision kind's held requests: answers each execution
/// from its script for the decision (a commit when none is scripted), keeps
/// its own record of the commits it made, and delivers notices while it
/// can.
struct ScriptedOwner {
    kind: DecisionKind,
    scripts: Mutex<BTreeMap<DecisionId, VecDeque<DecisionExecution>>>,
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

#[test]
fn a_second_executor_of_a_store_is_refused_until_the_first_ends() {
    let (store, clock, service) = fixture();
    let other_owner = service_over(&store, &clock);
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();

    let first = start(&service, &[&owner], &approvers).expect("the first executor");
    assert_eq!(
        start(&other_owner, &[&owner], &approvers).expect_err("a second executor"),
        DecisionExecutorLoopError::Decision(DecisionError::ExecutorHeld)
    );
    assert_eq!(
        start(&service, &[&owner], &approvers).expect_err("a second loop of the same owner"),
        DecisionExecutorLoopError::Decision(DecisionError::ExecutorHeld)
    );
    drop(first);
    let _second = start(&other_owner, &[&owner], &approvers).expect("once the first ends");
}

#[test]
fn a_second_executor_of_a_sqlite_store_is_refused_across_opens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let open = || {
        let store: Arc<dyn DecisionStore> =
            Arc::new(SqliteDecisionStore::in_store_path(dir.path()));
        let clock: Arc<dyn DecisionClock> = ManualClock::at(HELD_AT);
        DurableDecisionService::with_clock(store, clock)
    };
    let owner = ScriptedOwner::release();
    let approvers = Approvers::current();

    let first = start(&open(), &[&owner], &approvers).expect("the first executor");
    assert!(
        dir.path()
            .join("approval_decisions.executor.lock")
            .is_file(),
        "the claim holds a lock file beside the database"
    );
    assert!(
        !dir.path().join(APPROVAL_DECISIONS_FILE).exists(),
        "the claim creates no database"
    );
    assert_eq!(
        start(&open(), &[&owner], &approvers).expect_err("another open's executor"),
        DecisionExecutorLoopError::Decision(DecisionError::ExecutorHeld)
    );
    drop(first);
    let _second = start(&open(), &[&owner], &approvers).expect("once the first ends");
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
    assert_eq!(store.calls(), vec!["claim_executor"]);
}

/// A store that keeps no executor claim; every other call is the in-memory
/// store's.
#[derive(Default)]
struct NoClaimStore(InMemoryDecisionStore);

impl DecisionStore for NoClaimStore {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        self.0.get(decision_id)
    }

    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        self.0.insert_if_absent(row)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        self.0.replace_if_revision(expected_revision, row)
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.0.list(filter, after, limit)
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        self.0.owed(after, limit)
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.0.due(now_ms, after, limit)
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        self.0.count(filter)
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[test]
fn a_store_that_keeps_no_executor_claim_starts_no_executor() {
    let store: Arc<dyn DecisionStore> = Arc::new(NoClaimStore::default());
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
