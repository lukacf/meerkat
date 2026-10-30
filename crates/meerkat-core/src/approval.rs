//! Durable approval records and service contracts.
//!
//! Generated approval lifecycle authority owns approval status transitions.
//! Public surfaces may request, list, read, and decide approvals; the service
//! stores and projects the generated lifecycle decisions.
//!
//! The same generated owner also holds process-local review attempts for
//! retained native operations (see [`review`]). Review attempts are never
//! persisted or restored, so a stored approval record can never reconstruct
//! a review allow or spendable consent. Human consent for a retained R3
//! candidate (see [`consent`]) is one such attempt plus its approval record.

pub mod consent;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod consent_test_support;
mod owner_loss;
pub mod review;

pub use owner_loss::ConsentOwnerLossSettlement;

use crate::generated::approval_lifecycle::{
    ApprovalLifecycleDecision, ApprovalLifecycleError, ApprovalLifecycleMachineAuthority,
    ApprovalLifecycleOutcome, ApprovalLifecycleRejectionReason, ApprovalLifecycleStatus,
    ReviewAttemptStatus, ReviewRetirementReason, ReviewVerdict,
};
use crate::lifecycle::identifiers::RunId;
use crate::{PrincipalRef, SessionId, SurfaceMetadata, ToolCallId};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use review::{ReservedReviewError, ReviewAttemptHandle, ReviewOwnerError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Durable approval id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl ApprovalId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ApprovalId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(value)?))
    }
}

/// Principal identifier used for requester and decision actor projections.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalPrincipalId(String);

impl ApprovalPrincipalId {
    pub fn new(value: impl Into<String>) -> Result<Self, ApprovalError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalPrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed reference to a mob owning an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalMobRef(String);

impl ApprovalMobRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalMobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed reference to a mob member owning an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalMemberRef(String);

impl ApprovalMemberRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalMemberRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed identifier for the resource affected by an approval.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ApprovalResourceId(String);

impl ApprovalResourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApprovalResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed owner for an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "owner_type", rename_all = "snake_case")]
pub enum ApprovalOwnerRef {
    Runtime,
    Session {
        session_id: SessionId,
    },
    Mob {
        mob_id: ApprovalMobRef,
    },
    Run {
        run_id: RunId,
    },
    ToolCall {
        tool_call_id: ToolCallId,
    },
    ExternalMember {
        mob_id: ApprovalMobRef,
        member_ref: ApprovalMemberRef,
    },
}

/// Typed resource kind affected by an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResourceKind {
    File,
    ShellCommand,
    ToolCall,
    Device,
    Runtime,
    Network,
    Other,
}

/// Resource affected by an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalResourceRef {
    pub kind: ApprovalResourceKind,
    pub id: ApprovalResourceId,
}

/// Typed action kind for the proposed action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalActionKind {
    ShellCommand,
    FileWrite,
    FileDelete,
    NetworkCall,
    DeviceControl,
    ToolCall,
    Other,
}

/// Action proposed by the requester.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalProposedAction {
    pub kind: ApprovalActionKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// Risk classification for an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
    Critical,
}

/// Allowed terminal decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Approval record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Cancelled,
}

/// Durable decision audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalDecisionRecord {
    pub decision: ApprovalDecision,
    pub actor: ApprovalPrincipalId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub decided_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<serde_json::Value>,
}

/// Durable approval record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalRecord {
    pub approval_id: ApprovalId,
    pub status: ApprovalStatus,
    pub requester: ApprovalPrincipalId,
    pub owner: ApprovalOwnerRef,
    pub resource: ApprovalResourceRef,
    pub proposed_action: ApprovalProposedAction,
    pub risk: ApprovalRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<serde_json::Value>,
    pub allowed_decisions: BTreeSet<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub updated_at: DateTime<Utc>,
    pub metadata: SurfaceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_provenance: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecisionRecord>,
    /// Present only on an approval the native owner issued for one retained
    /// R3 candidate (human consent). History, never an executable
    /// capability: spendable consent lives only in the owner's process-local
    /// custody of that candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ApprovalConsentRecord>,
}

/// Authentication assurance the host's ingress owner established for one
/// decision event. A displayed identity alone never implies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAssurance {
    /// The person was authenticated on the actual channel event.
    PersonAuthenticated,
    /// A delegated service credential authenticated the event.
    ServiceCredential,
    /// Only device or network trust was established.
    DeviceTrusted,
}

/// The authenticated approver recorded on a consent decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentApprover {
    pub principal: PrincipalRef,
    pub assurance: ApprovalAssurance,
}

/// Passive owner scope of a consent approval, committed with its issuance:
/// the hosting domain and the session of the run that admitted the
/// candidate. On the native path the runtime fills it from the work it
/// admitted and staged; request metadata never supplies it. A trusted host
/// that builds its own work context may set it too
/// ([`crate::WorkAuthorizationContext::with_consent_scope`]).
///
/// Never authority. Every owner-authored transition of the row keeps it
/// unchanged; that is not tamper-proofing, since the store row is writable.
/// Recovery, wake and handoff re-validate it against the actual hosting
/// claim and current candidate and never treat it as custody. A consent row
/// without it takes no part in them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentScope {
    pub domain: crate::session_hosting::HostingDomainRef,
    pub session_id: SessionId,
}

/// Consent facts of an approval issued for one retained R3 candidate.
///
/// A durable projection of the approval owner's own transitions. Decision
/// status, spend and execution are separate facts: `Approved` with no
/// `consumed_at` is not spendable after a restart, and `consumed_at` is not
/// proof that the effect completed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentRecord {
    /// The owner scope committed at issuance, when the admitting run had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ApprovalConsentScope>,
    /// The authenticated approver of the recorded decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<ApprovalConsentApprover>,
    /// When a fresh exact attempt consumed the approval at its entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub consumed_at: Option<DateTime<Utc>>,
    /// The owner withdrew the unspent consent: a pending approval is then
    /// `Cancelled` (never a denial), and approved history stays `Approved`
    /// but can no longer be spent. Never set on spent consent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawal: Option<ApprovalConsentWithdrawal>,
    /// The retained work of the run that admitted the candidate, committed
    /// at issuance with the scope. Together they are the exact native
    /// destination of this consent's notices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ApprovalConsentOrigin>,
    /// One delivery obligation per committed transition that owes the
    /// owning session a notice, committed with that transition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliveries: Vec<ApprovalConsentDelivery>,
    /// Cross-run custody history of the retained candidate. History only:
    /// custody itself is process-local and never restored from this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custody: Option<ApprovalConsentCustody>,
}

impl ApprovalConsentRecord {
    /// Whether this consent has an exact native destination for its notices:
    /// both a committed scope and a committed origin. A row without both owes
    /// no notice and takes no part in delivery or transfer.
    pub fn routable(&self) -> bool {
        self.scope.is_some() && self.origin.is_some()
    }

    /// The delivery obligation of `transition`, if one was committed.
    pub fn delivery(
        &self,
        transition: ApprovalConsentTransition,
    ) -> Option<&ApprovalConsentDelivery> {
        self.deliveries
            .iter()
            .find(|delivery| delivery.transition == transition)
    }

    /// Owe `transition`'s notice once, when the row is routable. Called only
    /// on the record that commits `transition`, so the obligation is durable
    /// exactly when the transition is.
    pub(crate) fn owe_delivery(
        &mut self,
        transition: ApprovalConsentTransition,
        committed_at: DateTime<Utc>,
    ) {
        if self.routable() && self.delivery(transition).is_none() {
            self.deliveries.push(ApprovalConsentDelivery {
                transition,
                committed_at,
                state: ApprovalConsentDeliveryState::Owed,
            });
        }
    }
}

/// The retained work of the run that admitted a consent candidate: with the
/// scope, the exact native destination of the consent's notices.
///
/// Committed at issuance and never changed. Never authority: a notice is
/// admitted only through the runtime's own retained-resume validation of
/// this identity, and a cross-run transfer compares the runtime-minted
/// resume evidence, never this copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentOrigin {
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub retained_work: crate::retained_work::RetainedWorkIdentity,
}

/// A committed consent transition that owes the owning session a notice.
/// Each transition is owed on its own: a later transition never settles,
/// replaces or retracts an earlier one's notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalConsentTransition {
    /// An authenticated human approved it.
    Approved,
    /// An authenticated human declined it.
    Denied,
    /// The owner withdrew the unspent consent, for any withdrawal reason
    /// (the row's `withdrawal` says which).
    Withdrawn,
}

impl ApprovalConsentTransition {
    /// The transition's stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Withdrawn => "withdrawn",
        }
    }
}

impl FromStr for ApprovalConsentTransition {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "withdrawn" => Ok(Self::Withdrawn),
            _ => Err(()),
        }
    }
}

/// One committed transition's notice obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentDelivery {
    pub transition: ApprovalConsentTransition,
    /// When the transition (and with it this obligation) committed.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub committed_at: DateTime<Utc>,
    pub state: ApprovalConsentDeliveryState,
}

/// Where one transition's notice stands. It is committed `Owed` with its
/// transition, and only the delivery owner's exact receipt for that same
/// transition settles it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalConsentDeliveryState {
    /// Committed with its transition and not known to be admitted. An
    /// unknown admission stays owed until it is reconciled.
    Owed,
    /// The owning session's native admission durably accepted the notice.
    /// Not model observation and not execution: a notice wakes work and
    /// spends nothing.
    Admitted {
        session_id: SessionId,
        input_id: String,
    },
    /// Proven terminal non-delivery, such as a settled policy refusal or a
    /// retired owner. Never inferred from a later refusal after an unknown
    /// earlier attempt.
    Failed { reason: String },
}

/// Cross-run custody history of a retained consent candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ApprovalConsentCustody {
    /// Parked at its run's close for a resumed run of its origin work.
    Parked,
    /// Transferred once into the resumed run staged with this admitted
    /// notice input.
    Transferred { input_id: String },
}

/// The owner's durable withdrawal of unspent consent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalConsentWithdrawal {
    pub reason: ApprovalConsentWithdrawalReason,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub withdrawn_at: DateTime<Utc>,
}

/// Why the owner withdrew unspent consent. None of these is a human denial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalConsentWithdrawalReason {
    /// The run that admitted the candidate closed.
    RunEnded,
    /// The consented decision is no longer the current one.
    ContextChanged,
    /// The current authorization refused the consented operation.
    AuthorityChanged,
    /// The consent request or its spendability expired.
    Expired,
    /// The owner abandoned the custody (for example after a conflicting or
    /// uncertain store commit).
    Abandoned,
    /// The admitted notice input that would have moved the parked consent
    /// into a resumed run ended before the move. It names that input only:
    /// how the input ended stays the runtime owner's fact.
    NoticeInputEnded { input_id: String },
}

impl From<ReviewRetirementReason> for ApprovalConsentWithdrawalReason {
    fn from(reason: ReviewRetirementReason) -> Self {
        match reason {
            ReviewRetirementReason::RunEnded => Self::RunEnded,
            ReviewRetirementReason::ContextChanged => Self::ContextChanged,
            ReviewRetirementReason::AuthorityChanged => Self::AuthorityChanged,
            ReviewRetirementReason::DeadlineExpired => Self::Expired,
            ReviewRetirementReason::Abandoned => Self::Abandoned,
        }
    }
}

/// Input used by tools/runtime code to request an approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalRequest {
    pub requester: ApprovalPrincipalId,
    pub owner: ApprovalOwnerRef,
    pub resource: ApprovalResourceRef,
    pub proposed_action: ApprovalProposedAction,
    pub risk: ApprovalRisk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<serde_json::Value>,
    pub allowed_decisions: BTreeSet<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "SurfaceMetadata::is_empty")]
    pub metadata: SurfaceMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_provenance: Option<serde_json::Value>,
}

/// Filter for listing approvals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovalListFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ApprovalStatus>,
}

/// Errors from the approval service.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval not found: {approval_id}")]
    NotFound { approval_id: ApprovalId },
    #[error("approval has already been decided: {approval_id}")]
    AlreadyDecided { approval_id: ApprovalId },
    #[error("approval is expired: {approval_id}")]
    Expired { approval_id: ApprovalId },
    #[error("decision is not allowed for approval: {decision:?}")]
    InvalidDecision { decision: ApprovalDecision },
    #[error("approval request must allow at least one decision")]
    EmptyAllowedDecisions,
    #[error("approval principal id must not be empty")]
    InvalidPrincipal,
    /// A consent approval accepts only an authenticated human decision;
    /// caller-supplied actor text is not authentication.
    #[error("approval requires an authenticated human decision: {approval_id}")]
    AuthenticationRequired { approval_id: ApprovalId },
    /// The authenticated approver is not currently eligible to decide it.
    #[error("approver is not eligible to decide approval: {approval_id}")]
    NotEligible { approval_id: ApprovalId },
    /// This owner holds no live retained candidate for the approval (it was
    /// consumed, declined, expired, or lost on restart): fresh consent is
    /// required.
    #[error("approval has no live consent candidate: {approval_id}")]
    ConsentUnavailable { approval_id: ApprovalId },
    #[error(transparent)]
    InvalidMetadata(#[from] crate::SurfaceMetadataError),
    /// The store committed nothing (unavailable, contended or failed before
    /// any write). An explicitly attempted retry through fresh checks is
    /// valid; nothing is resent automatically. Also the answer of a consent
    /// approval that recorded nothing because its candidate's authorization
    /// or its approver's eligibility could not be settled now (an owner
    /// change in progress, owners that are unavailable, or a publication
    /// that kept changing; ADR-001 D9): the same retry is valid.
    #[error("approval store unavailable: {0}")]
    StoreUnavailable(String),
    /// Another owner of the backing changed the row first. Nothing of this
    /// commit was published; the row is reconciled before reuse.
    #[error("approval changed in the store since it was checked: {approval_id}")]
    StoreConflict { approval_id: ApprovalId },
    /// The store may or may not have committed. Nothing was published and
    /// any live consent custody was retired; the row is reconciled from the
    /// store before reuse.
    #[error("approval store commit outcome unknown: {approval_id}")]
    StoreCommitUnknown { approval_id: ApprovalId },
    /// The row awaits reconciliation and its stored form is unreadable,
    /// missing or invalid: it stays unavailable, never spendable.
    #[error("approval awaits reconciliation with its store: {approval_id}")]
    Unreconciled { approval_id: ApprovalId },
    /// A stored row contradicts its own history (status, decision audit or
    /// consent facts); it is never restored.
    #[error("stored approval is invalid: {approval_id}")]
    InvalidStoredRecord { approval_id: ApprovalId },
    /// An internal approval lifecycle invariant failed.
    #[error("approval lifecycle invariant failed: {0}")]
    LifecycleInvariant(String),
}

/// Durable approval store mechanics.
///
/// Stores persist full records and do not decide status legality. Every
/// write is conditional on the row the owner checked, which makes the store
/// the one commit domain for every owner of the same backing (other
/// instances or processes): a stale owner can neither overwrite another
/// owner's decision nor record a consumed consent as unspent.
pub trait ApprovalStore: Send + Sync {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError>;
    /// The stored row for one approval, for reconciliation.
    fn get(&self, approval_id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError>;
    /// Write `record` only if the stored row for its id still equals
    /// `expected` (`None`: no row yet). Never waits for another owner of the
    /// backing. The error says what is known about the write: nothing was
    /// written ([`ApprovalStoreError::NotCommitted`],
    /// [`ApprovalStoreError::Contended`]), another row won
    /// ([`ApprovalStoreError::Conflict`]), or the write may have committed
    /// ([`ApprovalStoreError::CommitUnknown`]).
    fn put_if_current(
        &self,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError>;
    fn is_persistent(&self) -> bool;
}

/// Approval store outcomes, each a statement about commitment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalStoreError {
    /// Proven no write: the failure happened before anything could commit.
    #[error("{0}")]
    NotCommitted(String),
    /// The stored row is no longer the one the owner checked; nothing was
    /// written by this call.
    #[error("approval row changed since it was checked")]
    Conflict,
    /// Another owner of the backing holds it; nothing was written.
    #[error("approval store is held by another owner")]
    Contended,
    /// The write may have committed (for example, a failed rename that the
    /// filesystem cannot prove did not happen).
    #[error("approval store commit outcome unknown: {0}")]
    CommitUnknown(String),
}

impl ApprovalStoreError {
    /// The service outcome of a failed conditional write of `approval_id`.
    fn into_commit_error(self, approval_id: &ApprovalId) -> ApprovalError {
        match self {
            Self::NotCommitted(reason) => ApprovalError::StoreUnavailable(reason),
            Self::Contended => ApprovalError::StoreUnavailable(self.to_string()),
            Self::Conflict => ApprovalError::StoreConflict {
                approval_id: approval_id.clone(),
            },
            Self::CommitUnknown(_) => ApprovalError::StoreCommitUnknown {
                approval_id: approval_id.clone(),
            },
        }
    }

    /// Whether this failed write leaves the stored row unknown to the owner.
    fn leaves_row_unknown(&self) -> bool {
        matches!(self, Self::Conflict | Self::CommitUnknown(_))
    }
}

/// In-memory approval store for tests and process-local runtimes.
#[derive(Debug, Default)]
pub struct InMemoryApprovalStore {
    records: RwLock<BTreeMap<ApprovalId, ApprovalRecord>>,
}

impl InMemoryApprovalStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ApprovalStore for InMemoryApprovalStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        Ok(self.records.read().values().cloned().collect())
    }

    fn get(&self, approval_id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        Ok(self.records.read().get(approval_id).cloned())
    }

    /// Never waits: a map held by another owner (a service sharing this
    /// store) is [`ApprovalStoreError::Contended`], and nothing is written.
    fn put_if_current(
        &self,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        let Some(mut records) = self.records.try_write() else {
            return Err(ApprovalStoreError::Contended);
        };
        if records.get(&record.approval_id) != expected {
            return Err(ApprovalStoreError::Conflict);
        }
        records.insert(record.approval_id.clone(), record.clone());
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
struct ApprovalServiceState {
    records: BTreeMap<ApprovalId, ApprovalRecord>,
    authority: ApprovalLifecycleMachineAuthority,
    /// Process-local custody of retained R3 candidates, by approval. Never
    /// persisted or restored: spendable consent dies with the process. It
    /// survives an in-process rebuild of the session's agent (the same
    /// owner, the session's claim lineage joined), whose fresh resolution of
    /// the retained call it is rebound to only as the same durable
    /// execution; a process restart still drops it.
    consent: BTreeMap<ApprovalId, Arc<consent::ConsentCandidate>>,
    /// Consent approvals this service issued itself. A consent row loaded
    /// from a shared store without such an entry may still be held by
    /// another owner; this service cannot infer that custody was lost, and
    /// reports it unavailable without claiming anything about that owner.
    issued_consent: BTreeSet<ApprovalId>,
    /// Rows whose stored form this owner no longer knows (a conflicting or
    /// uncertain commit, or an unconfirmed withdrawal), with the mark that
    /// reconciliation must still observe before it publishes.
    unreconciled: BTreeMap<ApprovalId, u64>,
    next_mark: u64,
    /// Withdrawals whose custody is already retired but whose durable record
    /// (and `Withdrawn` notice) waits for a store-capable owner holder.
    pending_withdrawals: Vec<consent::PendingWithdrawal>,
    /// Decided consent whose allowance is not published yet: its custody is
    /// visible to withdrawal but cannot be spent until the decision's final
    /// checks, after its store and owner-exit work, publish it.
    unpublished: BTreeSet<ApprovalId>,
    /// Retained candidates parked at their run's close for a resumed run of
    /// their origin work. Their custody is kept, but nothing can spend them
    /// until a confirmed transfer publishes them for that resumed run.
    parked: BTreeSet<ApprovalId>,
    /// Parks whose durable record waits for a store-capable owner holder.
    pending_parks: Vec<ApprovalId>,
    /// Runs open now whose staged inputs resume retained work: parked
    /// consent their committed notice receipts name moves into them.
    resumed_runs: Vec<consent::ResumedRun>,
    /// Staged inputs of resumed runs that closed while parked consent of the
    /// work they resumed, for their destination, still awaited its approved
    /// notice's receipt: a receipt naming one arrives after its only run.
    closed_inputs: Vec<consent::ClosedInput>,
    /// Withdrawals whose custody is retired and whose durable record is not
    /// yet confirmed, each with its exact cause: owed from before its first
    /// conditional write, and recorded against its row once that row is
    /// known (reconciled after a failed or uncertain write).
    unreconciled_withdrawals: BTreeMap<ApprovalId, consent::PendingWithdrawal>,
}

impl ApprovalServiceState {
    fn empty() -> Self {
        Self {
            records: BTreeMap::new(),
            authority: ApprovalLifecycleMachineAuthority::new(),
            consent: BTreeMap::new(),
            issued_consent: BTreeSet::new(),
            unreconciled: BTreeMap::new(),
            next_mark: 0,
            pending_withdrawals: Vec::new(),
            unpublished: BTreeSet::new(),
            parked: BTreeSet::new(),
            pending_parks: Vec::new(),
            resumed_runs: Vec::new(),
            closed_inputs: Vec::new(),
            unreconciled_withdrawals: BTreeMap::new(),
        }
    }

    fn from_records(records: Vec<ApprovalRecord>) -> Result<Self, ApprovalError> {
        let mut state = Self::empty();
        for record in records {
            restore_record_into_authority(&mut state.authority, &record)?;
            state.records.insert(record.approval_id.clone(), record);
        }
        Ok(state)
    }

    /// The stored row of `approval_id` is no longer known: it is reconciled
    /// before any reuse.
    fn mark_unreconciled(&mut self, approval_id: &ApprovalId) {
        self.next_mark += 1;
        self.unreconciled
            .insert(approval_id.clone(), self.next_mark);
    }

    /// The reconcile-before-reuse barrier, enforced under the reservation
    /// itself: a marked row is never read, decided or written here.
    fn ensure_reconciled(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        if self.unreconciled.contains_key(approval_id) {
            return Err(ApprovalError::Unreconciled {
                approval_id: approval_id.clone(),
            });
        }
        Ok(())
    }
}

fn approval_lifecycle_status(status: ApprovalStatus) -> ApprovalLifecycleStatus {
    match status {
        ApprovalStatus::Pending => ApprovalLifecycleStatus::Pending,
        ApprovalStatus::Approved => ApprovalLifecycleStatus::Approved,
        ApprovalStatus::Denied => ApprovalLifecycleStatus::Denied,
        ApprovalStatus::Expired => ApprovalLifecycleStatus::Expired,
        ApprovalStatus::Cancelled => ApprovalLifecycleStatus::Cancelled,
    }
}

fn approval_status_from_lifecycle(status: ApprovalLifecycleStatus) -> ApprovalStatus {
    match status {
        ApprovalLifecycleStatus::Pending => ApprovalStatus::Pending,
        ApprovalLifecycleStatus::Approved => ApprovalStatus::Approved,
        ApprovalLifecycleStatus::Denied => ApprovalStatus::Denied,
        ApprovalLifecycleStatus::Expired => ApprovalStatus::Expired,
        ApprovalLifecycleStatus::Cancelled => ApprovalStatus::Cancelled,
    }
}

fn approval_lifecycle_decision(decision: ApprovalDecision) -> ApprovalLifecycleDecision {
    match decision {
        ApprovalDecision::Approve => ApprovalLifecycleDecision::Approve,
        ApprovalDecision::Deny => ApprovalLifecycleDecision::Deny,
    }
}

fn allowed_decision_flags(allowed_decisions: &BTreeSet<ApprovalDecision>) -> (bool, bool) {
    (
        allowed_decisions.contains(&ApprovalDecision::Approve),
        allowed_decisions.contains(&ApprovalDecision::Deny),
    )
}

fn lifecycle_rejection_error(
    approval_id: &ApprovalId,
    reason: ApprovalLifecycleRejectionReason,
    decision: Option<ApprovalDecision>,
) -> ApprovalError {
    match reason {
        ApprovalLifecycleRejectionReason::NotFound => ApprovalError::NotFound {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::AlreadyDecided => ApprovalError::AlreadyDecided {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::Expired => ApprovalError::Expired {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::InvalidDecision => {
            let Some(decision) = decision else {
                return ApprovalError::LifecycleInvariant(format!(
                    "generated approval lifecycle rejected {approval_id} with InvalidDecision but no decision context"
                ));
            };
            ApprovalError::InvalidDecision { decision }
        }
        ApprovalLifecycleRejectionReason::EmptyAllowedDecisions => {
            ApprovalError::EmptyAllowedDecisions
        }
        ApprovalLifecycleRejectionReason::AuthenticationRequired => {
            ApprovalError::AuthenticationRequired {
                approval_id: approval_id.clone(),
            }
        }
        // The approval and its live attempt are no longer linked: the
        // consent was spent, withdrawn or declined, so none is available.
        ApprovalLifecycleRejectionReason::ConsentMismatch
        | ApprovalLifecycleRejectionReason::AlreadyConsumed
        | ApprovalLifecycleRejectionReason::NotConsent => ApprovalError::ConsentUnavailable {
            approval_id: approval_id.clone(),
        },
        ApprovalLifecycleRejectionReason::InvalidRestoredRecord => {
            ApprovalError::InvalidStoredRecord {
                approval_id: approval_id.clone(),
            }
        }
        ApprovalLifecycleRejectionReason::AlreadyExists
        | ApprovalLifecycleRejectionReason::ConsentLinked
        | ApprovalLifecycleRejectionReason::ReviewRetired
        | ApprovalLifecycleRejectionReason::ReviewNotSatisfied
        | ApprovalLifecycleRejectionReason::ReviewPending => {
            ApprovalError::LifecycleInvariant(format!(
                "generated approval lifecycle authority rejected {approval_id} with {reason:?}"
            ))
        }
    }
}

fn lifecycle_status_from_outcome(
    approval_id: &ApprovalId,
    outcome: ApprovalLifecycleOutcome,
    decision: Option<ApprovalDecision>,
) -> Result<ApprovalStatus, ApprovalError> {
    match outcome {
        ApprovalLifecycleOutcome::Status(status) => Ok(approval_status_from_lifecycle(status)),
        ApprovalLifecycleOutcome::Rejected(reason) => {
            Err(lifecycle_rejection_error(approval_id, reason, decision))
        }
        ApprovalLifecycleOutcome::ReviewStatus(status) => {
            Err(ApprovalError::LifecycleInvariant(format!(
                "generated approval lifecycle emitted review status {status:?} for approval {approval_id}"
            )))
        }
    }
}

fn review_status_from_outcome(
    outcome: Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
    match outcome {
        Ok(ApprovalLifecycleOutcome::ReviewStatus(status)) => Ok(status),
        Ok(ApprovalLifecycleOutcome::Rejected(reason)) => Err(ReviewOwnerError::Rejected(reason)),
        Ok(ApprovalLifecycleOutcome::Status(_)) | Err(_) => Err(ReviewOwnerError::Unavailable),
    }
}

fn lifecycle_error(error: impl std::fmt::Display) -> ApprovalError {
    ApprovalError::LifecycleInvariant(format!(
        "generated approval lifecycle authority failed: {error}"
    ))
}

fn restored_lifecycle_decision(record: &ApprovalRecord) -> Option<ApprovalLifecycleDecision> {
    record
        .decision
        .as_ref()
        .map(|decision| approval_lifecycle_decision(decision.decision))
}

/// Consent facts of a stored row for the generated owner: (consent,
/// authenticated, consumed), after validating the history the flags stand
/// for. An approver must be a qualified human whose principal is the
/// recorded decision's actor; a withdrawal is never on spent consent.
fn stored_consent_flags(record: &ApprovalRecord) -> Result<(bool, bool, bool), ApprovalError> {
    let invalid = || ApprovalError::InvalidStoredRecord {
        approval_id: record.approval_id.clone(),
    };
    let Some(consent) = &record.consent else {
        return Ok((false, false, false));
    };
    if let Some(approver) = &consent.approver {
        let decided_by_approver = record
            .decision
            .as_ref()
            .is_some_and(|decision| decision.actor.as_str() == approver.principal.id.as_str());
        if approver.principal.kind != crate::PrincipalKind::Human
            || approver.principal.validate_qualified().is_err()
            || !decided_by_approver
        {
            return Err(invalid());
        }
    }
    let consumed = consent.consumed_at.is_some();
    if consent.withdrawal.is_some() && consumed {
        return Err(invalid());
    }
    if record.status == ApprovalStatus::Cancelled && consent.withdrawal.is_none() {
        return Err(invalid());
    }
    Ok((true, consent.approver.is_some(), consumed))
}

/// Generated restore of one stored row as history: never linked, never
/// executable. The owner rejects contradictory status, decision audit and
/// consent history.
fn restore_record_into_authority(
    authority: &mut ApprovalLifecycleMachineAuthority,
    record: &ApprovalRecord,
) -> Result<(), ApprovalError> {
    let (consent, authenticated, consumed) = stored_consent_flags(record)?;
    let (approve_allowed, deny_allowed) = allowed_decision_flags(&record.allowed_decisions);
    let outcome = authority
        .restore_approval(
            record.approval_id.to_string(),
            approval_lifecycle_status(record.status),
            approve_allowed,
            deny_allowed,
            record.expires_at.is_some(),
            restored_lifecycle_decision(record),
            consent,
            authenticated,
            consumed,
        )
        .map_err(lifecycle_error)?;
    restored_status_matches(record, outcome)
}

fn restored_status_matches(
    record: &ApprovalRecord,
    outcome: ApprovalLifecycleOutcome,
) -> Result<(), ApprovalError> {
    match outcome {
        ApprovalLifecycleOutcome::Status(status)
            if approval_status_from_lifecycle(status) == record.status =>
        {
            Ok(())
        }
        // Contradictory history or a duplicate stored row.
        ApprovalLifecycleOutcome::Rejected(
            ApprovalLifecycleRejectionReason::InvalidRestoredRecord
            | ApprovalLifecycleRejectionReason::AlreadyExists
            | ApprovalLifecycleRejectionReason::EmptyAllowedDecisions,
        ) => Err(ApprovalError::InvalidStoredRecord {
            approval_id: record.approval_id.clone(),
        }),
        other => Err(ApprovalError::LifecycleInvariant(format!(
            "generated approval lifecycle restored {} as {other:?}, stored {:?}",
            record.approval_id, record.status
        ))),
    }
}

/// In-process approval service.
#[derive(Clone)]
pub struct ApprovalService {
    state: Arc<RwLock<ApprovalServiceState>>,
    store: Arc<dyn ApprovalStore>,
    unavailable_reason: Option<Arc<str>>,
    /// Review inputs handed over without waiting (attempt disposals and the
    /// finish of an entered effect's step), settled in order by the owner's
    /// next holder. Never held while waiting for `state`.
    queued_disposals: Arc<parking_lot::Mutex<Vec<QueuedReviewDisposal>>>,
    /// Work associations whose run closed while the owner was held; their
    /// unspent consent is parked or withdrawn by the next lock holder's
    /// release.
    queued_withdrawals:
        Arc<parking_lot::Mutex<Vec<crate::authorization::WorkAuthorizationContext>>>,
    /// Set while a withdrawal's custody is retired, or a park kept, but its
    /// durable record is still pending (a store-free release left it).
    withdrawals_pending: Arc<std::sync::atomic::AtomicBool>,
    /// Advances after every committed consent notice obligation and every
    /// recorded park ([`ApprovalService::subscribe_consent_deliveries`]).
    consent_delivery_changes: Arc<tokio::sync::watch::Sender<u64>>,
    /// Test-only wall clock offset, in seconds.
    #[cfg(test)]
    clock_skew: Arc<std::sync::atomic::AtomicI64>,
    #[cfg(test)]
    reservation_probe: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test-only count of review-path acquisitions of `state`.
    #[cfg(test)]
    review_lock_acquisitions: Arc<std::sync::atomic::AtomicUsize>,
}

impl ApprovalService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: None,
            queued_disposals: Arc::default(),
            queued_withdrawals: Arc::default(),
            withdrawals_pending: Arc::default(),
            consent_delivery_changes: Arc::new(tokio::sync::watch::Sender::new(0)),
            #[cfg(test)]
            clock_skew: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        }
    }

    pub fn with_store(store: Arc<dyn ApprovalStore>) -> Result<Self, ApprovalError> {
        let records = store
            .load_all()
            .map_err(|error| ApprovalError::StoreUnavailable(error.to_string()))?;
        let state = ApprovalServiceState::from_records(records)?;
        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            store,
            unavailable_reason: None,
            queued_disposals: Arc::default(),
            queued_withdrawals: Arc::default(),
            withdrawals_pending: Arc::default(),
            consent_delivery_changes: Arc::new(tokio::sync::watch::Sender::new(0)),
            #[cfg(test)]
            clock_skew: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        })
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            state: Arc::new(RwLock::new(ApprovalServiceState::empty())),
            store: Arc::new(InMemoryApprovalStore::new()),
            unavailable_reason: Some(Arc::from(reason.into())),
            queued_disposals: Arc::default(),
            queued_withdrawals: Arc::default(),
            withdrawals_pending: Arc::default(),
            consent_delivery_changes: Arc::new(tokio::sync::watch::Sender::new(0)),
            #[cfg(test)]
            clock_skew: Arc::default(),
            #[cfg(test)]
            reservation_probe: None,
            #[cfg(test)]
            review_lock_acquisitions: Arc::default(),
        }
    }

    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.store.is_persistent()
    }

    /// The owner's wall clock for record expiry.
    pub(super) fn now(&self) -> DateTime<Utc> {
        let now = Utc::now();
        #[cfg(test)]
        let now = now
            + chrono::Duration::seconds(self.clock_skew.load(std::sync::atomic::Ordering::SeqCst));
        now
    }

    fn ensure_available(&self) -> Result<(), ApprovalError> {
        if let Some(reason) = &self.unavailable_reason {
            return Err(ApprovalError::StoreUnavailable(format!(
                "approval service unavailable: {reason}"
            )));
        }
        Ok(())
    }

    pub fn request(&self, request: ApprovalRequest) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        if request.requester.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        request.metadata.validate_public()?;
        let now = Utc::now();
        let approval_id = ApprovalId::new();
        let mut state = self.state.write();
        let result = self.request_locked(&mut state, request, approval_id, now);
        // Settles queued review disposals as the lock is released.
        self.release_state(state).run();
        result
    }

    fn request_locked(
        &self,
        state: &mut ApprovalServiceState,
        request: ApprovalRequest,
        approval_id: ApprovalId,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let (approve_allowed, deny_allowed) = allowed_decision_flags(&request.allowed_decisions);
        let mut authority = state.authority.clone();
        let outcome = authority
            .create_approval(
                approval_id.to_string(),
                approve_allowed,
                deny_allowed,
                request.expires_at.is_some(),
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(&approval_id, outcome, None)?;
        let record = ApprovalRecord {
            approval_id,
            status,
            requester: request.requester,
            owner: request.owner,
            resource: request.resource,
            proposed_action: request.proposed_action,
            risk: request.risk,
            request_body: request.request_body,
            allowed_decisions: request.allowed_decisions,
            expires_at: request.expires_at,
            created_at: now,
            updated_at: now,
            metadata: request.metadata,
            request_provenance: request.request_provenance,
            decision: None,
            consent: None,
        };
        self.commit_row_locked(state, None, &record)?;
        state.authority = authority;
        state
            .records
            .insert(record.approval_id.clone(), record.clone());
        Ok(record)
    }

    pub fn get(&self, approval_id: &ApprovalId) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        self.reconcile_if_unreconciled(approval_id)?;
        self.refresh_expiry(approval_id)?;
        // The row and its reconciliation mark are read in one reservation: a
        // row marked since the refresh is never returned as current.
        let record = {
            let state = self.state.read();
            state
                .ensure_reconciled(approval_id)
                .map(|()| state.records.get(approval_id).cloned())
        };
        // A reader may have made a disposer's try-lock fail; settle after it.
        self.settle_owner_now().run();
        record?.ok_or_else(|| ApprovalError::NotFound {
            approval_id: approval_id.clone(),
        })
    }

    pub fn list(&self, filter: ApprovalListFilter) -> Result<Vec<ApprovalRecord>, ApprovalError> {
        self.ensure_available()?;
        let unreconciled: Vec<ApprovalId> =
            self.state.read().unreconciled.keys().cloned().collect();
        self.settle_owner_now().run();
        for approval_id in unreconciled {
            self.reconcile(&approval_id)?;
        }
        self.refresh_all_expiry()?;
        let records = {
            let state = self.state.read();
            match state.unreconciled.keys().next() {
                Some(approval_id) => Err(ApprovalError::Unreconciled {
                    approval_id: approval_id.clone(),
                }),
                None => Ok(state
                    .records
                    .values()
                    .filter(|record| filter.status.is_none_or(|status| record.status == status))
                    .cloned()
                    .collect()),
            }
        };
        self.settle_owner_now().run();
        records
    }

    pub fn decide(
        &self,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        actor: ApprovalPrincipalId,
        reason: Option<String>,
        provenance: Option<serde_json::Value>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        if actor.as_str().trim().is_empty() {
            return Err(ApprovalError::InvalidPrincipal);
        }
        self.reconcile_if_unreconciled(approval_id)?;
        let now = Utc::now();
        let mut state = self.state.write();
        let result = self.decide_locked(
            &mut state,
            approval_id,
            decision,
            actor,
            reason,
            provenance,
            now,
        );
        // Settles review disposals queued before or during this commit as
        // the lock is released, so none waits for a later owner call.
        self.release_state(state).run();
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn decide_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        decision: ApprovalDecision,
        actor: ApprovalPrincipalId,
        reason: Option<String>,
        provenance: Option<serde_json::Value>,
        now: DateTime<Utc>,
    ) -> Result<ApprovalRecord, ApprovalError> {
        if !state.records.contains_key(approval_id) {
            let mut authority = state.authority.clone();
            let outcome = authority
                .decide_approval(
                    approval_id.to_string(),
                    approval_lifecycle_decision(decision),
                )
                .map_err(lifecycle_error)?;
            return match outcome {
                ApprovalLifecycleOutcome::Rejected(reason) => Err(lifecycle_rejection_error(
                    approval_id,
                    reason,
                    Some(decision),
                )),
                ApprovalLifecycleOutcome::Status(status) => {
                    Err(ApprovalError::LifecycleInvariant(format!(
                        "generated approval lifecycle emitted {status:?} for missing approval {approval_id}"
                    )))
                }
                ApprovalLifecycleOutcome::ReviewStatus(status) => {
                    Err(ApprovalError::LifecycleInvariant(format!(
                        "generated approval lifecycle emitted review status {status:?} for missing approval {approval_id}"
                    )))
                }
            };
        }

        self.refresh_expiry_locked(state, approval_id, now)?;
        let record =
            state
                .records
                .get(approval_id)
                .cloned()
                .ok_or_else(|| ApprovalError::NotFound {
                    approval_id: approval_id.clone(),
                })?;

        // Caller-supplied actor text is not authentication: the generated
        // owner refuses it for a consent approval (`decide_consent` is the
        // authenticated path).
        let mut authority = state.authority.clone();
        let outcome = authority
            .decide_approval(
                approval_id.to_string(),
                approval_lifecycle_decision(decision),
            )
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(approval_id, outcome, Some(decision))?;

        let mut decided_record = record.clone();
        decided_record.status = status;
        decided_record.updated_at = now;
        decided_record.decision = Some(ApprovalDecisionRecord {
            decision,
            actor,
            decided_at: now,
            reason,
            provenance,
        });
        self.commit_row_locked(state, Some(&record), &decided_record)?;
        state.authority = authority;
        state
            .records
            .insert(approval_id.clone(), decided_record.clone());
        Ok(decided_record)
    }

    /// Test adapter: apply one raw review input in place under an unbounded
    /// lock. Production review inputs go through the non-blocking reservation or
    /// the non-blocking disposal. Review inputs never touch the store: review
    /// attempts are memory-only by contract.
    #[cfg(test)]
    fn apply_review(
        &self,
        apply: impl FnOnce(
            &mut ApprovalLifecycleMachineAuthority,
        ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.ensure_available()
            .map_err(|_| ReviewOwnerError::Unavailable)?;
        let mut state = self.state.write();
        self.count_review_lock();
        let result = review_status_from_outcome(apply(&mut state.authority));
        self.release_review_state(state).run();
        result
    }

    /// Commit one review input that admits an effect (a verdict, or the spend
    /// at entry) under ONE commit reservation, the same state lock `decide`
    /// and expiry hold through `ApprovalStore::put_if_current`:
    ///
    /// 0. first settle already queued disposals and carry their reports
    ///    back to the caller (they run arbitrary observers, so the caller
    ///    delivers them only outside its locks, and for a spend after the
    ///    effect);
    /// 1. take the reservation WITHOUT waiting (callers run on async
    ///    threads; a timed wait would still block them): contention refuses
    ///    locally at once and queues this attempt's disposal;
    /// 2. under it, check the attempt's identity binding and run
    ///    `final_check` (deadline and currentness) immediately before the
    ///    conditional commit;
    /// 3. commit; a refusal retires and disposes the attempt in the same
    ///    reservation. A spend leaves the attempt registered as `Used`, the
    ///    entered state of its effect: the admitting dispatch's end releases
    ///    it ([`Self::dispose_review`], never waiting), and until then the
    ///    effect's later steps are admitted against it (the generated
    ///    `AdmitEnteredStep`). Nothing waits on this owner and no queued
    ///    report runs between the spend and the leaf's effect.
    ///
    /// On refusal the returned disposal is the owner's settled result, or
    /// `None` when it was queued (`on_queued` then reports it once settled).
    /// `final_check` runs under the reservation and must not re-enter this
    /// owner; policy owners never consult approval state.
    #[allow(clippy::too_many_arguments)]
    fn commit_review_reserved<E>(
        &self,
        handle: &ReviewAttemptHandle,
        bound: Option<(
            &crate::authorization::PreparedOperationCheck,
            &crate::authorization::WorkAuthorizationContext,
        )>,
        final_check: impl Fn() -> Result<(), E>,
        commit: impl FnOnce(
            &mut ApprovalLifecycleMachineAuthority,
        ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError>,
        spend: bool,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        if self.ensure_available().is_err() {
            return Err(RefusedReviewCommit {
                error: ReservedReviewError::Owner(ReviewOwnerError::Unavailable),
                disposal: None,
                reports: SettledReports::default(),
                committed: None,
            });
        }
        // Settle queued disposals before the reservation and final check;
        // their reports go back to the caller, which runs them only after it
        // released its own locks (and, for a spend, after the effect).
        let mut reports = self.settle_queued_disposals_now();
        #[cfg(test)]
        if let Some(probe) = &self.reservation_probe {
            probe();
        }
        let Some(mut state) = self.state.try_write() else {
            let error = ReservedReviewError::Contended;
            let reason = retirement(&error);
            let (disposal, more) = self.dispose_review(handle.id(), Some(reason), on_queued);
            reports.absorb(more);
            return Err(RefusedReviewCommit {
                error,
                disposal,
                reports,
                committed: None,
            });
        };
        self.count_review_lock();
        let refused = if bound.is_some_and(|(check, work)| !handle.bound_to(check.binding(), work))
        {
            Some(ReservedReviewError::Owner(ReviewOwnerError::Mismatch))
        } else if let Err(failure) = final_check() {
            Some(ReservedReviewError::FinalCheck(failure))
        } else {
            None
        };
        let result = match refused {
            Some(error) => Err(error),
            // A spend keeps the attempt registered (`Used`) for its effect's
            // later steps; the admitting dispatch's end releases it.
            None => review_status_from_outcome(commit(&mut state.authority))
                .map_err(ReservedReviewError::Owner),
        };
        let result = result.map_err(|error| {
            let reason = retirement(&error);
            let disposal = dispose_locked(&mut state.authority, handle.id(), Some(reason));
            RefusedReviewCommit {
                error,
                disposal: Some(disposal),
                reports: SettledReports::default(),
                committed: None,
            }
        });
        // Disposals queued during this commit are settled as the lock is
        // released (in memory only: review paths make no store call). Every
        // report goes back to the caller, which runs them only after
        // releasing its own locks: after the effect for a spend, otherwise at
        // once.
        reports.absorb(self.release_review_state(state));
        match result {
            // The spend's final check comes after every owner-exit step: the
            // callback-free retained check runs again here, immediately
            // before the effect handoff. A refusal now has zero effect; the
            // committed spend stays spent (no refund).
            Ok(status) if spend => match final_check() {
                Ok(()) => Ok((status, reports)),
                Err(failure) => Err(RefusedReviewCommit {
                    error: ReservedReviewError::FinalCheck(failure),
                    disposal: None,
                    reports,
                    committed: Some(status),
                }),
            },
            Ok(status) => Ok((status, reports)),
            Err(mut refused) => {
                refused.reports = reports;
                Err(refused)
            }
        }
    }

    /// Settle and report queued disposals now if the owner is free, without
    /// waiting; reports run after the lock is released.
    #[must_use = "run the reports once outside every lock"]
    fn settle_queued_disposals_now(&self) -> SettledReports {
        if self.queued_disposals.lock().is_empty() && self.queued_withdrawals.lock().is_empty() {
            return SettledReports::default();
        }
        match self.state.try_write() {
            Some(state) => {
                self.count_review_lock();
                self.release_review_state(state)
            }
            None => SettledReports::default(),
        }
    }

    /// Settle everything queued or pending from a store-capable path, now,
    /// without waiting: queued disposals, run withdrawals, and withdrawals
    /// whose durable record a store-free release left. When the owner is
    /// held, its holder settles on release (or carries the duty on).
    #[must_use = "run the reports once outside every lock"]
    pub(crate) fn settle_owner_now(&self) -> SettledReports {
        if self.queued_disposals.lock().is_empty()
            && self.queued_withdrawals.lock().is_empty()
            && !self
                .withdrawals_pending
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return SettledReports::default();
        }
        match self.state.try_write() {
            Some(state) => {
                self.count_review_lock();
                self.release_state(state)
            }
            None => SettledReports::default(),
        }
    }

    /// Retire (when `retire` is set) and dispose an attempt WITHOUT waiting
    /// for the owner: settled now when the state lock is free (the result is
    /// returned), otherwise queued for the owner's next commit and reported
    /// through `on_queued` once settled (`None` is returned). Reports of
    /// other disposals settled here are returned, never run: the caller may
    /// hold its own locks and runs them after releasing those.
    pub(crate) fn dispose_review(
        &self,
        id: &str,
        retire: Option<ReviewRetirementReason>,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> (
        Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
        SettledReports,
    ) {
        self.settle_review_input(id, QueuedReviewInput::Dispose(retire), on_queued)
    }

    /// Admit one later step of an entered effect (`AdmitEnteredStep`) under
    /// the owner's non-waiting reservation: a held owner is `Unavailable` at
    /// once. Under that same reservation every review input queued behind
    /// an earlier holder (a dispatch end's hand-back, a step's finish) is
    /// settled into the owner first, so the generated owner alone decides
    /// the admission: a step is admitted only while the attempt, spent once
    /// at the effect's entry, is registered as `Used` and its effect is
    /// open. Admitting spends nothing. `handle` must be bound to exactly
    /// `binding` and `work`. The returned reports run once the caller is
    /// outside every lock and outside the step.
    pub(crate) fn admit_entered_step(
        &self,
        handle: &ReviewAttemptHandle,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &crate::authorization::WorkAuthorizationContext,
    ) -> (Result<(), ReviewOwnerError>, SettledReports) {
        if self.ensure_available().is_err() {
            return (
                Err(ReviewOwnerError::Unavailable),
                SettledReports::default(),
            );
        }
        let Some(mut state) = self.state.try_write() else {
            return (
                Err(ReviewOwnerError::Unavailable),
                SettledReports::default(),
            );
        };
        self.count_review_lock();
        let mut reports = self.settle_queued_locked(&mut state);
        let result = if handle.bound_to(binding, work) {
            match review_status_from_outcome(
                state.authority.admit_entered_step(handle.id().to_owned()),
            ) {
                Ok(ReviewAttemptStatus::Used) => Ok(()),
                Ok(_) => Err(ReviewOwnerError::Unavailable),
                Err(error) => Err(error),
            }
        } else {
            Err(ReviewOwnerError::Mismatch)
        };
        reports.absorb(self.release_review_state(state));
        (result, reports)
    }

    /// One admitted step of an entered effect finished (`FinishEnteredStep`),
    /// WITHOUT waiting for the owner: applied now when the owner is free,
    /// otherwise queued for its next holder through the same protocol as a
    /// disposal, so a finish is never lost. The last finish of a closing
    /// effect releases its attempt. The returned reports run once the caller
    /// is outside every lock and outside the step.
    pub(crate) fn finish_entered_step(&self, id: &str) -> SettledReports {
        let (_, reports) =
            self.settle_review_input(id, QueuedReviewInput::FinishEnteredStep, || {
                Box::new(|_: Result<ReviewAttemptStatus, ReviewOwnerError>| {})
            });
        reports
    }

    /// Apply one review input WITHOUT waiting for the owner: now when the
    /// state lock is free (the result is returned), otherwise queued for the
    /// owner's next holder and reported through `on_queued` once settled
    /// (`None` is returned).
    fn settle_review_input(
        &self,
        id: &str,
        input: QueuedReviewInput,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> (
        Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
        SettledReports,
    ) {
        if self.ensure_available().is_err() {
            return (
                Some(Err(ReviewOwnerError::Unavailable)),
                SettledReports::default(),
            );
        }
        if let Some(mut state) = self.state.try_write() {
            self.count_review_lock();
            let result = apply_queued_locked(&mut state.authority, id, input);
            return (Some(result), self.release_review_state(state));
        }
        self.queued_disposals.lock().push(QueuedReviewDisposal {
            id: id.to_owned(),
            input,
            report: on_queued(),
        });
        // Every lock holder settles the queue as it releases (writers under
        // the queue mutex, readers right after), so an entry pushed while the
        // lock was held is never stranded. If the holder already released,
        // settle now, still without waiting.
        match self.state.try_write() {
            Some(state) => {
                self.count_review_lock();
                (None, self.release_review_state(state))
            }
            None => (None, SettledReports::default()),
        }
    }

    /// Release the state lock from a store-capable owner path: queued run
    /// withdrawals are retired and recorded durably, then queued disposals
    /// settle (see [`Self::release_with`]).
    fn release_state(
        &self,
        state: parking_lot::RwLockWriteGuard<'_, ApprovalServiceState>,
    ) -> SettledReports {
        self.release_with(state, true)
    }

    /// Release the state lock from a review path, which makes no store call:
    /// queued run withdrawals only retire their custody here (in memory);
    /// their durable record and notice wait for a store-capable holder.
    fn release_review_state(
        &self,
        state: parking_lot::RwLockWriteGuard<'_, ApprovalServiceState>,
    ) -> SettledReports {
        self.release_with(state, false)
    }

    /// Release the state lock. Queued run withdrawals are drained in rounds:
    /// their custody is retired in memory and, from a store-capable path,
    /// recorded durably with no queue mutex held across that store or
    /// observer work. The last round is decided UNDER the withdrawal and
    /// disposal queue mutexes, and the state is unlocked while they are
    /// still held: a pusher that failed its try-lock pushed under one of
    /// them, so its entry is either drained here or pushed after the unlock
    /// (when its own retry can take the lock). No entry is lost between the
    /// last drain and the unlock. The reports run later, outside every lock.
    fn release_with(
        &self,
        mut state: parking_lot::RwLockWriteGuard<'_, ApprovalServiceState>,
        durable: bool,
    ) -> SettledReports {
        loop {
            if durable {
                self.record_pending_withdrawals_locked(&mut state);
            }
            let mut withdrawals = self.queued_withdrawals.lock();
            if withdrawals.is_empty() {
                let mut queue = self.queued_disposals.lock();
                let settled = std::mem::take(&mut *queue)
                    .into_iter()
                    .map(|queued| {
                        let result =
                            apply_queued_locked(&mut state.authority, &queued.id, queued.input);
                        (queued.report, result)
                    })
                    .collect();
                // A store-free release never drops a pending durable record:
                // it hands the settlement duty to its reports.
                let duty = !durable
                    && (!state.pending_withdrawals.is_empty() || !state.pending_parks.is_empty());
                drop(state);
                drop(queue);
                drop(withdrawals);
                let reports = SettledReports(settled);
                return if duty {
                    reports.with_settlement_duty(self.clone())
                } else {
                    reports
                };
            }
            let works = std::mem::take(&mut *withdrawals);
            drop(withdrawals);
            for work in works {
                self.retire_work_custody_locked(&mut state, &work);
            }
        }
    }

    /// Settle every queued review input into the owner now, in queue order,
    /// under the reservation the caller holds (the state lock stays held).
    /// Their reports run later, outside every lock.
    fn settle_queued_locked(&self, state: &mut ApprovalServiceState) -> SettledReports {
        let queued = std::mem::take(&mut *self.queued_disposals.lock());
        SettledReports(
            queued
                .into_iter()
                .map(|queued| {
                    let result =
                        apply_queued_locked(&mut state.authority, &queued.id, queued.input);
                    (queued.report, result)
                })
                .collect(),
        )
    }

    #[inline]
    fn count_review_lock(&self) {
        #[cfg(test)]
        self.review_lock_acquisitions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn with_reservation_probe(mut self, probe: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.reservation_probe = Some(probe);
        self
    }

    /// Test-only: whether a writer holds the owner's state now (a review
    /// input under its reservation, or a decision).
    #[cfg(test)]
    pub(crate) fn state_write_held(&self) -> bool {
        self.state.is_locked_exclusive()
    }

    /// Test-only: read the owner's state as a concurrent approval read
    /// (`get`, `list`) does, on the calling thread: report the read through
    /// `held` once it holds it, and hold it until `release` answers or its
    /// sender is dropped.
    #[cfg(test)]
    pub(crate) fn read_state_until(
        &self,
        held: &std::sync::mpsc::Sender<()>,
        release: &std::sync::mpsc::Receiver<()>,
    ) {
        let _read = self.state.read();
        let _ = held.send(());
        let _ = release.recv();
    }

    /// Issue a fresh, never-reused attempt bound to the exact operation and
    /// its original admitted work, through the same non-blocking reservation:
    /// a held owner refuses locally (`Contended`) at once with no attempt
    /// created, so nothing needs cleanup and no async thread is blocked.
    pub(crate) fn try_begin_review(
        &self,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &crate::authorization::WorkAuthorizationContext,
    ) -> Result<ReviewAttemptHandle, ReservedReviewError<std::convert::Infallible>> {
        self.ensure_available()
            .map_err(|_| ReservedReviewError::Owner(ReviewOwnerError::Unavailable))?;
        let id: Arc<str> = Arc::from(crate::time_compat::new_uuid_v7().to_string());
        let Some(mut state) = self.state.try_write() else {
            return Err(ReservedReviewError::Contended);
        };
        self.count_review_lock();
        let status = review_status_from_outcome(state.authority.begin_review(id.to_string()));
        self.release_review_state(state).run();
        match status {
            Ok(ReviewAttemptStatus::Pending) => {
                Ok(ReviewAttemptHandle::new(id, binding.clone(), work.clone()))
            }
            Ok(_) => Err(ReservedReviewError::Owner(ReviewOwnerError::Unavailable)),
            Err(error) => Err(ReservedReviewError::Owner(error)),
        }
    }

    /// Owner-transition tests: begin on an uncontended owner.
    #[cfg(test)]
    pub(crate) fn begin_review(
        &self,
        binding: &crate::authorization::PreparedAuthorizationBinding,
        work: &crate::authorization::WorkAuthorizationContext,
    ) -> Result<ReviewAttemptHandle, ReviewOwnerError> {
        self.try_begin_review(binding, work)
            .map_err(|error| match error {
                ReservedReviewError::Owner(error) => error,
                _ => ReviewOwnerError::Unavailable,
            })
    }

    /// Accept a verdict under the single commit reservation, after
    /// `final_check`; a refusal retires and disposes the attempt there.
    pub(crate) fn record_review_verdict<E>(
        &self,
        handle: &ReviewAttemptHandle,
        verdict: ReviewVerdict,
        final_check: impl Fn() -> Result<(), E>,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        self.commit_review_reserved(
            handle,
            None,
            final_check,
            |authority| authority.record_review_verdict(handle.id().to_owned(), verdict),
            false,
            retirement,
            on_queued,
        )
    }

    /// Record a reviewer failure under the non-blocking commit reservation;
    /// this is a refusal path, so it never waits on the owner.
    pub(crate) fn record_review_unavailable(
        &self,
        handle: &ReviewAttemptHandle,
        retirement: impl FnOnce(
            &ReservedReviewError<std::convert::Infallible>,
        ) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<std::convert::Infallible>>
    {
        self.commit_review_reserved(
            handle,
            None,
            || Ok(()),
            |authority| authority.record_review_unavailable(handle.id().to_owned()),
            false,
            retirement,
            on_queued,
        )
    }

    #[cfg(test)]
    pub(crate) fn retire_review(
        &self,
        handle: &ReviewAttemptHandle,
        reason: ReviewRetirementReason,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.apply_review(|authority| authority.retire_review(handle.id().to_owned(), reason))
    }

    /// Spend the allow once, only for the exact operation binding and work
    /// association the attempt was issued for, under the single commit
    /// reservation after `final_check`. The attempt stays registered as
    /// `Used`, the entered state of its effect, until its admitting dispatch
    /// ends; a spend refused by its final check after the commit is released
    /// by the caller at once, since nothing entered.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn consume_review_for_entry<E>(
        &self,
        handle: &ReviewAttemptHandle,
        check: &crate::authorization::PreparedOperationCheck,
        work: &crate::authorization::WorkAuthorizationContext,
        final_check: impl Fn() -> Result<(), E>,
        retirement: impl FnOnce(&ReservedReviewError<E>) -> ReviewRetirementReason,
        on_queued: impl FnOnce() -> QueuedDisposalReport,
    ) -> Result<(ReviewAttemptStatus, SettledReports), RefusedReviewCommit<E>> {
        self.commit_review_reserved(
            handle,
            Some((check, work)),
            final_check,
            |authority| authority.consume_review_for_entry(handle.id().to_owned()),
            true,
            retirement,
            on_queued,
        )
    }

    /// Dispose a settled attempt. A pending attempt must be retired first.
    #[cfg(test)]
    pub(crate) fn release_review(
        &self,
        handle: ReviewAttemptHandle,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        self.apply_review(|authority| authority.release_review(handle.id().to_owned()))
    }

    /// The conditional store write of one row, under the owner's
    /// reservation, before anything is published. A conflicting or uncertain
    /// write leaves the stored row unknown: the row is marked for
    /// reconciliation and its live consent custody is retired (zero entry,
    /// no refund, no automatic resend). A proven no-commit changes nothing.
    fn commit_row_locked(
        &self,
        state: &mut ApprovalServiceState,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), ApprovalError> {
        let Err(error) = self.store.put_if_current(expected, record) else {
            return Ok(());
        };
        if error.leaves_row_unknown() {
            state.mark_unreconciled(&record.approval_id);
            self.abandon_consent_locked(state, &record.approval_id);
        }
        Err(error.into_commit_error(&record.approval_id))
    }

    /// Reconcile one row with its store through the same generated owner,
    /// before it is reused. The stored row is read outside the owner's
    /// reservation and published under it only if the row is still marked
    /// exactly as it was before the read. A missing, unreadable or invalid
    /// stored row stays unavailable; reconciled history never recreates a
    /// binding or executable consent, and never undoes a spend.
    pub fn reconcile(&self, approval_id: &ApprovalId) -> Result<ApprovalRecord, ApprovalError> {
        self.ensure_available()?;
        let (mark, known) = {
            let state = self.state.read();
            (
                state.unreconciled.get(approval_id).copied(),
                state.records.get(approval_id).cloned(),
            )
        };
        self.settle_owner_now().run();
        let Some(mark) = mark else {
            return known.ok_or_else(|| ApprovalError::NotFound {
                approval_id: approval_id.clone(),
            });
        };
        let stored = self
            .store
            .get(approval_id)
            .map_err(|error| ApprovalError::StoreUnavailable(error.to_string()))?
            .filter(|stored| stored.approval_id == *approval_id)
            .ok_or_else(|| ApprovalError::Unreconciled {
                approval_id: approval_id.clone(),
            })?;
        let mut state = self.state.write();
        let result =
            Self::reconcile_locked(&mut state, approval_id, mark, stored).and_then(|stored| {
                // A withdrawal an unknown write left owed settles against the
                // confirmed row under this reservation; when its own write
                // fails, the row is unknown again and so reported.
                self.settle_reconciled_withdrawal_locked(&mut state, approval_id)?;
                Ok(state.records.get(approval_id).cloned().unwrap_or(stored))
            });
        self.release_state(state).run();
        result
    }

    fn reconcile_locked(
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        mark: u64,
        stored: ApprovalRecord,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let unreconciled = || ApprovalError::Unreconciled {
            approval_id: approval_id.clone(),
        };
        match state.unreconciled.get(approval_id) {
            Some(current) if *current == mark => {}
            // Marked again while the store was read: that read is stale.
            Some(_) => return Err(unreconciled()),
            None => {
                return state
                    .records
                    .get(approval_id)
                    .cloned()
                    .ok_or_else(unreconciled);
            }
        }
        let (consent, authenticated, consumed) =
            stored_consent_flags(&stored).map_err(|_| unreconciled())?;
        let (approve_allowed, deny_allowed) = allowed_decision_flags(&stored.allowed_decisions);
        let mut authority = state.authority.clone();
        let id = approval_id.to_string();
        let status = approval_lifecycle_status(stored.status);
        let decision = restored_lifecycle_decision(&stored);
        let has_expiry = stored.expires_at.is_some();
        // A row the generated owner never committed (an uncertain create) is
        // restored as history; a known row is reconciled in place.
        let outcome = if authority.status_for(&id).is_some() {
            authority.reconcile_approval(
                id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision,
                consent,
                authenticated,
                consumed,
            )
        } else {
            authority.restore_approval(
                id,
                status,
                approve_allowed,
                deny_allowed,
                has_expiry,
                decision,
                consent,
                authenticated,
                consumed,
            )
        }
        .map_err(lifecycle_error)?;
        restored_status_matches(&stored, outcome).map_err(|_| unreconciled())?;
        state.authority = authority;
        state.records.insert(approval_id.clone(), stored.clone());
        state.unreconciled.remove(approval_id);
        Ok(stored)
    }

    /// Reconcile `approval_id` first when its stored row is unknown.
    fn reconcile_if_unreconciled(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        let marked = self.state.read().unreconciled.contains_key(approval_id);
        self.settle_owner_now().run();
        if marked {
            self.reconcile(approval_id)?;
        }
        Ok(())
    }

    fn refresh_expiry(&self, approval_id: &ApprovalId) -> Result<(), ApprovalError> {
        // A live consent candidate whose decision an owner publication change
        // made stale is re-adjudicated first, outside the reservation, on
        // this caller's thread (ADR-001 D9). One its own check refuses, or
        // answers that its owners are unavailable, is left for an approval
        // or an attempt: a polling read never calls its owners for it. A
        // read reports what it left.
        let _ = self.readjudicate_consent(approval_id, consent::ReadjudicationStep::Read);
        let now = self.now();
        let mut state = self.state.write();
        let result = self.refresh_expiry_locked(&mut state, approval_id, now);
        // An expiry put is an owner commit too: settle queued disposals.
        self.release_state(state).run();
        result
    }

    fn refresh_expiry_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        now: DateTime<Utc>,
    ) -> Result<(), ApprovalError> {
        self.observe_expiry_locked(state, approval_id, now, false)
    }

    /// Observe expiry at `now`, or earlier when the caller already observed
    /// a passed deadline. A consent approval that expires retires its
    /// retained candidate in the same reservation.
    fn observe_expiry_locked(
        &self,
        state: &mut ApprovalServiceState,
        approval_id: &ApprovalId,
        now: DateTime<Utc>,
        deadline_passed: bool,
    ) -> Result<(), ApprovalError> {
        // Withdrawals whose custody is already retired are recorded first,
        // and a row whose stored form is unknown is never read or written
        // here: it is reconciled through the owner before reuse.
        self.record_pending_withdrawals_locked(state);
        state.ensure_reconciled(approval_id)?;
        // An approved parked candidate past its deadline is withdrawn here
        // (callback-free, under the reservation). A live candidate whose
        // decision is no longer current was re-adjudicated before this
        // reservation as its caller's step allows (ADR-001 D9), and its
        // owners' refusal withdrew it there. A withdrawal that could not be
        // recorded stops this read: nothing is derived from the old record
        // and the retired machine state.
        self.withdraw_stale_consent_locked(state, approval_id);
        state.ensure_reconciled(approval_id)?;
        let Some(record) = state.records.get(approval_id).cloned() else {
            return Ok(());
        };
        let expired = deadline_passed
            || record
                .expires_at
                .is_some_and(|expires_at| expires_at <= now);
        let mut authority = state.authority.clone();
        let outcome = authority
            .observe_approval_expiry(approval_id.to_string(), expired)
            .map_err(lifecycle_error)?;
        let status = lifecycle_status_from_outcome(approval_id, outcome, None)?;
        if status == record.status {
            state.authority = authority;
        } else {
            let mut expired_record = record.clone();
            expired_record.status = status;
            expired_record.updated_at = now;
            self.commit_row_locked(state, Some(&record), &expired_record)?;
            state.authority = authority;
            state.records.insert(approval_id.clone(), expired_record);
            self.withdraw_consent_locked(
                state,
                approval_id,
                ReviewRetirementReason::DeadlineExpired,
            );
            // The expiry is committed; a withdrawal that could not be recorded
            // leaves the row to reconciliation, never returned as current.
            state.ensure_reconciled(approval_id)?;
        }
        Ok(())
    }

    fn refresh_all_expiry(&self) -> Result<(), ApprovalError> {
        let ids = self
            .state
            .read()
            .records
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        self.settle_owner_now().run();
        for id in ids {
            self.refresh_expiry(&id)?;
        }
        Ok(())
    }
}

/// Reports a queued review disposal's settled result once the owner settles
/// it (on its next commit). Runs after the state lock is released.
pub(crate) type QueuedDisposalReport =
    Box<dyn FnOnce(Result<ReviewAttemptStatus, ReviewOwnerError>) + Send>;

type SettledReviewDisposal = (
    QueuedDisposalReport,
    Result<ReviewAttemptStatus, ReviewOwnerError>,
);

/// A review input handed to the owner without waiting for it.
struct QueuedReviewDisposal {
    id: String,
    input: QueuedReviewInput,
    report: QueuedDisposalReport,
}

/// What a queued review input applies once the owner settles it.
#[derive(Debug, Clone, Copy)]
enum QueuedReviewInput {
    /// Retire the attempt (when a reason is set), then hand it back
    /// (`ReleaseReview`): released, or its entered effect closed while a
    /// step admitted under it is in flight.
    Dispose(Option<ReviewRetirementReason>),
    /// One admitted step of the attempt's entered effect finished
    /// (`FinishEnteredStep`).
    FinishEnteredStep,
}

/// A refused reserved review commit: the typed refusal and the attempt's
/// disposal, settled under the same reservation, or `None` when queued.
pub(crate) struct RefusedReviewCommit<E> {
    pub(crate) error: ReservedReviewError<E>,
    pub(crate) disposal: Option<Result<ReviewAttemptStatus, ReviewOwnerError>>,
    /// Reports of unrelated disposals settled along the way; the caller
    /// runs them only after releasing its own locks.
    pub(crate) reports: SettledReports,
    /// The commit already happened (a spend refused by its final check after
    /// the owner exit): its status stays committed and nothing refunds.
    pub(crate) committed: Option<ReviewAttemptStatus>,
}

/// Apply one queued review input under a reservation already held.
fn apply_queued_locked(
    authority: &mut ApprovalLifecycleMachineAuthority,
    id: &str,
    input: QueuedReviewInput,
) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
    match input {
        QueuedReviewInput::Dispose(retire) => dispose_locked(authority, id, retire),
        QueuedReviewInput::FinishEnteredStep => {
            review_status_from_outcome(authority.finish_entered_step(id.to_owned()))
        }
    }
}

/// Retire (when set) and release one attempt under a reservation already
/// held. Release is best-effort cleanup of memory-only state; the reported
/// result is the retirement's when there is one.
fn dispose_locked(
    authority: &mut ApprovalLifecycleMachineAuthority,
    id: &str,
    retire: Option<ReviewRetirementReason>,
) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
    let retired = retire
        .map(|reason| review_status_from_outcome(authority.retire_review(id.to_owned(), reason)));
    let released = review_status_from_outcome(authority.release_review(id.to_owned()));
    retired.unwrap_or(released)
}

/// Settled review disposal reports, run outside every lock. Dropping runs
/// any report not yet run, so none is lost.
///
/// A store-free (review path) release that left a withdrawal awaiting its
/// durable record carries the owner's settlement duty here: once these
/// reports run (outside every lock, after the effect for a spend), the
/// owner settles from a store-capable path, or the holder that prevents it
/// settles or carries the duty on in turn.
#[must_use = "run the reports once outside every lock"]
#[derive(Default)]
pub(crate) struct SettledReports(Vec<SettledReviewDisposal>);

impl SettledReports {
    pub(crate) fn run(self) {
        drop(self);
    }

    /// Carry `other`'s reports (and any settlement duty) with these, to run
    /// together later.
    pub(crate) fn absorb(&mut self, mut other: SettledReports) {
        self.0.append(&mut other.0);
    }

    /// The owner's settlement duty, run with these reports.
    fn with_settlement_duty(mut self, owner: ApprovalService) -> Self {
        let settle: QueuedDisposalReport = Box::new(move |_| owner.settle_owner_now().run());
        self.0.push((settle, Ok(ReviewAttemptStatus::Retired)));
        self
    }
}

impl Drop for SettledReports {
    fn drop(&mut self) {
        for (report, result) in self.0.drain(..) {
            report(result);
        }
    }
}

impl Default for ApprovalService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct TestApprovalStore {
        records: RwLock<BTreeMap<ApprovalId, ApprovalRecord>>,
        put_calls: AtomicUsize,
        fail_on_put_call: Option<usize>,
        /// The failing put commits first and then reports an unknown outcome.
        commit_then_fail: bool,
        /// The failing put writes nothing but reports an unknown outcome.
        unknown_without_write: bool,
    }

    impl TestApprovalStore {
        fn new(fail_on_put_call: Option<usize>) -> Self {
            Self {
                records: RwLock::new(BTreeMap::new()),
                put_calls: AtomicUsize::new(0),
                fail_on_put_call,
                commit_then_fail: false,
                unknown_without_write: false,
            }
        }

        fn record(&self, approval_id: &ApprovalId) -> Option<ApprovalRecord> {
            self.records.read().get(approval_id).cloned()
        }
    }

    impl ApprovalStore for TestApprovalStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            Ok(self.records.read().values().cloned().collect())
        }

        fn get(
            &self,
            approval_id: &ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            Ok(self.record(approval_id))
        }

        fn put_if_current(
            &self,
            expected: Option<&ApprovalRecord>,
            record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            let put_call = self.put_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let fails = self
                .fail_on_put_call
                .is_some_and(|fail_on_put_call| fail_on_put_call == put_call);
            if fails && self.unknown_without_write {
                return Err(ApprovalStoreError::CommitUnknown(
                    "injected unknown outcome without a write".to_string(),
                ));
            }
            if fails && !self.commit_then_fail {
                return Err(ApprovalStoreError::NotCommitted(
                    "injected approval store failure".to_string(),
                ));
            }
            let mut records = self.records.write();
            if records.get(&record.approval_id) != expected {
                return Err(ApprovalStoreError::Conflict);
            }
            records.insert(record.approval_id.clone(), record.clone());
            if fails {
                return Err(ApprovalStoreError::CommitUnknown(
                    "injected failure after the write".to_string(),
                ));
            }
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    fn principal(value: &str) -> ApprovalPrincipalId {
        ApprovalPrincipalId::new(value).expect("valid principal")
    }

    fn request_with_allowed(allowed_decisions: BTreeSet<ApprovalDecision>) -> ApprovalRequest {
        ApprovalRequest {
            requester: principal("human:alice"),
            owner: ApprovalOwnerRef::Session {
                session_id: SessionId::new(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::ShellCommand,
                id: ApprovalResourceId::new("shell:rm"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::ShellCommand,
                summary: "run destructive command".to_string(),
                body: Some(json!({"cmd": "rm -rf target/tmp"})),
            },
            risk: ApprovalRisk::High,
            request_body: Some(json!({"why": "cleanup"})),
            allowed_decisions,
            expires_at: None,
            metadata: SurfaceMetadata::default(),
            request_provenance: Some(json!({"tool_call_id": "call-1"})),
        }
    }

    pub(super) fn request() -> ApprovalRequest {
        request_with_allowed(BTreeSet::from([
            ApprovalDecision::Approve,
            ApprovalDecision::Deny,
        ]))
    }

    #[test]
    fn approval_request_creates_pending_auditable_record() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert_eq!(record.requester.as_str(), "human:alice");
        assert_eq!(
            record.request_provenance,
            Some(json!({"tool_call_id": "call-1"}))
        );
        assert!(record.decision.is_none());
    }

    #[test]
    fn decide_preserves_request_provenance_and_records_decision_audit() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        let decided = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                Some("looks intentional".to_string()),
                Some(json!({"client": "mobile"})),
            )
            .expect("decision accepted");

        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(decided.request_provenance, record.request_provenance);
        let decision = decided.decision.expect("decision audit");
        assert_eq!(decision.actor.as_str(), "human:bob");
        assert_eq!(decision.provenance, Some(json!({"client": "mobile"})));
    }

    #[test]
    fn invalid_decision_is_rejected() {
        let service = ApprovalService::new();
        let record = service
            .request(request_with_allowed(BTreeSet::from([
                ApprovalDecision::Deny,
            ])))
            .expect("request accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("approval should reject disallowed decision");
        assert!(matches!(
            err,
            ApprovalError::InvalidDecision {
                decision: ApprovalDecision::Approve
            }
        ));
    }

    #[test]
    fn empty_allowed_decisions_are_rejected_by_generated_authority() {
        let service = ApprovalService::new();
        let err = service
            .request(request_with_allowed(BTreeSet::new()))
            .expect_err("empty allowed decisions rejected");
        assert!(matches!(err, ApprovalError::EmptyAllowedDecisions));
    }

    #[test]
    fn duplicate_decision_is_rejected() {
        let service = ApprovalService::new();
        let record = service.request(request()).expect("request accepted");
        service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None,
            )
            .expect("first decision accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("duplicate rejected");
        assert!(matches!(err, ApprovalError::AlreadyDecided { .. }));
    }

    #[test]
    fn failed_decision_persist_keeps_approval_pending_for_retry() {
        let store = Arc::new(TestApprovalStore::new(Some(2)));
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let record = service.request(request()).expect("request accepted");

        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("decision write should fail");

        assert!(matches!(err, ApprovalError::StoreUnavailable(_)));
        let cached = service.get(&record.approval_id).expect("cached record");
        assert_eq!(cached.status, ApprovalStatus::Pending);
        assert!(cached.decision.is_none());
        let persisted = store.record(&record.approval_id).expect("persisted record");
        assert_eq!(persisted.status, ApprovalStatus::Pending);
        assert!(persisted.decision.is_none());

        let retried = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                Some("changed my mind".to_string()),
                None,
            )
            .expect("retry should decide approval");
        assert_eq!(retried.status, ApprovalStatus::Denied);
    }

    #[test]
    fn restore_rejects_inconsistent_persisted_status() {
        let store = Arc::new(TestApprovalStore::new(None));
        let mut record = ApprovalService::new()
            .request(request())
            .expect("request accepted");
        record.status = ApprovalStatus::Approved;
        record.updated_at = Utc::now();
        store
            .records
            .write()
            .insert(record.approval_id.clone(), record);

        let restored = ApprovalService::with_store(store);
        assert!(matches!(
            restored,
            Err(ApprovalError::InvalidStoredRecord { .. })
        ));
    }

    /// The reconcile-before-reuse barrier holds at the reservation itself:
    /// a decision that passed its preflight before the row was marked
    /// (CommitUnknown that wrote nothing) cannot publish over the marked row,
    /// so reconciliation publishes the actual stored row, never an older
    /// snapshot over a newer confirmed one. The confirmed control decides.
    #[test]
    fn a_decision_past_preflight_cannot_publish_over_an_unreconciled_row() {
        let mut store = TestApprovalStore::new(Some(2));
        store.unknown_without_write = true;
        let store = Arc::new(store);
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let id = service
            .request(request())
            .expect("request accepted")
            .approval_id;
        assert_eq!(
            service.decide(
                &id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None
            ),
            Err(ApprovalError::StoreCommitUnknown {
                approval_id: id.clone()
            })
        );
        // Reconciliation reads the stored row outside the reservation.
        let mark = *service.state.read().unreconciled.get(&id).expect("marked");
        let stored = store.record(&id).expect("stored row");
        // A decision that already passed its preflight reaches the reservation.
        let mut state = service.state.write();
        assert_eq!(
            service.decide_locked(
                &mut state,
                &id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
                Utc::now(),
            ),
            Err(ApprovalError::Unreconciled {
                approval_id: id.clone()
            })
        );
        assert_eq!(
            store.put_calls.load(Ordering::SeqCst),
            2,
            "no write past the barrier"
        );
        let published =
            ApprovalService::reconcile_locked(&mut state, &id, mark, stored).expect("reconciled");
        service.release_state(state).run();
        assert_eq!(published.status, ApprovalStatus::Pending);
        // Confirmed control.
        let decided = service
            .decide(
                &id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect("decided");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(
            store.record(&id).map(|row| row.status),
            Some(ApprovalStatus::Approved)
        );
    }

    /// A decision whose store write committed before reporting failure is
    /// never published as a no-commit: the outcome is typed unknown, nothing
    /// is visible from the uncertain write, and the row is reconciled from
    /// the store before reuse.
    #[test]
    fn a_commit_then_error_decision_is_unknown_and_reconciled() {
        let mut store = TestApprovalStore::new(Some(2));
        store.commit_then_fail = true;
        let store = Arc::new(store);
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let record = service.request(request()).expect("request accepted");

        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("the decision outcome is unknown");
        assert_eq!(
            err,
            ApprovalError::StoreCommitUnknown {
                approval_id: record.approval_id.clone()
            }
        );
        // The store did commit; reading reconciles to it.
        let reconciled = service.get(&record.approval_id).expect("reconciled");
        assert_eq!(reconciled.status, ApprovalStatus::Approved);
        assert_eq!(
            store.record(&record.approval_id).map(|row| row.status),
            Some(ApprovalStatus::Approved)
        );
        assert!(matches!(
            service.decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                principal("human:bob"),
                None,
                None,
            ),
            Err(ApprovalError::AlreadyDecided { .. })
        ));
    }

    #[test]
    fn unavailable_approval_service_fails_closed() {
        let service = ApprovalService::unavailable("persistent approval restore failed");
        let err = service
            .list(ApprovalListFilter::default())
            .expect_err("unavailable service should reject reads");
        assert!(matches!(
            err,
            ApprovalError::StoreUnavailable(message) if message.contains("persistent approval restore failed")
        ));
        let err = service
            .request(request())
            .expect_err("unavailable service should reject writes");
        assert!(matches!(
            err,
            ApprovalError::StoreUnavailable(message) if message.contains("persistent approval restore failed")
        ));
    }

    #[test]
    fn expired_approval_cannot_be_decided() {
        let service = ApprovalService::new();
        let mut request = request();
        request.expires_at = Some(Utc::now() - Duration::seconds(1));
        let record = service.request(request).expect("request accepted");
        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("expired approval rejected");
        assert!(matches!(err, ApprovalError::Expired { .. }));
        assert_eq!(
            service.get(&record.approval_id).expect("record").status,
            ApprovalStatus::Expired
        );
    }

    #[test]
    fn deciding_expired_approval_persists_expiry_transition() {
        let store = Arc::new(TestApprovalStore::new(None));
        let service = ApprovalService::with_store(store.clone()).expect("service");
        let mut request = request();
        request.expires_at = Some(Utc::now() - Duration::seconds(1));
        let record = service.request(request).expect("request accepted");

        let err = service
            .decide(
                &record.approval_id,
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("expired approval rejected");

        assert!(matches!(err, ApprovalError::Expired { .. }));
        let persisted = store.record(&record.approval_id).expect("persisted record");
        assert_eq!(persisted.status, ApprovalStatus::Expired);
        assert!(persisted.decision.is_none());
    }

    #[test]
    fn nonexistent_approval_cannot_be_decided() {
        let service = ApprovalService::new();
        let err = service
            .decide(
                &ApprovalId::new(),
                ApprovalDecision::Approve,
                principal("human:bob"),
                None,
                None,
            )
            .expect_err("unknown approval rejected");
        assert!(matches!(err, ApprovalError::NotFound { .. }));
    }

    #[test]
    fn reserved_metadata_spoofing_is_rejected() {
        let service = ApprovalService::new();
        let mut request = request();
        request
            .metadata
            .labels
            .insert("meerkat.approval_id".to_string(), "spoof".to_string());
        let err = service
            .request(request)
            .expect_err("reserved metadata rejected");
        assert!(matches!(
            err,
            ApprovalError::InvalidMetadata(crate::SurfaceMetadataError::ReservedLabelKey { .. })
        ));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod review_owner_tests {
    //! Generated review attempt ownership: identity binding, single use,
    //! retirement ordering and memory-only state.

    use super::*;
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        PreparedAuthorizationBinding, PreparedOperationAuthorization, PreparedOperationCheck,
        SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
        WorkAuthorization, WorkAuthorizationContext,
    };
    use crate::exact_operation::OperationExecutionScope;
    use crate::memory::MemorySearchScope;
    use crate::ops::OperationId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Raw generated owner transitions, without the reserved-commit
    /// settlement policy (which retires refused commits); these tests pin
    /// the transitions themselves. The binding check is the production one.
    fn record(
        approvals: &ApprovalService,
        handle: &ReviewAttemptHandle,
        verdict: ReviewVerdict,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        approvals.apply_review(|authority| {
            authority.record_review_verdict(handle.id().to_owned(), verdict)
        })
    }

    fn spend(
        approvals: &ApprovalService,
        handle: &ReviewAttemptHandle,
        check: &PreparedOperationCheck,
        work: &WorkAuthorizationContext,
    ) -> Result<ReviewAttemptStatus, ReviewOwnerError> {
        if !handle.bound_to(check.binding(), work) {
            return Err(ReviewOwnerError::Mismatch);
        }
        approvals
            .apply_review(|authority| authority.consume_review_for_entry(handle.id().to_owned()))
    }

    fn binding() -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId(Uuid::nil()),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(
                    SessionId::from_uuid(Uuid::nil()),
                )),
                usage: SourceAuthorizationUse::Read,
            }),
        })
    }

    struct Allow;

    impl PreparedOperationAuthorization for Allow {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }
    }

    struct AllowWork;

    impl WorkAuthorization for AllowWork {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Allow))
        }
    }

    fn work() -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(Arc::new(AllowWork), OperationExecutionScope::Domain)
    }

    fn check(
        work: &WorkAuthorizationContext,
        binding: &PreparedAuthorizationBinding,
    ) -> PreparedOperationCheck {
        PreparedOperationCheck::prepare(work.clone(), binding.clone()).expect("prepared")
    }

    #[test]
    fn late_verdict_after_retirement_is_rejected_and_cannot_enter() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        assert_eq!(
            approvals.retire_review(&handle, ReviewRetirementReason::DeadlineExpired),
            Ok(ReviewAttemptStatus::Retired)
        );
        assert_eq!(
            record(&approvals, &handle, ReviewVerdict::Allow),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewRetired
            ))
        );
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &binding), &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewRetired
            ))
        );
    }

    #[test]
    fn allow_is_spent_once_and_used_is_never_relabelled() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let entering = check(&work, &binding);
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewNotSatisfied
            )),
            "a pending review cannot be spent"
        );
        assert_eq!(
            record(&approvals, &handle, ReviewVerdict::Allow),
            Ok(ReviewAttemptStatus::Allowed)
        );
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Ok(ReviewAttemptStatus::Used)
        );
        assert_eq!(
            spend(&approvals, &handle, &entering, &work),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::AlreadyDecided
            ))
        );
        assert_eq!(
            approvals.retire_review(&handle, ReviewRetirementReason::Abandoned),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::AlreadyDecided
            )),
            "a spent allow is never relabelled as retired"
        );
    }

    #[test]
    fn denied_or_escalated_review_cannot_be_retired_or_spent() {
        for (verdict, settled) in [
            (ReviewVerdict::Deny, ReviewAttemptStatus::Denied),
            (ReviewVerdict::Escalate, ReviewAttemptStatus::Escalated),
        ] {
            let approvals = ApprovalService::new();
            let (work, binding) = (work(), binding());
            let handle = approvals.begin_review(&binding, &work).expect("attempt");
            assert_eq!(record(&approvals, &handle, verdict), Ok(settled));
            assert!(
                approvals
                    .retire_review(&handle, ReviewRetirementReason::ContextChanged)
                    .is_err()
            );
            assert!(spend(&approvals, &handle, &check(&work, &binding), &work).is_err());
        }
    }

    #[test]
    fn equal_facts_or_another_work_association_cannot_spend_an_allow() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        record(&approvals, &handle, ReviewVerdict::Allow).expect("allowed");
        let equal_facts = PreparedAuthorizationBinding::new(binding.facts().clone());
        let other_work = self::work();
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &equal_facts), &work),
            Err(ReviewOwnerError::Mismatch)
        );
        assert_eq!(
            spend(
                &approvals,
                &handle,
                &check(&other_work, &binding),
                &other_work
            ),
            Err(ReviewOwnerError::Mismatch)
        );
        assert_eq!(
            spend(&approvals, &handle, &check(&work, &binding), &work),
            Ok(ReviewAttemptStatus::Used),
            "the exact operation and work still enter once"
        );
    }

    #[test]
    fn attempts_are_fresh_and_released_attempts_are_unknown() {
        let approvals = ApprovalService::new();
        let (work, binding) = (work(), binding());
        let first = approvals.begin_review(&binding, &work).expect("attempt");
        let second = approvals.begin_review(&binding, &work).expect("attempt");
        assert_ne!(first.id(), second.id(), "attempt ids are never reused");
        assert_eq!(
            approvals.release_review(first),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::ReviewPending
            )),
            "a pending attempt must be retired before disposal"
        );
        let first = approvals.begin_review(&binding, &work).expect("attempt");
        approvals
            .apply_review(|authority| authority.record_review_unavailable(first.id().to_owned()))
            .expect("unavailable");
        let id = first.id().to_owned();
        assert_eq!(
            approvals.release_review(first),
            Ok(ReviewAttemptStatus::Unavailable)
        );
        let copied = ReviewAttemptHandle::new(Arc::from(id), binding, work);
        assert_eq!(
            record(&approvals, &copied, ReviewVerdict::Allow),
            Err(ReviewOwnerError::Rejected(
                ApprovalLifecycleRejectionReason::NotFound
            )),
            "a disposed attempt cannot be revived by its id"
        );
    }

    /// A held in-memory store is contention, reported at once: the
    /// conditional write never waits and writes nothing; once free it
    /// commits.
    #[test]
    fn a_held_in_memory_store_is_contention_not_a_wait() {
        let store = InMemoryApprovalStore::new();
        let record = ApprovalService::new()
            .request(super::tests::request())
            .expect("request accepted");
        let held = store.records.read();
        assert_eq!(
            store.put_if_current(None, &record),
            Err(ApprovalStoreError::Contended)
        );
        drop(held);
        assert!(store.load_all().expect("rows").is_empty());
        store
            .put_if_current(None, &record)
            .expect("a free store commits");
    }

    /// The generated owner keeps a spent attempt `Used` while its effect is
    /// entered: later steps are admitted and finished without spending; the
    /// dispatch end releases it with no step in flight, or closes it so that
    /// no new step begins and the last admitted step's finish releases it;
    /// an unspent, retired or consent-linked attempt enters nothing, and a
    /// consent spend opens the same entered state.
    #[test]
    fn the_generated_owner_keeps_an_entered_attempt_until_its_last_step() {
        use ApprovalLifecycleOutcome::{Rejected, ReviewStatus, Status};
        use ApprovalLifecycleRejectionReason as Reason;
        let mut owner = ApprovalLifecycleMachineAuthority::new();
        let s = |value: &str| value.to_owned();
        let used = || Ok(ReviewStatus(ReviewAttemptStatus::Used));

        assert_eq!(
            owner.begin_review(s("r1")),
            Ok(ReviewStatus(ReviewAttemptStatus::Pending))
        );
        assert_eq!(
            owner.admit_entered_step(s("r1")),
            Ok(Rejected(Reason::ReviewNotSatisfied))
        );
        assert_eq!(
            owner.record_review_verdict(s("r1"), ReviewVerdict::Allow),
            Ok(ReviewStatus(ReviewAttemptStatus::Allowed))
        );
        assert_eq!(
            owner.admit_entered_step(s("r1")),
            Ok(Rejected(Reason::ReviewNotSatisfied)),
            "an allowed but unspent review enters nothing"
        );
        assert_eq!(
            owner.finish_entered_step(s("r1")),
            Ok(Rejected(Reason::AlreadyDecided)),
            "no step in flight to finish"
        );
        assert_eq!(owner.consume_review_for_entry(s("r1")), used());
        assert_eq!(owner.admit_entered_step(s("r1")), used());
        assert_eq!(
            owner.admit_entered_step(s("r1")),
            used(),
            "two steps in flight, no second spend"
        );
        assert_eq!(
            owner.consume_review_for_entry(s("r1")),
            Ok(Rejected(Reason::AlreadyDecided)),
            "a second spend is refused while entered"
        );
        assert_eq!(
            owner.release_review(s("r1")),
            used(),
            "the dispatch ends with steps in flight: still Used, closing"
        );
        assert_eq!(
            owner.admit_entered_step(s("r1")),
            Ok(Rejected(Reason::Expired)),
            "no step begins after the dispatch end"
        );
        assert_eq!(
            owner.release_review(s("r1")),
            Ok(Rejected(Reason::AlreadyDecided)),
            "the dispatch hands its attempt back once"
        );
        assert_eq!(
            owner.retire_review(s("r1"), ReviewRetirementReason::Abandoned),
            Ok(Rejected(Reason::AlreadyDecided)),
            "an entered attempt is never retired"
        );
        assert_eq!(
            owner.finish_entered_step(s("r1")),
            used(),
            "one step finished, one still in flight"
        );
        assert_eq!(
            owner.consume_review_for_entry(s("r1")),
            Ok(Rejected(Reason::AlreadyDecided)),
            "still registered while a step is in flight"
        );
        assert_eq!(
            owner.finish_entered_step(s("r1")),
            used(),
            "the last finish releases the attempt"
        );
        for (input, answer) in [
            (
                owner.admit_entered_step(s("r1")),
                Rejected(Reason::NotFound),
            ),
            (
                owner.finish_entered_step(s("r1")),
                Rejected(Reason::NotFound),
            ),
            (owner.release_review(s("r1")), Rejected(Reason::NotFound)),
            (
                owner.consume_review_for_entry(s("r1")),
                Rejected(Reason::NotFound),
            ),
        ] {
            assert_eq!(input, Ok(answer), "released: unknown to the owner");
        }

        // With no step in flight the dispatch end releases at once.
        assert_eq!(
            owner.begin_review(s("r2")),
            Ok(ReviewStatus(ReviewAttemptStatus::Pending))
        );
        assert_eq!(
            owner.record_review_verdict(s("r2"), ReviewVerdict::Allow),
            Ok(ReviewStatus(ReviewAttemptStatus::Allowed))
        );
        assert_eq!(owner.consume_review_for_entry(s("r2")), used());
        assert_eq!(owner.admit_entered_step(s("r2")), used());
        assert_eq!(
            owner.finish_entered_step(s("r2")),
            used(),
            "a step that ends while the dispatch is live"
        );
        assert_eq!(owner.release_review(s("r2")), used());
        assert_eq!(
            owner.admit_entered_step(s("r2")),
            Ok(Rejected(Reason::NotFound))
        );

        // A retired attempt enters nothing.
        assert_eq!(
            owner.begin_review(s("r3")),
            Ok(ReviewStatus(ReviewAttemptStatus::Pending))
        );
        assert_eq!(
            owner.retire_review(s("r3"), ReviewRetirementReason::ContextChanged),
            Ok(ReviewStatus(ReviewAttemptStatus::Retired))
        );
        assert_eq!(
            owner.admit_entered_step(s("r3")),
            Ok(Rejected(Reason::ReviewRetired))
        );

        // A consent-linked attempt enters nothing until its consent is spent,
        // and the consent spend opens the same entered state.
        assert_eq!(
            owner.create_consent_approval(s("a1"), s("r4"), true, true, true),
            Ok(Status(ApprovalLifecycleStatus::Pending))
        );
        assert_eq!(
            owner.admit_entered_step(s("r4")),
            Ok(Rejected(Reason::ConsentLinked))
        );
        assert_eq!(
            owner.decide_consent(s("a1"), s("r4"), ApprovalLifecycleDecision::Approve, true),
            Ok(Status(ApprovalLifecycleStatus::Approved))
        );
        assert_eq!(
            owner.admit_entered_step(s("r4")),
            Ok(Rejected(Reason::ConsentLinked)),
            "approved but unspent consent enters nothing"
        );
        assert_eq!(owner.consume_consent_for_entry(s("a1"), s("r4")), used());
        assert_eq!(owner.admit_entered_step(s("r4")), used());
        assert_eq!(
            owner.consume_consent_for_entry(s("a1"), s("r4")),
            Ok(Rejected(Reason::AlreadyConsumed))
        );
        assert_eq!(owner.release_review(s("r4")), used());
        assert_eq!(owner.finish_entered_step(s("r4")), used());
        assert_eq!(
            owner.admit_entered_step(s("r4")),
            Ok(Rejected(Reason::NotFound))
        );
    }

    /// The generated owner holds the consent link and its one-use legality:
    /// only the linked pair moves, only on an authenticated decision; a
    /// generic input never decides the approval or spends its attempt; one
    /// valid spend succeeds and a second is refused.
    #[test]
    fn the_generated_owner_spends_only_the_linked_consent_once() {
        use ApprovalLifecycleOutcome::{Rejected, ReviewStatus, Status};
        use ApprovalLifecycleRejectionReason as Reason;
        let mut owner = ApprovalLifecycleMachineAuthority::new();
        let s = |value: &str| value.to_owned();
        for (approval, attempt) in [("a1", "r1"), ("a2", "r2")] {
            assert_eq!(
                owner.create_consent_approval(s(approval), s(attempt), true, true, true),
                Ok(Status(ApprovalLifecycleStatus::Pending))
            );
        }
        let approve = ApprovalLifecycleDecision::Approve;
        assert_eq!(
            owner.decide_approval(s("a1"), approve),
            Ok(Rejected(Reason::AuthenticationRequired)),
            "the generic decision never decides consent"
        );
        assert_eq!(
            owner.decide_consent(s("a1"), s("r2"), approve, true),
            Ok(Rejected(Reason::ConsentMismatch)),
            "another approval's attempt is not linked"
        );
        assert_eq!(
            owner.decide_consent(s("a1"), s("r1"), approve, false),
            Ok(Rejected(Reason::AuthenticationRequired))
        );
        assert_eq!(
            owner.decide_consent(s("a1"), s("r1"), approve, true),
            Ok(Status(ApprovalLifecycleStatus::Approved))
        );
        assert_eq!(
            owner.consume_review_for_entry(s("r1")),
            Ok(Rejected(Reason::ConsentLinked)),
            "the generic spend refuses a linked attempt"
        );
        assert_eq!(
            owner.consume_consent_for_entry(s("a1"), s("r2")),
            Ok(Rejected(Reason::ConsentMismatch))
        );
        assert_eq!(
            owner.consume_consent_for_entry(s("a2"), s("r2")),
            Ok(Rejected(Reason::ReviewNotSatisfied)),
            "a pending consent is not spendable"
        );
        assert_eq!(
            owner.consume_consent_for_entry(s("a1"), s("r1")),
            Ok(ReviewStatus(ReviewAttemptStatus::Used))
        );
        assert_eq!(
            owner.consume_consent_for_entry(s("a1"), s("r1")),
            Ok(Rejected(Reason::AlreadyConsumed))
        );
        assert_eq!(
            owner.withdraw_consent(s("a2"), s("r2"), ReviewRetirementReason::RunEnded),
            Ok(Status(ApprovalLifecycleStatus::Cancelled)),
            "a withdrawn pending consent is cancelled, not denied"
        );
        assert_eq!(
            owner.decide_consent(s("a2"), s("r2"), approve, true),
            Ok(Rejected(Reason::ConsentMismatch)),
            "a withdrawn attempt can no longer be decided"
        );
    }

    /// The generated owner settles a consent row no attempt is linked to,
    /// without any candidate, whether its own owner still owes the
    /// withdrawal's record or its issuing owner is proven lost: a pending
    /// row is cancelled, approved unspent history is kept, and a missing,
    /// non-consent, linked (held here), spent, expired or already settled
    /// row is refused.
    #[test]
    fn the_generated_owner_settles_unlinked_consent_rows() {
        use ApprovalLifecycleOutcome::{Rejected, Status};
        use ApprovalLifecycleRejectionReason as Reason;
        use ApprovalLifecycleStatus as Lifecycle;
        let mut owner = ApprovalLifecycleMachineAuthority::new();
        let s = |value: &str| value.to_owned();
        let approve = Some(ApprovalLifecycleDecision::Approve);
        let deny = Some(ApprovalLifecycleDecision::Deny);
        for (id, status, decision, consent, authenticated, consumed) in [
            ("pending", Lifecycle::Pending, None, true, false, false),
            ("approved", Lifecycle::Approved, approve, true, true, false),
            ("spent", Lifecycle::Approved, approve, true, true, true),
            ("expired", Lifecycle::Expired, None, true, false, false),
            ("denied", Lifecycle::Denied, deny, true, true, false),
            ("cancelled", Lifecycle::Cancelled, None, true, false, false),
            ("plain", Lifecycle::Pending, None, false, false, false),
        ] {
            assert_eq!(
                owner.restore_approval(
                    s(id),
                    status,
                    true,
                    true,
                    true,
                    decision,
                    consent,
                    authenticated,
                    consumed
                ),
                Ok(Status(status)),
                "{id} restored"
            );
        }
        assert_eq!(
            owner.create_consent_approval(s("held"), s("r1"), true, true, true),
            Ok(Status(Lifecycle::Pending))
        );
        for (id, expected) in [
            ("missing", Rejected(Reason::NotFound)),
            ("plain", Rejected(Reason::NotConsent)),
            ("held", Rejected(Reason::ConsentLinked)),
            ("spent", Rejected(Reason::AlreadyConsumed)),
            ("expired", Rejected(Reason::Expired)),
            ("denied", Rejected(Reason::AlreadyDecided)),
            ("cancelled", Rejected(Reason::AlreadyDecided)),
            ("pending", Status(Lifecycle::Cancelled)),
            ("approved", Status(Lifecycle::Approved)),
        ] {
            assert_eq!(owner.settle_unlinked_consent(s(id)), Ok(expected), "{id}");
        }
        assert_eq!(
            owner.settle_unlinked_consent(s("pending")),
            Ok(Rejected(Reason::AlreadyDecided)),
            "a cancelled row is settled once"
        );
    }

    #[derive(Default)]
    struct CountingStore(AtomicUsize);

    impl ApprovalStore for CountingStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            Ok(Vec::new())
        }

        fn get(
            &self,
            _approval_id: &ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            Ok(None)
        }

        fn put_if_current(
            &self,
            _expected: Option<&ApprovalRecord>,
            _record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    #[test]
    fn review_attempts_never_reach_the_approval_store() {
        let store = Arc::new(CountingStore::default());
        let approvals = ApprovalService::with_store(store.clone()).expect("service");
        let (work, binding) = (work(), binding());
        let handle = approvals.begin_review(&binding, &work).expect("attempt");
        record(&approvals, &handle, ReviewVerdict::Allow).expect("allowed");
        spend(&approvals, &handle, &check(&work, &binding), &work).expect("used");
        approvals.release_review(handle).expect("released");
        assert_eq!(store.0.load(Ordering::SeqCst), 0);
        // A reopened owner over the same store knows no review attempt.
        let reopened = ApprovalService::with_store(store).expect("reopened");
        assert!(
            reopened
                .list(ApprovalListFilter::default())
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn unavailable_owner_refuses_new_attempts() {
        let approvals = ApprovalService::unavailable("fixture");
        assert_eq!(
            approvals.begin_review(&binding(), &work()).err(),
            Some(ReviewOwnerError::Unavailable)
        );
    }
}
