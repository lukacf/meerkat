//! Library owner of durable job delivery.
//!
//! One owner per runtime delivery inbox projects the job outbox into the inbox
//! and applies pending inbox rows through a host-supplied sink. It is woken
//! only by typed signals: a job outbox commit, a runtime delivery commit, and a
//! runtime attachment becoming serving. Arming runs one reconcile pass over
//! the stores first, which also finds rows committed by another process. There
//! is no timer: a row whose application fails stays pending and is retried on
//! the next wake that names its runtime, or on the next arming.

use std::collections::HashSet;
use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::DetachedJobStore;
use meerkat_runtime::{
    LogicalRuntimeId, RuntimeDeliveryInbox, RuntimeDeliveryOwnerAlreadyArmed,
    RuntimeDeliveryOwnership,
};
use tokio::sync::watch;

use crate::{JobDeliverySink, JobOutboxProjector, JobRuntimeDeliveryApplier};

/// Rows read per projection or application page.
const DELIVERY_PAGE: usize = 256;

/// The host side of a [`RuntimeDeliveryOwner`]: where deliveries are applied.
#[async_trait::async_trait]
pub trait RuntimeDeliveryHost: Send + Sync {
    /// The sink that applies deliveries whose origin runtime belongs to
    /// `session_id`, or `None` once the host is gone. The owner stops when the
    /// host is gone.
    async fn delivery_sink(&self, session_id: &SessionId) -> Option<Arc<dyn JobDeliverySink>>;
}

/// Outcome of one owner pass, published after every pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeDeliveryPass {
    /// Passes completed since arming; the first is the reconcile pass.
    pub generation: u64,
    /// Outbox entries projected into the inbox this pass.
    pub projected: usize,
    /// Inbox rows applied this pass.
    pub applied: usize,
    /// Sessions whose rows stayed pending after this pass, awaiting a wake
    /// that names them.
    pub blocked_sessions: Vec<SessionId>,
    /// Failures observed this pass, in order.
    pub failures: Vec<String>,
}

/// The stores and signals one owner is armed over.
#[derive(Clone)]
pub struct RuntimeDeliveryOwner {
    job_store: Arc<dyn DetachedJobStore>,
    runtime_inbox: RuntimeDeliveryInbox,
    attachment_commits: Option<watch::Receiver<u64>>,
}

impl std::fmt::Debug for RuntimeDeliveryOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwner")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwner {
    pub fn new(job_store: Arc<dyn DetachedJobStore>, runtime_inbox: RuntimeDeliveryInbox) -> Self {
        Self {
            job_store,
            runtime_inbox,
            attachment_commits: None,
        }
    }

    /// Retry blocked sessions when a runtime attachment becomes serving, for
    /// example from [`meerkat_runtime::MeerkatMachine::subscribe_attachment_commits`].
    #[must_use]
    pub fn with_attachment_commits(mut self, attachment_commits: watch::Receiver<u64>) -> Self {
        self.attachment_commits = Some(attachment_commits);
        self
    }

    /// Arm the owner: claim the inbox, run the reconcile pass, then apply on
    /// typed wakes until the returned handle is dropped or the host is gone.
    ///
    /// Must be called inside a tokio runtime. Fails with
    /// [`RuntimeDeliveryOwnerAlreadyArmed`] while another owner holds the
    /// inbox; there is never a second loop over one inbox.
    pub fn arm(
        self,
        host: Arc<dyn RuntimeDeliveryHost>,
    ) -> Result<RuntimeDeliveryOwnerHandle, RuntimeDeliveryOwnerAlreadyArmed> {
        let ownership = self.runtime_inbox.claim_delivery_ownership()?;
        let (passes, passes_rx) = watch::channel(RuntimeDeliveryPass::default());
        let job_commits = self.job_store.outbox_commit_signal().subscribe();
        let inbox_commits = self.runtime_inbox.subscribe_commits();
        let task = tokio::spawn(run_owner(OwnerLoop {
            projector: JobOutboxProjector::new(self.job_store, self.runtime_inbox.clone()),
            ownership,
            host,
            job_commits,
            inbox_commits,
            attachment_commits: self.attachment_commits,
            passes,
        }));
        Ok(RuntimeDeliveryOwnerHandle {
            task,
            passes: passes_rx,
        })
    }
}

/// A running owner. Dropping it stops the owner and releases the inbox.
pub struct RuntimeDeliveryOwnerHandle {
    task: tokio::task::JoinHandle<()>,
    passes: watch::Receiver<RuntimeDeliveryPass>,
}

impl std::fmt::Debug for RuntimeDeliveryOwnerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDeliveryOwnerHandle")
            .finish_non_exhaustive()
    }
}

impl RuntimeDeliveryOwnerHandle {
    /// Observe the outcome of each pass.
    pub fn subscribe_passes(&self) -> watch::Receiver<RuntimeDeliveryPass> {
        self.passes.clone()
    }

    /// Whether the owner has stopped (its host is gone).
    pub fn is_stopped(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for RuntimeDeliveryOwnerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct OwnerLoop {
    projector: JobOutboxProjector,
    ownership: RuntimeDeliveryOwnership,
    host: Arc<dyn RuntimeDeliveryHost>,
    job_commits: watch::Receiver<u64>,
    inbox_commits: watch::Receiver<u64>,
    attachment_commits: Option<watch::Receiver<u64>>,
    passes: watch::Sender<RuntimeDeliveryPass>,
}

/// Which signals fired since the previous pass.
#[derive(Debug, Clone, Copy, Default)]
struct Wake {
    reconcile: bool,
    job_commit: bool,
    attachment_commit: bool,
}

async fn run_owner(mut owner: OwnerLoop) {
    let mut blocked: HashSet<SessionId> = HashSet::new();
    let mut wake = Wake {
        reconcile: true,
        ..Wake::default()
    };
    let mut generation = 0_u64;
    loop {
        // Mark the job and attachment signals seen BEFORE reading the
        // authority they announce, so a commit that lands during this pass
        // wakes the next one. The inbox signal is marked inside the pass.
        wake.job_commit |= owner.job_commits.has_changed().unwrap_or(false);
        owner.job_commits.borrow_and_update();
        if let Some(attachments) = owner.attachment_commits.as_mut() {
            wake.attachment_commit |= attachments.has_changed().unwrap_or(false);
            attachments.borrow_and_update();
        }

        generation = generation.wrapping_add(1);
        let mut pass = RuntimeDeliveryPass {
            generation,
            ..RuntimeDeliveryPass::default()
        };
        let Some(reconcile_failed) = run_pass(&mut owner, wake, &mut blocked, &mut pass).await
        else {
            return;
        };
        pass.blocked_sessions = blocked.iter().cloned().collect();
        if !pass.failures.is_empty() {
            tracing::warn!(
                failures = ?pass.failures,
                "durable job delivery left rows pending until the next delivery wake"
            );
        }
        owner.passes.send_replace(pass);

        // A failed reconcile read stays owed to the next wake.
        wake = Wake {
            reconcile: reconcile_failed,
            ..Wake::default()
        };
        tokio::select! {
            changed = owner.job_commits.changed() => {
                if changed.is_err() {
                    return;
                }
                wake.job_commit = true;
            }
            changed = owner.inbox_commits.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            changed = attachment_commit_changed(&mut owner.attachment_commits) => {
                match changed {
                    Ok(()) => wake.attachment_commit = true,
                    // The runtime is gone; no attachment can serve again.
                    Err(_) => owner.attachment_commits = None,
                }
            }
        }
    }
}

async fn attachment_commit_changed(
    attachments: &mut Option<watch::Receiver<u64>>,
) -> Result<(), watch::error::RecvError> {
    match attachments {
        Some(attachments) => attachments.changed().await,
        None => std::future::pending().await,
    }
}

/// One pass. Returns `None` when the host is gone, otherwise whether a
/// reconcile read failed and must be repeated on the next wake.
async fn run_pass(
    owner: &mut OwnerLoop,
    wake: Wake,
    blocked: &mut HashSet<SessionId>,
    pass: &mut RuntimeDeliveryPass,
) -> Option<bool> {
    let mut reconcile_failed = false;
    if wake.reconcile || wake.job_commit {
        loop {
            match owner.projector.project_pending(DELIVERY_PAGE).await {
                Ok(projection) => {
                    pass.projected += projection.projected.len();
                    for skipped in &projection.skipped {
                        pass.failures.push(format!(
                            "projection of job {} delivery {} failed: {}",
                            skipped.job_id, skipped.delivery_sequence, skipped.error
                        ));
                    }
                    if projection.projected.is_empty() {
                        break;
                    }
                }
                Err(error) => {
                    reconcile_failed = true;
                    pass.failures
                        .push(format!("job outbox could not be read: {error}"));
                    break;
                }
            }
        }
    }

    let mut sessions: Vec<SessionId> = Vec::new();
    if wake.reconcile {
        match owner.projector.sessions_with_pending_deliveries().await {
            Ok(found) => sessions.extend(found),
            Err(error) => {
                reconcile_failed = true;
                pass.failures.push(format!(
                    "runtime delivery backlog could not be read: {error}"
                ));
            }
        }
    }
    // Marked seen only after this pass's own projection, so the rows it just
    // committed are drained below instead of waking a redundant pass. A
    // runtime is recorded before the generation advances, so any commit not
    // in the set taken here wakes the next pass.
    owner.inbox_commits.borrow_and_update();
    let committed = owner.ownership.take_committed_runtimes();
    if !committed.is_empty() {
        match owner.projector.sessions_for_runtimes(&committed).await {
            Ok(found) => sessions.extend(found),
            Err(error) => {
                reconcile_failed = true;
                pass.failures.push(format!(
                    "committed runtime deliveries could not be read: {error}"
                ));
            }
        }
    }
    if wake.attachment_commit {
        sessions.extend(blocked.iter().cloned());
    }
    let mut seen = HashSet::new();
    sessions.retain(|session_id| seen.insert(session_id.clone()));

    for session_id in sessions {
        let sink = owner.host.delivery_sink(&session_id).await?;
        let applier = JobRuntimeDeliveryApplier::new(owner.ownership.inbox().clone(), sink);
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        loop {
            match applier.apply_pending(&runtime_id, DELIVERY_PAGE).await {
                Ok(drain) => {
                    pass.applied += drain.applied.len();
                    if let Some(row) = drain.blocked {
                        blocked.insert(session_id.clone());
                        pass.failures.push(format!(
                            "delivery {} (sequence {}) for session {session_id} is blocked: {}",
                            row.delivery_id, row.runtime_sequence, row.error
                        ));
                        break;
                    }
                    if drain.applied.len() < DELIVERY_PAGE {
                        blocked.remove(&session_id);
                        break;
                    }
                }
                Err(error) => {
                    blocked.insert(session_id.clone());
                    pass.failures.push(format!(
                        "delivery drain for session {session_id} failed: {error}"
                    ));
                    break;
                }
            }
        }
    }
    Some(reconcile_failed)
}
