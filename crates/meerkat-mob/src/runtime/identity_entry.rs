//! Identity entries: the identities a host declared as its own durable
//! identities, as opposed to worker members, and the reads that show them
//! without waking anyone.
//!
//! MobMachine owns every fact here (`identity_entry_origins`). An entry's
//! member is a member like any other; the entry adds custody of the name:
//!
//! - no worker reconcile ([`super::MobHandle::reconcile`],
//!   [`super::MobHandle::ensure_member`]) spawns or retires an entry;
//! - no spawn without entry custody
//!   ([`super::SpawnMemberSpec::entry_custody`]) takes an entry's name: it is
//!   refused [`crate::MobError::IdentityMember`];
//! - the agent lane's member commands (retire, respawn, rotation, release,
//!   reload, cancellation, wiring) addressed to an entry are refused
//!   [`crate::MobError::IdentityMember`].
//!
//! An entry is recorded by the declared apply
//! ([`super::MobHandle::apply_declared_roster`]), by the membership commit of
//! a spawn, or by the dormant seat, whose spec carries entry custody, and by
//! the adoption of a legacy member
//! ([`super::MobHandle::adopt_legacy_identity_entry`]). Its member's journal
//! row records it ([`crate::event::MemberSpawnedEvent`]'s stored
//! `identity_entry`, or the adoption's binding row), so a restart restores a
//! seated entry without reading anything else. A rotation (a reset, a
//! deletion, a same-session successor) keeps it. An entry outlives a
//! retirement the declared apply did not start: it stays unseated
//! ([`EntryStanding::Unseated`]), and the next apply seats it again on its own
//! archived session. Only the end of an unlisted retirement (the declared
//! apply's seal), the mob's destroy and the mob's reset end an entry. An
//! unseated entry is not journaled: after a restart, the next apply declares
//! it again.
//!
//! A created entry ([`IdentityEntryOrigin::Created`]) has its desired record
//! in the mob's identity store: its created entry row. Every start recovers
//! it from that row, after the journal replay and before the start's member
//! plan: a present row of a seated member records the member a created
//! entry; a present row whose member the journal shows retired (a build that
//! predates created entries retired it) records the entry owed its seat, and
//! the start seats it again, Dormant, on its archived session; a row pending
//! its host's preparation records it unseated
//! ([`UnseatedEntryCause::TemplatePreparation`]); and a removed entry's row
//! reserves its name for good ([`IdentityEntryRead::Removed`]). A row this
//! build cannot read, a row the journal contradicts, and a member row whose
//! origin this build does not know reserve the name as unreadable
//! ([`IdentityEntryRead::Unreadable`]). Nothing acts on a reserved name: no
//! spawn, seat, declaration, worker reconcile, adoption or agent-lane
//! command. The declared apply never declares over a created entry, and its
//! seal never unlists one.
//!
//! The reads ([`super::MobHandle::identity_entry`],
//! [`super::MobHandle::identity_entries`], [`super::MobHandle::identity_census`],
//! [`super::MobHandle::member_lifecycle_events`],
//! [`super::MobHandle::members_holding_run_starts`]) need no actor command and
//! never wake a member. The first three read the machine state the actor last
//! published, as [`super::MobHandle::member_unmaterialized`] does; the answer
//! can be stale by the time the caller acts on it, and it never says whether a
//! member is materialized.

use std::ops::Bound;

use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};

use crate::MobError;
use crate::event::{MobEvent, MobEventKind};
use crate::ids::{AgentIdentity, Generation, MobId, ProfileName};
use crate::machines::mob_machine as mob_dsl;

/// The most entries one [`super::MobHandle::identity_entries`] page and one
/// [`super::MobHandle::member_lifecycle_events`] page return.
pub const IDENTITY_ENTRY_PAGE_MAX: usize = 256;

/// The label key a legacy roster member names its own identity under. Read
/// once, by the legacy adoption's observation
/// ([`super::MobHandle::adopt_legacy_identity_entry`]), and nowhere else.
pub(crate) const LEGACY_IDENTITY_LABEL: &str = "agent_identity";

/// How an identity became an identity entry.
///
/// The handle's entry reads and the journal's rows answer with this type.
/// MobMachine's own state holds the generated machine enum of the same name
/// and variants (`machines::mob_machine::IdentityEntryOrigin`); this type is
/// its public projection, which a new machine variant extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityEntryOrigin {
    /// The host declared it: a spawn or a dormant seat whose spec carried
    /// entry custody, or the adoption of a legacy member.
    Declared,
    /// Created at runtime: its desired record is its created entry row in
    /// the mob's identity store, from which every start recovers it, and
    /// MobMachine alone drives its member. No declared apply declares over
    /// it or unlists it, and no worker reconcile, spawn or agent-lane
    /// command acts on it.
    Created,
}

impl IdentityEntryOrigin {
    pub(crate) fn from_machine(origin: mob_dsl::IdentityEntryOrigin) -> Self {
        match origin {
            mob_dsl::IdentityEntryOrigin::Declared => Self::Declared,
            mob_dsl::IdentityEntryOrigin::Created => Self::Created,
        }
    }

    pub(crate) fn to_machine(self) -> mob_dsl::IdentityEntryOrigin {
        match self {
            Self::Declared => mob_dsl::IdentityEntryOrigin::Declared,
            Self::Created => mob_dsl::IdentityEntryOrigin::Created,
        }
    }

    /// The stored token of the origin in a journal row.
    pub(crate) fn as_stored(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Created => "created",
        }
    }

    /// The origin a journal row's stored token names. `None` for a token
    /// this build does not know (a row a later release wrote): the row keeps
    /// the token as stored, and its replay reserves the identity's name as
    /// unreadable, so nothing acts on it.
    pub(crate) fn from_stored(token: &str) -> Option<Self> {
        match token {
            "declared" => Some(Self::Declared),
            "created" => Some(Self::Created),
            _ => None,
        }
    }
}

/// Why an identity entry has no member ([`EntryStanding::Unseated`]).
///
/// MobMachine's own state holds the generated machine enum of the same name
/// and variants (`machines::mob_machine::UnseatedEntryCause`); this type is
/// its public projection, which a new machine variant extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UnseatedEntryCause {
    /// The declared apply owes the entry a seat: an apply seats it Dormant, on
    /// its archived session when the identity has one.
    SeatOwed,
    /// A placed or externally bound entry: its host creates the member when
    /// the identity is first used (a spawn whose spec carries entry custody).
    HostedOnFirstUse,
    /// The host declared the entry's creation held, and the identity was never
    /// created: the host's held protocol creates it
    /// ([`super::MobHandle::spawn_held_until`] with entry custody), never a
    /// dormant seat.
    HeldCreation,
    /// A created entry whose host preparation is pending: it has no member
    /// until its host settles the preparation, and no seat takes its name
    /// meanwhile.
    TemplatePreparation,
}

impl UnseatedEntryCause {
    pub(crate) fn from_machine(cause: mob_dsl::UnseatedEntryCause) -> Self {
        match cause {
            mob_dsl::UnseatedEntryCause::SeatOwed => Self::SeatOwed,
            mob_dsl::UnseatedEntryCause::HostedOnFirstUse => Self::HostedOnFirstUse,
            mob_dsl::UnseatedEntryCause::HeldCreation => Self::HeldCreation,
            mob_dsl::UnseatedEntryCause::TemplatePreparation => Self::TemplatePreparation,
        }
    }
}

/// Why an identity's host preparation is owed
/// ([`IdentityEntryView::host_preparation`]). While one is owed, every wake
/// of the identity's member (a host's, a peer's, the boot plan's) is refused
/// with [`crate::MemberWakeRefusal::HostPreparationPending`], retryable.
///
/// MobMachine's own state holds the generated machine enum of the same name
/// and variants (`machines::mob_machine::HostPreparationCause`); this type is
/// its public projection, which a new machine variant extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostPreparationCause {
    /// A first-boot request the host named when it built the mob
    /// ([`super::MobBuilder::with_host_pending_members`]) and has not carried
    /// out: a seat on the request's session, a rotation, or the declared
    /// apply's carry-out of the request.
    FirstBootRow,
    /// The host's roster moved the member to another role without declaring
    /// the migration: the member is held until an apply declares the move or
    /// restores the role.
    RoleMigrationUndeclared,
    /// A build preparation the host asked to retry.
    BuildPreparationRetry,
}

impl HostPreparationCause {
    pub(crate) fn from_machine(cause: mob_dsl::HostPreparationCause) -> Self {
        match cause {
            mob_dsl::HostPreparationCause::FirstBootRow => Self::FirstBootRow,
            mob_dsl::HostPreparationCause::RoleMigrationUndeclared => Self::RoleMigrationUndeclared,
            mob_dsl::HostPreparationCause::BuildPreparationRetry => Self::BuildPreparationRetry,
        }
    }
}

/// Why a member's retirement started, when its journal row records it
/// ([`MemberLifecycleEvent::RetirementStarted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberRetirementCause {
    /// The declared apply's seal unlisted the member's identity entry: the
    /// end of the retirement ends the entry.
    Unlisted,
}

impl MemberRetirementCause {
    /// The stored token of the cause in a retirement start row.
    pub(crate) fn as_stored(self) -> &'static str {
        match self {
            Self::Unlisted => "unlisted",
        }
    }

    /// The cause a retirement start row's stored token names. `None` for a
    /// token this build does not know (a row a later release wrote): the
    /// retirement then replays as one the seal did not start, which keeps the
    /// entry unseated, and the next apply seals the roster again.
    pub(crate) fn from_stored(token: &str) -> Option<Self> {
        match token {
            "unlisted" => Some(Self::Unlisted),
            _ => None,
        }
    }
}

/// The notice that an identity was seated on a fresh session because the
/// archived session it was last bound to is gone from the store: its history
/// is not available to the new session. The declared apply answers it
/// ([`super::DeclaredEntryOutcome::Seated`]) and the seat's journal row
/// records it ([`MemberLifecycleEvent::Spawned`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HistoryUnavailable {
    /// The archived session the identity's member was last bound to.
    pub previous_session: SessionId,
}

impl HistoryUnavailable {
    pub(crate) fn new(previous_session: SessionId) -> Self {
        Self { previous_session }
    }
}

/// The answer of [`super::MobHandle::adopt_legacy_identity_entry`]: the
/// machine's verdict for one legacy member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LegacyIdentityEntryAdoption {
    /// The member is now an identity entry; the adoption is journaled.
    Adopted,
    /// The identity is an entry already: nothing changed, and its spawn row
    /// was not read.
    AlreadyAnEntry,
    /// A worker holds the name: the member's spawn row carries neither the
    /// durable-identity continuity marker nor the legacy identity label
    /// naming its own id.
    NameHeldByWorker,
    /// The member is placed on a member host, bound to no session, or
    /// retiring: it is not adopted.
    NotAdoptable,
    /// No member holds the identity.
    NotSeated,
    /// This composition of the mob is not authoritative: it adopts nothing.
    NonAuthoritative,
}

/// A seated member's facts, as MobMachine records them. Never says whether
/// the member is materialized in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SeatedMember {
    /// The incarnation's generation.
    pub generation: Generation,
    /// The session the member is bound to; `None` for a member bound to no
    /// session (a peer-only member, or one whose retirement released it).
    pub session_id: Option<SessionId>,
    /// The profile the incarnation runs.
    pub profile: ProfileName,
    /// The incarnation's runtime mode.
    pub runtime_mode: crate::MobRuntimeMode,
}

/// Where an identity's member stands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryStanding {
    /// A member holds the identity.
    Seated(SeatedMember),
    /// The member's retirement started and has not finished.
    Retiring(SeatedMember),
    /// No member holds the identity entry, for a typed cause. A worker is
    /// never unseated: a name no member holds is no worker.
    Unseated(UnseatedEntryCause),
}

/// A member MobMachine records Broken: its restore failed, and the failure's
/// stable code when the refusal named one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberBrokenFact {
    /// The failure's display reason.
    pub reason: String,
    /// The stable refusal code, when the failure came from a host refusal.
    pub code: Option<String>,
}

/// An identity entry, as MobMachine records it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityEntryView {
    /// The entry's identity.
    pub identity: AgentIdentity,
    /// How the identity became an entry.
    pub origin: IdentityEntryOrigin,
    /// Where its member stands.
    pub standing: EntryStanding,
    /// Set when the member is Broken.
    pub broken: Option<MemberBrokenFact>,
    /// The host preparation the identity owes, with its cause: while one is
    /// owed, its member's wakes are refused, retryable.
    pub host_preparation: Option<HostPreparationCause>,
    /// The target of the identity's rotation in flight, if one is: a new
    /// session (a reset, a deletion), or the member's own session (a
    /// same-session successor).
    pub rotation_in_flight: Option<super::RotationTarget>,
}

/// A worker member: a member whose identity is not an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkerMember {
    /// Where the member stands.
    pub standing: EntryStanding,
    /// Set when the member is Broken.
    pub broken: Option<MemberBrokenFact>,
}

/// The answer of [`super::MobHandle::identity_entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityEntryRead {
    /// The identity is no entry: a worker member holds it, or nothing does.
    NotAnEntry { worker: Option<WorkerMember> },
    /// The identity is an entry.
    Entry(IdentityEntryView),
    /// The identity's entry is one this build cannot read: its created entry
    /// row is in a format this build does not know, the journal contradicts
    /// the row, or its member's row names an origin this build does not
    /// know. The name is reserved: it keeps what the journal recorded for
    /// it, and nothing new acts on it (no spawn or seat starts, and no
    /// declaration, worker reconcile, adoption or agent-lane command acts).
    /// `member` is where the member that holds the name stands, if one does.
    Unreadable { member: Option<EntryStanding> },
    /// The identity was a created entry and was removed: its name is never
    /// an entry or a member again.
    Removed,
}

/// One page of [`super::MobHandle::identity_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityEntryPage {
    /// The entries after the page's start, in identity order.
    pub entries: Vec<IdentityEntryView>,
    /// The names of the page whose entry this build cannot read
    /// ([`IdentityEntryRead::Unreadable`]), in identity order: listed here,
    /// never in `entries`. Each counts toward the page's limit.
    pub unreadable: Vec<AgentIdentity>,
    /// The identity to start the next page after; `None` on the last page.
    pub next: Option<AgentIdentity>,
}

/// The unseated entries of an [`IdentityCensus`], by cause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnseatedEntryCounts {
    /// Entries owed a seat ([`UnseatedEntryCause::SeatOwed`]).
    pub seat_owed: usize,
    /// Entries hosted on their first use
    /// ([`UnseatedEntryCause::HostedOnFirstUse`]).
    pub hosted_on_first_use: usize,
    /// Entries awaiting their held creation
    /// ([`UnseatedEntryCause::HeldCreation`]).
    pub held_creation: usize,
    /// Created entries awaiting their host's preparation
    /// ([`UnseatedEntryCause::TemplatePreparation`]).
    pub template_preparation: usize,
}

/// The counts of [`super::MobHandle::identity_census`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityCensus {
    /// Identity entries the host declared.
    pub declared: usize,
    /// Identity entries created at runtime
    /// ([`IdentityEntryOrigin::Created`]).
    pub created: usize,
    /// Entries whose member is seated and not retiring.
    pub seated: usize,
    /// Entries whose member's retirement is in flight.
    pub retiring: usize,
    /// Entries no member holds, by cause.
    pub unseated_by_cause: UnseatedEntryCounts,
    /// Entries whose member is Broken.
    pub broken: usize,
    /// Entries whose host preparation is owed, whatever its cause.
    pub host_preparation_pending: usize,
    /// Entries with a rotation in flight.
    pub rotation_in_flight: usize,
    /// Names of removed created entries ([`IdentityEntryRead::Removed`]).
    pub tombstoned: usize,
    /// Names whose entry this build cannot read
    /// ([`IdentityEntryRead::Unreadable`]). They are counted here alone,
    /// and while there is one the census is incomplete
    /// ([`Self::is_complete`]).
    pub unreadable: usize,
}

impl IdentityCensus {
    /// Whether the census counts every identity entry of the mob: `false`
    /// while a name's entry is unreadable ([`Self::unreadable`]), since the
    /// other counts cannot include what this build cannot read.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.unreadable == 0
    }
}

/// A position in a mob's journal: the cursor of the last event a reader
/// consumed. [`Self::START`] is before the first event.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct MobJournalCursor(u64);

impl MobJournalCursor {
    /// Before the first event.
    pub const START: Self = Self(0);

    /// The cursor of a journal event.
    #[must_use]
    pub const fn new(cursor: u64) -> Self {
        Self(cursor)
    }

    /// The raw cursor.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One member lifecycle fact of the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberLifecycleEvent {
    /// A member incarnation was seated: spawned, seated dormant, or seated by
    /// a rotation's terminal. `identity_entry` is set for the member of an
    /// identity entry, and `history_unavailable` for an identity seated on a
    /// fresh session because its archived session is gone from the store.
    Spawned {
        agent_identity: AgentIdentity,
        generation: Generation,
        role: ProfileName,
        identity_entry: Option<IdentityEntryOrigin>,
        history_unavailable: Option<HistoryUnavailable>,
    },
    /// A legacy member was adopted as an identity entry.
    EntryAdopted {
        agent_identity: AgentIdentity,
        generation: Generation,
        origin: IdentityEntryOrigin,
    },
    /// A member's retirement started. `rotation` is set when the retirement
    /// carries out a rotation, with its target: its terminal seats the
    /// identity's next incarnation in the same step (a `Retired` and a
    /// `Spawned` fact). `cause` is set when the journal records why the
    /// retirement started: the declared apply unlisted the identity
    /// ([`MemberRetirementCause::Unlisted`]), and the retirement's end ends
    /// its entry.
    RetirementStarted {
        agent_identity: AgentIdentity,
        generation: Generation,
        rotation: Option<super::RotationTarget>,
        cause: Option<MemberRetirementCause>,
    },
    /// A member incarnation was retired.
    Retired {
        agent_identity: AgentIdentity,
        generation: Generation,
        role: ProfileName,
    },
    /// A legacy member reset carried the member to its next generation.
    Reset {
        agent_identity: AgentIdentity,
        previous_generation: Generation,
        new_generation: Generation,
    },
    /// The mob was reset: no member before it is a member after it.
    MobReset,
}

/// One journal event of [`MemberLifecyclePage`], at its cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberLifecycleRecord {
    /// The event's journal cursor.
    pub cursor: MobJournalCursor,
    /// The lifecycle fact.
    pub event: MemberLifecycleEvent,
}

/// One page of [`super::MobHandle::member_lifecycle_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberLifecyclePage {
    /// The page's member lifecycle facts, in journal order.
    pub records: Vec<MemberLifecycleRecord>,
    /// The cursor to read the next page after: the last journal event this
    /// page read, a lifecycle fact or not. The page is the last one for now
    /// when it read nothing (`next` is the cursor it was asked for).
    pub next: MobJournalCursor,
}

// ---------------------------------------------------------------------------
// Reads over the published machine state
// ---------------------------------------------------------------------------

/// Where `identity`'s member stands, or `None` when no member holds it. A
/// seated member always has a generation, a profile and a runtime mode, and
/// a bound session parses (MobMachine's own invariants): a missing or
/// malformed fact is a defect, answered as one, never filled in.
fn seated_member(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> Result<Option<EntryStanding>, MobError> {
    let Some(runtime_id) = state.identity_to_runtime.get(identity) else {
        return Ok(None);
    };
    let missing = |fact: &str| {
        MobError::Internal(format!(
            "seated member '{}' holds no {fact} in the machine state",
            identity.0
        ))
    };
    let generation = state
        .identity_runtime_generations
        .get(identity)
        .map(|generation| Generation::new(generation.0))
        .ok_or_else(|| missing("generation"))?;
    let session_id = state
        .member_session_bindings
        .get(identity)
        .map(|session| SessionId::parse(&session.0))
        .transpose()
        .map_err(|error| {
            MobError::Internal(format!(
                "seated member '{}' holds an invalid session binding: {error}",
                identity.0
            ))
        })?;
    let profile = state
        .member_profile_names
        .get(identity)
        .map(|profile| ProfileName::from(profile.as_str()))
        .ok_or_else(|| missing("profile"))?;
    let runtime_mode = state
        .member_runtime_modes
        .get(identity)
        .copied()
        .map(crate::MobRuntimeMode::from)
        .ok_or_else(|| missing("runtime mode"))?;
    let member = SeatedMember {
        generation,
        session_id,
        profile,
        runtime_mode,
    };
    Ok(Some(
        if state.member_state_markers.get(runtime_id) == Some(&mob_dsl::MobMemberState::Retiring) {
            EntryStanding::Retiring(member)
        } else {
            EntryStanding::Seated(member)
        },
    ))
}

fn broken_fact(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> Option<MemberBrokenFact> {
    state
        .member_restore_failures
        .get(identity)
        .map(|reason| MemberBrokenFact {
            reason: reason.clone(),
            code: state.member_restore_failure_codes.get(identity).cloned(),
        })
}

fn entry_view(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
    origin: mob_dsl::IdentityEntryOrigin,
) -> Result<IdentityEntryView, MobError> {
    // Every entry is seated or unseated with a typed cause
    // (entry_is_seated_or_has_an_unseated_cause): an entry with neither is a
    // machine defect, answered as one, never as no entry.
    let standing = match seated_member(state, identity)? {
        Some(standing) => standing,
        None => state
            .identity_entry_unseated
            .get(identity)
            .copied()
            .map(|cause| EntryStanding::Unseated(UnseatedEntryCause::from_machine(cause)))
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "identity entry '{}' has neither a seated member nor an unseated cause in \
                     the machine state",
                    identity.0
                ))
            })?,
    };
    Ok(IdentityEntryView {
        identity: AgentIdentity::from(identity.0.as_str()),
        origin: IdentityEntryOrigin::from_machine(origin),
        standing,
        broken: broken_fact(state, identity),
        host_preparation: state
            .member_host_preparation_causes
            .get(identity)
            .copied()
            .map(HostPreparationCause::from_machine),
        rotation_in_flight: state
            .member_rotation_targets
            .get(identity)
            .copied()
            .map(super::RotationTarget::from_machine),
    })
}

/// Whether a member holds `identity` in one published machine state.
pub(crate) fn machine_seats(state: &mob_dsl::MobMachineState, identity: &AgentIdentity) -> bool {
    state
        .identity_to_runtime
        .contains_key(&mob_dsl::AgentIdentity::from_domain(identity))
}

/// [`super::MobHandle::identity_entry`] over one published machine state.
pub(crate) fn identity_entry_read(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
) -> Result<IdentityEntryRead, MobError> {
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
    // A name whose entry this build cannot read is reserved, whatever else
    // the machine records for it.
    if state.identity_entry_unreadable.contains(&dsl_identity) {
        return Ok(IdentityEntryRead::Unreadable {
            member: seated_member(state, &dsl_identity)?,
        });
    }
    if state.identity_entry_tombstones.contains(&dsl_identity) {
        return Ok(IdentityEntryRead::Removed);
    }
    if let Some(origin) = state.identity_entry_origins.get(&dsl_identity) {
        return Ok(IdentityEntryRead::Entry(entry_view(
            state,
            &dsl_identity,
            *origin,
        )?));
    }
    Ok(IdentityEntryRead::NotAnEntry {
        worker: seated_member(state, &dsl_identity)?.map(|standing| WorkerMember {
            standing,
            broken: broken_fact(state, &dsl_identity),
        }),
    })
}

/// [`super::MobHandle::identity_entries`] over one published machine state.
pub(crate) fn identity_entry_page(
    state: &mob_dsl::MobMachineState,
    after: Option<&AgentIdentity>,
    limit: usize,
) -> Result<IdentityEntryPage, MobError> {
    let limit = limit.clamp(1, IDENTITY_ENTRY_PAGE_MAX);
    let start = match after {
        Some(after) => Bound::Excluded(mob_dsl::AgentIdentity::from_domain(after)),
        None => Bound::Unbounded,
    };
    // The entries and the unreadable names, merged in identity order: an
    // unreadable name is listed once, as unreadable, even when the machine
    // records an origin for it.
    let mut origins = state
        .identity_entry_origins
        .range((start.clone(), Bound::Unbounded))
        .peekable();
    let mut unreadable = state
        .identity_entry_unreadable
        .range((start, Bound::Unbounded))
        .peekable();
    let mut page = IdentityEntryPage {
        entries: Vec::new(),
        unreadable: Vec::new(),
        next: None,
    };
    let mut last = None;
    for _ in 0..limit {
        let unreadable_first = match (origins.peek(), unreadable.peek()) {
            (None, None) => break,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (Some((entry, _)), Some(name)) => name <= entry,
        };
        if unreadable_first {
            let Some(name) = unreadable.next() else {
                break;
            };
            if origins.peek().is_some_and(|(entry, _)| *entry == name) {
                origins.next();
            }
            page.unreadable.push(AgentIdentity::from(name.0.as_str()));
            last = Some(name);
        } else {
            let Some((identity, origin)) = origins.next() else {
                break;
            };
            page.entries.push(entry_view(state, identity, *origin)?);
            last = Some(identity);
        }
    }
    if origins.peek().is_some() || unreadable.peek().is_some() {
        page.next = last.map(|identity| AgentIdentity::from(identity.0.as_str()));
    }
    Ok(page)
}

/// [`super::MobHandle::identity_census`] over one published machine state.
pub(crate) fn identity_census(
    state: &mob_dsl::MobMachineState,
) -> Result<IdentityCensus, MobError> {
    let mut census = IdentityCensus {
        tombstoned: state.identity_entry_tombstones.len(),
        unreadable: state.identity_entry_unreadable.len(),
        ..IdentityCensus::default()
    };
    for (identity, origin) in &state.identity_entry_origins {
        // An unreadable name is counted as unreadable alone.
        if state.identity_entry_unreadable.contains(identity) {
            continue;
        }
        let view = entry_view(state, identity, *origin)?;
        match view.origin {
            IdentityEntryOrigin::Declared => census.declared += 1,
            IdentityEntryOrigin::Created => census.created += 1,
        }
        match view.standing {
            EntryStanding::Seated(_) => census.seated += 1,
            EntryStanding::Retiring(_) => census.retiring += 1,
            EntryStanding::Unseated(UnseatedEntryCause::SeatOwed) => {
                census.unseated_by_cause.seat_owed += 1;
            }
            EntryStanding::Unseated(UnseatedEntryCause::HostedOnFirstUse) => {
                census.unseated_by_cause.hosted_on_first_use += 1;
            }
            EntryStanding::Unseated(UnseatedEntryCause::HeldCreation) => {
                census.unseated_by_cause.held_creation += 1;
            }
            EntryStanding::Unseated(UnseatedEntryCause::TemplatePreparation) => {
                census.unseated_by_cause.template_preparation += 1;
            }
        }
        if view.broken.is_some() {
            census.broken += 1;
        }
        if view.host_preparation.is_some() {
            census.host_preparation_pending += 1;
        }
        if view.rotation_in_flight.is_some() {
            census.rotation_in_flight += 1;
        }
    }
    Ok(census)
}

// ---------------------------------------------------------------------------
// The journal read
// ---------------------------------------------------------------------------

/// The member lifecycle fact one journal row records, if any.
fn member_lifecycle_event(kind: &MobEventKind) -> Option<MemberLifecycleEvent> {
    match kind {
        MobEventKind::MemberSpawned(spawned) => Some(MemberLifecycleEvent::Spawned {
            agent_identity: spawned.agent_identity.clone(),
            generation: spawned.generation,
            role: spawned.role.clone(),
            identity_entry: spawned.identity_entry(),
            history_unavailable: spawned.history_unavailable().cloned(),
        }),
        MobEventKind::MemberSessionBindingRecovered(recovered) => {
            recovered
                .identity_entry()
                .map(|origin| MemberLifecycleEvent::EntryAdopted {
                    agent_identity: recovered.agent_identity.clone(),
                    generation: recovered.agent_runtime_id.generation,
                    origin,
                })
        }
        MobEventKind::MemberRetirementStarted {
            agent_identity,
            generation,
            ..
        } => Some(MemberLifecycleEvent::RetirementStarted {
            agent_identity: agent_identity.clone(),
            generation: *generation,
            rotation: kind
                .retirement_rotation()
                .map(super::MemberRotationRecord::target),
            cause: kind.retirement_cause(),
        }),
        MobEventKind::MemberRetired {
            agent_identity,
            generation,
            role,
            ..
        } => Some(MemberLifecycleEvent::Retired {
            agent_identity: agent_identity.clone(),
            generation: *generation,
            role: role.clone(),
        }),
        MobEventKind::MemberReset {
            agent_identity,
            previous_generation,
            new_generation,
            ..
        } => Some(MemberLifecycleEvent::Reset {
            agent_identity: agent_identity.clone(),
            previous_generation: *previous_generation,
            new_generation: *new_generation,
        }),
        MobEventKind::MobReset => Some(MemberLifecycleEvent::MobReset),
        _ => None,
    }
}

/// One [`MemberLifecyclePage`] from the journal events a store poll after
/// `after` returned: `mob_id`'s member lifecycle facts, and the cursor of the
/// last event read.
pub(crate) fn member_lifecycle_page(
    mob_id: &MobId,
    after: MobJournalCursor,
    events: &[MobEvent],
) -> MemberLifecyclePage {
    let mut next = after;
    let mut records = Vec::new();
    for event in events {
        next = next.max(MobJournalCursor::new(event.cursor));
        if &event.mob_id != mob_id {
            continue;
        }
        if let Some(lifecycle) = member_lifecycle_event(&event.kind) {
            records.push(MemberLifecycleRecord {
                cursor: MobJournalCursor::new(event.cursor),
                event: lifecycle,
            });
        }
    }
    MemberLifecyclePage { records, next }
}

// ---------------------------------------------------------------------------
// The legacy adoption's observation
// ---------------------------------------------------------------------------

/// The shell's raw observation of a legacy roster member: the spawn row of
/// the binding's incarnation carries the durable-identity continuity marker,
/// or the legacy identity label naming the binding's own identity. The one
/// place a label is read as a lifecycle input; MobMachine decides the
/// adoption from it ([`super::MobHandle::adopt_legacy_identity_entry`]).
pub(crate) fn legacy_roster_member(binding: &super::MemberBinding) -> bool {
    matches!(
        binding.continuity_intent(),
        super::SpawnContinuityIntent::DurableIdentity { .. }
    ) || binding
        .labels()
        .get(LEGACY_IDENTITY_LABEL)
        .is_some_and(|named| named.as_str() == binding.identity().as_str())
}

/// The machine's adoption verdict as the handle's answer; `None` for
/// [`mob_dsl::LegacyIdentityEntryAdoptionKind::MobNotRunning`] (answered as
/// an invalid transition) and [`mob_dsl::LegacyIdentityEntryAdoptionKind::StaleObservation`]
/// (observed again).
pub(crate) fn legacy_adoption_answer(
    verdict: mob_dsl::LegacyIdentityEntryAdoptionKind,
) -> Option<LegacyIdentityEntryAdoption> {
    match verdict {
        mob_dsl::LegacyIdentityEntryAdoptionKind::Adopted => {
            Some(LegacyIdentityEntryAdoption::Adopted)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::AlreadyAnEntry => {
            Some(LegacyIdentityEntryAdoption::AlreadyAnEntry)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::NameHeldByWorker => {
            Some(LegacyIdentityEntryAdoption::NameHeldByWorker)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::NotAdoptable => {
            Some(LegacyIdentityEntryAdoption::NotAdoptable)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::NotSeated => {
            Some(LegacyIdentityEntryAdoption::NotSeated)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::NonAuthoritative => {
            Some(LegacyIdentityEntryAdoption::NonAuthoritative)
        }
        mob_dsl::LegacyIdentityEntryAdoptionKind::StaleObservation
        | mob_dsl::LegacyIdentityEntryAdoptionKind::MobNotRunning => None,
    }
}

/// The entry origins a published state records, by identity: the projection
/// the member rows of a spawn, a seat and a rotation read their
/// `identity_entry` from.
pub(crate) fn machine_entry_origin(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
) -> Option<IdentityEntryOrigin> {
    state
        .identity_entry_origins
        .get(&mob_dsl::AgentIdentity::from_domain(identity))
        .copied()
        .map(IdentityEntryOrigin::from_machine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemberSpawnedEvent;
    use crate::ids::{AgentRuntimeId, FenceToken};
    use crate::runtime::{
        MemberBinding, MemberBindingState, MemberIdentityManagement, SpawnContinuityIntent,
    };

    fn binding(
        identity: &str,
        continuity_intent: SpawnContinuityIntent,
        label: Option<&str>,
    ) -> MemberBinding {
        let binding = MemberBinding::for_test(
            AgentIdentity::from(identity),
            Generation::new(0),
            Some(SessionId::new()),
            MemberBindingState::Live,
            ProfileName::from("worker"),
            continuity_intent,
            MemberIdentityManagement::CommandDriven,
        );
        match label {
            Some(named) => binding.with_labels(std::collections::BTreeMap::from([(
                LEGACY_IDENTITY_LABEL.to_string(),
                named.to_string(),
            )])),
            None => binding,
        }
    }

    /// The legacy observation reads the durable-identity continuity marker,
    /// or the legacy label naming the member's own identity, and nothing
    /// else.
    #[test]
    fn the_legacy_observation_reads_the_marker_or_the_own_identity_label() {
        let durable = SpawnContinuityIntent::DurableIdentity {
            continuity_key: "legacy".to_string(),
        };
        assert!(legacy_roster_member(&binding("legacy", durable, None)));
        assert!(legacy_roster_member(&binding(
            "legacy",
            SpawnContinuityIntent::Ephemeral,
            Some("legacy"),
        )));
        assert!(!legacy_roster_member(&binding(
            "worker",
            SpawnContinuityIntent::Ephemeral,
            Some("someone-else"),
        )));
        assert!(!legacy_roster_member(&binding(
            "worker",
            SpawnContinuityIntent::Ephemeral,
            None,
        )));
    }

    /// The machine's two verdicts that are not answers (a stale observation
    /// is observed again, a mob that is not running is refused) map to none.
    #[test]
    fn the_adoption_answer_maps_every_verdict() {
        use mob_dsl::LegacyIdentityEntryAdoptionKind as Kind;
        for (verdict, answer) in [
            (Kind::Adopted, Some(LegacyIdentityEntryAdoption::Adopted)),
            (
                Kind::AlreadyAnEntry,
                Some(LegacyIdentityEntryAdoption::AlreadyAnEntry),
            ),
            (
                Kind::NameHeldByWorker,
                Some(LegacyIdentityEntryAdoption::NameHeldByWorker),
            ),
            (
                Kind::NotAdoptable,
                Some(LegacyIdentityEntryAdoption::NotAdoptable),
            ),
            (
                Kind::NotSeated,
                Some(LegacyIdentityEntryAdoption::NotSeated),
            ),
            (
                Kind::NonAuthoritative,
                Some(LegacyIdentityEntryAdoption::NonAuthoritative),
            ),
            (Kind::StaleObservation, None),
            (Kind::MobNotRunning, None),
        ] {
            assert_eq!(legacy_adoption_answer(verdict), answer, "{verdict:?}");
        }
    }

    /// Seat `name` as MobMachine seats a member (its runtime, generation,
    /// profile and runtime mode), an identity entry when `entry`.
    fn seat(state: &mut mob_dsl::MobMachineState, name: &str, entry: bool) {
        let identity = mob_dsl::AgentIdentity::from(name);
        state.identity_to_runtime.insert(
            identity.clone(),
            mob_dsl::AgentRuntimeId::from(format!("{name}:0")),
        );
        state
            .identity_runtime_generations
            .insert(identity.clone(), mob_dsl::Generation(0));
        state
            .member_profile_names
            .insert(identity.clone(), "worker".to_string());
        state.member_runtime_modes.insert(
            identity.clone(),
            mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
        );
        if entry {
            state
                .identity_entry_origins
                .insert(identity, mob_dsl::IdentityEntryOrigin::Declared);
        }
    }

    /// Entries page in identity order, the next page starting after the
    /// last identity of the previous one; a worker is never listed, and the
    /// census counts entries only.
    #[test]
    fn entries_page_in_identity_order_and_the_census_counts_entries_only() {
        let mut state = mob_dsl::MobMachineState::default();
        for name in ["entry-c", "entry-a", "entry-b"] {
            seat(&mut state, name, true);
        }
        seat(&mut state, "worker", false);
        let first = identity_entry_page(&state, None, 2).expect("the first page");
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.identity.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["entry-a", "entry-b"]
        );
        assert_eq!(first.next, Some(AgentIdentity::from("entry-b")));
        let second = identity_entry_page(&state, first.next.as_ref(), 2).expect("the second page");
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].identity, AgentIdentity::from("entry-c"));
        assert_eq!(second.next, None);
        assert_eq!(
            identity_entry_page(&state, None, 0)
                .expect("a page")
                .entries
                .len(),
            1,
            "a zero limit reads one entry"
        );
        let census = identity_census(&state).expect("the census");
        assert_eq!((census.declared, census.seated, census.retiring), (3, 3, 0));
        assert!(matches!(
            identity_entry_read(&state, &AgentIdentity::from("worker")),
            Ok(IdentityEntryRead::NotAnEntry { worker: Some(_) })
        ));
        assert!(matches!(
            identity_entry_read(&state, &AgentIdentity::from("nobody")),
            Ok(IdentityEntryRead::NotAnEntry { worker: None })
        ));
    }

    /// A machine fact the reads need that is missing is a defect, answered
    /// as one: an entry with no seated member is not read as no entry, and a
    /// seated member's missing profile or runtime mode is not filled in.
    #[test]
    fn the_entry_reads_fill_in_no_missing_machine_fact() {
        let mut memberless = mob_dsl::MobMachineState::default();
        memberless.identity_entry_origins.insert(
            mob_dsl::AgentIdentity::from("memberless"),
            mob_dsl::IdentityEntryOrigin::Declared,
        );
        for read in [
            identity_entry_read(&memberless, &AgentIdentity::from("memberless")).map(|_| ()),
            identity_entry_page(&memberless, None, IDENTITY_ENTRY_PAGE_MAX).map(|_| ()),
            identity_census(&memberless).map(|_| ()),
        ] {
            assert!(matches!(read, Err(MobError::Internal(_))), "{read:?}");
        }

        for fact in ["profile", "runtime mode", "generation"] {
            let mut state = mob_dsl::MobMachineState::default();
            seat(&mut state, "entry", true);
            seat(&mut state, "worker", false);
            match fact {
                "profile" => state.member_profile_names.clear(),
                "runtime mode" => state.member_runtime_modes.clear(),
                _ => state.identity_runtime_generations.clear(),
            }
            for name in ["entry", "worker"] {
                let read = identity_entry_read(&state, &AgentIdentity::from(name));
                assert!(
                    matches!(&read, Err(MobError::Internal(reason)) if reason.contains(fact)),
                    "{name} without its {fact}: {read:?}"
                );
            }
        }
    }

    /// An entry no member holds stands unseated for its machine cause, and
    /// the census counts the unseated entries by cause; the host preparation
    /// an entry owes is read with it.
    #[test]
    fn an_unseated_entry_stands_for_its_cause_and_the_census_counts_each_cause() {
        let mut state = mob_dsl::MobMachineState::default();
        seat(&mut state, "seated", true);
        state.member_host_preparation_causes.insert(
            mob_dsl::AgentIdentity::from("seated"),
            mob_dsl::HostPreparationCause::RoleMigrationUndeclared,
        );
        let unseated = [
            (
                "owed",
                mob_dsl::UnseatedEntryCause::SeatOwed,
                UnseatedEntryCause::SeatOwed,
            ),
            (
                "hosted",
                mob_dsl::UnseatedEntryCause::HostedOnFirstUse,
                UnseatedEntryCause::HostedOnFirstUse,
            ),
            (
                "held",
                mob_dsl::UnseatedEntryCause::HeldCreation,
                UnseatedEntryCause::HeldCreation,
            ),
            (
                "preparing",
                mob_dsl::UnseatedEntryCause::TemplatePreparation,
                UnseatedEntryCause::TemplatePreparation,
            ),
        ];
        for (name, machine_cause, _) in unseated {
            let identity = mob_dsl::AgentIdentity::from(name);
            let origin = if machine_cause == mob_dsl::UnseatedEntryCause::TemplatePreparation {
                mob_dsl::IdentityEntryOrigin::Created
            } else {
                mob_dsl::IdentityEntryOrigin::Declared
            };
            state
                .identity_entry_origins
                .insert(identity.clone(), origin);
            state
                .identity_entry_unseated
                .insert(identity, machine_cause);
        }
        for (name, _, cause) in unseated {
            let read = identity_entry_read(&state, &AgentIdentity::from(name));
            assert!(
                matches!(
                    &read,
                    Ok(IdentityEntryRead::Entry(view))
                        if view.standing == EntryStanding::Unseated(cause)
                            && view.host_preparation.is_none()
                ),
                "{name}: {read:?}"
            );
        }
        let read = identity_entry_read(&state, &AgentIdentity::from("seated"));
        assert!(
            matches!(
                &read,
                Ok(IdentityEntryRead::Entry(view))
                    if view.host_preparation == Some(HostPreparationCause::RoleMigrationUndeclared)
            ),
            "{read:?}"
        );
        let census = identity_census(&state).expect("the census");
        assert_eq!((census.declared, census.created, census.seated), (4, 1, 1));
        assert_eq!(
            census.unseated_by_cause,
            UnseatedEntryCounts {
                seat_owed: 1,
                hosted_on_first_use: 1,
                held_creation: 1,
                template_preparation: 1,
            }
        );
        assert_eq!(census.host_preparation_pending, 1);
        assert!(census.is_complete());
    }

    /// A reserved name reads as what reserves it, never as an entry or a
    /// worker: an unreadable name with the standing of the member that holds
    /// it (even when the machine records an origin for it), and a removed
    /// created entry's name as removed. The page lists an unreadable name
    /// among the unreadable ones, in identity order, and never a removed
    /// one; the census counts both apart and is incomplete while a name is
    /// unreadable.
    #[test]
    fn a_reserved_name_reads_unreadable_or_removed_and_the_census_is_incomplete() {
        let mut state = mob_dsl::MobMachineState::default();
        seat(&mut state, "entry-a", true);
        seat(&mut state, "unreadable-member", false);
        seat(&mut state, "unreadable-entry", true);
        for name in ["unreadable-member", "unreadable-entry", "unreadable-free"] {
            state
                .identity_entry_unreadable
                .insert(mob_dsl::AgentIdentity::from(name));
        }
        state
            .identity_entry_tombstones
            .insert(mob_dsl::AgentIdentity::from("removed"));

        for (name, seated) in [
            ("unreadable-member", true),
            ("unreadable-entry", true),
            ("unreadable-free", false),
        ] {
            let read = identity_entry_read(&state, &AgentIdentity::from(name));
            assert!(
                matches!(
                    &read,
                    Ok(IdentityEntryRead::Unreadable { member })
                        if member.is_some() == seated
                ),
                "{name}: {read:?}"
            );
        }
        assert_eq!(
            identity_entry_read(&state, &AgentIdentity::from("removed")).expect("a read"),
            IdentityEntryRead::Removed
        );

        let first = identity_entry_page(&state, None, 2).expect("the first page");
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.identity.as_str())
                .collect::<Vec<_>>(),
            vec!["entry-a"]
        );
        assert_eq!(
            first.unreadable,
            vec![AgentIdentity::from("unreadable-entry")]
        );
        assert_eq!(first.next, Some(AgentIdentity::from("unreadable-entry")));
        let second = identity_entry_page(&state, first.next.as_ref(), 2).expect("the second page");
        assert!(second.entries.is_empty());
        assert_eq!(
            second.unreadable,
            vec![
                AgentIdentity::from("unreadable-free"),
                AgentIdentity::from("unreadable-member"),
            ]
        );
        assert_eq!(second.next, None);

        let census = identity_census(&state).expect("the census");
        assert_eq!((census.declared, census.seated), (1, 1));
        assert_eq!((census.tombstoned, census.unreadable), (1, 3));
        assert!(!census.is_complete());
    }

    /// A retirement cause round-trips through its stored token, and a token
    /// this build does not know reads as no cause.
    #[test]
    fn a_retirement_cause_round_trips_through_its_stored_token() {
        assert_eq!(MemberRetirementCause::Unlisted.as_stored(), "unlisted");
        assert_eq!(
            MemberRetirementCause::from_stored("unlisted"),
            Some(MemberRetirementCause::Unlisted)
        );
        assert_eq!(MemberRetirementCause::from_stored("a_later_cause"), None);
    }

    /// The lifecycle page keeps the mob's member lifecycle facts in journal
    /// order and answers the cursor of the last event it read, a lifecycle
    /// fact or not.
    #[test]
    fn the_lifecycle_page_keeps_the_mobs_lifecycle_facts() {
        let mob = MobId::from("lifecycle-mob");
        let other = MobId::from("other-mob");
        let identity = AgentIdentity::from("entry");
        let spawned = MemberSpawnedEvent::new(
            identity.clone(),
            Generation::new(0),
            FenceToken::new(1),
            AgentRuntimeId::new(identity.clone(), Generation::new(0)),
            ProfileName::from("worker"),
        )
        .with_identity_entry(Some(IdentityEntryOrigin::Declared));
        let event = |cursor: u64, mob_id: &MobId, kind: MobEventKind| MobEvent {
            cursor,
            timestamp: chrono::Utc::now(),
            mob_id: mob_id.clone(),
            kind,
        };
        let events = [
            event(3, &mob, MobEventKind::MemberSpawned(spawned)),
            event(4, &other, MobEventKind::MobReset),
            event(5, &mob, MobEventKind::MobCompleted),
            event(
                6,
                &mob,
                MobEventKind::MemberRetired {
                    agent_identity: identity.clone(),
                    generation: Generation::new(0),
                    role: ProfileName::from("worker"),
                    session_archived: false,
                },
            ),
        ];
        let page = member_lifecycle_page(&mob, MobJournalCursor::new(2), &events);
        assert_eq!(page.next, MobJournalCursor::new(6));
        assert_eq!(
            page.records
                .iter()
                .map(|record| record.cursor)
                .collect::<Vec<_>>(),
            vec![MobJournalCursor::new(3), MobJournalCursor::new(6)]
        );
        assert!(matches!(
            &page.records[0].event,
            MemberLifecycleEvent::Spawned {
                identity_entry: Some(IdentityEntryOrigin::Declared),
                ..
            }
        ));
        let empty = member_lifecycle_page(&mob, MobJournalCursor::new(6), &[]);
        assert_eq!(empty.next, MobJournalCursor::new(6));
        assert!(empty.records.is_empty());
    }
}
