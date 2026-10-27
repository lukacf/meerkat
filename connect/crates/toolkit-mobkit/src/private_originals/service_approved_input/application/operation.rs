//! One owned service operation. Canceling a borrowed drive/result waiter does
//! not drop its preparation, blocking-worker handle or conditional write.
use super::*;
use std::{future::Future, pin::Pin, sync::atomic::{AtomicBool, Ordering}};
use tokio::sync::{oneshot, watch, Notify};
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceParentOperationOutcome { Succeeded, RefusedBeforeEntry, FailedAfterEffects, Uncertain }

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceParentCustody { Running, Joined }

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceParentOperationReason { RequiresHuman, Denied, Unavailable, NotSettled, Conflict, Cancelled }

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceParentOperationObservation {
    pub outcome: Option<ServiceParentOperationOutcome>,
    pub custody: ServiceParentCustody,
    pub reason: Option<ServiceParentOperationReason>,
    pub parent_issuance_id: Option<EvidenceId>,
    pub parent_issued: bool,
    pub child: Option<PrivateServiceGrantObservation>,
    pub source: Option<PrivateOriginalRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<ServiceToolPolicyObservation>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApplicationServicePreparedStatus { PreparedForEntry, RequiresHuman }

#[derive(Clone, Copy)]
pub(crate) enum ServiceOperationError {
    Source(PrivateOriginalError),
    Authority(HostLifetimeError),
    Cancelled,
    RequiresHuman,
}
impl From<PrivateOriginalError> for ServiceOperationError {
    fn from(error: PrivateOriginalError) -> Self { Self::Source(error) }
}
impl From<HostLifetimeError> for ServiceOperationError {
    fn from(error: HostLifetimeError) -> Self { Self::Authority(error) }
}
impl ServiceOperationError {
    pub(crate) fn source(self) -> PrivateOriginalError {
        match self { Self::Source(error) => error, Self::Authority(error) => policy_error(error),
            Self::Cancelled | Self::RequiresHuman => PrivateOriginalError::Denied }
    }
    fn policy_current(self) -> HostLifetimeError {
        match self {
            Self::Authority(error) => error,
            Self::Source(PrivateOriginalError::Unavailable) => HostLifetimeError::Unavailable,
            Self::Source(PrivateOriginalError::Conflict) => HostLifetimeError::InvalidSelection,
            _ => HostLifetimeError::Denied,
        }
    }
    fn reason(self) -> ServiceParentOperationReason {
        match self {
            Self::Source(PrivateOriginalError::Unavailable) | Self::Authority(HostLifetimeError::Unavailable) => ServiceParentOperationReason::Unavailable,
            Self::Authority(HostLifetimeError::NotSettled) => ServiceParentOperationReason::NotSettled,
            Self::Source(PrivateOriginalError::Conflict) => ServiceParentOperationReason::Conflict,
            Self::Cancelled => ServiceParentOperationReason::Cancelled,
            Self::RequiresHuman => ServiceParentOperationReason::RequiresHuman,
            _ => ServiceParentOperationReason::Denied,
        }
    }
}

type EnteredCurrent = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) type ServiceOperationCurrent = Arc<dyn Fn() -> Result<(), ServiceOperationError> + Send + Sync>;

struct ControlState {
    prepared: Option<ApplicationServicePreparedStatus>,
    entry: Option<oneshot::Sender<EnteredCurrent>>,
    retired: bool,
}
struct OperationShared {
    control: Mutex<ControlState>,
    changed: Notify,
    retired: AtomicBool,
    observation: watch::Sender<ServiceParentOperationObservation>,
}

/// This observer has no entry, authorization, retirement or driving method.
#[derive(Clone)]
pub struct ServiceParentOperationObserver { shared: Arc<OperationShared> }
impl ServiceParentOperationObserver {
    pub fn observe(&self) -> ServiceParentOperationObservation { self.shared.observation.borrow().clone() }
    /// Observe the same cancellation fence while an authorization answer is
    /// pending. This grants no entry, retirement or completion authority.
    pub async fn wait_retiring(&self) {
        let mut receiver = self.shared.observation.subscribe();
        loop {
            if self.shared.retired.load(Ordering::Acquire) { return; }
            if receiver.changed().await.is_err() { return; }
        }
    }
    pub async fn result(&self) -> ServiceParentOperationObservation {
        let mut receiver = self.shared.observation.subscribe();
        loop {
            let observed = receiver.borrow_and_update().clone();
            if observed.custody == ServiceParentCustody::Joined { return observed; }
            if receiver.changed().await.is_err() { return self.observe(); }
        }
    }
}

#[derive(Clone)]
pub struct ApplicationServiceOperationControl { shared: Arc<OperationShared> }
impl ApplicationServiceOperationControl {
    pub fn same_operation(&self, observer: &ServiceParentOperationObserver) -> bool {
        Arc::ptr_eq(&self.shared, &observer.shared)
    }
    pub async fn prepared(&self) -> Result<ApplicationServicePreparedStatus, PrivateOriginalError> {
        loop {
            let notified = self.shared.changed.notified();
            {
                let state = self.shared.control.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
                if let Some(prepared) = state.prepared { return Ok(prepared); }
                let observed = self.shared.observation.borrow();
                if observed.custody == ServiceParentCustody::Joined || state.retired {
                    return Err(match observed.reason {
                        Some(ServiceParentOperationReason::Unavailable | ServiceParentOperationReason::NotSettled) => PrivateOriginalError::Unavailable,
                        Some(ServiceParentOperationReason::Conflict) => PrivateOriginalError::Conflict,
                        _ => PrivateOriginalError::Denied,
                    });
                }
            }
            notified.await;
        }
    }
    /// The trusted caller enters its real ReviewedEntry first and captures its
    /// custody in current. This method never constructs or replaces that entry.
    pub fn enter(&self, current: impl Fn() -> bool + Send + Sync + 'static) -> Result<(), PrivateOriginalError> {
        let mut state = self.shared.control.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        if state.retired || state.prepared != Some(ApplicationServicePreparedStatus::PreparedForEntry) {
            return Err(PrivateOriginalError::Denied);
        }
        let entry = state.entry.take().ok_or(PrivateOriginalError::Denied)?;
        entry.send(Arc::new(current)).map_err(|_| PrivateOriginalError::Unavailable)
    }
    pub fn begin_retire(&self) {
        self.shared.retired.store(true, Ordering::Release);
        if let Ok(mut state) = self.shared.control.lock() { state.retired = true; state.entry.take(); }
        self.shared.observation.send_modify(|observed| {
            if observed.custody == ServiceParentCustody::Running { observed.reason = Some(ServiceParentOperationReason::Cancelled); }
        });
        self.shared.changed.notify_waiters();
    }
    pub async fn result(&self) -> ServiceParentOperationObservation {
        ServiceParentOperationObserver { shared: self.shared.clone() }.result().await
    }
    /// Wake the existing owner while reserve is pending. This observes the
    /// same retirement fence and neither cancels work nor reports completion.
    pub async fn wait_retiring(&self) {
        ServiceParentOperationObserver { shared: self.shared.clone() }.wait_retiring().await;
    }
    pub async fn retire(&self) -> ServiceParentOperationObservation { self.begin_retire(); self.result().await }
}

/// Retain and drive this value in the embedding host's existing task/actor.
/// Drop fences entry but cannot perform asynchronous retirement or claim join.
pub struct ApplicationServiceOperation {
    work: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    shared: Arc<OperationShared>,
}
impl ApplicationServiceOperation {
    pub fn control(&self) -> ApplicationServiceOperationControl { ApplicationServiceOperationControl { shared: self.shared.clone() } }
    pub async fn drive(&mut self) -> ServiceParentOperationObservation {
        if let Some(work) = self.work.as_mut() { work.as_mut().await; self.work = None; }
        self.shared.observation.borrow().clone()
    }
    pub async fn retire(&mut self) -> ServiceParentOperationObservation { self.control().begin_retire(); self.drive().await }
}
impl Drop for ApplicationServiceOperation { fn drop(&mut self) { self.control().begin_retire(); } }

/// Private execution observations, populated by the actual grant/source path.
/// They are not replay authority or a replacement for durable native intent.
#[derive(Default)]
pub(crate) struct ServiceOperationProgress {
    entered: AtomicBool,
    uncertain: AtomicBool,
    known_effect: AtomicBool,
    native_refused: AtomicBool,
    parent_issued: AtomicBool,
    parent_issuance_id: std::sync::OnceLock<EvidenceId>,
}
impl ServiceOperationProgress {
    pub(crate) fn parent_attempt(&self, issuance_id: &EvidenceId) {
        // The canonical operation computes this coordinate once. Retaining
        // it is observation only, including a late preparation failure.
        let _ = self.parent_issuance_id.set(issuance_id.clone());
    }
    pub(crate) fn entering(&self) { self.entered.store(true, Ordering::Release); }
    pub(crate) fn uncertain(&self) { self.uncertain.store(true, Ordering::Release); }
    pub(crate) fn effect(&self) { self.known_effect.store(true, Ordering::Release); }
    pub(crate) fn native_refused(&self) { self.native_refused.store(true, Ordering::Release); }
    pub(crate) fn parent_effect(&self) { self.parent_issued.store(true, Ordering::Release); self.effect(); }
    pub(crate) fn resolved_pending_parent(&self) { self.uncertain.store(false, Ordering::Release); }
}

impl PreparedApplicationServiceInstruction {
    async fn apply_owned(self, invocation: EnteredCurrent, progress: Arc<ServiceOperationProgress>,
        shared: Arc<OperationShared>) -> Result<(), ServiceOperationError> {
        let Self { task, mut approved, reservation, parent_provisioning, administrator, tool_policy, tool_policy_approval } = self;
        let administrator = administrator.ok_or(PrivateOriginalError::Denied)?;
        let (operator, parent) = parent_provisioning.ok_or(PrivateOriginalError::Denied)?;
        task.current().await?;
        let login = task.console_login.clone();
        let login_reservation = login.as_ref().map(|login| login.reserve()).transpose()
            .map_err(|_| PrivateOriginalError::Denied)?;
        let retained_login = Arc::new(Mutex::new(login_reservation));
        let worker_login = retained_login.clone();
        let current: ServiceOperationCurrent = Arc::new(move || {
            let _actual_login_custody = &worker_login;
            reservation.check_current()?;
            administrator.check_current()?;
            if !invocation() { return Err(ServiceOperationError::Cancelled); }
            if login.as_ref().is_some_and(|login| login.try_check().is_err()) {
                return Err(PrivateOriginalError::Denied.into());
            }
            Ok(())
        });
        let result = async {
            current()?;
            let mut tool_policy = match (tool_policy, tool_policy_approval) {
                (None, None) => None,
                (Some(prepared), Some(approval)) => {
                    let begun = prepared.begin(&approval, &|| current().map_err(ServiceOperationError::policy_current)).await;
                    let transaction = match begun {
                        Ok(transaction) => { progress.effect(); transaction },
                        Err(error) => {
                            if matches!(error, HostLifetimeError::Unavailable | HostLifetimeError::NotSettled) { progress.uncertain(); }
                            return Err(error.into());
                        }
                    };
                    shared.observation.send_modify(|state| state.tool_policy = Some(transaction.observation()));
                    Some(transaction)
                }
                _ => return Err(PrivateOriginalError::Denied.into()),
            };
            let parent = operator.apply_standing_service_parent(parent, &task, current.clone(), progress.clone()).await?;
            approved.parent = Some(match parent {
                ServiceMcpParentOutcome::Approved(parent) => {
                    shared.observation.send_modify(|observed| observed.parent_issued = true);
                    parent
                }
                ServiceMcpParentOutcome::Denied => return Err(PrivateOriginalError::Denied.into()),
                ServiceMcpParentOutcome::Unavailable | ServiceMcpParentOutcome::PendingReconciliation => {
                    progress.uncertain();
                    return Err(PrivateOriginalError::Unavailable.into());
                }
            });
            task.current().await?;
            current()?;
            let evidence = approved.decision.evidence.clone();
            let observed = task.source.apply_application_service_access_owned(approved, current.clone(), Some(progress.clone())).await?;
            shared.observation.send_modify(|state| state.child = Some(observed.clone()));
            match observed.state {
                PrivateServiceGrantState::GrantIssued => { progress.effect(); }
                PrivateServiceGrantState::ApprovalDenied => return Err(PrivateOriginalError::Denied.into()),
                PrivateServiceGrantState::GrantUnavailable if progress.native_refused.load(Ordering::Acquire) =>
                    return Err(PrivateOriginalError::Denied.into()),
                PrivateServiceGrantState::GrantUnavailable | PrivateServiceGrantState::PendingReconciliation => {
                    progress.uncertain(); return Err(PrivateOriginalError::Unavailable.into());
                }
            }
            task.current().await?;
            current()?;
            let condition = match &mut tool_policy {
                Some(policy) => {
                    let published = policy.publish(&|| current().map_err(ServiceOperationError::policy_current)).await;
                    let observation = policy.observation();
                    if observation.native == Some(ServiceToolPolicyNativeOutcome::Accepted) { progress.effect(); }
                    shared.observation.send_modify(|state| state.tool_policy = Some(observation));
                    match published {
                        Ok(condition) => Some(condition),
                        Err(error) => {
                            if matches!(error, HostLifetimeError::Unavailable | HostLifetimeError::NotSettled) { progress.uncertain(); }
                            return Err(error.into());
                        }
                    }
                }
                None => None,
            };
            // The same retained future owns this conditional write and the
            // policy-slot exclusion until actual return, including retirement.
            let retained = task.retain_approved_with_policy(observed.grant, evidence, &|| current(), condition).await;
            let source = match retained {
                Ok(source) => source,
                Err(error) => {
                    if matches!(error, ServiceOperationError::Source(PrivateOriginalError::Unavailable)) { progress.uncertain(); }
                    return Err(error);
                }
            };
            progress.effect();
            shared.observation.send_modify(|state| state.source = Some(source));
            current()?;
            Ok(())
        }.await;
        // Every blocking handle and submitted store call above has returned.
        // Count settlement is independent from native outcome certainty.
        if let Some(login) = retained_login.lock().map_err(|_| PrivateOriginalError::Unavailable)?.take() {
            if progress.uncertain.load(Ordering::Acquire) { login.uncertain(); } else { login.settled(); }
        }
        result
    }
}

impl InstalledServiceTask {
    async fn prepare_application_service_owned(self, policy: &InstalledApplicationServicePolicy,
        config: &meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
        operations: BTreeSet<Id>, expires_in_ms: Option<u64>, observer: ServiceParentOperationObserver,
        shared: Arc<OperationShared>, progress: &ServiceOperationProgress,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServicePreparation, ServiceOperationError> {
        let preparation = self.prepare_application_service(policy, config, selection, operations, expires_in_ms, Some(progress), tool_policy).await?;
        let mut prepared = match preparation {
            ApplicationServicePreparation::RequiresHuman => return Ok(ApplicationServicePreparation::RequiresHuman),
            ApplicationServicePreparation::Prepared(prepared) => prepared,
        };
        if shared.retired.load(Ordering::Acquire) { return Err(ServiceOperationError::Cancelled); }
        let (_, _, authorizer) = policy.parent_provisioning.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let (_, parent) = prepared.parent_provisioning.as_mut().ok_or(PrivateOriginalError::Denied)?;
        let proposal = parent.authorization_proposal()?;
        shared.observation.send_modify(|observed| observed.parent_issuance_id = Some(proposal.issuance_id.clone()));
        let authorization = authorizer.reserve(&ServiceParentAuthorizationRequest {
            proposal: &proposal, configuration: config, observation: observer,
        }).await;
        // Retirement keeps driving reserve until its actual producer settles.
        // It does not drop that future or mistake an answer for completion.
        if shared.retired.load(Ordering::Acquire) { return Err(ServiceOperationError::Cancelled); }
        let reservation = match authorization? {
            ServiceParentAuthorization::RequiresHuman => return Ok(ApplicationServicePreparation::RequiresHuman),
            ServiceParentAuthorization::Reserved(reservation) => reservation,
        };
        // An awaited answer does not bypass current source, service authority,
        // administrative authority or the original exact proposal binding.
        prepared.task.current().await?;
        policy.check(&prepared.task)?;
        prepared.reservation.check_current()?;
        reservation.check_current()?;
        if shared.retired.load(Ordering::Acquire) { return Err(ServiceOperationError::Cancelled); }
        if reservation.operator() != &proposal.operator || reservation.rule() != &proposal.rule
            || reservation.proposal_sha256() != proposal.proposal_sha256()?
        { return Err(PrivateOriginalError::Denied.into()); }
        match (&proposal.tool_policy, reservation.tool_policy_approval()) {
            (None, None) => {},
            (Some(proposal), Some(approval)) => approval.check(proposal)?,
            _ => return Err(PrivateOriginalError::Denied.into()),
        }
        parent.authorize(ServiceParentAuthorizationEvidence {
            proposal_sha256: reservation.proposal_sha256().to_owned(), provenance: reservation.provenance().clone(),
        })?;
        prepared.administrator = Some(Arc::from(reservation));
        Ok(ApplicationServicePreparation::Prepared(prepared))
    }
    pub fn begin_application_connector(self, policy: Arc<InstalledApplicationServicePolicy>,
        installation: meerkat::ConnectorMcpInstallation, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
    ) -> Result<ApplicationServiceOperation, PrivateOriginalError> {
        self.begin_application_connector_with_tool_policy(policy, installation, operations, expires_in_ms, None)
    }
    pub fn begin_application_no_connector(self, policy: Arc<InstalledApplicationServicePolicy>,
        config: meerkat_core::McpServerConfig, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
    ) -> Result<ApplicationServiceOperation, PrivateOriginalError> {
        self.begin_application_no_connector_with_tool_policy(policy, config, operations, expires_in_ms, None)
    }
    pub fn begin_application_connector_with_tool_policy(self, policy: Arc<InstalledApplicationServicePolicy>,
        installation: meerkat::ConnectorMcpInstallation, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServiceOperation, PrivateOriginalError> {
        let config = installation.config().clone();
        let selection = ServiceMcpSelection::connector(&installation);
        self.begin_application_service(policy, config, selection, operations, expires_in_ms, tool_policy)
    }
    pub fn begin_application_no_connector_with_tool_policy(self, policy: Arc<InstalledApplicationServicePolicy>,
        config: meerkat_core::McpServerConfig, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServiceOperation, PrivateOriginalError> {
        let selection = ServiceMcpSelection::no_connector(&config)?;
        self.begin_application_service(policy, config, selection, operations, expires_in_ms, tool_policy)
    }
    fn begin_application_service(self, policy: Arc<InstalledApplicationServicePolicy>,
        config: meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
        operations: BTreeSet<Id>, expires_in_ms: Option<u64>, tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServiceOperation, PrivateOriginalError> {
        if policy.parent_provisioning.is_none() { return Err(PrivateOriginalError::Denied); }
        let (entry, receiver) = oneshot::channel();
        let (observation, _) = watch::channel(ServiceParentOperationObservation { outcome: None,
            custody: ServiceParentCustody::Running, reason: None, parent_issuance_id: None,
            parent_issued: false, child: None, source: None, tool_policy: None });
        let shared = Arc::new(OperationShared { control: Mutex::new(ControlState {
            prepared: None, entry: Some(entry), retired: false }), changed: Notify::new(),
            retired: AtomicBool::new(false), observation });
        let owned = shared.clone();
        let work = Box::pin(async move {
            let progress = Arc::new(ServiceOperationProgress::default());
            let observer = ServiceParentOperationObserver { shared: owned.clone() };
            let result = run(self, policy, config, selection, operations, expires_in_ms,
                owned.clone(), observer, receiver, progress.clone(), tool_policy).await;
            owned.observation.send_modify(|observed| {
                observed.parent_issued |= progress.parent_issued.load(Ordering::Acquire);
                if observed.parent_issuance_id.is_none() {
                    observed.parent_issuance_id = progress.parent_issuance_id.get().cloned();
                }
                observed.reason = result.err().map(ServiceOperationError::reason);
                observed.outcome = Some(operation_outcome(&result, &progress));
                // run has awaited every entered native handle and source call.
                observed.custody = ServiceParentCustody::Joined;
            });
            owned.changed.notify_waiters();
        });
        Ok(ApplicationServiceOperation { work: Some(work), shared })
    }
}

fn operation_outcome(result: &Result<(), ServiceOperationError>, progress: &ServiceOperationProgress)
    -> ServiceParentOperationOutcome {
    if result.is_ok() { ServiceParentOperationOutcome::Succeeded }
    else if progress.uncertain.load(Ordering::Acquire) { ServiceParentOperationOutcome::Uncertain }
    else if progress.known_effect.load(Ordering::Acquire) { ServiceParentOperationOutcome::FailedAfterEffects }
    else if progress.entered.load(Ordering::Acquire) && !progress.native_refused.load(Ordering::Acquire) {
        ServiceParentOperationOutcome::Uncertain
    } else { ServiceParentOperationOutcome::RefusedBeforeEntry }
}

async fn run(task: InstalledServiceTask, policy: Arc<InstalledApplicationServicePolicy>,
    config: meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
    operations: BTreeSet<Id>, expires_in_ms: Option<u64>, shared: Arc<OperationShared>,
    observer: ServiceParentOperationObserver, entry: oneshot::Receiver<EnteredCurrent>,
    progress: Arc<ServiceOperationProgress>, tool_policy: Option<ServiceToolPolicySelection>,
) -> Result<(), ServiceOperationError> {
    if shared.retired.load(Ordering::Acquire) { return Err(ServiceOperationError::Cancelled); }
    let prepared = task.prepare_application_service_owned(&policy, &config, selection, operations,
        expires_in_ms, observer, shared.clone(), &progress, tool_policy).await?;
    let prepared = match prepared {
        ApplicationServicePreparation::RequiresHuman => {
            shared.control.lock().map_err(|_| PrivateOriginalError::Unavailable)?.prepared = Some(ApplicationServicePreparedStatus::RequiresHuman);
            shared.observation.send_modify(|observed| observed.reason = Some(ServiceParentOperationReason::RequiresHuman));
            shared.changed.notify_waiters();
            return Err(ServiceOperationError::RequiresHuman);
        }
        ApplicationServicePreparation::Prepared(prepared) => prepared,
    };
    {
        let mut state = shared.control.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        if state.retired { return Err(ServiceOperationError::Cancelled); }
        state.prepared = Some(ApplicationServicePreparedStatus::PreparedForEntry);
    }
    shared.changed.notify_waiters();
    let entered = entry.await.map_err(|_| ServiceOperationError::Cancelled)?;
    let current_shared = shared.clone();
    let current = Arc::new(move || !current_shared.retired.load(Ordering::Acquire) && entered());
    prepared.apply_owned(current, progress, shared).await
}
