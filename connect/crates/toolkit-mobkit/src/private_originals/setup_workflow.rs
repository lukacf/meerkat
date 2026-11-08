//! Bind dormant workflow records to the existing live source/controller owner.
//!
//! These methods are internal entry points for the next workflow launch and
//! plan-resolution adapters. They do not spawn G, dispatch an input, acknowledge
//! an outbox effect, or substitute stored membership for native authorization.
use super::*;
use crate::original_ingress::workflow_custody::{WorkflowCallCustody, WorkflowWaiterCustody};
use meerkat_authorization_contracts::work_association::{GrantTarget, InputAuthorityAssociation};
use meerkat_core::{
    ControllerModelClient, ControllerRouteKey, MobMemberBinding, ToolCallView,
    authorization::PreparedAuthorizationBinding,
};
use meerkat_toolkit::setup::{
    SetupAccess, SetupScope, SetupStore, SetupWorkflowRef, SetupWorkflowSnapshot,
    SetupWorkflowWaiter,
};
use std::sync::atomic::{AtomicBool, Ordering};
use toolkit_package::ContentDigest;

pub(crate) mod execution;
mod member;
pub use member::{InstalledWorkflowMember, PreparedWorkflowMember};
mod launch;
pub(crate) use launch::InvocationPlanning;
#[cfg(test)]
pub(crate) use launch::OwnedTransferCommit;
pub(in crate::private_originals) use launch::PreparedConsoleChild;
pub(in crate::private_originals) use launch::PreparedWorkflowLaunch;
pub(crate) use launch::RetainedWorkflowChild;
pub(crate) use launch::WorkflowInterestRecovery;
pub(crate) use launch::WorkflowLaunchObservation;
pub(crate) use launch::WorkflowTerminalRecovery;
pub(crate) use launch::WorkflowTransferObservation;
pub use launch::{
    ConsoleWorkflowChildInput, ConsoleWorkflowChildPreparer, ConsoleWorkflowChildRequest,
};
pub use launch::{
    InstalledLateWorkflowHelper, InstalledLateWorkflowWaiter, InstalledWorkflowContext,
    InstalledWorkflowContextTransfer, InstalledWorkflowPlanning, WorkflowContextObservation,
    WorkflowDeliveryObservation,
};
pub use launch::{
    InstalledWorkflowChild, WorkflowChildLaunchRequest, WorkflowChildPreparationError,
};

/// Process-local attachment under the actual G source, never reconstructed
/// from workflow terms. Runnable custody stays on the canonical source owner;
/// this attachment retains only the route that admitted custody must match.
pub(crate) struct HeldSetupWorkflow {
    source: Weak<PrivateOriginals>,
    planning: Arc<RetainedToolkitOriginal>,
    reference: SetupWorkflowRef,
    member: MobMemberBinding,
    session: meerkat_core::SessionId,
    controller_route: ControllerRouteKey,
    accepted: AtomicBool,
    deadline_ms: u64,
    waiters: Mutex<Vec<Arc<HeldWorkflowWaiter>>>,
}

/// Canonical inline request bytes remain owned here until the later governed
/// disclosure binder can carry them through its actual final-admission fence.
/// There is deliberately no public payload accessor or delivery receipt.
pub(crate) struct HeldWorkflowWaiter {
    workflow: Weak<HeldSetupWorkflow>,
    membership: SetupWorkflowWaiter,
    original: PrivateOriginalRef,
    request: String,
    request_digest: ContentDigest,
    authority: Arc<WorkflowWaiterCustody>,
}

impl PrivateOriginals {
    pub(in crate::private_originals) fn workflow_store(
        &self,
    ) -> Result<SetupStore<'_>, PrivateOriginalError> {
        SetupStore::new(
            self.store.as_ref(),
            SetupScope::new(
                Id::new(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
                Id::new(self.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        )
        .map_err(|_| PrivateOriginalError::Unavailable)
    }

    /// Synchronous native policy projection only. Durable reads/relationship
    /// selection finished before native preparation. No policy answer is cached.
    pub(crate) fn workflow_call_for_policy(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Option<(
            Arc<WorkflowCallCustody>,
            crate::original_ingress::helper_originals::HelperSetupOrigin,
        )>,
        PrivateOriginalError,
    > {
        self.workflow_call_observed(association, binding, true)
    }

    /// Review only reads a binding retained by actual native preparation.
    pub(crate) fn workflow_call_for_review(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Option<(
            Arc<WorkflowCallCustody>,
            crate::original_ingress::helper_originals::HelperSetupOrigin,
        )>,
        PrivateOriginalError,
    > {
        self.workflow_call_observed(association, binding, false)
    }

    fn workflow_call_observed(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        retain_binding: bool,
    ) -> Result<
        Option<(
            Arc<WorkflowCallCustody>,
            crate::original_ingress::helper_originals::HelperSetupOrigin,
        )>,
        PrivateOriginalError,
    > {
        self.owner
            .check_workflow_policy_capture(association)
            .map_err(custody_error)?;
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.original.association() == association)
            .cloned();
        let Some(workflow) = held.as_ref().and_then(|held| held.setup_workflow.get()) else {
            return Ok(None);
        };
        // Native may execute G's own bound input before accepted settlement.
        // Exact staged waiter custody still prevents a singleton fallback.
        workflow.current_operation_source()?;
        let waiters = workflow
            .waiters
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let publication = self.owner.publication();
        let _poison = publication.poison_on_unwind();
        let mut found = None;
        for waiter in waiters.iter() {
            if let Some(call) = (if retain_binding {
                waiter.authority.prepared_call(association, binding)
            } else {
                waiter.authority.review_prepared_call(association, binding)
            })
            .map_err(custody_error)?
            {
                if found.is_some() {
                    return Err(PrivateOriginalError::Conflict);
                }
                found = Some((call, self.setup_origin_for(&waiter.original)?));
            }
        }
        // Once G owns a workflow, an unstaged MCP call cannot silently take
        // the ordinary singleton path. Controller/catalog planning is unchanged.
        if found.is_none()
            && matches!(
                meerkat_authorization::grant_policy::operation_grant_target(binding)
                    .map_err(|_| PrivateOriginalError::Denied)?,
                Some(GrantTarget::McpServer { .. })
            ) || found.is_none() && meerkat_authorization::grant_policy::operation_grant_target(binding)
                .map_err(|_| PrivateOriginalError::Denied)? == Some(crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target())
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(found)
    }
}

impl HeldSetupWorkflow {
    fn current_source(&self) -> Result<Arc<PrivateOriginals>, PrivateOriginalError> {
        if !self.accepted.load(Ordering::Acquire) {
            return Err(PrivateOriginalError::Unavailable);
        }
        let source = self.current_operation_source()?;
        source
            .owner
            .workflow_controller(&self.planning)
            .map_err(custody_error)?;
        Ok(source)
    }

    /// Only G's actual own tool operation uses this path. Named-source reads,
    /// context transfer and retained recovery keep current_source's promotion.
    fn current_operation_source(&self) -> Result<Arc<PrivateOriginals>, PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if source
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .unix_ms
            >= self.deadline_ms
        {
            return Err(PrivateOriginalError::Denied);
        }
        let controller = source
            .owner
            .workflow_own_controller(&self.planning)
            .map_err(custody_error)?;
        if controller
            .plain_facts()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .route_key()
            != self.controller_route
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(source)
    }
}

impl HeldWorkflowWaiter {
    /// Next caller: the workflow-aware native plan resolver, before Core
    /// creates/prepares its real binding. Selection IDs are only lookup keys.
    pub(crate) fn stage_call(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<Arc<WorkflowCallCustody>, PrivateOriginalError> {
        let workflow = self
            .workflow
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)
            .inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.workflow_owner","kind":"unavailable"}}"#))?;
        workflow.current_operation_source().inspect_err(|_| eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"workflow.stage.current_source"}}"#))?;
        self.authority.stage_call(call).map_err(custody_error)
    }
}

fn require_active(
    workflow: &SetupWorkflowSnapshot,
    waiter: &SetupWorkflowWaiter,
) -> Result<(), OriginalWriterError> {
    // Sealing closes admission of new members. Exact active members keep
    // their independently authorized continuation and recovery path.
    if workflow
        .departed(waiter)
        .map_err(|_| OriginalWriterError::Conflict)?
    {
        return Err(OriginalWriterError::OperationDenied);
    }
    Ok(())
}
pub(super) fn custody_error(error: OriginalWriterError) -> PrivateOriginalError {
    match error {
        OriginalWriterError::Unavailable => PrivateOriginalError::Unavailable,
        OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
        _ => PrivateOriginalError::Denied,
    }
}
