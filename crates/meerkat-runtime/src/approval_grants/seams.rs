//! The host seams of approval-carried grants: the approval branch, the
//! requester reply route, and the native custody that revokes a branch's
//! controller grant.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use meerkat_authorization::grants::{GrantRefusal, LocalGrantAuthority};
use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, ResourceDomain};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, NativeWorkTarget, OriginalWorkRef,
};
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::decision::DecisionId;
use meerkat_core::connection::RealmId;
use meerkat_core::{ControllerModelClient, PrincipalRef, SessionId};
use serde::{Deserialize, Serialize};

use crate::input::Input;
use crate::input_admission_custody::NativeInputAdmissionCustody;

/// Where an access request came from, and so where its notice and its reply
/// go: the origin work, in the realm and ingress domain it was bound in, and
/// the requester it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalOrigin {
    pub realm: RealmId,
    pub ingress: ResourceDomain,
    pub work: OriginalWorkRef,
    pub requester: PrincipalRef,
}

/// The grantee's own side of its approval branches: the controller grant
/// lineage the grantee's work runs under, and the principal it acts for. A
/// branch's controller grant is a child of the lineage's leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApprovalGranteeBasis {
    pub controller_grant_lineage: Vec<GrantLineageRef>,
    pub represented_subject: Option<PrincipalRef>,
}

impl ApprovalGranteeBasis {
    /// The grantee runs under `controller_grant_lineage`, acting for
    /// `represented_subject`.
    #[must_use]
    pub fn new(
        controller_grant_lineage: Vec<GrantLineageRef>,
        represented_subject: Option<PrincipalRef>,
    ) -> Self {
        Self {
            controller_grant_lineage,
            represented_subject,
        }
    }
}

/// What Core asks the host to open: the one-time approval branch of
/// `grantee` for `decision`, whose executor principal is `branch`, until
/// `expires_at_ms` (the approval grant's window end) at the latest.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApprovalBranchRequest {
    pub decision: DecisionId,
    pub grantee: PrincipalRef,
    pub branch: PrincipalRef,
    pub expires_at_ms: u64,
}

/// The branch the host opened, or found open: its session, and the branch's
/// own side of the approved work's binding (its destination, its pinned
/// controller, its ceilings). Built with [`Self::new`].
#[non_exhaustive]
pub struct ApprovalBranchSeat {
    pub session_id: SessionId,
    pub target: NativeWorkTarget,
    pub controller: ControllerModelClient,
    pub controller_ceiling: ExecutionRestrictions,
    pub admitted_ceiling: ExecutionRestrictions,
}

impl ApprovalBranchSeat {
    /// The branch on `session_id`, reached at `target`, with its live
    /// controller client pinned (`controller`) and no ceiling of its own.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        target: NativeWorkTarget,
        controller: ControllerModelClient,
    ) -> Self {
        Self {
            session_id,
            target,
            controller,
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
        }
    }

    /// The same seat with the branch's own ceilings.
    #[must_use]
    pub fn with_ceilings(
        mut self,
        controller_ceiling: ExecutionRestrictions,
        admitted_ceiling: ExecutionRestrictions,
    ) -> Self {
        self.controller_ceiling = controller_ceiling;
        self.admitted_ceiling = admitted_ceiling;
        self
    }
}

impl std::fmt::Debug for ApprovalBranchSeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalBranchSeat")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// The approved work as the branch admitted it: the admitting session, the
/// admitted input, and a future that resolves when the work's run ends.
#[non_exhaustive]
pub struct ApprovedWorkAdmission {
    pub session_id: SessionId,
    pub input_id: String,
    pub ended: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl ApprovedWorkAdmission {
    /// The input `input_id` admitted on `session_id`, whose run ends when
    /// `ended` resolves.
    #[must_use]
    pub fn new(
        session_id: SessionId,
        input_id: String,
        ended: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Self {
        Self {
            session_id,
            input_id,
            ended,
        }
    }
}

impl std::fmt::Debug for ApprovedWorkAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovedWorkAdmission")
            .field("session_id", &self.session_id)
            .field("input_id", &self.input_id)
            .finish_non_exhaustive()
    }
}

/// Why the host's approval branch did not do what Core asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ApprovalBranchError {
    /// For good: the grantee cannot be branched here, or the branch cannot
    /// run the approved work.
    #[error("approval branch refused: {0}")]
    Refused(String),
    /// Not now: Core asks again at a later pass.
    #[error("approval branch unavailable: {0}")]
    Unavailable(String),
    /// The host already holds another association enrolled for the approved
    /// work's original. The first enrollment stands.
    #[error("the approved work is enrolled with another association")]
    EnrollmentConflict,
}

/// The trusted host's approval branches: one-time members forked from a
/// grantee with an empty transcript, each running exactly one approved work
/// and conversing with nobody.
///
/// Contract:
/// - [`Self::grantee_basis`] is a synchronous read: `None` when the grantee
///   cannot be branched here (it is not an agent this host runs, or it
///   composes no branch).
/// - [`Self::open`] is idempotent per decision: the branch's identity
///   derives from the decision, so a retry finds the branch an earlier
///   attempt opened. The branch carries the authority label
///   [`super::APPROVAL_BRANCH_LABEL`] with the decision id, and no peer,
///   host or console ingress besides the approved work.
/// - [`Self::seat`] answers the open branch's own side of the approved
///   work's binding: its session, destination, pinned controller and
///   ceilings. It never waits on the branch's turn.
/// - [`Self::enroll`] puts the exact association Core minted for the
///   approved work in the host's custody of originals, first-writer-wins per
///   realm, ingress domain and original work, as for a governed peer work:
///   the same association again is accepted, and another one is refused
///   [`ApprovalBranchError::EnrollmentConflict`]. It is synchronous and
///   bounded.
/// - [`Self::admit`] admits the bound input into the branch of `decision` as
///   internal work, with the custody [`Self::enroll`] returned.
/// - [`Self::retire`] retires the branch of `decision` through the ordinary
///   member retire path; retiring a retired or never opened branch succeeds.
/// - [`Self::open_branches`] lists every branch open now, by its decision
///   (for [`super::ApprovalGrants::sweep_branches`]).
#[async_trait]
pub trait ApprovalBranchHost: Send + Sync {
    fn grantee_basis(&self, grantee: &PrincipalRef) -> Option<ApprovalGranteeBasis>;

    async fn open(&self, request: &ApprovalBranchRequest) -> Result<(), ApprovalBranchError>;

    async fn seat(&self, decision: &DecisionId) -> Result<ApprovalBranchSeat, ApprovalBranchError>;

    fn enroll(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Box<dyn NativeInputAdmissionCustody>>, ApprovalBranchError>;

    async fn admit(
        &self,
        decision: &DecisionId,
        input: Input,
        custody: Option<Box<dyn NativeInputAdmissionCustody>>,
    ) -> Result<ApprovedWorkAdmission, ApprovalBranchError>;

    async fn retire(&self, decision: &DecisionId) -> Result<(), ApprovalBranchError>;

    async fn open_branches(&self) -> Result<Vec<DecisionId>, ApprovalBranchError>;
}

/// The host's receipt of one delivered reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyReceipt {
    /// The host's reference of the delivery.
    pub delivery: String,
}

/// Why the requester reply route delivered nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReplyRouteError {
    /// The reply id was delivered before with another text. Nothing was
    /// delivered.
    #[error("a reply with another text was already delivered")]
    AlreadyDelivered,
    /// Not now; nothing was delivered.
    #[error("requester reply route unavailable: {0}")]
    Unavailable(String),
}

/// How an access request ended, as its origin work is told. Never content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccessOutcome {
    /// Approved: the approved work runs, and its answer goes to the
    /// requester directly.
    Approved,
    /// The approver declined.
    Declined,
    /// Undecided at the deadline, or approved and never executed.
    Expired,
    /// Withdrawn before a decision.
    Withdrawn,
    /// Approved, and refused at execution (for example the offering grant
    /// narrowed since).
    Refused,
}

impl AccessOutcome {
    /// The outcome's stable token (`approved`, `declined`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Withdrawn => "withdrawn",
            Self::Refused => "refused",
        }
    }
}

/// The trusted host's route from an approved work to its requester.
///
/// Contract:
/// - [`Self::can_deliver`] is a synchronous read, asked when an access offer
///   is held: whether a reply to `origin`'s requester can be delivered.
/// - [`Self::deliver`] delivers `text` to `origin`'s requester at most once
///   per `reply`: the same reply id with the same text again answers the
///   first delivery's receipt, and with another text is refused
///   [`ReplyRouteError::AlreadyDelivered`].
/// - [`Self::notify_origin`] tells `origin`'s work, at its grantee, how the
///   access request of `decision` ended, with no content: at least once,
///   idempotently per decision. `None` when it cannot now; it is asked again.
#[async_trait]
pub trait RequesterReplyRoute: Send + Sync {
    fn can_deliver(&self, origin: &ApprovalOrigin) -> bool;

    async fn deliver(
        &self,
        origin: &ApprovalOrigin,
        reply: &EvidenceId,
        text: &str,
    ) -> Result<ReplyReceipt, ReplyRouteError>;

    async fn notify_origin(
        &self,
        origin: &ApprovalOrigin,
        decision: &DecisionId,
        outcome: AccessOutcome,
    ) -> Option<ApprovalConsentDeliveryState>;
}

/// The native custody that revokes an approval branch's controller grant
/// once the branch is retired: the controller grant of a branch whose work
/// has not finished is never revoked. [`crate::meerkat_machine::MeerkatMachine`]
/// implements it over its controller grant mutation custody.
pub trait ApprovalControllerCustody: Send + Sync {
    /// Revoke `grant`, issued by `issuer`, unless unfinished work of this
    /// runtime still runs under it.
    ///
    /// # Errors
    /// The grant owner's refusal: `ControllerInUse` while work runs under
    /// it, `Unavailable` when the custody cannot be taken now.
    fn revoke_controller(
        &self,
        grants: &LocalGrantAuthority,
        issuer: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal>;
}

impl ApprovalControllerCustody for crate::meerkat_machine::MeerkatMachine {
    fn revoke_controller(
        &self,
        grants: &LocalGrantAuthority,
        issuer: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), GrantRefusal> {
        let mut custody = self
            .try_controller_grant_mutation()
            .map_err(|_| GrantRefusal::Unavailable)?;
        grants.revoke(issuer, grant, &mut custody)
    }
}
