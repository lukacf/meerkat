//! Local operator review within the existing application's Rust composition.
//!
//! The caller retains the actual gateway and its native client after leaving
//! the identity binding lease. The existing Connect terminal loop owns the
//! ApprovalPresenter and input. No remote command, principal, model facts or
//! serialized approval can select this operator entry.

use crate::{ConnectError, approval::LocalApprovals};
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk,
};
use std::time::{Duration, SystemTime};
use toolkit_mobkit::{
    host_gateway::ToolkitGatewayHost,
    installed_host::operator_setup::OperatorParentOutcome,
    private_originals::LocalControllerSetup,
};

/// Review the actual application controller through the existing local
/// terminal presenter and same native grant/source owner. Call only from the
/// trusted local setup composition, after its terminal check, with no native
/// identity lease held. The caller's ordinary presenter loop must continue
/// consuming LocalApprovals prompts while this future waits. Its once-bound
/// session must be the actual prepared native SessionId; no second setup
/// runtime is created to obtain an approval envelope.
/// Obtain `prepared` from this host's `prepare_local_controller_setup` first;
/// that native preparation has already returned without submitting the input.
///
/// Selected is an exact reference for the existing application compiler. It
/// does not publish a policy, admit an input, or make the current host adopt a
/// new selection. Keep the returned reference if compiler publication fails;
/// publish and close/reopen through the existing application owner.
pub async fn review_local_controller_parent(
    host: &ToolkitGatewayHost,
    prepared: LocalControllerSetup,
    approvals: &LocalApprovals,
) -> Result<OperatorParentOutcome, ConnectError> {
    // Check the actual source-owner Arc before extracting any route/client or
    // asking the human. Equal route names across installations are not a join.
    let owner = host.local_operator_setup_for(&prepared)
        .map_err(|error| ConnectError::native("retain local application operator owner", error))?;
    review_controller_parent(owner, prepared, approvals, None).await
}

/// Protected application presenter over the same gateway. The selected native
/// root comes from its trusted bootstrap; login_scope is attribution only.
pub(crate) async fn review_application_controller_parent(
    host: &ToolkitGatewayHost,
    prepared: LocalControllerSetup,
    approvals: &LocalApprovals,
    login_scope: &str,
) -> Result<OperatorParentOutcome, ConnectError> {
    let owner = host.application_operator_setup_for(&prepared)
        .map_err(|error| ConnectError::native("retain application operator owner", error))?;
    review_controller_parent(owner, prepared, approvals, Some(login_scope)).await
}

async fn review_controller_parent(
    owner: toolkit_mobkit::installed_host::operator_setup::OperatorSetup,
    prepared: LocalControllerSetup,
    approvals: &LocalApprovals,
    login_scope: Option<&str>,
) -> Result<OperatorParentOutcome, ConnectError> {
    let (original, native_session, controller) = prepared.into_parts();
    let route = original.route;
    let facts = controller.plain_facts()
        .map_err(|error| ConnectError::native("observe retained application controller", error))?;
    if let Some(selected) = owner.selected_controller_parent(&route, &facts)
        .map_err(|error| ConnectError::native("check selected application controller access", error))?
    {
        return Ok(selected);
    }
    let expires = SystemTime::now().checked_add(Duration::from_secs(120))
        .ok_or_else(|| ConnectError::InvalidInput("The operator review deadline is invalid".into()))?;
    let request = ApprovalRequest {
        // Attribution only. The already selected operator owner supplies
        // the native caller, never this label or the approval actor.
        requester: ApprovalPrincipalId::new("connect-setup-agent")
            .map_err(|error| ConnectError::native("identify setup review requester", error))?,
        owner: ApprovalOwnerRef::Session { session_id: native_session },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("application-controller"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review this application's selected controller access".into(),
            body: None,
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
        expires_at: Some(expires.into()),
        metadata: meerkat_core::SurfaceMetadata {
            app_context: login_scope.map(|scope| serde_json::json!({"protected_login_scope":scope})),
            ..Default::default()
        },
        request_provenance: None,
    };
    let proposal = owner.prepare_controller_parent_review(route, &facts, request)
        .map_err(|error| ConnectError::native("retain exact application controller review", error))?;
    let approval = approvals.review_access(proposal.request().clone()).await?;
    // Never substitute another model client after the human has reviewed.
    // There is no credential refresh, provider call or new resolver here.
    if approval.status == meerkat_core::ApprovalStatus::Approved {
        let current = controller.plain_facts()
            .map_err(|error| ConnectError::native("recheck retained application controller", error))?;
        if current.route_key() != facts.route_key() {
            return Err(ConnectError::InvalidInput(
                "The application controller changed after its review; no grant was issued".into(),
            ));
        }
    }
    owner.apply_parent_decision(proposal, approval).await
        .map_err(|error| ConnectError::native("record reviewed application controller selection", error))
}

/// A separate protected review of setup requests for one pending original.
/// It neither grants controller access nor approves installing or using a service.
/// The same source owner retains the approved selection before the first bind;
/// no compiler publication/restart or policy mutation is needed for this task.
pub async fn review_local_setup_request(
    host: &ToolkitGatewayHost, prepared: LocalControllerSetup, approvals: &LocalApprovals,
) -> Result<OperatorParentOutcome, ConnectError> {
    let owner = host.local_operator_setup_for(&prepared)
        .map_err(|error| ConnectError::native("retain local setup-request owner", error))?;
    review_setup_request(owner, prepared, approvals, None).await
}

pub(crate) async fn review_application_setup_request(
    host: &ToolkitGatewayHost, prepared: LocalControllerSetup, approvals: &LocalApprovals,
    login_scope: &str,
) -> Result<OperatorParentOutcome, ConnectError> {
    let owner = host.application_operator_setup_for(&prepared)
        .map_err(|error| ConnectError::native("retain application setup-request owner", error))?;
    review_setup_request(owner, prepared, approvals, Some(login_scope)).await
}

async fn review_setup_request(
    owner: toolkit_mobkit::installed_host::operator_setup::OperatorSetup,
    prepared: LocalControllerSetup, approvals: &LocalApprovals, login_scope: Option<&str>,
) -> Result<OperatorParentOutcome, ConnectError> {
    let expires = SystemTime::now().checked_add(Duration::from_secs(120))
        .ok_or_else(|| ConnectError::InvalidInput("The setup review deadline is invalid".into()))?;
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-setup-agent")
            .map_err(|error| ConnectError::native("identify setup review requester", error))?,
        owner: ApprovalOwnerRef::Session { session_id: prepared.session_id().clone() },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime, id: ApprovalResourceId::new("original-setup-request"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Allow this original task to request connector setup".into(), body: None,
        },
        risk: ApprovalRisk::High, request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
        expires_at: Some(expires.into()),
        metadata: meerkat_core::SurfaceMetadata {
            app_context: login_scope.map(|scope| serde_json::json!({"protected_login_scope":scope})),
            ..Default::default()
        }, request_provenance: None,
    };
    // Both terminal and protected application reviews delegate the complete
    // recover/review/select sequence to its existing decision/source owner.
    owner.review_original_setup_request(prepared, request, |request| async {
        approvals.review_access(request).await
            .map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)
    }).await.map_err(|error| ConnectError::native("review or recover original setup selection", error))
}
