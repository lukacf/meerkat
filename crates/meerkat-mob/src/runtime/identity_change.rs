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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::tokio::sync::{RwLock, oneshot, watch};
use meerkat_core::SessionId;
use meerkat_core::approval::decision::{ConsentProvenance, DecisionOperationId, Sha256Digest};
use meerkat_core::authorization::PreparedOperationCheck;
use meerkat_core::service::{LiveMobIncarnation, MobToolAuthorityContext};
use meerkat_core::{
    AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
    OperationExecutionScope, OperationId, OperationRefusalKind, OperationRefused,
    OperationReviewTier, OwnerQualifiedTarget, PreparedAuthorizationBinding,
    PreparedOperationAuthorization, PrincipalKind, PrincipalRef, SourceAuthorizationFacts,
    SourceAuthorizationTarget, SourceAuthorizationUse, WorkAuthorization, WorkAuthorizationContext,
};
use meerkat_runtime::decision::DecisionExecutionPermit;

use super::handle::SpawnMemberSpec;
use super::roster_authority::RosterAuthority;
use crate::MobError;
use crate::definition::MobDefinition;
use crate::event::MobEvent;
use crate::identity::{
    CreatedIdentityPresence, CreatedIdentityProvenance, CreatedIdentityRecord, CreatedIdentitySpec,
    IDENTITY_CHANGE_BATCH_MAX, IdentityChangeConsent, IdentityChangeEntryOutcome,
    IdentityChangeKind, IdentityChangeReceipt, IdentityChangeReceiptEntry, IdentityChangeRequester,
    IdentityEntered, IdentityPolicyOutcome, IdentityPreparationMode, IdentitySourceEvidence,
};
use crate::ids::{AgentIdentity, MobId, ProfileName};
use crate::machines::mob_machine as mob_dsl;
use crate::store::{CreatedIdentityRowExpectation, CreatedIdentityRowWrite, IdentityChangeCommit};

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

/// A removal of one identity, of either custody, as one change: its
/// member's retirement ends the entry and tombstones the name, or, for an
/// identity no member holds, the tombstone is written at once. The name is
/// never an identity or a member again.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityRemoval {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    pub identity: AgentIdentity,
}

impl IdentityRemoval {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        identity: impl Into<AgentIdentity>,
    ) -> Self {
        Self {
            operation,
            requester,
            identity: identity.into(),
        }
    }
}

/// An archive, or an unarchive, of one store-owned identity, as one change.
/// An archived identity has no member and keeps its name, its record and its
/// session; its unarchive seats it again on that session. A roster-owned
/// identity's archive is its host's: a one-entry unlist of its roster
/// ([`super::MobHandle::unlist_declared_identity`]), and its unarchive a
/// re-list.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityArchiveChange {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    pub identity: AgentIdentity,
}

impl IdentityArchiveChange {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        identity: impl Into<AgentIdentity>,
    ) -> Self {
        Self {
            operation,
            requester,
            identity: identity.into(),
        }
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
    /// No identity of this mob holds the name: a worker member, or nothing.
    IdentityUnknown { identity: AgentIdentity },
    /// The identity is not archived: there is nothing to unarchive.
    NotArchived { identity: AgentIdentity },
    /// The change is its owner's to write, and this writer is not its owner:
    /// a roster-owned identity's archive is its host's one-entry unlist
    /// (`owner` is `roster`).
    OwnerReadOnly { owner: &'static str },
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
            Self::IdentityUnknown { .. } => "identity_unknown",
            Self::NotArchived { .. } => "not_archived",
            Self::OwnerReadOnly { .. } => "owner_read_only",
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
            Self::IdentityUnknown { identity } => {
                write!(formatter, "no identity of the mob is named '{identity}'")
            }
            Self::NotArchived { identity } => write!(formatter, "'{identity}' is not archived"),
            Self::OwnerReadOnly { owner } => write!(
                formatter,
                "the identity's owner ({owner}) writes this change, not the identity store"
            ),
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
    pub(super) fn new(
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

/// The error of a refused identity change.
pub(super) fn refused(refusal: IdentityChangeRefusal) -> MobError {
    MobError::IdentityChangeRefused { refusal }
}

/// The current time in UNIX milliseconds.
pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// The request: its shape, and each identity's seat spec
// ---------------------------------------------------------------------------

/// Refuse a create whose shape no commit can take: no identity, more than
/// one change commits, an identity named twice, an invalid spec, operation
/// id or request digest.
fn validate_create(create: &StoreOwnedIdentityCreate) -> Result<(), IdentityChangeRefusal> {
    let invalid = |problem: String| IdentityChangeRefusal::InvalidRequest { problem };
    if create.identities.len() > IDENTITY_CHANGE_BATCH_MAX {
        return Err(IdentityChangeRefusal::BatchTooLarge {
            max: IDENTITY_CHANGE_BATCH_MAX,
        });
    }
    if create.identities.is_empty() {
        return Err(invalid("a create names no identity".to_string()));
    }
    if create.operation.operation_id.trim().is_empty()
        || create.operation.request_digest.trim().is_empty()
    {
        return Err(invalid(
            "a change needs an operation id and a request digest".to_string(),
        ));
    }
    let mut named = BTreeSet::new();
    for request in &create.identities {
        request
            .spec
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if !named.insert(&request.spec.identity) {
            return Err(invalid(format!(
                "'{}' is named twice in one change",
                request.spec.identity
            )));
        }
    }
    Ok(())
}

/// The runtime binding a spec's wire binding names.
fn runtime_binding_from_wire(
    binding: meerkat_contracts::WireRuntimeBinding,
) -> Result<crate::RuntimeBinding, IdentityChangeRefusal> {
    match binding {
        meerkat_contracts::WireRuntimeBinding::Session => Ok(crate::RuntimeBinding::Session),
        meerkat_contracts::WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved =
                identity
                    .resolve()
                    .map_err(|error| IdentityChangeRefusal::InvalidRequest {
                        problem: format!("the spec's external binding: {error}"),
                    })?;
            Ok(crate::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

/// The seat spec of a store-owned identity's declared spec: exactly what a
/// roster-owned identity of the same spec is seated with (its profile, its
/// runtime mode, its labels, its context and instructions, its initial
/// message, its binding and placement), as a host root, under created
/// custody. The spec's display name and addressability are the host's
/// reads; nothing of the seat holds them.
pub(super) fn lower_created_spec(
    spec: &CreatedIdentitySpec,
) -> Result<SpawnMemberSpec, IdentityChangeRefusal> {
    let mut seat = SpawnMemberSpec::host_root(spec.profile.clone(), spec.identity.clone());
    seat.runtime_mode = spec.runtime_mode_override;
    seat.backend = spec.backend;
    seat.binding = spec
        .binding
        .clone()
        .map(runtime_binding_from_wire)
        .transpose()?;
    seat.placement = spec
        .placement
        .as_ref()
        .map(|host| mob_dsl::HostId(host.0.clone()));
    seat.context = spec.context.clone();
    seat.labels = Some(spec.labels.clone());
    seat.additional_instructions = Some(spec.additional_instructions.clone());
    seat.initial_message = spec.initial_message.clone();
    seat.created_custody = true;
    Ok(seat)
}

/// One identity a create's plan takes: its seat spec, and its preparation.
pub(super) struct CreatedIdentityPlanItem {
    pub(super) spec: SpawnMemberSpec,
    pub(super) preparation: IdentityPreparationMode,
}

impl CreatedIdentityPlanItem {
    /// The cause a new entry of this identity is unseated for until its seat
    /// or its host's preparation.
    pub(super) fn unseated_cause(&self) -> mob_dsl::UnseatedEntryCause {
        match self.preparation {
            IdentityPreparationMode::None => mob_dsl::UnseatedEntryCause::SeatOwed,
            _ => mob_dsl::UnseatedEntryCause::PreparationPending,
        }
    }
}

// ---------------------------------------------------------------------------
// Admission facts and the machine's verdicts
// ---------------------------------------------------------------------------

/// The facts of a change's admission the handle observes: its consent's form,
/// whether a decision's permit binds the change and admits a commit now, and
/// whether the requester is the host operator. The actor observes the
/// store's receipt and the grant itself.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChangeAdmissionFacts {
    pub(super) consent: mob_dsl::IdentityChangeConsentKind,
    pub(super) permit_binds: bool,
    pub(super) permit_admits: bool,
    pub(super) host_operator: bool,
}

/// The refusal a non-admitting admission verdict answers; `None` for
/// `Admitted` and `Replayed`.
pub(super) fn admission_refusal(
    verdict: mob_dsl::IdentityChangeAdmissionKind,
    operation_id: &str,
) -> Option<IdentityChangeRefusal> {
    use mob_dsl::IdentityChangeAdmissionKind as Verdict;
    match verdict {
        Verdict::Admitted | Verdict::Replayed => None,
        Verdict::OperationReused => Some(IdentityChangeRefusal::OperationReused {
            operation_id: operation_id.to_string(),
        }),
        Verdict::NonAuthoritative => Some(IdentityChangeRefusal::CandidateLaunch),
        Verdict::ChangesHeld => Some(IdentityChangeRefusal::ChangesHeld),
        Verdict::DecisionUnbound => Some(IdentityChangeRefusal::SubjectWidened),
        Verdict::DecisionExpired => {
            Some(IdentityChangeRefusal::DecisionClosed { state: "expired" })
        }
        Verdict::ImportedNotHostOperator => Some(IdentityChangeRefusal::ImportRefused),
        Verdict::GrantRevoked => Some(IdentityChangeRefusal::GrantRevoked),
        Verdict::MobNotRunning => Some(IdentityChangeRefusal::MobNotRunning),
    }
}

/// The refusal a new entry's record verdict answers; `None` for
/// `Recorded`.
pub(super) fn record_refusal(
    verdict: mob_dsl::CreatedIdentityRecordVerdict,
    identity: &AgentIdentity,
) -> Option<Result<IdentityChangeRefusal, MobError>> {
    use mob_dsl::CreatedIdentityRecordVerdict as Verdict;
    let taken = |by| {
        Some(Ok(IdentityChangeRefusal::NameTaken {
            identity: identity.clone(),
            by,
        }))
    };
    match verdict {
        Verdict::Recorded => None,
        Verdict::NameHeldByIdentity => taken(IdentityNameHolder::Identity),
        Verdict::NameHeldByWorker => taken(IdentityNameHolder::Worker),
        Verdict::NameTombstoned => taken(IdentityNameHolder::Removed),
        Verdict::NameUnreadable => taken(IdentityNameHolder::Unreadable),
        Verdict::NonAuthoritative => Some(Ok(IdentityChangeRefusal::CandidateLaunch)),
        Verdict::ChangesHeld => Some(Ok(IdentityChangeRefusal::ChangesHeld)),
        Verdict::MobNotRunning => Some(Ok(IdentityChangeRefusal::MobNotRunning)),
        Verdict::CauseRefused => Some(Err(MobError::Internal(format!(
            "MobMachine refused the cause of '{identity}'s new entry"
        )))),
    }
}

/// The journal rows a change's store transaction committed, or the typed
/// answer of one it did not.
pub(super) fn committed_change_events(
    commit: IdentityChangeCommit,
) -> Result<Vec<MobEvent>, MobError> {
    match commit {
        IdentityChangeCommit::Committed { events } => Ok(events),
        IdentityChangeCommit::OperationReused { receipt } => {
            Err(refused(IdentityChangeRefusal::OperationReused {
                operation_id: receipt.operation_id,
            }))
        }
        IdentityChangeCommit::RowConflict { identity, current } => {
            let by = match current {
                Some(crate::identity::CreatedIdentityObservation::Created(row))
                    if row.presence == CreatedIdentityPresence::Removed =>
                {
                    IdentityNameHolder::Removed
                }
                Some(crate::identity::CreatedIdentityObservation::Unreadable { .. }) => {
                    IdentityNameHolder::Unreadable
                }
                _ => IdentityNameHolder::Identity,
            };
            Err(refused(IdentityChangeRefusal::NameTaken { identity, by }))
        }
        IdentityChangeCommit::Replayed { .. } => Err(MobError::Internal(
            "a change's receipt was committed during its own commit step".to_string(),
        )),
    }
}

/// Whether the change's grant is current at its commit, the final sink: its
/// prepared check, current, or prepared afresh for a decided change, which
/// waited for its decision (its owners answer from the requester identity's
/// current grant either way). A refusal is a revoked grant; an owner that
/// cannot answer fails the step, retryable.
pub(super) fn grant_current_at_the_sink(
    grant: &IdentityChangeGrant,
    decided: bool,
) -> Result<bool, MobError> {
    let checked = if decided {
        grant.check().reprepared()
    } else {
        grant.check().current()
    };
    match checked {
        Ok(_) => Ok(true),
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
        {
            // The owner's facts moved again while it prepared: one more
            // preparation decides.
            match grant.check().current() {
                Ok(_) => Ok(true),
                Err(OperationAuthorizationError::Refused(_)) => Ok(false),
                Err(error) => Err(MobError::Internal(format!(
                    "the identity change's grant could not be read: {error}"
                ))),
            }
        }
        Err(OperationAuthorizationError::Refused(_)) => Ok(false),
        Err(error) => Err(MobError::Internal(format!(
            "the identity change's grant could not be read: {error}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// The commands the actor serves
// ---------------------------------------------------------------------------

/// What a create's plan answers.
pub(super) enum CreatePlanAnswer {
    /// The operation committed before with the same request.
    Replayed(IdentityChangeReceipt),
    /// What each identity's commit needs, in request order.
    Planned(CreatePlan),
}

/// A create's plan: each identity's seat, spawn or wait, and where the seats
/// take their members' endpoints from.
pub(super) struct CreatePlan {
    pub(super) entries: Vec<CreatePlanEntry>,
    pub(super) endpoint_source: Result<
        super::dormant_seat::MemberSeatEndpointSource,
        super::dormant_seat::MemberSeatEndpointsUnavailable,
    >,
}

/// One identity of a create's plan.
pub(super) enum CreatePlanEntry {
    /// Seated Dormant by the commit step: what its seat needs.
    Seat(Box<super::dormant_seat::DormantSeatCandidate>),
    /// Spawned once the change is committed: its spec owes a kickoff (an
    /// initial message, or an autonomous host's kickoff turn), and a fresh
    /// member that owes one is embodied by a spawn, as the dormant seat
    /// embodies one.
    Embody(Box<SpawnMemberSpec>),
    /// Unseated until its host's preparation settles.
    Preparing,
}

/// What a create's commit step takes.
pub(super) struct CreateCommit {
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
    pub(super) proposer: Option<String>,
    pub(super) source_evidence: Option<IdentitySourceEvidence>,
    /// Each identity, in request order, with its new entry's cause.
    pub(super) identities: Vec<(CreatedIdentityRequest, mob_dsl::UnseatedEntryCause)>,
    /// The prepared seats of the identities seated Dormant.
    pub(super) seats: Vec<super::dormant_seat::DormantSeatCommitEntry>,
}

/// What a create's commit step answers.
pub(super) enum CreateCommitted {
    Committed(IdentityChangeReceipt),
    Replayed(IdentityChangeReceipt),
}

/// One step of a store-owned identity change on the actor.
pub(super) enum IdentityChangeCommand {
    /// Plan a create.
    PlanCreate {
        operation: IdentityOperation,
        facts: ChangeAdmissionFacts,
        grant: Option<IdentityChangeGrant>,
        decided: bool,
        items: Vec<CreatedIdentityPlanItem>,
        reply_tx: oneshot::Sender<Result<CreatePlanAnswer, MobError>>,
    },
    /// Commit a planned and prepared create.
    CommitCreate {
        commit: Box<CreateCommit>,
        reply_tx: oneshot::Sender<Result<CreateCommitted, MobError>>,
    },
    /// Hold or release the mob's identity changes.
    SetHeld {
        held: bool,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
}

impl IdentityChangeCommand {
    /// Answer the command with `error`, unserved.
    pub(super) fn reply_error(self, error: MobError) {
        match self {
            Self::PlanCreate { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitCreate { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::SetHeld { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The grant: its owner reads the requester identity's current grant
// ---------------------------------------------------------------------------

/// The namespace of an identity change's prepared operation.
const IDENTITY_CHANGE_OPERATION_NAMESPACE: &str = "meerkat.mob.identity_change";

/// The binding of an identity change's prepared check: a domain operation
/// that retains the change's operation of `requester` in this mob, owned by
/// the mob's runtime host. Only the change's own grant owner reads it.
fn identity_change_binding(
    mob_id: &MobId,
    operation_id: &str,
    requester: &AgentIdentity,
) -> Result<PreparedAuthorizationBinding, MobError> {
    let authority = PrincipalRef::new(PrincipalKind::RuntimeHost, mob_id.as_str())
        .map_err(|error| MobError::Internal(format!("the mob's principal: {error}")))?;
    Ok(PreparedAuthorizationBinding::new(
        OperationAuthorizationFacts {
            operation_id: OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority,
                    namespace: Arc::from(IDENTITY_CHANGE_OPERATION_NAMESPACE),
                    id: Arc::from(format!("{operation_id}/{requester}")),
                }),
                usage: SourceAuthorizationUse::Retain,
            }),
        },
    ))
}

/// What reads an identity's current grant without building it: the
/// published machine state, the roster, the definition and the host's mint.
#[derive(Clone)]
pub(super) struct MemberGrantReader {
    mob_id: MobId,
    incarnation: LiveMobIncarnation,
    definition: Arc<MobDefinition>,
    roster: Arc<RwLock<RosterAuthority>>,
    machine_state: watch::Receiver<mob_dsl::MobMachineState>,
    mint: Option<Arc<dyn MemberGrantMint>>,
    /// Whether each realm-referenced profile of the mob has mob tools, as
    /// the admission resolved it.
    realm_mob_tools: Arc<BTreeMap<ProfileName, bool>>,
}

impl MemberGrantReader {
    /// Whether `profile` has mob tools: an inline profile's own flag, a
    /// realm-referenced one's as the admission resolved it.
    fn profile_mob_tools(
        &self,
        profile: &ProfileName,
    ) -> Result<bool, OperationAuthorizationError> {
        match self.definition.profiles.get(profile) {
            Some(binding) => match binding.as_inline() {
                Some(inline) => Ok(inline.tools.mob),
                None => self
                    .realm_mob_tools
                    .get(profile)
                    .copied()
                    .ok_or(OperationAuthorizationError::Unavailable),
            },
            None => Ok(false),
        }
    }

    /// `identity`'s current grant, as its next build would carry it: the
    /// authority the runtime composes for its current profile (a profile
    /// with mob tools spawns every profile of the mob), through the host's
    /// mint. `None` when it holds none: no member holds the identity, or its
    /// profile has no mob tools and the host mints none. Reads no session, so
    /// a dormant identity's grant is read without waking it.
    fn current_identity_grant(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MobToolAuthorityContext>, OperationAuthorizationError> {
        if !self
            .machine_state
            .borrow()
            .identity_to_runtime
            .contains_key(&mob_dsl::AgentIdentity::from_domain(identity))
        {
            return Ok(None);
        }
        let (profile, mob_tools) = {
            let roster = self
                .roster
                .try_read()
                .map_err(|_| OperationAuthorizationError::Unavailable)?;
            let Some(entry) = roster.get(identity) else {
                return Ok(None);
            };
            let mob_tools = match entry.effective_profile_override.as_ref() {
                Some(profile) => profile.tools.mob,
                None => self.profile_mob_tools(&entry.role)?,
            };
            (entry.role.clone(), mob_tools)
        };
        let composed = if mob_tools {
            crate::build::default_member_mob_operator_authority(
                &self.definition,
                &self.mob_id,
                self.incarnation,
            )
            .map_err(|_| OperationAuthorizationError::Unavailable)?
        } else {
            None
        };
        Ok(match &self.mint {
            Some(mint) => mint.mint(&MemberGrantMintRequest {
                mob_id: &self.mob_id,
                agent_identity: identity,
                profile: &profile,
                incarnation: self.incarnation,
                composed: composed.as_ref(),
            }),
            None => composed,
        })
    }

    fn mint_revision(&self) -> u64 {
        self.mint.as_ref().map_or(0, |mint| mint.revision())
    }
}

/// Whether MobMachine's spawn admission admits every entry of `request`
/// under `grant`, from the grant's raw facts (its manage scope over the mob
/// and its spawn scope's membership of each profile) and the privileged
/// fields each spec sets, as the spawn tools feed them. A pure verdict, asked
/// of a scratch instance of the machine.
fn spawn_admission_admits(
    grant: Option<&MobToolAuthorityContext>,
    mob_id: &MobId,
    incarnation: meerkat_core::service::MobIncarnationId,
    request: &IdentityGrantRequest,
) -> Result<bool, OperationAuthorizationError> {
    let manage = grant.is_some_and(|grant| grant.can_manage_mob(mob_id.as_str(), incarnation));
    for entry in &request.entries {
        let profile_contains = grant.is_some_and(|grant| {
            grant.spawn_profile_scope_contains(mob_id.as_str(), incarnation, entry.profile.as_str())
        });
        let mut scratch = mob_dsl::MobMachineAuthority::new();
        let admission = mob_dsl::MobMachineMutator::apply(
            &mut scratch,
            spawn_admission_input(manage, profile_contains, entry, true),
        )
        .map_err(|_| OperationAuthorizationError::Unavailable)?
        .effects()
        .iter()
        .find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::SpawnMemberAdmissionResolved { admission } => {
                Some(*admission)
            }
            _ => None,
        })
        .ok_or(OperationAuthorizationError::Unavailable)?;
        if admission != mob_dsl::MobSpawnMemberAdmissionKind::Allowed {
            return Ok(false);
        }
    }
    Ok(true)
}

/// MobMachine's spawn admission input for one identity of a change: the
/// grant's raw facts and, when `privileged`, the privileged fields its spec
/// sets.
fn spawn_admission_input(
    manage_scope_present: bool,
    profile_scope_contains: bool,
    entry: &IdentityGrantEntry,
    privileged: bool,
) -> mob_dsl::MobMachineInput {
    mob_dsl::MobMachineInput::ResolveSpawnMemberAdmission {
        manage_scope_present,
        profile_scope_contains,
        privileged_resume_bridge_session_present: false,
        privileged_resume_session_present: false,
        privileged_backend_present: privileged && entry.backend_present,
        privileged_runtime_mode_present: privileged && entry.runtime_mode_present,
        privileged_launch_mode_present: false,
        privileged_tool_access_policy_present: false,
        privileged_tooling_present: false,
        privileged_auth_binding_present: false,
        privileged_binding_present: privileged && entry.binding_present,
        privileged_placement_present: privileged && entry.placement_present,
    }
}

/// The owner of an identity change's prepared check: it answers from the
/// requester identity's current grant, never its session, so dormancy, a
/// new activation and a session reset revoke nothing, and a grant revoked
/// while the change waits refuses it.
struct IdentityChangeGrantOwner {
    reader: MemberGrantReader,
    requester: AgentIdentity,
    request: IdentityGrantRequest,
    binding_id: Arc<str>,
}

impl IdentityChangeGrantOwner {
    /// Whether `binding` is this change's own.
    fn binds(&self, binding: &PreparedAuthorizationBinding) -> bool {
        matches!(
            &binding.facts().operation,
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(target),
                usage: SourceAuthorizationUse::Retain,
            }) if target.namespace.as_ref() == IDENTITY_CHANGE_OPERATION_NAMESPACE
                && target.id == self.binding_id
        )
    }
}

impl WorkAuthorization for IdentityChangeGrantOwner {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        if !self.binds(binding) {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
        }
        // What the decision observes, taken before the read: any later
        // change of either asks for a new preparation.
        let mut machine_state = self.reader.machine_state.clone();
        let _ = machine_state.borrow_and_update();
        let mint_revision = self.reader.mint_revision();
        let grant = self.reader.current_identity_grant(&self.requester)?;
        if !spawn_admission_admits(
            grant.as_ref(),
            &self.reader.mob_id,
            self.reader.incarnation.incarnation(),
            &self.request,
        )? {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(IdentityChangeGrantDecision {
            binding: binding.clone(),
            machine_state,
            mint: self.reader.mint.clone(),
            mint_revision,
        }))
    }
}

/// One preparation's decision: current while the machine state, the roster
/// and the host's mint it read have not moved.
struct IdentityChangeGrantDecision {
    binding: PreparedAuthorizationBinding,
    machine_state: watch::Receiver<mob_dsl::MobMachineState>,
    mint: Option<Arc<dyn MemberGrantMint>>,
    mint_revision: u64,
}

impl PreparedOperationAuthorization for IdentityChangeGrantDecision {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R1
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        let moved = !self.binding.same_operation(binding)
            || self.machine_state.has_changed().unwrap_or(true)
            || self
                .mint
                .as_ref()
                .is_some_and(|mint| mint.revision() != self.mint_revision);
        if moved {
            Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
        } else {
            Ok(())
        }
    }
}

impl super::MobHandle {
    /// The reader of this mob's members' current grants.
    async fn member_grant_reader(&self) -> Result<MemberGrantReader, MobError> {
        let mut realm_mob_tools = BTreeMap::new();
        for (name, binding) in &self.definition.profiles {
            if binding.as_inline().is_none() {
                let profile = self
                    .definition
                    .resolve_profile(name, self.realm_profile_store.as_ref())
                    .await?;
                realm_mob_tools.insert(name.clone(), profile.tools.mob);
            }
        }
        Ok(MemberGrantReader {
            mob_id: self.definition.id.clone(),
            incarnation: self.live_incarnation(),
            definition: Arc::clone(&self.definition),
            roster: Arc::clone(&self.roster),
            machine_state: self.machine_state_watch_rx.clone(),
            mint: self.member_grant_mint.clone(),
            realm_mob_tools: Arc::new(realm_mob_tools),
        })
    }

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
        if request.entries.is_empty() {
            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                problem: "the change names no identity".to_string(),
            }));
        }
        for entry in &request.entries {
            if !self.definition.profiles.contains_key(&entry.profile) {
                return Err(refused(IdentityChangeRefusal::ProfileUnknown {
                    profile: entry.profile.clone(),
                }));
            }
        }
        let first_profile = request.entries[0].profile.clone();
        let Some((requester, _)) = self.member_of_session(origin_session).await else {
            // A session no member of this mob holds holds no grant here.
            return Err(refused(IdentityChangeRefusal::ProfileNotAllowed {
                profile: first_profile,
            }));
        };
        // The receiver's two raw facts for the requesting session, fed to
        // MobMachine's spawn admission as the spawn tools feed them: first
        // the profile alone, then with the privileged fields the spec sets.
        let manage_scope_present = self.member_manage_scope_contains(origin_session);
        let mut asked: Vec<(
            super::SpawnMemberAdmissionObservations,
            super::SpawnMemberAdmission,
        )> = Vec::new();
        for entry in &request.entries {
            let profile_scope_contains =
                self.member_spawn_profile_scope_contains(origin_session, entry.profile.as_str());
            for privileged in [false, true] {
                let observations = super::SpawnMemberAdmissionObservations {
                    manage_scope_present,
                    profile_scope_contains,
                    backend_present: privileged && entry.backend_present,
                    runtime_mode_present: privileged && entry.runtime_mode_present,
                    binding_present: privileged && entry.binding_present,
                    placement_present: privileged && entry.placement_present,
                    ..super::SpawnMemberAdmissionObservations::default()
                };
                let admission = match asked.iter().find(|(asked, _)| *asked == observations) {
                    Some((_, admission)) => *admission,
                    None => {
                        let admission = self.resolve_spawn_member_admission(observations).await?;
                        asked.push((observations, admission));
                        admission
                    }
                };
                if admission == super::SpawnMemberAdmission::Allowed {
                    continue;
                }
                return Err(refused(if privileged {
                    IdentityChangeRefusal::PrivilegedField {
                        field: privileged_field(entry),
                    }
                } else {
                    IdentityChangeRefusal::ProfileNotAllowed {
                        profile: entry.profile.clone(),
                    }
                }));
            }
        }
        let owner = IdentityChangeGrantOwner {
            reader: self.member_grant_reader().await?,
            requester: requester.clone(),
            request: request.clone(),
            binding_id: Arc::from(format!("{}/{requester}", operation.operation_id)),
        };
        let context =
            WorkAuthorizationContext::new(Arc::new(owner), OperationExecutionScope::Domain);
        // The requester identity's grant, read now as the commit will read it
        // again: a host whose mint disagrees with the grant its build hook
        // minted is refused here, typed. The owner reads the roster without
        // waiting; a read that met the actor's write is asked again.
        const GRANT_READ_ATTEMPTS: usize = 8;
        let mut attempt = 0;
        let check = loop {
            attempt += 1;
            let binding =
                identity_change_binding(self.mob_id(), &operation.operation_id, &requester)?;
            match PreparedOperationCheck::prepare(context.clone(), binding) {
                Ok(check) => break check,
                Err(OperationAuthorizationError::Refused(_)) => {
                    return Err(refused(IdentityChangeRefusal::ProfileNotAllowed {
                        profile: first_profile,
                    }));
                }
                Err(OperationAuthorizationError::Unavailable) if attempt < GRANT_READ_ATTEMPTS => {
                    crate::tokio::task::yield_now().await;
                }
                Err(error) => {
                    return Err(MobError::Internal(format!(
                        "the identity change's grant could not be read: {error}"
                    )));
                }
            }
        };
        Ok(IdentityChangeGrant::new(
            requester,
            operation.operation_id.clone(),
            check,
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
        use super::dormant_seat::prepare_dormant_seat;
        use futures::StreamExt as _;

        validate_create(&create).map_err(refused)?;
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        let StoreOwnedIdentityCreate {
            operation,
            requester,
            proposer,
            source_evidence,
            identities,
        } = create;
        let mut items = Vec::with_capacity(identities.len());
        for request in &identities {
            if !self.definition.profiles.contains_key(&request.spec.profile) {
                return Err(refused(IdentityChangeRefusal::ProfileUnknown {
                    profile: request.spec.profile.clone(),
                }));
            }
            if request.spec.is_hosted() {
                return Err(refused(IdentityChangeRefusal::Unsupported {
                    feature: "a placed or externally bound store-owned identity".to_string(),
                }));
            }
            items.push(CreatedIdentityPlanItem {
                spec: lower_created_spec(&request.spec).map_err(refused)?,
                preparation: request.preparation,
            });
        }
        let (requester_record, host_operator, grant) = match requester {
            IdentityChangeRequesterInput::Agent { grant } => {
                if grant.operation_id() != operation.operation_id {
                    return Err(refused(IdentityChangeRefusal::InvalidRequest {
                        problem: "the requester's grant admits another operation".to_string(),
                    }));
                }
                (
                    IdentityChangeRequester::Agent {
                        identity: grant.requester().clone(),
                    },
                    false,
                    Some(grant),
                )
            }
            IdentityChangeRequesterInput::Host {
                surface,
                host_operator,
            } => (
                IdentityChangeRequester::Host { surface },
                host_operator,
                None,
            ),
            IdentityChangeRequesterInput::Human { principal } => {
                (IdentityChangeRequester::Human { principal }, false, None)
            }
        };
        let (consent_record, consent_kind, decided) = match consent {
            IdentityChangeConsentInput::Policy(outcome) => (
                IdentityChangeConsent::Policy { outcome },
                mob_dsl::IdentityChangeConsentKind::Policy,
                false,
            ),
            IdentityChangeConsentInput::Decided(permit) => (
                IdentityChangeConsent::Decided {
                    provenance: permit.consent().clone(),
                },
                mob_dsl::IdentityChangeConsentKind::Decided,
                true,
            ),
            IdentityChangeConsentInput::Imported(provenance) => (
                IdentityChangeConsent::Imported {
                    provenance: provenance.clone(),
                },
                mob_dsl::IdentityChangeConsentKind::Imported,
                false,
            ),
        };
        let permit = match consent {
            IdentityChangeConsentInput::Decided(permit) => Some(permit),
            _ => None,
        };
        let permit_binds = permit.is_some_and(|permit| {
            match (
                DecisionOperationId::new(operation.operation_id.clone()),
                Sha256Digest::new(operation.request_digest.clone()),
            ) {
                (Ok(operation_id), Ok(request_digest)) => {
                    permit.binds(&operation_id, &request_digest)
                }
                _ => false,
            }
        });
        let facts = |now_ms: u64| ChangeAdmissionFacts {
            consent: consent_kind,
            permit_binds,
            permit_admits: permit.is_some_and(|permit| permit.admits_commit_at(now_ms)),
            host_operator,
        };

        // The plan: the change's admission and its entries' records probed,
        // and each identity's seat planned.
        let answer = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::PlanCreate {
                    operation: operation.clone(),
                    facts: facts(now_ms()),
                    grant: grant.clone(),
                    decided,
                    items,
                    reply_tx,
                }))
            })
            .await??;
        let CreatePlan {
            entries,
            endpoint_source,
        } = match answer {
            CreatePlanAnswer::Replayed(receipt) => {
                return Ok(IdentityChangeOutcome::Replayed { receipt });
            }
            CreatePlanAnswer::Planned(plan) => plan,
        };

        // Each seat's session and endpoint, off the actor; one that cannot
        // be prepared fails the change, and nothing is written.
        let mut embody = Vec::new();
        let mut candidates = Vec::new();
        for entry in entries {
            match entry {
                CreatePlanEntry::Seat(candidate) => candidates.push(candidate),
                CreatePlanEntry::Embody(spec) => embody.push(*spec),
                CreatePlanEntry::Preparing => {}
            }
        }
        let session_service = self.session_service.as_ref();
        let mob_id = &self.definition.id;
        let prepared = futures::stream::iter(candidates)
            .map(|candidate| {
                let endpoint_source = endpoint_source.as_ref();
                async move {
                    prepare_dormant_seat(session_service, endpoint_source, mob_id, candidate).await
                }
            })
            .buffered(super::dormant_seat::DORMANT_SEAT_PREPARE_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;
        let mut seats = Vec::with_capacity(prepared.len());
        for prepared in prepared {
            seats.push(prepared.map_err(|outcome| (*outcome).into_unseated_error())?);
        }

        let identities = identities
            .into_iter()
            .map(|request| {
                let cause = match request.preparation {
                    IdentityPreparationMode::None => mob_dsl::UnseatedEntryCause::SeatOwed,
                    _ => mob_dsl::UnseatedEntryCause::PreparationPending,
                };
                (request, cause)
            })
            .collect::<Vec<_>>();
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::CommitCreate {
                    commit: Box::new(CreateCommit {
                        operation,
                        facts: facts(now_ms()),
                        grant,
                        decided,
                        requester: requester_record,
                        consent: consent_record,
                        proposer,
                        source_evidence,
                        identities,
                        seats,
                    }),
                    reply_tx,
                }))
            })
            .await??;
        let receipt = match committed {
            CreateCommitted::Replayed(receipt) => {
                return Ok(IdentityChangeOutcome::Replayed { receipt });
            }
            CreateCommitted::Committed(receipt) => receipt,
        };
        // The identities whose specs owe a kickoff, spawned under created
        // custody now that their rows and entries are committed: each runs
        // its kickoff once. One whose spawn fails stays owed its seat, which
        // the mob's next start seats from its row.
        for spec in embody {
            let agent_identity = spec.identity.clone();
            if let Err(error) = Box::pin(self.spawn_spec_internal(spec)).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    error = %error,
                    "a store-owned identity that owes a kickoff was not spawned"
                );
            }
        }
        Ok(IdentityChangeOutcome::Committed { receipt })
    }

    /// Hold or release this mob's identity changes: while they are held,
    /// every change is refused [`IdentityChangeRefusal::ChangesHeld`]
    /// (retryable), and nothing of it is written. Not durable: a host that
    /// holds changes holds them again at its next start
    /// ([`super::MobBuilder::with_identity_changes_held`]).
    pub async fn set_identity_changes_held(&self, held: bool) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| {
            super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::SetHeld {
                held,
                reply_tx,
            }))
        })
        .await?
    }

    /// The receipt of the change committed under `operation_id` in this mob,
    /// read from the identity store; `None` when none committed.
    pub async fn identity_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<IdentityChangeReceipt>, MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        Ok(store
            .observe_identity_change_receipt(self.mob_id(), operation_id)
            .await?)
    }
}

/// The first privileged field `entry`'s spec sets, in the order a refusal
/// names one.
fn privileged_field(entry: &IdentityGrantEntry) -> &'static str {
    if entry.runtime_mode_present {
        "runtime_mode_override"
    } else if entry.backend_present {
        "backend"
    } else if entry.binding_present {
        "binding"
    } else {
        "placement"
    }
}

/// A create's rows and receipt, sealed at its commit: each identity's row
/// at revision 1 with its spec, its presence and the change's provenance,
/// and the change's receipt with the requester and the consent.
pub(super) fn create_rows_and_receipt(
    mob_id: &MobId,
    commit: &CreateCommit,
    committed_at_ms: u64,
) -> Result<(Vec<CreatedIdentityRowWrite>, IdentityChangeReceipt), MobError> {
    let invalid = |error: crate::identity::IdentityIntentError| {
        MobError::Internal(format!("a store-owned identity's row: {error}"))
    };
    let mut rows = Vec::with_capacity(commit.identities.len());
    let mut entries = Vec::with_capacity(commit.identities.len());
    for (request, cause) in &commit.identities {
        let pending = *cause == mob_dsl::UnseatedEntryCause::PreparationPending;
        let presence = if pending {
            CreatedIdentityPresence::PendingPreparation {
                attempt: 0,
                constructed: request.preparation == IdentityPreparationMode::Constructed,
            }
        } else {
            CreatedIdentityPresence::Present
        };
        let mut provenance = CreatedIdentityProvenance::entered(
            IdentityEntered::Created,
            commit.operation.operation_id.clone(),
            committed_at_ms,
            commit.requester.clone(),
            commit.consent.clone(),
        );
        if let Some(proposer) = &commit.proposer {
            provenance = provenance.with_proposer(proposer.clone());
        }
        if let Some(evidence) = &commit.source_evidence {
            provenance = provenance.with_source_evidence(evidence.clone());
        }
        let record = CreatedIdentityRecord::with_spec(
            mob_id.clone(),
            request.spec.clone(),
            1,
            presence,
            provenance,
        )
        .map_err(invalid)?;
        let spec_digest = request.spec.spec_digest().map_err(invalid)?;
        entries.push(
            IdentityChangeReceiptEntry::new(
                request.spec.identity.clone(),
                if pending {
                    IdentityChangeEntryOutcome::PendingPreparation
                } else {
                    IdentityChangeEntryOutcome::Committed
                },
            )
            .with_spec(spec_digest, Some(1)),
        );
        rows.push(CreatedIdentityRowWrite {
            record,
            expected: CreatedIdentityRowExpectation::Absent,
        });
    }
    let receipt = IdentityChangeReceipt::sealed(
        mob_id.clone(),
        commit.operation.operation_id.clone(),
        commit.operation.request_digest.clone(),
        IdentityChangeKind::Create,
        entries,
        committed_at_ms,
    )
    .and_then(|receipt| receipt.with_attribution(commit.requester.clone(), commit.consent.clone()))
    .map_err(invalid)?;
    Ok((rows, receipt))
}

impl super::MobHandle {
    /// Remove one identity, of either custody, as one change. A seated
    /// identity's member retires, its retirement's start row recording the
    /// cause `removed`: the identity reads retiring until the retirement's
    /// end, which ends the entry and tombstones its name. An identity no
    /// member holds (owed its seat, archived, awaiting its preparation or its
    /// creation) is tombstoned at once. The store-owned row (presence
    /// `removed`, with the removal's record) and the receipt are written in
    /// one store transaction before anything retires; a roster-owned
    /// identity's tombstone is the store's first row of its name. The name is
    /// never an identity or a member again: a create, a declaration, a spawn
    /// and an adoption of it are refused. Meanwhile the declared apply defers
    /// the identity, and afterwards refuses its name.
    ///
    /// Refused typed, with nothing written: no identity of this mob holds the
    /// name ([`IdentityChangeRefusal::IdentityUnknown`]), and every refusal of
    /// [`Self::create_store_owned_identities`] that applies to a change.
    pub async fn remove_identity(
        &self,
        removal: IdentityRemoval,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let _ = (removal, consent);
        Err(MobError::Internal(
            "identity removals are not built yet".to_string(),
        ))
    }

    /// Archive one store-owned identity as one change: its member retires
    /// with the cause `archived`, and its entry stays, unseated, archived,
    /// with the session its member was bound to recorded; its row records
    /// presence `archived` and its name stays reserved. Deliveries to it are
    /// refused ([`MobError::IdentityArchived`]) until its unarchive; updates
    /// of it are allowed. A roster-owned identity's archive is refused
    /// [`IdentityChangeRefusal::OwnerReadOnly`]: its host unlists it.
    pub async fn archive_identity(
        &self,
        change: IdentityArchiveChange,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let _ = (change, consent);
        Err(MobError::Internal(
            "identity archives are not built yet".to_string(),
        ))
    }

    /// Unarchive one archived store-owned identity as one change: it is
    /// seated Dormant again on its archived session, its history kept, or on
    /// a fresh session with the notice when that session is gone from the
    /// store. Refused [`IdentityChangeRefusal::NotArchived`] for an identity
    /// that is not archived.
    pub async fn unarchive_identity(
        &self,
        change: IdentityArchiveChange,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let _ = (change, consent);
        Err(MobError::Internal(
            "identity archives are not built yet".to_string(),
        ))
    }
}
