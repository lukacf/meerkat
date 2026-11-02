//! Changes of an identity through its one durable owner (0.9.9).
//!
//! An identity has one durable owner, its custody: a roster-owned
//! identity's record is its host's roster, which the host declares again at
//! each apply ([`super::DeclaredRoster`]); a store-owned identity's record is
//! its row in Core's identity store ([`crate::CreatedIdentityRecord`]).
//! Custody decides only where the record lives, which recovery reads it at a
//! start, which writer persists a change of it, and the name reservations. A
//! store-owned identity's member is a member like any other: it is seated
//! from its spec exactly as a roster-owned identity of the same spec is, with
//! its profile's tools, its labels and the definition's wiring, and it is a
//! host root whatever asked for it.
//!
//! This module is the store-owned writer: the commit of a create
//! ([`super::MobHandle::create_store_owned_identities`]) writes each
//! identity's row (revision 1, its spec and provenance), the journal rows of
//! its seat and the change's receipt in one store transaction, all or
//! nothing, on the built-in storages
//! ([`crate::store::MobIdentityMemberStore`]); a custom storage composition
//! refuses every change. MobMachine decides the change's admission
//! (`AdmitIdentityChange`: a candidate composition, a replay, another
//! request under the same operation, a changes hold, the consent's permit
//! and the requester's grant) and records each identity's entry
//! (`RecordCreatedIdentityEntry`) in the same prepared step as the seats, so
//! a refusal of any part leaves nothing written and nothing recorded.
//!
//! An agent requester's change is admitted by the ordinary spawn grant
//! ([`super::MobHandle::admit_identity_change_grant`]): the live grant of
//! the requesting member session, read through the runtime's receiver,
//! answers MobMachine's spawn admission for each profile the change names, as
//! the spawn tools ask it. The admission prepares the change's
//! [`PreparedOperationCheck`], bound to the operation and the requester
//! identity, whose owner answers from that identity's current grant (its
//! current profile and the host's build mint, [`MemberGrantMint`]), for a
//! dormant identity too, without waking it. The commit is the final sink:
//! it checks the prepared check current (re-preparing it for a decided
//! change, which waited for its decision), and a grant revoked meanwhile
//! ends the change [`IdentityChangeRefusal::GrantRevoked`] with nothing
//! written. Dormancy, a new activation and a session reset of the requester
//! revoke nothing: the check reads its identity, never its session. Host and
//! human requests carry no grant.

use meerkat_core::SessionId;
use meerkat_core::approval::decision::ConsentProvenance;
use meerkat_core::authorization::PreparedOperationCheck;
use meerkat_core::service::{LiveMobIncarnation, MobToolAuthorityContext};
use meerkat_runtime::decision::DecisionExecutionPermit;

use crate::MobError;
use crate::identity::{
    CreatedIdentitySpec, IdentityChangeReceipt, IdentityPolicyOutcome, IdentityPreparationMode,
    IdentitySourceEvidence,
};
use crate::ids::{AgentIdentity, MobId, ProfileName};

/// One identity change's operation: the id its caller chose and the
/// canonical digest of its normalized request. A retry with the same id and
/// digest replays the committed change; another digest under the same id is
/// refused ([`IdentityChangeRefusal::OperationReused`]), and nothing changes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityOperation {
    pub operation_id: String,
    pub request_digest: String,
}

impl IdentityOperation {
    #[must_use]
    pub fn new(operation_id: impl Into<String>, request_digest: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            request_digest: request_digest.into(),
        }
    }
}

/// Who asks for an identity change, as its commit takes it. Recorded in the
/// identity's provenance and the change's receipt; an agent's change carries
/// its grant, which the commit checks again, and nothing else is authority.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum IdentityChangeRequesterInput {
    /// A member agent of this mob, by its grant for this change
    /// ([`super::MobHandle::admit_identity_change_grant`]).
    Agent { grant: IdentityChangeGrant },
    /// The host, on one of its surfaces. `host_operator` says the caller is
    /// the host's operator: only an operator's change commits under
    /// imported consent.
    Host {
        surface: String,
        host_operator: bool,
    },
    /// An authenticated human principal, through a host surface.
    Human { principal: String },
}

impl IdentityChangeRequesterInput {
    /// A host surface's request, not the operator's.
    #[must_use]
    pub fn host(surface: impl Into<String>) -> Self {
        Self::Host {
            surface: surface.into(),
            host_operator: false,
        }
    }

    /// The host operator's request on `surface`.
    #[must_use]
    pub fn host_operator(surface: impl Into<String>) -> Self {
        Self::Host {
            surface: surface.into(),
            host_operator: true,
        }
    }
}

/// The consent a change commits under.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum IdentityChangeConsentInput<'a> {
    /// The host's change rule committed it without a decision.
    Policy(IdentityPolicyOutcome),
    /// A human decision, through its execution permit: the commit refuses a
    /// permit that does not bind the change's operation and request digest
    /// ([`IdentityChangeRefusal::SubjectWidened`]) or is past its deadline
    /// ([`IdentityChangeRefusal::DecisionClosed`]), and seals the permit's
    /// consent. The caller settles the permit after the commit.
    Decided(&'a DecisionExecutionPermit),
    /// Consent decided outside Meerkat, carried by a host operator's import
    /// only ([`IdentityChangeRefusal::ImportRefused`] otherwise).
    Imported(&'a ConsentProvenance),
}

/// One identity a store-owned create commits: its declared spec, and how its
/// host prepares it before its first use (the host's rule for the spec's
/// profile).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreatedIdentityRequest {
    pub spec: CreatedIdentitySpec,
    pub preparation: IdentityPreparationMode,
}

impl CreatedIdentityRequest {
    /// `spec`, with no host preparation.
    #[must_use]
    pub fn new(spec: CreatedIdentitySpec) -> Self {
        Self {
            spec,
            preparation: IdentityPreparationMode::None,
        }
    }

    #[must_use]
    pub fn with_preparation(mut self, preparation: IdentityPreparationMode) -> Self {
        self.preparation = preparation;
        self
    }
}

/// A create of 1 to [`crate::identity::IDENTITY_CHANGE_BATCH_MAX`] store-owned identities,
/// committed as one change: one store transaction, all or nothing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StoreOwnedIdentityCreate {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    /// Who drafted the request, when it was not the requester: recorded,
    /// never authority.
    pub proposer: Option<String>,
    /// What a trusted host ingress attested about the request's source:
    /// recorded, never authority.
    pub source_evidence: Option<IdentitySourceEvidence>,
    pub identities: Vec<CreatedIdentityRequest>,
}

impl StoreOwnedIdentityCreate {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        identities: Vec<CreatedIdentityRequest>,
    ) -> Self {
        Self {
            operation,
            requester,
            proposer: None,
            source_evidence: None,
            identities,
        }
    }

    #[must_use]
    pub fn with_proposer(mut self, proposer: impl Into<String>) -> Self {
        self.proposer = Some(proposer.into());
        self
    }

    #[must_use]
    pub fn with_source_evidence(mut self, evidence: IdentitySourceEvidence) -> Self {
        self.source_evidence = Some(evidence);
        self
    }
}

/// What a committed change answers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityChangeOutcome {
    /// The change committed now; its receipt.
    Committed { receipt: IdentityChangeReceipt },
    /// The operation committed before with the same request: its receipt.
    /// Nothing was written again.
    Replayed { receipt: IdentityChangeReceipt },
}

impl IdentityChangeOutcome {
    /// The change's receipt, committed now or before.
    #[must_use]
    pub fn receipt(&self) -> &IdentityChangeReceipt {
        match self {
            Self::Committed { receipt } | Self::Replayed { receipt } => receipt,
        }
    }
}

/// Who holds a name a change wanted ([`IdentityChangeRefusal::NameTaken`]).
/// It says nothing else about the holder: not its custody, not whether the
/// requester may see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IdentityNameHolder {
    /// An identity of either custody.
    Identity,
    /// A removed identity: the name is its tombstone.
    Removed,
    /// A worker member, or a spawn under way.
    Worker,
    /// An identity whose row this build cannot read.
    Unreadable,
}

impl IdentityNameHolder {
    /// The holder's stable token (`identity`, `removed`, `worker`,
    /// `unreadable`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Removed => "removed",
            Self::Worker => "worker",
            Self::Unreadable => "unreadable",
        }
    }
}

/// Why an identity change was refused. Nothing of a refused change is
/// written. Each refusal has a stable kind token ([`Self::kind`]), and only
/// [`Self::ChangesHeld`] is retryable unchanged ([`Self::is_retryable`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityChangeRefusal {
    /// The request is malformed: no identity, an identity named twice, an
    /// invalid spec, operation id or request digest.
    InvalidRequest { problem: String },
    /// More identities than one change commits.
    BatchTooLarge { max: usize },
    /// The spec names a profile the mob does not have.
    ProfileUnknown { profile: ProfileName },
    /// The requester's grant does not admit the profile.
    ProfileNotAllowed { profile: ProfileName },
    /// The spec sets a field only a grant with manage scope admits
    /// (`runtime_mode_override`, `backend`, `binding`, `placement`).
    PrivilegedField { field: &'static str },
    /// The requester's grant no longer admits the change at its commit.
    GrantRevoked,
    /// The name is held.
    NameTaken {
        identity: AgentIdentity,
        by: IdentityNameHolder,
    },
    /// Another request committed under the operation id.
    OperationReused { operation_id: String },
    /// The mob's identity changes are held: retry after the hold ends.
    ChangesHeld,
    /// A candidate composition writes nothing.
    CandidateLaunch,
    /// The mob's storage composition commits no identity change (a custom
    /// composition without the built-in identity and member store).
    StorageUnsupported,
    /// The mob is not running.
    MobNotRunning,
    /// The decision's permit binds another operation or request.
    SubjectWidened,
    /// The decision cannot be executed: its permit is past its deadline
    /// (`state` is `expired`).
    DecisionClosed { state: &'static str },
    /// Imported consent comes only with a host operator's import.
    ImportRefused,
    /// The change asks for something this build does not support yet.
    Unsupported { feature: String },
}

impl IdentityChangeRefusal {
    /// The refusal's stable kind token.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "invalid_request",
            Self::BatchTooLarge { .. } => "batch_too_large",
            Self::ProfileUnknown { .. } => "profile_unknown",
            Self::ProfileNotAllowed { .. } => "profile_not_allowed",
            Self::PrivilegedField { .. } => "privileged_field",
            Self::GrantRevoked => "grant_revoked",
            Self::NameTaken { .. } => "name_taken",
            Self::OperationReused { .. } => "operation_reused",
            Self::ChangesHeld => "changes_held",
            Self::CandidateLaunch => "candidate_launch",
            Self::StorageUnsupported => "storage_unsupported",
            Self::MobNotRunning => "mob_not_running",
            Self::SubjectWidened => "subject_widened",
            Self::DecisionClosed { .. } => "decision_closed",
            Self::ImportRefused => "import_refused",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// Whether the same request may commit later unchanged: only a changes
    /// hold ends by itself.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ChangesHeld)
    }
}

impl std::fmt::Display for IdentityChangeRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest { problem } => write!(formatter, "invalid request: {problem}"),
            Self::BatchTooLarge { max } => {
                write!(formatter, "a change commits at most {max} identities")
            }
            Self::ProfileUnknown { profile } => {
                write!(formatter, "the mob has no profile '{profile}'")
            }
            Self::ProfileNotAllowed { profile } => write!(
                formatter,
                "the requester's grant does not admit the profile '{profile}'"
            ),
            Self::PrivilegedField { field } => write!(
                formatter,
                "the spec sets '{field}', which only a grant with manage scope admits"
            ),
            Self::GrantRevoked => formatter
                .write_str("the requester's grant no longer admits the change at its commit"),
            Self::NameTaken { identity, by } => write!(
                formatter,
                "the name '{identity}' is taken (by {})",
                by.as_str()
            ),
            Self::OperationReused { operation_id } => write!(
                formatter,
                "another request committed under the operation '{operation_id}'"
            ),
            Self::ChangesHeld => formatter.write_str("the mob's identity changes are held"),
            Self::CandidateLaunch => formatter.write_str("a candidate composition writes nothing"),
            Self::StorageUnsupported => {
                formatter.write_str("the mob's storage composition commits no identity change")
            }
            Self::MobNotRunning => formatter.write_str("the mob is not running"),
            Self::SubjectWidened => {
                formatter.write_str("the decision's permit binds another operation or request")
            }
            Self::DecisionClosed { state } => {
                write!(formatter, "the decision cannot be executed: {state}")
            }
            Self::ImportRefused => {
                formatter.write_str("imported consent comes only with a host operator's import")
            }
            Self::Unsupported { feature } => write!(formatter, "unsupported: {feature}"),
        }
    }
}

/// What an identity change asks of its requester's grant: for each identity
/// it creates or changes, the profile and whether its spec sets each field
/// only manage scope admits, as the spawn tools observe a spawn's arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityGrantRequest {
    pub entries: Vec<IdentityGrantEntry>,
}

/// One identity of an [`IdentityGrantRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityGrantEntry {
    pub profile: ProfileName,
    pub runtime_mode_present: bool,
    pub backend_present: bool,
    pub binding_present: bool,
    pub placement_present: bool,
}

impl IdentityGrantEntry {
    /// What `spec` asks of a grant.
    #[must_use]
    pub fn for_spec(spec: &CreatedIdentitySpec) -> Self {
        Self {
            profile: spec.profile.clone(),
            runtime_mode_present: spec.runtime_mode_override.is_some(),
            backend_present: spec.backend.is_some(),
            binding_present: spec.binding.is_some(),
            placement_present: spec.placement.is_some(),
        }
    }
}

impl IdentityGrantRequest {
    /// What a change of `specs` asks of a grant.
    #[must_use]
    pub fn for_specs<'a>(specs: impl IntoIterator<Item = &'a CreatedIdentitySpec>) -> Self {
        Self {
            entries: specs
                .into_iter()
                .map(IdentityGrantEntry::for_spec)
                .collect(),
        }
    }
}

/// An agent requester's grant for one identity change, admitted by the
/// ordinary spawn grant ([`super::MobHandle::admit_identity_change_grant`]):
/// the change's prepared check, bound to the change's operation and the
/// requester identity. The check's owner answers from that identity's current
/// grant, and the change's commit checks it current. Process-local: it has
/// no serde, and a change that waits across a restart is admitted again.
#[derive(Clone)]
pub struct IdentityChangeGrant {
    requester: AgentIdentity,
    operation_id: String,
    check: PreparedOperationCheck,
}

impl IdentityChangeGrant {
    pub(crate) fn new(
        requester: AgentIdentity,
        operation_id: String,
        check: PreparedOperationCheck,
    ) -> Self {
        Self {
            requester,
            operation_id,
            check,
        }
    }

    /// The requester identity: the member of the session that asked.
    #[must_use]
    pub fn requester(&self) -> &AgentIdentity {
        &self.requester
    }

    /// The operation the grant admits.
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// The change's prepared check.
    #[must_use]
    pub fn check(&self) -> &PreparedOperationCheck {
        &self.check
    }
}

impl std::fmt::Debug for IdentityChangeGrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityChangeGrant")
            .field("requester", &self.requester)
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

/// What a [`MemberGrantMint`] mints for: one identity of one mob, built
/// under its current profile.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MemberGrantMintRequest<'a> {
    pub mob_id: &'a MobId,
    pub agent_identity: &'a AgentIdentity,
    pub profile: &'a ProfileName,
    /// The witness of the mob runtime that reads the grant, for the mob
    /// operator grant functions.
    pub incarnation: LiveMobIncarnation,
    /// The authority the runtime composes for the profile before a host's
    /// member build hook runs (`None` for a profile without mob tools).
    pub composed: Option<&'a MobToolAuthorityContext>,
}

/// The host's mint of a member's mob operator grant, for the reads of an
/// identity's current grant that build nothing: the recheck of an agent
/// requester's identity change at its commit, for a dormant requester too.
/// It answers what the host's member build hook would mint for a build of the
/// identity under its current profile, from the authority the runtime
/// composes for that profile ([`MemberGrantMintRequest::composed`]). Without
/// one ([`super::MobBuilder::with_member_grant_mint`]), an identity's grant
/// is the composed authority.
pub trait MemberGrantMint: Send + Sync {
    /// The identity's grant, as its next build would carry it.
    fn mint(&self, request: &MemberGrantMintRequest<'_>) -> Option<MobToolAuthorityContext>;

    /// A counter the host advances whenever [`Self::mint`] may answer
    /// differently than before. A change checked at its commit reads the
    /// grant again when it moved.
    fn revision(&self) -> u64 {
        0
    }
}

impl super::MobHandle {
    /// Admit an agent requester's identity change by its ordinary spawn
    /// grant: the live grant of `origin_session`, the requesting member's
    /// session as the dispatch that carries the request names it (never a
    /// value from the request's arguments), read through the runtime's
    /// receiver ([`Self::member_spawn_profile_scope_contains`] and
    /// [`Self::member_manage_scope_contains`]), answers MobMachine's spawn
    /// admission for each profile `request` names, with the privileged
    /// fields its specs set, exactly as the spawn tools ask it. A profile the
    /// mob lacks is refused [`IdentityChangeRefusal::ProfileUnknown`], one
    /// the grant does not admit [`IdentityChangeRefusal::ProfileNotAllowed`]
    /// and a privileged field without manage scope
    /// [`IdentityChangeRefusal::PrivilegedField`].
    ///
    /// The admitted grant carries the change's prepared check, bound to
    /// `operation` and to the requester identity (the session's member),
    /// whose owner answers from that identity's current grant at the commit.
    pub async fn admit_identity_change_grant(
        &self,
        origin_session: &SessionId,
        operation: &IdentityOperation,
        request: &IdentityGrantRequest,
    ) -> Result<IdentityChangeGrant, MobError> {
        let _ = (origin_session, operation, request);
        Err(MobError::Internal(
            "store-owned identity changes are not built yet".to_string(),
        ))
    }

    /// Commit a create of store-owned identities as one change: each
    /// identity's row in the identity store (revision 1, its spec, its
    /// provenance with the requester and the consent), the journal rows of
    /// its seat and the change's receipt, in one store transaction, all or
    /// nothing. Each identity is seated Dormant, as a roster-owned identity
    /// of the same spec is (its profile's tools, its labels, the definition's
    /// wiring, its runtime mode, a host root), and nothing wakes; an
    /// identity whose spec owes a kickoff (an initial message, or an
    /// autonomous host's kickoff turn) is spawned once the change is
    /// committed, as the dormant seat spawns such a spec, and runs its
    /// kickoff once. An identity whose host prepares it first waits unseated
    /// for its preparation.
    ///
    /// Refused typed, with nothing written: more than
    /// [`crate::identity::IDENTITY_CHANGE_BATCH_MAX`] identities, a malformed request, an
    /// unknown profile, a name another identity, a worker, a removed
    /// identity or an unreadable row holds, another request under the same
    /// operation, a changes hold (retryable), a candidate composition, a
    /// custom storage composition, a mob that is not running, a decision's
    /// permit that does not bind the change or is past its deadline,
    /// imported consent from anyone but the host operator, and an agent
    /// requester's grant that no longer admits the change at the commit. A
    /// retry of a committed operation with the same request answers
    /// [`IdentityChangeOutcome::Replayed`].
    pub async fn create_store_owned_identities(
        &self,
        create: StoreOwnedIdentityCreate,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let _ = (create, consent);
        Err(MobError::Internal(
            "store-owned identity changes are not built yet".to_string(),
        ))
    }

    /// Hold or release this mob's identity changes: while they are held,
    /// every change is refused [`IdentityChangeRefusal::ChangesHeld`]
    /// (retryable), and nothing of it is written. Not durable: a host that
    /// holds changes holds them again at its next start
    /// ([`super::MobBuilder::with_identity_changes_held`]).
    pub async fn set_identity_changes_held(&self, held: bool) -> Result<(), MobError> {
        let _ = held;
        Err(MobError::Internal(
            "store-owned identity changes are not built yet".to_string(),
        ))
    }

    /// The receipt of the change committed under `operation_id` in this mob,
    /// read from the identity store; `None` when none committed.
    pub async fn identity_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<IdentityChangeReceipt>, MobError> {
        let _ = operation_id;
        Err(MobError::Internal(
            "store-owned identity changes are not built yet".to_string(),
        ))
    }
}
