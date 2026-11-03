//! Host-selected first G input through the existing Original binder and outbox.
//! The launch receipt never acknowledges any waiter's context-delivery intent.
use super::*;
#[cfg(any(test, feature = "test-support"))]
use crate::original_ingress::workflow_custody::{WorkflowLaunchDiagnostics, WorkflowLaunchStage};
use crate::review_custody::ReviewCustody;

mod child;
mod console_child;
mod late_helper;
pub(crate) use child::RetainedWorkflowChild;
pub use child::{
    InstalledWorkflowChild, WorkflowChildLaunchRequest, WorkflowChildPreparationError,
};
pub(in crate::private_originals) use console_child::PreparedConsoleChild;
pub use console_child::{
    ConsoleWorkflowChildInput, ConsoleWorkflowChildPreparer, ConsoleWorkflowChildRequest,
};
pub use late_helper::{InstalledLateWorkflowHelper, InstalledLateWorkflowWaiter};
mod context_transfer;
mod effects;
#[cfg(test)]
pub(crate) use context_transfer::OwnedTransferCommit;
pub(crate) use context_transfer::WorkflowTransferObservation;
pub use context_transfer::{
    InstalledWorkflowContext, InstalledWorkflowContextTransfer, WorkflowContextObservation,
    WorkflowDeliveryObservation,
};
mod close;
mod recovery;
mod restore;
use crate::private_originals::governed_submit::{OriginalBinder, evidence};
#[cfg(test)]
pub(crate) use close::WorkflowCloseObservation;
pub(crate) use close::WorkflowTerminalRecovery;
use meerkat_authorization_contracts::{constraints::DelegationDepth, grant::GrantLineageRef};
use meerkat_mobkit::identity_first::{
    BoundGovernedInput, GovernedBindContext, GovernedEntryFence, GovernedEntryOutcome,
    GovernedEntryPermit, GovernedEntryRefusal, GovernedInputBinder, GovernedInputError,
    GovernedSettlement, IdentityRuntimeError, TurnTracking,
};
use meerkat_runtime::{Input, PromptInput};
use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;
use meerkat_toolkit::setup::{SetupWorkContext, SetupWorkflowTerms};
pub(crate) use recovery::WorkflowInterestRecovery;
use toolkit_store::{
    EffectClaimOutcome, EffectLease, EffectOutcome, EffectStatus, Reconciliation, StoreError,
};

// Fixed failure labels reuse the existing sink. No source/grant/error payload
// is formatted, and observation failure never changes the returned error.
fn child_variant(error: &PrivateOriginalError) -> &'static str {
    match error {
        PrivateOriginalError::Denied => "PrivateOriginalError::Denied",
        PrivateOriginalError::Unavailable => "PrivateOriginalError::Unavailable",
        PrivateOriginalError::Conflict => "PrivateOriginalError::Conflict",
    }
}

fn child_failure(stage: &'static str, error: &PrivateOriginalError) {
    crate::aa1_failure(stage, child_variant(error));
}

fn child_host_variant(
    error: &meerkat_toolkit::host_lifetime::control::HostLifetimeError,
) -> &'static str {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    match error {
        HostLifetimeError::InvalidSelection => "HostLifetimeError::InvalidSelection",
        HostLifetimeError::Denied => "HostLifetimeError::Denied",
        HostLifetimeError::WrongOwner => "HostLifetimeError::WrongOwner",
        HostLifetimeError::Unavailable => "HostLifetimeError::Unavailable",
        HostLifetimeError::NotSettled => "HostLifetimeError::NotSettled",
    }
}

fn child_writer_variant(error: &OriginalWriterError) -> &'static str {
    use meerkat_authorization::grants::GrantRefusal;
    match error {
        OriginalWriterError::Unavailable => "OriginalWriterError::Unavailable",
        OriginalWriterError::Conflict => "OriginalWriterError::Conflict",
        OriginalWriterError::WrongOwner => "OriginalWriterError::WrongOwner",
        OriginalWriterError::UnsupportedMandate => "OriginalWriterError::UnsupportedMandate",
        OriginalWriterError::OperationDenied => "OriginalWriterError::OperationDenied",
        OriginalWriterError::OperationCancelled => "OriginalWriterError::OperationCancelled",
        OriginalWriterError::Grant(GrantRefusal::Denied) => "OriginalWriterError::Grant::Denied",
        OriginalWriterError::Grant(GrantRefusal::Duplicate) => {
            "OriginalWriterError::Grant::Duplicate"
        }
        OriginalWriterError::Grant(GrantRefusal::ControllerInUse) => {
            "OriginalWriterError::Grant::ControllerInUse"
        }
        OriginalWriterError::Grant(GrantRefusal::Unavailable) => {
            "OriginalWriterError::Grant::Unavailable"
        }
    }
}

fn child_grant_variant(error: &meerkat_authorization::grants::GrantRefusal) -> &'static str {
    use meerkat_authorization::grants::GrantRefusal;
    match error {
        GrantRefusal::Denied => "GrantRefusal::Denied",
        GrantRefusal::Duplicate => "GrantRefusal::Duplicate",
        GrantRefusal::ControllerInUse => "GrantRefusal::ControllerInUse",
        GrantRefusal::Unavailable => "GrantRefusal::Unavailable",
    }
}

fn child_publication_variant(
    error: &meerkat_authorization::publication::PublicationError,
) -> &'static str {
    use meerkat_authorization::publication::PublicationError;
    match error {
        PublicationError::Changed => "PublicationError::Changed",
        PublicationError::Unavailable => "PublicationError::Unavailable",
    }
}

// One fixed label is sufficient: each synchronous check returns at its first
// refusal. The enclosing observation never performs diagnostic I/O.
type ChildCheckDiagnostic = (&'static str, &'static str);

// At most two observations, retaining only fixed variant labels. Delay every
// diagnostic until final refusal: a successful rebuild performs no logging.
type ChildObservationDiagnostic = (&'static str, &'static str, Option<ChildCheckDiagnostic>);

fn child_observation_failure(
    observations: &[Option<ChildObservationDiagnostic>; 2],
    terminal: &'static str,
) {
    let stages = [
        ("plan.observe.first.outer", "plan.observe.first.inner"),
        ("plan.observe.rebuild.outer", "plan.observe.rebuild.inner"),
    ];
    for (observation, (outer_stage, inner_stage)) in observations.iter().zip(stages) {
        if let Some((outer, inner, diagnostic)) = observation {
            crate::aa1_failure(outer_stage, outer);
            crate::aa1_failure(inner_stage, inner);
            if let Some((stage, variant)) = diagnostic {
                crate::aa1_failure(stage, variant);
            }
        }
    }
    crate::aa1_failure("plan.observe.refusal", terminal);
}

/// A binding read's answer to its caller: one still stale once its rebuild
/// (and, on a dispatch, its waits) ended is unavailability, never a denial.
fn binding_refusal(error: WorkflowChildPreparationError) -> PrivateOriginalError {
    match error {
        WorkflowChildPreparationError::ObservationChanged => PrivateOriginalError::Unavailable,
        WorkflowChildPreparationError::Refused(error) => error,
    }
}

fn observe_child_preparation(
    publication: &meerkat_authorization::publication::LocalAuthorizationPublication,
    mut read: impl FnMut(&mut Option<ChildCheckDiagnostic>) -> Result<(), WorkflowChildPreparationError>,
) -> Result<(), WorkflowChildPreparationError> {
    use WorkflowChildPreparationError::{ObservationChanged, Refused};
    let mut observations = [None; 2];
    for attempt in 0..2 {
        let mut unavailable = false;
        let mut diagnostic = None;
        let mut inner = "NotRead";
        let observed = publication.observe(|| {
            let result = read(&mut diagnostic);
            inner = match &result {
                Ok(()) => "Ok",
                Err(ObservationChanged) => "WorkflowChildPreparationError::ObservationChanged",
                Err(Refused(error)) => child_variant(error),
            };
            // Match Core controller admission: genuine inability to observe
            // wins over outer Changed. All other stale facts are discarded.
            unavailable = matches!(result, Err(Refused(PrivateOriginalError::Unavailable)));
            result
        });
        let outer = match &observed {
            Ok(_) => "PublicationObserved",
            Err(error) => child_publication_variant(error),
        };
        observations[attempt] = Some((outer, inner, diagnostic));
        if unavailable {
            child_observation_failure(&observations, "InnerUnavailablePrecedence");
            return Err(Refused(PrivateOriginalError::Unavailable));
        }
        let result = match observed {
            Ok((result, _stamp)) => result,
            Err(error) => Err(WorkflowChildPreparationError::publication(error)),
        };
        match result {
            Err(ObservationChanged) => continue,
            Err(Refused(error)) => {
                child_observation_failure(&observations, child_variant(&error));
                return Err(Refused(error));
            }
            Ok(()) => return Ok(()),
        }
    }
    child_observation_failure(&observations, "ObservationChangedRebuildExhausted");
    Err(ObservationChanged)
}

/// The one launch owner's observation, never waiter-context delivery. Only an
/// exact pre-admission binder refusal or native settled refusal can produce
/// the definitive NotAdmitted case.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkflowLaunchObservation {
    Native {
        observation: PrivateOriginalSubmission,
    },
    /// The current native owner verified G's original admitted input after
    /// restoration. No submission, source guard or turn ticket was created.
    HistoricalAdmitted {
        session_id: String,
        input_id: String,
    },
    NotAdmitted,
}

/// An actual host selection under PrivateOriginals, with no wire constructor.
/// Its independently authenticated parent and G task leaf are rechecked by
/// their existing owners. Workflow terms and grant references alone cannot
/// construct this handle or substitute for native admission.
pub struct InstalledWorkflowPlanning {
    source: Weak<PrivateOriginals>,
    parent: Arc<RetainedToolkitOriginal>,
    parent_reference: PrivateOriginalRef,
    submission: PrivateOriginalSubmit,
    terms: SetupWorkflowTerms,
    lineage: Vec<GrantLineageRef>,
    late_helper: Option<InstalledLateWorkflowHelper>,
    interests: Mutex<Vec<LaunchInterest>>,
    // Canonical completed-context ownership survives donor departure.
    context_transfers: Mutex<Vec<Arc<InstalledWorkflowContextTransfer>>>,
    // Bounded send proposals, not source or permission authority. Claimed
    // entries remain with this plan until its existing owner is dropped.
    console_children: Mutex<Vec<Arc<PreparedConsoleChild>>>,
    #[cfg(test)]
    binding_observed_for_test: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

struct LaunchInterest {
    membership: SetupWorkflowWaiter,
    // A closed membership may arrive before its local source interest.
    original: Option<Arc<RetainedToolkitOriginal>>,
    closed: bool,
    entries: Vec<Arc<crate::original_ingress::shared_entry::SharedEntry>>,
    effects: Option<effects::PreparedWaiterEffects>,
}

pub(crate) struct InvocationPlanning {
    correlation: Id,
    selection: Option<Arc<InstalledWorkflowPlanning>>,
    // Invocation selection is an alias; the recipient planning owner retains custody.
    context_transfer: Option<Weak<InstalledWorkflowContextTransfer>>,
}

/// Canonical launch data staged before native admission. No durable store read
/// is performed by the synchronous accepted callback.
pub(in crate::private_originals) struct PreparedWorkflowLaunch {
    selection: Arc<InstalledWorkflowPlanning>,
    reference: SetupWorkflowRef,
}

impl PrivateOriginals {
    /// Called by the host's original_binding producer. The source event must
    /// already be retained by this same authenticated application. The actual
    /// G task leaf must be independently issued to G, not borrowed from A.
    /// Its complete bounds are the workflow's frozen operation envelope.
    pub async fn prepare_workflow_planning(
        self: &Arc<Self>,
        parent: &PrivateOriginalRef,
        submission: PrivateOriginalSubmit,
        terms: SetupWorkflowTerms,
        lineage: Vec<GrantLineageRef>,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        self.prepare_workflow_planning_selected(parent, submission, terms, lineage, None)
            .await
    }

    /// Same planning owner, with the real late-G receipt supplied by the
    /// installed operator. This never manufactures a static helper selection.
    pub async fn prepare_late_workflow_planning(
        self: &Arc<Self>,
        parent: &PrivateOriginalRef,
        submission: PrivateOriginalSubmit,
        terms: SetupWorkflowTerms,
        helper: InstalledLateWorkflowHelper,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        helper
            .check(self)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let binding = helper
            .admission
            .workflow
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        if &binding.parent != parent
            || binding.original != submission.original
            || binding.workflow
                != self
                    .workflow_store()?
                    .workflow_ref(&terms)
                    .map_err(|_| PrivateOriginalError::Conflict)?
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let lineage = helper.lineage.clone();
        self.prepare_workflow_planning_selected(parent, submission, terms, lineage, Some(helper))
            .await
    }

    async fn prepare_workflow_planning_selected(
        self: &Arc<Self>,
        parent: &PrivateOriginalRef,
        submission: PrivateOriginalSubmit,
        terms: SetupWorkflowTerms,
        lineage: Vec<GrantLineageRef>,
        late_helper: Option<InstalledLateWorkflowHelper>,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        self.workflow_store()?
            .workflow_ref(&terms)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let parent = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| &held.reference == parent)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.owner
            .workflow_controller(&parent.original)
            .map_err(custody_error)?;
        let event = self.event(&submission.original).await?;
        let route = self.route(&submission.original.route)?;
        let p = parent.original.association().candidate();
        if terms.planning_source != self.key(&submission.original)?
            || terms.requester != p.requester
            || terms.realm != p.ingress_namespace.realm
            || route.requester != terms.requester
            || route.realm != terms.realm
            || route.source_authority != p.original_work.authority
            || event.contributors.as_slice() != [p.original_work.work.clone()]
            || digest(submission.content.as_bytes()) != event.content_sha256
            || submission.original == parent.reference
            || lineage.len() < 2
        {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let (compiled, member, _) = self.selected_source_route(&submission.original.route)?;
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let (requester, actor) = if native.routes.iter().any(|row| row.id == compiled.id) {
            let (_, requester, actor) = compiled
                .source_binding(selected)
                .map_err(|_| PrivateOriginalError::Denied)?;
            (requester, actor)
        } else {
            let CompiledNativeSourceOrigin::LocalConsole {
                requester,
                ingress_actor,
                ..
            } = &compiled.origin
            else {
                return Err(PrivateOriginalError::Denied);
            };
            (requester, ingress_actor)
        };
        if member.member != terms.lead
            || requester != &route.requester
            || actor != &route.ingress_actor
            || route.grant_target.as_str() != terms.lead.member
        {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some((admission, _)) = self
            .service_tool_policy
            .get()
            .map(|owner| owner.member_route(&submission.original.route))
            .transpose()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .flatten()
        {
            let workflow = admission
                .workflow
                .as_ref()
                .ok_or(PrivateOriginalError::Denied)?;
            if admission.class.purpose.is_ordinary()
                || workflow.parent != parent.reference
                || workflow.original != submission.original
                || workflow.workflow
                    != self
                        .workflow_store()?
                        .workflow_ref(&terms)
                        .map_err(|_| PrivateOriginalError::Conflict)?
            {
                return Err(PrivateOriginalError::Denied);
            }
        }
        let record = self
            .store
            .get(&self.key(&submission.original)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&record, &submission.original)?;
        if record.setup_request.is_some()
            || record.configured_setup.is_some()
            || record.approved_task.is_some()
            || record.operation_cancelled
        {
            return Err(PrivateOriginalError::Denied);
        }
        let candidate = Arc::new(InstalledWorkflowPlanning {
            source: Arc::downgrade(self),
            parent: parent.original.clone(),
            parent_reference: parent.reference.clone(),
            submission,
            terms,
            lineage,
            late_helper,
            interests: Mutex::new(Vec::new()),
            context_transfers: Mutex::new(Vec::new()),
            console_children: Mutex::new(Vec::new()),
            #[cfg(test)]
            binding_observed_for_test: Mutex::new(None),
        });
        candidate.check_for_binding(self, &candidate.submission.original)?;
        if record
            .final_association
            .as_ref()
            .is_some_and(|saved| saved.candidate().authority_basis != candidate.operation_basis())
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // This actual host planning choice opts G into exact input evidence.
        // A legacy already-bound G remains usable without retrofitting a body.
        self.select_workflow_lead_evidence(
            &candidate.submission.original,
            &parent.reference,
            candidate.late_helper.as_ref(),
        )
        .await?;
        // Intern this process custody on the actual authenticated P owner.
        // Equal terms cannot construct permission, and repeated host lookups
        // must select the same launch-interest reservations for A and B.
        let mut plans = parent
            .workflow_plans
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(prior) = plans
            .iter()
            .find(|plan| plan.submission.original == candidate.submission.original)
        {
            return if same_optional(Some(prior), Some(&candidate)) {
                Ok(prior.clone())
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        if plans.len() >= 128 {
            return Err(PrivateOriginalError::Unavailable);
        }
        plans.push(candidate.clone());
        Ok(candidate)
    }

    /// Retain the exact optional producer result on the existing live A owner.
    /// Re-entry through admission cannot replace Some, erase it, or infer it
    /// from portable terms. The first None is retained too: concurrent first
    /// admissions may both have observed absent durable invocation terms.
    #[cfg(test)]
    pub(crate) fn retain_workflow_selection(
        self: &Arc<Self>,
        original: &super::super::acquisition::AcquisitionOriginal,
        correlation: &Id,
        selection: Option<&Arc<InstalledWorkflowPlanning>>,
        work: Option<&SetupWorkContext>,
    ) -> Result<(), PrivateOriginalError> {
        self.retain_workflow_binding(original, correlation, selection, None, work)
    }

    // Only the pure owner reads are repeated. The caller retains the actual
    // WorkAuthorization; Core owns the fixed deadline, backoff and rebuild
    // count. Only typed stale observations permit a wait, including a change
    // that completed before the wait asks. Real unavailability and denial stop.
    pub(crate) async fn observe_workflow_for_admission<T>(
        &self,
        authorization: &dyn meerkat_core::authorization::WorkAuthorization,
        churn: &tokio::sync::Mutex<meerkat_core::authorization::OwnerChangeChurn>,
        deadline: tokio::time::Instant,
        mut read: impl FnMut() -> Result<T, WorkflowChildPreparationError>,
    ) -> Result<T, WorkflowChildPreparationError> {
        loop {
            match read() {
                Err(WorkflowChildPreparationError::ObservationChanged) => {
                    if !Self::wait_for_workflow_change(authorization, churn, deadline).await {
                        return Err(WorkflowChildPreparationError::ObservationChanged);
                    }
                }
                result => return result,
            }
        }
    }

    async fn wait_for_workflow_change(
        authorization: &dyn meerkat_core::authorization::WorkAuthorization,
        churn: &tokio::sync::Mutex<meerkat_core::authorization::OwnerChangeChurn>,
        deadline: tokio::time::Instant,
    ) -> bool {
        // This is only the invocation's wait-state mutex. No native/source/
        // selection/store guard is held by this caller across the wait.
        let mut churn = churn.lock().await;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        !remaining.is_zero()
            && churn
                .wait_after_stale_read(
                    authorization,
                    Some(remaining),
                    "toolkit.workflow.binding",
                    None,
                )
                .await
                .settled()
            && tokio::time::Instant::now() < deadline
    }

    #[cfg(test)]
    pub(crate) fn retain_workflow_binding(
        self: &Arc<Self>,
        original: &super::super::acquisition::AcquisitionOriginal,
        correlation: &Id,
        selection: Option<&Arc<InstalledWorkflowPlanning>>,
        context_transfer: Option<&Arc<InstalledWorkflowContextTransfer>>,
        work: Option<&SetupWorkContext>,
    ) -> Result<(), PrivateOriginalError> {
        self.check_workflow_binding_observed(
            original,
            correlation,
            selection,
            context_transfer,
            work,
        )
        .map_err(|error| match error {
            WorkflowChildPreparationError::ObservationChanged => PrivateOriginalError::Unavailable,
            WorkflowChildPreparationError::Refused(error) => error,
        })?;
        self.retain_workflow_binding_checked(original, correlation, selection, context_transfer)
    }

    pub(crate) async fn retain_workflow_binding_waiting<T, Before, Fut>(
        self: &Arc<Self>,
        original: &super::super::acquisition::AcquisitionOriginal,
        correlation: &Id,
        selection: Option<&Arc<InstalledWorkflowPlanning>>,
        context_transfer: Option<&Arc<InstalledWorkflowContextTransfer>>,
        work: Option<&SetupWorkContext>,
        authorization: &dyn meerkat_core::authorization::WorkAuthorization,
        churn: &tokio::sync::Mutex<meerkat_core::authorization::OwnerChangeChurn>,
        deadline: tokio::time::Instant,
        mut before_observation: Before,
    ) -> Result<T, WorkflowChildPreparationError>
    where
        Before: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, WorkflowChildPreparationError>>,
    {
        loop {
            // Only read-only admission facts may be supplied here. Re-read
            // them after each settled change, before the last synchronous
            // observation; a refusal has retained no new workflow choice.
            let facts = before_observation().await?;
            match self.check_workflow_binding_observed(
                original,
                correlation,
                selection,
                context_transfer,
                work,
            ) {
                Ok(()) => {
                    // No async gap after the final observation. This exact
                    // canonical retention is outside every retry path.
                    self.retain_workflow_binding_checked(
                        original,
                        correlation,
                        selection,
                        context_transfer,
                    )
                    .map_err(WorkflowChildPreparationError::Refused)?;
                    return Ok(facts);
                }
                Err(WorkflowChildPreparationError::ObservationChanged) => {
                    drop(facts);
                    if !Self::wait_for_workflow_change(authorization, churn, deadline).await {
                        return Err(WorkflowChildPreparationError::ObservationChanged);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn check_workflow_binding_observed(
        self: &Arc<Self>,
        original: &super::super::acquisition::AcquisitionOriginal,
        correlation: &Id,
        selection: Option<&Arc<InstalledWorkflowPlanning>>,
        context_transfer: Option<&Arc<InstalledWorkflowContextTransfer>>,
        work: Option<&SetupWorkContext>,
    ) -> Result<(), WorkflowChildPreparationError> {
        if let Some(transfer) = context_transfer {
            transfer.check_selection_observed(
                self,
                original,
                correlation,
                selection.ok_or(PrivateOriginalError::Denied)?,
            )?;
        }
        if let Some(selection) = selection {
            selection.check_child_preparation(self, &selection.submission.original)?;
            let work = work.ok_or(PrivateOriginalError::Denied)?;
            let a = original.original.association().candidate();
            if selection.terms.requester != a.requester
                || selection.terms.realm != a.ingress_namespace.realm
                || work.backend() != &selection.terms.backend
                || work.parent() != &selection.terms.parent
                || Arc::ptr_eq(&selection.parent, &original.original)
            {
                return Err(PrivateOriginalError::Denied.into());
            }
        }
        Ok(())
    }

    fn retain_workflow_binding_checked(
        self: &Arc<Self>,
        original: &super::super::acquisition::AcquisitionOriginal,
        correlation: &Id,
        selection: Option<&Arc<InstalledWorkflowPlanning>>,
        context_transfer: Option<&Arc<InstalledWorkflowContextTransfer>>,
    ) -> Result<(), PrivateOriginalError> {
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| Arc::ptr_eq(&held.original, &original.original))
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut selections = held
            .workflow_selections
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(prior) = selections
            .iter()
            .find(|item| &item.correlation == correlation)
        {
            return if same_optional(prior.selection.as_ref(), selection)
                && context_transfer::same_transfer(
                    prior
                        .context_transfer
                        .as_ref()
                        .map(|weak| weak.upgrade().ok_or(PrivateOriginalError::Unavailable))
                        .transpose()?
                        .as_ref(),
                    context_transfer,
                )? {
                Ok(())
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        if selections.len() >= 128 {
            return Err(PrivateOriginalError::Unavailable);
        }
        // All donor-selection failures precede canonical recipient retention.
        let context_transfer = match context_transfer {
            Some(candidate) => Some(Arc::downgrade(
                &selection
                    .ok_or(PrivateOriginalError::Denied)?
                    .retain_context_transfer(candidate.clone())?,
            )),
            None => None,
        };
        selections.push(InvocationPlanning {
            correlation: correlation.clone(),
            selection: selection.cloned(),
            context_transfer,
        });
        Ok(())
    }

    pub(crate) fn retained_workflow_selection(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        correlation: &Id,
    ) -> Result<Option<Arc<InstalledWorkflowPlanning>>, PrivateOriginalError> {
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| Arc::ptr_eq(&held.original, original))
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let selections = held
            .workflow_selections
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        // Custom pre-default SetupToolHost owners never selected this optional
        // binding. They remain singleton; a recorded Some can never disappear.
        Ok(selections
            .iter()
            .find(|item| &item.correlation == correlation)
            .and_then(|item| item.selection.clone()))
    }
}

impl InstalledWorkflowPlanning {
    pub(in crate::private_originals) fn matches_class_workflow(
        &self,
        binding: &crate::installed_host::dynamic_members::MemberClassWorkflowBinding,
    ) -> bool {
        self.source
            .upgrade()
            .and_then(|source| {
                source
                    .workflow_store()
                    .ok()
                    .and_then(|store| store.workflow_ref(&self.terms).ok())
            })
            .is_some_and(|reference| reference == binding.workflow)
            && self.parent_reference == binding.parent
            && self.submission.original == binding.original
    }
    pub(in crate::private_originals) fn operation_basis(
        &self,
    ) -> meerkat_authorization_contracts::work_association::WorkAuthorityBasis {
        self.late_helper.as_ref().map(InstalledLateWorkflowHelper::operation_basis)
            .unwrap_or_else(|| meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage {
                lineage: self.lineage.clone(),
            })
    }
    pub(in crate::private_originals) fn has_mcp_target(&self) -> bool {
        self.late_helper
            .as_ref()
            .is_some_and(|helper| helper.mcp_lineage.is_some())
    }
    pub(crate) fn terms(&self) -> &SetupWorkflowTerms {
        &self.terms
    }
    /// Correlation in the existing immutable environment revision, not a
    /// transport for the live source handle or a grant to use it.
    pub(crate) fn binding_digest(&self) -> Result<ContentDigest, PrivateOriginalError> {
        let bytes = if self.has_mcp_target() {
            canonical_json(&(
                "installed-workflow-selection-v2",
                &self.terms,
                &self.submission.original,
                &self.operation_basis(),
            ))
        } else {
            // Preserve the historical single-lineage selection digest exactly.
            canonical_json(&(
                "installed-workflow-selection-v1",
                &self.terms,
                &self.submission.original,
                &self.lineage,
            ))
        };
        bytes
            .map(|bytes| ContentDigest::of_bytes(&bytes))
            .map_err(|_| PrivateOriginalError::Conflict)
    }

    fn binding_prerequisites<'a>(
        &self,
        source: &'a Arc<PrivateOriginals>,
        original: &PrivateOriginalRef,
        diagnostic: &mut Option<ChildCheckDiagnostic>,
    ) -> Result<std::borrow::Cow<'a, PrivateOriginalRoute>, PrivateOriginalError> {
        let actual = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| *diagnostic = Some(("plan.source", child_variant(error))))?;
        if !Arc::ptr_eq(source, &actual) || original != &self.submission.original {
            *diagnostic = Some((
                "plan.source_identity",
                child_variant(&PrivateOriginalError::Denied),
            ));
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(helper) = &self.late_helper {
            helper
                .check(source)
                .inspect_err(|error| *diagnostic = Some(("plan.helper", child_host_variant(error))))
                .map_err(|_| PrivateOriginalError::Denied)?;
        }
        source
            .owner
            .workflow_controller(&self.parent)
            .inspect_err(|error| *diagnostic = Some(("plan.parent", child_writer_variant(error))))
            .map_err(custody_error)?;
        let now = source
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)
            .inspect_err(|error| *diagnostic = Some(("plan.clock", child_variant(error))))?
            .unix_ms;
        if now >= self.terms.deadline_ms {
            *diagnostic = Some((
                "plan.deadline",
                child_variant(&PrivateOriginalError::Denied),
            ));
            return Err(PrivateOriginalError::Denied);
        }
        source
            .route(&original.route)
            .inspect_err(|error| *diagnostic = Some(("plan.route", child_variant(error))))
    }

    fn check_binding_envelope(
        &self,
        grant: &meerkat_authorization::grants::ResolvedGrant,
        diagnostic: &mut Option<ChildCheckDiagnostic>,
    ) -> Result<(), PrivateOriginalError> {
        if grant.restrictions().delegation_depth != DelegationDepth::remaining(0)
            || grant.expires_at_ms() < self.terms.deadline_ms
            || ContentDigest::of_bytes(
                &canonical_json(grant.restrictions())
                    .map_err(|_| PrivateOriginalError::Conflict)
                    .inspect_err(|error| {
                        *diagnostic = Some(("plan.envelope_encoding", child_variant(error)))
                    })?,
            ) != self.terms.envelope_digest
        {
            *diagnostic = Some((
                "plan.envelope",
                child_variant(&PrivateOriginalError::Denied),
            ));
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }

    pub(in crate::private_originals) fn check_for_binding(
        &self,
        source: &Arc<PrivateOriginals>,
        original: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        self.check_child_preparation(source, original)
            .map_err(binding_refusal)
    }

    // A dispatch decision gets fresh Core wait state. Only the pure owner
    // observation repeats; every guard from that read is gone before awaiting.
    // Background recovery keeps its existing non-waiting authority contract.
    async fn check_for_launch(
        &self,
        source: &Arc<PrivateOriginals>,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<(), WorkflowChildPreparationError> {
        self.check_binding_after_owner_change(source, &self.submission.original, dispatch)
            .await
    }

    /// [`Self::check_for_binding`] for `original`, waiting on a dispatch: a
    /// binding read that stays stale after its one rebuild waits with the
    /// dispatch's own Core churn and reads again, within the dispatch and
    /// request deadlines. Without a dispatch it keeps the non-waiting form.
    async fn check_binding_after_owner_change(
        &self,
        source: &Arc<PrivateOriginals>,
        original: &PrivateOriginalRef,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<(), WorkflowChildPreparationError> {
        let Some((context, deadline)) = dispatch else {
            return self.check_for_binding(source, original).map_err(Into::into);
        };
        let work = context
            .work_authorization()
            .ok_or(PrivateOriginalError::Denied)?;
        let authorization = work.authorization();
        let mut churn = context.owner_change_churn();
        let remaining = || {
            let request = deadline.saturating_duration_since(tokio::time::Instant::now());
            context
                .remaining_tool_time()
                .map_or(request, |tool| tool.min(request))
        };
        loop {
            match self.check_child_preparation(source, original) {
                Err(WorkflowChildPreparationError::ObservationChanged) => {
                    let budget = remaining();
                    if budget.is_zero()
                        || !churn
                            .wait_after_stale_read(
                                authorization.as_ref(),
                                Some(budget),
                                "toolkit.workflow.launch_binding",
                                None,
                            )
                            .await
                            .settled()
                        || remaining().is_zero()
                    {
                        return Err(WorkflowChildPreparationError::ObservationChanged);
                    }
                }
                result => return result,
            }
        }
    }

    /// Shared pure binding read for child preparation and later invocation.
    /// Rebuild once on native Changed, rereading prerequisites each time. This
    /// does not repeat any async work, issuance, source write or submission.
    /// The child command preserves Changed; standalone checks map it above.
    fn check_child_preparation(
        &self,
        source: &Arc<PrivateOriginals>,
        original: &PrivateOriginalRef,
    ) -> Result<(), WorkflowChildPreparationError> {
        use WorkflowChildPreparationError::{ObservationChanged, Refused};
        use meerkat_authorization::grants::LineageResolutionRefusal;
        observe_child_preparation(&source.owner.publication(), |diagnostic| {
            // These facts feed resolution and must belong to this same view.
            let route = self.binding_prerequisites(source, original, diagnostic)?;
            let grant = source
                .owner
                .grants()
                .resolve_lineage_observed(
                    &self.lineage,
                    &route.logical_executor,
                    route.represented_subject.as_ref(),
                )
                .map_err(|error| match error {
                    LineageResolutionRefusal::Changed => ObservationChanged,
                    LineageResolutionRefusal::Refused(error) => {
                        *diagnostic = Some(("plan.grant_resolve", child_grant_variant(&error)));
                        Refused(match error {
                            meerkat_authorization::grants::GrantRefusal::Unavailable => {
                                PrivateOriginalError::Unavailable
                            }
                            _ => PrivateOriginalError::Denied,
                        })
                    }
                })?;
            self.check_binding_envelope(&grant, diagnostic)?;
            let mcp = if let Some(helper) = &self.late_helper {
                if let Some(target) = &helper.mcp_lineage {
                    let application = source.owner.grants();
                    let lifetime = source.status_lifetime().map_err(Refused)?;
                    let native = lifetime
                        .selected_policy()
                        .native_host
                        .as_ref()
                        .ok_or(Refused(PrivateOriginalError::Unavailable))?;
                    Some(crate::installed_host::operator_setup::workflow_mcp::resolve_target_observed(&application,
                        &native.grant_administrator, &route.logical_executor, route.represented_subject.as_ref(),
                        &self.lineage, target, grant.restrictions(), &helper.server)
                        .map_err(|error| match error {
                            LineageResolutionRefusal::Changed => ObservationChanged,
                            LineageResolutionRefusal::Refused(error) => Refused(match error {
                                meerkat_authorization::grants::GrantRefusal::Unavailable => PrivateOriginalError::Unavailable,
                                _ => PrivateOriginalError::Denied,
                            }),
                        })?)
                } else {
                    None
                }
            } else {
                None
            };
            #[cfg(test)]
            {
                let observed = self.binding_observed_for_test.lock().unwrap().take();
                if let Some(observed) = observed {
                    observed();
                }
            }
            if let Some(mcp) = &mcp {
                mcp.check_current()
                    .map_err(WorkflowChildPreparationError::publication)?;
            }
            grant
                .check_current()
                .inspect_err(|error| {
                    *diagnostic = Some(("plan.grant_current", child_publication_variant(error)))
                })
                .map_err(WorkflowChildPreparationError::publication)
        })
    }

    /// The installed coordinator supplies its actual conditional SQLite
    /// adapter, which must be the same Arc retained by this source owner.
    /// Storage predicates fence a queued worker after caller loss; the source
    /// writer also orders the live currentness check against cancellation.
    /// The join runs on the Find dispatch, so its binding read waits out a
    /// stale publication with that dispatch's Core churn, as a launch does.
    pub(crate) async fn join_waiter_conditionally(
        &self,
        selected_store: &Arc<toolkit_store::SqliteStore>,
        membership: &SetupWorkflowWaiter,
        original: &super::super::acquisition::AcquisitionOriginal,
        access: SetupAccess<'_>,
        backend: &meerkat_toolkit::setup::SetupWorkBackend,
        dispatch: (&meerkat_core::ToolDispatchContext, tokio::time::Instant),
    ) -> Result<
        Result<meerkat_toolkit::setup::SetupWorkflowJoin, meerkat_toolkit::setup::SetupError>,
        PrivateOriginalError,
    > {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.check_binding_after_owner_change(&source, &self.submission.original, Some(dispatch))
            .await
            .map_err(binding_refusal)?;
        let erased: Arc<dyn ToolkitStore> = selected_store.clone();
        if !Arc::ptr_eq(&source.store, &erased)
            || membership.source() != &original.source_record.key
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let scope = SetupScope::new(
            Id::new(source.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            Id::new(source.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
        );
        let store = SetupStore::new_conditional(selected_store.as_ref(), scope)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        source
            .owner
            .retain_followup_source(original.original.association(), || async {
                // This is the actual retained source row, not a portable workflow
                // ref. An intervening durable source change makes the commit fail.
                let condition = toolkit_store::RecordCondition {
                    key: original.source_record.key.clone(),
                    revision: original.source_record.revision,
                    content_digest: original.source_record.content_digest.clone(),
                    claim: None,
                    dispatch_claim: None,
                };
                Ok(store
                    .join_setup_workflow_conditionally(
                        &self.terms,
                        membership,
                        access,
                        backend,
                        condition,
                    )
                    .await)
            })
            .await
            .map_err(custody_error)
    }

    /// Validate the actual planning selection after the canonical source read,
    /// under the existing original writer. The shared persistence body still
    /// checks source facts, replay equality, cancellation and the source CAS.
    pub(in crate::private_originals) fn check_persistence_binding(
        &self,
        source: &PrivateOriginals,
        reference: &PrivateOriginalRef,
        record: &SourceRecord,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let actual = self
            .source
            .upgrade()
            .ok_or(OriginalWriterError::Unavailable)?;
        if !std::ptr::eq(source, actual.as_ref()) {
            return Err(OriginalWriterError::WrongOwner);
        }
        self.check_for_binding(&actual, reference)
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => OriginalWriterError::Unavailable,
                PrivateOriginalError::Conflict => OriginalWriterError::Conflict,
                PrivateOriginalError::Denied => OriginalWriterError::OperationDenied,
            })?;
        let claims = association.candidate();
        if record.setup_request.is_some()
            || record.configured_setup.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
            || source
                .key(reference)
                .map_err(|_| OriginalWriterError::WrongOwner)?
                != self.terms.planning_source
            || claims.requester != self.terms.requester
            || claims.ingress_namespace.realm != self.terms.realm
            || claims.contributing_work.as_slice()
                != [self.parent.association().candidate().original_work.clone()]
            || claims.authority_basis != self.operation_basis()
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    /// Canonical join is complete. Retain only actual A launch interest here.
    /// No pending native reservation exists until the final synchronous entry
    /// fence. P/G still supply the native input's independent authority. On
    /// the Find dispatch its binding read waits out a stale publication with
    /// that dispatch's Core churn, as the join does.
    pub(crate) async fn retain_launch_interest(
        self: &Arc<Self>,
        snapshot: &SetupWorkflowSnapshot,
        membership: &SetupWorkflowWaiter,
        original: &super::super::acquisition::AcquisitionOriginal,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.check_binding_after_owner_change(&source, &self.submission.original, dispatch)
            .await
            .map_err(binding_refusal)?;
        if snapshot.terms() != &self.terms
            || membership.workflow() != snapshot.reference()
            || membership.source() != &original.source_record.key
            || snapshot
                .departed(membership)
                .map_err(|_| PrivateOriginalError::Conflict)?
            || Arc::ptr_eq(&self.parent, &original.original)
        {
            return Err(PrivateOriginalError::Denied);
        }
        source
            .owner
            .retain_followup_source(original.original.association(), || async {
                let current = source
                    .workflow_store()
                    .map_err(|_| OriginalWriterError::Unavailable)?
                    .load_setup_workflow(membership.workflow())
                    .await
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                if current.terms() != &self.terms
                    || current
                        .departed(membership)
                        .map_err(|_| OriginalWriterError::Conflict)?
                {
                    return Err(OriginalWriterError::OperationDenied);
                }
                source.owner.workflow_controller(&original.original)?;
                {
                    let mut interests = self
                        .interests
                        .lock()
                        .map_err(|_| crate::original_ingress::OriginalWriterError::Unavailable)?;
                    if let Some(prior) = interests.iter().find(|interest| {
                        interest.membership.acquisition() == membership.acquisition()
                    }) {
                        return if prior.membership == *membership
                            && prior
                                .original
                                .as_ref()
                                .is_some_and(|held| Arc::ptr_eq(held, &original.original))
                            && !prior.closed
                        {
                            Ok(())
                        } else {
                            Err(crate::original_ingress::OriginalWriterError::Conflict)
                        };
                    }
                    if interests.len() >= usize::from(self.terms.max_waiters) {
                        return Err(crate::original_ingress::OriginalWriterError::Unavailable);
                    }
                    interests.push(LaunchInterest {
                        membership: membership.clone(),
                        original: Some(original.original.clone()),
                        closed: false,
                        entries: Vec::new(),
                        effects: None,
                    });
                    Ok(())
                }
            })
            .await
            .map_err(custody_error)
    }

    /// The actual waiter owner closes the same final-entry reservation before
    /// awaiting durable departure. A failed write leaves it closed; replay
    /// cannot reopen it. Accepted/uncertain native work remains owned until its
    /// actual admission observation, not inferred from this store transition.
    pub(crate) async fn depart_waiter(
        &self,
        membership: &SetupWorkflowWaiter,
    ) -> Result<SetupWorkflowSnapshot, PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if source
            .workflow_store()?
            .workflow_ref(&self.terms)
            .map_err(|_| PrivateOriginalError::Conflict)?
            != *membership.workflow()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let _writer = source.owner.source_writer().await;
        let prior_closed = {
            let interests = self
                .interests
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            match interests
                .iter()
                .find(|interest| interest.membership.acquisition() == membership.acquisition())
            {
                Some(interest) if interest.membership == *membership => Some(interest.closed),
                Some(_) => return Err(PrivateOriginalError::Conflict),
                None => None,
            }
        };
        if prior_closed.is_none() {
            // A lost join reply may have no local interest. Validate the exact
            // canonical row before this portable value can consume a slot.
            // The source writer prevents restoration from passing this read.
            let current = source
                .workflow_store()?
                .load_setup_workflow(membership.workflow())
                .await
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            if current.terms() != &self.terms {
                return Err(PrivateOriginalError::Conflict);
            }
            current
                .departed(membership)
                .map_err(|_| PrivateOriginalError::Conflict)?;
            if self
                .interests
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .len()
                >= usize::from(self.terms.max_waiters)
            {
                return Err(PrivateOriginalError::Unavailable);
            }
        }
        {
            let publication = source.owner.publication();
            // Only first closure changes source policy. Exact closed replay
            // retries entry closure and the canonical write without publishing.
            let _publication = if prior_closed == Some(true) {
                None
            } else {
                Some(
                    publication
                        .begin_owner_change()
                        .map_err(|_| PrivateOriginalError::Unavailable)?,
                )
            };
            let mut interests = self
                .interests
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            if let Some(interest) = interests
                .iter_mut()
                .find(|interest| interest.membership == *membership)
            {
                // An actual held interest closes before the first store await.
                interest.closed = true;
                for entry in &interest.entries {
                    entry.close().map_err(custody_error)?;
                }
            } else {
                // Exact membership was validated above under this writer.
                // A failed durable departure leaves its tombstone closed.
                interests.push(LaunchInterest {
                    membership: membership.clone(),
                    original: None,
                    closed: true,
                    entries: Vec::new(),
                    effects: None,
                });
            }
        }
        self.revoke_waiter_effects(&source, membership).await?;
        // The closed entry is now published. Keep only the async source
        // writer across storage: native readers see closure, and restoration
        // cannot pass until canonical departure settles or the caller drops.
        source
            .workflow_store()?
            .depart_setup_workflow(membership)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)
    }

    fn settle_launch_interest(&self) -> Result<(), PrivateOriginalError> {
        let interests = self
            .interests
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        for interest in interests.iter() {
            for entry in &interest.entries {
                entry.settle().map_err(custody_error)?;
            }
        }
        Ok(())
    }

    /// Claim exactly the existing launch intent. An expired/lost claim is
    /// observed only: it never causes a second G input to be invented.
    #[cfg(test)]
    pub(crate) async fn launch(
        self: &Arc<Self>,
        snapshot: &SetupWorkflowSnapshot,
    ) -> Result<WorkflowLaunchObservation, PrivateOriginalError> {
        self.launch_with_review(snapshot, ReviewCustody::default())
            .await
    }

    /// Retain this invocation's consumed review through its actual native
    /// binder and source handoff, without changing launch authority or replay.
    #[cfg(test)]
    pub(crate) async fn launch_with_review(
        self: &Arc<Self>,
        snapshot: &SetupWorkflowSnapshot,
        review_custody: ReviewCustody,
    ) -> Result<WorkflowLaunchObservation, PrivateOriginalError> {
        self.launch_with_observation(snapshot, review_custody, None)
            .await
            .map_err(|error| match error {
                WorkflowChildPreparationError::ObservationChanged => {
                    PrivateOriginalError::Unavailable
                }
                WorkflowChildPreparationError::Refused(error) => error,
            })
    }

    /// The original dispatch supplies only the wait hint and remaining budget.
    /// A stale binding read never causes the launch effects to be replayed.
    pub(crate) async fn launch_from_dispatch_with_review(
        self: &Arc<Self>,
        snapshot: &SetupWorkflowSnapshot,
        review_custody: ReviewCustody,
        context: &meerkat_core::ToolDispatchContext,
        deadline: tokio::time::Instant,
    ) -> Result<WorkflowLaunchObservation, WorkflowChildPreparationError> {
        self.launch_with_observation(snapshot, review_custody, Some((context, deadline)))
            .await
    }

    async fn launch_with_observation(
        self: &Arc<Self>,
        snapshot: &SetupWorkflowSnapshot,
        review_custody: ReviewCustody,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<WorkflowLaunchObservation, WorkflowChildPreparationError> {
        review_custody
            .retain_io(async {
                let source = self
                    .source
                    .upgrade()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                self.check_for_launch(&source, dispatch).await?;
                if snapshot.terms() != &self.terms || snapshot.active_waiters() == 0 {
                    return Err(PrivateOriginalError::Denied.into());
                }
                let key = snapshot.reference().launch_effect();
                let effect = source
                    .store
                    .get_effect(&key)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                if effect.attempt > 0 || effect.status != EffectStatus::Pending {
                    return self.observe_launch(snapshot, dispatch).await;
                }
                let claimed = match source
                    .store
                    .claim_effect(
                        &key,
                        snapshot.revision(),
                        "installed-workflow-launch",
                        60_000,
                    )
                    .await
                {
                    Ok(claimed) => claimed,
                    Err(StoreError::Claimed | StoreError::Conflict | StoreError::EffectState) => {
                        return self.observe_launch(snapshot, dispatch).await;
                    }
                    Err(error) => return Err(store_error(error).into()),
                };
                let EffectClaimOutcome::Claimed(claimed) = claimed else {
                    return self.observe_launch(snapshot, dispatch).await;
                };
                let lease = claimed.lease.ok_or(PrivateOriginalError::Unavailable)?;
                let prepared = Arc::new(PreparedWorkflowLaunch {
                    selection: self.clone(),
                    reference: snapshot.reference().clone(),
                });
                let remaining = self
                    .terms
                    .deadline_ms
                    .checked_sub(
                        source
                            .owner
                            .clock()
                            .now()
                            .map_err(|_| PrivateOriginalError::Unavailable)?
                            .unix_ms,
                    )
                    .ok_or(PrivateOriginalError::Denied)?;
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(remaining),
                    prepared.submit_with_review(&source, review_custody.clone(), dispatch),
                )
                .await
                .unwrap_or(Err(PrivateOriginalError::Unavailable));
                // Meta retains its governed operation and binder/settlement after the
                // foreground receiver is dropped. Timeout is not proof of nonentry.
                let observation = result.unwrap_or(WorkflowLaunchObservation::Native {
                    observation: PrivateOriginalSubmission::NativeOutcomeUnavailable,
                });
                self.record_launch_observation(&source, &lease, &observation)
                    .await?;
                Ok(observation)
            })
            .await
    }

    async fn record_launch_observation(
        &self,
        source: &PrivateOriginals,
        lease: &EffectLease,
        observation: &WorkflowLaunchObservation,
    ) -> Result<(), PrivateOriginalError> {
        let receipt = RecordData {
            schema: "toolkit.setup-workflow-launch".into(),
            version: 1,
            body: serde_json::to_value(observation).map_err(|_| PrivateOriginalError::Conflict)?,
        };
        let outcome = match observation {
            WorkflowLaunchObservation::Native {
                observation:
                    PrivateOriginalSubmission::Admitted {
                        source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                        ..
                    },
            } => EffectOutcome::Succeeded { receipt },
            WorkflowLaunchObservation::NotAdmitted => EffectOutcome::Failed { evidence: receipt },
            _ => EffectOutcome::Uncertain { evidence: receipt },
        };
        source
            .store
            .acknowledge_effect(lease, outcome)
            .await
            .map_err(store_error)?;
        Ok(())
    }

    async fn observe_launch(
        &self,
        snapshot: &SetupWorkflowSnapshot,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<WorkflowLaunchObservation, WorkflowChildPreparationError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.check_for_launch(&source, dispatch).await?;
        let key = snapshot.reference().launch_effect();
        let effect = source
            .store
            .get_effect(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if effect.status == EffectStatus::Failed {
            // A definitive recorded decision for this exact launch is not
            // undone by absence of a native input row. It never permits replay.
            let Some(EffectOutcome::Failed { evidence }) = &effect.outcome else {
                return Err(PrivateOriginalError::Conflict.into());
            };
            if evidence.schema != "toolkit.setup-workflow-launch" || evidence.version != 1 {
                return Err(PrivateOriginalError::Conflict.into());
            }
            let observed: WorkflowLaunchObservation = serde_json::from_value(evidence.body.clone())
                .map_err(|_| PrivateOriginalError::Conflict)?;
            return match observed {
                WorkflowLaunchObservation::NotAdmitted => Ok(observed),
                _ => Err(PrivateOriginalError::Conflict.into()),
            };
        }
        // This existing owner observes exact native input state and promotes
        // the same pending guard. A saved receipt by itself cannot do that.
        let observed = source
            .reconcile_original_submission(&self.submission.original)
            .await?;
        let Some(observed) = observed else {
            // A completed cold launch has no in-process pending binder. Its
            // saved success selects this observation, but only the current
            // native input and restored source owners can establish admission.
            // Uncertain effects keep the existing unresolved path below.
            if effect.status == EffectStatus::Succeeded {
                let (planning, observed) = source
                    .restore_workflow_planning_observed(
                        snapshot.reference(),
                        &self.submission.original,
                        &self.terms.lead,
                    )
                    .await?;
                if !std::ptr::eq(self, planning.as_ref()) {
                    return Err(PrivateOriginalError::Conflict.into());
                }
                let held = source
                    .retained
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .iter()
                    .find(|held| held.reference == self.submission.original)
                    .cloned()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let workflow = held
                    .setup_workflow
                    .get()
                    .ok_or(PrivateOriginalError::Unavailable)?;
                if workflow.reference != *snapshot.reference() {
                    return Err(PrivateOriginalError::Conflict.into());
                }
                workflow.current_source()?;
                self.settle_launch_interest()?;
                return Ok(observed);
            }
            return Ok(WorkflowLaunchObservation::Native {
                observation: PrivateOriginalSubmission::AdmissionUnresolved,
            });
        };
        if !matches!(
            &observed,
            PrivateOriginalSubmission::Admitted {
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }
        ) {
            return Ok(WorkflowLaunchObservation::Native {
                observation: observed,
            });
        }
        let held = source
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.reference == self.submission.original)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let workflow = held
            .setup_workflow
            .get()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if workflow.reference != *snapshot.reference() {
            return Err(PrivateOriginalError::Conflict.into());
        }
        workflow.current_source()?;
        self.settle_launch_interest()?;
        if matches!(
            effect.status,
            EffectStatus::InFlight | EffectStatus::Uncertain
        ) || (effect.status == EffectStatus::Pending && effect.attempt > 0)
        {
            // The store atomically distinguishes a live claim from an expired
            // one, including InFlight with Some(expired lease). Never infer a
            // second dispatch from lease expiry or a lost acknowledgment.
            let claim = match source
                .store
                .claim_reconciliation(&key, "installed-workflow-observe", 60_000)
                .await
            {
                Ok(claim) => claim,
                Err(StoreError::Claimed | StoreError::EffectState) => {
                    return Ok(WorkflowLaunchObservation::Native {
                        observation: observed,
                    });
                }
                Err(error) => return Err(store_error(error).into()),
            };
            source
                .store
                .reconcile_effect(
                    &claim.lease.ok_or(PrivateOriginalError::Unavailable)?,
                    Reconciliation::Applied {
                        receipt: RecordData {
                            schema: "toolkit.setup-workflow-launch".into(),
                            version: 1,
                            body: serde_json::to_value(WorkflowLaunchObservation::Native {
                                observation: observed.clone(),
                            })
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                        },
                    },
                )
                .await
                .map_err(store_error)?;
        }
        Ok(WorkflowLaunchObservation::Native {
            observation: observed,
        })
    }
}

impl PreparedWorkflowLaunch {
    pub(in crate::private_originals) fn selection(&self) -> &InstalledWorkflowPlanning {
        &self.selection
    }

    pub(in crate::private_originals) fn stage(
        &self,
        held: &Arc<HeldPrivateOriginal>,
        context: &GovernedBindContext,
    ) -> Result<Arc<HeldSetupWorkflow>, PrivateOriginalError> {
        let source = self
            .selection
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.selection.check_for_binding(&source, &held.reference)?;
        let claims = held.original.association().candidate();
        if claims.target.context.as_str() != context.session_id.to_string()
            || claims.controller_model.as_ref() != Some(context.controller.selection())
            || claims.requester != self.selection.terms.requester
            || claims.ingress_namespace.realm != self.selection.terms.realm
        {
            return Err(PrivateOriginalError::Denied);
        }
        let route = context
            .controller
            .plain_facts()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .route_key();
        let candidate = Arc::new(HeldSetupWorkflow {
            source: Arc::downgrade(&source),
            planning: held.original.clone(),
            reference: self.reference.clone(),
            member: self.selection.terms.lead.clone(),
            session: context.session_id.clone(),
            controller_route: route.clone(),
            accepted: AtomicBool::new(false),
            deadline_ms: self.selection.terms.deadline_ms,
            waiters: Mutex::new(Vec::new()),
        });
        let publication = source.owner.publication();
        let _change = publication
            .begin_owner_change()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let prior = held.setup_workflow.get_or_init(|| candidate);
        if prior.reference != self.reference
            || prior.session != context.session_id
            || prior.member != self.selection.terms.lead
            || !Arc::ptr_eq(&prior.planning, &held.original)
            || prior.controller_route != route
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let prior = prior.clone();
        drop(_change);
        Ok(prior)
    }

    /// Stage precomputed effects only while the original binder owns an actual
    /// Candidate or admitted guard. No grant lookup or external callback runs.
    pub(in crate::private_originals) fn stage_effects(
        &self,
        source: &Arc<PrivateOriginals>,
        workflow: &Arc<HeldSetupWorkflow>,
    ) -> Result<(), PrivateOriginalError> {
        self.selection.stage_effects(source, workflow)
    }

    #[cfg(test)]
    async fn submit(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
    ) -> Result<WorkflowLaunchObservation, PrivateOriginalError> {
        self.submit_with_review(source, ReviewCustody::default(), None)
            .await
    }

    async fn submit_with_review(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        review_custody: ReviewCustody,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<WorkflowLaunchObservation, PrivateOriginalError> {
        review_custody
            .retain_io(async {
                self.selection
                    .check_binding_after_owner_change(
                        source,
                        &self.selection.submission.original,
                        dispatch,
                    )
                    .await
                    .map_err(binding_refusal)?;
                let route = source.route(&self.selection.submission.original.route)?;
                let binder = self
                    .binder_with_review(source, &route, review_custody.clone())
                    .await?;
                #[cfg(any(test, feature = "test-support"))]
                let diagnostics = binder.launch_diagnostics.clone();
                let result = self
                    .submit_with_binder(source, review_custody.bind(binder), dispatch)
                    .await;
                #[cfg(any(test, feature = "test-support"))]
                diagnostics.emit_after_submit();
                result
            })
            .await
    }

    #[cfg(test)]
    async fn binder(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        route: &PrivateOriginalRoute,
    ) -> Result<Arc<WorkflowBinder>, PrivateOriginalError> {
        self.binder_with_review(source, route, ReviewCustody::default())
            .await
    }

    async fn binder_with_review(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        route: &PrivateOriginalRoute,
        review_custody: ReviewCustody,
    ) -> Result<Arc<WorkflowBinder>, PrivateOriginalError> {
        let delegate = OriginalBinder {
            source: source.clone(),
            grant: None,
            batch: None,
            original: self.selection.submission.original.clone(),
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication: evidence(
                &route.authentication_domain,
                &self.selection.submission.transport_authentication,
            )?,
            event_key: source
                .original_event_key(&self.selection.submission.original)
                .await?,
            console: None,
            approved_login: None,
            review_custody,
        };
        Ok(Arc::new(WorkflowBinder {
            delegate,
            planning: self.clone(),
            #[cfg(any(test, feature = "test-support"))]
            launch_diagnostics: Arc::new(WorkflowLaunchDiagnostics::default()),
        }))
    }

    async fn submit_with_binder(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        binder: Arc<dyn GovernedInputBinder>,
        dispatch: Option<(&meerkat_core::ToolDispatchContext, tokio::time::Instant)>,
    ) -> Result<WorkflowLaunchObservation, PrivateOriginalError> {
        self.selection
            .check_binding_after_owner_change(source, &self.selection.submission.original, dispatch)
            .await
            .map_err(binding_refusal)?;
        let route = source.route(&self.selection.submission.original.route)?;
        let runtime = source.bound_identity_runtime()?;
        let alias = route.grant_target.as_str();
        let identity = runtime
            .owned_identity_for_member_alias(alias)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let input = Input::Prompt(PromptInput::new(
            self.selection.submission.content.clone(),
            None,
        ));
        let dedicated = source
            .owner
            .dedicated_workflow_session(
                &self.reference,
                &self.selection.terms.lead,
                &self.selection.submission.original.original,
            )
            .map_err(custody_error)?;
        let result = if dedicated.is_some() {
            runtime
                .dispatch_governed_with_turn_ticket(&identity, Some(alias), input, binder)
                .await
        } else {
            runtime
                .send_governed_with_turn_ticket(&identity, Some(alias), input, binder)
                .await
        };
        if let Err(error) = &result {
            PrivateOriginals::native_submission_error_diagnostic(
                "toolkit_workflow_launch_submission_error",
                error,
            );
        }
        Ok(match result {
            Ok(outcome) => WorkflowLaunchObservation::Native {
                observation: PrivateOriginalSubmission::Admitted {
                    session_id: outcome.admission.session_id.to_string(),
                    input_id: outcome.admission.input_id.to_string(),
                    deduplicated: outcome.admission.deduplicated,
                    turn_ticket: match outcome.ticketed.turn {
                        TurnTracking::Tracked(ticket) => Some(ticket.to_string()),
                        _ => None,
                    },
                    source_settlement: if outcome.admission.deduplicated {
                        PrivateSourceSettlement::RetryWithdrawn
                    } else if outcome.settlement.is_ok() {
                        PrivateSourceSettlement::FreshAcceptancePromoted
                    } else {
                        PrivateSourceSettlement::PostAdmissionPublicationFailed
                    },
                },
            },
            // Exact selected Meta returns Bind before creating native admission
            // custody or calling the bridge. Mob follows NotAdmitted custody,
            // and EntryRefused follows a fence that prevented submission.
            Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Bind(_)
                | GovernedInputError::Mob(_)
                | GovernedInputError::EntryRefused(_),
            )) => WorkflowLaunchObservation::NotAdmitted,
            Err(IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved {
                ..
            })) => WorkflowLaunchObservation::Native {
                observation: PrivateOriginalSubmission::AdmissionUnresolved,
            },
            Err(_) => WorkflowLaunchObservation::Native {
                observation: PrivateOriginalSubmission::NativeOutcomeUnavailable,
            },
        })
    }
}

struct WorkflowBinder {
    delegate: OriginalBinder,
    planning: Arc<PreparedWorkflowLaunch>,
    #[cfg(any(test, feature = "test-support"))]
    launch_diagnostics: Arc<WorkflowLaunchDiagnostics>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for WorkflowBinder {
    async fn bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, String> {
        let source = self
            .planning
            .selection
            .source
            .upgrade()
            .ok_or_else(|| "The workflow source owner is unavailable".to_owned())?;
        if let Some(session) = source
            .owner
            .dedicated_workflow_session(
                &self.planning.reference,
                &self.planning.selection.terms.lead,
                &self.planning.selection.submission.original.original,
            )
            .map_err(|_| "The dedicated workflow member is unavailable".to_owned())?
            && (session != context.session_id
                || context.spec.addressability
                    != meerkat_mobkit::identity_first::AgentAddressability::InternalOnly)
        {
            return Err("The dedicated workflow member changed".into());
        }
        let mut bound = self
            .delegate
            .bind_retained_with_workflow(context, input, Some(&self.planning))
            .await?;
        if bound.entry_fence.is_some() {
            return Err("The planning input has a conflicting entry owner".into());
        }
        let fence = LaunchFence {
            planning: self.planning.selection.clone(),
        };
        #[cfg(any(test, feature = "test-support"))]
        let fence = ObservedLaunchFence {
            inner: fence,
            diagnostics: self.launch_diagnostics.clone(),
        };
        bound.entry_fence = Some(Arc::new(fence));
        Ok(bound)
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        self.delegate.retain_unresolved(settlement, detail);
    }
}

/// Native invokes this after every bind/member await. SharedEntry's same
/// local lock orders each close against entry. Any one still-current waiter
/// is sufficient; waiters never become G contributing-work conjuncts.
struct LaunchFence {
    planning: Arc<InstalledWorkflowPlanning>,
}
impl LaunchFence {
    fn enter_observed(
        &self,
        #[cfg(any(test, feature = "test-support"))] diagnostics: Option<&WorkflowLaunchDiagnostics>,
    ) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        let source = self.planning.source.upgrade().ok_or_else(|| {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::SourceOwner);
            }
            GovernedEntryRefusal::Unavailable("The planning source owner is unavailable".into())
        })?;
        let mut interests = self.planning.interests.lock().map_err(|_| {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::InterestsLock);
            }
            GovernedEntryRefusal::Unavailable("The planning interests are unavailable".into())
        })?;
        let now = source
            .owner
            .clock()
            .now()
            .map_err(|_| {
                #[cfg(any(test, feature = "test-support"))]
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(WorkflowLaunchStage::Clock);
                }
                GovernedEntryRefusal::Unavailable("The planning clock is unavailable".into())
            })?
            .unix_ms;
        if now >= self.planning.terms.deadline_ms {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::Deadline);
            }
            return Err(GovernedEntryRefusal::Expired);
        }
        let mut unavailable = false;
        for interest in interests.iter_mut() {
            if interest.closed || now >= interest.membership.deadline_ms() {
                continue;
            }
            let Some(original) = &interest.original else {
                continue;
            };
            #[cfg(any(test, feature = "test-support"))]
            let entry = source
                .owner
                .enter_workflow_launch_observed(original, diagnostics);
            #[cfg(not(any(test, feature = "test-support")))]
            let entry = source.owner.enter_workflow_launch(original);
            match entry {
                Ok(entry) => {
                    interest.entries.push(entry.clone());
                    return Ok(Box::new(LaunchPermit { entry }));
                }
                Err(crate::original_ingress::OriginalWriterError::OperationCancelled) => continue,
                // A pending/failed cancellation is not eligible interest,
                // but it must not block another independently current waiter.
                // Only that waiter's actual successful entry can admit G.
                Err(_) => unavailable = true,
            }
        }
        // No source reservation was created and native has not admitted it.
        // Preserve uncertainty if none of the independent entries succeeded.
        if unavailable {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::WaiterEntry);
            }
            Err(GovernedEntryRefusal::Unavailable(
                "The waiter entry owner is unavailable".into(),
            ))
        } else {
            #[cfg(any(test, feature = "test-support"))]
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(WorkflowLaunchStage::NoCurrentWaiter);
            }
            Err(GovernedEntryRefusal::Retired)
        }
    }
}
impl GovernedEntryFence for LaunchFence {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.enter_observed(None)
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            self.enter_observed()
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
struct ObservedLaunchFence {
    inner: LaunchFence,
    diagnostics: Arc<WorkflowLaunchDiagnostics>,
}
#[cfg(any(test, feature = "test-support"))]
impl GovernedEntryFence for ObservedLaunchFence {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        self.inner.enter_observed(Some(&self.diagnostics))
    }
}
struct LaunchPermit {
    entry: Arc<crate::original_ingress::shared_entry::SharedEntry>,
}
impl GovernedEntryPermit for LaunchPermit {
    fn finish(self: Box<Self>, outcome: GovernedEntryOutcome) {
        // A lost/uncertain decision remains in both native input custody and
        // the actual source reservations. No Drop path fabricates settlement.
        if !matches!(outcome, GovernedEntryOutcome::Uncertain) {
            let _ = self.entry.settle();
        }
    }
}

impl HeldSetupWorkflow {
    pub(in crate::private_originals) fn accepted(&self) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let publication = source.owner.publication();
        let _change = publication
            .begin_owner_change()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let controller = source
            .owner
            .workflow_controller(&self.planning)
            .map_err(custody_error)?;
        if controller
            .plain_facts()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .route_key()
            != self.controller_route
        {
            return Err(PrivateOriginalError::Conflict);
        }
        self.accepted.store(true, Ordering::Release);
        Ok(())
    }
}

fn same_optional(
    a: Option<&Arc<InstalledWorkflowPlanning>>,
    b: Option<&Arc<InstalledWorkflowPlanning>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            Arc::ptr_eq(&a.parent, &b.parent)
                && Weak::ptr_eq(&a.source, &b.source)
                && a.terms == b.terms
                && a.lineage == b.lineage
                && match (&a.late_helper, &b.late_helper) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.same(b),
                    _ => false,
                }
                && a.submission.original == b.submission.original
                && a.submission.content == b.submission.content
                && a.submission.transport_authentication == b.submission.transport_authentication
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
