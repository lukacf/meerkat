//! The closed refusal vocabulary of identity requests (the wire's
//! `data.kind`), and the typed refusal of a creation policy.
//!
//! Every identity refusal is one kind of a closed list, with the request
//! field it concerns as a path (`agents[1].labels`). The list is
//! closed: a client must handle every kind, and a new kind is a vocabulary
//! change. It is one vocabulary for every identity, and no kind or detail
//! names where an identity's record lives. Normalization produces the request
//! kinds (request data, name, peers, policy); the executor, the decision
//! ingresses and Core's commit path produce the others through the same
//! type: every refusal of Core's change path has its kind here, under Core's
//! own token (`executor::refusal_of_core`).
//!
//! A refusal is what a typed outcome's `refused {kind, field?, retryable,
//! detail}` carries ([`IdentityRefusal::to_refused`]): the kind, the request
//! field, whether the same request may succeed later unchanged, and one
//! sentence a caller, a model included, can act on.

use std::fmt;

use serde_json::{Value, json};

use super::config::ChangeClass;
use super::digest::Sha256Digest;

/// Why an identity request is refused: one kind of the closed vocabulary,
/// with the request field it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityRefusal {
    /// The kind, with its details.
    pub kind: IdentityRefusalKind,
    /// The request field the refusal concerns, as a path such as
    /// `agents[1].labels`; `None` for the request as a whole.
    pub field: Option<String>,
    /// What was wrong and what to send instead, in one sentence, when the
    /// refusal's source gave one (Core's commit path does). `None` leaves
    /// [`Self::detail`] to describe the kind.
    pub detail: Option<String>,
}

impl IdentityRefusal {
    /// A refusal of the request as a whole.
    pub fn new(kind: IdentityRefusalKind) -> Self {
        Self {
            kind,
            field: None,
            detail: None,
        }
    }

    /// This refusal, about the request field `field`.
    #[must_use]
    pub fn at(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// This refusal, with `detail` as what was wrong and what to send
    /// instead.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The kind's stable token (`reserved_key`, ...).
    pub fn kind_token(&self) -> &'static str {
        self.kind.token()
    }

    /// Whether the same request may succeed later unchanged
    /// ([`IdentityRefusalKind::is_retryable`]). Every other kind is
    /// permanent.
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// What was wrong and what to send instead: the sentence the refusal's
    /// source gave, or the kind's own ([`IdentityRefusalKind::describe`]).
    pub fn detail(&self) -> String {
        self.detail
            .clone()
            .unwrap_or_else(|| self.kind.describe().to_owned())
    }

    /// The refusal as a typed outcome carries it: [`Self::to_data`] with
    /// `retryable` and `detail`. This is the `refused {kind, field?,
    /// retryable, detail}` of every surface.
    pub fn to_refused(&self) -> Value {
        let mut data = self.to_data();
        if let Value::Object(map) = &mut data {
            map.insert("retryable".to_owned(), Value::from(self.is_retryable()));
            map.insert("detail".to_owned(), Value::from(self.detail()));
        }
        data
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

/// Who holds a name a request asked for. It says nothing else about the
/// holder: not where its record lives, and not whether the requester may see
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NameTakenBy {
    /// An identity of the mob, or another identity of the same request.
    Identity,
    /// A removed identity: its name is a tombstone, never reused.
    Removed,
    /// A worker member that is no identity.
    Worker,
    /// An entry this build cannot read.
    Unreadable,
}

impl NameTakenBy {
    /// The stable token (`identity`, `removed`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Removed => "removed",
            Self::Worker => "worker",
            Self::Unreadable => "unreadable",
        }
    }
}

/// Why an adoption takes no agent of a shape (`not_adoptable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum NotAdoptableReason {
    /// The agent is changed at run time already: there is nothing to adopt.
    NothingToAdopt,
    /// The name is no agent an adoption takes: nothing holds it, or what
    /// holds it is not what the adoption named.
    NotAnAgent,
    /// The agent's member lives on a member host.
    Placed,
    /// The agent's member is bound to no session.
    NoSession,
    /// The agent is hosted on its first use: retryable once its first use
    /// seated it.
    HostedOnFirstUse,
    /// The agent awaits its held creation: retryable once its host released
    /// it.
    AwaitingHeldCreation,
    /// The agent awaits its host's preparation: retryable once it settled.
    AwaitingHostPreparation,
    /// The member was spawned with a profile or model override, which an
    /// agent's spec does not carry.
    ProfileOverride,
}

impl NotAdoptableReason {
    /// The stable token (`nothing_to_adopt`, `placed`, ...).
    pub fn token(self) -> &'static str {
        match self {
            Self::NothingToAdopt => "nothing_to_adopt",
            Self::NotAnAgent => "not_an_agent",
            Self::Placed => "placed",
            Self::NoSession => "no_session",
            Self::HostedOnFirstUse => "hosted_on_first_use",
            Self::AwaitingHeldCreation => "awaiting_held_creation",
            Self::AwaitingHostPreparation => "awaiting_host_preparation",
            Self::ProfileOverride => "profile_override",
        }
    }

    /// Whether an adoption refused for this reason may commit later
    /// unchanged: only the host's own step settles it (the first use, the
    /// held creation's release, the preparation).
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::HostedOnFirstUse | Self::AwaitingHeldCreation | Self::AwaitingHostPreparation
        )
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
    // Profile.
    /// The profile is not one of the mob definition's profiles.
    ProfileUnknown { profile: String },
    /// The requester's grant does not admit the profile, as its admission
    /// checks it. Normalization never refuses it.
    ProfileNotAllowed { profile: String },
    /// The spec sets a field only a grant with manage scope admits.
    PrivilegedField { field: String },
    /// The requester's grant no longer admits the change at its commit.
    GrantRevoked,
    // Request data.
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
    /// The requester is a member private to a person: such a member asks
    /// for no change of agents.
    PrivateRequester,
    /// The spec the requester read, named by its digest, is not the current
    /// one: `current_digest` is, with its revision for an identity Core's
    /// store holds.
    StaleSpec {
        current_digest: Sha256Digest,
        current_revision: Option<u64>,
    },
    /// The operation id was used with another request.
    OperationReused,
    /// The agent is declared by its host's configuration, and a run-time
    /// change does not write a host's configuration: change it there, or
    /// release the agent from configuration first (`adopt`), after which
    /// it is changed at run time.
    OwnerReadOnly,
    /// An adoption takes no agent of this shape.
    NotAdoptable { reason: NotAdoptableReason },
    /// The agent is not archived: there is nothing to unarchive.
    NotArchived,
    /// The host's configuration is being applied to the mob: an adoption
    /// waits for it (retryable).
    DeclaredApplyOpen,
    /// A spawn or held spawn of the name is not settled.
    AdmissionUnsettled { correlation: String },
    /// The member an adoption names is not the one the request observed:
    /// its generation moved.
    PredecessorMismatch,
    /// The import is refused.
    ImportRefused,
    /// The purge of the mob's records is refused: `reason` is Core's stable
    /// token for why.
    PurgeRefused { reason: String },
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
    /// The mob is not running.
    MobNotRunning,
    /// The mob's records were purged before a downgrade: its changes are
    /// refused until it restarts.
    Purged,
    /// Core refused the held construction.
    ConstructionRefused { kind: String },
}

impl IdentityRefusalKind {
    /// The kind's stable token, the wire's `data.kind`.
    pub fn token(&self) -> &'static str {
        match self {
            Self::ProfileUnknown { .. } => "profile_unknown",
            Self::ProfileNotAllowed { .. } => "profile_not_allowed",
            Self::PrivilegedField { .. } => "privileged_field",
            Self::GrantRevoked => "grant_revoked",
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
            Self::PrivateRequester => "private_requester",
            Self::StaleSpec { .. } => "stale_spec",
            Self::OperationReused => "operation_reused",
            Self::OwnerReadOnly => "owner_read_only",
            Self::NotAdoptable { .. } => "not_adoptable",
            Self::NotArchived => "not_archived",
            Self::DeclaredApplyOpen => "declared_apply_open",
            Self::AdmissionUnsettled { .. } => "admission_unsettled",
            Self::PredecessorMismatch => "predecessor_mismatch",
            Self::ImportRefused => "import_refused",
            Self::PurgeRefused { .. } => "purge_refused",
            Self::PreparationNotPending => "preparation_not_pending",
            Self::PreparationMismatch => "preparation_mismatch",
            Self::ConstructionOpen => "construction_open",
            Self::StaleAttempt { .. } => "stale_attempt",
            Self::NotConstructed => "not_constructed",
            Self::ConstructionRequired => "construction_required",
            Self::ActivationPending { .. } => "activation_pending",
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
            Self::MobNotRunning => "mob_not_running",
            Self::Purged => "purged",
            Self::ConstructionRefused { .. } => "construction_refused",
        }
    }

    /// Whether the same request may succeed later unchanged: changes held,
    /// host preparation pending, consequence lines the host could not
    /// produce, a member step or a declaration in flight, and an adoption of
    /// an agent its host still settles.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ChangesHeld
            | Self::HostPreparationPending
            | Self::ConsequencesUnavailable
            | Self::AdmissionUnsettled { .. }
            | Self::DeclaredApplyOpen => true,
            Self::NotAdoptable { reason } => reason.is_retryable(),
            _ => false,
        }
    }

    /// What was wrong and what to send instead, in one sentence a caller, a
    /// model included, can act on. The kind's details ([`Self::token`]'s
    /// payload) carry the values the sentence refers to.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::ProfileUnknown { .. } => {
                "The mob has no profile of this name: name one of the mob's profiles."
            }
            Self::ProfileNotAllowed { .. } => {
                "Your grant does not admit this profile: ask for a profile your grant covers."
            }
            Self::PrivilegedField { .. } => {
                "The spec sets a field that needs manage scope: leave the field out."
            }
            Self::GrantRevoked => {
                "Your grant no longer admits this change: it was not made, and asking again \
                 needs a grant that admits it."
            }
            Self::ReservedKey { .. } => "The request names a reserved label key: use another key.",
            Self::AuthorityFieldNeedsApproval { .. } => {
                "This label is an authority field, which only a human decision sets: leave it \
                 out, or ask under a rule that requires approval."
            }
            Self::AuthorityFieldInvalid { .. } => {
                "The authority field is not declared, or its value is not one the host allows."
            }
            Self::TextTooLarge { .. } => {
                "A text is larger than its limit in bytes: shorten it. Text is never cut."
            }
            Self::ConsequencesUnavailable => {
                "The host could not state the request's consequences: retry the same request \
                 later."
            }
            Self::InvalidRequest { .. } => {
                "The request has nothing to act on, contradicts itself or holds a malformed \
                 value: correct the named field."
            }
            Self::InvalidName => "The name is not a valid agent name: choose another.",
            Self::NameTaken { .. } => {
                "The name is taken: choose another. A removed agent's name is never available \
                 again."
            }
            Self::PeerNotAllowed { .. } => "The host's peer rules refuse this peer: leave it out.",
            Self::PeerUnknown => {
                "The peer is no agent you may name: list agents of the mob, or other agents of \
                 the same request."
            }
            Self::BatchTooLarge { .. } => {
                "The request names more agents than one change commits: split it."
            }
            Self::ValidationRefused { .. } => {
                "The host's own check refused the request for the quoted reason."
            }
            Self::ChangeDenied { .. } => {
                "The host's rules deny this change for you: it was not made, and repeating it \
                 changes nothing."
            }
            Self::Unsupported { .. } => {
                "The request needs something this release does not support: leave it out."
            }
            Self::IdentityUnknown => "No agent has this name: list the agents and name one.",
            Self::SelfChangeRefused => "An agent never changes itself: name another agent.",
            Self::PrivateRequester => {
                "A member private to a person asks for no change of agents: nothing was made."
            }
            Self::StaleSpec { .. } => {
                "The agent changed since you read it: read it again and send the current spec \
                 digest."
            }
            Self::OperationReused => {
                "This operation id was used with another request: send a new id for a new \
                 request, or the same request to read its result."
            }
            Self::OwnerReadOnly => {
                "This agent is declared by the host's configuration, and a run-time change does \
                 not write the host's configuration: change it there, or release the agent from \
                 configuration first (adopt), after which it is changed at run time."
            }
            Self::NotAdoptable { .. } => {
                "An adoption does not take this agent, for the reason given."
            }
            Self::NotArchived => "The agent is not archived: there is nothing to unarchive.",
            Self::DeclaredApplyOpen => {
                "The host's configuration is being applied: retry the same request shortly."
            }
            Self::AdmissionUnsettled { .. } => {
                "A step of this agent is still in flight: retry the same request once it settled."
            }
            Self::PredecessorMismatch => {
                "The member is not the one the request observed: observe it again."
            }
            Self::ImportRefused => "The import is refused: only the host operator imports.",
            Self::PurgeRefused { .. } => "The purge is refused, for the reason given.",
            Self::PreparationNotPending => "No host preparation is pending for the agent.",
            Self::PreparationMismatch => "The preparation settle names another operation.",
            Self::ConstructionOpen => "A held construction is open for the agent.",
            Self::StaleAttempt { .. } => "The settle names an attempt that is not the current one.",
            Self::NotConstructed => "The agent has no held construction.",
            Self::ConstructionRequired => {
                "The agent's preparation holds a construction: a settle names the held attempt."
            }
            Self::ActivationPending { .. } => {
                "The attempt's activation is in flight: nothing aborts or replaces it."
            }
            Self::ApprovalUnavailable => {
                "The change needs a decision and nobody can be asked to decide it: nothing was \
                 written."
            }
            Self::DecisionOpen { .. } => {
                "A decision is already open for this agent: wait for its outcome."
            }
            Self::SubjectWidened => {
                "The request is no longer what was approved: ask again with the current request."
            }
            Self::DecisionClosed { .. } => "The decision has ended: ask again.",
            Self::DecisionNotFound => "No such decision.",
            Self::EvidenceRequired => "The decide carries no attested evidence.",
            Self::SubjectChanged => "The presented digest is not the decision's subject digest.",
            Self::PresentationIncomplete => "The presentation did not cover every part.",
            Self::RequesterCannotDecide => "The requester cannot decide its own request.",
            Self::ApproverNotEligible => "The approver is not eligible.",
            Self::DecidedBeforePresentation => {
                "The decide was observed before the decision was first presented."
            }
            Self::ChangesHeld => {
                "The host holds changes of agents for now: retry the same request later."
            }
            Self::HostPreparationPending => {
                "The host is still preparing the agent: retry the same request later."
            }
            Self::CandidateLaunch => "A candidate launch changes nothing.",
            Self::IdentityMember => {
                "The member is a long-term agent: the worker tools do not act on it."
            }
            Self::StorageUnsupported => {
                "The mob's storage cannot commit a change of agents in one transaction."
            }
            Self::MobNotRunning => "The mob is not running.",
            Self::Purged => {
                "The mob's records were purged before a downgrade: changes are refused until \
                 it restarts."
            }
            Self::ConstructionRefused { .. } => "The held construction could not be made.",
        }
    }

    /// The kind's details, as the members of the wire's `error.data` beside
    /// `kind` and `field`.
    fn details(&self) -> Value {
        match self {
            Self::ProfileUnknown { profile } | Self::ProfileNotAllowed { profile } => {
                json!({ "profile": profile })
            }
            Self::PrivilegedField { field } => json!({ "privileged": field }),
            Self::NotAdoptable { reason } => json!({ "reason": reason.token() }),
            Self::PurgeRefused { reason } => json!({ "reason": reason }),
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
            Self::StaleSpec {
                current_digest,
                current_revision: Some(revision),
            } => json!({ "current_digest": current_digest.as_str(), "current_revision": revision }),
            Self::StaleSpec {
                current_digest,
                current_revision: None,
            } => json!({ "current_digest": current_digest.as_str() }),
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
            | Self::PrivateRequester
            | Self::OperationReused
            | Self::GrantRevoked
            | Self::OwnerReadOnly
            | Self::NotArchived
            | Self::DeclaredApplyOpen
            | Self::MobNotRunning
            | Self::Purged
            | Self::PredecessorMismatch
            | Self::ImportRefused
            | Self::PreparationNotPending
            | Self::PreparationMismatch
            | Self::ConstructionOpen
            | Self::StaleAttempt { current: None }
            | Self::NotConstructed
            | Self::ConstructionRequired
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
    /// type or a missing member, where the detail is the parser's; or a
    /// requester selector whose members contradict each other, so that it
    /// selects nobody, where the detail says which.
    Malformed { detail: String },
    /// A required list, map or text is empty.
    Empty,
    /// A value outside its alphabet or form.
    Invalid { value: String },
    /// The same entry appears twice.
    Duplicate { value: String },
    /// One of Meta's own label keys, which a policy neither reserves nor
    /// declares an authority field.
    MetaLabelKey { key: String },
    /// An authority field whose key is not reserved: neither listed in
    /// `reserved_keys.labels` nor read by a selector.
    AuthorityFieldNotReserved { key: String },
    /// A profile the mob definition does not declare.
    UnknownProfile { profile: String },
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
    /// A profile's preparation (`constructed`) holds a construction, which
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
            Self::MetaLabelKey { key } => write!(
                formatter,
                "label key {key:?} is Meta's own: Meta sets it, and a policy neither reserves \
                 it nor declares it an authority field"
            ),
            Self::AuthorityFieldNotReserved { key } => write!(
                formatter,
                "authority field {key:?} is not a reserved label key: list it in \
                 reserved_keys.labels"
            ),
            Self::UnknownProfile { profile } => {
                write!(
                    formatter,
                    "profile {profile:?} is not in the mob definition"
                )
            }
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
