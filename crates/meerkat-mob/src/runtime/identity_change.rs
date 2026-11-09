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
//! dormant identity too, without waking it. The grant keeps the request its
//! admission checked and the operation's request digest. The commit is the
//! final sink: it checks the prepared check current (re-preparing it for a
//! decided change, which waited for its decision), and a grant revoked
//! meanwhile ends the change [`IdentityChangeRefusal::GrantRevoked`] with
//! nothing written; and it commits only what the admitted request covers
//! (each identity it creates or updates, by profile and privileged fields,
//! and the current profile of each identity it updates, removes, archives,
//! unarchives or settles), refusing anything else
//! [`IdentityChangeRefusal::InvalidRequest`]. Dormancy, a new activation and
//! a session reset of the requester revoke nothing: the check reads its
//! identity, never its session. A change that waits across a restart is
//! admitted again by its executor from the recorded requester identity
//! ([`super::MobHandle::identity_change_grant_for`]), reading no session and
//! waking nothing. Host and human requests carry no grant.

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

/// One identity a store-owned create commits: its declared spec, how its
/// host prepares it before its first use (the host's rule for the spec's
/// profile), whether its host holds its creation, and the peers it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreatedIdentityRequest {
    pub spec: CreatedIdentitySpec,
    pub preparation: IdentityPreparationMode,
    /// Whether the host holds the identity's creation for its held protocol
    /// ([`super::MobHandle::spawn_held_until`] of a spec with created
    /// custody, [`super::SpawnMemberSpec::with_created_custody`]): the
    /// commit seats nothing, the identity awaits its held creation, and no
    /// start seats it Dormant before its host created it, as a roster-owned
    /// identity declared held awaits its held creation.
    pub held_creation: bool,
    /// The edges the create declares from the identity, each to a peer (an
    /// identity of the mob of either custody, the requester included when it
    /// is one, or another identity of the same create) with the peer rule
    /// that admitted it: the identity owns each edge its create declares
    /// first. Nothing else wires a new identity to anything, its requester
    /// included, beyond its profile's definition wiring. An edge is realized
    /// once both its ends are seated.
    pub peers: Vec<crate::identity::OwnedIdentityEdge>,
}

impl CreatedIdentityRequest {
    /// `spec`, with no host preparation and no peer, created by the commit.
    #[must_use]
    pub fn new(spec: CreatedIdentitySpec) -> Self {
        Self {
            spec,
            preparation: IdentityPreparationMode::None,
            held_creation: false,
            peers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_preparation(mut self, preparation: IdentityPreparationMode) -> Self {
        self.preparation = preparation;
        self
    }

    /// Whether the host holds the identity's creation for its held protocol
    /// ([`Self::held_creation`]).
    #[must_use]
    pub fn with_held_creation(mut self, held_creation: bool) -> Self {
        self.held_creation = held_creation;
        self
    }

    /// The edges the create declares from the identity ([`Self::peers`]).
    #[must_use]
    pub fn with_peers(mut self, peers: Vec<crate::identity::OwnedIdentityEdge>) -> Self {
        self.peers = peers;
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
    /// The spec digest the removal was decided against, if any: a
    /// store-owned identity whose row's spec digest differs refuses it
    /// ([`IdentityChangeRefusal::StaleSpec`]).
    pub expected_spec_digest: Option<String>,
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
            expected_spec_digest: None,
        }
    }

    /// Decide the removal against the spec whose digest is `digest`.
    #[must_use]
    pub fn with_expected_spec_digest(mut self, digest: impl Into<String>) -> Self {
        self.expected_spec_digest = Some(digest.into());
        self
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
    /// The spec digest the change was decided against, if any: an identity
    /// whose row's spec digest differs refuses it
    /// ([`IdentityChangeRefusal::StaleSpec`]).
    pub expected_spec_digest: Option<String>,
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
            expected_spec_digest: None,
        }
    }

    /// Decide the change against the spec whose digest is `digest`.
    #[must_use]
    pub fn with_expected_spec_digest(mut self, digest: impl Into<String>) -> Self {
        self.expected_spec_digest = Some(digest.into());
        self
    }
}

/// An update of one store-owned identity's spec, as one change. The
/// identity's row takes the new spec at its next revision, in the change's
/// store transaction, and its member is changed exactly as the declared
/// apply changes a roster-owned identity's member for the same change of
/// spec: a descriptive change keeps it, a runtime-mode change restamps it
/// on its own session, and a profile change moves it to the new profile on
/// its own session (the store-owned identity's migration from its current
/// profile is declared by the update itself, and kept in its row until the
/// restamp commits).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StoreOwnedIdentityUpdate {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    /// The identity's new spec, whole; its identity names the identity.
    pub spec: CreatedIdentitySpec,
    /// The spec digest the change was decided against
    /// ([`CreatedIdentitySpec::spec_digest`] of the spec it changes): a row
    /// whose spec digest differs refuses the change
    /// ([`IdentityChangeRefusal::StaleSpec`]).
    pub expected_spec_digest: String,
    /// How the identity's host prepares it, under its new spec.
    pub preparation: IdentityPreparationMode,
    /// The edges the update declares from the identity, each to a peer with
    /// the peer rule that admitted it (see
    /// [`CreatedIdentityRequest::peers`]): the identity owns each one no
    /// identity owns yet; one an identity owns already is left to its owner.
    pub peers_add: Vec<crate::identity::OwnedIdentityEdge>,
    /// The peers whose edges the update ends: each must be an edge the
    /// identity owns. The edge's realization is unwired.
    pub peers_remove: Vec<AgentIdentity>,
}

impl StoreOwnedIdentityUpdate {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        spec: CreatedIdentitySpec,
        expected_spec_digest: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            requester,
            spec,
            expected_spec_digest: expected_spec_digest.into(),
            preparation: IdentityPreparationMode::None,
            peers_add: Vec::new(),
            peers_remove: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_preparation(mut self, preparation: IdentityPreparationMode) -> Self {
        self.preparation = preparation;
        self
    }

    /// The edges the update declares ([`Self::peers_add`]).
    #[must_use]
    pub fn with_peers_add(mut self, peers: Vec<crate::identity::OwnedIdentityEdge>) -> Self {
        self.peers_add = peers;
        self
    }

    /// The peers whose owned edges the update ends ([`Self::peers_remove`]).
    #[must_use]
    pub fn with_peers_remove(mut self, peers: Vec<AgentIdentity>) -> Self {
        self.peers_remove = peers;
        self
    }
}

/// What a store-owned update answers: the change (committed now, with its
/// receipt, or replayed), and what it did to the identity's member, as the
/// declared apply answers a roster-owned identity's change of the same spec
/// (`None` for a replay, and for an archived identity or one awaiting its
/// host's preparation, whose row alone changes).
#[derive(Debug)]
#[non_exhaustive]
pub struct IdentityUpdateOutcome {
    pub change: IdentityChangeOutcome,
    pub applied: Option<super::DeclaredEntryOutcome>,
}

/// What a store-owned create answers: the change (committed now, with its
/// receipt, or replayed), and the spawn of each identity whose spec owes a
/// kickoff (an initial message, or an autonomous host's kickoff turn),
/// spawned once the change committed, as the declared apply answers a
/// roster-owned identity's embodiment of the same spec.
#[derive(Debug)]
#[non_exhaustive]
pub struct IdentityCreateOutcome {
    pub change: IdentityChangeOutcome,
    /// Per identity whose spec owes a kickoff:
    /// [`super::DeclaredEntryOutcome::Embodied`] with the member's session,
    /// its kickoff running once; or [`super::DeclaredEntryOutcome::Failed`]
    /// with the spawn's error, the identity then owed its seat, with no
    /// member, which the mob's next start embodies once. Empty for a replay
    /// and for a create whose identities owe no kickoff.
    pub embodied: BTreeMap<AgentIdentity, super::DeclaredEntryOutcome>,
}

/// A roster-owned identity's change, written by its host's roster writer
/// and applied by a partial declared apply
/// ([`super::DeclaredRoster::with_change`]): the apply writes the change's
/// receipt in its declaration step's transaction.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityRosterChange {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequester,
    pub consent: IdentityChangeConsent,
}

impl IdentityRosterChange {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequester,
        consent: IdentityChangeConsent,
    ) -> Self {
        Self {
            operation,
            requester,
            consent,
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
/// [`Self::ChangesHeld`] and [`Self::HostPreparationPending`] are retryable
/// unchanged ([`Self::is_retryable`]).
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
    /// The identity's spec moved since the change was decided: its current
    /// spec digest, and its revision (a store-owned identity's; a
    /// roster-owned identity has none).
    StaleSpec {
        identity: AgentIdentity,
        current_digest: String,
        current_revision: Option<u64>,
    },
    /// The identity awaits no preparation.
    PreparationNotPending { identity: AgentIdentity },
    /// The settle's binding is not the held construction's.
    PreparationMismatch { identity: AgentIdentity },
    /// A construction of the identity is held or activating: its attempt.
    ConstructionOpen {
        identity: AgentIdentity,
        attempt: u64,
    },
    /// The attempt is not the identity's held construction: the current
    /// one, if any.
    StaleAttempt {
        identity: AgentIdentity,
        current: Option<u64>,
    },
    /// The identity's preparation is not `constructed`: it takes no held
    /// construction.
    NotConstructed { identity: AgentIdentity },
    /// The identity's preparation is `constructed`: only the activation of
    /// its held construction settles it.
    ConstructionRequired { identity: AgentIdentity },
    /// The activation of the attempt was accepted: its publication is in
    /// flight, and nothing aborts or replaces it.
    ActivationPending {
        identity: AgentIdentity,
        attempt: u64,
    },
    /// The held construction could not be made.
    ConstructionRefused {
        identity: AgentIdentity,
        reason: String,
    },
    /// The identity awaits its host's preparation: it is not archived until
    /// the preparation is settled, or the identity removed. `attempt` names
    /// its held or activating construction, if one is open. Retryable.
    HostPreparationPending {
        identity: AgentIdentity,
        attempt: Option<u64>,
    },
    /// A peer the change lists for `identity` is no identity of the mob (a
    /// worker member's name, nothing's, or an identity whose removal is
    /// retiring it).
    PeerUnknown {
        identity: AgentIdentity,
        peer: AgentIdentity,
    },
    /// An adoption into store custody takes no identity of this shape:
    /// `reason` says why. Retryable for a roster-owned identity its host
    /// still settles ([`NotAdoptableReason::is_retryable`]).
    NotAdoptable {
        identity: AgentIdentity,
        reason: NotAdoptableReason,
    },
    /// An adoption of a worker member names a generation that is not its
    /// member's current one: observe the member again.
    PredecessorMismatch { identity: AgentIdentity },
    /// A member step of the identity an adoption names is in flight (a
    /// spawn, a wake, a rotation, a retirement, a removal, an archive or a
    /// held construction): retryable once it settled.
    AdmissionUnsettled { identity: AgentIdentity },
    /// A declared apply of the mob is open: an adoption waits for it to
    /// close, retryable.
    DeclaredApplyOpen,
    /// The mob's rows of schema version 2 and later were purged before a
    /// downgrade: its identity changes are refused until it restarts.
    Purged,
    /// The purge of the mob's rows is refused: `reason` says why.
    PurgeRefused { reason: PurgeRefusedReason },
}

/// Why a purge of the mob's rows is refused
/// ([`IdentityChangeRefusal::PurgeRefused`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PurgeRefusedReason {
    /// Only the host operator purges.
    NotHostOperator,
    /// The mob's identity changes are not held: the cycle holds them first.
    ChangesNotHeld,
    /// The rows the store holds are not the export's: export them again.
    /// `current_digest` is theirs.
    ExportMismatch { current_digest: String },
    /// Rows this build cannot read remain, which no import would restore.
    UnreadableRows { identities: Vec<AgentIdentity> },
    /// The mob holds no row of schema version 2 or later: nothing stands in
    /// the way of a downgrade.
    NothingToPurge,
}

impl PurgeRefusedReason {
    /// The reason's stable token.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotHostOperator => "not_host_operator",
            Self::ChangesNotHeld => "changes_not_held",
            Self::ExportMismatch { .. } => "export_mismatch",
            Self::UnreadableRows { .. } => "unreadable_rows",
            Self::NothingToPurge => "nothing_to_purge",
        }
    }
}

impl std::fmt::Display for PurgeRefusedReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExportMismatch { current_digest } => write!(
                formatter,
                "export_mismatch (the rows' digest is {current_digest})"
            ),
            Self::UnreadableRows { identities } => {
                formatter.write_str("unreadable_rows (")?;
                for (index, identity) in identities.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{identity}")?;
                }
                formatter.write_str(")")
            }
            other => formatter.write_str(other.as_str()),
        }
    }
}

/// Why an adoption into store custody takes no identity of a shape
/// ([`IdentityChangeRefusal::NotAdoptable`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotAdoptableReason {
    /// The identity is store-owned already.
    StoreOwned,
    /// A roster source names no roster-owned identity.
    NotAnIdentity,
    /// A worker source names a roster-owned identity: its adoption names
    /// the roster as its source.
    RosterOwned,
    /// A worker source names no member.
    NotAMember,
    /// The member lives on a member host: an adoption keeps a local
    /// member's binding only.
    Placed,
    /// The member is bound to no session.
    NoSession,
    /// The roster-owned identity is hosted on its first use: retryable
    /// once its first use seated it. One placed on a member host is
    /// seated as a placed member, whose adoption is refused `Placed`,
    /// which is not retryable.
    HostedOnFirstUse,
    /// The roster-owned identity awaits its held creation: retryable once
    /// its host released the held creation.
    AwaitingHeldCreation,
    /// The roster-owned identity awaits its host's preparation: retryable
    /// once its host's preparation settled.
    AwaitingHostPreparation,
    /// The worker member was spawned with a profile or model override,
    /// which a store-owned identity's spec does not carry.
    ProfileOverride,
}

impl NotAdoptableReason {
    /// The reason's stable token.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StoreOwned => "store_owned",
            Self::NotAnIdentity => "not_an_identity",
            Self::RosterOwned => "roster_owned",
            Self::NotAMember => "not_a_member",
            Self::Placed => "placed",
            Self::NoSession => "no_session",
            Self::HostedOnFirstUse => "hosted_on_first_use",
            Self::AwaitingHeldCreation => "awaiting_held_creation",
            Self::AwaitingHostPreparation => "awaiting_host_preparation",
            Self::ProfileOverride => "profile_override",
        }
    }

    /// Whether an adoption refused for this reason may commit later
    /// unchanged: a roster-owned identity its host still settles (hosted
    /// on its first use, awaiting its held creation or its host's
    /// preparation) is adopted once its host settled it. Only the host's
    /// own action settles it (the first use, the held creation's release,
    /// the preparation), so a caller retries after it and bounds its
    /// retries. An identity placed on a member host is the exception: its
    /// first use seats a placed member, whose adoption is refused
    /// `Placed`, which is not retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::HostedOnFirstUse | Self::AwaitingHeldCreation | Self::AwaitingHostPreparation
        )
    }

    /// The reason MobMachine's verdict names.
    pub(super) fn from_machine(reason: mob_dsl::IdentityNotAdoptableReason) -> Self {
        match reason {
            mob_dsl::IdentityNotAdoptableReason::StoreOwned => Self::StoreOwned,
            mob_dsl::IdentityNotAdoptableReason::NotAnIdentity => Self::NotAnIdentity,
            mob_dsl::IdentityNotAdoptableReason::RosterOwned => Self::RosterOwned,
            mob_dsl::IdentityNotAdoptableReason::NotAMember => Self::NotAMember,
            mob_dsl::IdentityNotAdoptableReason::Placed => Self::Placed,
            mob_dsl::IdentityNotAdoptableReason::NoSession => Self::NoSession,
            mob_dsl::IdentityNotAdoptableReason::HostedOnFirstUse => Self::HostedOnFirstUse,
            mob_dsl::IdentityNotAdoptableReason::AwaitingHeldCreation => Self::AwaitingHeldCreation,
            mob_dsl::IdentityNotAdoptableReason::AwaitingHostPreparation => {
                Self::AwaitingHostPreparation
            }
            mob_dsl::IdentityNotAdoptableReason::ProfileOverride => Self::ProfileOverride,
        }
    }
}

impl std::fmt::Display for NotAdoptableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
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
            Self::StaleSpec { .. } => "stale_spec",
            Self::PreparationNotPending { .. } => "preparation_not_pending",
            Self::PreparationMismatch { .. } => "preparation_mismatch",
            Self::ConstructionOpen { .. } => "construction_open",
            Self::StaleAttempt { .. } => "stale_attempt",
            Self::NotConstructed { .. } => "not_constructed",
            Self::ConstructionRequired { .. } => "construction_required",
            Self::ActivationPending { .. } => "activation_pending",
            Self::ConstructionRefused { .. } => "construction_refused",
            Self::HostPreparationPending { .. } => "host_preparation_pending",
            Self::PeerUnknown { .. } => "peer_unknown",
            Self::NotAdoptable { .. } => "not_adoptable",
            Self::PredecessorMismatch { .. } => "predecessor_mismatch",
            Self::AdmissionUnsettled { .. } => "admission_unsettled",
            Self::DeclaredApplyOpen => "declared_apply_open",
            Self::Purged => "purged",
            Self::PurgeRefused { .. } => "purge_refused",
        }
    }

    /// Whether the same request may commit later unchanged: a changes hold
    /// ends by itself, an identity's pending preparation ends with its
    /// settle, a member step in flight and an open declared apply end with
    /// their own steps, and an adoption of a roster-owned identity its host
    /// still settles ([`NotAdoptableReason::is_retryable`]) commits once
    /// the host settled it. An identity placed on a member host is the
    /// exception: its first use seats a placed member, whose adoption is
    /// refused `not_adoptable` (`placed`), which is not retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ChangesHeld
            | Self::HostPreparationPending { .. }
            | Self::AdmissionUnsettled { .. }
            | Self::DeclaredApplyOpen => true,
            Self::NotAdoptable { reason, .. } => reason.is_retryable(),
            _ => false,
        }
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
            Self::StaleSpec {
                identity,
                current_digest,
                ..
            } => write!(
                formatter,
                "the spec of '{identity}' moved since the change was decided (now {current_digest})"
            ),
            Self::PreparationNotPending { identity } => {
                write!(formatter, "'{identity}' awaits no preparation")
            }
            Self::PreparationMismatch { identity } => write!(
                formatter,
                "the settle of '{identity}' does not bind its held construction"
            ),
            Self::ConstructionOpen { identity, attempt } => write!(
                formatter,
                "a construction of '{identity}' is open (attempt {attempt})"
            ),
            Self::StaleAttempt { identity, current } => match current {
                Some(current) => write!(
                    formatter,
                    "the attempt is not the held construction of '{identity}' (attempt {current})"
                ),
                None => write!(formatter, "no construction of '{identity}' is held"),
            },
            Self::NotConstructed { identity } => write!(
                formatter,
                "the preparation of '{identity}' takes no held construction"
            ),
            Self::ConstructionRequired { identity } => write!(
                formatter,
                "the preparation of '{identity}' is settled by activating its held construction"
            ),
            Self::ActivationPending { identity, attempt } => write!(
                formatter,
                "the activation of '{identity}' (attempt {attempt}) was accepted and its \
                 publication is in flight"
            ),
            Self::ConstructionRefused { identity, reason } => write!(
                formatter,
                "the construction of '{identity}' was refused: {reason}"
            ),
            Self::HostPreparationPending { identity, attempt } => match attempt {
                Some(attempt) => write!(
                    formatter,
                    "'{identity}' awaits its host's preparation (attempt {attempt} is open)"
                ),
                None => write!(formatter, "'{identity}' awaits its host's preparation"),
            },
            Self::PeerUnknown { identity, peer } => write!(
                formatter,
                "the peer '{peer}' listed for '{identity}' is no identity of the mob"
            ),
            Self::NotAdoptable { identity, reason } => {
                write!(formatter, "'{identity}' is not adoptable: {reason}")
            }
            Self::PredecessorMismatch { identity } => write!(
                formatter,
                "the generation observed of '{identity}' is not its member's current one"
            ),
            Self::AdmissionUnsettled { identity } => write!(
                formatter,
                "a member step of '{identity}' is in flight: retry once it settled"
            ),
            Self::DeclaredApplyOpen => {
                write!(formatter, "a declared apply is open: retry once it closed")
            }
            Self::Purged => formatter.write_str(
                "the mob's identity rows were purged: its identity changes are refused until \
                 it restarts",
            ),
            Self::PurgeRefused { reason } => write!(formatter, "the purge is refused: {reason}"),
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

    /// What a change of an identity under `profile` that sets no field of
    /// its spec asks of a grant: its removal, archive, unarchive or settle.
    #[must_use]
    pub fn for_profile(profile: impl Into<ProfileName>) -> Self {
        Self {
            profile: profile.into(),
            runtime_mode_present: false,
            backend_present: false,
            binding_present: false,
            placement_present: false,
        }
    }

    /// Whether a grant admitted for this entry admits `needed`: the same
    /// profile, and every privileged field `needed` sets set here too.
    #[must_use]
    pub fn covers(&self, needed: &IdentityGrantEntry) -> bool {
        self.profile == needed.profile
            && (self.runtime_mode_present || !needed.runtime_mode_present)
            && (self.backend_present || !needed.backend_present)
            && (self.binding_present || !needed.binding_present)
            && (self.placement_present || !needed.placement_present)
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

    /// What a removal, archive, unarchive or settle of an identity whose
    /// current profile is `profile` asks of a grant.
    #[must_use]
    pub fn for_identity(profile: impl Into<ProfileName>) -> Self {
        Self {
            entries: vec![IdentityGrantEntry::for_profile(profile)],
        }
    }

    /// What an update of an identity whose current profile is `current` to
    /// `spec` asks of a grant: the new spec (its profile and the privileged
    /// fields it sets) and the current profile.
    #[must_use]
    pub fn for_update(current: &ProfileName, spec: &CreatedIdentitySpec) -> Self {
        let mut entries = vec![IdentityGrantEntry::for_spec(spec)];
        if *current != spec.profile {
            entries.push(IdentityGrantEntry::for_profile(current.clone()));
        }
        Self { entries }
    }

    /// Whether a grant admitted for this request admits `needed`: each of
    /// its entries is covered by an entry here
    /// ([`IdentityGrantEntry::covers`]).
    #[must_use]
    pub fn covers(&self, needed: &IdentityGrantRequest) -> bool {
        needed
            .entries
            .iter()
            .all(|needed| self.entries.iter().any(|admitted| admitted.covers(needed)))
    }
}

/// An agent requester's grant for one identity change, admitted by the
/// ordinary spawn grant ([`super::MobHandle::admit_identity_change_grant`]):
/// the change's prepared check, bound to the change's operation and the
/// requester identity, with the request its admission checked and the
/// operation's request digest. The check's owner answers from that
/// identity's current grant, and the change's commit checks it current and
/// commits only what the admitted request covers. Process-local: it has no
/// serde, and an executor admits a change that waits across a restart again
/// from its recorded requester identity
/// ([`super::MobHandle::identity_change_grant_for`]).
#[derive(Clone)]
pub struct IdentityChangeGrant {
    requester: AgentIdentity,
    operation_id: String,
    request_digest: String,
    request: IdentityGrantRequest,
    check: PreparedOperationCheck,
}

impl IdentityChangeGrant {
    pub(super) fn new(
        requester: AgentIdentity,
        operation: &IdentityOperation,
        request: IdentityGrantRequest,
        check: PreparedOperationCheck,
    ) -> Self {
        Self {
            requester,
            operation_id: operation.operation_id.clone(),
            request_digest: operation.request_digest.clone(),
            request,
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

    /// The request digest of the operation the grant admits.
    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    /// The request the grant's admission checked: what the change may
    /// commit.
    #[must_use]
    pub fn request(&self) -> &IdentityGrantRequest {
        &self.request
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
            .field("request_digest", &self.request_digest)
            .field("request", &self.request)
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

/// Test seam: holds of a mob's next change step of one kind (`unlist` or
/// `update`), each consumed once, so a row can act between the handle's
/// preparation of the step and the step itself.
#[cfg(test)]
static CHANGE_STEP_HOLDS_FOR_TEST: std::sync::Mutex<Vec<ArmedChangeStepHold>> =
    std::sync::Mutex::new(Vec::new());

/// One armed hold: the mob, the step, and the hold's two ends.
#[cfg(test)]
type ArmedChangeStepHold = (
    MobId,
    &'static str,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
);

/// A held change step: `entered` fires once the step reaches its hold;
/// sending on `release` (or dropping it) lets the step go on.
#[cfg(test)]
pub(in crate::runtime) struct ChangeStepHold {
    pub(in crate::runtime) entered: oneshot::Receiver<()>,
    pub(in crate::runtime) release: oneshot::Sender<()>,
}

/// Hold `mob_id`'s next `step` (`unlist` or `update`), consumed once.
#[cfg(test)]
pub(in crate::runtime) fn hold_next_change_step_for_test(
    mob_id: &MobId,
    step: &'static str,
) -> ChangeStepHold {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    CHANGE_STEP_HOLDS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((mob_id.clone(), step, entered_tx, release_rx));
    ChangeStepHold {
        entered: entered_rx,
        release: release_tx,
    }
}

/// Pass `mob_id`'s hold of `step`, if one is armed: announce the step and
/// wait for its release.
#[cfg(test)]
pub(super) async fn pass_change_step_hold_for_test(mob_id: &MobId, step: &'static str) {
    let hold = {
        let mut holds = CHANGE_STEP_HOLDS_FOR_TEST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        holds
            .iter()
            .position(|(armed, armed_step, ..)| armed == mob_id && *armed_step == step)
            .map(|index| holds.remove(index))
    };
    if let Some((_, _, entered, release)) = hold {
        let _ = entered.send(());
        let _ = release.await;
    }
}

/// Test seam: the identities whose next post-commit kickoff spawn of a
/// create fails, by mob, each consumed once.
#[cfg(test)]
static KICKOFF_SPAWN_FAILURES_FOR_TEST: std::sync::Mutex<Vec<(MobId, AgentIdentity)>> =
    std::sync::Mutex::new(Vec::new());

/// Fail `identity`'s next post-commit kickoff spawn in `mob_id`'s create,
/// consumed once.
#[cfg(test)]
pub(in crate::runtime) fn fail_next_kickoff_spawn_for_test(
    mob_id: &MobId,
    identity: &AgentIdentity,
) {
    KICKOFF_SPAWN_FAILURES_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((mob_id.clone(), identity.clone()));
}

/// Whether `identity`'s kickoff spawn in `mob_id` fails now (consuming the
/// armed failure).
#[cfg(test)]
fn take_kickoff_spawn_failure_for_test(mob_id: &MobId, identity: &AgentIdentity) -> bool {
    let mut failures = KICKOFF_SPAWN_FAILURES_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match failures
        .iter()
        .position(|(armed, failing)| armed == mob_id && failing == identity)
    {
        Some(index) => {
            failures.remove(index);
            true
        }
        None => false,
    }
}

/// The current time in UNIX milliseconds (`time_compat`: the browser's
/// clock on wasm32, where `std::time::SystemTime::now` panics).
pub(super) fn now_ms() -> u64 {
    meerkat_core::time_compat::SystemTime::now()
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
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
        validate_declared_peers(&request.spec.identity, &request.peers)?;
    }
    Ok(())
}

/// Refuse peers a change of `identity` cannot declare: an invalid peer or
/// rule, the identity itself, a peer listed twice, or an adopted edge, which
/// only an adoption declares.
fn validate_declared_peers(
    identity: &AgentIdentity,
    peers: &[crate::identity::OwnedIdentityEdge],
) -> Result<(), IdentityChangeRefusal> {
    let invalid = |problem: String| IdentityChangeRefusal::InvalidRequest { problem };
    let mut listed = BTreeSet::new();
    for edge in peers {
        crate::identity::validate_identity(&edge.peer)
            .map_err(|error| invalid(format!("peer '{}': {error}", edge.peer)))?;
        edge.rule
            .validate()
            .map_err(|error| invalid(format!("peer '{}': {error}", edge.peer)))?;
        if edge.rule == crate::identity::OwnedEdgeAdmission::Adopted {
            return Err(invalid(format!(
                "the edge to '{}' is listed as adopted: only an adoption declares an adopted edge",
                edge.peer
            )));
        }
        if edge.peer == *identity {
            return Err(invalid(format!("'{identity}' lists itself as its peer")));
        }
        if !listed.insert(&edge.peer) {
            return Err(invalid(format!(
                "'{identity}' lists the peer '{}' twice",
                edge.peer
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

/// One identity a create's plan takes: its seat spec, the request's facts
/// its new entry is recorded from (MobMachine derives the cause it is
/// unseated for until its seat, `RecordCreatedIdentityEntry`), and the edges
/// its create declares.
pub(super) struct CreatedIdentityPlanItem {
    pub(super) spec: SpawnMemberSpec,
    pub(super) facts: CreatedEntryFacts,
    pub(super) peers: Vec<crate::identity::OwnedIdentityEdge>,
}

/// The facts of a create's request MobMachine records a new store-owned
/// entry from: whether its host's rule requires its preparation, where its
/// member lives, and whether its host holds its creation.
#[derive(Debug, Clone, Copy)]
pub(super) struct CreatedEntryFacts {
    pub(super) preparation_required: bool,
    pub(super) placement: mob_dsl::EntryPlacementKind,
    pub(super) held_creation: bool,
}

impl CreatedEntryFacts {
    /// The facts of `request`, whose seat spec is `seat`.
    pub(super) fn of(request: &CreatedIdentityRequest, seat: &SpawnMemberSpec) -> Self {
        Self {
            preparation_required: super::identity_preparation::preparation_required(
                request.preparation,
            ),
            placement: super::declared_apply::placement_kind(seat),
            held_creation: request.held_creation,
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
    stale: Option<IdentityChangeRefusal>,
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
        // The step observed the stale spec, and names its current digest.
        Verdict::StaleSpec => Some(stale.unwrap_or(IdentityChangeRefusal::InvalidRequest {
            problem: "the change's spec is not its identity's current one".to_string(),
        })),
        Verdict::RowsPurged => Some(IdentityChangeRefusal::Purged),
    }
}

/// The refusal a new entry's record verdict answers; `None` for
/// `Recorded`. An unlisted roster-owned identity's archived session record
/// holds its name, as an entry does (`by: identity`).
pub(super) fn record_refusal(
    verdict: mob_dsl::CreatedIdentityRecordVerdict,
    identity: &AgentIdentity,
) -> Option<IdentityChangeRefusal> {
    use mob_dsl::CreatedIdentityRecordVerdict as Verdict;
    let taken = |by| {
        Some(IdentityChangeRefusal::NameTaken {
            identity: identity.clone(),
            by,
        })
    };
    match verdict {
        Verdict::Recorded => None,
        Verdict::NameHeldByIdentity => taken(IdentityNameHolder::Identity),
        Verdict::NameHeldByWorker => taken(IdentityNameHolder::Worker),
        Verdict::NameTombstoned => taken(IdentityNameHolder::Removed),
        Verdict::NameUnreadable => taken(IdentityNameHolder::Unreadable),
        Verdict::NonAuthoritative => Some(IdentityChangeRefusal::CandidateLaunch),
        Verdict::ChangesHeld => Some(IdentityChangeRefusal::ChangesHeld),
        Verdict::MobNotRunning => Some(IdentityChangeRefusal::MobNotRunning),
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

/// Whether an agent's change commits only what its grant's admission
/// checked, at the commit, the final sink: each identity of `needed` (what
/// the step commits) is covered by the admitted request
/// ([`IdentityGrantRequest::covers`]). `needed` is `None` when the step
/// holds no profile of its target to check the grant against (a
/// roster-owned identity no member holds, whose spec only its host's roster
/// holds), which an agent's change does not commit. Refused
/// [`IdentityChangeRefusal::InvalidRequest`]; a host or human change
/// carries no grant.
pub(super) fn grant_covers_the_commit(
    grant: Option<&IdentityChangeGrant>,
    needed: Option<&IdentityGrantRequest>,
) -> Result<(), MobError> {
    let Some(grant) = grant else {
        return Ok(());
    };
    match needed {
        Some(needed) if grant.request().covers(needed) => Ok(()),
        Some(_) => Err(refused(IdentityChangeRefusal::InvalidRequest {
            problem: "the requester's grant was admitted for another change than the one \
                      committed"
                .to_string(),
        })),
        None => Err(refused(IdentityChangeRefusal::InvalidRequest {
            problem: "no profile of the identity is held here to check the requester's \
                      grant against: its host's roster holds its spec"
                .to_string(),
        })),
    }
}

/// The current profile of the identity a change targets, as its grant is
/// checked against it: its store-owned row's spec's (`row_spec`), else its
/// seated member's (`state`); `None` when this mob holds neither (a
/// roster-owned identity no member holds).
pub(super) fn change_target_profile(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
    row_spec: Option<&CreatedIdentitySpec>,
) -> Option<ProfileName> {
    if let Some(spec) = row_spec {
        return Some(spec.profile.clone());
    }
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
    state
        .identity_to_runtime
        .contains_key(&dsl_identity)
        .then(|| state.member_profile_names.get(&dsl_identity))
        .flatten()
        .map(|profile| ProfileName::from(profile.as_str()))
}

/// The stale refusal of a removal, archive or unarchive decided against the
/// spec whose digest is `expected`: a store-owned identity's row whose spec
/// digest differs answers [`IdentityChangeRefusal::StaleSpec`] with its
/// current digest and revision, which MobMachine's admission answers after
/// a replay. `None` without an expected digest, for the current one, and
/// for a row that is removed, unreadable, absent or holds no spec (the
/// change's verdict answers those, and an expected digest with no row spec
/// to check is refused at the step).
pub(super) fn entry_change_stale(
    identity: &AgentIdentity,
    expected: Option<&str>,
    current: Option<&crate::identity::CreatedIdentityObservation>,
) -> Option<IdentityChangeRefusal> {
    let expected = expected?;
    let Some(crate::identity::CreatedIdentityObservation::Created(row)) = current else {
        return None;
    };
    if row.presence == CreatedIdentityPresence::Removed {
        return None;
    }
    let current_digest = row.spec_digest().ok().flatten()?;
    (current_digest != expected).then(|| IdentityChangeRefusal::StaleSpec {
        identity: identity.clone(),
        current_digest,
        current_revision: Some(row.revision),
    })
}

// ---------------------------------------------------------------------------
// A change's requester and consent, as its commit takes them
// ---------------------------------------------------------------------------

/// A change's requester and consent, as its records seal them, with the
/// grant an agent's change carries.
pub(super) struct ChangeAttribution {
    pub(super) requester: IdentityChangeRequester,
    pub(super) grant: Option<IdentityChangeGrant>,
    /// Whether a human decision held the change (its grant is re-prepared at
    /// the commit).
    pub(super) decided: bool,
    pub(super) consent: IdentityChangeConsent,
}

/// The admission facts of a change's consent, before the clock reading the
/// permit's deadline is taken.
#[derive(Clone, Copy)]
pub(super) struct ConsentFacts<'a> {
    consent: mob_dsl::IdentityChangeConsentKind,
    permit: Option<&'a DecisionExecutionPermit>,
    permit_binds: bool,
    host_operator: bool,
}

impl ConsentFacts<'_> {
    /// The change's admission facts at the clock reading `now_ms`.
    pub(super) fn at(&self, now_ms: u64) -> ChangeAdmissionFacts {
        ChangeAdmissionFacts {
            consent: self.consent,
            permit_binds: self.permit_binds,
            permit_admits: self
                .permit
                .is_some_and(|permit| permit.admits_commit_at(now_ms)),
            host_operator: self.host_operator,
        }
    }
}

impl ChangeAttribution {
    /// The attribution of a change of `operation` by `requester` under
    /// `consent`; an agent's grant must admit this very operation and
    /// request.
    pub(super) fn of(
        requester: IdentityChangeRequesterInput,
        consent: IdentityChangeConsentInput<'_>,
        operation: &IdentityOperation,
    ) -> Result<Self, MobError> {
        let (requester, grant) = match requester {
            IdentityChangeRequesterInput::Agent { grant } => {
                if grant.operation_id() != operation.operation_id {
                    return Err(refused(IdentityChangeRefusal::InvalidRequest {
                        problem: "the requester's grant admits another operation".to_string(),
                    }));
                }
                if grant.request_digest() != operation.request_digest {
                    return Err(refused(IdentityChangeRefusal::InvalidRequest {
                        problem: "the requester's grant admits another request under its \
                                  operation"
                            .to_string(),
                    }));
                }
                (
                    IdentityChangeRequester::Agent {
                        identity: grant.requester().clone(),
                    },
                    Some(grant),
                )
            }
            IdentityChangeRequesterInput::Host { surface, .. } => {
                (IdentityChangeRequester::Host { surface }, None)
            }
            IdentityChangeRequesterInput::Human { principal } => {
                (IdentityChangeRequester::Human { principal }, None)
            }
        };
        let (consent, decided) = match consent {
            IdentityChangeConsentInput::Policy(outcome) => {
                (IdentityChangeConsent::Policy { outcome }, false)
            }
            IdentityChangeConsentInput::Decided(permit) => (
                IdentityChangeConsent::Decided {
                    provenance: permit.consent().clone(),
                },
                true,
            ),
            IdentityChangeConsentInput::Imported(provenance) => (
                IdentityChangeConsent::Imported {
                    provenance: provenance.clone(),
                },
                false,
            ),
        };
        Ok(Self {
            requester,
            grant,
            decided,
            consent,
        })
    }

    /// Whether `requester` is the host operator.
    pub(super) fn is_host_operator(requester: &IdentityChangeRequesterInput) -> bool {
        matches!(
            requester,
            IdentityChangeRequesterInput::Host {
                host_operator: true,
                ..
            }
        )
    }

    /// The consent facts of a change of `operation` under `consent`, by a
    /// requester that is the host operator or not.
    pub(super) fn facts_of<'a>(
        consent: IdentityChangeConsentInput<'a>,
        operation: &IdentityOperation,
        host_operator: bool,
    ) -> ConsentFacts<'a> {
        let (kind, permit) = match consent {
            IdentityChangeConsentInput::Policy(_) => {
                (mob_dsl::IdentityChangeConsentKind::Policy, None)
            }
            IdentityChangeConsentInput::Decided(permit) => {
                (mob_dsl::IdentityChangeConsentKind::Decided, Some(permit))
            }
            IdentityChangeConsentInput::Imported(_) => {
                (mob_dsl::IdentityChangeConsentKind::Imported, None)
            }
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
        ConsentFacts {
            consent: kind,
            permit,
            permit_binds,
            host_operator,
        }
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
    /// Unseated until its host's preparation settles, or its host creates
    /// it (hosted on its first use, or held for its held protocol).
    Unseated,
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
    /// Each identity, in request order, with the facts its new entry is
    /// recorded from.
    pub(super) identities: Vec<(CreatedIdentityRequest, CreatedEntryFacts)>,
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
        /// What the create asks of a grant: its specs.
        grant_request: IdentityGrantRequest,
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
    /// Commit one identity's removal, archive or unarchive.
    CommitEntryChange {
        commit: Box<EntryChangeCommit>,
        reply_tx: oneshot::Sender<Result<EntryChangeCommitted, MobError>>,
    },
    /// Commit a settle of one identity's preparation.
    Settle {
        commit: Box<super::identity_preparation::PreparationSettleCommit>,
        reply_tx: oneshot::Sender<
            Result<super::identity_preparation::PreparationSettleCommitted, MobError>,
        >,
    },
    /// Begin a held construction of one identity: MobMachine holds a new
    /// attempt (a store-owned identity's row records it).
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    BeginConstruction {
        request: Box<super::identity_preparation::IdentityConstructionRequest>,
        reply_tx: oneshot::Sender<Result<super::identity_preparation::ConstructionBegun, MobError>>,
    },
    /// Keep the hold of a held construction in the mob's registry.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    AdoptConstruction {
        identity: AgentIdentity,
        construction: Box<super::identity_preparation::HeldConstruction>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// A held construction's spawn completed: published (`Ok`), or ended
    /// with its error.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    EndConstruction {
        identity: AgentIdentity,
        attempt: u64,
        completion: Result<(), MobError>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    /// Unlist one roster-owned identity (its archive, which its host's
    /// roster owns): MobMachine's verdict, decided on that identity alone.
    Unlist {
        identity: AgentIdentity,
        reply_tx: oneshot::Sender<Result<mob_dsl::UnlistEntryVerdict, MobError>>,
    },
    /// End the owned edge `owner` holds to `peer`: the host's policy reload
    /// denies it.
    EndDeniedEdge {
        owner: AgentIdentity,
        peer: AgentIdentity,
        reply_tx: oneshot::Sender<Result<super::owned_edges::OwnedEdgeEndOutcome, MobError>>,
    },
    /// Realize every owned edge whose ends are both seated and which the mob
    /// has not wired yet.
    RealizeOwnedEdges { reply_tx: oneshot::Sender<()> },
    /// Commit one adoption into store custody.
    CommitAdoption {
        commit: Box<super::adoption::AdoptionCommit>,
        reply_tx: oneshot::Sender<Result<super::adoption::AdoptionCommitted, MobError>>,
    },
    /// Commit the purge of the mob's rows of schema version 2 and later.
    CommitPurge {
        commit: Box<super::created_set::PurgeCommit>,
        reply_tx: oneshot::Sender<Result<super::created_set::ChangeCommitted, MobError>>,
    },
    /// Commit the import of an export.
    CommitImport {
        commit: Box<super::created_set::ImportCommit>,
        reply_tx: oneshot::Sender<Result<super::created_set::ChangeCommitted, MobError>>,
    },
    /// Abort a held construction attempt before its activation.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    AbortConstruction {
        identity: AgentIdentity,
        attempt: u64,
        reply_tx: oneshot::Sender<
            Result<
                crate::tokio::sync::watch::Receiver<
                    Option<super::identity_preparation::ConstructionOutcome>,
                >,
                MobError,
            >,
        >,
    },
}

/// What one identity's removal, archive or unarchive commit step takes.
pub(super) struct EntryChangeCommit {
    pub(super) kind: mob_dsl::IdentityEntryChangeKind,
    pub(super) identity: AgentIdentity,
    /// The spec digest the change was decided against, if any.
    pub(super) expected_spec_digest: Option<String>,
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
}

/// What an entry change's commit step answers: the committed change's
/// receipt and verdict, with the report of the end of a held construction
/// its removal ended, or the receipt of the operation committed before.
pub(super) enum EntryChangeCommitted {
    Committed {
        receipt: IdentityChangeReceipt,
        verdict: mob_dsl::IdentityEntryChangeVerdict,
        ended_construction: Option<super::identity_preparation::ConstructionEndReport>,
    },
    Replayed(IdentityChangeReceipt),
}

/// The change a declared apply's declaration step decides and writes with
/// its verdicts (`MobCommand::DeclareIdentityEntries`), in the step's one
/// store transaction: a roster-owned identity's change (its receipt), or a
/// store-owned identity's update (its row at the next revision, and its
/// receipt). The step decides the change's admission first, on the step's
/// prepared authority.
pub(super) struct DeclaredChangeWrite {
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
    pub(super) kind: DeclaredChangeKind,
}

/// Which change a declaration step writes ([`DeclaredChangeWrite`]).
pub(super) enum DeclaredChangeKind {
    /// A roster-owned identity's change, applied by a partial declared
    /// apply: its receipt, written with the declarations it answers.
    Roster,
    /// A store-owned identity's update to `spec`, decided against the spec
    /// whose digest is `expected_spec_digest`, with the edges it declares
    /// and the peers whose owned edges it ends.
    StoreOwnedUpdate {
        spec: CreatedIdentitySpec,
        expected_spec_digest: String,
        peers_add: Vec<crate::identity::OwnedIdentityEdge>,
        peers_remove: Vec<AgentIdentity>,
    },
}

impl DeclaredChangeWrite {
    /// The declaration step's write of a roster-owned identity's change. The
    /// host's roster writer applied the change under its own consent; the
    /// step records it: the consent's form, with nothing of a permit left to
    /// check, under the host's roster.
    pub(super) fn roster(change: IdentityRosterChange) -> Self {
        let IdentityRosterChange {
            operation,
            requester,
            consent,
        } = change;
        let kind = match consent {
            IdentityChangeConsent::Decided { .. } => mob_dsl::IdentityChangeConsentKind::Decided,
            IdentityChangeConsent::Policy { .. } => mob_dsl::IdentityChangeConsentKind::Policy,
            IdentityChangeConsent::Imported { .. } => mob_dsl::IdentityChangeConsentKind::Imported,
        };
        Self {
            operation,
            facts: ChangeAdmissionFacts {
                consent: kind,
                permit_binds: true,
                permit_admits: true,
                host_operator: true,
            },
            grant: None,
            decided: false,
            requester,
            consent,
            kind: DeclaredChangeKind::Roster,
        }
    }

    /// Whether the change is a store-owned identity's update.
    pub(super) fn is_store_owned_update(&self) -> bool {
        matches!(self.kind, DeclaredChangeKind::StoreOwnedUpdate { .. })
    }
}

/// What a declaration step answers of its change.
pub(super) enum DeclaredChangeAnswer {
    /// The change committed in the step: its receipt.
    Committed(IdentityChangeReceipt),
    /// The operation committed before with the same request: its receipt.
    Replayed(IdentityChangeReceipt),
}

impl DeclaredChangeAnswer {
    pub(super) fn receipt(&self) -> &IdentityChangeReceipt {
        match self {
            Self::Committed(receipt) | Self::Replayed(receipt) => receipt,
        }
    }
}

/// A declared identity's outcome in its change's receipt, from its verdict:
/// `None` for a verdict that leaves the identity to the next apply or
/// refuses it, which the receipt does not record.
pub(super) fn declared_entry_receipt_outcome(
    verdict: mob_dsl::IdentityEntryVerdict,
) -> Option<IdentityChangeEntryOutcome> {
    use mob_dsl::IdentityEntryVerdict as Verdict;
    match verdict {
        // A record-only update of an archived or pending identity commits
        // its row.
        Verdict::Kept
        | Verdict::SeatOwed
        | Verdict::RuntimeModeRestamp
        | Verdict::RoleMigrate
        | Verdict::RoleRotate
        | Verdict::RoleMigrationHeld
        | Verdict::RecordOnly => Some(IdentityChangeEntryOutcome::Committed),
        Verdict::HostedOnFirstUse => Some(IdentityChangeEntryOutcome::HostedOnFirstUse),
        Verdict::AwaitHeldCreation => Some(IdentityChangeEntryOutcome::AwaitingHeldCreation),
        Verdict::PreparationPending => Some(IdentityChangeEntryOutcome::PendingPreparation),
        Verdict::LifecycleInFlight
        | Verdict::RefusedNameHeldByWorker
        | Verdict::RefusedHostAdopted
        | Verdict::RefusedNonAuthoritative
        | Verdict::MobNotRunning
        | Verdict::RefusedNameOwnedByCreated
        | Verdict::RefusedNameTombstoned
        | Verdict::RefusedNameUnreadable => None,
    }
}

/// A store-owned update's row, written at the change's final sink: the
/// identity's current row (`current`) at its next revision with `spec`, and
/// `pending_role_migration`, the migration the update declares, kept until
/// the restamp commits; and, when the row's spec digest is not the one the
/// change was decided against, the stale refusal (with the current digest
/// and revision) MobMachine's admission answers.
pub(super) fn store_owned_update_row(
    spec: &CreatedIdentitySpec,
    expected_spec_digest: &str,
    pending_role_migration: Option<crate::identity::PendingRoleMigration>,
    current: Option<crate::identity::CreatedIdentityObservation>,
    roster_owned: bool,
) -> Result<(CreatedIdentityRowWrite, Option<IdentityChangeRefusal>), MobError> {
    let invalid = |error: crate::identity::IdentityIntentError| {
        MobError::Internal(format!("a store-owned update's row: {error}"))
    };
    // A roster-owned identity's change is its host roster's.
    let unknown = || {
        refused(if roster_owned {
            IdentityChangeRefusal::OwnerReadOnly { owner: "roster" }
        } else {
            IdentityChangeRefusal::IdentityUnknown {
                identity: spec.identity.clone(),
            }
        })
    };
    let row = match current {
        Some(crate::identity::CreatedIdentityObservation::Created(row))
            if row.presence != CreatedIdentityPresence::Removed =>
        {
            row
        }
        _ => return Err(unknown()),
    };
    let current_digest = row.spec_digest().map_err(invalid)?.ok_or_else(unknown)?;
    let stale =
        (current_digest != expected_spec_digest).then(|| IdentityChangeRefusal::StaleSpec {
            identity: spec.identity.clone(),
            current_digest,
            current_revision: Some(row.revision),
        });
    let expected = CreatedIdentityRowExpectation::Current {
        authority_digest: row.authority_digest.clone(),
    };
    let record = row
        .updated(spec.clone(), pending_role_migration)
        .map_err(invalid)?;
    Ok((CreatedIdentityRowWrite { record, expected }, stale))
}

/// The answer of a store-owned update's store transaction: a row that moved
/// since the step read it answers a stale spec (or an unknown identity).
pub(super) fn store_owned_update_commit(commit: IdentityChangeCommit) -> Result<(), MobError> {
    match commit {
        IdentityChangeCommit::RowConflict { identity, current } => match current {
            Some(crate::identity::CreatedIdentityObservation::Created(row))
                if row.presence != CreatedIdentityPresence::Removed =>
            {
                match row.spec_digest() {
                    Ok(Some(current_digest)) => Err(refused(IdentityChangeRefusal::StaleSpec {
                        identity,
                        current_digest,
                        current_revision: Some(row.revision),
                    })),
                    _ => Err(refused(IdentityChangeRefusal::IdentityUnknown { identity })),
                }
            }
            _ => Err(refused(IdentityChangeRefusal::IdentityUnknown { identity })),
        },
        other => committed_change_events(other).map(|_| ()),
    }
}

/// The change kind, the receipt's outcome for the identity, and the row a
/// verdict writes; a verdict that writes nothing answers a refusal.
/// `open_attempt` is the identity's held or activating construction, which
/// the refusal of an archive of an identity awaiting its preparation names.
pub(super) fn entry_change_answer(
    kind: mob_dsl::IdentityEntryChangeKind,
    verdict: mob_dsl::IdentityEntryChangeVerdict,
    identity: &AgentIdentity,
    open_attempt: Option<u64>,
) -> Result<(IdentityChangeKind, IdentityChangeEntryOutcome), MobError> {
    use mob_dsl::IdentityEntryChangeVerdict as Verdict;
    let change = match kind {
        mob_dsl::IdentityEntryChangeKind::Remove => IdentityChangeKind::Remove,
        mob_dsl::IdentityEntryChangeKind::Archive => IdentityChangeKind::Archive,
        mob_dsl::IdentityEntryChangeKind::Unarchive => IdentityChangeKind::Unarchive,
    };
    let outcome = match verdict {
        Verdict::Retiring => IdentityChangeEntryOutcome::Retiring,
        Verdict::Removed | Verdict::AlreadyRemoved => IdentityChangeEntryOutcome::Removed,
        Verdict::Archiving => IdentityChangeEntryOutcome::Archiving,
        Verdict::Archived | Verdict::AlreadyArchived => IdentityChangeEntryOutcome::Archived,
        Verdict::Restored => IdentityChangeEntryOutcome::Restored,
        Verdict::NotArchived => {
            return Err(refused(IdentityChangeRefusal::NotArchived {
                identity: identity.clone(),
            }));
        }
        Verdict::OwnedByRoster => {
            return Err(refused(IdentityChangeRefusal::OwnerReadOnly {
                owner: "roster",
            }));
        }
        Verdict::Unknown => {
            return Err(refused(IdentityChangeRefusal::IdentityUnknown {
                identity: identity.clone(),
            }));
        }
        Verdict::NameUnreadable => {
            return Err(refused(IdentityChangeRefusal::NameTaken {
                identity: identity.clone(),
                by: IdentityNameHolder::Unreadable,
            }));
        }
        Verdict::InFlight => {
            return Err(MobError::LifecycleOperationPending {
                intent: format!(
                    "an identity change of '{identity}' while a member step of it runs"
                ),
            });
        }
        Verdict::NonAuthoritative => return Err(refused(IdentityChangeRefusal::CandidateLaunch)),
        Verdict::ChangesHeld => return Err(refused(IdentityChangeRefusal::ChangesHeld)),
        Verdict::MobNotRunning => return Err(refused(IdentityChangeRefusal::MobNotRunning)),
        // Its preparation is never dropped: settled first, or the identity
        // removed.
        Verdict::PreparationPending => {
            return Err(refused(IdentityChangeRefusal::HostPreparationPending {
                identity: identity.clone(),
                attempt: open_attempt,
            }));
        }
    };
    Ok((change, outcome))
}

/// The row an entry change writes, from the identity's current row (`None`
/// when the store holds none: a roster-owned identity's): a removal writes
/// the tombstone (the store-owned row, removed, with the removal's record,
/// or a roster-owned identity's first row), an archive the row archived on
/// `archived_session`, an unarchive the row present again. A verdict that
/// changes nothing writes none.
pub(super) fn entry_change_row(
    mob_id: &MobId,
    commit: &EntryChangeCommit,
    verdict: mob_dsl::IdentityEntryChangeVerdict,
    current: Option<crate::identity::CreatedIdentityObservation>,
    archived_session: Option<SessionId>,
    at_ms: u64,
) -> Result<Option<CreatedIdentityRowWrite>, MobError> {
    use mob_dsl::IdentityEntryChangeVerdict as Verdict;
    let invalid = |error: crate::identity::IdentityIntentError| {
        MobError::Internal(format!("an identity change's row: {error}"))
    };
    let current = match current {
        Some(crate::identity::CreatedIdentityObservation::Created(row)) => Some(row),
        Some(crate::identity::CreatedIdentityObservation::Unreadable { .. }) => {
            return Err(refused(IdentityChangeRefusal::NameTaken {
                identity: commit.identity.clone(),
                by: IdentityNameHolder::Unreadable,
            }));
        }
        None => None,
    };
    let updated =
        |row: CreatedIdentityRecord, written: CreatedIdentityRecord| CreatedIdentityRowWrite {
            expected: CreatedIdentityRowExpectation::Current {
                authority_digest: row.authority_digest,
            },
            record: written,
        };
    let removal = || {
        crate::identity::IdentityRemovalRecord::new(
            commit.operation.operation_id.clone(),
            at_ms,
            commit.requester.clone(),
            commit.consent.clone(),
        )
    };
    Ok(match (verdict, current) {
        // The removal ends the identity's owned edges with it: its
        // tombstone owns none.
        (Verdict::Retiring | Verdict::Removed, Some(row)) => {
            let written = row
                .clone()
                .with_owned_edges(Vec::new())
                .and_then(|written| written.with_presence(CreatedIdentityPresence::Removed))
                .and_then(|written| written.with_removal(removal()))
                .map_err(invalid)?;
            Some(updated(row, written))
        }
        // A roster-owned identity's removal: the store's first row of its
        // name is its tombstone.
        (Verdict::Retiring | Verdict::Removed, None) => Some(CreatedIdentityRowWrite {
            record: CreatedIdentityRecord::sealed(
                mob_id.clone(),
                commit.identity.clone(),
                1,
                CreatedIdentityPresence::Removed,
                CreatedIdentityProvenance::entered(
                    IdentityEntered::RosterRemoval,
                    commit.operation.operation_id.clone(),
                    at_ms,
                    commit.requester.clone(),
                    commit.consent.clone(),
                ),
            )
            .and_then(|record| record.with_removal(removal()))
            .map_err(invalid)?,
            expected: CreatedIdentityRowExpectation::Absent,
        }),
        (Verdict::Archiving | Verdict::Archived, Some(row)) => {
            let written = row
                .clone()
                .with_presence(CreatedIdentityPresence::Archived { archived_session })
                .map_err(invalid)?;
            Some(updated(row, written))
        }
        (Verdict::Restored, Some(row)) => {
            let written = row
                .clone()
                .with_presence(CreatedIdentityPresence::Present)
                .map_err(invalid)?;
            Some(updated(row, written))
        }
        (Verdict::Archiving | Verdict::Archived | Verdict::Restored, None) => {
            return Err(MobError::Internal(format!(
                "the store-owned identity '{}' has no row",
                commit.identity
            )));
        }
        _ => None,
    })
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
            Self::CommitEntryChange { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Settle { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Unlist { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::EndDeniedEdge { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            // A realization pass answers nothing but its end.
            Self::RealizeOwnedEdges { reply_tx } => {
                let _ = reply_tx.send(());
            }
            Self::CommitAdoption { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitPurge { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitImport { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            Self::BeginConstruction { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            // The refused hold's decision channel goes with the command: the
            // hold aborts, and its completion ends the attempt.
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            Self::AdoptConstruction { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            Self::EndConstruction { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            Self::AbortConstruction { reply_tx, .. } => {
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

    fn spend_at_entry(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
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
        self.validate_grant_request(request)?;
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
        // The requester identity's grant, read now as the commit will read it
        // again: a host whose mint disagrees with the grant its build hook
        // minted is refused here, typed.
        let Some(check) = self
            .prepare_identity_change_check(&requester, operation, request)
            .await?
        else {
            return Err(refused(IdentityChangeRefusal::ProfileNotAllowed {
                profile: first_profile,
            }));
        };
        Ok(IdentityChangeGrant::new(
            requester,
            operation,
            request.clone(),
            check,
        ))
    }

    /// The executor's re-admission of a change admitted before: the grant
    /// of `requester`'s change `operation`, asking `request` of its grant,
    /// rebuilt from the recorded requester identity, after a restart (the
    /// grant is process-local) or in a settlement-first recovery. It
    /// prepares the change's check exactly as the admission does, bound to
    /// the operation and the requester identity, whose owner answers from
    /// that identity's current grant (its current profile and the host's
    /// build mint), for a dormant identity too: it reads no session and
    /// wakes nothing. The commit checks it current at its sink, as any
    /// grant.
    ///
    /// Refused typed: a malformed request
    /// ([`IdentityChangeRefusal::InvalidRequest`]), a profile the mob lacks
    /// ([`IdentityChangeRefusal::ProfileUnknown`]), and a grant the
    /// identity no longer holds for the request (revoked by its host, its
    /// profile moved, or no member holds the identity):
    /// [`IdentityChangeRefusal::GrantRevoked`].
    pub async fn identity_change_grant_for(
        &self,
        requester: &AgentIdentity,
        operation: &IdentityOperation,
        request: &IdentityGrantRequest,
    ) -> Result<IdentityChangeGrant, MobError> {
        self.validate_grant_request(request)?;
        let Some(check) = self
            .prepare_identity_change_check(requester, operation, request)
            .await?
        else {
            return Err(refused(IdentityChangeRefusal::GrantRevoked));
        };
        Ok(IdentityChangeGrant::new(
            requester.clone(),
            operation,
            request.clone(),
            check,
        ))
    }

    /// Refuse a grant request that names no identity or a profile the mob
    /// lacks.
    fn validate_grant_request(&self, request: &IdentityGrantRequest) -> Result<(), MobError> {
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
        Ok(())
    }

    /// The change's prepared check, bound to `operation` and the identity
    /// `requester`, whose owner answers from that identity's current grant
    /// for `request`; `None` when the owner refuses it now. The owner reads
    /// the roster without waiting; a read that met the actor's write is
    /// asked again.
    async fn prepare_identity_change_check(
        &self,
        requester: &AgentIdentity,
        operation: &IdentityOperation,
        request: &IdentityGrantRequest,
    ) -> Result<Option<PreparedOperationCheck>, MobError> {
        let owner = IdentityChangeGrantOwner {
            reader: self.member_grant_reader().await?,
            requester: requester.clone(),
            request: request.clone(),
            binding_id: Arc::from(format!("{}/{requester}", operation.operation_id)),
        };
        let context =
            WorkAuthorizationContext::new(Arc::new(owner), OperationExecutionScope::Domain);
        const GRANT_READ_ATTEMPTS: usize = 8;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let binding =
                identity_change_binding(self.mob_id(), &operation.operation_id, requester)?;
            match PreparedOperationCheck::prepare(context.clone(), binding) {
                Ok(check) => return Ok(Some(check)),
                Err(OperationAuthorizationError::Refused(_)) => return Ok(None),
                Err(OperationAuthorizationError::Unavailable) if attempt < GRANT_READ_ATTEMPTS => {
                    crate::tokio::task::yield_now().await;
                }
                Err(error) => {
                    return Err(MobError::Internal(format!(
                        "the identity change's grant could not be read: {error}"
                    )));
                }
            }
        }
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
    /// kickoff once. Each such spawn is answered per identity
    /// ([`IdentityCreateOutcome::embodied`]): embodied, or failed, the
    /// identity then owed its seat (the change stays committed), which the
    /// mob's next start embodies once. An identity whose host prepares it
    /// first waits unseated for its preparation. A placed or externally bound
    /// identity is hosted on its first use, and one whose host holds its
    /// creation ([`CreatedIdentityRequest::held_creation`]) awaits it:
    /// neither is seated, and its host's spawn holding its created custody
    /// ([`super::SpawnMemberSpec::with_created_custody`]) creates it. The
    /// peers each identity's create lists ([`CreatedIdentityRequest::peers`])
    /// are its owned edges, in its row: each is wired by the seat that
    /// makes both its ends seated, the commit's own included.
    ///
    /// Refused typed, with nothing written: more than
    /// [`crate::identity::IDENTITY_CHANGE_BATCH_MAX`] identities, a malformed request (a
    /// peer listed twice, the identity itself or an adopted edge among its
    /// peers included), an unknown profile, a peer that is no identity of
    /// the mob ([`IdentityChangeRefusal::PeerUnknown`]; another identity of
    /// the same create is one), a name another identity, a worker, a removed
    /// identity or an unreadable row holds, another request under the same
    /// operation, a changes hold (retryable), a candidate composition, a
    /// custom storage composition, a mob that is not running, a decision's
    /// permit that does not bind the change or is past its deadline,
    /// imported consent from anyone but the host operator, an agent
    /// requester's grant that no longer admits the change at the commit
    /// ([`IdentityChangeRefusal::GrantRevoked`]), and one admitted for
    /// another request or other specs than the change commits
    /// ([`IdentityChangeRefusal::InvalidRequest`]). A retry of a committed
    /// operation with the same request answers
    /// [`IdentityChangeOutcome::Replayed`].
    pub async fn create_store_owned_identities(
        &self,
        create: StoreOwnedIdentityCreate,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityCreateOutcome, MobError> {
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
            let spec = lower_created_spec(&request.spec).map_err(refused)?;
            items.push(CreatedIdentityPlanItem {
                facts: CreatedEntryFacts::of(request, &spec),
                spec,
                peers: request.peers.clone(),
            });
        }
        let attribution_facts = ChangeAttribution::facts_of(
            consent,
            &operation,
            ChangeAttribution::is_host_operator(&requester),
        );
        let ChangeAttribution {
            requester: requester_record,
            grant,
            decided,
            consent: consent_record,
        } = ChangeAttribution::of(requester, consent, &operation)?;
        let facts = |now_ms: u64| attribution_facts.at(now_ms);
        let grant_request =
            IdentityGrantRequest::for_specs(identities.iter().map(|request| &request.spec));

        // The plan: the change's admission and its entries' records probed,
        // and each identity's seat planned.
        let answer = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::PlanCreate {
                    operation: operation.clone(),
                    facts: facts(now_ms()),
                    grant: grant.clone(),
                    decided,
                    grant_request,
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
                return Ok(IdentityCreateOutcome {
                    change: IdentityChangeOutcome::Replayed { receipt },
                    embodied: BTreeMap::new(),
                });
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
                CreatePlanEntry::Unseated => {}
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
                let facts = lower_created_spec(&request.spec)
                    .map(|seat| CreatedEntryFacts::of(&request, &seat))
                    .map_err(refused)?;
                Ok((request, facts))
            })
            .collect::<Result<Vec<_>, MobError>>()?;
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
                return Ok(IdentityCreateOutcome {
                    change: IdentityChangeOutcome::Replayed { receipt },
                    embodied: BTreeMap::new(),
                });
            }
            CreateCommitted::Committed(receipt) => receipt,
        };
        // The identities whose specs owe a kickoff, spawned under created
        // custody now that their rows and entries are committed: each runs
        // its kickoff once, and each spawn is answered. One whose spawn fails
        // stays owed its seat, which the mob's next start embodies from its
        // row.
        let mut embodied = BTreeMap::new();
        for spec in embody {
            let agent_identity = spec.identity.clone();
            #[cfg(test)]
            let spawned = if take_kickoff_spawn_failure_for_test(self.mob_id(), &agent_identity) {
                Err(MobError::Internal(
                    "the test seam failed the kickoff spawn".to_string(),
                ))
            } else {
                Box::pin(self.spawn_spec_internal(spec)).await
            };
            #[cfg(not(test))]
            let spawned = Box::pin(self.spawn_spec_internal(spec)).await;
            let answer = match spawned {
                Ok(member) => match member.bridge_session_id() {
                    Some(session_id) => super::DeclaredEntryOutcome::Embodied {
                        session_id: session_id.clone(),
                        history_unavailable: None,
                    },
                    // A create embodies only a local identity, whose member
                    // is session-backed.
                    None => super::DeclaredEntryOutcome::Failed {
                        error: MobError::Internal(format!(
                            "the embodied store-owned identity '{agent_identity}' has no session"
                        )),
                    },
                },
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %agent_identity,
                        error = %error,
                        "a store-owned identity that owes a kickoff was not spawned: it is \
                         owed its seat"
                    );
                    super::DeclaredEntryOutcome::Failed { error }
                }
            };
            embodied.insert(agent_identity, answer);
        }
        Ok(IdentityCreateOutcome {
            change: IdentityChangeOutcome::Committed { receipt },
            embodied,
        })
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
    /// read from the identity store; `None` when none committed. It carries
    /// the requester and the consent, so it needs the List scope, as the
    /// record reads do ([`MobError::ScopeDenied`]).
    pub async fn identity_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<IdentityChangeReceipt>, MobError> {
        self.read_published_machine_state(crate::ControlScope::List, |_| ())?;
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
/// and the change's receipt with the requester and the consent. `causes`
/// are the causes MobMachine recorded each new entry unseated for, in
/// request order.
pub(super) fn create_rows_and_receipt(
    mob_id: &MobId,
    commit: &CreateCommit,
    causes: &[mob_dsl::UnseatedEntryCause],
    owned: &BTreeMap<AgentIdentity, Vec<crate::identity::OwnedIdentityEdge>>,
    committed_at_ms: u64,
) -> Result<(Vec<CreatedIdentityRowWrite>, IdentityChangeReceipt), MobError> {
    let invalid = |error: crate::identity::IdentityIntentError| {
        MobError::Internal(format!("a store-owned identity's row: {error}"))
    };
    if causes.len() != commit.identities.len() {
        return Err(MobError::Internal(
            "a create's records answered another count of identities".to_string(),
        ));
    }
    let mut rows = Vec::with_capacity(commit.identities.len());
    let mut entries = Vec::with_capacity(commit.identities.len());
    for ((request, _), cause) in commit.identities.iter().zip(causes) {
        let pending = *cause == mob_dsl::UnseatedEntryCause::PreparationPending;
        let presence = if pending {
            CreatedIdentityPresence::PendingPreparation {
                attempt: 0,
                constructed: request.preparation == IdentityPreparationMode::Constructed,
            }
        } else {
            CreatedIdentityPresence::Present
        };
        let outcome = match cause {
            mob_dsl::UnseatedEntryCause::PreparationPending => {
                IdentityChangeEntryOutcome::PendingPreparation
            }
            mob_dsl::UnseatedEntryCause::HostedOnFirstUse => {
                IdentityChangeEntryOutcome::HostedOnFirstUse
            }
            mob_dsl::UnseatedEntryCause::HeldCreation => {
                IdentityChangeEntryOutcome::AwaitingHeldCreation
            }
            _ => IdentityChangeEntryOutcome::Committed,
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
        .and_then(|record| record.with_held_creation(request.held_creation))
        .and_then(|record| {
            record.with_owned_edges(
                owned
                    .get(&request.spec.identity)
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .map_err(invalid)?;
        let spec_digest = request.spec.spec_digest().map_err(invalid)?;
        entries.push(
            IdentityChangeReceiptEntry::new(request.spec.identity.clone(), outcome)
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
    /// creation) is tombstoned at once; a held construction of it ends in the
    /// same step (its hold aborts, its member never published, and the
    /// removal answers once the hold ended), and one whose activation was
    /// accepted is in flight: the removal is refused retryable until its
    /// publication ends. The store-owned row (presence
    /// `removed`, with the removal's record) and the receipt are written in
    /// one store transaction before anything retires; a roster-owned
    /// identity's tombstone is the store's first row of its name. The name is
    /// never an identity or a member again: a create, a declaration, a spawn
    /// and an adoption of it are refused. Meanwhile the declared apply defers
    /// the identity, and afterwards refuses its name.
    ///
    /// Answers once the member's retirement ended. A retirement that fails
    /// leaves the removal marked: a retry of the same operation (which
    /// answers [`IdentityChangeOutcome::Replayed`]) or the mob's next start
    /// retires the member again.
    ///
    /// Decided against a spec ([`IdentityRemoval::with_expected_spec_digest`]),
    /// a store-owned identity's removal is refused
    /// [`IdentityChangeRefusal::StaleSpec`] when its row's spec moved since;
    /// a roster-owned identity's spec is its host roster's to check, and a
    /// removal of one naming a digest is refused
    /// [`IdentityChangeRefusal::InvalidRequest`]. An agent requester's
    /// removal commits only against the identity's current profile its
    /// grant's admission checked (its row's, or its member's): a
    /// roster-owned identity no member holds has no profile here to check,
    /// and its host removes it.
    ///
    /// Refused typed, with nothing written: no identity of this mob holds the
    /// name ([`IdentityChangeRefusal::IdentityUnknown`]), and every refusal of
    /// [`Self::create_store_owned_identities`] that applies to a change.
    pub async fn remove_identity(
        &self,
        removal: IdentityRemoval,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityRemoval {
            operation,
            requester,
            identity,
            expected_spec_digest,
        } = removal;
        self.change_identity_entry(
            mob_dsl::IdentityEntryChangeKind::Remove,
            identity,
            expected_spec_digest,
            operation,
            requester,
            consent,
        )
        .await
    }

    /// Archive one store-owned identity as one change: its member retires
    /// with the cause `archived`, and its entry stays, unseated, archived,
    /// with the session its member was bound to recorded; its row records
    /// presence `archived` and its name stays reserved. Deliveries to it are
    /// refused ([`MobError::IdentityArchived`]) until its unarchive; updates
    /// of it are allowed. A roster-owned identity's archive is refused
    /// [`IdentityChangeRefusal::OwnerReadOnly`]: its host unlists it
    /// ([`Self::unlist_declared_identity`]). An identity awaiting its host's
    /// preparation is not archived: refused
    /// [`IdentityChangeRefusal::HostPreparationPending`] (retryable, naming
    /// its open construction attempt), with nothing written, until its
    /// preparation is settled, or the identity removed. Decided against a
    /// spec ([`IdentityArchiveChange::with_expected_spec_digest`]), it is
    /// refused [`IdentityChangeRefusal::StaleSpec`] when the row's spec moved
    /// since. Answers once the member's retirement ended, as a removal does.
    pub async fn archive_identity(
        &self,
        change: IdentityArchiveChange,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityArchiveChange {
            operation,
            requester,
            identity,
            expected_spec_digest,
        } = change;
        self.change_identity_entry(
            mob_dsl::IdentityEntryChangeKind::Archive,
            identity,
            expected_spec_digest,
            operation,
            requester,
            consent,
        )
        .await
    }

    /// Unarchive one archived store-owned identity as one change: its row is
    /// present again, and it is seated Dormant on its archived session, its
    /// history kept, or on a fresh session with the notice when that session
    /// is gone from the store. Refused [`IdentityChangeRefusal::NotArchived`]
    /// for an identity that is not archived, and
    /// [`IdentityChangeRefusal::StaleSpec`] when decided against a spec the
    /// row no longer holds. A seat that fails leaves the identity owed its
    /// seat, which the mob's next start seats.
    pub async fn unarchive_identity(
        &self,
        change: IdentityArchiveChange,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityArchiveChange {
            operation,
            requester,
            identity,
            expected_spec_digest,
        } = change;
        self.change_identity_entry(
            mob_dsl::IdentityEntryChangeKind::Unarchive,
            identity,
            expected_spec_digest,
            operation,
            requester,
            consent,
        )
        .await
    }

    /// One identity's removal, archive or unarchive: its commit step on the
    /// actor, then the member's retirement it owes, or the seat it owes.
    async fn change_identity_entry(
        &self,
        kind: mob_dsl::IdentityEntryChangeKind,
        identity: AgentIdentity,
        expected_spec_digest: Option<String>,
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                problem: "a change needs an operation id and a request digest".to_string(),
            }));
        }
        let consent_facts = ChangeAttribution::facts_of(
            consent,
            &operation,
            ChangeAttribution::is_host_operator(&requester),
        );
        let ChangeAttribution {
            requester,
            grant,
            decided,
            consent,
        } = ChangeAttribution::of(requester, consent, &operation)?;
        let commit = EntryChangeCommit {
            kind,
            identity: identity.clone(),
            expected_spec_digest,
            operation,
            facts: consent_facts.at(now_ms()),
            grant,
            decided,
            requester,
            consent,
        };
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::CommitEntryChange {
                        commit: Box::new(commit),
                        reply_tx,
                    },
                ))
            })
            .await??;
        let (receipt, verdict, replayed) = match committed {
            EntryChangeCommitted::Committed {
                receipt,
                verdict,
                ended_construction,
            } => {
                // A removal that ended a held construction answers once the
                // construction's hold ended, its member never published.
                #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
                if let Some(report) = ended_construction
                    && let super::identity_preparation::ConstructionOutcome::Ended {
                        cleanup: Some(cleanup),
                        ..
                    } = super::identity_preparation::construction_outcome(report).await
                {
                    tracing::warn!(
                        mob_id = %self.mob_id(),
                        agent_identity = %identity,
                        cleanup = %cleanup,
                        "the removed identity's held construction did not confirm its cleanup"
                    );
                }
                #[cfg(not(all(feature = "runtime-adapter", not(target_arch = "wasm32"))))]
                let _ = ended_construction;
                (receipt, Some(verdict), false)
            }
            EntryChangeCommitted::Replayed(receipt) => (receipt, None, true),
        };
        match verdict {
            // The member's retirement the change owes, with the mark's cause.
            Some(
                mob_dsl::IdentityEntryChangeVerdict::Retiring
                | mob_dsl::IdentityEntryChangeVerdict::Archiving,
            ) => self.retire(identity.clone()).await?,
            Some(mob_dsl::IdentityEntryChangeVerdict::Restored) => {
                Box::pin(self.seat_restored_identity(&identity)).await?;
            }
            // A replay finishes an ending a retirement did not carry out.
            None if super::identity_entry::entry_ending_owed(
                &self.machine_state_watch_rx.borrow(),
                &identity,
            ) =>
            {
                self.retire(identity.clone()).await?;
            }
            // A replayed unarchive seats an identity its first seat left owed
            // its seat (an apply that ran meanwhile, a session store that
            // could not be read).
            None if kind == mob_dsl::IdentityEntryChangeKind::Unarchive
                && self
                    .machine_state_watch_rx
                    .borrow()
                    .identity_entry_unseated
                    .get(&mob_dsl::AgentIdentity::from_domain(&identity))
                    == Some(&mob_dsl::UnseatedEntryCause::SeatOwed) =>
            {
                Box::pin(self.seat_restored_identity(&identity)).await?;
            }
            _ => {}
        }
        Ok(if replayed {
            IdentityChangeOutcome::Replayed { receipt }
        } else {
            IdentityChangeOutcome::Committed { receipt }
        })
    }

    /// Seat an unarchived store-owned identity from its row's spec
    /// ([`Self::seat_store_owned_from_spec`]). An identity whose row holds
    /// no spec is seated from its last spawn row, as a start seats an entry
    /// owed its seat.
    async fn seat_restored_identity(&self, identity: &AgentIdentity) -> Result<(), MobError> {
        match self.store_owned_seat_facts(identity).await? {
            Some((spec, held_creation)) => {
                self.seat_store_owned_from_spec(identity, &spec, held_creation)
                    .await
            }
            None => {
                self.reseat_owed_created_entries().await;
                Ok(())
            }
        }
    }

    /// The spec `identity`'s store-owned row holds, with whether its host
    /// holds its creation; `None` for a row that holds no spec.
    pub(super) async fn store_owned_seat_facts(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<(CreatedIdentitySpec, bool)>, MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        Ok(
            match store
                .observe_created_identity(self.mob_id(), identity)
                .await?
            {
                Some(crate::identity::CreatedIdentityObservation::Created(row)) => {
                    row.spec.map(|spec| (spec, row.held_creation))
                }
                _ => None,
            },
        )
    }

    /// Seat a store-owned identity no member holds from `spec`, its row's
    /// spec (an unarchive, and a start's seat of an entry owed its seat),
    /// through the declared apply's own verdicts under created custody: a
    /// local identity is seated Dormant on its archived session record (its
    /// history kept, declaring the migration from the role that session was
    /// last built under when the spec names another), on a fresh session
    /// with the notice when that session is gone from the store, or on a
    /// fresh one when it has none, and a spec that owes a kickoff is
    /// embodied; a placed or externally bound identity is hosted on its
    /// first use; and one whose host holds its creation (`held_creation`)
    /// awaits it while it was never created. Once the member holds the
    /// spec's profile, the row lets its role migration go.
    pub(super) async fn seat_store_owned_from_spec(
        &self,
        identity: &AgentIdentity,
        spec: &CreatedIdentitySpec,
        held_creation: bool,
    ) -> Result<(), MobError> {
        match Box::pin(self.seat_store_owned_outcome(identity, spec, held_creation)).await? {
            super::DeclaredEntryOutcome::Seated { .. }
            | super::DeclaredEntryOutcome::Embodied { .. }
            | super::DeclaredEntryOutcome::Kept
            | super::DeclaredEntryOutcome::HostedOnFirstUse
            | super::DeclaredEntryOutcome::AwaitingHeldCreation => Ok(()),
            super::DeclaredEntryOutcome::Failed { error }
            | super::DeclaredEntryOutcome::Deferred { reason: error } => Err(error),
            other => Err(MobError::Internal(format!(
                "the seat of store-owned '{identity}' answered {other:?}"
            ))),
        }
    }

    /// [`Self::seat_store_owned_from_spec`], answering the seat as the
    /// declared apply answers it, a failed or deferred seat included; an
    /// error only when the apply itself could not run. The row lets its
    /// role migration go once the member holds the spec's profile.
    pub(super) async fn seat_store_owned_outcome(
        &self,
        identity: &AgentIdentity,
        spec: &CreatedIdentitySpec,
        held_creation: bool,
    ) -> Result<super::DeclaredEntryOutcome, MobError> {
        let archived = self
            .machine_state_watch_rx
            .borrow()
            .identity_entry_archived_sessions
            .get(&mob_dsl::AgentIdentity::from_domain(identity))
            .map(|session| SessionId::parse(&session.0))
            .transpose()
            .map_err(|error| {
                MobError::Internal(format!(
                    "'{identity}' holds an invalid archived session record: {error}"
                ))
            })?;
        let mut roster = super::DeclaredRoster::partial([super::DeclaredEntry::new(
            lower_created_spec(spec).map_err(refused)?,
        )
        .with_held_creation(held_creation)]);
        if let Some(session) = archived
            && let Some(role) = self.resume_session_role(&session).await?
            && role != spec.profile
        {
            roster = roster.with_role_migration(identity.clone(), role);
        }
        let (mut outcome, _) = self.run_declared_apply(roster, None).await?;
        let seat = outcome.entries.remove(identity).ok_or_else(|| {
            MobError::Internal(format!(
                "the seat of store-owned '{identity}' was not answered"
            ))
        })?;
        self.settle_store_owned_role_migration(identity).await?;
        Ok(seat)
    }
}

impl super::MobHandle {
    /// Update one store-owned identity's spec as one change (see
    /// [`StoreOwnedIdentityUpdate`]): a declared apply of the identity under
    /// created custody, whose declaration step decides the change's
    /// admission and the identity's verdict (the declared apply's own arms,
    /// `DeclareIdentityEntry`) and writes the identity's row at its next
    /// revision and the change's receipt in one store transaction; the
    /// verdict is then realized as a roster-owned identity's change of the
    /// same spec is ([`IdentityUpdateOutcome::applied`]). A descriptive
    /// change (display name, labels, context, instructions) keeps the member
    /// and is in the row at once: a seat whose session no build created yet
    /// takes the new context and instructions at its first build; otherwise
    /// the seat keeps its recorded build inputs, as a roster-owned
    /// identity's does, and its labels change when it is seated again or
    /// respawned with them ([`super::MemberRespawn::with_labels`], counted
    /// in `seats_stale` until then). A runtime-mode change is a restamp on
    /// the member's own session; a profile change is the declared migration
    /// from the member's current role, kept in the row until the restamp
    /// commits (a crash before it is carried out by the next start); either
    /// restamp seats the row's labels;
    /// a placement, binding or backend change is read only while no member
    /// holds the identity. An archived identity's update, and the update of
    /// one awaiting its host's preparation, changes its row only: MobMachine
    /// decides that in the declaration step from the step's own state, so an
    /// unarchive or a settle that runs before the step is the update's
    /// starting point.
    ///
    /// A retry of a committed update with the same request answers
    /// [`IdentityChangeOutcome::Replayed`], never a stale spec. The update's
    /// peers ([`StoreOwnedIdentityUpdate::peers_add`],
    /// [`StoreOwnedIdentityUpdate::peers_remove`]) declare and end the
    /// identity's own owned edges in the same step and transaction, its row
    /// holding them; an edge whose ends are both seated is wired, or
    /// unwired, once the step committed. An agent
    /// requester's update commits only what its grant's admission checked
    /// ([`IdentityGrantRequest::for_update`]: the new spec and the
    /// identity's current profile). Refused typed, with nothing written: a
    /// spec digest that is not the identity's current one
    /// ([`IdentityChangeRefusal::StaleSpec`], with the current digest and
    /// revision), an agent's grant admitted for another change
    /// ([`IdentityChangeRefusal::InvalidRequest`]), an identity that is not
    /// store-owned ([`IdentityChangeRefusal::IdentityUnknown`], or
    /// [`IdentityChangeRefusal::OwnerReadOnly`] for a roster-owned one), a
    /// peer that is no identity of the mob
    /// ([`IdentityChangeRefusal::PeerUnknown`]), a peer removed whose edge
    /// the identity does not own ([`IdentityChangeRefusal::InvalidRequest`]:
    /// its change ends only its own edges), and
    /// every refusal of [`Self::create_store_owned_identities`] that applies
    /// to a change. An identity whose member's spawn, wake, retirement,
    /// removal or archive, or whose held construction, is in flight is
    /// refused [`MobError::LifecycleOperationPending`], retryable, and so is
    /// an update while another declared apply of the mob runs.
    pub async fn update_store_owned_identity(
        &self,
        update: StoreOwnedIdentityUpdate,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityUpdateOutcome, MobError> {
        let StoreOwnedIdentityUpdate {
            operation,
            requester,
            spec,
            expected_spec_digest,
            preparation,
            peers_add,
            peers_remove,
        } = update;
        let invalid = |problem: String| refused(IdentityChangeRefusal::InvalidRequest { problem });
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(invalid(
                "a change needs an operation id and a request digest".to_string(),
            ));
        }
        spec.validate()
            .map_err(|error| invalid(error.to_string()))?;
        validate_declared_peers(&spec.identity, &peers_add).map_err(refused)?;
        let mut removed = BTreeSet::new();
        for peer in &peers_remove {
            if !removed.insert(peer) || peers_add.iter().any(|edge| edge.peer == *peer) {
                return Err(invalid(format!(
                    "the peer '{peer}' is listed twice in one update"
                )));
            }
        }
        if preparation != IdentityPreparationMode::None {
            return Err(refused(IdentityChangeRefusal::Unsupported {
                feature: "a store-owned update's host preparation".to_string(),
            }));
        }
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        if !self.definition.profiles.contains_key(&spec.profile) {
            return Err(refused(IdentityChangeRefusal::ProfileUnknown {
                profile: spec.profile.clone(),
            }));
        }
        let attribution_facts = ChangeAttribution::facts_of(
            consent,
            &operation,
            ChangeAttribution::is_host_operator(&requester),
        );
        let ChangeAttribution {
            requester: requester_record,
            grant,
            decided,
            consent: consent_record,
        } = ChangeAttribution::of(requester, consent, &operation)?;
        // The identity's declaration under created custody, always:
        // MobMachine routes it in the declaration step from that step's own
        // state. An archived identity, and one awaiting its host's
        // preparation, is answered record only (its row changes, and its
        // unarchive or its settle seats it from the new spec); an identity
        // with no store-owned entry is answered at the step's sink (a retry
        // of an update that committed replays, anything else is refused).
        let entries = vec![super::DeclaredEntry::new(
            lower_created_spec(&spec).map_err(refused)?,
        )];
        let change = DeclaredChangeWrite {
            operation,
            facts: attribution_facts.at(now_ms()),
            grant,
            decided,
            requester: requester_record,
            consent: consent_record,
            kind: DeclaredChangeKind::StoreOwnedUpdate {
                spec: spec.clone(),
                expected_spec_digest,
                peers_add,
                peers_remove,
            },
        };
        #[cfg(test)]
        pass_change_step_hold_for_test(self.mob_id(), "update").await;
        let (mut outcome, answer) = self
            .run_declared_apply(super::DeclaredRoster::partial(entries), Some(change))
            .await?;
        match answer {
            Some(DeclaredChangeAnswer::Committed(receipt)) => {
                let applied = outcome.entries.remove(&spec.identity);
                // The restamp committed, or the member already holds the
                // profile: the row lets the migration go.
                if let Err(error) = self.settle_store_owned_role_migration(&spec.identity).await {
                    tracing::warn!(
                        mob_id = %self.mob_id(),
                        agent_identity = %spec.identity,
                        error = %error,
                        "a store-owned update's row kept its role migration"
                    );
                }
                Ok(IdentityUpdateOutcome {
                    change: IdentityChangeOutcome::Committed { receipt },
                    applied,
                })
            }
            Some(DeclaredChangeAnswer::Replayed(receipt)) => {
                // A retry of a committed update: what its restamp left owed
                // is carried out now.
                if let Err(error) = self.redeclare_store_owned_identity(&spec.identity).await {
                    tracing::warn!(
                        mob_id = %self.mob_id(),
                        agent_identity = %spec.identity,
                        error = %error,
                        "a replayed store-owned update's restamp was not carried out"
                    );
                }
                Ok(IdentityUpdateOutcome {
                    change: IdentityChangeOutcome::Replayed { receipt },
                    applied: None,
                })
            }
            None => Err(MobError::Internal(
                "a store-owned update's declaration step answered no change".to_string(),
            )),
        }
    }

    /// The role `identity`'s member holds, in the published machine state;
    /// `None` when no member holds it.
    fn seated_role(&self, identity: &AgentIdentity) -> Option<ProfileName> {
        let state = self.machine_state_watch_rx.borrow();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        if !state.identity_to_runtime.contains_key(&dsl_identity) {
            return None;
        }
        state
            .member_profile_names
            .get(&dsl_identity)
            .map(|profile| ProfileName::from(profile.as_str()))
    }

    /// Let a store-owned identity's row go of the role migration it keeps,
    /// once the member holds the row's profile (its restamp committed): a
    /// follow-up write of the row, checked against the row read here, that
    /// a newer change of the row supersedes.
    pub(super) async fn settle_store_owned_role_migration(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let Some(crate::identity::CreatedIdentityObservation::Created(row)) = store
            .observe_created_identity(self.mob_id(), identity)
            .await?
        else {
            return Ok(());
        };
        let Some(profile) = row.spec.as_ref().map(|spec| spec.profile.clone()) else {
            return Ok(());
        };
        if row.pending_role_migration.is_none()
            || self.seated_role(identity).as_ref() != Some(&profile)
        {
            return Ok(());
        }
        let expected = CreatedIdentityRowExpectation::Current {
            authority_digest: row.authority_digest.clone(),
        };
        let record = row.with_pending_role_migration(None).map_err(|error| {
            MobError::Internal(format!("a store-owned identity's row: {error}"))
        })?;
        store
            .settle_created_identity_row(&CreatedIdentityRowWrite { record, expected })
            .await?;
        Ok(())
    }

    /// Declare a store-owned identity again from its row's spec, under
    /// created custody, through the declared apply's own verdicts: what an
    /// update's restamp left owed (a crash, or a member busy at the update)
    /// is carried out, with the migration from the role its member holds,
    /// and nothing is written when the member already matches its spec. The
    /// row then lets its role migration go once the member holds its
    /// profile. An identity no member holds, an archived one and one
    /// awaiting its host's preparation are left as they are: their seat
    /// takes the spec.
    pub(super) async fn redeclare_store_owned_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let Some(crate::identity::CreatedIdentityObservation::Created(row)) = store
            .observe_created_identity(self.mob_id(), identity)
            .await?
        else {
            return Ok(());
        };
        let Some(spec) = row.spec.as_ref() else {
            return Ok(());
        };
        if row.presence != CreatedIdentityPresence::Present {
            return Ok(());
        }
        let Some(role) = self.seated_role(identity) else {
            return Ok(());
        };
        let mut roster = super::DeclaredRoster::partial([super::DeclaredEntry::new(
            lower_created_spec(spec).map_err(refused)?,
        )]);
        if role != spec.profile {
            roster = roster.with_role_migration(identity.clone(), role);
        }
        let (outcome, _) = self.run_declared_apply(roster, None).await?;
        if let Some(super::DeclaredEntryOutcome::Failed { error }) = outcome
            .entries
            .into_iter()
            .find_map(|(declared, answer)| (declared == *identity).then_some(answer))
        {
            return Err(error);
        }
        self.settle_store_owned_role_migration(identity).await
    }
}
