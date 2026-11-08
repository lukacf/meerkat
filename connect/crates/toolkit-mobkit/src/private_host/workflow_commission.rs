//! Fresh workflow commissioning on the existing private operation and listing owners.
use super::*;
use crate::installed_host::dynamic_members::AdmittedMemberClass;
use crate::private_originals::{
    PrivateAuthenticatedEvent, PrivateOriginalRef, PrivateOriginalSubmit,
};
use meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance;
use meerkat_workgraph::{WorkItemRef, WorkOwnerKey};

/// Application selections and requested bounds only. The host supplies the
/// actual profile, storage, backend, route parents, grant references and plan.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCommissionRequest {
    pub admission_id: Id,
    pub identity: meerkat_mobkit::identity_first::AgentIdentity,
    pub parent: PrivateOriginalSubmit,
    pub instance: CompiledMemberClassInstance,
    pub event: PrivateAuthenticatedEvent,
    pub content: String,
    pub workflow: Id,
    pub parent_work: WorkItemRef,
    pub parent_owner: WorkOwnerKey,
    pub cycle_label: String,
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub deadline_ms: u64,
    pub max_waiters: u16,
    pub hold_timeout_ms: u64,
    pub controller_issuance_id: meerkat_authorization_contracts::evidence::EvidenceId,
    pub planning_root_issuance_id: meerkat_authorization_contracts::evidence::EvidenceId,
    pub planning_leaf_issuance_id: meerkat_authorization_contracts::evidence::EvidenceId,
}

/// Prepare the exact future commissioning selection before P's first input.
/// IDs name independent P grants, never G grants or a recovered native result.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowParentRequest {
    pub commission: WorkflowCommissionRequest,
    pub parent_root_issuance_id: meerkat_authorization_contracts::evidence::EvidenceId,
    pub parent_leaf_issuance_id: meerkat_authorization_contracts::evidence::EvidenceId,
}

/// Native settlement and subsequent planning publication stay distinct. A
/// configuration refusal after Created does not roll back the native member
/// or the application's acknowledged desired-member publication.
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowCommissionResponse {
    Prepared {
        parent: PrivateOriginalRef,
        planning: PrivateOriginalRef,
        admission_id: Id,
        native: MemberClassResponse,
    },
    Native {
        outcome: MemberClassResponse,
    },
    CreatedWithoutPlanning {
        parent: PrivateOriginalRef,
        planning: PrivateOriginalRef,
        admission_id: Id,
        native: MemberClassResponse,
        error: WireError,
    },
    AddNotAccepted {
        native: MemberClassResponse,
        retirement: MemberClassControlOutcome,
    },
    PreparationRefused {
        error: WireError,
        retirement: MemberClassControlOutcome,
        physical_cleanup: Option<MemberClassControlOutcome>,
    },
}

/// Constructed only by the existing inherited-pipe owner. This is transport
/// custody for its existing application publication callback, not a roster.
pub struct WorkflowMemberPublication {
    endpoint: MemberClassEndpoint,
    operation_id: u64,
    selection: member_classes::ListingSelection,
    channel: member_classes::ListingChannel,
}
impl WorkflowMemberPublication {
    pub(super) fn new(
        endpoint: MemberClassEndpoint,
        operation_id: u64,
        selection: member_classes::ListingSelection,
        channel: member_classes::ListingChannel,
    ) -> Self {
        Self {
            endpoint,
            operation_id,
            selection,
            channel,
        }
    }
    /// Used only as the actual held callback's finalizer, after canonical
    /// member/route commitment and before native activation.
    pub async fn publish(
        self,
        admitted: &AdmittedMemberClass,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
        deadline: tokio::time::Instant,
    ) -> Result<(), HostLifetimeError> {
        if meerkat_mobkit::member_comms_id::roster_member_id_for_identity(identity.as_str())
            .as_str()
            != admitted.instance().member.member.member
        {
            return Err(HostLifetimeError::Denied);
        }
        let selected = member_classes::ListingSelection {
            admission_id: admitted.admission_id().clone(),
            identity: identity.to_string(),
            class: admitted.class().id.clone(),
        };
        if selected != self.selection {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.endpoint.check_workflow_publication(admitted)?;
        self.endpoint
            .request_listing(self.operation_id, selected, deadline, self.channel)
            .await
    }
    /// Cancellation signals the preparer's existing cleanup; it never drops
    /// an entered preparation future or claims the app mutation was joined.
    pub async fn closed(&self) {
        self.channel.closed.wait_closed().await;
    }
}

impl From<crate::host_gateway::MemberClassAddOutcome> for MemberClassResponse {
    fn from(outcome: crate::host_gateway::MemberClassAddOutcome) -> Self {
        member_classes::settled_result(outcome)
    }
}
