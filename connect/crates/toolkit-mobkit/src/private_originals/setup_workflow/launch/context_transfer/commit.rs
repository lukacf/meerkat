//! One retained Find-fenced SQLite admission. A bounded retry can follow only
//! an unrun entered fence refusal; recipient selection stays in the parent.
use super::commit_diagnostic::{CommitDiagnostic, WorkerStage};
use super::*;
use meerkat::governed_realm::helper_admission::{
    GovernedHelperAdmissionFence, HelperAdmissionRefusal,
};
use meerkat_workgraph::{WorkGraphAdmissionFence, WorkGraphError};

// Selection is explicit at the caller. An entered step never falls back to
// the old dispatch path, which remains unreviewed-only and does not retry.
#[derive(Clone, Copy)]
pub(super) enum CommitEntry<'a> {
    Unreviewed(&'a ToolDispatchContext),
    Entered(
        &'a meerkat_core::EnteredDispatchOperation,
        &'a ToolDispatchContext,
    ),
}

#[cfg(test)]
type AttemptObserver = Arc<
    dyn Fn(std::sync::Weak<GovernedHelperAdmissionFence>, Option<HelperAdmissionRefusal>, bool)
        + Send
        + Sync,
>;

pub(crate) struct OwnedTransferCommit {
    #[cfg(test)]
    admission: WorkAdmission,
    join: tokio::sync::Mutex<CommitJoin>,
    diagnostic: Arc<CommitDiagnostic>,
}
enum CommitJoin {
    Running(tokio::task::JoinHandle<Result<WorkAdmission, PrivateOriginalError>>),
    Joined(Result<WorkAdmission, PrivateOriginalError>),
}
impl OwnedTransferCommit {
    /// The caller holds RecoveryControl::enter_transfer and its existing
    /// canonical attempt slot, then retains this owner before any await.
    #[cfg(test)]
    pub(crate) fn start(
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
    ) -> Result<Self, PrivateOriginalError> {
        Self::start_with_review(
            call,
            context,
            source,
            service,
            key,
            request,
            deadline_ms,
            ReviewCustody::default(),
            Arc::new(CommitDiagnostic::new()),
        )
    }

    /// Existing R1-only compatibility path. Custody is lifetime-only and
    /// cannot permit a reviewed operation through from_dispatch.
    pub(crate) fn start_with_review(
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
        review_custody: ReviewCustody,
        diagnostic: Arc<CommitDiagnostic>,
    ) -> Result<Self, PrivateOriginalError> {
        Self::start_selected(
            call,
            CommitEntry::Unreviewed(context),
            source,
            service,
            key,
            request,
            deadline_ms,
            review_custody,
            diagnostic,
            #[cfg(test)]
            None,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(not(test))]
    pub(crate) fn start_entered(
        call: ToolCallView<'_>,
        entered: &meerkat_core::EnteredDispatchOperation,
        context: &ToolDispatchContext,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
        review_custody: ReviewCustody,
        diagnostic: Arc<CommitDiagnostic>,
    ) -> Result<Self, PrivateOriginalError> {
        Self::start_selected(
            call,
            CommitEntry::Entered(entered, context),
            source,
            service,
            key,
            request,
            deadline_ms,
            review_custody,
            diagnostic,
            #[cfg(test)]
            None,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    pub(super) fn start_entered_observed(
        call: ToolCallView<'_>,
        entered: &meerkat_core::EnteredDispatchOperation,
        context: &ToolDispatchContext,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
        review_custody: ReviewCustody,
        diagnostic: Arc<CommitDiagnostic>,
        checkpoint: Option<Arc<CommitCheckpoint>>,
    ) -> Result<Self, PrivateOriginalError> {
        Self::start_selected(
            call,
            CommitEntry::Entered(entered, context),
            source,
            service,
            key,
            request,
            deadline_ms,
            review_custody,
            diagnostic,
            checkpoint,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_entered_probe(
        call: ToolCallView<'_>,
        entered: &meerkat_core::EnteredDispatchOperation,
        context: &ToolDispatchContext,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
        observer: AttemptObserver,
    ) -> Result<Self, PrivateOriginalError> {
        Self::start_selected(
            call,
            CommitEntry::Entered(entered, context),
            source,
            service,
            key,
            request,
            deadline_ms,
            ReviewCustody::entered_operation(entered),
            Arc::new(CommitDiagnostic::new()),
            None,
            Some(observer),
        )
    }

    fn start_selected(
        call: ToolCallView<'_>,
        entry: CommitEntry<'_>,
        source: Arc<ToolkitOriginalOwner>,
        service: WorkGraphService,
        key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        deadline_ms: u64,
        review_custody: ReviewCustody,
        diagnostic: Arc<CommitDiagnostic>,
        #[cfg(test)] checkpoint: Option<Arc<CommitCheckpoint>>,
        #[cfg(test)] observer: Option<AttemptObserver>,
    ) -> Result<Self, PrivateOriginalError> {
        diagnostic.enter(WorkerStage::ValidateCall);
        if call.name != crate::FIND_AND_INSTALL
            || service.store().kind() != WorkGraphStoreKind::Sqlite
        {
            return Err(PrivateOriginalError::Denied);
        }
        diagnostic.enter(WorkerStage::Runtime);
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        diagnostic.enter(WorkerStage::AdmissionIdentity);
        let admission = service
            .admission_identity(key.clone(), request.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        diagnostic.enter(WorkerStage::DispatchFence);
        let (fence, entered) = match entry {
            CommitEntry::Unreviewed(context) => (
                GovernedHelperAdmissionFence::from_dispatch(
                    call,
                    context,
                    source.clone(),
                    Arc::from([]),
                    service.default_realm_id(),
                    service.default_namespace().clone(),
                    admission.clone(),
                )
                .map_err(|error| {
                    diagnostic.refusal(Some(error));
                    fence_refusal(error)
                })?,
                None,
            ),
            CommitEntry::Entered(entered, context) => (
                GovernedHelperAdmissionFence::from_entered(
                    entered,
                    source.clone(),
                    Arc::from([]),
                    service.default_realm_id(),
                    service.default_namespace().clone(),
                    admission.clone(),
                ),
                Some((entered.clone(), context.clone())),
            ),
        };
        let exact = admission.clone();
        diagnostic.enter(WorkerStage::Spawn);
        let worker_diagnostic = diagnostic.clone();
        let join = tokio::task::spawn_blocking(move || {
            // One worker retains the actual witness and review custody through
            // all attempts and waits, even if its caller drops the join future.
            let mut churn = entered
                .as_ref()
                .map(|(_, context)| context.owner_change_churn());
            let outcome = (|| {
                let mut fence = fence;
                let outcome = loop {
                    let guarded = Arc::new(TransferDeadlineFence {
                        native: fence.clone(),
                        source: source.clone(),
                        deadline_ms,
                        expired: std::sync::atomic::AtomicBool::new(false),
                        diagnostic: worker_diagnostic.clone(),
                        #[cfg(test)]
                        checkpoint: checkpoint.clone(),
                    });
                    let report = guarded.clone();
                    worker_diagnostic.enter(WorkerStage::Storage);
                    let result = runtime.block_on(review_custody.retain_io(
                        service.create_idempotent_with_fence(key.clone(), request.clone(), guarded),
                    ));
                    #[cfg(test)]
                    if let Some(observer) = &observer {
                        // Observe the actual fence allocation in this test only.
                        // Weak custody cannot extend the native fence lifetime.
                        observer(Arc::downgrade(&fence), fence.refusal(), result.is_ok());
                    }
                    // The create has returned: the SQLite transaction and the
                    // native owner exclusion are gone. Only this typed pair
                    // proves that this attempt ran no store callback. A target
                    // result, including a target Conflict, is never retried.
                    let refused_unrun = matches!(&result, Err(WorkGraphError::Conflict(_)))
                        && fence.refusal() == Some(HelperAdmissionRefusal::Unavailable);
                    let can_retry = entered.as_ref().zip(churn.as_mut()).is_some_and(
                        |((_, context), churn)| {
                            if !refused_unrun
                                || matches!(source.publication().observe(|| ()),
                            Err(meerkat_authorization::publication::PublicationError::Unavailable))
                            {
                                return false;
                            }
                            let Some(work) = context.work_authorization() else {
                                return false;
                            };
                            let remaining = || {
                                let now = source.clock().now().ok()?.unix_ms;
                                let request = std::time::Duration::from_millis(
                                    deadline_ms.saturating_sub(now),
                                );
                                Some(
                                    context
                                        .remaining_tool_time()
                                        .map_or(request, |tool| tool.min(request)),
                                )
                            };
                            let Some(budget) = remaining().filter(|budget| !budget.is_zero())
                            else {
                                return false;
                            };
                            // Core owns the three-rebuild/two-second bound and its
                            // holder-or-completed-change wait. No lock is held here.
                            runtime
                                .block_on(churn.wait_after_stale_read(
                                    work.authorization().as_ref(),
                                    Some(budget),
                                    "toolkit.workflow.helper_admission",
                                    None,
                                ))
                                .settled()
                                && remaining().is_some_and(|budget| !budget.is_zero())
                        },
                    );
                    if can_retry {
                        let (entered, _) =
                            entered.as_ref().expect("only entered refusals can retry");
                        fence = GovernedHelperAdmissionFence::from_entered(
                            entered,
                            source.clone(),
                            Arc::from([]),
                            service.default_realm_id(),
                            service.default_namespace().clone(),
                            exact.clone(),
                        );
                        continue;
                    }
                    worker_diagnostic.graph_result(&result);
                    break result.map_err(|_| {
                        if report.expired.load(Ordering::Acquire) {
                            PrivateOriginalError::Denied
                        } else {
                            let refusal = fence.refusal();
                            worker_diagnostic.refusal(refusal);
                            refusal
                                .map(fence_refusal)
                                .unwrap_or(PrivateOriginalError::Unavailable)
                        }
                    })?;
                };
                worker_diagnostic.enter(WorkerStage::ReturnedItem);
                let item = outcome.item().ok_or(PrivateOriginalError::Conflict)?;
                worker_diagnostic.enter(WorkerStage::ValidateItem);
                validate_item_identity(&service, &exact, &request, item)?;
                worker_diagnostic.enter(WorkerStage::Completed);
                Ok(exact)
            })();
            worker_diagnostic.returned(&outcome);
            outcome
        });
        Ok(Self {
            #[cfg(test)]
            admission,
            join: tokio::sync::Mutex::new(CommitJoin::Running(join)),
            diagnostic,
        })
    }
    pub(super) fn diagnostic(&self) -> Arc<CommitDiagnostic> {
        self.diagnostic.clone()
    }
    #[cfg(test)]
    pub(crate) fn admission(&self) -> &WorkAdmission {
        &self.admission
    }
    pub(crate) fn finished(&self) -> bool {
        self.join.try_lock().is_ok_and(|held| match &*held {
            CommitJoin::Running(task) => task.is_finished(),
            CommitJoin::Joined(_) => true,
        })
    }
    pub(crate) async fn join(&self) -> Result<WorkAdmission, PrivateOriginalError> {
        let mut held = self.join.lock().await;
        let result = match &mut *held {
            CommitJoin::Running(task) => match task.await {
                Ok(result) => result,
                Err(_) => {
                    self.diagnostic.join_failed();
                    Err(PrivateOriginalError::Unavailable)
                }
            },
            CommitJoin::Joined(result) => {
                self.diagnostic.cached_join();
                result.clone()
            }
        };
        *held = CommitJoin::Joined(result.clone());
        self.diagnostic.joined();
        result
    }
}
struct TransferDeadlineFence {
    native: Arc<GovernedHelperAdmissionFence>,
    source: Arc<ToolkitOriginalOwner>,
    deadline_ms: u64,
    expired: std::sync::atomic::AtomicBool,
    diagnostic: Arc<CommitDiagnostic>,
    #[cfg(test)]
    checkpoint: Option<Arc<CommitCheckpoint>>,
}
impl WorkGraphAdmissionFence for TransferDeadlineFence {
    fn execute_if_current(
        &self,
        realm: &str,
        namespace: &WorkNamespace,
        admission: &WorkAdmission,
        operation: Box<dyn FnOnce() -> Result<(), WorkGraphError> + '_>,
    ) -> Result<(), WorkGraphError> {
        #[cfg(test)]
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.reach(
                CheckpointPosition::BeforeNativeStep,
                admission,
                self.deadline_ms,
            );
        }
        self.native.execute_if_current(
            realm,
            namespace,
            admission,
            Box::new(|| {
                self.diagnostic.callback_entered();
                #[cfg(test)]
                if let Some(checkpoint) = &self.checkpoint {
                    checkpoint.reach(
                        CheckpointPosition::AdmittedNativeStep,
                        admission,
                        self.deadline_ms,
                    );
                }
                let now =
                    self.source.clock().now().map_err(|_| {
                        WorkGraphError::Conflict("transfer clock unavailable".into())
                    })?;
                if now.unix_ms >= self.deadline_ms {
                    self.expired.store(true, Ordering::Release);
                    return Err(WorkGraphError::Conflict("transfer request expired".into()));
                }
                operation()
            }),
        )
    }
}
fn fence_refusal(refusal: HelperAdmissionRefusal) -> PrivateOriginalError {
    match refusal {
        HelperAdmissionRefusal::Denied => PrivateOriginalError::Denied,
        HelperAdmissionRefusal::Unavailable => PrivateOriginalError::Unavailable,
    }
}
