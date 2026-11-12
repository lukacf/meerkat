//! The executors of the decision kinds a runtime owns, driven for it.
//!
//! Core's [`DecisionExecutorLoop`] is the one executor of the decision kinds
//! registered with it: it authorizes an approved decision, has the kind's
//! [`DecisionExecutor`] carry out the held request, settles the permit,
//! records an expiry and delivers the notice an ended decision owes. A
//! [`DecisionExecutorDriver`] makes its passes for a runtime: one when it
//! starts (which settles what an earlier process left), one each time a
//! decision of the runtime changed in a way that may owe work
//! ([`RuntimeDecisions::notify_work`]: a hold, a decide, a withdrawal), one
//! at once after a pass that changed a decision (a decision it carried out
//! owes its notice next), and one per tick while a pass may find work that
//! nothing announces: a decision of its kinds is open or approved (its
//! deadline passes without an event), or the last pass left something it
//! could not do then. A pass with nothing owed reads decision records
//! only: it loads no session and wakes no member. A pass that carries a
//! decision out or delivers its notice does what that work does (a create
//! with a first message starts that agent's first turn; a notice to an
//! agent starts a turn of an idle agent). A runtime with no decision of the
//! driver's kinds makes its first pass and no other.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use meerkat_core::approval::decision::{DecisionKind, DecisionListFilter, DecisionPhase};
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionExecutor, DecisionExecutorLoop, DecisionExecutorLoopError,
    DecisionExecutorPass, DecisionPassOutcome, DurableDecisionService,
};
use tokio::sync::Notify;

use super::owner::RuntimeDecisions;

/// The driver of a runtime's decision executors: see the module
/// documentation. Dropping it stops the passes; a pass in flight is dropped
/// at its next await, and what it left outstanding is settled first by the
/// next pass, of this process or the next.
pub struct DecisionExecutorDriver {
    task: Option<tokio::task::JoinHandle<()>>,
    passes: Arc<AtomicU64>,
}

impl std::fmt::Debug for DecisionExecutorDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionExecutorDriver")
            .field("running", &self.task.is_some())
            .field("passes", &self.passes())
            .finish()
    }
}

impl DecisionExecutorDriver {
    /// The time between two passes while a decision waits for its deadline
    /// or a pass left work, unless a host gives another.
    pub const DEFAULT_TICK: Duration = Duration::from_secs(15);

    /// Start the executors `executors`, one per decision kind, over the
    /// runtime's decisions, and drive their passes. `approver` answers
    /// whether the party that decided a decision may still do so
    /// ([`crate::UnifiedRuntime::decision_approver_currentness`]).
    ///
    /// # Errors
    /// What [`DecisionExecutorLoop::start`] refuses: two executors of one
    /// kind, a candidate launch (which executes nothing), and a store that
    /// cannot serve an executor claim.
    pub fn spawn(
        decisions: &RuntimeDecisions,
        executors: Vec<Arc<dyn DecisionExecutor>>,
        approver: Arc<dyn DecisionApproverCurrentness>,
        tick: Duration,
    ) -> Result<Self, DecisionExecutorLoopError> {
        let kinds: Vec<DecisionKind> = executors
            .iter()
            .map(|executor| executor.kind().clone())
            .collect();
        let owner = decisions.owner().clone();
        let mut executor_loop = DecisionExecutorLoop::start(owner.clone(), executors, approver)?;
        let work = decisions.work_signal();
        let passes = Arc::new(AtomicU64::new(0));
        let made = Arc::clone(&passes);
        let task = tokio::spawn(async move {
            loop {
                let pass = executor_loop.pass().await;
                made.fetch_add(1, Ordering::Relaxed);
                match after(&pass) {
                    // What the pass changed may owe more: pass again.
                    Next::Now => tokio::task::yield_now().await,
                    Next::Later => wait_for_work(&work, Some(tick)).await,
                    Next::Quiet => {
                        let waits = decisions_wait(&owner, &kinds);
                        wait_for_work(&work, waits.then_some(tick)).await;
                    }
                }
            }
        });
        Ok(Self {
            task: Some(task),
            passes,
        })
    }

    /// How many passes the driver has finished: one at its start, then one
    /// for each change of a decision and each tick it waited out. A host
    /// with no decision of the driver's kinds makes the first and no other.
    pub fn passes(&self) -> u64 {
        self.passes.load(Ordering::Relaxed)
    }

    /// Stop the passes and wait until the driver's task has ended.
    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for DecisionExecutorDriver {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Wait until a decision changed, or, with a tick, until it passed.
async fn wait_for_work(work: &Notify, tick: Option<Duration>) {
    match tick {
        Some(tick) => {
            tokio::select! {
                () = work.notified() => {}
                () = tokio::time::sleep(tick) => {}
            }
        }
        None => work.notified().await,
    }
}

/// When the next pass is due after one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// At once: the pass changed a decision, which may owe more work (a
    /// decision it carried out or ended owes its notice).
    Now,
    /// After a tick: the pass left work it could not do then (a decision it
    /// deferred, released or failed, a kind it could not claim, a page it
    /// could not read).
    Later,
    /// When a decision changes, or after a tick while one waits.
    Quiet,
}

/// When the next pass is due after `pass`.
fn after(pass: &DecisionExecutorPass) -> Next {
    for failure in &pass.read_failures {
        tracing::warn!(%failure, "a decision executor pass could not read a page");
    }
    for (kind, refusal) in &pass.unclaimed {
        tracing::debug!(%kind, %refusal, "a decision kind's executor claim is not held");
    }
    let mut next = if pass.read_failures.is_empty() && pass.unclaimed.is_empty() {
        Next::Quiet
    } else {
        Next::Later
    };
    for (decision_id, outcome) in &pass.outcomes {
        match outcome {
            DecisionPassOutcome::Failed(error) => {
                tracing::warn!(%decision_id, %error, "a decision executor pass failed a decision");
                next = Next::Later;
            }
            DecisionPassOutcome::Deferred(_)
            | DecisionPassOutcome::KindNotClaimed
            | DecisionPassOutcome::Released => next = Next::Later,
            // A decision of a kind this driver does not execute is another
            // executor's: it is no work of this one.
            DecisionPassOutcome::NoExecutor => {}
            _ if next == Next::Quiet => next = Next::Now,
            _ => {}
        }
    }
    next
}

/// Whether a decision of `kinds` is open or approved: its deadline passes,
/// or its execution is owed, with no event to announce it. A count the
/// store cannot give counts as one that waits.
fn decisions_wait(owner: &DurableDecisionService, kinds: &[DecisionKind]) -> bool {
    kinds.iter().any(|kind| {
        [DecisionPhase::Open, DecisionPhase::Approved]
            .into_iter()
            .any(|phase| {
                let filter = DecisionListFilter::default()
                    .with_kind(kind.clone())
                    .with_phase(phase);
                owner.count(&filter).map_or(true, |count| count > 0)
            })
    })
}
