//! Toolkit-local real registered Find composition. All additions are compiled
//! only under the existing unit-test parent; the observers supply no authority.
use super::super::acquisition_denial::{DispatchEnd, DispatchObserver, Script};
use super::*;
use crate::ToolAdmissionError;
use crate::installed_host::{
    InstalledOperationReviewContext, InstalledOperationReviewPolicy,
    acquisition::{
        InstalledNativeContinuation, InstalledOriginalInvocation, InstalledOriginalTaskBinding,
        InstalledOriginalTaskOwner, InstalledSetupBootstrap, InstalledSetupSelection,
    },
};
use meerkat_core::{
    OperationAuthorizationError, ToolOperationTarget,
    approval::review::{
        BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
        ReviewCandidate, ReviewObservation, ReviewVerdict, ReviewerFailure,
    },
    authorization::{AuthorizationOperation, OperationReviewTier},
};
use meerkat_toolkit::{
    authority::{Action, ActionPermission, Ceiling},
    continuation::{
        ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation,
        ContinuationReconciliation, SetupContinuationDelivery, SetupContinuationOwner,
    },
    request::{ContextReference, TaskContinuation},
    setup::WorkAdmissionStepOptions,
};
use meerkat_workgraph::WorkAdmission;
use std::sync::{Condvar, OnceLock};

pub(in crate::private_originals) const FIND_CALL: &str = "controller-only-find-and-install";
pub(in crate::private_originals) const BOUND: Duration = Duration::from_secs(30);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::private_originals) enum CheckpointPosition {
    BeforeNativeStep,
    AdmittedNativeStep,
}

pub(in crate::private_originals) struct CommitCheckpoint {
    position: CheckpointPosition,
    state: Mutex<CheckpointState>,
    release: Condvar,
    changed: tokio::sync::Notify,
}
#[derive(Default)]
struct CheckpointState {
    reached: Vec<(CheckpointPosition, WorkAdmission, u64)>,
    released: bool,
}
impl CommitCheckpoint {
    pub fn new(position: CheckpointPosition) -> Arc<Self> {
        Arc::new(Self {
            position,
            state: Mutex::new(CheckpointState::default()),
            release: Condvar::new(),
            changed: tokio::sync::Notify::new(),
        })
    }
    pub fn reach(&self, position: CheckpointPosition, admission: &WorkAdmission, deadline: u64) {
        let mut state = self.state.lock().unwrap();
        if position == CheckpointPosition::AdmittedNativeStep {
            assert!(
                !state.reached.iter().any(|row| row.0 == position),
                "no replayed admitted callback"
            );
        }
        state.reached.push((position, admission.clone(), deadline));
        self.changed.notify_one();
        if position != self.position {
            return;
        }
        let limit = std::time::Instant::now() + BOUND;
        while !state.released {
            let left = limit.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "actual commit checkpoint was not released");
            state = self.release.wait_timeout(state, left).unwrap().0;
        }
    }
    pub async fn waiting(&self) -> (WorkAdmission, u64) {
        loop {
            let changed = self.changed.notified();
            if let Some(row) = self
                .state
                .lock()
                .unwrap()
                .reached
                .iter()
                .find(|row| row.0 == self.position)
            {
                return (row.1.clone(), row.2);
            }
            changed.await;
        }
    }
    pub fn is_held(&self) -> bool {
        !self.state.lock().unwrap().released
    }
    pub fn positions(&self) -> Vec<CheckpointPosition> {
        self.state
            .lock()
            .unwrap()
            .reached
            .iter()
            .map(|row| row.0)
            .collect()
    }
    pub fn release(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .released = true;
        self.release.notify_all();
    }
    pub fn release_guard(self: &Arc<Self>) -> CheckpointRelease {
        CheckpointRelease(self.clone())
    }
}
pub(in crate::private_originals) struct CheckpointRelease(Arc<CommitCheckpoint>);
impl Drop for CheckpointRelease {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Default)]
pub(in crate::private_originals) struct NativeReview {
    runs: Mutex<Vec<meerkat_core::RunId>>,
    rows: Mutex<Vec<ReviewObservation>>,
    changed: tokio::sync::Notify,
}
struct R2Find;
impl InstalledOperationReviewPolicy for R2Find {
    fn review_floor(
        &self,
        context: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError> {
        let ToolOperationTarget::RegisteredToolBundleOperation {
            bundle, operation, ..
        } = context.target()
        else {
            return Ok(OperationReviewTier::R1);
        };
        if bundle != crate::FIND_AND_INSTALL_BUNDLE || operation != crate::FIND_AND_INSTALL {
            return Ok(OperationReviewTier::R1);
        }
        if context.selected().native_host.as_ref() != Some(context.native()) {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let claims = context.association().candidate();
        let mut members = context.native().members.iter().filter(|member| {
            member.member.member == MEMBER
                && claims.target.logical_owner == member.logical_owner
                && claims.target.context_generation == member.context_generation
                && claims.target.audience == member.audience
        });
        if members.next().is_none() || members.next().is_some() {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(OperationReviewTier::R2)
    }
}
#[async_trait]
impl OperationReviewer for NativeReview {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        if candidate.tool_call_id() != FIND_CALL
            || candidate.tool_name() != crate::FIND_AND_INSTALL
            || serde_json::from_str::<serde_json::Value>(candidate.arguments().get()).ok()
                != Some(serde_json::json!({"request":"Find a tool for this original request"}))
        {
            return Err(ReviewerFailure::Unavailable);
        }
        let AuthorizationOperation::Tool(tool) = &candidate.binding().facts().operation else {
            return Err(ReviewerFailure::Unavailable);
        };
        assert_eq!(tool.name, crate::FIND_AND_INSTALL);
        assert_eq!(tool.arguments.get(), candidate.arguments().get());
        let run = candidate
            .binding()
            .facts()
            .run_id
            .clone()
            .ok_or(ReviewerFailure::Unavailable)?;
        self.runs.lock().unwrap().push(run);
        Ok(ReviewVerdict::Allow)
    }
}
impl OperationReviewObserver for NativeReview {
    fn observe(&self, row: ReviewObservation) {
        self.rows.lock().unwrap().push(row);
        self.changed.notify_one();
    }
}
impl NativeReview {
    pub fn run(&self) -> meerkat_core::RunId {
        let runs = self.runs.lock().unwrap();
        assert_eq!(
            runs.len(),
            1,
            "this control requires the first actual review attempt"
        );
        runs[0].clone()
    }
    pub fn used(&self) -> usize {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.status == ReviewAttemptStatus::Used)
            .count()
    }
    pub async fn assert_used_once(&self) {
        loop {
            let changed = self.changed.notified();
            if self.used() != 0 {
                break;
            }
            changed.await;
        }
        let rows = self.rows.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.tool_call_id.as_ref() == FIND_CALL
            && row.attempt == rows[0].attempt
            && row.retirement.is_none()));
        assert_eq!(
            rows.iter().map(|row| row.status).collect::<Vec<_>>(),
            vec![
                ReviewAttemptStatus::Pending,
                ReviewAttemptStatus::Allowed,
                ReviewAttemptStatus::Used
            ]
        );
        assert_eq!(self.runs.lock().unwrap().len(), 1);
    }
}
#[derive(Default)]
pub(in crate::private_originals) struct LeafEnd {
    values: Mutex<Vec<(String, DispatchEnd)>>,
    changed: tokio::sync::Notify,
}
impl DispatchObserver for LeafEnd {
    fn ended(&self, call: &str, end: DispatchEnd) {
        self.values.lock().unwrap().push((call.into(), end));
        self.changed.notify_one();
    }
}
impl LeafEnd {
    pub async fn dropped(&self) {
        loop {
            let changed = self.changed.notified();
            if !self.values.lock().unwrap().is_empty() {
                break;
            }
            changed.await;
        }
        assert_eq!(
            *self.values.lock().unwrap(),
            vec![(FIND_CALL.into(), DispatchEnd::Dropped)]
        );
    }
}

pub(in crate::private_originals) struct ReviewedWorkflow {
    pub(super) script: Arc<Script>,
    pub review: Arc<NativeReview>,
    pub leaf: Arc<LeafEnd>,
    pub checkpoint: Arc<CommitCheckpoint>,
    pub(super) delivery: OnceLock<meerkat_mobkit::MobRuntimeDelivery>,
    owner: OnceLock<Arc<WorkflowOriginalOwner>>,
}
impl ReviewedWorkflow {
    pub fn new(position: CheckpointPosition) -> Arc<Self> {
        let leaf = Arc::new(LeafEnd::default());
        Arc::new(Self {
            script: Script::for_entered_workflow(leaf.clone()),
            review: Arc::new(NativeReview::default()),
            leaf,
            checkpoint: CommitCheckpoint::new(position),
            delivery: OnceLock::new(),
            owner: OnceLock::new(),
        })
    }
    pub(super) fn bound_review(&self) -> Arc<BoundOperationReview> {
        Arc::new(
            BoundOperationReview::new(
                self.review.clone(),
                meerkat_core::ApprovalService::new(),
                BOUND,
            )
            .with_observer(self.review.clone()),
        )
    }
    pub(super) fn enroll(
        &self,
        installed: crate::installed_host::InstalledToolkitHost,
        helper: InstalledSetupSelection,
    ) -> crate::installed_host::InstalledToolkitHost {
        let native = installed
            .native_continuation([id("person")].into())
            .unwrap();
        let source = installed.originals();
        // Setup admission targets the original source's selected member. The
        // configured G helper remains a distinct destination, not that target.
        let proposal_target = source.route(&id("console")).unwrap().grant_target.clone();
        assert_eq!(proposal_target, id(MEMBER));
        assert_ne!(proposal_target, id(&helper.member.member));
        let owner = Arc::new(WorkflowOriginalOwner {
            native,
            source,
            proposal_target: proposal_target.clone(),
            selected: OnceLock::new(),
            planning: OnceLock::new(),
            transfer: Mutex::new(None),
            checkpoint: self.checkpoint.clone(),
        });
        assert!(self.owner.set(owner.clone()).is_ok());
        let actions = [Action::FindAndInstall, Action::Delegate]
            .into_iter()
            .map(|action| ActionPermission {
                action,
                resource: proposal_target.clone(),
            })
            .collect();
        installed
            .with_enrolled_bootstrap_acquisition(
                InstalledSetupBootstrap {
                    helper,
                    proposal_ceiling: Ceiling {
                        actions,
                        services: Default::default(),
                        frontends: Default::default(),
                        output_audiences: [id("person")].into(),
                    },
                    continuation: owner,
                    backend_generation: id("one"),
                    references: None,
                    limits: self.script.setup_limits(),
                    work_step: WorkAdmissionStepOptions {
                        worker: id("reviewed-workflow-worker"),
                        lease_ms: 60_000,
                        owner_timeout_ms: 10_000,
                        settlement_budget_ms: 10_000,
                    },
                },
                self.script.tools(),
            )
            .unwrap()
            .with_operation_review_policy(Some(Arc::new(R2Find)))
    }
    pub(super) fn continuation(&self) -> Arc<InstalledNativeContinuation> {
        self.owner.get().unwrap().native.clone()
    }
    pub(super) fn select_work(
        &self,
        original: &PrivateOriginalRef,
        backend: SetupWorkBackend,
        work: SetupWorkContext,
    ) {
        assert!(
            self.owner
                .get()
                .unwrap()
                .selected
                .set((original.clone(), backend, work))
                .is_ok()
        );
    }
    pub fn select_planning(&self, planning: Arc<InstalledWorkflowPlanning>) {
        assert!(self.owner.get().unwrap().planning.set(planning).is_ok());
    }
    pub fn transfer(&self) -> Arc<InstalledWorkflowContextTransfer> {
        self.owner
            .get()
            .unwrap()
            .transfer
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone()
    }
    pub fn script_census(&self) -> (Vec<meerkat_core::ToolResult>, usize, usize) {
        self.script.observations()
    }
    pub(super) fn model(&self, inner: Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        Arc::new(OriginalModel {
            find: self.script.provider(inner.clone()),
            inner,
        })
    }
}
fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
struct WorkflowOriginalOwner {
    native: Arc<InstalledNativeContinuation>,
    source: Arc<PrivateOriginals>,
    proposal_target: Id,
    selected: OnceLock<(PrivateOriginalRef, SetupWorkBackend, SetupWorkContext)>,
    planning: OnceLock<Arc<InstalledWorkflowPlanning>>,
    transfer: Mutex<Option<Arc<InstalledWorkflowContextTransfer>>>,
    checkpoint: Arc<CommitCheckpoint>,
}
#[async_trait]
impl InstalledOriginalTaskOwner for WorkflowOriginalOwner {
    async fn original_task(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<TaskContinuation, ToolAdmissionError> {
        self.native.original_task(invocation).await
    }
    async fn original_binding(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<InstalledOriginalTaskBinding, ToolAdmissionError> {
        let (original, backend, work) =
            self.selected.get().ok_or(ToolAdmissionError::Unavailable)?;
        if invocation.original() != original {
            return Err(ToolAdmissionError::Denied);
        }
        let (_, member, _) = self.source.selected_source_route(&original.route).unwrap();
        assert_eq!(self.proposal_target, id(&member.member.member));
        assert_eq!(member.member.member, MEMBER);
        assert_eq!(
            invocation.association().candidate().target.logical_owner,
            member.logical_owner
        );
        work.check_current(backend)
            .await
            .map_err(|_| ToolAdmissionError::Denied)?;
        let planning = self
            .planning
            .get()
            .ok_or(ToolAdmissionError::Unavailable)?
            .clone();
        let recipient = WorkGraphService::with_scope(
            backend.service().store().clone(),
            backend.service().default_realm_id(),
            planning
                .context_namespace()
                .map_err(|_| ToolAdmissionError::Denied)?,
        );
        let transfer = self
            .source
            .prepare_workflow_context_transfer(
                &invocation,
                planning.clone(),
                recipient,
                backend,
                [id("person")].into(),
            )
            .await
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => ToolAdmissionError::Unavailable,
                _ => ToolAdmissionError::Denied,
            })?;
        // Real continuation verifies this invocation's run/session/service,
        // retained work, roster membership and runtime-input scope unchanged.
        let continuation = self.native.original_task(invocation).await?;
        // Admission reobserves this same invocation before advance. Keep the
        // first actual transfer/checkpoint owner and reject a changed choice;
        // an equivalent fresh proposal must not replace the retained attempt.
        let transfer = {
            let mut retained = self.transfer.lock().unwrap();
            match retained.as_ref() {
                Some(first) => {
                    assert!(
                        first.same_transfer_for_test(&transfer).unwrap(),
                        "repeated A binding preserves the exact transfer selection and owners"
                    );
                    first.clone()
                }
                None => {
                    transfer.observe_commit_for_test(self.checkpoint.clone());
                    *retained = Some(transfer.clone());
                    transfer
                }
            }
        };
        Ok(InstalledOriginalTaskBinding {
            continuation,
            work_context: Some(work.clone()),
            planning: Some(planning),
            context_transfer: Some(transfer),
        })
    }
}
#[async_trait]
impl SetupContinuationOwner for WorkflowOriginalOwner {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        expected: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.native.verify_admission(obligation, expected).await
    }
}
#[async_trait]
impl SetupContinuationDelivery for WorkflowOriginalOwner {
    async fn authorize_delivery(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        self.native.authorize_delivery(obligation).await
    }
    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.native.admit(obligation).await
    }
    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        self.native.reconcile(obligation).await
    }
}

// Only the physical response for A's exact retained original prompt emits Find.
// P and G keep the same real native-authorized provider response as before.
struct OriginalModel {
    inner: Arc<dyn LlmClient>,
    find: Arc<dyn LlmClient>,
}
#[async_trait]
impl LlmClient for OriginalModel {
    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        if request.request().messages.iter().any(|message| {
            matches!(message,
            Message::User(user) if user.text_content() == PROMPT)
        }) {
            self.find.stream_prepared(request)
        } else {
            self.inner.stream_prepared(request)
        }
    }
}
