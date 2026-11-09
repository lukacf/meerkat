//! One existing source-row transition for a new host-selected waiter.
use super::*;
use crate::private_originals::{
    InstalledLateWorkflowHelper, planning_source_evidence::PlanningSourceEvidence,
};
use meerkat_toolkit::setup::SetupWorkflowRef;

impl PrivateOriginals {
    pub(in crate::private_originals) async fn retain_late_workflow_waiter(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        capability: CompiledAgentSetupCapability,
        workflow: &SetupWorkflowRef,
        planning: &PrivateOriginalRef,
        helper: &InstalledLateWorkflowHelper,
    ) -> Result<(), PrivateOriginalError> {
        let route = self.route(&reference.route)?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: reference.original.clone(),
        };
        self.owner
            .with_unbound_source_selection(&route.realm, &original, || async {
                self.retain_late_workflow_waiter_locked(
                    reference, session, capability, workflow, planning, helper, None,
                )
                .await
            })
            .await
            .map_err(|error| match error {
                OriginalWriterError::Conflict | OriginalWriterError::OperationCancelled => {
                    PrivateOriginalError::Denied
                }
                _ => PrivateOriginalError::Unavailable,
            })?
    }

    /// Caller retains the existing source writer through configuration and CAS.
    pub(in crate::private_originals) async fn retain_late_workflow_waiter_locked(
        self: &Arc<Self>,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        capability: CompiledAgentSetupCapability,
        workflow: &SetupWorkflowRef,
        planning: &PrivateOriginalRef,
        helper: &InstalledLateWorkflowHelper,
        child: Option<crate::private_originals::setup_workflow::RetainedWorkflowChild>,
    ) -> Result<(), PrivateOriginalError> {
        helper
            .check(self)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let binding = helper
            .admission
            .workflow
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        if &binding.workflow != workflow
            || &binding.original != planning
            || capability.route != reference.route
            || !capability.helper_targets.is_empty()
            || capability.helper.as_ref() != Some(&helper.admission.instance.member.member)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        crate::installed_host::operator_setup::setup_targets::validate_workflow_compiled(
            &capability.workflow_targets,
        )
        .map_err(|_| PrivateOriginalError::Denied)?;
        let facts = self.configured_setup(reference)?;
        if !self.configured_parent_current(&facts, &capability.parent)? {
            return Err(PrivateOriginalError::Denied);
        }
        for target in &capability.workflow_targets {
            let current = self
                .owner
                .grants()
                .resolve_lineage(
                    std::slice::from_ref(&target.parent),
                    &facts.application,
                    facts.route.represented_subject.as_ref(),
                )
                .map_err(|_| PrivateOriginalError::Denied)?;
            if current.restrictions() != &target.restrictions {
                return Err(PrivateOriginalError::Conflict);
            }
            current
                .check_current()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
        }
        let selected = ConfiguredSetupSelection {
            capability,
            session_id: session.clone(),
        };
        let evidence = PlanningSourceEvidence::EffectWaiter {
            workflow: workflow.clone(),
            planning: planning.clone(),
        };
        let key = self.key(reference)?;
        let saved = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut record = self.decode_record(&saved, reference)?;
        self.check_planning_evidence_source(&record, reference)?;
        if record.operation_cancelled
            || record.final_association.is_some()
            || record.setup_request.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
            || !record.event.contributors.is_empty()
        {
            return Err(PrivateOriginalError::Denied);
        }
        if record.configured_setup.as_ref() == Some(&selected)
            && record.planning_source_evidence.as_ref() == Some(&evidence)
            && record.workflow_child == child
        {
            helper
                .check(self)
                .map_err(|_| PrivateOriginalError::Denied)?;
            return Ok(());
        }
        if record.configured_setup.is_some()
            || record.planning_source_evidence.is_some()
            || record.workflow_child.is_some()
            || self
                .owner
                .grants()
                .issued_reference(&self.configured_child_id(reference)?)
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .is_some()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        helper
            .check(self)
            .map_err(|_| PrivateOriginalError::Denied)?;
        record.configured_setup = Some(selected);
        record.planning_source_evidence = Some(evidence);
        record.workflow_child = child;
        super::super::planning_source_evidence::validate(&record)?;
        self.store
            .compare_and_swap(Transition {
                key,
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest),
                claim: None,
                data: encode(record)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(store_error)?;
        Ok(())
    }
}
