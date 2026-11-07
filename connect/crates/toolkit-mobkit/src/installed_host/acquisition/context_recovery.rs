//! Explicit host read selection for historical workflow metadata only.
use super::*;
use meerkat_toolkit::setup::{SetupWorkflowRef, SetupWorkflowTerms};
use meerkat_workgraph::WorkGraphService;
use std::collections::BTreeSet;

/// Trusted host choice, never a model-facing reader or a grant inferred from
/// namespace text. The worker checks this actual service against its selected
/// backend and exact canonical workflow before reading any payload.
pub struct InstalledWorkflowContextRead {
    pub recipient: WorkGraphService,
    pub output_audiences: BTreeSet<Id>,
}
impl InstalledWorkflowContextRead {
    /// Explicitly authorize this workflow's private metadata read using the
    /// host's actual backend. Its ordinary namespace grant does not imply this
    /// choice. The returned service stays inside the recovery owner.
    pub fn for_workflow(
        backend: &SetupWorkBackend,
        workflow: &SetupWorkflowRef,
        output_audiences: BTreeSet<Id>,
    ) -> Result<Self, HostLifetimeError> {
        if output_audiences.is_empty() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let namespace = PrivateOriginals::workflow_context_namespace(workflow)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(Self {
            recipient: WorkGraphService::with_scope(
                backend.service().store().clone(),
                backend.service().default_realm_id(),
                namespace,
            ),
            output_audiences,
        })
    }
}

/// Synchronous and request-free. None deliberately declines this workflow;
/// Unavailable retains uncertainty. The host may select workflows created after
/// bootstrap, but the selector itself cannot be replaced after configuration.
pub type InstalledWorkflowContextSelector = Arc<
    dyn Fn(
            &SetupWorkflowRef,
            &SetupWorkflowTerms,
            &SetupWorkBackend,
        ) -> Result<Option<InstalledWorkflowContextRead>, HostLifetimeError>
        + Send
        + Sync,
>;

/// The ordinary installed application's fixed host selection. These owned
/// parameters come from its compiled host and acquisition configuration, not
/// init JSON or a request. Workflow terms can arrive after bootstrap.
pub fn installed_workflow_context_selector(
    realm: meerkat_core::RealmId,
    generation: Id,
    members: Vec<MobMemberBinding>,
    output_audiences: BTreeSet<Id>,
) -> InstalledWorkflowContextSelector {
    Arc::new(move |workflow, terms, backend| {
        if terms.realm != realm
            || terms.backend != *backend.pin()
            || backend.pin().backend_generation() != &generation
            || members
                .iter()
                .filter(|member| **member == terms.lead)
                .count()
                != 1
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(Some(InstalledWorkflowContextRead::for_workflow(
            backend,
            workflow,
            output_audiences.clone(),
        )?))
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkflowContextRecovery {
    NotSelected,
    Observed { admitted: usize, pending: usize },
    Stopped,
}

impl InstalledAcquisition {
    pub(super) async fn recover_context_source(
        &self,
        observed: &toolkit_store::StoredRecord,
        control: &recovery::RecoveryControl,
    ) -> Result<WorkflowContextRecovery, PrivateOriginalError> {
        let Some(selector) = self.context_recovery.get() else {
            return Ok(WorkflowContextRecovery::NotSelected);
        };
        // This is the actual selected acquisition owner, not a portable backend
        // or an independently supplied recovery control from another host.
        let published = self
            .transfer_recovery
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !std::ptr::eq(published.as_ref(), control) {
            return Err(PrivateOriginalError::Denied);
        }
        self.host
            .originals
            .recover_workflow_context_source(observed, &self.work_backend, selector, || {
                control.enter_transfer(|| ()).is_some()
            })
            .await
    }
}
