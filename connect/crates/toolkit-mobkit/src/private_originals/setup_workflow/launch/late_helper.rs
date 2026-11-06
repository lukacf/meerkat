//! Exact late-G selection retained by the existing planning custody.
//! Native grant references and the canonical member remain their own owners.
use super::*;
use crate::installed_host::dynamic_members::{AdmittedMemberClass, MemberClassAdmission};
use meerkat_authorization_contracts::{constraints::ExecutionRestrictions, grant::GrantLineageRef};
use meerkat_core::McpServerConfig;
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

/// An operator-selected, actually constructed workflow helper. There is no
/// wire constructor: equal serialized membership is not this owner receipt.
#[derive(Clone)]
pub struct InstalledLateWorkflowHelper {
    pub(crate) source: Weak<PrivateOriginals>,
    pub(crate) admitted: Arc<AdmittedMemberClass>,
    pub(crate) admission: MemberClassAdmission,
    // Current physical selection may advance without changing historical plan
    // provenance. Every other canonical admission field must remain exact.
    pub(crate) current_resource:
        Option<meerkat_toolkit::host_lifetime::control::SelectedHostArtifact>,
    pub(crate) server: McpServerConfig,
    pub(crate) envelope: ExecutionRestrictions,
    pub(crate) lineage: Vec<GrantLineageRef>,
    pub(crate) mcp_lineage: Option<Vec<GrantLineageRef>>,
    pub(crate) public_lineage: Option<Vec<GrantLineageRef>>,
}
impl InstalledLateWorkflowHelper {
    pub(crate) fn operation_basis(
        &self,
    ) -> meerkat_authorization_contracts::work_association::WorkAuthorityBasis {
        match &self.mcp_lineage {
            Some(target) => crate::installed_host::operator_setup::workflow_mcp::basis_with_public(
                self.lineage.clone(), target.clone(), self.public_lineage.clone()),
            None => meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage { lineage: self.lineage.clone() },
        }
    }

    pub(in crate::private_originals) fn provenance(
        &self,
    ) -> crate::private_originals::planning_source_evidence::LateWorkflowHelperProvenance {
        crate::private_originals::planning_source_evidence::LateWorkflowHelperProvenance {
            admission: self.admission.clone(),
            server: self.server.clone(),
        }
    }
    pub(crate) fn check(&self, source: &Arc<PrivateOriginals>) -> Result<(), HostLifetimeError> {
        self.check_source(source)
    }
    pub(in crate::private_originals) fn check_source(
        &self,
        source: &PrivateOriginals,
    ) -> Result<(), HostLifetimeError> {
        let held = self.check_selection(source)?;
        let (current, _) = held.completed_workflow_class(&self.admitted)?;
        let mut expected = self.admission.clone();
        expected.workflow_resource = self.current_resource.clone();
        if current != expected {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
    // Selection alone is used before completing the actual restored controller.
    // It cannot replace the completed member/source check at any operation use.
    pub(super) fn check_selection(
        &self,
        source: &PrivateOriginals,
    ) -> Result<Arc<PrivateOriginals>, HostLifetimeError> {
        let held = self
            .source
            .upgrade()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !std::ptr::eq(held.as_ref(), source) {
            return Err(HostLifetimeError::WrongOwner);
        }
        held.check_member_class_owner(&self.admitted)?;
        let binding = self.admitted.workflow().ok_or(HostLifetimeError::Denied)?;
        let (current, _) = self
            .admitted
            .owner
            .member_route(&binding.original.route)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let mut expected = self.admission.clone();
        expected.workflow_resource = self.current_resource.clone();
        if current != expected {
            return Err(HostLifetimeError::Denied);
        }
        Ok(held)
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.source, &other.source)
            && self.admission == other.admission
            && self.server == other.server
            && self.envelope == other.envelope
            && self.lineage == other.lineage
            && self.mcp_lineage == other.mcp_lineage
            && self.public_lineage == other.public_lineage
            && self.current_resource == other.current_resource
    }
    pub(crate) fn digest(&self) -> Result<ContentDigest, PrivateOriginalError> {
        let bytes = if self.mcp_lineage.is_some() {
            canonical_json(&(
                &self.admission,
                &self.server,
                &self.envelope,
                &self.operation_basis(),
            ))
        } else {
            canonical_json(&(&self.admission, &self.server, &self.envelope, &self.lineage))
        };
        bytes
            .map(|bytes| ContentDigest::of_bytes(&bytes))
            .map_err(|_| PrivateOriginalError::Conflict)
    }
}
impl PrivateOriginals {
    /// Serialize native issuance with the actual G's first binding and source
    /// cancellation. A saved source can only replay its existing full basis.
    pub(crate) async fn with_workflow_helper_grants<T, F, Fut>(
        &self,
        admission: &MemberClassAdmission,
        server: &McpServerConfig,
        action: F,
    ) -> Result<T, HostLifetimeError>
    where
        F: FnOnce(
            Option<meerkat_authorization_contracts::work_association::WorkAuthorityBasis>,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<T, HostLifetimeError>>,
    {
        let _writer = self.owner.source_writer().await;
        let reference = &admission
            .workflow
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?
            .original;
        let row = self
            .store
            .get(&self.key(reference).map_err(|_| HostLifetimeError::Denied)?)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let record = self
            .decode_record(&row, reference)
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
                _ => HostLifetimeError::Denied,
            })?;
        if record.operation_cancelled {
            return Err(HostLifetimeError::Denied);
        }
        if record.final_association.is_some()
            && record
                .late_workflow_helper
                .as_ref()
                .is_none_or(|saved| saved.admission != *admission || saved.server != *server)
        {
            return Err(HostLifetimeError::Denied);
        }
        action(
            record
                .final_association
                .map(|saved| saved.candidate().authority_basis.clone()),
        )
        .await
    }

    /// Current policy revision used by the existing installed acquisition.
    /// Reading it neither reserves work nor authenticates a workflow caller.
    pub fn workflow_authority_revision(
        &self,
    ) -> Result<toolkit_package::Revision, HostLifetimeError> {
        let lifetime = self
            .status_lifetime()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let policy = lifetime.current_setup_policy()?;
        let bytes = canonical_json(&policy).map_err(|_| HostLifetimeError::Unavailable)?;
        toolkit_package::Revision::new(ContentDigest::of_bytes(&bytes).to_string())
            .map_err(|_| HostLifetimeError::Unavailable)
    }
    pub(crate) fn workflow_helper_owner(
        &self,
        owner: &Arc<ToolkitOriginalOwner>,
        store: &Arc<dyn ToolkitStore>,
        lifetime: &Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
    ) -> Result<(), HostLifetimeError> {
        if !Arc::ptr_eq(&self.owner, owner)
            || !Arc::ptr_eq(&self.store, store)
            || !self
                .host_lifetime
                .get()
                .and_then(Weak::upgrade)
                .is_some_and(|bound| Arc::ptr_eq(&bound, lifetime))
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        lifetime.validate_current_selection()
    }
}

/// An exact separately configured caller selection. Only OperatorSetup creates
/// it; retaining it does not bind an input or replace native permission checks.
pub struct InstalledLateWorkflowWaiter {
    pub(crate) helper: InstalledLateWorkflowHelper,
    pub(crate) capability: meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupCapability,
}
impl InstalledWorkflowPlanning {
    /// Host group preparation before this new original's first submit. An
    /// already-bound singleton original cannot be converted to a shared waiter.
    pub async fn prepare_late_waiter_source(
        &self,
        original: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        selection: &InstalledLateWorkflowWaiter,
    ) -> Result<(), PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.check_for_binding(&source, &self.submission.original)?;
        let helper = self
            .late_helper
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        if !helper.same(&selection.helper) {
            return Err(PrivateOriginalError::Conflict);
        }
        source
            .retain_late_workflow_waiter(
                original,
                session,
                selection.capability.clone(),
                &source
                    .workflow_store()?
                    .workflow_ref(&self.terms)
                    .map_err(|_| PrivateOriginalError::Conflict)?,
                &self.submission.original,
                helper,
            )
            .await
    }
    #[cfg(test)]
    pub(crate) fn late_helper_digest(
        &self,
        source: &Arc<PrivateOriginals>,
        requested: Option<&meerkat_core::MobMemberBinding>,
    ) -> Result<Option<ContentDigest>, PrivateOriginalError> {
        self.late_helper_digest_observed(source, requested)
            .map_err(|error| match error {
                WorkflowChildPreparationError::ObservationChanged => {
                    PrivateOriginalError::Unavailable
                }
                WorkflowChildPreparationError::Refused(error) => error,
            })
    }
    pub(crate) fn late_helper_digest_observed(
        &self,
        source: &Arc<PrivateOriginals>,
        requested: Option<&meerkat_core::MobMemberBinding>,
    ) -> Result<Option<ContentDigest>, WorkflowChildPreparationError> {
        let Some(helper) = &self.late_helper else {
            return Ok(None);
        };
        self.check_child_preparation(source, &self.submission.original)?;
        if requested != Some(&helper.admission.instance.member.member) {
            return Err(PrivateOriginalError::Denied.into());
        }
        helper
            .digest()
            .map(Some)
            .map_err(WorkflowChildPreparationError::Refused)
    }
}
