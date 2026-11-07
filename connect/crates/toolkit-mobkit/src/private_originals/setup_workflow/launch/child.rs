//! Pre-submit child ownership on the existing canonical source row.
use super::*;
use crate::installed_host::operator_setup::{
    OperatorSetup, SetupTargetRequest, SharedWorkflowSetupRequest,
};
use meerkat_toolkit::setup::SetupWorkBackend;
use meerkat_workgraph::{WorkItemRef, WorkOwnerKey};
use std::collections::BTreeSet;

/// Selectors and requested bounds from the trusted inherited application pipe.
/// No backend, native association, grant or live planning handle is accepted.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowChildLaunchRequest {
    pub parent: PrivateOriginalRef,
    pub planning: PrivateOriginalRef,
    pub submission: PrivateOriginalSubmit,
    pub child: WorkItemRef,
    pub child_owner: WorkOwnerKey,
    pub issuance_id: EvidenceId,
    pub targets: Vec<SetupTargetRequest>,
    pub output_audiences: BTreeSet<Id>,
}

/// A child preparation refusal, or a stale disposable native observation.
/// Neither outcome proves that earlier issuance or source writes had no effect.
/// ObservationChanged is not permission to retry the whole preparation command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowChildPreparationError {
    #[error(transparent)]
    Refused(#[from] PrivateOriginalError),
    #[error("workflow child authority observation changed")]
    ObservationChanged,
}
impl WorkflowChildPreparationError {
    pub(super) fn publication(error: meerkat_authorization::publication::PublicationError) -> Self {
        match error {
            meerkat_authorization::publication::PublicationError::Changed => {
                Self::ObservationChanged
            }
            meerkat_authorization::publication::PublicationError::Unavailable => {
                Self::Refused(PrivateOriginalError::Unavailable)
            }
        }
    }
}
fn preparation_failure(stage: &'static str, error: &WorkflowChildPreparationError) {
    match error {
        WorkflowChildPreparationError::Refused(error) => child_failure(stage, error),
        WorkflowChildPreparationError::ObservationChanged => {
            crate::aa1_failure(stage, "WorkflowChildPreparationError::ObservationChanged")
        }
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedWorkflowChild {
    parent: PrivateOriginalRef,
    planning: PrivateOriginalRef,
    workflow: SetupWorkflowRef,
    work: SetupWorkContext,
    output_audiences: BTreeSet<Id>,
    request_digest: ContentDigest,
}

/// Only a current lookup of the same source's retained live plan produces this.
pub struct InstalledWorkflowChild {
    pub planning: Arc<InstalledWorkflowPlanning>,
    pub work: SetupWorkContext,
    pub output_audiences: BTreeSet<Id>,
}

impl PrivateOriginals {
    pub(super) fn retained_child_plan(
        self: &Arc<Self>,
        parent: &PrivateOriginalRef,
        planning: &PrivateOriginalRef,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)
            .inspect_err(|error| child_failure("child.plan_parent_lock", error))?
            .iter()
            .find(|row| &row.reference == parent)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| child_failure("child.plan_parent", error))?;
        let plan = held
            .workflow_plans
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)
            .inspect_err(|error| child_failure("child.plan_lock", error))?
            .iter()
            .find(|plan| &plan.parent_reference == parent && &plan.submission.original == planning)
            .cloned()
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|error| child_failure("child.plan_selection", error))?;
        Ok(plan)
    }

    fn live_child_plan(
        self: &Arc<Self>,
        parent: &PrivateOriginalRef,
        planning: &PrivateOriginalRef,
    ) -> Result<Arc<InstalledWorkflowPlanning>, PrivateOriginalError> {
        let plan = self.retained_child_plan(parent, planning)?;
        plan.check_for_binding(self, planning)
            .inspect_err(|error| child_failure("child.plan_current", error))?;
        Ok(plan)
    }

    /// The actual host launcher selects a new child before any input bind.
    /// Existing source-writer custody orders configuration, correlation and
    /// first bind/cancellation. Native grants remain issued by OperatorSetup.
    pub async fn prepare_workflow_child_launch(
        self: &Arc<Self>,
        backend: &SetupWorkBackend,
        operator: &OperatorSetup,
        session: &meerkat_core::SessionId,
        request: &WorkflowChildLaunchRequest,
        allowed_outputs: &BTreeSet<Id>,
    ) -> Result<(), WorkflowChildPreparationError> {
        self.prepare_workflow_child_launch_selected(
            backend,
            operator,
            session,
            request,
            allowed_outputs,
            None,
        )
        .await
    }

    /// The Console binder already owns the actual target context. Reuse that
    /// controller/session rather than entering the identity owner recursively.
    pub async fn prepare_console_workflow_child_launch(
        self: &Arc<Self>,
        backend: &SetupWorkBackend,
        operator: &OperatorSetup,
        input: &ConsoleWorkflowChildInput,
        allowed_outputs: &BTreeSet<Id>,
    ) -> Result<(), WorkflowChildPreparationError> {
        if input
            .controller
            .source_owner()
            .is_none_or(|source| !Arc::ptr_eq(&source, self))
            || input.controller.original() != &input.request.submission.original
        {
            return Err(PrivateOriginalError::Denied.into());
        }
        input
            .entry
            .check()
            .map_err(|_| PrivateOriginalError::Denied)?;
        self.prepare_workflow_child_launch_selected(
            backend,
            operator,
            input.controller.session_id(),
            &input.request,
            allowed_outputs,
            Some(input),
        )
        .await
    }

    async fn prepare_workflow_child_launch_selected(
        self: &Arc<Self>,
        backend: &SetupWorkBackend,
        operator: &OperatorSetup,
        session: &meerkat_core::SessionId,
        request: &WorkflowChildLaunchRequest,
        allowed_outputs: &BTreeSet<Id>,
        console: Option<&ConsoleWorkflowChildInput>,
    ) -> Result<(), WorkflowChildPreparationError> {
        if request.output_audiences.is_empty()
            || !request.output_audiences.is_subset(allowed_outputs)
            || request.submission.original == request.parent
            || request.submission.original == request.planning
        {
            child_failure("child.bounds", &PrivateOriginalError::Denied);
            return Err(PrivateOriginalError::Denied.into());
        }
        let plan = self.retained_child_plan(&request.parent, &request.planning)?;
        plan.check_child_preparation(self, &request.planning)
            .inspect_err(|error| preparation_failure("child.plan_current", error))?;
        let helper = plan
            .late_helper
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)
            .inspect_err(|error| child_failure("child.helper_selection", error))?;
        if backend.pin() != &plan.terms.backend {
            child_failure("child.backend", &PrivateOriginalError::Denied);
            return Err(PrivateOriginalError::Denied.into());
        }
        let work = SetupWorkContext::from_owner(
            backend.pin().clone(),
            plan.terms.parent.clone(),
            request.child.clone(),
            plan.terms.parent_owner.clone(),
            request.child_owner.clone(),
            plan.terms.cycle_label.clone(),
        )
        .inspect_err(|error| crate::aa1_setup_failure("child.work_structure", error))
        .map_err(|_| PrivateOriginalError::Denied)?;
        work.check_current(backend)
            .await
            .inspect_err(|error| crate::aa1_setup_failure("child.work_current", error))
            .map_err(work_error)?;
        let reference = &request.submission.original;
        let selected = RetainedWorkflowChild {
            parent: request.parent.clone(),
            planning: request.planning.clone(),
            workflow: self
                .workflow_store()
                .inspect_err(|error| child_failure("child.workflow_store", error))?
                .workflow_ref(&plan.terms)
                .inspect_err(|error| crate::aa1_setup_failure("child.workflow_reference", error))
                .map_err(|_| PrivateOriginalError::Conflict)?,
            work,
            output_audiences: request.output_audiences.clone(),
            request_digest: ContentDigest::of_bytes(
                &canonical_json(request)
                    .map_err(|_| PrivateOriginalError::Conflict)
                    .inspect_err(|error| child_failure("child.request_encoding", error))?,
            ),
        };
        // An already-bound exact replay is an observation only. Check the
        // same live original's cancellation disposition under the source writer.
        {
            let _writer = self.owner.source_writer().await;
            let saved = self
                .store
                .get(
                    &self
                        .key(reference)
                        .inspect_err(|error| child_failure("child.bound_key", error))?,
                )
                .await
                .map_err(store_error)
                .inspect_err(|error| child_failure("child.bound_read", error))?
                .ok_or(PrivateOriginalError::Unavailable)
                .inspect_err(|error| child_failure("child.bound_missing", error))?;
            let record = self
                .decode_record(&saved, reference)
                .inspect_err(|error| child_failure("child.bound_decode", error))?;
            check_child_source(&record, request, console)?;
            if let Some(association) = &record.final_association {
                if record.workflow_child.as_ref() != Some(&selected) {
                    child_failure("child.bound_correlation", &PrivateOriginalError::Conflict);
                    return Err(PrivateOriginalError::Conflict.into());
                }
                check_child_record(&record, &selected)
                    .inspect_err(|error| child_failure("child.bound_record", error))?;
                self.check_live_child_binding(reference, association)
                    .inspect_err(|error| child_failure("child.bound_live_binding", error))?;
                plan.check_child_preparation(self, &request.planning)
                    .inspect_err(|error| preparation_failure("child.bound_plan", error))?;
                return Ok(());
            }
        }
        let route = self
            .route(&reference.route)
            .inspect_err(|error| child_failure("child.unbound_route", error))?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: reference.original.clone(),
        };
        // Reuse the existing first-bind/cancellation fence. No native effect is
        // entered when an unbound cancellation is already pending.
        self.owner
            .with_unbound_source_selection(&route.realm, &original, || async {
                let saved = self
                    .store
                    .get(&self.key(reference)?)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let record = self.decode_record(&saved, reference)?;
                check_child_source(&record, request, console)?;
                if record.operation_cancelled
                    || record.event.original_authentication
                        != request.submission.transport_authentication
                {
                    return Err(PrivateOriginalError::Denied.into());
                }
                if let Some(prior) = &record.workflow_child {
                    if prior != &selected {
                        return Err(PrivateOriginalError::Conflict.into());
                    }
                    check_child_record(&record, prior)?;
                    plan.check_child_preparation(self, &request.planning)?;
                    return Ok(());
                }
                // No effects on a previously configured/bound source, even if the
                // requester supplies new issuance IDs for an otherwise equal child.
                if record.final_association.is_some()
                    || record.configured_setup.is_some()
                    || record.planning_source_evidence.is_some()
                    || record.setup_request.is_some()
                    || record.approved_task.is_some()
                    || record.service_admission.is_some()
                    || !record.event.contributors.is_empty()
                {
                    return Err(PrivateOriginalError::Denied.into());
                }
                plan.check_child_preparation(self, &request.planning)?;
                if let Some(console) = console { console.entry.check().map_err(|_| PrivateOriginalError::Denied)?; }
                let (_, member, _) = self.selected_source_route(&reference.route)?;
                let configured = operator
                    .configure_late_workflow_setup(
                        self,
                        helper,
                        &SharedWorkflowSetupRequest {
                            member: member.member,
                            route: reference.route.clone(),
                            issuance_id: request.issuance_id.clone(),
                            helper: helper.admission.instance.member.member.clone(),
                            targets: request.targets.clone(),
                        },
                    )
                    .await
                    .map_err(|error| match error {
                        meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable
                        | meerkat_toolkit::host_lifetime::control::HostLifetimeError::NotSettled => PrivateOriginalError::Unavailable,
                        _ => PrivateOriginalError::Denied,
                    })?;
                plan.check_child_preparation(self, &request.planning)?;
                if let Some(console) = console { console.entry.check().map_err(|_| PrivateOriginalError::Denied)?; }
                self.retain_late_workflow_waiter_locked(
                    reference,
                    session,
                    configured.capability,
                    &selected.workflow,
                    &request.planning,
                    helper,
                    Some(selected.clone()),
                )
                .await?;
                // A successful write is not a native admission or submission receipt.
                // Revocation during its await must not be reported as current authority.
                plan.check_child_preparation(self, &request.planning)?;
                Ok(())
            })
            .await
            .map_err(|error| match error {
                OriginalWriterError::Conflict | OriginalWriterError::OperationCancelled => {
                    PrivateOriginalError::Denied
                }
                _ => PrivateOriginalError::Unavailable,
            })
            .inspect_err(|error| child_failure("child.unbound_entry", error))?
            .inspect_err(|error| preparation_failure("child.unbound_selection", error))
    }

    fn check_live_child_binding(
        &self,
        reference: &PrivateOriginalRef,
        association: &InputAuthorityAssociation,
    ) -> Result<(), PrivateOriginalError> {
        let original = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|row| &row.reference == reference && row.original.association() == association)
            .map(|row| row.original.clone())
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.owner
            .check_bound_source(&original)
            .map_err(|error| match error {
                OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                _ => PrivateOriginalError::Unavailable,
            })
    }

    /// Read the immutable launch correlation and the real live plan. A cold
    /// source without its original plan refuses instead of rebuilding custody.
    pub async fn workflow_child_invocation(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        association: &InputAuthorityAssociation,
        backend: &SetupWorkBackend,
    ) -> Result<Option<InstalledWorkflowChild>, PrivateOriginalError> {
        let saved = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&saved, reference)?;
        let Some(child) = record.workflow_child.clone() else {
            return Ok(None);
        };
        check_child_record(&record, &child)?;
        if record.final_association.as_ref() != Some(association) {
            return Err(PrivateOriginalError::Denied);
        }
        let plan = self.live_child_plan(&child.parent, &child.planning)?;
        if self
            .workflow_store()?
            .workflow_ref(&plan.terms)
            .map_err(|_| PrivateOriginalError::Conflict)?
            != child.workflow
        {
            return Err(PrivateOriginalError::Conflict);
        }
        child
            .work
            .check_current(backend)
            .await
            .map_err(work_error)?;
        // Graph I/O precedes the source writer. Recheck cancellation and exact
        // source bytes afterward; never hold a host lock across app callbacks.
        let _writer = self.owner.source_writer().await;
        let current = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let current = self.decode_record(&current, reference)?;
        if current.workflow_child.as_ref() != Some(&child)
            || current.final_association.as_ref() != Some(association)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        check_child_record(&current, &child)?;
        self.check_live_child_binding(reference, association)?;
        plan.check_for_binding(self, &child.planning)?;
        Ok(Some(InstalledWorkflowChild {
            planning: plan,
            work: child.work,
            output_audiences: child.output_audiences,
        }))
    }
}

fn check_child_source(
    record: &SourceRecord,
    request: &WorkflowChildLaunchRequest,
    console: Option<&ConsoleWorkflowChildInput>,
) -> Result<(), PrivateOriginalError> {
    match console {
        None if record.console_child_input.is_none()
            && record.event.content_sha256 == digest(request.submission.content.as_bytes()) =>
        {
            Ok(())
        }
        Some(console) if record.console_child_input.is_some() => {
            console
                .entry
                .validate_custody_input(record, &console.raw, &console.raw)?;
            if !matches!(&console.raw, Input::Prompt(prompt)
                if matches!(&prompt.content, meerkat_core::ContentInput::Text(text) if text == &request.submission.content))
            {
                return Err(PrivateOriginalError::Denied);
            }
            Ok(())
        }
        _ => Err(PrivateOriginalError::Denied),
    }
}

fn check_child_record(
    record: &SourceRecord,
    child: &RetainedWorkflowChild,
) -> Result<(), PrivateOriginalError> {
    if record.operation_cancelled || record.configured_setup.is_none()
        || record.planning_source_evidence.as_ref() != Some(&super::super::super::planning_source_evidence::PlanningSourceEvidence::EffectWaiter {
            workflow: child.workflow.clone(), planning: child.planning.clone(),
        })
    { return Err(PrivateOriginalError::Denied); }
    Ok(())
}

// Storage/owner-read failure does not establish an authority denial. Keep the
// same distinction as the installed workflow admission's work-read boundary.
pub(super) fn work_error(error: meerkat_toolkit::setup::SetupError) -> PrivateOriginalError {
    use meerkat_toolkit::setup::SetupError;
    match error {
        SetupError::Store(_)
        | SetupError::WorkGraph(_)
        | SetupError::NativeExecutionUnavailable => PrivateOriginalError::Unavailable,
        _ => PrivateOriginalError::Denied,
    }
}
