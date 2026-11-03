//! The application's existing hidden admission row retains these locators.
//! This participant is transport/application custody, never grant authority.
use super::*;
use crate::installed_host::operator_setup::{MemberMcpBindingRef, MemberMcpError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use toolkit_package::Id;

/// `admission_id` is the exact Add request id, not a second correlation id.
/// The native owner supplies all fields from its current canonical admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberMcpAdmissionSelection {
    pub admission_id: Id,
    pub identity: String,
    pub class: Id,
}
impl MemberMcpAdmissionSelection {
    pub(crate) fn from_target(target: &operator_setup::member_mcp::MemberMcpTarget) -> Self {
        Self {
            admission_id: target.id.clone(),
            identity: meerkat_mobkit::member_comms_id::durable_identity_for_roster_member(
                &target.instance.member.member.member,
            ),
            class: target.instance.class.clone(),
        }
    }

    pub(crate) fn from_admission(
        admission: &dynamic_members::MemberClassAdmission,
    ) -> Result<Self, HostLifetimeError> {
        let identity = meerkat_mobkit::member_comms_id::durable_identity_for_roster_member(
            &admission.instance.member.member.member,
        );
        Ok(Self {
            admission_id: admission.id.clone(),
            identity: identity.to_string(),
            class: admission.instance.class.clone(),
        })
    }
}

/// The guarantee supplied by the actual application participant. Remote owners
/// explicitly select observation; an unavailable current owner never falls back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberMcpRequirementContract {
    #[default]
    CurrentOwner,
    ObservedRequirement,
}
impl MemberMcpRequirementContract {
    fn is_current_owner(&self) -> bool {
        *self == Self::CurrentOwner
    }
}

/// The existing application's exact accepted requirement, including its full
/// ordered MCP intent. This is review input, never approval or grant authority.
/// Explicit empty services are known absence; an unavailable row is not empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedMemberMcpRequirement {
    #[serde(
        default,
        skip_serializing_if = "MemberMcpRequirementContract::is_current_owner"
    )]
    pub contract: MemberMcpRequirementContract,
    pub selection: MemberMcpAdmissionSelection,
    pub source: crate::private_originals::PrivateSourceEvidence,
    pub services: Vec<operator_setup::MemberMcpIntent>,
}
impl AcceptedMemberMcpRequirement {
    pub(crate) fn check_owner(
        &self,
        application: &dyn MemberMcpApplication,
    ) -> Result<(), HostLifetimeError> {
        if self.contract != application.requirement_contract() {
            return Err(HostLifetimeError::Denied);
        }
        match self.contract {
            MemberMcpRequirementContract::CurrentOwner => {
                application.check_accepted_requirement(self)
            }
            MemberMcpRequirementContract::ObservedRequirement => Ok(()),
        }
    }
    pub fn check(
        &self,
        selection: &MemberMcpAdmissionSelection,
        source: &crate::private_originals::PrivateSourceEvidence,
    ) -> Result<(), HostLifetimeError> {
        if &self.selection != selection
            || &self.source != source
            || self.source.sha256.len() != 64
            || !self
                .source
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.services.len() > operator_setup::member_mcp::MAX_BINDINGS
            || self.services.iter().enumerate().any(|(index, service)| {
                service.check().is_err()
                    || self.services[..index]
                        .iter()
                        .any(|prior| prior.server == service.server)
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
}

/// Exact application-owned row value. Explicit [] means no MCP access;
/// absence, unreadability, a void/retired row or any mismatch must refuse.
/// Keep this value out of the public/model desired-roster projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedMemberMcpBindings {
    pub selection: MemberMcpAdmissionSelection,
    pub bindings: Vec<MemberMcpBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_requirement: Option<AcceptedMemberMcpRequirement>,
}
impl RetainedMemberMcpBindings {
    pub fn check(&self, expected: &MemberMcpAdmissionSelection) -> Result<(), HostLifetimeError> {
        if let Some(value) = &self.accepted_requirement {
            value.check(expected, &value.source)?;
            if value.services.len() != self.bindings.len() {
                return Err(HostLifetimeError::Denied);
            }
        }
        if &self.selection != expected
            || self.bindings.len() > operator_setup::member_mcp::MAX_BINDINGS
            || self
                .bindings
                .iter()
                .enumerate()
                .any(|(index, value)| self.bindings[..index].contains(value))
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    pub(crate) async fn reobserve_requirement(
        &self,
        application: &dyn MemberMcpApplication,
    ) -> Result<(), HostLifetimeError> {
        self.check(&self.selection)?;
        if let Some(expected) = &self.accepted_requirement {
            let current = application
                .accepted_requirement(self.selection.clone(), expected.source.clone())
                .await?;
            if &current != expected {
                return Err(HostLifetimeError::Denied);
            }
            expected.check_owner(application)?;
        }
        Ok(())
    }
}

/// Trusted application participant over its existing admitting/listed row.
/// Retain must atomically compare id/identity/class/nonvoided admitting state,
/// durably attach the complete set, and acknowledge only that exact commit.
/// Exact replay is allowed; differing refs never overwrite an existing value.
/// Lookup must check that same row and return its exact retained set. It cannot
/// issue access, reconstruct missing refs or infer [] from absence.
/// When accepted_requirement is present, retain atomically compares that full
/// expected source/intent with the row and preserves it with the bindings.
/// Source changes cannot overwrite this association, including an empty set.
#[async_trait]
pub trait MemberMcpApplication: Send + Sync {
    /// In-process owners keep the synchronous predicate. Only an explicit
    /// observed participant can receive rows reviewed with that weaker contract.
    fn requirement_contract(&self) -> MemberMcpRequirementContract {
        MemberMcpRequirementContract::CurrentOwner
    }
    async fn retain(&self, value: RetainedMemberMcpBindings) -> Result<(), HostLifetimeError>;
    async fn lookup(
        &self,
        selection: MemberMcpAdmissionSelection,
    ) -> Result<RetainedMemberMcpBindings, HostLifetimeError>;

    /// Read the full exact requirement from the same nonvoided admission row.
    /// Implementations must not derive it from model prose or a public roster.
    /// Missing support or missing intent must refuse, never return services=[].
    async fn accepted_requirement(
        &self,
        _selection: MemberMcpAdmissionSelection,
        _source: crate::private_originals::PrivateSourceEvidence,
    ) -> Result<AcceptedMemberMcpRequirement, HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }

    /// Recompare this entire row with the actual application owner at native
    /// issue. Called under the native member writer: no I/O, awaits or reentry
    /// into Connect. A cached copy is not currentness. This is the existing
    /// currentness predicate contract, not new approval or a source-mutation
    /// exclusion guarantee; implementations need their actual owner state.
    fn check_accepted_requirement(
        &self,
        _requirement: &AcceptedMemberMcpRequirement,
    ) -> Result<(), HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }
}

impl PrivateOriginals {
    pub fn capture_member_mcp_factory(
        &self,
        construction: &meerkat_mobkit::rpc_gateway::GatewayFactoryConstruction<'_>,
    ) -> Result<(), HostLifetimeError> {
        let factory = operator_setup::member_mcp::MemberMcpFactory::capture(construction)
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.member_mcp_factory
            .set(factory)
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    pub(crate) fn member_mcp_factory_host(
        &self,
        operator: &operator_setup::OperatorSetup,
    ) -> Result<operator_setup::MemberMcpSetupHost, MemberMcpError> {
        operator.member_mcp_setup_host_from_factory(
            self.member_mcp_factory
                .get()
                .ok_or(MemberMcpError::Unavailable)?,
        )
    }
    pub fn bind_member_mcp_application(
        &self,
        participant: Arc<dyn MemberMcpApplication>,
    ) -> Result<(), HostLifetimeError> {
        self.member_mcp_application
            .set(participant)
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    pub(crate) async fn retained_member_mcp_bindings(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<Vec<MemberMcpBindingRef>>, MemberMcpError> {
        let policy = self.member_mcp_policy()?;
        let Some(admission) = policy.observed_admission(identity.member())? else {
            return Ok(None);
        };
        let selection = MemberMcpAdmissionSelection::from_admission(&admission)?;
        let participant = self
            .member_mcp_application
            .get()
            .ok_or(MemberMcpError::Unavailable)?;
        let retained = participant.lookup(selection.clone()).await?;
        retained.check(&selection)?;
        retained.reobserve_requirement(participant.as_ref()).await?;
        // A cold callback is not authority. Recheck canonical admission after
        // application I/O; a successor cannot inherit the predecessor's refs.
        let current = policy
            .observed_admission(identity.member())?
            .ok_or(MemberMcpError::Denied)?;
        if current.id != admission.id
            || current.class != admission.class
            || current.instance != admission.instance
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(Some(retained.bindings))
    }
}

/// Explicit Add review intent. The enrollment is merely an existing
/// private-pipe login locator; only its authenticated Console action approves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberMcpAddIntent {
    pub origin: operator_setup::MemberMcpOrigin,
    pub console_enrollment_id: u64,
    pub services: Vec<operator_setup::MemberMcpIntent>,
}

/// Actual protected decision custody, retained through native issue settlement.
/// The reservation is entered-operation/drain custody, not an expiry mutex:
/// retirement can race an already-entered issue. No login mutex is held while
/// the native owner takes its publication writer.
pub struct MemberMcpApproval {
    pub approval: meerkat_core::ApprovalRecord,
    pub reservation: crate::private_host::ConsoleLoginReservation,
    pub current: Arc<dyn Fn() -> bool + Send + Sync>,
}
#[async_trait]
pub trait MemberMcpPresenter: Send + Sync {
    async fn review(
        &self,
        selection: MemberMcpAdmissionSelection,
        login: crate::private_host::ConsoleLoginHandle,
        request: meerkat_core::ApprovalRequest,
        closed: crate::private_host::PrivateHostClosed,
    ) -> Result<MemberMcpApproval, HostLifetimeError>;
}

/// The accepted Add retains its existing owners through settlement. This is
/// task-local cleanup custody, never a serialized grant or another owner.
pub(crate) struct PreparedMemberMcp {
    pub(crate) operator: operator_setup::OperatorSetup,
    pub(crate) originals: Arc<PrivateOriginals>,
    pub(crate) bindings: Vec<MemberMcpBindingRef>,
}

impl crate::host_gateway::ToolkitGatewayHost {
    pub fn with_member_mcp_presenter(
        self,
        presenter: Arc<dyn MemberMcpPresenter>,
    ) -> Result<Self, HostLifetimeError> {
        self.bind_member_mcp_presenter(presenter)?;
        Ok(self)
    }
    pub(crate) async fn prepare_member_mcp_add(
        &self,
        admitted: &dynamic_members::AdmittedMemberClass,
        request: Option<(MemberMcpAddIntent, crate::private_host::ConsoleLoginHandle)>,
    ) -> Result<Option<PreparedMemberMcp>, HostLifetimeError> {
        use meerkat_core::{
            ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
            ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
            ApprovalResourceRef, ApprovalRisk,
        };
        let originals = self.member_mcp_originals()?;
        let target = originals
            .member_mcp_target(admitted)
            .await
            .map_err(|_| HostLifetimeError::Denied)?;
        let selection = MemberMcpAdmissionSelection::from_target(&target);
        let participant = originals
            .member_mcp_application
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let Some((request, login)) = request else {
            participant
                .retain(RetainedMemberMcpBindings {
                    selection,
                    bindings: vec![],
                    accepted_requirement: None,
                })
                .await?;
            return Ok(None);
        };
        let accepted = match &request.origin {
            operator_setup::MemberMcpOrigin::OperatorSelf => None,
            operator_setup::MemberMcpOrigin::AcceptedApplicationRequirement { source } => {
                let value = participant
                    .accepted_requirement(selection.clone(), source.clone())
                    .await?;
                value.check(&selection, source)?;
                if value.services != request.services {
                    return Err(HostLifetimeError::Denied);
                }
                value.check_owner(participant.as_ref())?;
                Some(value)
            }
        };
        if (request.services.is_empty() && accepted.is_none())
            || request.services.len() > operator_setup::member_mcp::MAX_BINDINGS
            || request.services.iter().enumerate().any(|(i, service)| {
                request.services[..i]
                    .iter()
                    .any(|prior| prior.server == service.server)
            })
        {
            return Err(HostLifetimeError::Denied);
        }
        login.check().map_err(|_| HostLifetimeError::Denied)?;
        if request.services.is_empty() {
            let expected = accepted.as_ref().ok_or(HostLifetimeError::Denied)?;
            participant
                .retain(RetainedMemberMcpBindings {
                    selection,
                    bindings: vec![],
                    accepted_requirement: accepted.clone(),
                })
                .await?;
            let current = participant
                .accepted_requirement(expected.selection.clone(), expected.source.clone())
                .await?;
            if &current != expected {
                return Err(HostLifetimeError::Denied);
            }
            expected.check_owner(participant.as_ref())?;
            return Ok(None);
        }
        let operator = self.member_mcp_operator()?;
        let factory = originals
            .member_mcp_factory_host(&operator)
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let presenter = originals
            .member_mcp_presenter
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let mut reviews = Vec::new();
        let mut bindings = Vec::new();
        let result = async {
            for service in request.services {
                let approval_request = ApprovalRequest {
                    requester: ApprovalPrincipalId::new("connect-member-operator")
                        .map_err(|_| HostLifetimeError::Denied)?,
                    owner: ApprovalOwnerRef::Runtime,
                    resource: ApprovalResourceRef {
                        kind: ApprovalResourceKind::Runtime,
                        id: ApprovalResourceId::new("configured-member-access"),
                    },
                    proposed_action: ApprovalProposedAction {
                        kind: ApprovalActionKind::Other,
                        summary: String::new(),
                        body: None,
                    },
                    risk: ApprovalRisk::High,
                    request_body: None,
                    allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                        .into_iter()
                        .collect(),
                    expires_at: std::time::SystemTime::now()
                        .checked_add(std::time::Duration::from_secs(120))
                        .map(Into::into),
                    metadata: Default::default(),
                    request_provenance: None,
                };
                let review = operator
                    .prepare_member_mcp_review(
                        &originals,
                        &factory,
                        admitted,
                        request.origin.clone(),
                        service,
                        approval_request,
                    )
                    .await
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                bindings.push(review.binding().clone());
                if review.accepted_requirement() != accepted.as_ref() {
                    return Err(HostLifetimeError::Denied);
                }
                reviews.push(review);
            }
            // Persist the complete pending locator set before any grant issue.
            // On lost ACK the same application row/Connect records are the
            // recovery inputs. Native held construction has not begun.
            participant
                .retain(RetainedMemberMcpBindings {
                    selection: selection.clone(),
                    bindings: bindings.clone(),
                    accepted_requirement: accepted.clone(),
                })
                .await?;
            for review in &reviews {
                let entered = presenter
                    .review(
                        selection.clone(),
                        login.clone(),
                        review.request().clone(),
                        self.member_mcp_closed(),
                    )
                    .await?;
                let result = operator
                    .finalize_member_mcp_review(
                        &originals,
                        &factory,
                        review,
                        entered.approval,
                        &|| (entered.current)(),
                    )
                    .await;
                if matches!(
                    result,
                    Err(MemberMcpError::PendingReconciliation | MemberMcpError::Unavailable)
                ) {
                    entered.reservation.uncertain();
                } else {
                    entered.reservation.settled();
                }
                result.map_err(|_| HostLifetimeError::Denied)?;
            }
            Ok(())
        }
        .await;
        let prepared = PreparedMemberMcp {
            operator,
            originals,
            bindings,
        };
        if let Err(primary) = result {
            let cleanup = self.cleanup_member_mcp_bindings(Some(&prepared)).await;
            return Err(if cleanup.is_err() {
                HostLifetimeError::NotSettled
            } else {
                primary
            });
        }
        Ok(Some(prepared))
    }
    pub(crate) async fn cleanup_member_mcp_bindings(
        &self,
        prepared: Option<&PreparedMemberMcp>,
    ) -> Result<(), HostLifetimeError> {
        let Some(prepared) = prepared else {
            return Ok(());
        };
        let mut failure = None;
        for binding in &prepared.bindings {
            if prepared
                .operator
                .revoke_member_mcp_binding(&prepared.originals, binding)
                .await
                .is_err()
            {
                failure = Some(HostLifetimeError::NotSettled);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
