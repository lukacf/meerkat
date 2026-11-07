//! Rows of an approval branch's cleanup: the branch is retired and its
//! controller grant revoked from durable state, by Core's pass, whatever the
//! process or the custody did in between; the sweep's reading of each
//! decision phase; and the origin told how an admission that failed for good
//! ended.

use std::sync::atomic::AtomicU32;

use meerkat_authorization::grants::GrantRefusal;
use meerkat_core::approval::ApprovalStoreError;
use meerkat_core::approval::decision::{
    DecisionExecutorClaim, DecisionKind, DecisionListFilter, DecisionPage, DecisionRow,
    UnreadableDecision,
};

use super::*;

/// The runtime's native custody, busy while `refusing`: every revoke is
/// refused as unavailable, as when some session's mutation gate or driver is
/// held at each try.
struct BusyCustody {
    refusing: AtomicBool,
    tries: AtomicU32,
}

impl BusyCustody {
    fn busy() -> Arc<Self> {
        Arc::new(Self {
            refusing: AtomicBool::new(true),
            tries: AtomicU32::new(0),
        })
    }

    fn free(&self) {
        self.refusing.store(false, Ordering::SeqCst);
    }

    fn tries(&self) -> u32 {
        self.tries.load(Ordering::SeqCst)
    }
}

impl ApprovalControllerCustody for BusyCustody {
    fn revoke_controller(
        &self,
        grants: &LocalGrantAuthority,
        issuer: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal> {
        self.tries.fetch_add(1, Ordering::SeqCst);
        if self.refusing.load(Ordering::SeqCst) {
            return Err(GrantRefusal::Unavailable);
        }
        grants.revoke(issuer, grant, &mut AnyCustody)
    }
}

/// The decision store, whose read of one decision answers unavailable or a
/// row this binary cannot decode, when the row sets it.
#[derive(Default)]
struct DecisionRows {
    inner: InMemoryDecisionStore,
    unavailable: Mutex<Option<DecisionId>>,
    unreadable: Mutex<Option<DecisionId>>,
}

impl DecisionRows {
    fn unavailable_for(&self, decision: &DecisionId) {
        *self.unavailable.lock().expect("unavailable") = Some(decision.clone());
    }

    fn unreadable_for(&self, decision: &DecisionId) {
        *self.unreadable.lock().expect("unreadable") = Some(decision.clone());
    }

    fn readable(&self) {
        *self.unavailable.lock().expect("unavailable") = None;
        *self.unreadable.lock().expect("unreadable") = None;
    }
}

impl DecisionStore for DecisionRows {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        if self.unavailable.lock().expect("unavailable").as_ref() == Some(decision_id) {
            return Err(ApprovalStoreError::Contended);
        }
        if self.unreadable.lock().expect("unreadable").as_ref() == Some(decision_id) {
            return Ok(Some(DecisionRow::Unreadable(UnreadableDecision::new(
                decision_id.as_str(),
                "a row this binary cannot decode",
            ))));
        }
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
        self.inner.is_persistent()
    }

    fn claim_executor_kind(
        &self,
        kind: &DecisionKind,
    ) -> Result<DecisionExecutorClaim, ApprovalStoreError> {
        self.inner.claim_executor_kind(kind)
    }
}

#[tokio::test(start_paused = true)]
async fn a_controller_grant_whose_custody_refuses_every_try_is_revoked_by_the_next_pass() {
    let custody = BusyCustody::busy();
    let fixture = Fixture::with(Terms {
        custody: Some(custody.clone()),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    // The approved work ended: the branch retires, and the runtime's custody
    // refuses every try to revoke its controller grant.
    assert_eq!(
        fixture.grants.clean_up(&decision).await,
        executor::Cleanup::Pending,
        "the revoke is left to the next pass"
    );
    assert_eq!(fixture.branches.retired(), vec![decision.clone()]);
    assert!(custody.tries() > 0, "the revoke was tried");
    assert!(
        controller_current(&fixture, &decision),
        "the busy custody revoked nothing"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Owed,
        "the decision still owes its cleanup"
    );
    // Once the custody frees, Core's next pass revokes it and settles the
    // decision's cleanup.
    custody.free();
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.clone(), DecisionPassOutcome::CleanupSettled)),
        "{pass:?}"
    );
    assert!(
        !controller_current(&fixture, &decision),
        "the next pass revoked the retired branch's controller grant"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Settled
    );
}

#[tokio::test(start_paused = true)]
async fn an_owner_read_that_answers_unavailable_leaves_the_revoke_to_the_pass_after_a_restart() {
    let journal = Arc::new(UncertainJournal::default());
    let fixture = Fixture::with(Terms {
        grant_store: Some(journal.clone()),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    // An append whose outcome is unknown: the grant owner answers every read
    // unavailable until it restarts.
    journal.uncertain.store(true, Ordering::SeqCst);
    assert!(
        fixture
            .owner
            .issue_root(
                &agent("grant-root"),
                id("unrelated-grant"),
                owner_agent(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .is_err()
    );
    let controller_id = branch_controller_grant_id(&decision).expect("the derived id");
    assert!(
        fixture.owner.issued_reference(&controller_id).is_err(),
        "the owner read answers unavailable"
    );
    // The approved work ended: the branch retires, and its controller grant
    // cannot be read now.
    assert_eq!(
        fixture.grants.clean_up(&decision).await,
        executor::Cleanup::Pending
    );
    assert_eq!(fixture.branches.retired(), vec![decision.clone()]);
    // The host restarts: the grant owner is restored from its journal.
    journal.uncertain.store(false, Ordering::SeqCst);
    let fixture = fixture.restarted();
    assert!(
        controller_current(&fixture, &decision),
        "nothing revoked it before the restart"
    );
    let _ = fixture.executor_loop().pass().await;
    assert!(
        !controller_current(&fixture, &decision),
        "Core's first pass after the restart revoked it"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Settled
    );
}

#[tokio::test]
async fn a_restart_between_the_retirement_and_the_revoke_is_revoked_by_the_next_pass() {
    let journal = Arc::new(UncertainJournal::default());
    let fixture = Fixture::with(Terms {
        grant_store: Some(journal),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    // The host retired the branch, and the process stopped before Core
    // revoked the branch's controller grant.
    fixture
        .branches
        .retire(&decision)
        .await
        .expect("the host retires the branch");
    let fixture = fixture.restarted();
    assert!(
        controller_current(&fixture, &decision),
        "nothing revoked it before the restart"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Owed
    );
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.clone(), DecisionPassOutcome::CleanupSettled)),
        "{pass:?}"
    );
    assert!(
        !controller_current(&fixture, &decision),
        "Core's first pass after the restart revoked it"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Settled
    );
    // Cold: the pass read the open branches and retired, and read neither
    // the asking agent nor a branch's seat.
    let calls = fixture.branches.calls();
    assert!(
        calls
            .iter()
            .all(|call| matches!(*call, "open_branches" | "retire")),
        "{calls:?}"
    );
}

/// A crash after every undo step and before the settlement leaves the
/// decision's cleanup owed. The branch is retired and its controller grant
/// revoked, so the owed fact carries no authority: no pass finds anything of
/// it to sweep or lists it as owed work, and the pass runs as before.
#[tokio::test]
async fn a_cleanup_left_owed_by_a_crash_after_the_undo_holds_nothing() {
    let journal = Arc::new(UncertainJournal::default());
    let fixture = Fixture::with(Terms {
        grant_store: Some(journal),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    // Every undo step ran, and the process stopped before the settlement.
    assert_eq!(
        fixture.grants.undo(&decision).await,
        executor::Cleanup::Done
    );
    let fixture = fixture.restarted();
    assert!(!controller_current(&fixture, &decision));
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.iter().all(|(id, _)| *id != decision),
        "nothing of the decision is swept: {pass:?}"
    );
    assert!(pass.read_failures.is_empty(), "{pass:?}");
    let row = fixture.stored(&decision);
    assert_eq!(row.state.phase, DecisionPhase::Executed);
    assert_eq!(
        row.state.cleanup,
        DecisionCleanup::Owed,
        "the fact stays as the crash left it"
    );
    let owed = fixture.decisions.owed(None, 10).expect("the owed page");
    assert!(
        owed.decisions.iter().all(|row| row.decision_id != decision),
        "an owed cleanup is no owed work"
    );
    assert_eq!(
        fixture.branches.calls(),
        vec!["open_branches"],
        "the sweep found no branch and no controller grant of it"
    );
}

#[tokio::test]
async fn an_unavailable_read_of_the_decision_retires_and_revokes_nothing() {
    let rows = Arc::new(DecisionRows::default());
    let fixture = Fixture::with(Terms {
        store: Some(rows.clone()),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    // The approval grant's window ended: the branch is retired as soon as
    // its decision can be read, and never on a read that answers
    // unavailable.
    fixture.clock.advance(15 * MINUTE_MS + 1);
    rows.unavailable_for(&decision);
    let _ = fixture.grants.sweep_branches().await;
    assert!(
        fixture.branches.retired().is_empty(),
        "an unavailable read retires nothing"
    );
    assert!(
        controller_current(&fixture, &decision),
        "and revokes nothing"
    );
    rows.readable();
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Owed,
        "and settles nothing"
    );
    let _ = fixture.grants.sweep_branches().await;
    assert_eq!(fixture.branches.retired(), vec![decision.clone()]);
    assert!(!controller_current(&fixture, &decision));
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Settled
    );
}

/// The decision `phase` leaves an offer of the data agent's peer work in,
/// with a branch the host holds open for it; whether a sweep retires that
/// branch.
async fn swept(phase: DecisionPhase) -> bool {
    let start = real_now_ms();
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + 20 * MINUTE_MS)),
        ..Terms::default()
    });
    let held = fixture.offer(true).expect("held");
    let decision = held.decision_id.clone();
    match phase {
        DecisionPhase::Open => {}
        DecisionPhase::Approved => {
            fixture.decide(&held, true);
        }
        DecisionPhase::Rejected => {
            fixture.decide(&held, false);
        }
        DecisionPhase::Withdrawn => {
            fixture
                .decisions
                .withdraw(&decision, &crate::decision::DecisionWithdrawer::Host)
                .expect("withdrawn");
        }
        DecisionPhase::Expired => {
            fixture.clock.advance(61 * MINUTE_MS);
        }
        DecisionPhase::ExecutionRefused => {
            fixture.decide(&held, true);
            // The offering grant's window now ends first: terms_changed.
            fixture.clock.advance(10 * MINUTE_MS);
            let _ = fixture.executor_loop().pass().await;
        }
        DecisionPhase::Executed => unreachable!("the executed phase has rows of its own"),
    }
    assert_eq!(fixture.stored(&decision).state.phase, phase);
    let now = fixture.clock.now();
    fixture
        .branches
        .open(&ApprovalBranchRequest {
            decision: decision.clone(),
            grantee: data_agent(),
            branch: approval_branch_principal(&data_agent(), &decision).expect("branch"),
            expires_at_ms: now + 15 * MINUTE_MS,
        })
        .await
        .expect("the host holds a branch open");
    let _ = fixture.grants.sweep_branches().await;
    fixture.branches.retired().contains(&decision)
}

#[tokio::test]
async fn the_sweep_keeps_a_branch_whose_decision_may_still_execute() {
    for phase in [DecisionPhase::Open, DecisionPhase::Approved] {
        assert!(
            !swept(phase).await,
            "{phase:?}: its execution may be in flight"
        );
    }
}

#[tokio::test]
async fn the_sweep_retires_the_branch_of_a_decision_that_ended_without_its_work() {
    for phase in [
        DecisionPhase::Rejected,
        DecisionPhase::Expired,
        DecisionPhase::Withdrawn,
        DecisionPhase::ExecutionRefused,
    ] {
        assert!(swept(phase).await, "{phase:?}: nothing runs in its branch");
    }
}

#[tokio::test]
async fn an_open_branch_whose_decision_cannot_be_read_is_retired_once_its_window_passed() {
    let rows = Arc::new(DecisionRows::default());
    let fixture = Fixture::with(Terms {
        store: Some(rows.clone()),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    settle_the_notice(&fixture, &decision);
    rows.unreadable_for(&decision);
    // Within the approval grant's window its work may still run: kept.
    let _ = fixture.grants.sweep_branches().await;
    assert!(fixture.branches.retired().is_empty());
    assert!(controller_current(&fixture, &decision));
    // Once that window must have passed: retired, and its controller grant
    // revoked.
    fixture.clock.advance(15 * MINUTE_MS + 1);
    let _ = fixture.grants.sweep_branches().await;
    assert_eq!(
        fixture.branches.retired(),
        vec![decision.clone()],
        "a branch whose decision cannot be read is retired once its window passed"
    );
    assert!(!controller_current(&fixture, &decision));
}

#[tokio::test]
async fn an_admission_that_fails_for_good_tells_the_origin_refused_and_never_approved() {
    let fixture = Fixture::new();
    fixture.branches.seat_refused.store(true, Ordering::SeqCst);
    let decision = executed(&fixture).await;
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.clone(), DecisionPassOutcome::NoticeSettled)),
        "{pass:?}"
    );
    assert_eq!(
        *fixture.route.notices.lock().expect("notices"),
        vec![(decision.clone(), AccessOutcome::Refused)],
        "the origin learns that no answer is coming, and is never told it was approved"
    );
    assert!(
        !controller_current(&fixture, &decision),
        "the branch's controller grant is revoked"
    );
    assert_eq!(
        fixture.stored(&decision).state.cleanup,
        DecisionCleanup::Settled
    );
}
