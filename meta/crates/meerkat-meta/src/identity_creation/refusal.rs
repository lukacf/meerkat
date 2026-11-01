//! The closed refusal vocabulary of identity requests (the wire's
//! `data.kind`), and the typed refusal of a creation policy.
//!
//! Every identity refusal is one kind of a closed list, with the request
//! field it concerns as a path (`identities[1].spec.labels`). The list is
//! closed: a client must handle every kind, and a new kind is a vocabulary
//! change. It is one vocabulary for every identity, whatever its origin.
//! Normalization produces the request kinds (request data, name, peers,
//! policy); the executor, the decision ingresses and Core's commit path
//! produce the others through the same type.

use std::fmt;

use serde_json::{Value, json};

use super::config::ChangeClass;

/// Why an identity request is refused: one kind of the closed vocabulary,
/// with the request field it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityRefusal {
    /// The kind, with its details.
    pub kind: IdentityRefusalKind,
    /// The request field the refusal concerns, as a path such as
    /// `identities[1].spec.labels`; `None` for the request as a whole.
    pub field: Option<String>,
}

impl IdentityRefusal {
    /// A refusal of the request as a whole.
    pub fn new(kind: IdentityRefusalKind) -> Self {
        Self { kind, field: None }
    }

    /// This refusal, about the request field `field`.
    #[must_use]
    pub fn at(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// The kind's stable token (`reserved_key`, ...).
    pub fn kind_token(&self) -> &'static str {
        self.kind.token()
    }

    /// Whether the same request may succeed later unchanged: changes held,
    /// host preparation pending, or consequence lines the host could not
    /// produce. Every other kind is permanent.
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// The refusal as the wire's `error.data`: `kind`, the kind's details,
    /// and `field` when the refusal concerns one field.
    pub fn to_data(&self) -> Value {
        let mut data = self.kind.details();
        if let Value::Object(map) = &mut data {
            map.insert("kind".to_owned(), Value::from(self.kind.token()));
            if let Some(field) = &self.field {
                map.insert("field".to_owned(), Value::from(field.as_str()));
            }
        }
        data
    }
}

impl From<IdentityRefusalKind> for IdentityRefusal {
    fn from(kind: IdentityRefusalKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for IdentityRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind.token())?;
        if let Some(field) = &self.field {
            write!(formatter, " at {field}")?;
        }
        let details = self.kind.details();
        if details.as_object().is_some_and(|map| !map.is_empty()) {
            write!(formatter, " {details}")?;
        }
        Ok(())
    }
}

impl std::error::Error for IdentityRefusal {}

/// A host's refusal of a request in its own domain check. The requester sees
/// `validation_refused`, with the reason as quoted data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationRefusal {
    /// Why, in the host's words.
    pub reason: String,
}

impl ValidationRefusal {
    /// A refusal for `reason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl From<ValidationRefusal> for IdentityRefusal {
    fn from(refusal: ValidationRefusal) -> Self {
        Self::new(IdentityRefusalKind::ValidationRefused {
            reason: refusal.reason,
        })
    }
}

impl fmt::Display for ValidationRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "the host refused the request: {}", self.reason)
    }
}

impl std::error::Error for ValidationRefusal {}

/// Who holds a name a request asked for. `declared` and `created` name
/// where the identity's desired record lives, the host's roster or Core's
/// store, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NameTakenBy {
    /// An identity a host's roster declares.
    Declared,
    /// An identity Core's store holds, created at run time, or another
    /// identity of the same request.
    Created,
    /// A removed identity: its name is a tombstone, never reused.
    Removed,
    /// A worker member that is no identity.
    Worker,
    /// An entry this build cannot read.
    Unreadable,
}

impl NameTakenBy {
    /// The stable token (`declared`, `created`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Created => "created",
            Self::Removed => "removed",
            Self::Worker => "worker",
            Self::Unreadable => "unreadable",
        }
    }
}

/// What is wrong with the shape of a request that is well typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RequestProblem {
    /// Nothing to act on: a create with no identity, or an update that
    /// changes nothing.
    Empty,
    /// Two parts of the request contradict each other, such as one peer
    /// both added and removed.
    Conflicting,
    /// A value outside its form, such as a descriptive label key outside
    /// the key alphabet or a label value with a control character.
    Malformed,
}

impl RequestProblem {
    /// The stable token (`empty`, `conflicting`, `malformed`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Conflicting => "conflicting",
            Self::Malformed => "malformed",
        }
    }
}

/// One kind of the closed refusal vocabulary, with its details.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityRefusalKind {
    // Request data.
    /// The profile is not one of the mob definition's profiles.
    ProfileNotAllowed { profile: String },
    /// The request names a reserved label key.
    ReservedKey { key: String },
    /// An authority field under an outcome other than a human decision.
    AuthorityFieldNeedsApproval { key: String },
    /// An authority field the policy does not declare, or a value its
    /// declaration does not allow.
    AuthorityFieldInvalid { key: String },
    /// A text larger than its limit, in bytes. Text is never cut.
    TextTooLarge { limit: u64 },
    /// The host's consequence lines could not be produced. The request is
    /// never rendered without them (retryable).
    ConsequencesUnavailable,
    /// The request is well typed but has nothing to act on, or contradicts
    /// itself.
    InvalidRequest { problem: RequestProblem },
    // Name.
    /// The name is not a valid identity name.
    InvalidName,
    /// The name is taken.
    NameTaken { by: NameTakenBy },
    // Peers.
    /// The peer rules refuse this peer.
    PeerNotAllowed { identity: String },
    /// The peer is not an identity the requester may name.
    PeerUnknown,
    // Policy.
    /// More identities than one store transaction commits.
    BatchTooLarge { max: u64 },
    /// The host's validation hook refused the request; the reason is the
    /// host's, quoted.
    ValidationRefused { reason: String },
    /// An approval rule denies this change.
    ChangeDenied { change: ChangeClass },
    /// The request needs a form this release does not support.
    Unsupported { feature: String },
    // Identities and operations.
    /// No identity has this name.
    IdentityUnknown,
    /// An agent asked to change its own identity.
    SelfChangeRefused,
    /// The spec revision the requester read is not the current one, which
    /// `current` names.
    StaleSpec { current: u64 },
    /// The operation id was used with another request.
    OperationReused,
    /// A spawn or held spawn of the name is not settled.
    AdmissionUnsettled { correlation: String },
    /// The identity is not in a custody transfer.
    NotTransferring,
    /// The custody transfer's predecessor does not match.
    PredecessorMismatch,
    /// The import is refused.
    ImportRefused,
    /// No host preparation is pending for the identity.
    PreparationNotPending,
    /// The preparation settle names another operation.
    PreparationMismatch,
    /// A held construction is open for the identity.
    ConstructionOpen,
    /// No construction is held, or the settle names an attempt that is not
    /// the current one, which `current` names.
    StaleAttempt { current: Option<u64> },
    /// The identity has no held construction.
    NotConstructed,
    /// The policy's preparation is `constructed`: a settle must name the
    /// held attempt.
    ConstructionRequired,
    /// The attempt's activation was accepted and its publication is in
    /// flight: nothing may abort or replace it.
    ActivationPending { attempt: u64 },
    /// The identity is in a custody transfer.
    IdentityTransferring,
    /// A custody transfer is in flight.
    TransferInFlight,
    /// The observed seat does not match.
    ObservationMismatch,
    // Decisions.
    /// No routable decider exists; nothing was written.
    ApprovalUnavailable,
    /// A decision is already open for the identity.
    DecisionOpen { decision_id: String },
    /// At execution the request would be wider than what was presented.
    SubjectWidened,
    /// The decision has ended.
    DecisionClosed { state: String },
    /// No such decision.
    DecisionNotFound,
    // Decide side.
    /// The decide carries no attested evidence.
    EvidenceRequired,
    /// The presented digest is not the decision's subject digest.
    SubjectChanged,
    /// The presentation did not cover every canonical part.
    PresentationIncomplete,
    /// The requester cannot decide its own request.
    RequesterCannotDecide,
    /// The approver is not eligible.
    ApproverNotEligible,
    /// The decide was observed before the decision was first presented.
    DecidedBeforePresentation,
    // Admission.
    /// Changes are held (retryable).
    ChangesHeld,
    /// Host preparation is pending (retryable).
    HostPreparationPending,
    /// A candidate launch creates, presents, decides and executes nothing.
    CandidateLaunch,
    /// An agent mob tool named an identity member.
    IdentityMember,
    /// The mob's storage composition cannot commit an operation in one
    /// transaction.
    StorageUnsupported,
    /// Core refused the held construction.
    ConstructionRefused { kind: String },
}

impl IdentityRefusalKind {
    /// The kind's stable token, the wire's `data.kind`.
    pub fn token(&self) -> &'static str {
        match self {
            Self::ProfileNotAllowed { .. } => "profile_not_allowed",
            Self::ReservedKey { .. } => "reserved_key",
            Self::AuthorityFieldNeedsApproval { .. } => "authority_field_needs_approval",
            Self::AuthorityFieldInvalid { .. } => "authority_field_invalid",
            Self::TextTooLarge { .. } => "text_too_large",
            Self::ConsequencesUnavailable => "consequences_unavailable",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::InvalidName => "invalid_name",
            Self::NameTaken { .. } => "name_taken",
            Self::PeerNotAllowed { .. } => "peer_not_allowed",
            Self::PeerUnknown => "peer_unknown",
            Self::BatchTooLarge { .. } => "batch_too_large",
            Self::ValidationRefused { .. } => "validation_refused",
            Self::ChangeDenied { .. } => "change_denied",
            Self::Unsupported { .. } => "unsupported",
            Self::IdentityUnknown => "identity_unknown",
            Self::SelfChangeRefused => "self_change_refused",
            Self::StaleSpec { .. } => "stale_spec",
            Self::OperationReused => "operation_reused",
            Self::AdmissionUnsettled { .. } => "admission_unsettled",
            Self::NotTransferring => "not_transferring",
            Self::PredecessorMismatch => "predecessor_mismatch",
            Self::ImportRefused => "import_refused",
            Self::PreparationNotPending => "preparation_not_pending",
            Self::PreparationMismatch => "preparation_mismatch",
            Self::ConstructionOpen => "construction_open",
            Self::StaleAttempt { .. } => "stale_attempt",
            Self::NotConstructed => "not_constructed",
            Self::ConstructionRequired => "construction_required",
            Self::ActivationPending { .. } => "activation_pending",
            Self::IdentityTransferring => "identity_transferring",
            Self::TransferInFlight => "transfer_in_flight",
            Self::ObservationMismatch => "observation_mismatch",
            Self::ApprovalUnavailable => "approval_unavailable",
            Self::DecisionOpen { .. } => "decision_open",
            Self::SubjectWidened => "subject_widened",
            Self::DecisionClosed { .. } => "decision_closed",
            Self::DecisionNotFound => "decision_not_found",
            Self::EvidenceRequired => "evidence_required",
            Self::SubjectChanged => "subject_changed",
            Self::PresentationIncomplete => "presentation_incomplete",
            Self::RequesterCannotDecide => "requester_cannot_decide",
            Self::ApproverNotEligible => "approver_not_eligible",
            Self::DecidedBeforePresentation => "decided_before_presentation",
            Self::ChangesHeld => "changes_held",
            Self::HostPreparationPending => "host_preparation_pending",
            Self::CandidateLaunch => "candidate_launch",
            Self::IdentityMember => "identity_member",
            Self::StorageUnsupported => "storage_unsupported",
            Self::ConstructionRefused { .. } => "construction_refused",
        }
    }

    /// Whether the same request may succeed later unchanged.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ChangesHeld | Self::HostPreparationPending | Self::ConsequencesUnavailable
        )
    }

    /// The kind's details, as the members of the wire's `error.data` beside
    /// `kind` and `field`.
    fn details(&self) -> Value {
        match self {
            Self::ProfileNotAllowed { profile } => json!({ "profile": profile }),
            Self::ReservedKey { key }
            | Self::AuthorityFieldNeedsApproval { key }
            | Self::AuthorityFieldInvalid { key } => json!({ "key": key }),
            Self::TextTooLarge { limit } => json!({ "limit": limit }),
            Self::InvalidRequest { problem } => json!({ "problem": problem.token() }),
            Self::NameTaken { by } => json!({ "by": by.token() }),
            Self::PeerNotAllowed { identity } => json!({ "identity": identity }),
            Self::BatchTooLarge { max } => json!({ "max": max }),
            Self::ValidationRefused { reason } => json!({ "reason": reason }),
            Self::ChangeDenied { change } => json!({ "change": change.token() }),
            Self::Unsupported { feature } => json!({ "feature": feature }),
            Self::StaleSpec { current } => json!({ "current": current }),
            Self::AdmissionUnsettled { correlation } => json!({ "correlation": correlation }),
            Self::DecisionOpen { decision_id } => json!({ "decision_id": decision_id }),
            Self::DecisionClosed { state } => json!({ "state": state }),
            Self::ConstructionRefused { kind } => json!({ "construction": kind }),
            Self::StaleAttempt {
                current: Some(current),
            } => json!({ "current": current }),
            Self::ActivationPending { attempt } => json!({ "attempt": attempt }),
            Self::ConsequencesUnavailable
            | Self::InvalidName
            | Self::PeerUnknown
            | Self::IdentityUnknown
            | Self::SelfChangeRefused
            | Self::OperationReused
            | Self::NotTransferring
            | Self::PredecessorMismatch
            | Self::ImportRefused
            | Self::PreparationNotPending
            | Self::PreparationMismatch
            | Self::ConstructionOpen
            | Self::StaleAttempt { current: None }
            | Self::NotConstructed
            | Self::ConstructionRequired
            | Self::IdentityTransferring
            | Self::TransferInFlight
            | Self::ObservationMismatch
            | Self::ApprovalUnavailable
            | Self::SubjectWidened
            | Self::DecisionNotFound
            | Self::EvidenceRequired
            | Self::SubjectChanged
            | Self::PresentationIncomplete
            | Self::RequesterCannotDecide
            | Self::ApproverNotEligible
            | Self::DecidedBeforePresentation
            | Self::ChangesHeld
            | Self::HostPreparationPending
            | Self::CandidateLaunch
            | Self::IdentityMember
            | Self::StorageUnsupported => json!({}),
        }
    }
}

/// Why an identity policy is refused, with where in it. The path is
/// relative to the policy object (`authority_fields.clearance.values`); the
/// display prefixes `identity_creation`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PolicyRefusal {
    /// Where in the policy, as a dotted path; empty for the whole policy.
    pub path: String,
    /// What is wrong there.
    pub kind: PolicyRefusalKind,
}

impl PolicyRefusal {
    /// A refusal of what is at `path`.
    pub fn new(path: impl Into<String>, kind: PolicyRefusalKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

impl fmt::Display for PolicyRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(formatter, "identity_creation: {}", self.kind)
        } else {
            write!(formatter, "identity_creation.{}: {}", self.path, self.kind)
        }
    }
}

impl std::error::Error for PolicyRefusal {}

/// What is wrong with an identity policy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyRefusalKind {
    /// The JSON does not have the policy's shape: an unknown key, a wrong
    /// type or a missing member. The detail is the parser's.
    Malformed { detail: String },
    /// A required list, map or text is empty.
    Empty,
    /// A value outside its alphabet or form.
    Invalid { value: String },
    /// The same entry appears twice.
    Duplicate { value: String },
    /// An authority field whose key is not a host-declared reserved label
    /// key.
    AuthorityFieldNotReserved { key: String },
    /// A form this release does not support.
    Unsupported { feature: String },
    /// A number outside its range.
    OutOfRange { value: u64, min: u64, max: u64 },
    /// An approval outcome that needs an approver names none.
    MissingApprover,
    /// An outcome that takes no approver names one.
    UnexpectedApprover,
    /// A review outcome names no reviewer.
    MissingReviewer,
    /// An outcome other than review names a reviewer or review rounds.
    UnexpectedReviewer,
    /// An approver names not exactly one of `id` and `label`.
    InvalidApprover,
    /// A rule lets an authority change through without a human decision.
    AuthorityChangeNeedsHumanDecision,
    /// A peer rule id the policy reserves.
    ReservedRuleId { id: String },
    /// The policy's preparation (`constructed`) holds a construction, which
    /// needs a native host composition, and the launch has none.
    NeedsNativeHost,
    /// The launch has no persistence (`persistent_state`), and the identity
    /// records and decisions a policy governs are durable: a policy, even a
    /// disabled one, is refused rather than ignored.
    NeedsPersistentLaunch,
    /// The policy is given both as `runtime_options.identity_creation` and
    /// by the native host composition.
    TwoSources,
}

impl fmt::Display for PolicyRefusalKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { detail } => write!(formatter, "malformed: {detail}"),
            Self::Empty => formatter.write_str("must not be empty"),
            Self::Invalid { value } => write!(formatter, "invalid value {value:?}"),
            Self::Duplicate { value } => write!(formatter, "duplicate {value:?}"),
            Self::AuthorityFieldNotReserved { key } => write!(
                formatter,
                "authority field {key:?} is not a reserved label key the host declares"
            ),
            Self::Unsupported { feature } => write!(formatter, "{feature} is not supported"),
            Self::OutOfRange { value, min, max } => {
                write!(formatter, "{value} is outside {min}..={max}")
            }
            Self::MissingApprover => formatter.write_str("this outcome needs an approver"),
            Self::UnexpectedApprover => formatter.write_str("this outcome takes no approver"),
            Self::MissingReviewer => formatter.write_str("a review needs a reviewer"),
            Self::UnexpectedReviewer => {
                formatter.write_str("only a review takes a reviewer and review rounds")
            }
            Self::InvalidApprover => {
                formatter.write_str("an approver is exactly one of {\"id\"} or {\"label\"}")
            }
            Self::AuthorityChangeNeedsHumanDecision => formatter.write_str(
                "an authority change needs require_approval or deny: only a human decision \
                 makes an authority field authority",
            ),
            Self::ReservedRuleId { id } => write!(formatter, "rule id {id:?} is reserved"),
            Self::NeedsNativeHost => formatter.write_str(
                "preparation \"constructed\" holds a construction, which needs a native host \
                 composition",
            ),
            Self::NeedsPersistentLaunch => formatter.write_str(
                "identity records are durable: the policy needs a persistent launch \
                 (persistent_state)",
            ),
            Self::TwoSources => formatter.write_str(
                "the policy is given both as runtime_options.identity_creation and by the \
                 native host composition; give one",
            ),
        }
    }
}
