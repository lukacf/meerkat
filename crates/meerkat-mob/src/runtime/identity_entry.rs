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
//! An entry is recorded by the membership commit of a spawn, or by the
//! dormant seat, whose spec carries entry custody, and by the adoption of a
//! legacy member ([`super::MobHandle::adopt_legacy_identity_entry`]). Its
//! journal row records it ([`crate::event::MemberSpawnedEvent`]'s stored
//! `identity_entry`, or the adoption's binding row), so a restart restores it
//! without reading anything else. A rotation keeps it; every retirement
//! removes it.
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
}

impl IdentityEntryOrigin {
    pub(crate) fn from_machine(origin: mob_dsl::IdentityEntryOrigin) -> Self {
        match origin {
            mob_dsl::IdentityEntryOrigin::Declared => Self::Declared,
        }
    }

    pub(crate) fn to_machine(self) -> mob_dsl::IdentityEntryOrigin {
        match self {
            Self::Declared => mob_dsl::IdentityEntryOrigin::Declared,
        }
    }

    /// The stored token of the origin in a journal row.
    pub(crate) fn as_stored(self) -> &'static str {
        match self {
            Self::Declared => "declared",
        }
    }

    /// The origin a journal row's stored token names. `None` for a token
    /// this build does not know (a row a later release wrote): the row then
    /// replays as an ordinary member.
    pub(crate) fn from_stored(token: &str) -> Option<Self> {
        match token {
            "declared" => Some(Self::Declared),
            _ => None,
        }
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
    /// The host named the identity's preparation pending when it built the
    /// mob, and has not carried it out: its wakes are refused, retryable.
    pub host_preparation_pending: bool,
    /// A rotation (a reset or a deletion) of the identity is in flight.
    pub rotation_in_flight: bool,
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
}

/// One page of [`super::MobHandle::identity_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityEntryPage {
    /// The entries after the page's start, in identity order.
    pub entries: Vec<IdentityEntryView>,
    /// The identity to start the next page after; `None` on the last page.
    pub next: Option<AgentIdentity>,
}

/// The counts of [`super::MobHandle::identity_census`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityCensus {
    /// Identity entries the host declared.
    pub declared: usize,
    /// Entries whose member is seated and not retiring.
    pub seated: usize,
    /// Entries whose member's retirement is in flight.
    pub retiring: usize,
    /// Entries whose member is Broken.
    pub broken: usize,
    /// Entries whose host preparation is pending.
    pub host_preparation_pending: usize,
    /// Entries with a rotation in flight.
    pub rotation_in_flight: usize,
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
    /// identity entry.
    Spawned {
        agent_identity: AgentIdentity,
        generation: Generation,
        role: ProfileName,
        identity_entry: Option<IdentityEntryOrigin>,
    },
    /// A legacy member was adopted as an identity entry.
    EntryAdopted {
        agent_identity: AgentIdentity,
        generation: Generation,
        origin: IdentityEntryOrigin,
    },
    /// A member's retirement started.
    RetirementStarted {
        agent_identity: AgentIdentity,
        generation: Generation,
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
    // Every entry is seated (entry_is_seated_or_has_an_unseated_cause): an
    // entry without a member is a machine defect, answered as one, never
    // as no entry.
    let standing = seated_member(state, identity)?.ok_or_else(|| {
        MobError::Internal(format!(
            "identity entry '{}' has no seated member in the machine state",
            identity.0
        ))
    })?;
    Ok(IdentityEntryView {
        identity: AgentIdentity::from(identity.0.as_str()),
        origin: IdentityEntryOrigin::from_machine(origin),
        standing,
        broken: broken_fact(state, identity),
        host_preparation_pending: state.member_host_preparation_pending.contains(identity),
        rotation_in_flight: state.member_rotation_session_ids.contains_key(identity),
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
    let mut range = match after {
        Some(after) => state.identity_entry_origins.range((
            Bound::Excluded(mob_dsl::AgentIdentity::from_domain(after)),
            Bound::Unbounded,
        )),
        None => state
            .identity_entry_origins
            .range::<mob_dsl::AgentIdentity, _>(..),
    };
    let mut entries = Vec::new();
    for (identity, origin) in range.by_ref().take(limit) {
        entries.push(entry_view(state, identity, *origin)?);
    }
    let next = if range.next().is_some() {
        entries.last().map(|entry| entry.identity.clone())
    } else {
        None
    };
    Ok(IdentityEntryPage { entries, next })
}

/// [`super::MobHandle::identity_census`] over one published machine state.
pub(crate) fn identity_census(
    state: &mob_dsl::MobMachineState,
) -> Result<IdentityCensus, MobError> {
    let mut census = IdentityCensus::default();
    for (identity, origin) in &state.identity_entry_origins {
        let view = entry_view(state, identity, *origin)?;
        match view.origin {
            IdentityEntryOrigin::Declared => census.declared += 1,
        }
        match view.standing {
            EntryStanding::Seated(_) => census.seated += 1,
            EntryStanding::Retiring(_) => census.retiring += 1,
        }
        if view.broken.is_some() {
            census.broken += 1;
        }
        if view.host_preparation_pending {
            census.host_preparation_pending += 1;
        }
        if view.rotation_in_flight {
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
        }),
        MobEventKind::MemberRetired {
            agent_identity,
            generation,
            role,
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
