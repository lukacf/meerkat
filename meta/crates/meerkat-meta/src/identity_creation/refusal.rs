//! The closed refusal vocabulary of identity requests (the wire's
//! `data.kind`), and the typed refusal of a creation policy.
//!
//! Every identity refusal is one kind of a closed list, with the request
//! field it concerns as a path (`identities[1].tools`). The list is closed:
//! a client must handle every kind, and a new kind is a vocabulary change.
//! This slice produces the policy kinds (template, request data, name,
//! peers, limits); the executor, the decision ingresses and Core's commit
//! path produce the others through the same type.

use std::fmt;

use serde_json::{Value, json};

use super::config::{ChangeClass, KickoffRule};

/// Why an identity request is refused: one kind of the closed vocabulary,
/// with the request field it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityRefusal {
    /// The kind, with its details.
    pub kind: IdentityRefusalKind,
    /// The request field the refusal concerns, as a path such as
    /// `identities[1].tools`; `None` for the request as a whole.
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

    /// The kind's stable token (`template_not_allowed`, ...).
    pub fn kind_token(&self) -> &'static str {
        self.kind.token()
    }

    /// Whether the same request may succeed later unchanged: the creation
    /// hold, pending host preparation, an unreadable creator, or consequence
    /// lines the host could not produce. Every other kind is permanent.
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

/// Who holds a name a request asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NameTakenBy {
    /// A declared identity.
    Declared,
    /// A created identity, or another identity of the same request.
    Created,
    /// A removed created identity: its name is a tombstone, never reused.
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

/// Which creation limit a request reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// Created identities in the mob.
    Total,
    /// Created identities of one creator.
    PerCreator,
    /// Peers of one created identity.
    PeersPerIdentity,
    /// Identities in one create request.
    Batch,
    /// Open decisions of one requester.
    OpenDecisionsPerCreator,
}

impl LimitKind {
    /// The stable token (`total`, `per_creator`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::PerCreator => "per_creator",
            Self::PeersPerIdentity => "peers_per_identity",
            Self::Batch => "batch",
            Self::OpenDecisionsPerCreator => "open_decisions_per_creator",
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
}

impl RequestProblem {
    /// The stable token (`empty`, `conflicting`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Conflicting => "conflicting",
        }
    }
}

/// One kind of the closed refusal vocabulary, with its details.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityRefusalKind {
    // Template.
    /// The template is unknown, or not one the requester may use.
    TemplateNotAllowed { template: String },
    /// The profile is not one the template binds; `None` when the request
    /// names none and the template has no default.
    ProfileNotAllowed { profile: Option<String> },
    /// The model is not one the template allows for the profile.
    ModelNotAllowed { model: String },
    /// The template's binding changed since the identity was sealed.
    TemplateBindingChanged,
    /// Requested tools outside the template's ceiling (strict mode).
    ExceedsTemplate { tools: Vec<String> },
    /// Requested tools the creating agent does not hold (strict mode).
    ExceedsCreator { tools: Vec<String> },
    // Request data.
    /// The request names a reserved label or context key.
    ReservedKey { key: String },
    /// An authority field under an outcome other than a human decision.
    AuthorityFieldNeedsApproval { key: String },
    /// An authority field the template does not declare, or a value its
    /// declaration does not allow.
    AuthorityFieldInvalid { key: String },
    /// A text larger than its limit, in bytes. Text is never cut.
    TextTooLarge { limit: u64 },
    /// The template's kickoff rule refuses the request's kickoff.
    KickoffRefused { rule: KickoffRule },
    /// The host's consequence lines could not be produced. The request is
    /// never rendered without them (retryable).
    ConsequencesUnavailable,
    /// The request is well typed but has nothing to act on, or contradicts
    /// itself.
    InvalidRequest { problem: RequestProblem },
    // Name.
    /// The slug does not follow the naming rule.
    InvalidName,
    /// The slug is reserved.
    ReservedName,
    /// The name is taken.
    NameTaken { by: NameTakenBy },
    // Peers.
    /// The peer rules refuse this peer.
    PeerNotAllowed { identity: String },
    /// The peer is not an identity the requester may name.
    PeerUnknown,
    // Limits and policy.
    /// A creation limit is reached.
    LimitReached { limit: LimitKind },
    /// More identities than one store transaction commits.
    BatchTooLarge { max: u64 },
    /// The host's validation hook refused the request; the reason is the
    /// host's, quoted.
    ValidationRefused { reason: String },
    /// An approval rule denies this change.
    CreationDenied { change: ChangeClass },
    // Custody and operations.
    /// No created identity has this name.
    IdentityUnknown,
    /// The requester does not hold custody of the identity.
    NotCreatorCustody,
    /// A created identity asked to change its own entry.
    SelfChangeRefused,
    /// The expected revision is not the current one.
    StaleRevision { current: u64 },
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
    /// The template's preparation is `constructed`: a settle must name the
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
    /// Creation is held (retryable).
    CreationHeld,
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
    // Per call.
    /// The creating agent cannot be resolved (retryable while it is only
    /// unreadable).
    CreatorUnresolved,
}

impl IdentityRefusalKind {
    /// The kind's stable token, the wire's `data.kind`.
    pub fn token(&self) -> &'static str {
        match self {
            Self::TemplateNotAllowed { .. } => "template_not_allowed",
            Self::ProfileNotAllowed { .. } => "profile_not_allowed",
            Self::ModelNotAllowed { .. } => "model_not_allowed",
            Self::TemplateBindingChanged => "template_binding_changed",
            Self::ExceedsTemplate { .. } => "exceeds_template",
            Self::ExceedsCreator { .. } => "exceeds_creator",
            Self::ReservedKey { .. } => "reserved_key",
            Self::AuthorityFieldNeedsApproval { .. } => "authority_field_needs_approval",
            Self::AuthorityFieldInvalid { .. } => "authority_field_invalid",
            Self::TextTooLarge { .. } => "text_too_large",
            Self::KickoffRefused { .. } => "kickoff_refused",
            Self::ConsequencesUnavailable => "consequences_unavailable",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::InvalidName => "invalid_name",
            Self::ReservedName => "reserved_name",
            Self::NameTaken { .. } => "name_taken",
            Self::PeerNotAllowed { .. } => "peer_not_allowed",
            Self::PeerUnknown => "peer_unknown",
            Self::LimitReached { .. } => "limit_reached",
            Self::BatchTooLarge { .. } => "batch_too_large",
            Self::ValidationRefused { .. } => "validation_refused",
            Self::CreationDenied { .. } => "creation_denied",
            Self::IdentityUnknown => "identity_unknown",
            Self::NotCreatorCustody => "not_creator_custody",
            Self::SelfChangeRefused => "self_change_refused",
            Self::StaleRevision { .. } => "stale_revision",
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
            Self::CreationHeld => "creation_held",
            Self::HostPreparationPending => "host_preparation_pending",
            Self::CandidateLaunch => "candidate_launch",
            Self::IdentityMember => "identity_member",
            Self::StorageUnsupported => "storage_unsupported",
            Self::ConstructionRefused { .. } => "construction_refused",
            Self::CreatorUnresolved => "creator_unresolved",
        }
    }

    /// Whether the same request may succeed later unchanged.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::CreationHeld
                | Self::HostPreparationPending
                | Self::CreatorUnresolved
                | Self::ConsequencesUnavailable
        )
    }

    /// The kind's details, as the members of the wire's `error.data` beside
    /// `kind` and `field`.
    fn details(&self) -> Value {
        match self {
            Self::TemplateNotAllowed { template } => json!({ "template": template }),
            Self::ProfileNotAllowed { profile } => json!({ "profile": profile }),
            Self::ModelNotAllowed { model } => json!({ "model": model }),
            Self::ExceedsTemplate { tools } | Self::ExceedsCreator { tools } => {
                json!({ "tools": tools })
            }
            Self::ReservedKey { key }
            | Self::AuthorityFieldNeedsApproval { key }
            | Self::AuthorityFieldInvalid { key } => json!({ "key": key }),
            Self::TextTooLarge { limit } => json!({ "limit": limit }),
            Self::KickoffRefused { rule } => json!({ "rule": rule.token() }),
            Self::InvalidRequest { problem } => json!({ "problem": problem.token() }),
            Self::NameTaken { by } => json!({ "by": by.token() }),
            Self::PeerNotAllowed { identity } => json!({ "identity": identity }),
            Self::LimitReached { limit } => json!({ "limit": limit.token() }),
            Self::BatchTooLarge { max } => json!({ "max": max }),
            Self::ValidationRefused { reason } => json!({ "reason": reason }),
            Self::CreationDenied { change } => json!({ "change": change.token() }),
            Self::StaleRevision { current } => json!({ "current": current }),
            Self::AdmissionUnsettled { correlation } => json!({ "correlation": correlation }),
            Self::DecisionOpen { decision_id } => json!({ "decision_id": decision_id }),
            Self::DecisionClosed { state } => json!({ "state": state }),
            Self::ConstructionRefused { kind } => json!({ "construction": kind }),
            Self::StaleAttempt {
                current: Some(current),
            } => json!({ "current": current }),
            Self::ActivationPending { attempt } => json!({ "attempt": attempt }),
            Self::TemplateBindingChanged
            | Self::ConsequencesUnavailable
            | Self::InvalidName
            | Self::ReservedName
            | Self::PeerUnknown
            | Self::IdentityUnknown
            | Self::NotCreatorCustody
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
            | Self::CreationHeld
            | Self::HostPreparationPending
            | Self::CandidateLaunch
            | Self::IdentityMember
            | Self::StorageUnsupported
            | Self::CreatorUnresolved => json!({}),
        }
    }
}

/// Why a creation policy is refused, with where in it. The path is relative
/// to the policy object (`templates.research.tools.floor`); the display
/// prefixes `identity_creation`.
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

/// What is wrong with a creation policy.
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
    /// A template this policy does not declare.
    UnknownTemplate { template: String },
    /// A profile the mob definition does not declare.
    UnknownProfile { profile: String },
    /// A floor tool outside the template's ceiling.
    FloorOutsideCeiling { tool: String },
    /// A creation tool in a template that does not grant creation.
    CreationToolWithoutGrant { tool: String },
    /// An authority field whose key is not a host-declared reserved label
    /// key.
    AuthorityFieldNotReserved { key: String },
    /// A form this release does not support.
    Unsupported { feature: String },
    /// A number outside its range.
    OutOfRange { value: u64, min: u64, max: u64 },
    /// A preamble placeholder other than `creator`, `peers`,
    /// `display_name` and `purpose`, or an unbalanced brace.
    InvalidPlaceholder { placeholder: String },
    /// An approval outcome that needs an approver names none.
    MissingApprover,
    /// An outcome that takes no approver names one.
    UnexpectedApprover,
    /// A review outcome names no reviewer.
    MissingReviewer,
    /// An outcome other than review names a reviewer or review rounds.
    UnexpectedReviewer,
    /// An approver names not exactly one of `id`, `label` and
    /// `creator_identity: true`.
    InvalidApprover,
    /// A rule lets an authority change through without a human decision.
    AuthorityChangeNeedsHumanDecision,
    /// A peer rule id the policy reserves.
    ReservedRuleId { id: String },
    /// A template's preparation mode needs a native host composition, and
    /// the launch has none.
    NeedsNativeHost { template: String },
    /// The launch has no persistence (`persistent_state`), and created
    /// identities are durable: a policy, even a disabled one, is refused
    /// rather than ignored.
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
            Self::UnknownTemplate { template } => {
                write!(formatter, "unknown template {template:?}")
            }
            Self::UnknownProfile { profile } => {
                write!(
                    formatter,
                    "profile {profile:?} is not in the mob definition"
                )
            }
            Self::FloorOutsideCeiling { tool } => {
                write!(formatter, "floor tool {tool:?} is outside the ceiling")
            }
            Self::CreationToolWithoutGrant { tool } => write!(
                formatter,
                "creation tool {tool:?} needs may_grant_creation on the template"
            ),
            Self::AuthorityFieldNotReserved { key } => write!(
                formatter,
                "authority field {key:?} is not a reserved label key the host declares"
            ),
            Self::Unsupported { feature } => write!(formatter, "{feature} is not supported"),
            Self::OutOfRange { value, min, max } => {
                write!(formatter, "{value} is outside {min}..={max}")
            }
            Self::InvalidPlaceholder { placeholder } => write!(
                formatter,
                "invalid placeholder {placeholder:?}: use creator, peers, display_name or \
                 purpose in braces, and double a literal brace"
            ),
            Self::MissingApprover => formatter.write_str("this outcome needs an approver"),
            Self::UnexpectedApprover => formatter.write_str("this outcome takes no approver"),
            Self::MissingReviewer => formatter.write_str("a review needs a reviewer"),
            Self::UnexpectedReviewer => {
                formatter.write_str("only a review takes a reviewer and review rounds")
            }
            Self::InvalidApprover => formatter.write_str(
                "an approver is exactly one of {\"id\"}, {\"label\"} or \
                 {\"creator_identity\": true}",
            ),
            Self::AuthorityChangeNeedsHumanDecision => formatter.write_str(
                "an authority change needs require_approval or deny: only a human decision \
                 makes an authority field authority",
            ),
            Self::ReservedRuleId { id } => write!(formatter, "rule id {id:?} is reserved"),
            Self::NeedsNativeHost { template } => write!(
                formatter,
                "template {template:?} uses preparation \"constructed\", which needs a native \
                 host composition"
            ),
            Self::NeedsPersistentLaunch => formatter.write_str(
                "created identities are durable: identity creation needs a persistent launch \
                 (persistent_state)",
            ),
            Self::TwoSources => formatter.write_str(
                "the policy is given both as runtime_options.identity_creation and by the \
                 native host composition; give one",
            ),
        }
    }
}
