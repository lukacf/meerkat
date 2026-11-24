//! Who the members of a runtime's mobs are private to.
//!
//! A member is private to a person when its roster labels carry a non-blank
//! private owner ([`crate::access::PRIVATE_TO_LABEL`]): a console side chat
//! a signed-in person opened, every member such a member creates (in its own
//! mob, or in a mob it created: a `mob_create` mob, `delegate`'s implicit
//! mob), and a fork of one. Only Meta writes the label on a roster: every
//! caller label path refuses it
//! ([`crate::member_comms_id::RESERVED_AUTHORITY_LABELS`]), the side chat
//! fork writes its creator's, [`PrivateOwners::spawn_customizer`] gives a
//! member of the runtime's own mob the owner of the member that asked for it
//! (or of the member a fork launch copies), and
//! [`PrivateOwners::child_spawn_customizer`] gives every member of a mob a
//! private member created that member's owner. Each such member is seated
//! exempt from its mob definition's wiring rules, so it joins no shared
//! topology and no role-wide flow step.
//!
//! [`PrivateOwners`] knows who each member is private to: each member build
//! records its own labels, unless it is a build of a mob the record knows
//! ended (the base session layer's pre-build hook,
//! [`PrivateOwners::after_hook`]), every mob a member creates records its
//! creator's owner (the agent mob tools' create admission,
//! [`PrivateOwners::before_create_hook`]; a temporary council's mob is
//! created by the member that convened the council), and the seed reads the
//! durable records: the event log of the runtime's mob and of every mob its
//! members created (each spawn with its labels, retired members included), the
//! creation record of each such mob (which member created it), and the
//! retirements the memory store recorded as private
//! ([`PrivateOwners::bind_private_retirements`]). The seed runs in the
//! background from the start and is tried again until it lands
//! ([`PrivateOwners::seed_in_background`]); until then a spawn into a mob
//! whose owner is not known is refused as retryable (a member already seated
//! there is restored and woken as before: its rebuild repeats its seating),
//! and a console with sign-in withholds a member it does not know from
//! everyone but an admin. A mob whose creation record does not say which
//! member created it (several rosters named the session that created it, and
//! the record picks none) gets no owner from the seed: who it is private to
//! cannot be read, so a spawn into it is refused, and the members already
//! seated there keep the owner their own spawn gave them.
//!
//! The end of an ephemeral member private to a person (a side chat, or a
//! member, a fork or a helper such a member made), in the runtime's own mob,
//! is handed to the follower a composition installs
//! ([`PrivateOwners::follow_member_ends`]): the runtime's mob event pump
//! shows each journal event to [`PrivateOwners::observe_mob_event`], which
//! tells an end (a retirement that seats no next incarnation under the name
//! and is no step of a long-term agent's own lifecycle) from a respawn, a
//! reset, a role migration and a rebuild, and reads what the ended
//! incarnation was from Core's member history. An end seen before a
//! follower is installed is held, in memory, and handed over at the
//! install.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock, Weak};

use meerkat_core::service::CreateSessionRequest;
use meerkat_mob::launch::MemberLaunchMode;
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};

use crate::mob_handle_runtime::PreBuildHook;

/// Events read per page while a mob's event log is read.
const SEED_PAGE: usize = 512;

/// The longest wait between two tries of a durable read that failed.
const SEED_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// The longest a reader waits for a first read that is still on its first
/// attempt ([`PrivateOwners::first_attempts_settled`]).
pub(crate) const FIRST_READ_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How far the first reads a record owes have come: the seed, and each
/// read of the private retirements a memory store recorded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FirstReads {
    /// Owed reads still on their first attempt.
    in_flight: usize,
    /// Owed reads whose first attempt failed, or outlasted a reader's wait:
    /// they are tried again, and nobody waits for them.
    overdue: usize,
}

/// The stage of the seed among the first reads a record owes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SeedStage {
    /// No seed was started: a record nothing reads owes none.
    #[default]
    NotOwed,
    /// The seed is on its first attempt.
    InFlight,
    /// The seed failed, or outlasted a reader's wait, and has not landed.
    Overdue,
    Landed,
}

/// The first reads' progress, watched by the readers that wait for it.
struct FirstReadsSignal(tokio::sync::watch::Sender<FirstReads>);

impl Default for FirstReadsSignal {
    fn default() -> Self {
        Self(tokio::sync::watch::Sender::new(FirstReads::default()))
    }
}

/// How far a record has read each mob's durable records: what tells a
/// member the record does not hold because no record names a person for it
/// from a member of a mob the record has not read
/// ([`PrivateOwners::person_for_a_reader`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MobReads {
    /// The mob whose log the seed reads: the launch's own mob, when the
    /// seed was started over its handle
    /// ([`PrivateOwners::seed_in_background`]).
    seed_mob: Option<String>,
    /// The mobs whose durable records this record has read at least once
    /// in this process, by id. A mob created anew under an id leaves it
    /// ([`PrivateOwners::forget_the_read_of`]).
    read: BTreeSet<String>,
    /// The mobs whose read is owed: the host said it is about to build the
    /// mob ([`PrivateOwners::owe_the_read_of`]) and the record has not read
    /// the mob's durable records since.
    owed: BTreeSet<String>,
    /// The mobs the launch composes beside its launch mob whose memory's
    /// private retirements are being read
    /// ([`PrivateOwners::bind_private_retirements_of`]), each with the
    /// number of the binding that reads them: until that binding has read
    /// them, a member of such a mob that the record does not hold is not
    /// known. A mob created anew under an id leaves it
    /// ([`PrivateOwners::forget_the_read_of`]).
    retirements_owed: BTreeMap<String, u64>,
}

/// The mobs' reads, watched by a reader that waits for one mob's read.
struct MobReadsSignal(tokio::sync::watch::Sender<MobReads>);

impl Default for MobReadsSignal {
    fn default() -> Self {
        Self(tokio::sync::watch::Sender::new(MobReads::default()))
    }
}

/// The most ends held for a follower that is not installed yet
/// ([`PrivateOwners::member_ended`]); the oldest is dropped past it.
const HELD_ENDS_MAX: usize = 1024;

/// How often the member history is read for one retiring member before its
/// end is left unfollowed, when the read fails with an error that can pass
/// ([`history_read_can_pass`]).
const HISTORY_READ_ATTEMPTS: u32 = 3;

/// Whether a failed read of a mob's member history can pass when it is
/// asked again: a store error, other than the answer of a store that keeps
/// no member history. That answer, and a refusal of the read, stay as they
/// are however often the read is asked.
fn history_read_can_pass(error: &meerkat_mob::MobError) -> bool {
    match error {
        meerkat_mob::MobError::StorageError(source) => !matches!(
            source.downcast_ref::<meerkat_mob::MobStoreError>(),
            Some(meerkat_mob::MobStoreError::IdentityMemberAtomicPersistenceUnavailable)
        ),
        _ => false,
    }
}

/// The member history `read` answers for the retiring `member` of the mob
/// `mob_id`. A read that fails with an error that can pass is asked again
/// after a wait, [`HISTORY_READ_ATTEMPTS`] times in all; any other failure
/// is answered at once. Without a history the answer is none, with one
/// warning.
async fn read_member_history<Read, Reading>(
    mob_id: &str,
    member: &str,
    mut read: Read,
) -> Option<meerkat_mob::MemberBindingHistory>
where
    Read: FnMut() -> Reading,
    Reading: Future<Output = Result<meerkat_mob::MemberBindingHistory, meerkat_mob::MobError>>,
{
    let mut wait = std::time::Duration::from_millis(250);
    let mut attempt = 1;
    loop {
        match read().await {
            Ok(history) => return Some(history),
            Err(error) if attempt < HISTORY_READ_ATTEMPTS && history_read_can_pass(&error) => {
                tokio::time::sleep(wait).await;
                wait *= 4;
                attempt += 1;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    mob_id = %mob_id,
                    member = %member,
                    "the member history of a retiring member could not be read: if the member \
                     was private to a person, what follows its end is not done"
                );
                return None;
            }
        }
    }
}

/// A member, keyed as its mob's roster names it: (mob id, roster member id).
type MemberKey = (String, String);

/// A member private to a person that ended: its retirement seats no next
/// incarnation under its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndedPrivateMember {
    pub(crate) mob_id: String,
    /// The member, as its mob's roster named it.
    pub(crate) member: String,
    /// The session the member held at its end, when it held one.
    pub(crate) session: Option<String>,
    /// When its retirement started, as the mob's journal recorded it. What
    /// came to be under the member's name after this moment is not the
    /// ended member's.
    pub(crate) ended_at: chrono::DateTime<chrono::Utc>,
}

/// What a composition does once a member private to a person ended
/// ([`PrivateOwners::follow_member_ends`]).
#[async_trait::async_trait]
pub(crate) trait PrivateMemberEndFollower: Send + Sync {
    /// `ended` ended. Called once per end the record sees, and again for
    /// the same member when its end is seen again (an event delivered a
    /// second time): a follower leaves what it already did as it is.
    async fn member_ended(&self, ended: &EndedPrivateMember);
}

/// Which of a launch's followers of the ends of private members a follower
/// is ([`PrivateOwners::follow_member_ends_as`]). Each seat has one
/// follower, and its own hold of the ends seen before it was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MemberEndSeat {
    /// What becomes of the schedules that target the ended member.
    Schedules,
    /// What becomes of the question sets the ended member asked.
    QuestionSets,
}

impl MemberEndSeat {
    const ALL: [Self; 2] = [Self::Schedules, Self::QuestionSets];
}

/// One seat's follower of the ends of members private to a person, and the
/// ends seen while it had none.
#[derive(Default)]
struct SeatEnds {
    follower: Option<Arc<dyn PrivateMemberEndFollower>>,
    held: Vec<EndedPrivateMember>,
}

/// The followers of the ends of members private to a person, by seat.
#[derive(Default)]
struct MemberEnds {
    seats: BTreeMap<MemberEndSeat, SeatEnds>,
}

/// What one row of a mob's journal says of who holds a member's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HolderStep {
    /// A member was seated under the name: private to `owner`, or to no
    /// one. `ephemeral` for a member that ends with its retirement (a side
    /// chat, a helper, a fork); a member seated as a durable identity is
    /// retired and seated again by rebuilds that keep its agent.
    Seated {
        owner: Option<String>,
        ephemeral: bool,
        seated_as: SeatedAs,
    },
    /// The member under the name was retired, by a retirement that seats no
    /// next member under the name and records no cause: the end of an
    /// ephemeral member, and a rebuild of a durable one.
    Retired,
    /// The identity entry of the member under the name ended: its
    /// retirement records that the identity was removed or unlisted. The
    /// end of a long-term agent. An archive leaves the agent, and a host's
    /// adoption continues the identity: neither is an end.
    Removed,
}

/// The role and the labels a member was seated with, as its seating's row
/// of the journal recorded them. A record made while an earlier seating
/// held a name is judged by what its member was seated as then, besides
/// the member that holds the name now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SeatedAs {
    pub(crate) role: String,
    pub(crate) labels: BTreeMap<String, String>,
}

/// One row of a mob's journal that says who holds a member's name, with the
/// journal's cursor and time of the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberJournalStep {
    pub(crate) mob_id: String,
    /// The member, as its mob's roster names it.
    pub(crate) member: String,
    /// The journal's cursor of the row: the journal's own order.
    pub(crate) cursor: u64,
    /// When the journal recorded the row, in UNIX milliseconds.
    pub(crate) at_ms: u64,
    pub(crate) step: HolderStep,
}

impl MemberJournalStep {
    /// The seating of `member` in mob `mob_id` with `labels`, the journal's
    /// row `cursor`, recorded at `at_ms`.
    pub(crate) fn seated(
        mob_id: &str,
        member: &str,
        labels: &BTreeMap<String, String>,
        ephemeral: bool,
        cursor: u64,
        at_ms: u64,
    ) -> Self {
        Self {
            mob_id: mob_id.to_string(),
            member: member.to_string(),
            cursor,
            at_ms,
            step: HolderStep::Seated {
                owner: crate::access::private_owner(labels).map(str::to_string),
                ephemeral,
                seated_as: SeatedAs {
                    role: String::new(),
                    labels: labels.clone(),
                },
            },
        }
    }

    /// [`Self::seated`], of a member of the role `role`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn seated_as(
        mob_id: &str,
        member: &str,
        role: &str,
        labels: &BTreeMap<String, String>,
        ephemeral: bool,
        cursor: u64,
        at_ms: u64,
    ) -> Self {
        let mut seated = Self::seated(mob_id, member, labels, ephemeral, cursor, at_ms);
        if let HolderStep::Seated { seated_as, .. } = &mut seated.step {
            seated_as.role = role.to_string();
        }
        seated
    }

    /// What `event`, a row of a mob's journal, says of who holds a member's
    /// name. A spawn seats a member. A retirement start that carries no
    /// rotation and keeps no topology for a successor retires it: with no
    /// cause recorded (the test [`PrivateOwners::observe_mob_event`] applies
    /// to an end), or as the end of its identity entry (removed, unlisted).
    /// Every other row says nothing of it.
    pub(crate) fn of_event(event: &meerkat_mob::MobEvent) -> Option<Self> {
        let at_ms = u64::try_from(event.timestamp.timestamp_millis()).unwrap_or(0);
        match &event.kind {
            meerkat_mob::MobEventKind::MemberSpawned(spawned) => Some(Self::seated_as(
                event.mob_id.as_str(),
                spawned.agent_identity.as_str(),
                spawned.role.as_ref(),
                &spawned.labels,
                matches!(
                    spawned.continuity_intent,
                    meerkat_mob::SpawnContinuityIntent::Ephemeral
                ) || spawned
                    .labels
                    .contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL),
                event.cursor,
                at_ms,
            )),
            meerkat_mob::MobEventKind::MemberRetirementStarted {
                agent_identity,
                rotation,
                preserve_machine_topology,
                cause,
                ..
            } if rotation.is_none() && !*preserve_machine_topology => {
                let step = match cause {
                    None => HolderStep::Retired,
                    Some(
                        meerkat_mob::MemberRetirementCause::Removed
                        | meerkat_mob::MemberRetirementCause::Unlisted,
                    ) => HolderStep::Removed,
                    Some(_) => return None,
                };
                Some(Self {
                    mob_id: event.mob_id.to_string(),
                    member: agent_identity.to_string(),
                    cursor: event.cursor,
                    at_ms,
                    step,
                })
            }
            _ => None,
        }
    }
}

/// One page of a mob's journal: the rows that say who holds a member's name
/// ([`MemberJournalStep`]), in the journal's order, and the cursor to read
/// on from.
pub(crate) type MemberSpawnPage = (Vec<MemberJournalStep>, u64);

/// Where the seed reads the members a mob spawned, with the labels of each
/// spawn, and the members it retired: the mob's durable event log.
#[async_trait::async_trait]
pub(crate) trait MemberSpawnLog: Send + Sync {
    /// The member spawns and retirements recorded past `cursor`, reading at
    /// most `limit` events, or `None` once the log holds no event past it.
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String>;

    /// The time the log recorded for its first event: a mob's log begins
    /// with the mob's creation, so this dates the mob the log is of. `None`
    /// for a log that holds no event, and for a log that cannot say.
    async fn first_event_time(&self) -> Result<Option<std::time::SystemTime>, String> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl MemberSpawnLog for meerkat_mob::MobHandle {
    async fn first_event_time(&self) -> Result<Option<std::time::SystemTime>, String> {
        let first = self
            .poll_events(0, 1)
            .await
            .map_err(|error| error.to_string())?;
        Ok(first
            .first()
            .map(|event| std::time::SystemTime::from(event.timestamp)))
    }

    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<MemberSpawnPage>, String> {
        let events = self
            .poll_events(cursor, limit)
            .await
            .map_err(|error| error.to_string())?;
        let Some(last) = events.last().map(|event| event.cursor) else {
            return Ok(None);
        };
        let steps = events
            .iter()
            .filter_map(MemberJournalStep::of_event)
            .collect();
        Ok(Some((steps, last)))
    }
}

/// One step of a name's holders, as noted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotedStep {
    /// The journal's cursor of the step's row.
    cursor: u64,
    /// The time the journal recorded for the row.
    recorded_ms: u64,
    /// The step's time among the rows of its mob's journal
    /// ([`JournalTimes`]): its recorded time, and later than every row
    /// before it in the journal's order. The journal's order is the order
    /// things happened in, also where the clock that stamped the rows
    /// stepped back between two of them.
    at_ms: u64,
    step: HolderStep,
}

/// The times of the rows one mob's journal recorded for its members, by
/// cursor: each row's recorded time, and its time among the rows, which is
/// the recorded time lifted to be later than the row before it, with room
/// for the moment between them: an entry written once a row was shown is
/// stamped past it ([`Self::entry_floor`]) and before the next row. A pure
/// function of the journal's rows, whatever order they are shown in, so a
/// restart reads the same times.
#[derive(Debug, Default)]
struct JournalTimes(BTreeMap<u64, (u64, u64)>);

impl JournalTimes {
    /// Note the row `cursor`, recorded at `recorded_ms`. Answers the row's
    /// time among the rows, and whether the times of rows after it changed
    /// (a row shown out of the journal's order).
    fn note(&mut self, cursor: u64, recorded_ms: u64) -> (u64, bool) {
        if let Some((_, at_ms)) = self.0.get(&cursor) {
            return (*at_ms, false);
        }
        let lifted = |recorded_ms: u64, before: Option<u64>| {
            before.map_or(recorded_ms, |before| {
                recorded_ms.max(before.saturating_add(2))
            })
        };
        let before = self
            .0
            .range(..cursor)
            .next_back()
            .map(|(_, (_, at_ms))| *at_ms);
        let at_ms = lifted(recorded_ms, before);
        self.0.insert(cursor, (recorded_ms, at_ms));
        let mut latest = at_ms;
        let mut moved = false;
        for (_, (recorded_ms, at_ms)) in self.0.range_mut((
            std::ops::Bound::Excluded(cursor),
            std::ops::Bound::Unbounded,
        )) {
            let now = lifted(*recorded_ms, Some(latest));
            moved |= now != *at_ms;
            *at_ms = now;
            latest = now;
        }
        (at_ms, moved)
    }

    /// The time among the rows of the row `cursor`.
    fn at(&self, cursor: u64) -> Option<u64> {
        self.0.get(&cursor).map(|(_, at_ms)| *at_ms)
    }

    /// The time of the newest row.
    fn newest(&self) -> Option<u64> {
        self.0.values().next_back().map(|(_, at_ms)| *at_ms)
    }

    /// The floor of the times of entries written once the newest row was
    /// shown: the moment after that row.
    fn entry_floor(&self) -> Option<u64> {
        self.newest().map(|newest| newest.saturating_add(1))
    }
}

/// Who held one member name of one mob, over time: the steps the mob's
/// journal recorded for the name, in the journal's order. In memory only:
/// it is read again from the journal at each start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NameHolders(Vec<NotedStep>);

/// The position, among `seatings` (each with its time and whether its
/// member is private to a person, oldest first), of the seating that holds
/// the moment `at_ms`: the last one at or before it. A seating private to a
/// person holds every moment from its own up to the next seating's, that
/// moment included: a seating at exactly `at_ms` takes the moment from a
/// private one before it only when it is private itself.
fn holding_seating(seatings: &[(u64, bool)], at_ms: u64) -> Option<usize> {
    let private = |position: &usize| seatings.get(*position).is_some_and(|(_, private)| *private);
    let before = seatings
        .iter()
        .take_while(|(seated_at, _)| *seated_at < at_ms)
        .count()
        .checked_sub(1);
    let at = seatings
        .iter()
        .enumerate()
        .filter(|(_, (seated_at, _))| *seated_at == at_ms)
        .map(|(position, _)| position);
    let (private_at, any_at) = at.fold((None, None), |(private_at, _), position| {
        (
            private_at.or(private(&position).then_some(position)),
            Some(position),
        )
    });
    private_at.or(before.filter(private)).or(any_at).or(before)
}

/// The members seated under one name in one mob, over time: each seating's
/// time and the person its member is private to, if any, oldest first. What
/// a console's access controller holds of who held a name
/// ([`PrivateOwners::name_seatings`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NameSeatings(Vec<(u64, Option<String>)>, Vec<SeatedAs>);

/// Who held a name in one mob at a moment ([`NameSeatings::held_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldBy<'a> {
    /// No member was seated under the name there at or before the moment.
    NoKnownMember,
    /// A member private to this person.
    Private(&'a str),
    /// A member private to no one.
    Shared,
    /// The moment is the one at which a member private to one person was
    /// seated under the name after a member private to another person: what
    /// was recorded at it may be either member's.
    HandedOver,
}

/// Whether `at_ms` is the moment a member private to one person was seated
/// after a member private to another person, among `seatings` (each with
/// its time and the person its member is private to, oldest first).
fn handed_over_at(seatings: &[(u64, Option<String>)], at_ms: u64) -> bool {
    let before = seatings
        .iter()
        .take_while(|(seated_at, _)| *seated_at < at_ms)
        .last()
        .and_then(|(_, owner)| owner.as_deref());
    before.is_some_and(|before| {
        seatings
            .iter()
            .filter(|(seated_at, _)| *seated_at == at_ms)
            .any(|(_, owner)| owner.as_deref().is_some_and(|owner| owner != before))
    })
}

impl NameSeatings {
    /// The seatings `seatings`, oldest first.
    #[cfg(test)]
    pub(crate) fn of(seatings: Vec<(u64, Option<String>)>) -> Self {
        let seated_as = vec![SeatedAs::default(); seatings.len()];
        Self(seatings, seated_as)
    }

    /// The seatings `seatings`, oldest first, each with what its member
    /// was seated as.
    #[cfg(test)]
    pub(crate) fn of_seated(seatings: Vec<(u64, Option<String>, SeatedAs)>) -> Self {
        let (times, seated_as) = seatings
            .into_iter()
            .map(|(at_ms, owner, seated_as)| ((at_ms, owner), seated_as))
            .unzip();
        Self(times, seated_as)
    }

    /// The member private to no one that held the name at `at_ms`, as it
    /// was seated, where a later seating under the name follows its own:
    /// another member that replaced it, or the same member seated again (a
    /// respawn is a seating of its own). The seatings do not tell the two
    /// apart, so a member seated again with another role or other labels is
    /// answered here for what it recorded before. `None` for the seating
    /// that is the name's last (the rule on the name's holder now decides
    /// that one), for a member private to a person, and for a moment no
    /// member holds.
    pub(crate) fn earlier_shared_holder_at(&self, at_ms: u64) -> Option<&SeatedAs> {
        if handed_over_at(&self.0, at_ms) {
            return None;
        }
        let seatings: Vec<(u64, bool)> = self
            .0
            .iter()
            .map(|(seated_at, owner)| (*seated_at, owner.is_some()))
            .collect();
        let position = holding_seating(&seatings, at_ms)?;
        let (_, owner) = self.0.get(position)?;
        if owner.is_some() || position + 1 >= self.0.len() {
            return None;
        }
        self.1.get(position)
    }

    /// Whether a member private to a person was ever seated under the name.
    pub(crate) fn ever_private(&self) -> bool {
        self.0.iter().any(|(_, owner)| owner.is_some())
    }

    /// Who held the name at `at_ms` ([`holding_seating`]).
    pub(crate) fn held_at(&self, at_ms: u64) -> HeldBy<'_> {
        if handed_over_at(&self.0, at_ms) {
            return HeldBy::HandedOver;
        }
        let seatings: Vec<(u64, bool)> = self
            .0
            .iter()
            .map(|(seated_at, owner)| (*seated_at, owner.is_some()))
            .collect();
        match holding_seating(&seatings, at_ms).and_then(|position| self.0.get(position)) {
            Some((_, Some(owner))) => HeldBy::Private(owner),
            Some((_, None)) => HeldBy::Shared,
            None => HeldBy::NoKnownMember,
        }
    }
}

impl NameHolders {
    /// Note the journal's row `cursor`, recorded at `recorded_ms`, whose
    /// time among its journal's rows is `at_ms`, once: the journal shows a
    /// row to the seed's read and to the event pump alike, in either order.
    /// Answers whether the row was new.
    fn note(&mut self, cursor: u64, recorded_ms: u64, at_ms: u64, step: HolderStep) -> bool {
        if self.0.iter().any(|noted| noted.cursor == cursor) {
            return false;
        }
        let after = self.0.partition_point(|noted| noted.cursor < cursor);
        self.0.insert(
            after,
            NotedStep {
                cursor,
                recorded_ms,
                at_ms,
                step,
            },
        );
        true
    }

    /// Take each step's time among its journal's rows from `times` again:
    /// a row shown out of the journal's order moved the times after it.
    fn restamp(&mut self, times: &JournalTimes) {
        for noted in &mut self.0 {
            noted.at_ms = times.at(noted.cursor).unwrap_or(noted.recorded_ms);
        }
    }

    /// The seating at `index`: the person its member is private to, if any,
    /// and whether the member is ephemeral.
    fn seated(&self, index: usize) -> Option<(Option<&str>, bool)> {
        match self.0.get(index).map(|noted| &noted.step) {
            Some(HolderStep::Seated {
                owner, ephemeral, ..
            }) => Some((owner.as_deref(), *ephemeral)),
            _ => None,
        }
    }

    /// The index of the seating that holds the name at `at_ms`
    /// ([`holding_seating`]).
    fn seating_at(&self, at_ms: u64) -> Option<usize> {
        let (indices, seatings): (Vec<usize>, Vec<(u64, bool)>) = self
            .0
            .iter()
            .enumerate()
            .filter_map(|(index, noted)| match &noted.step {
                HolderStep::Seated { owner, .. } => Some((index, (noted.at_ms, owner.is_some()))),
                _ => None,
            })
            .unzip();
        holding_seating(&seatings, at_ms).and_then(|position| indices.get(position).copied())
    }

    /// The name's seatings, each with its time, its owner and what its
    /// member was seated as.
    fn seatings(&self) -> NameSeatings {
        let (times, seated_as) = self
            .0
            .iter()
            .filter_map(|noted| match &noted.step {
                HolderStep::Seated {
                    owner, seated_as, ..
                } => Some(((noted.at_ms, owner.clone()), seated_as.clone())),
                _ => None,
            })
            .unzip();
        NameSeatings(times, seated_as)
    }

    /// The person the member seated at `index` is private to, if any.
    fn owner_of(&self, index: usize) -> Option<&str> {
        self.seated(index).and_then(|(owner, _)| owner)
    }

    /// When the member seated at `index` ended, if it has: the first step
    /// after its seating and before the next one that ends it. A retirement
    /// with no cause ends an ephemeral member; the end of its identity entry
    /// ends any member.
    fn ended(&self, index: usize) -> Option<u64> {
        let (_, ephemeral) = self.seated(index)?;
        self.0
            .iter()
            .skip(index + 1)
            .take_while(|noted| !matches!(noted.step, HolderStep::Seated { .. }))
            .find(|noted| match noted.step {
                HolderStep::Removed => true,
                HolderStep::Retired => ephemeral,
                HolderStep::Seated { .. } => false,
            })
            .map(|noted| noted.at_ms)
    }

    /// The index of the seating the journal's row `cursor` follows.
    fn seating_before(&self, cursor: u64) -> Option<usize> {
        self.0
            .iter()
            .enumerate()
            .take_while(|(_, noted)| noted.cursor < cursor)
            .filter(|(_, noted)| matches!(noted.step, HolderStep::Seated { .. }))
            .last()
            .map(|(index, _)| index)
    }

    /// Whether the member that held the name at `at_ms` holds it now: no
    /// member under the name has ended since, and the newest seating is
    /// private to the same person (or, like it, to no one). A respawn, a
    /// reset and a rebuild seat the same member again.
    fn held_since(&self, at_ms: u64) -> bool {
        // The moment one person's member follows another's is neither's.
        if self.seatings().held_at(at_ms) == HeldBy::HandedOver {
            return false;
        }
        let Some(then) = self.seating_at(at_ms) else {
            return false;
        };
        let mut newest = then;
        for (index, noted) in self.0.iter().enumerate().skip(then) {
            if matches!(noted.step, HolderStep::Seated { .. }) {
                if self.ended(newest).is_some() {
                    return false;
                }
                newest = index;
            }
        }
        self.ended(newest).is_none() && self.owner_of(newest) == self.owner_of(then)
    }

    /// Whether the member that held the name at `at_ms` is private to a
    /// person and is the member last seated under the name, ended or not,
    /// and the one an end at `ended_at_ms` is of: no member under the name
    /// ended between that seating and the newest one, the newest is private
    /// to the same person, and it was seated no later than the end (a
    /// member seated after an end is not the member that ended). A respawn,
    /// a reset and a rebuild seat the same member again.
    fn held_by_the_last_private_member(&self, at_ms: u64, ended_at_ms: u64) -> bool {
        let Some(then) = self.seating_at(at_ms) else {
            return false;
        };
        let mut newest = then;
        for (index, noted) in self.0.iter().enumerate().skip(then + 1) {
            if matches!(noted.step, HolderStep::Seated { .. }) {
                if self.ended(newest).is_some() {
                    return false;
                }
                newest = index;
            }
        }
        self.owner_of(then).is_some()
            && self.owner_of(newest) == self.owner_of(then)
            && self
                .0
                .get(newest)
                .is_some_and(|seated| seated.at_ms <= ended_at_ms)
    }

    /// Whether the member that held the name at `at_ms` is private to a
    /// person and its end is in the journal.
    fn held_by_an_ended_private_member(&self, at_ms: u64) -> bool {
        self.seating_at(at_ms)
            .is_some_and(|index| self.owner_of(index).is_some() && self.ended(index).is_some())
    }
}

/// Which private-owner record of this process a value names: one per
/// runtime, the same through every clone of it, never the same for two
/// records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PrivateOwnerRecordId(u64);

impl Default for PrivateOwnerRecordId {
    /// The id of a new record: each call answers one no record holds.
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// What a create is refused with when a release it asks before its mob
/// exists fails ([`MobCreateRelease`]): one sentence for every release. What
/// the release itself said goes to the host's log.
///
/// It says nothing of whose the kept things were (a first create under an
/// id is asked the same release) and nothing of a retry: the refusal's data
/// says `retryable: false` for every reason, this one included.
pub(crate) const CREATE_RELEASE_FAILED: &str = "what the host keeps under this mob id could not be released, so the mob is not created; \
     the host's log has the cause";

/// One release the host owes before a mob is created under an id
/// ([`PrivateOwners::bind_create_release`]): what the host keeps under that
/// id was not written for the new mob (it is of an earlier mob of the id,
/// or was written before any mob held it), and the new mob is to find none
/// of it.
#[async_trait::async_trait]
pub(crate) trait MobCreateRelease: Send + Sync {
    /// What this releases, in a few words, for the host's log.
    fn what(&self) -> &'static str;

    /// Release what the host keeps under `mob_id`. Asked again, it releases
    /// what is left. An error means some may be left; its text is for the
    /// host's log, never for a caller.
    async fn release_before_create(&self, mob_id: &meerkat_mob::MobId) -> Result<(), String>;

    /// Forget what this holds of `mob_id` in memory that goes only with an
    /// admitted create: asked once every bound release passed and the
    /// record counted the create ([`PrivateOwners::before_create_hook`]),
    /// and of a create a release refuses not at all. It cannot fail and
    /// waits for nothing. A release that holds nothing of that kind
    /// forgets nothing.
    fn forget_at_admission(&self, mob_id: &meerkat_mob::MobId) {
        let _ = mob_id;
    }
}

/// The schedules a launch keeps under a mob's id, as a release a create
/// asks ([`crate::mob_schedules_release`]): a launch that declared no
/// schedules has none to release; one that declared them and has not bound
/// their release yet fails the release, so the create is refused; a bound
/// release ends the schedules that name the id and says in the host's log
/// how many it ended.
#[async_trait::async_trait]
impl MobCreateRelease for crate::mob_schedules_release::MobSchedulesSlot {
    fn what(&self) -> &'static str {
        "the schedules that name the mob"
    }

    async fn release_before_create(&self, mob_id: &meerkat_mob::MobId) -> Result<(), String> {
        use crate::mob_schedules_release::MobSchedulesStanding;
        match self.standing() {
            MobSchedulesStanding::NotDeclared => Ok(()),
            MobSchedulesStanding::Owed => Err(
                "the launch declared its schedules and has not bound their release to the \
                 runtime yet"
                    .to_string(),
            ),
            MobSchedulesStanding::Bound(release) => {
                let ended = release.release_mob(mob_id).await?;
                if ended > 0 {
                    tracing::warn!(
                        mob_id = %mob_id,
                        ended,
                        "a create ended the schedules that named the mob's id"
                    );
                }
                Ok(())
            }
        }
    }
}

/// The owner of each member of one runtime's mobs (see the module docs).
/// Cheap to clone; clones share the record.
#[derive(Clone, Default)]
pub(crate) struct PrivateOwners {
    inner: Arc<Inner>,
}

/// Test builds only: a first read a test makes a record owe
/// ([`PrivateOwners::owe_a_first_read_in_tests`]). Dropping it is the read
/// succeeding.
#[cfg(test)]
pub(crate) struct OwedFirstReadInTests(PrivateOwners);

#[cfg(test)]
impl Drop for OwedFirstReadInTests {
    fn drop(&mut self) {
        self.0
            .inner
            .first_reads
            .0
            .send_modify(|reads| reads.in_flight = reads.in_flight.saturating_sub(1));
    }
}

/// Test builds only: the read of one mob that a test makes a record owe
/// ([`PrivateOwners::owe_the_read_of_in_tests`]). Dropping it is the record
/// having read that mob.
#[cfg(test)]
pub(crate) struct OwedMobReadInTests(PrivateOwners, String);

#[cfg(test)]
impl Drop for OwedMobReadInTests {
    fn drop(&mut self) {
        self.0.inner.mob_reads.0.send_modify(|reads| {
            reads.owed.remove(&self.1);
            reads.read.insert(self.1.clone());
        });
    }
}

/// Who a mob a member created is private to, as far as its records say.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MobOwner {
    /// The creating member's owner, or `None` for a mob private to no one (a
    /// host's mob, a shared member's).
    Known(Option<String>),
    /// The mob's creation record does not say which member created it, so
    /// who the mob is private to cannot be read from it.
    Undecidable,
    /// The mob's records name its creating member, and nothing says for
    /// certain who that member is private to: no mob is hosted or stored
    /// here under its mob's id at the incarnation the record holds, or
    /// that mob's member history holds no binding of the member's session
    /// at the record's generation and its roster no member under the name
    /// at that generation. The mob is not taken to be private to no one.
    /// It is read again at every reading of the mob set
    /// ([`PrivateOwners::read_mobs`]), so it is settled once that member's
    /// mob can be read.
    CreatorUnknown,
}

/// What the runtime's council store says of a mob it hosts
/// ([`PrivateOwners::council_mob`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum CouncilMob {
    /// No council's record owns the mob: who created it is read from its
    /// own creation record.
    Ordinary,
    /// A council's record owns the mob. Inside: the session that convened
    /// the council, or `None` for a council no member's session convened (a
    /// host ran it, or its record is from before conveners were recorded).
    Convened(Option<meerkat_core::types::SessionId>),
    /// The council store could not be read.
    Unreadable,
}

/// The mob of the member that created a mob, as far as its records say:
/// what [`PrivateOwners::memory_home_of`] follows from a mob to the mob of
/// its creating member.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MobCreation {
    /// A member seated in the mob of this id created it.
    ByAMemberOf(String),
    /// The host created it, as its durable creation record says
    /// (`MobProvenance::Host`): on a launch that composes the mobs its
    /// host creates it is a mob of the host of its own, loaded or not
    /// ([`PrivateOwners::is_a_mob_of_the_host`]).
    ByTheHost,
    /// No member created it: a session no roster named, a store from
    /// before creations were recorded, or a create this record admitted
    /// for no member's session.
    ByNoMember,
    /// The record does not say which member created it.
    Undecidable,
}

impl MobCreation {
    /// What the durable creation record `provenance` says.
    fn of(provenance: &meerkat_mob::MobProvenance) -> Self {
        if matches!(provenance, meerkat_mob::MobProvenance::Host) {
            return Self::ByTheHost;
        }
        match creating_member(provenance) {
            CreatingMember::Named(parent, _) => Self::ByAMemberOf(parent.to_string()),
            CreatingMember::NoMember => Self::ByNoMember,
            CreatingMember::Undecidable => Self::Undecidable,
        }
    }
}

/// How many creating mobs a memory home is followed through: the bound the
/// host's own reach question uses for a mob's creation.
const MEMORY_HOME_STEPS: usize = 16;

/// The creates this record's admission admitted, counted by mob id. A
/// reading pass notes the counts before it lists the mobs it reads
/// ([`PrivateOwners::creates_admitted`]): a mob created under an id since
/// then is not the mob the pass holds.
pub(crate) type CreatesAdmitted = BTreeMap<String, u64>;

/// What this record keeps to date the mob that holds an id, under one
/// lock.
#[derive(Default)]
struct MobDates {
    /// The time of the first event of each mob's log, as this record read
    /// it from the mob that holds the id now: that mob's creation. Kept
    /// while the mob is unloaded; replaced by each read of the mob's log,
    /// and dropped by the create admission before it writes the new mob's
    /// creation, so it is never an earlier mob's of the same id.
    first_events: BTreeMap<String, std::time::SystemTime>,
    /// The creates admitted under each id: moved where the admission drops
    /// the id's time, and asked where a read writes one.
    admitted: CreatesAdmitted,
}

/// The incarnations of mobs this record met, each with its mob's id, and
/// the ones it knows ended. An incarnation is met where a member build
/// names it, where the record reads a mob the mob set manages, and where
/// the host asks the mob set to destroy its mob. It is known ended once a
/// mob is created under its mob's id, and once the host's destroy of its
/// mob is done, at once or by a repeat that continued it. A member build
/// of an ended incarnation is of a mob that no longer stands: the record
/// takes no write from it ([`PrivateOwners::record_built`]). In process
/// only.
#[derive(Default)]
struct MobIncarnations {
    met: BTreeSet<(String, meerkat_core::service::MobIncarnationId)>,
    ended: BTreeSet<(String, meerkat_core::service::MobIncarnationId)>,
    /// By mob id, the incarnation the host last asked the mob set to
    /// destroy, until that destroy is done or a mob is created under the
    /// id ([`PrivateOwners::destroy_asked`]).
    destroy_asked: BTreeMap<String, meerkat_core::service::MobIncarnationId>,
}

#[derive(Default)]
struct Inner {
    /// Which record this is among the records of the process.
    id: PrivateOwnerRecordId,
    /// The releases a create asks before its mob exists, in the order they
    /// were bound ([`PrivateOwners::bind_create_release`]).
    create_releases: RwLock<Vec<Arc<dyn MobCreateRelease>>>,
    /// What each member build recorded: the member's owner, or `None` for a
    /// member private to no one. The newest truth about a member.
    built: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// What the event logs say each member's last spawn left it: its owner,
    /// or `None` for a member private to no one.
    logged: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// The identities, as their memory scopes name them, whose retirement
    /// a memory store recorded as private: private to a person, though
    /// not to whom when no record above names the owner. Kept by the
    /// memory they were read from ([`MemoryHome`]): a retirement one mob's
    /// memory recorded names no member of a mob with a memory of its own.
    retired: RwLock<BTreeMap<MemoryHome, BTreeSet<String>>>,
    /// How many bindings of a memory store's private retirements this
    /// record was given ([`PrivateOwners::bind_private_retirements_of`]):
    /// each binding of a mob's own memory takes the next number.
    retirement_bindings: std::sync::atomic::AtomicU64,
    /// What is known of who each mob a member created is private to
    /// ([`MobOwner`]).
    mob_owners: RwLock<BTreeMap<String, MobOwner>>,
    /// The mob of the member that created each mob ([`MobCreation`]):
    /// written where `mob_owners` is written, and for the mobs the mob set
    /// keeps and has not loaded, and dropped where `mob_owners` is dropped.
    mob_creations: RwLock<BTreeMap<String, MobCreation>>,
    /// The launch's own mob, by id, once the launch said it
    /// ([`PrivateOwners::bind_launch_mob`]).
    launch_mob: std::sync::OnceLock<String>,
    /// The time of the first event of each mob's log, and the creates this
    /// record admitted under each id ([`MobDates`]).
    mob_dates: RwLock<MobDates>,
    /// Who held each member name over time ([`NameHolders`]), keyed by the
    /// mob and the identity the member's memory scope names it by.
    holders: RwLock<BTreeMap<MemberKey, NameHolders>>,
    /// The times of each mob's journal rows `holders` were noted from
    /// ([`JournalTimes`]), by mob. Locked before `holders`.
    journal_times: RwLock<BTreeMap<String, JournalTimes>>,
    /// How many journal rows `holders` have noted: it changes with them.
    holders_noted: std::sync::atomic::AtomicU64,
    /// Set once the launch said that it composes the mobs its host
    /// creates ([`PrivateOwners::the_launch_composes_its_hosts_mobs`]).
    composes_hosts_mobs: std::sync::OnceLock<()>,
    /// Set once the seed landed: every event log and creation record read.
    seeded: tokio::sync::OnceCell<()>,
    /// The seed's stage among the first reads this record owes.
    seed_stage: std::sync::Mutex<SeedStage>,
    /// How far the first reads this record owes have come.
    first_reads: FirstReadsSignal,
    /// The agent mob tool state of the runtime, whose mobs the seed reads.
    child_mobs: RwLock<Option<Weak<meerkat_mob_mcp::MobMcpState>>>,
    /// The mobs whose durable records this record read, each with the
    /// incarnation it was read at: a mob destroyed and created again under
    /// one id is read again. It keeps the mobs the set manages at the
    /// newest reading and no others, so a mob the set loads again is read
    /// again too.
    mobs_read: RwLock<BTreeSet<(String, meerkat_core::service::MobIncarnationId)>>,
    /// The incarnations of mobs this record met, and the ones among them
    /// it knows ended ([`MobIncarnations`]). Locked after the dates lock
    /// and before `built` and `logged`.
    incarnations: RwLock<MobIncarnations>,
    /// How far this record has read each mob's durable records
    /// ([`MobReads`]): what tells a member the record does not hold because
    /// nothing names a person for it from a member of a mob it has not read.
    mob_reads: MobReadsSignal,
    /// The mobs the launch composes for its host beside its launch mob
    /// ([`HostedMobs`]).
    hosted: HostedMobs,
    /// Who follows the end of a member private to a person, and the ends
    /// held for it.
    ends: std::sync::Mutex<MemberEnds>,
}

/// The mobs a launch composes for its host beside its launch mob, by id.
/// The launch's composition names each one before the mob is built, at its
/// create and at every load.
///
/// The members of such a mob are no members of the launch's memory: the
/// record's views for the launch's memory
/// ([`PrivateOwners::memory_members`]) leave them out, each such mob has
/// views of its own ([`PrivateOwners::memory_members_of`]), and a member
/// build of such a mob takes no part in what the launch's memory observes
/// of a member's model calls.
#[derive(Clone, Default)]
pub(crate) struct HostedMobs(Arc<RwLock<BTreeSet<String>>>);

/// The memory the members of a mob are members of, as far as it is
/// decided ([`PrivateOwners::memory_home_of`]): what each view of this
/// record for a memory, the retirements a memory store recorded and the
/// answer for a member the record does not hold are kept by.
///
/// Each reader takes its own closed side of what is not decided
/// ([`Self::NotKnown`]). What gives a member its memory gives none. This
/// record's views, which say who is private to a person, keep such a
/// mob's members where they always were, in the launch's view
/// ([`MemoryHome::view`]): a member is never dropped from the views because its
/// mob is not decided yet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MemoryHome {
    /// The launch's memory: the launch mob's members, and the members of
    /// every mob whose recorded creating members lead to the launch mob.
    Launch,
    /// The memory of a mob the launch composes beside its launch mob, by
    /// that mob's id: that mob's members.
    Hosted(String),
    /// No memory: the members of the mob are members of none. They are in
    /// no view, no memory's retirements name them, and a binding of a
    /// store for the mob reads nothing. A mob that a member of a mob the
    /// launch composes beside its launch mob created, at any depth.
    Nowhere,
    /// Not decided: what would say whose memory the mob's members are
    /// members of was not read, or does not say.
    NotKnown,
}

impl MemoryHome {
    /// The memory whose view of this record holds the members of a mob
    /// with this home: its own for a decided home, the launch's for one
    /// that is not decided, and none for a mob whose members are members
    /// of no memory.
    fn view(self) -> Option<Self> {
        match self {
            Self::Launch | Self::NotKnown => Some(Self::Launch),
            hosted @ Self::Hosted(_) => Some(hosted),
            Self::Nowhere => None,
        }
    }
}

impl HostedMobs {
    /// The launch composes the mob `mob_id` beside its launch mob.
    pub(crate) fn name(&self, mob_id: &str) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(mob_id.to_string());
    }

    /// Whether the launch composed a mob `mob_id` beside its launch mob.
    pub(crate) fn holds(&self, mob_id: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(mob_id)
    }

    /// The id `mob_id` is no longer a mob's the launch composes: a mob
    /// that the launch does not compose is created under it.
    fn withdraw(&self, mob_id: &str) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(mob_id);
    }

    /// Whether the launch names no mob beside its launch mob now: it built
    /// none since this process started, at a create or at a load, or every
    /// id it named was withdrawn since.
    pub(crate) fn names_none(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// Test builds only: no mob is named any more, as in a process that
    /// started over the same stores and has built no mob beside its launch
    /// mob yet.
    #[cfg(test)]
    pub(crate) fn name_none_in_tests(&self) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl std::fmt::Debug for PrivateOwners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateOwners")
            .field("seeded", &self.inner.seeded.initialized())
            .finish_non_exhaustive()
    }
}

/// The refusal of a spawn into a mob whose owner this record does not know
/// yet: retryable, since the seed lands by itself.
fn unknown_mob_owner(mob_id: &str) -> MobError {
    MobError::WiringError(format!(
        "who mob '{mob_id}' is private to is not known yet (the private-owner record is still \
         being read); retry the spawn"
    ))
}

/// The refusal of a spawn into a mob whose creating member's owner nothing
/// says for certain ([`MobOwner::CreatorUnknown`]). It can be made again:
/// the mob is read again at every reading of the mob set, and the refusal
/// stands until that member's mob can be read and its records name the
/// member. Where they no longer do (a chain of the member's seatings that a
/// mob reset broke), it stands for as long as the mob does.
fn unknown_creator_owner(mob_id: &str) -> MobError {
    MobError::WiringError(format!(
        "who mob '{mob_id}' is private to is not known: nothing says for certain who the member \
         that created it is private to (its mob cannot be read here, or that mob's records no \
         longer name the member); the spawn can be made again once they do"
    ))
}

/// The refusal of a spawn into a mob whose records do not say which member
/// created it: its creation record names a session several rosters hold or
/// is of a kind this release does not know, or the mob is a temporary
/// council's and no roster, or more than one, holds the session that
/// convened the council. What was read is kept, so the refusal stands for
/// as long as the mob does.
fn undecidable_mob_owner(mob_id: &str) -> MobError {
    MobError::WiringError(format!(
        "who mob '{mob_id}' is private to cannot be read: its records do not say which member \
         created it (several rosters named the session that created it, the member that \
         convened the council it belongs to is on no roster under the session it convened it \
         on, or the record is of a kind this release does not know), so no member can be \
         spawned into it"
    ))
}

/// The refusal of a spawn that would make a member private to one person
/// private to another.
fn crossed_owners(source: &str) -> MobError {
    MobError::WiringError(format!(
        "'{source}' is private to a person; a member private to another person may not take it \
         over"
    ))
}

/// What a mob's creation record says about the member that created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatingMember<'a> {
    /// The record names the member, with the mob that member is seated in:
    /// the mob is private to whoever that member is private to.
    Named(&'a meerkat_mob::MobId, &'a meerkat_mob::AgentIdentity),
    /// The record names no member: the mob is private to no one.
    NoMember,
    /// The record does not say which member created the mob: several rosters
    /// named the session that created it and the record picks none, or the
    /// record is of a kind this release does not know. Who the mob is
    /// private to cannot be read from it.
    Undecidable,
}

/// What a mob's creation record says about its creating member: a create a
/// host made for a member's session (`Member`) and a member's own
/// `mob_create` (`OperatorMember`) both name one. The host, a session no
/// roster named and a store from before creations were recorded name none.
/// A session several rosters named (`Ambiguous`) is not decidable: one of
/// those members created the mob, and the record does not say which. A
/// record of a kind this release does not know is not decidable either: it
/// may name a member.
fn creating_member(provenance: &meerkat_mob::MobProvenance) -> CreatingMember<'_> {
    use meerkat_mob::MobProvenance;
    match provenance {
        MobProvenance::Member {
            parent_mob_id,
            creator_identity,
            ..
        }
        | MobProvenance::OperatorMember {
            parent_mob_id,
            creator_identity,
            ..
        } => CreatingMember::Named(parent_mob_id, creator_identity),
        MobProvenance::Host
        | MobProvenance::Session { .. }
        | MobProvenance::OperatorSession { .. }
        | MobProvenance::Unrecorded => CreatingMember::NoMember,
        MobProvenance::Ambiguous { .. } => CreatingMember::Undecidable,
        // `MobProvenance` is open to new variants: one this release does
        // not know may name a member, so it is not taken to name none.
        _ => CreatingMember::Undecidable,
    }
}

/// The member a mob's creation record names as the mob's creator: the id
/// of the mob that member is seated in, and its roster member id. `None`
/// for a record that names no member, and for one that does not say which
/// member created the mob.
pub(crate) fn named_creator(provenance: &meerkat_mob::MobProvenance) -> Option<(String, String)> {
    match creating_member(provenance) {
        CreatingMember::Named(mob, member) => Some((mob.to_string(), member.to_string())),
        CreatingMember::NoMember | CreatingMember::Undecidable => None,
    }
}

/// How the member a creation record names was seated when it created the
/// mob: the incarnation its mob ran at, the generation it was seated at and
/// its session. A mob hosted under its mob's id at another incarnation is
/// another mob, not the creating member's. A member seated under its name
/// at another generation is another member, unless the journal seated it
/// again on that session in an unbroken chain from the creating member
/// ([`creators_binding`]).
#[derive(Debug, Clone, Copy)]
struct CreatorsSeat<'a> {
    ran_at: meerkat_core::service::MobIncarnationId,
    generation: meerkat_mob::Generation,
    session: &'a meerkat_core::types::SessionId,
}

/// The seat of a creation record's creating member ([`CreatorsSeat`]).
/// `None` for a record that names no member.
fn creating_members_seat(provenance: &meerkat_mob::MobProvenance) -> Option<CreatorsSeat<'_>> {
    use meerkat_mob::MobProvenance;
    match provenance {
        MobProvenance::Member {
            parent_incarnation,
            creator_generation,
            owner_session,
            ..
        }
        | MobProvenance::OperatorMember {
            parent_incarnation,
            creator_generation,
            owner_session,
            ..
        } => Some(CreatorsSeat {
            ran_at: *parent_incarnation,
            generation: *creator_generation,
            session: owner_session,
        }),
        _ => None,
    }
}

/// A member the lookup found a council's convening session to be
/// ([`PrivateOwners::council_mob_owner`]): its mob, the incarnation that
/// mob runs at, the member, and the generation it is seated at.
pub(crate) type Convener = (
    meerkat_mob::MobId,
    meerkat_core::service::MobIncarnationId,
    meerkat_mob::AgentIdentity,
    meerkat_mob::Generation,
);

/// One roster row of a mob, as far as the judgement of who a creating
/// member is reads it ([`PrivateOwners::owner_its_mobs_records_say`]): the
/// generation its member is seated at, the session the row is bound to and
/// its labels.
#[derive(Debug, Clone, Copy)]
struct SeatedRow<'a> {
    generation: meerkat_mob::Generation,
    session: Option<&'a meerkat_core::types::SessionId>,
    labels: &'a BTreeMap<String, String>,
}

impl SeatedRow<'_> {
    /// Whether the row is of the member seated as `seat` says: at that
    /// generation and on that session. A generation number alone does not
    /// name a member: a journal whose generation numbers began again can
    /// seat another member under the name at the same number, and that
    /// member sits on another session.
    fn is_seated_as(&self, seat: CreatorsSeat<'_>) -> bool {
        self.generation == seat.generation && self.session == Some(seat.session)
    }
}

/// What a mob's member history holds of a creating member
/// ([`creators_binding`]).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CreatorsBinding {
    /// The person the labels the member was seated with name, if any.
    owner: Option<String>,
    /// Whether the member is the latest one seated under its name: the
    /// name's latest binding is of the same generation on the same session
    /// (a generation number alone does not name a member). What this
    /// record holds under the name is then the member's own, and not a
    /// later member's.
    holds_its_name: bool,
}

/// The steps a creating member's seat is followed through: the same-session
/// successor steps, a declared role migration among them, as the mob set
/// follows a creator's grant over the mobs its session created.
fn creators_lineage_steps() -> meerkat_mob::LineageSteps {
    meerkat_mob::LineageSteps::same_session_successors().with_role_migrations(true)
}

/// The binding of the member `member`, seated as `seat` says, in the member
/// history `history` of its mob: the binding that named the seat's session,
/// when it is that member's at that generation, or at a later generation
/// that the unbroken chain of same-session successor steps reaches from it
/// (the member was seated again on its session: a respawn, a rebuild that
/// keeps the agent). Its labels are those of the session's latest
/// incarnation: whoever the session's member is private to. `None` when
/// the history holds no such binding (another member's, an earlier
/// generation, a chain that begins after the seat's generation, or a later
/// generation of a journal that holds a mob reset and so answers no
/// chain): it says nothing of the member. This is the journal's recorded
/// lineage: whoever holds the member's name on another session is not
/// asked who the member was private to.
fn creators_binding(
    history: &meerkat_mob::MemberBindingHistory,
    member: &meerkat_mob::AgentIdentity,
    seat: CreatorsSeat<'_>,
) -> Option<CreatorsBinding> {
    let binding = history
        .binding_for_session(seat.session)
        .filter(|binding| binding.identity() == member)?;
    let seated_then = binding.generation() == seat.generation
        || (seat.generation < binding.generation()
            && binding
                .same_session_lineage_start(creators_lineage_steps())
                .is_some_and(|start| start <= seat.generation));
    if !seated_then {
        return None;
    }
    Some(CreatorsBinding {
        owner: crate::access::private_owner(binding.labels()).map(str::to_string),
        holds_its_name: history.last_binding(member).is_some_and(|last| {
            last.generation() == binding.generation() && last.session_id() == binding.session_id()
        }),
    })
}

/// Whether a mob's creation record leaves undecided who the mob is private
/// to ([`CreatingMember::Undecidable`]). The console lists such a mob, and
/// its members, to access admins alone.
pub(crate) fn creation_record_owner_undecidable(provenance: &meerkat_mob::MobProvenance) -> bool {
    creating_member(provenance) == CreatingMember::Undecidable
}

/// Give the console's labels of a member of a mob whose owner is undecided
/// the owner no caller is ([`crate::access::UNDECIDED_PRIVATE_OWNER`]): the
/// member is an access admin's alone while access control is on, and the
/// anonymous operator's on a console with neither access control nor
/// sign-in (the access view decides which, per caller).
/// A member with an owner of its own (the one its own spawn gave it) keeps
/// that owner.
pub(crate) fn withhold_from_all_but_admins(labels: &mut BTreeMap<String, String>) {
    if crate::access::private_owner(labels).is_none() {
        labels.insert(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            crate::access::UNDECIDED_PRIVATE_OWNER.to_string(),
        );
    }
}

/// Who a member of a nested mob that has left its roster is private to, as
/// far as the record can tell ([`PrivateOwners::ended_nested_member_owner`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndedMemberOwner {
    /// The member's own owner, else the person its mob is private to.
    Person(String),
    /// The member has no owner of its own and its mob is private to no one.
    Shared,
    /// Neither the member's own record nor its mob's says: the record holds
    /// nothing for the mob, or the mob's creation record leaves its owner
    /// undecided. The member may be a person's.
    Unknown,
}

impl EndedMemberOwner {
    /// The owner the console records the ended member with: the person,
    /// none for a shared member, and the owner no caller is
    /// ([`crate::access::UNDECIDED_PRIVATE_OWNER`]) for one whose owner is
    /// not known, as a seated member of a mob of unknown owner is recorded
    /// ([`withhold_from_all_but_admins`]).
    pub(crate) fn recorded_owner(self) -> Option<String> {
        match self {
            Self::Person(owner) => Some(owner),
            Self::Shared => None,
            Self::Unknown => Some(crate::access::UNDECIDED_PRIVATE_OWNER.to_string()),
        }
    }
}

/// The wait before the next try of a durable read that failed, growing to
/// [`SEED_RETRY_MAX`].
fn next_wait(wait: std::time::Duration) -> std::time::Duration {
    (wait * 2).min(SEED_RETRY_MAX)
}

/// The members among `identities`, the identities a memory store marks as
/// private. The store also marks the key of every identity scope it keeps
/// apart from the members of a name
/// (`crate::memory::records::IdentityScopeKey`). Such a key is a scope, not
/// a member's name: only plain names are members to this record.
fn member_names(identities: BTreeSet<String>) -> impl Iterator<Item = String> {
    identities
        .into_iter()
        .filter(|identity| crate::memory::records::IdentityScopeKey::of(identity).is_named())
}

impl PrivateOwners {
    /// Record what `labels`, the labels member `member` of mob `mob_id` is
    /// built with, say about its private owner.
    pub(crate) fn record(
        &self,
        mob_id: &str,
        member: &str,
        labels: Option<&BTreeMap<String, String>>,
    ) {
        let owner = labels
            .and_then(crate::access::private_owner)
            .map(str::to_string);
        self.inner
            .built
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((mob_id.to_string(), member.to_string()), owner);
    }

    /// [`Self::record`] for a member built in the mob `mob_id` at
    /// `incarnation`: `None` for a build that names no incarnation, which
    /// is recorded. A build of an incarnation this record knows ended
    /// records nothing: no such member stands, and what the record holds
    /// under the name is the standing mob's member's, or nothing. Any
    /// other incarnation is met by the build, and the build is recorded.
    ///
    /// The incarnations are held across the write: the admission of a
    /// create marks the id's incarnations ended and forgets the id's
    /// members under the same lock ([`Self::forget_the_members_of`]), so a
    /// build is either recorded before that and forgotten there, or finds
    /// its incarnation ended.
    fn record_built(
        &self,
        mob_id: &str,
        member: &str,
        labels: Option<&BTreeMap<String, String>>,
        incarnation: Option<meerkat_core::service::MobIncarnationId>,
    ) {
        let Some(incarnation) = incarnation else {
            self.record(mob_id, member, labels);
            return;
        };
        let key = (mob_id.to_string(), incarnation);
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if incarnations.ended.contains(&key) {
            return;
        }
        incarnations.met.insert(key);
        self.record(mob_id, member, labels);
    }

    /// This record met the mob `mob_id` at `incarnation`, unless it knows
    /// that incarnation ended.
    fn meet(&self, mob_id: &str, incarnation: meerkat_core::service::MobIncarnationId) {
        let key = (mob_id.to_string(), incarnation);
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if !incarnations.ended.contains(&key) {
            incarnations.met.insert(key);
        }
    }

    /// The host destroyed the mob `mob_id` that ran at `incarnation`: the
    /// incarnation has ended, and a member build of it that reaches this
    /// record from here on records nothing ([`Self::record_built`]).
    pub(crate) fn mob_ended(
        &self,
        mob_id: &str,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) {
        let key = (mob_id.to_string(), incarnation);
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if incarnations.destroy_asked.get(mob_id) == Some(&incarnation) {
            incarnations.destroy_asked.remove(mob_id);
        }
        incarnations.met.remove(&key);
        incarnations.ended.insert(key);
    }

    /// The host asks the mob set to destroy the mob `mob_id` that runs at
    /// `incarnation`: the record meets the incarnation and keeps it as the
    /// one a destroy was asked for under the id. A destroy that is done at
    /// once is told with its incarnation ([`Self::mob_ended`]); one that
    /// its repeat continues is told by the id alone
    /// ([`Self::asked_destroy_continued`]).
    pub(crate) fn destroy_asked(
        &self,
        mob_id: &str,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) {
        let key = (mob_id.to_string(), incarnation);
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if !incarnations.ended.contains(&key) {
            incarnations.met.insert(key);
            incarnations
                .destroy_asked
                .insert(mob_id.to_string(), incarnation);
        }
    }

    /// The mob set continued, under `mob_id`, the storage cleanup of a mob
    /// the host's caller destroyed: that destroy ended its mob, and no mob
    /// is created under the id while such a cleanup is owed. The
    /// incarnation the host asked a destroy for under the id has ended, as
    /// [`Self::mob_ended`] marks it.
    pub(crate) fn asked_destroy_continued(&self, mob_id: &str) {
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(incarnation) = incarnations.destroy_asked.remove(mob_id) {
            let key = (mob_id.to_string(), incarnation);
            incarnations.met.remove(&key);
            incarnations.ended.insert(key);
        }
    }

    /// The mobs the launch composes for its host beside its launch mob, as
    /// this record keeps them ([`HostedMobs`]): the launch's composition
    /// names each one, and whatever feeds the launch's memory reads it.
    pub(crate) fn hosted_mobs(&self) -> HostedMobs {
        self.inner.hosted.clone()
    }

    /// A mob is created under the id `mob_id`: forget that this record read
    /// the durable records of a mob of that id, and the private retirements
    /// a memory of that mob's own recorded. What it read was an earlier
    /// mob's, and the new mob is not read until its own records are. A
    /// create asks it once its releases passed, a host's create as a
    /// member's ([`Self::before_create_hook`]). The members the record
    /// holds under the id are forgotten where the create is counted
    /// ([`Self::forget_the_members_of`]): each member of the new mob is
    /// recorded at its own build. A read that is owed for the id
    /// ([`Self::owe_the_read_of`]) stays owed. A binding of the earlier
    /// mob's memory that is still reading
    /// ([`Self::bind_private_retirements_of`]) is no longer the mob's: what
    /// it reads is dropped.
    fn forget_the_read_of(&self, mob_id: &str) {
        // The binding first, then what it read: a binding that lands in
        // between finds itself no longer the mob's, or what it put in is
        // taken out here.
        self.inner
            .mob_reads
            .0
            .send_if_modified(|reads| reads.retirements_owed.remove(mob_id).is_some());
        self.inner
            .retired
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&MemoryHome::Hosted(mob_id.to_string()));
        self.inner
            .mobs_read
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(mob, _)| mob != mob_id);
        self.inner
            .mob_reads
            .0
            .send_if_modified(|reads| reads.read.remove(mob_id));
    }

    /// Test builds only: forget what this record keeps under the mob id
    /// `mob_id`: its members' owners as their builds and the event log
    /// named them, who the mob is private to, the mob of its creating
    /// member, the time of its first event, and that its durable records
    /// were read, with the private retirements a memory of the mob's own
    /// recorded ([`Self::forget_the_read_of`]). The record then stands as
    /// one of a process that built none of the mob's members. A read that
    /// is owed for the id stays owed.
    #[cfg(test)]
    pub(crate) fn forget_mob_in_tests(&self, mob_id: &str) {
        for members in [&self.inner.built, &self.inner.logged] {
            members
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(mob, _), _| mob != mob_id);
        }
        self.inner
            .mob_owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(mob_id);
        self.inner
            .mob_creations
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(mob_id);
        self.inner
            .mob_dates
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .first_events
            .remove(mob_id);
        self.forget_the_read_of(mob_id);
    }

    /// The host is about to build the mob `mob_id`: it creates the mob, or
    /// loads it from its store. From this moment the record owes the read
    /// of that mob's durable records, and what it does not hold of that
    /// mob's members is not known ([`Self::person_for_a_reader`]) until it
    /// has read them. A host calls it before the mob is built, so that no
    /// request for the mob comes before the mark.
    ///
    /// The mark is the mob's own. Nothing is owed for any other mob, and
    /// the mark does not touch the record's first reads
    /// ([`Self::first_read_pending`]): a mark alone does not keep a
    /// console's callers, or the members of another mob, apart. The
    /// reading of the mob that follows its build does, for as long as it
    /// lasts, as every reading of a mob the set came to manage does (the
    /// follower's `read_loaded_mobs_until_read`). A mark whose mob was never
    /// built (its load failed) stays until that mob is read: it keeps only
    /// that mob's members on the closed side.
    ///
    /// The mark is by the mob's id, and any reading of that mob clears it:
    /// also a reading that was under way when the mark was set. The mob's
    /// durable log is the same one then (a load), or every member of the
    /// new mob is built here (a create), so what the record holds of the
    /// mob still holds. A mob the set manages whose read is owed is read
    /// whatever was read of it before, so the mark of a mob the set
    /// manages does not outlast the next reading of the set that succeeds.
    /// A reading that fails clears no mark and is tried again; a mob the
    /// set does not manage is not read by a reading of the set.
    pub(crate) fn owe_the_read_of(&self, mob_id: &str) {
        self.inner
            .mob_reads
            .0
            .send_if_modified(|reads| reads.owed.insert(mob_id.to_string()));
    }

    /// Wait, at most `bound` in all, for the reads a request that is for
    /// the mob `mob_id` must not come before: while the read of that mob is
    /// owed ([`Self::owe_the_read_of`]: the request can be the one that
    /// made the host load the mob), and then while a first read of the
    /// record is still on its first attempt.
    ///
    /// It marks nothing when the bound is reached: the reads stay as they
    /// are for everyone else. For a caller whose bound is shorter than a
    /// console's ([`FIRST_READ_WAIT`]): its wait must not end a console's
    /// own wait early, which [`Self::first_attempts_settled`] would do by
    /// making the read overdue. Each such caller waits its own bound for as
    /// long as the reads last.
    pub(crate) async fn read_of_the_mob_awaited(&self, mob_id: &str, bound: std::time::Duration) {
        let mut mobs = self.inner.mob_reads.0.subscribe();
        let mut firsts = self.inner.first_reads.0.subscribe();
        // The reads landed, or the bound was reached: nothing is marked
        // either way. A wait that ends with an error is the record gone.
        let _ = tokio::time::timeout(bound, async {
            let _ = mobs
                .wait_for(|reads| {
                    !reads.owed.contains(mob_id) && !reads.retirements_owed.contains_key(mob_id)
                })
                .await;
            let _ = firsts.wait_for(|reads| reads.in_flight == 0).await;
        })
        .await;
    }

    /// Whether a member of the mob `mob_id` that this record does not hold
    /// is thereby private to no one: the record has read all that names a
    /// person for a member of that mob.
    ///
    /// The launch's own mob keeps the answer it always had: a member of it
    /// that the record does not hold is private to no one, also while the
    /// seed is still owed. A host of one mob is answered as before.
    ///
    /// For every other mob: not while the seed is owed and has not landed
    /// (it reads every mob the set manages), not while the mob's own read
    /// is owed, not while the private retirements a memory of the mob's
    /// own recorded are being read ([`Self::bind_private_retirements_of`]),
    /// and not for a mob a reading record has never read: a record
    /// whose seed landed and that follows a mob set reads every mob the set
    /// comes to manage, so a mob it has not read is not loaded, or was
    /// loaded a moment ago. A record nothing reads (a runtime taken over a
    /// bare mob handle), and one that follows no mob set, owes no read and
    /// holds what its own builds recorded: for it a member it does not hold
    /// is private to no one, as before.
    pub(crate) fn has_read_the_mob(&self, mob_id: &str) -> bool {
        {
            let reads = self.inner.mob_reads.0.borrow();
            if reads.seed_mob.as_deref() == Some(mob_id) {
                return true;
            }
            if reads.owed.contains(mob_id) {
                return false;
            }
            // The private retirements a memory of the mob's own recorded
            // name a person for a member too.
            if reads.retirements_owed.contains_key(mob_id) {
                return false;
            }
        }
        let seed = *self
            .inner
            .seed_stage
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if matches!(seed, SeedStage::InFlight | SeedStage::Overdue) {
            return false;
        }
        if self.inner.mob_reads.0.borrow().read.contains(mob_id) {
            return true;
        }
        let follows_a_mob_set = self
            .inner
            .child_mobs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        !(seed == SeedStage::Landed && follows_a_mob_set)
    }

    /// The seed reads the log of the mob `mob_id`: the launch's own mob.
    fn note_the_seeds_mob(&self, mob_id: &str) {
        self.inner
            .mob_reads
            .0
            .send_modify(|reads| reads.seed_mob = Some(mob_id.to_string()));
    }

    /// The person member `member` of mob `mob_id` is private to, as far as
    /// this record knows: what its build recorded, else what the event log
    /// named. `None` for a member private to no one, and for one the record
    /// does not know.
    pub(crate) fn owner_of(&self, mob_id: &str, member: &str) -> Option<String> {
        let key = (mob_id.to_string(), member.to_string());
        if let Some(recorded) = self
            .inner
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return recorded.clone();
        }
        self.inner
            .logged
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .flatten()
    }

    /// What this record holds for member `member` of mob `mob_id`, whether
    /// it still runs or has ended: its person, that it is private to no
    /// one, or that the record does not hold it. What its build recorded
    /// comes first, then what the event log named; a member neither holds
    /// whose retirement the memory of its mob ([`Self::memory_home_of`])
    /// recorded as private is private to a person nobody is
    /// ([`crate::access::UNKNOWN_PRIVATE_OWNER`]). A retirement another
    /// mob's memory recorded names no member of this mob.
    pub(crate) fn person_of(
        &self,
        mob_id: &str,
        member: &str,
    ) -> crate::mob_announcements::MemberPerson {
        use crate::mob_announcements::MemberPerson;
        let key = (mob_id.to_string(), member.to_string());
        let built = self
            .inner
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let held = built.or_else(|| {
            self.inner
                .logged
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
                .cloned()
        });
        match held {
            Some(Some(person)) => MemberPerson::Private(person),
            Some(None) => MemberPerson::Shared,
            None if self.memory_home_of(mob_id).view().is_some_and(|view| {
                self.inner
                    .retired
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&view)
                    .is_some_and(|retired| {
                        retired.contains(&crate::member_comms_id::logical_memory_identity(member))
                    })
            }) =>
            {
                MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string())
            }
            None => MemberPerson::NotKnown,
        }
    }

    /// The memory the members of the mob `mob_id` are members of.
    ///
    /// - A mob the launch composes beside its launch mob has a memory of
    ///   its own ([`MemoryHome::Hosted`]).
    /// - The launch mob, and a mob whose recorded creating members lead to
    ///   the launch mob within [`MEMORY_HOME_STEPS`] mobs, are in the
    ///   launch's ([`MemoryHome::Launch`]).
    /// - A mob whose recorded creating members lead to a mob the launch
    ///   composes beside its launch mob is in none
    ///   ([`MemoryHome::Nowhere`]).
    /// - Everything else is not decided ([`MemoryHome::NotKnown`]): a mob
    ///   whose creation this record does not hold, one no member created
    ///   (a session no roster named, a store from before creations were
    ///   recorded), one whose creation does not say which member created
    ///   it, and a line of creating mobs longer than the bound.
    ///
    /// The one place that decides it, and it decides by what the launch
    /// said (its own mob, [`Self::bind_launch_mob`]; the mobs it composes,
    /// [`HostedMobs`]) and by recorded creations, never by a member's
    /// name, a label or an id a caller supplied. Every reader of the
    /// answer handles all four, and takes its own closed side of
    /// [`MemoryHome::NotKnown`].
    ///
    /// It never answers [`MemoryHome::Launch`] because no mob has been
    /// named beside the launch mob yet: such a mob is named when it is
    /// built in this process, at its create and at each load, so a stored
    /// mob that is not loaded is not named. A record whose launch has not
    /// said which mob is its own answers [`MemoryHome::NotKnown`] for a
    /// mob it does not name, when it follows a mob set; only a record
    /// that follows no mob set, a runtime of one mob that no door of it
    /// can add a mob to, answers [`MemoryHome::Launch`].
    ///
    /// This is the only walk over creations that memory asks. The host's
    /// reach question (`MobEntries::hosting`) answers "none" for the
    /// launch mob, for a mob of a launch mob's member at any depth, for a
    /// mob a session that is no member's created, for one whose creation
    /// was not recorded and for a runtime with no host or mob set: that
    /// "none" is not this function's [`MemoryHome::Launch`].
    pub(crate) fn memory_home_of(&self, mob_id: &str) -> MemoryHome {
        if self.inner.hosted.holds(mob_id) {
            return MemoryHome::Hosted(mob_id.to_string());
        }
        let Some(launch_mob) = self.inner.launch_mob.get() else {
            let follows_a_mob_set = self
                .inner
                .child_mobs
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some();
            return if follows_a_mob_set {
                MemoryHome::NotKnown
            } else {
                MemoryHome::Launch
            };
        };
        if mob_id == launch_mob.as_str() {
            return MemoryHome::Launch;
        }
        for (step, creating_mob) in self.creating_mobs_above(mob_id).iter().enumerate() {
            // The members of a mob that a member of a mob with a memory of
            // its own created are members of no memory.
            if self.inner.hosted.holds(creating_mob) {
                return MemoryHome::Nowhere;
            }
            // The launch mob is looked for within the bound: the mob
            // the last step of the line reaches is not read for it.
            if step < MEMORY_HOME_STEPS && creating_mob == launch_mob {
                return MemoryHome::Launch;
            }
        }
        // The line ended before the launch mob: no member, not decidable,
        // or a creation this record does not hold. Or the bound: a line
        // of creating mobs longer than it, or one that returns to itself.
        MemoryHome::NotKnown
    }

    /// The mobs of the creating members above the mob `mob_id`, nearest
    /// first, as the recorded creations name them: the mob of the member
    /// that created it, then the mob of the member that created that
    /// mob, for one step more than [`MEMORY_HOME_STEPS`] at most. The
    /// line ends where a mob was created by no member, where its record
    /// does not say which member created it, and where this record holds
    /// no creation. It is the one walk over creations: what memory asks
    /// ([`Self::memory_home_of`]) and what a console's access records ask
    /// ([`Self::nested_members`]) both read it. It reads recorded
    /// creations alone, never a member's name, a label or an id a caller
    /// supplied.
    fn creating_mobs_above(&self, mob_id: &str) -> Vec<String> {
        let creations = self
            .inner
            .mob_creations
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let mut line: Vec<String> = Vec::new();
        let mut mob = mob_id;
        while line.len() <= MEMORY_HOME_STEPS {
            let Some(MobCreation::ByAMemberOf(creating_mob)) = creations.get(mob) else {
                break;
            };
            line.push(creating_mob.clone());
            mob = creating_mob.as_str();
        }
        line
    }

    /// Whether the mob `mob_id` is a mob made under the mob `primary`: a
    /// member of `primary` created it, or a member of a mob made under
    /// `primary` did ([`Self::creating_mobs_above`]). A mob made under a
    /// mob of the host of its own ([`Self::is_a_mob_of_the_host`]: one
    /// the launch composes beside its launch mob, loaded or stored and
    /// not loaded) is made under that mob and under no other. A mob whose line of
    /// creating members ends before it reaches `primary` (created by no
    /// member, a record that does not say, a creation this record does
    /// not hold) is attributed to no mob the launch composes beside its
    /// launch mob; for any other `primary`, the launch's own mob, it
    /// stays where its members always were, as memory keeps a mob that is
    /// not decided in the launch's view ([`MemoryHome::view`]).
    fn is_made_under(&self, mob_id: &str, primary: &str) -> bool {
        for creating_mob in self.creating_mobs_above(mob_id) {
            if creating_mob == primary {
                return true;
            }
            if self.is_a_mob_of_the_host(&creating_mob) {
                return false;
            }
        }
        !self.inner.hosted.holds(primary)
    }

    /// Whose member `member` of mob `mob_id` is, for a reader that decides
    /// by it and must not take "the record does not hold it" for "private
    /// to no one": what [`Self::person_of`] answers, with a member the
    /// record does not hold told apart by whether the record has read the
    /// mob. Of a mob it has read ([`Self::has_read_the_mob`]) such a member
    /// is private to no one: nothing that names a person names one for it.
    /// Of a mob it has not read (the seed or the mob's own read is owed, the
    /// mob was loaded a moment ago, the mob was never loaded in this
    /// process) it is not known, and the reader takes the closed side.
    ///
    /// The launch's own mob is never "not known": it keeps the answers it
    /// had, so a host of one mob is answered as before.
    ///
    /// The limit: until a mob of the host beside the launch's own is first
    /// loaded in a process, this record does not know whose its members
    /// are, and answers not known for each of them. No mob is loaded to
    /// answer.
    pub(crate) fn person_for_a_reader(
        &self,
        mob_id: &str,
        member: &str,
    ) -> crate::mob_announcements::MemberPerson {
        use crate::mob_announcements::MemberPerson;
        match self.person_of(mob_id, member) {
            MemberPerson::NotKnown if self.has_read_the_mob(mob_id) => MemberPerson::Shared,
            held => held,
        }
    }

    /// Every member of the launch's memory this record knows, keyed by the
    /// identity its memory scope names it by
    /// ([`crate::member_comms_id::logical_memory_identity`]), with its
    /// owner, or `None` for a member private to no one. A member two
    /// records know of is private when either says so; an identity the
    /// launch's memory recorded as private whose owner no record names is
    /// private to [`crate::access::UNKNOWN_PRIVATE_OWNER`]. The members of
    /// the mobs the launch composes beside its launch mob are left out
    /// ([`HostedMobs`]): each of those mobs has a view of its own
    /// ([`Self::memory_members_of`]).
    pub(crate) fn memory_members(&self) -> BTreeMap<String, Option<String>> {
        self.memory_members_in(&MemoryHome::Launch)
    }

    /// The members of the memory `view` this record knows: the members of
    /// the mobs whose members that memory's view holds
    /// ([`MemoryHome::view`] of [`Self::memory_home_of`]), and the
    /// identities that memory recorded as retired in private.
    fn memory_members_in(&self, view: &MemoryHome) -> BTreeMap<String, Option<String>> {
        let mut members: BTreeMap<String, Option<String>> = BTreeMap::new();
        {
            let built = self
                .inner
                .built
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let logged = self
                .inner
                .logged
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            for ((_, member), owner) in logged
                .iter()
                .filter(|(key, _)| !built.contains_key(*key))
                .chain(built.iter())
                // A member is a member of the memory of its own mob, and
                // of no other.
                .filter(|((mob, _), _)| self.memory_home_of(mob).view().as_ref() == Some(view))
            {
                let identity = crate::member_comms_id::logical_memory_identity(member);
                let entry = members.entry(identity).or_default();
                if entry.is_none() {
                    entry.clone_from(owner);
                }
            }
        }
        if let Some(retired) = self
            .inner
            .retired
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(view)
        {
            for identity in retired {
                members
                    .entry(identity.clone())
                    .or_default()
                    .get_or_insert_with(|| crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
            }
        }
        members
    }

    /// The owner of every private member of the launch's memory this
    /// record knows, keyed by the identity its memory scope names it by.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn memory_owners(&self) -> BTreeMap<String, String> {
        self.memory_members()
            .into_iter()
            .filter_map(|(identity, owner)| owner.map(|owner| (identity, owner)))
            .collect()
    }

    /// [`Self::memory_members`] for the memory whose view holds the
    /// members of the mob `mob_id` ([`Self::memory_home_of`]): for a mob the
    /// launch composes beside its launch mob, that mob's members and the
    /// retirements its own memory recorded; for a mob whose members are
    /// members of no memory, nothing; for every other mob, the launch's
    /// view.
    pub(crate) fn memory_members_of(&self, mob_id: &str) -> BTreeMap<String, Option<String>> {
        self.memory_home_of(mob_id)
            .view()
            .map(|view| self.memory_members_in(&view))
            .unwrap_or_default()
    }

    /// [`Self::memory_owners`] for the memory the members of the mob
    /// `mob_id` are members of.
    pub(crate) fn memory_owners_of(&self, mob_id: &str) -> BTreeMap<String, String> {
        self.memory_members_of(mob_id)
            .into_iter()
            .filter_map(|(identity, owner)| owner.map(|owner| (identity, owner)))
            .collect()
    }

    /// The owner of mob `mob_id`, which a member created: `None` for a mob
    /// private to no one. A mob this record holds nothing certain for is
    /// refused, never taken to be private to no one: as retryable when the
    /// record does not know it yet ([`unknown_mob_owner`]), and until its
    /// creating member's mob says who that member is private to when that
    /// is what is missing ([`unknown_creator_owner`]). One whose records do
    /// not say
    /// which member created it is refused for good
    /// ([`undecidable_mob_owner`]).
    fn mob_owner(&self, mob_id: &str) -> Result<Option<String>, MobError> {
        let recorded = self
            .inner
            .mob_owners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mob_id)
            .cloned();
        match recorded {
            Some(MobOwner::Known(owner)) => Ok(owner),
            Some(MobOwner::Undecidable) => Err(undecidable_mob_owner(mob_id)),
            Some(MobOwner::CreatorUnknown) => Err(unknown_creator_owner(mob_id)),
            None => Err(unknown_mob_owner(mob_id)),
        }
    }

    /// What this record holds of who mob `mob_id` is private to; `None` for
    /// a mob it holds nothing for.
    fn held_mob_owner(&self, mob_id: &str) -> Option<MobOwner> {
        self.inner
            .mob_owners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(mob_id)
            .cloned()
    }

    /// Give the console's labels of a member of `mob_id`, a mob a console
    /// lists beside its runtime's own, the owner its mob gives it. A member
    /// with an owner of its own (the one its own spawn gave it) keeps that
    /// owner. Any other member takes its mob's: the person a mob is private
    /// to owns every member of it, whatever the member's own row says. A
    /// mob private to no one leaves its members as they are. A mob whose
    /// owner cannot be told (its creation record leaves it undecided,
    /// `creation_record_undecided`, its creating member's owner is not
    /// known, or this record holds nothing for the mob: it has not read it
    /// yet) may be private to a person, so its members are withheld from
    /// everyone but an admin ([`withhold_from_all_but_admins`]). No read is
    /// started here: the record reads a mob the set comes to manage by
    /// itself ([`Self::read_loaded_mobs`]).
    pub(crate) fn give_mob_owner(
        &self,
        mob_id: &str,
        creation_record_undecided: bool,
        labels: &mut BTreeMap<String, String>,
    ) {
        if crate::access::private_owner(labels).is_some() {
            return;
        }
        let recorded = if creation_record_undecided {
            Some(MobOwner::Undecidable)
        } else {
            self.held_mob_owner(mob_id)
        };
        match recorded {
            Some(MobOwner::Known(Some(owner))) => {
                labels.insert(crate::access::PRIVATE_TO_LABEL.to_string(), owner);
            }
            Some(MobOwner::Known(None)) => {}
            Some(MobOwner::Undecidable | MobOwner::CreatorUnknown) | None => {
                withhold_from_all_but_admins(labels);
            }
        }
    }

    /// Who the member a console names `name` in the nested mob `mob_id`,
    /// which has left its roster, is private to, as far as this record can
    /// tell: the member's own owner (it is recorded under its mob and its
    /// roster id), else what its mob says. The mob's answer is the one the
    /// record holds under the id: it holds it after the mob was destroyed
    /// too, until a mob is created under the id again, whose create writes
    /// its own ([`Self::record_created_mob`]). A mob the record holds
    /// nothing for, whose creation record leaves its owner undecided, or
    /// whose creating member's owner is not known, says nothing: the member
    /// may then be a person's ([`EndedMemberOwner::Unknown`]). No read is
    /// started here.
    pub(crate) fn ended_nested_member_owner(&self, mob_id: &str, name: &str) -> EndedMemberOwner {
        if let Some(owner) = self
            .owner_of(mob_id, &crate::member_comms_id::mob_member_id_str(name))
            .or_else(|| self.owner_of(mob_id, name))
        {
            return EndedMemberOwner::Person(owner);
        }
        match self.held_mob_owner(mob_id) {
            Some(MobOwner::Known(Some(owner))) => EndedMemberOwner::Person(owner),
            Some(MobOwner::Known(None)) => EndedMemberOwner::Shared,
            Some(MobOwner::Undecidable | MobOwner::CreatorUnknown) | None => {
                EndedMemberOwner::Unknown
            }
        }
    }

    /// Every member of a mob made under `primary_mob_id`
    /// ([`Self::is_made_under`]) this record knows from the mobs' durable
    /// records (a member this process built, or one a hosted mob's event
    /// log names), as its mob's id and the name a console gives the
    /// member, with who it is private to
    /// ([`Self::ended_nested_member_owner`]). A member still on its roster
    /// is among them: the caller records a seated member from its row. The
    /// members of a mob the launch composes for its host beside its launch
    /// mob ([`HostedMobs`]) are not among them: such a mob is no mob nested
    /// under `primary_mob_id`, and its members are its own door's. Neither
    /// are the members of a mob made under another mob of the host: the
    /// launch mob's own members and the mobs made under it for a mob
    /// composed beside it, and the mobs made under such a mob for the
    /// launch mob.
    pub(crate) fn nested_members(
        &self,
        primary_mob_id: &str,
    ) -> Vec<(String, String, EndedMemberOwner)> {
        let keys: BTreeSet<MemberKey> = {
            let built = self
                .inner
                .built
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let logged = self
                .inner
                .logged
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            built
                .keys()
                .chain(logged.keys())
                .filter(|(mob_id, _)| {
                    mob_id != primary_mob_id
                        && !self.inner.hosted.holds(mob_id)
                        && self.is_made_under(mob_id, primary_mob_id)
                })
                .cloned()
                .collect()
        };
        keys.into_iter()
            .map(|(mob_id, member)| {
                let name = crate::member_comms_id::runtime_alias_str(&member).into_owned();
                let owner = self.ended_nested_member_owner(&mob_id, &name);
                (mob_id, name, owner)
            })
            .collect()
    }

    /// Record who mob `mob_id` is private to, as a durable record read at
    /// the seed says it. The first record of a mob stands: its creator's
    /// owner does not change. A record that its creating member's owner was
    /// not known ([`MobOwner::CreatorUnknown`]) is no answer yet: what a
    /// later read says takes its place.
    fn record_mob(&self, mob_id: &str, owner: MobOwner) {
        let mut owners = self
            .inner
            .mob_owners
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if matches!(owners.get(mob_id), None | Some(MobOwner::CreatorUnknown)) {
            owners.insert(mob_id.to_string(), owner);
        }
    }

    /// Record who the mob the create admission admits under `mob_id` is
    /// private to, and the mob of the member that creates it. Both take the
    /// place of a record that stands under the id: the admission is asked
    /// only for an id no mob holds, so a record there is not this mob's.
    /// The members the record holds under the id are forgotten with it
    /// ([`Self::forget_the_members_of`]): the mob has none yet, and each
    /// member of it is recorded by its own build.
    ///
    /// First, before the new creation stands, the time an earlier mob of
    /// the id left is dropped and the create is counted, under one lock: a
    /// reader never finds the new mob's creation beside the earlier mob's
    /// time, and a read of the earlier mob that is still under way writes
    /// no time for the id ([`Self::date_the_mob`]), no member its log names
    /// ([`Self::read_log`]) and no mark that the id was read
    /// ([`Self::note_the_mob_read`]).
    ///
    /// Then, after the count, what this record read under the id is
    /// forgotten ([`Self::forget_the_read_of`]): what a reading pass wrote
    /// before the count goes here, and after the count it writes nothing.
    fn record_created_mob(&self, mob_id: &str, owner: MobOwner, creation: MobCreation) {
        {
            // The mob admitted here is not dated yet: its log has no event.
            let mut dates = self
                .inner
                .mob_dates
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            dates.first_events.remove(mob_id);
            *dates.admitted.entry(mob_id.to_string()).or_default() += 1;
            // After the count, under the same lock: the journal rows noted
            // under the id were an earlier mob's, and a row of that mob's
            // journal that is shown from here on is not noted
            // ([`Self::note_a_journal_step_of`]).
            self.forget_the_journal_of(mob_id);
            // So were the members held under the id, and a read of that
            // mob's log that ends from here on keeps none of them.
            self.forget_the_members_of(mob_id);
        }
        // The mob is created anew: what this record read under its id was
        // an earlier mob's.
        self.forget_the_read_of(mob_id);
        self.inner
            .mob_owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(mob_id.to_string(), owner);
        debug_assert!(
            self.first_event_of(mob_id).is_none(),
            "the creation of a mob admitted under an id is written after the time an earlier \
             mob of the id left is dropped"
        );
        self.inner
            .mob_creations
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(mob_id.to_string(), creation);
    }

    /// [`Self::record_created_mob`] for a mob created for a session, which
    /// the launch does not compose: the name an earlier mob of the id left
    /// with the launch's composition ([`HostedMobs`]) does not stand for
    /// it. A mob the launch composes is named by its composition at its
    /// create and at each load.
    fn record_a_mob_created_for_a_session(
        &self,
        mob_id: &str,
        owner: MobOwner,
        creation: MobCreation,
    ) {
        self.inner.hosted.withdraw(mob_id);
        self.record_created_mob(mob_id, owner, creation);
    }

    /// The creates this record's admission admitted so far, by mob id. A
    /// reading pass notes them before it lists the mobs it reads and hands
    /// them to [`Self::date_the_mob`].
    pub(crate) fn creates_admitted(&self) -> CreatesAdmitted {
        self.inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .admitted
            .clone()
    }

    /// Keep the time of the first event of the log `log`, the log of the
    /// mob that holds the id `mob_id` now. It takes the place of a time
    /// kept under the id: the mob read now is the one that holds it.
    ///
    /// `admitted_before` is what [`Self::creates_admitted`] answered
    /// before the pass listed the mob `log` is of. When a create was
    /// admitted under the id since, nothing is written and the answer is
    /// `false`: the mob read may be an earlier mob of the id, and when it
    /// is the mob created since, the pass cannot tell. The pass leaves the
    /// mob unread ([`Self::read_mobs`]), and the mob that holds the id is
    /// dated by the pass that lists it after the create. `true` otherwise,
    /// whether or not the log held an event.
    async fn date_the_mob(
        &self,
        mob_id: &str,
        log: &dyn MemberSpawnLog,
        admitted_before: &CreatesAdmitted,
    ) -> Result<bool, String> {
        let first_event = log.first_event_time().await?;
        let mut dates = self
            .inner
            .mob_dates
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if dates.admitted.get(mob_id) != admitted_before.get(mob_id) {
            return Ok(false);
        }
        if let Some(first_event) = first_event {
            dates.first_events.insert(mob_id.to_string(), first_event);
        }
        Ok(true)
    }

    /// The time of the first event of the log of the mob that holds the id
    /// `mob_id` now, as this record read it: that mob's creation. `None`
    /// for a mob this record has not dated: one it has not read, one
    /// admitted under the id since its last read, and one stored and never
    /// loaded in this process.
    ///
    /// Nothing decides by this time: which session is a mob's own is what
    /// the mob's journal bound (`crate::memory_wiring`). The admission of a
    /// create asserts that it dropped the time, and rows read it.
    pub(crate) fn first_event_of(&self, mob_id: &str) -> Option<std::time::SystemTime> {
        self.inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .first_events
            .get(mob_id)
            .copied()
    }

    /// Record the mob of the member that created the mob `mob_id`, as a
    /// durable creation record says it. The first record of a mob stands,
    /// as its owner's does ([`Self::record_mob`]).
    fn record_creation(&self, mob_id: &str, creation: MobCreation) {
        self.inner
            .mob_creations
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(mob_id.to_string())
            .or_insert(creation);
    }

    /// Test builds only: this record holds the creation of the mob
    /// `mob_id`, as a read of the mob's creation record would say it: by a
    /// member of the mob `creating_mob`, or by no member.
    #[cfg(test)]
    pub(crate) fn hold_the_creation_in_tests(&self, mob_id: &str, creating_mob: Option<&str>) {
        self.record_creation(
            mob_id,
            match creating_mob {
                Some(creating_mob) => MobCreation::ByAMemberOf(creating_mob.to_string()),
                None => MobCreation::ByNoMember,
            },
        );
    }

    /// Test builds only: this record's create admission admitted a mob
    /// under `mob_id`, created by a member of the mob `parent`
    /// ([`Self::record_created_mob`]).
    #[cfg(test)]
    pub(crate) fn admit_a_create_in_tests(&self, mob_id: &str, parent: &str) {
        self.record_created_mob(
            mob_id,
            MobOwner::Known(None),
            MobCreation::ByAMemberOf(parent.to_string()),
        );
    }

    /// Test builds only: [`Self::read_loaded_mobs`] as a pass that noted
    /// the creates before `meanwhile` ran and lists the mobs after it.
    #[cfg(test)]
    pub(crate) async fn read_loaded_mobs_noted_before(
        &self,
        meanwhile: impl std::future::Future<Output = ()>,
    ) -> Result<(), String> {
        let admitted_before = self.creates_admitted();
        meanwhile.await;
        let unread = self.unread_loaded_mobs().await?;
        self.read_mobs(unread, &admitted_before).await
    }

    /// Test builds only: this record follows a mob set, one that has gone
    /// since: what a launch's record is once the launch wired its mob set.
    #[cfg(test)]
    pub(crate) fn follow_a_mob_set_that_has_gone_in_tests(&self) {
        *self
            .inner
            .child_mobs
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Weak::new());
    }

    /// The launch's own mob is `mob_id`. Said by the launch where it
    /// declares its mob to its mob set, before the set builds any mob, and
    /// where it starts this record's seed over its mob's handle. The first
    /// word stands.
    pub(crate) fn bind_launch_mob(&self, mob_id: &str) {
        let _ = self.inner.launch_mob.set(mob_id.to_string());
    }

    /// The launch composes the mobs its host creates: its composition was
    /// given to its mob set, which asks it for every build of such a
    /// mob, its create and each load of its store, and the composition
    /// names the mob then ([`HostedMobs`]). Said by the launch at the
    /// one place where it gives its composition to the set. A launch
    /// that never says it composes none: no build names a mob its host
    /// created, and such a mob stays in the launch mob's world.
    pub(crate) fn the_launch_composes_its_hosts_mobs(&self) {
        let _ = self.inner.composes_hosts_mobs.set(());
    }

    /// Whether the mob `mob_id` is a mob of the host of its own, served
    /// by its own id: the launch's composition named it ([`HostedMobs`]:
    /// it was built in this process), or the record read that the host
    /// created it ([`MobCreation::ByTheHost`]) and the launch composes
    /// the mobs its host creates, so the next load of a mob beside the
    /// launch mob names it. The second holds for a mob that is stored
    /// and not loaded: nothing of it is loaded to answer. The launch mob
    /// reads as created by the host too, and no build names it: a line
    /// that reaches it is the launch mob's, which
    /// [`Self::is_made_under`] answers for the launch mob as its primary
    /// ahead of this question. It reads the launch's two statements and
    /// a recorded creation, never a name.
    fn is_a_mob_of_the_host(&self, mob_id: &str) -> bool {
        if self.inner.hosted.holds(mob_id) {
            return true;
        }
        self.inner.composes_hosts_mobs.get().is_some()
            && matches!(
                self.inner
                    .mob_creations
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(mob_id),
                Some(MobCreation::ByTheHost)
            )
    }

    /// The releases a create asks before its mob exists, in the order they
    /// were bound.
    fn create_releases(&self) -> Vec<Arc<dyn MobCreateRelease>> {
        self.inner
            .create_releases
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What this record holds of who mob `mob_id` is private to, for tests
    /// outside this module: `Some(owner)` for a mob whose owner it knows
    /// (`Some(None)` for a mob private to no one), `None` for a mob it does
    /// not know or cannot decide. The two layers are the fact under test: a
    /// mob recorded as private to no one is not a mob the record does not
    /// know.
    #[cfg(test)]
    #[allow(clippy::option_option)]
    pub(crate) fn recorded_mob_owner(&self, mob_id: &str) -> Option<Option<String>> {
        self.mob_owner(mob_id).ok()
    }

    /// `self`, once the seed has read `log` (the event log of the runtime's
    /// mob), then the event log and the creation record of every mob the
    /// runtime's agent mob tools host: the first call that reads them all
    /// lands the seed, and a read that fails is tried again by the next
    /// call ([`Self::landed`] tells which).
    pub(crate) async fn seeded(&self, log: &dyn MemberSpawnLog) -> &Self {
        self.owe_seed();
        let read = self
            .inner
            .seeded
            .get_or_try_init(|| async {
                // The launch's own mob: no mob is created under its id.
                self.read_log(log, &self.creates_admitted()).await?;
                if let Some(state) = self.child_mob_state() {
                    self.read_child_mobs(&state).await?;
                }
                Ok::<(), String>(())
            })
            .await;
        self.seed_attempt_ended(read.is_ok());
        if let Err(error) = read {
            tracing::warn!(
                %error,
                "who the members this process did not build are private to could not be read \
                 from their mobs' durable records; the next read tries again"
            );
        }
        self
    }

    /// Whether the seed has landed: every durable record it reads was read.
    pub(crate) fn landed(&self) -> bool {
        self.inner.seeded.initialized()
    }

    /// Which record this is: what a console counts this record's owed
    /// first read under, the same through every clone of its runtime.
    pub(crate) fn record_id(&self) -> PrivateOwnerRecordId {
        self.inner.id
    }

    /// Whether a first read this record owes has not succeeded yet: the
    /// seed, once anything started it, or a read of the private retirements
    /// a memory store recorded. Until every such read has succeeded this
    /// record cannot tell an ended private member from any other name, so
    /// a console keeps every caller who is not an admin apart meanwhile. A
    /// record nothing ever read (a runtime taken over a bare mob handle)
    /// owes no read and is never pending.
    pub(crate) fn first_read_pending(&self) -> bool {
        let reads = *self.inner.first_reads.0.borrow();
        reads.in_flight + reads.overdue > 0
    }

    /// Test builds only: this record owes one more first read, on its first
    /// attempt, until the value handed back is dropped: that read has then
    /// succeeded.
    #[cfg(test)]
    pub(crate) fn owe_a_first_read_in_tests(&self) -> OwedFirstReadInTests {
        self.inner
            .first_reads
            .0
            .send_modify(|reads| reads.in_flight += 1);
        OwedFirstReadInTests(self.clone())
    }

    /// Test builds only: the read of the mob `mob_id` is owed, as after the
    /// host said it is about to build the mob, until the value handed back
    /// is dropped: the record has then read the mob.
    #[cfg(test)]
    pub(crate) fn owe_the_read_of_in_tests(&self, mob_id: &str) -> OwedMobReadInTests {
        self.owe_the_read_of(mob_id);
        OwedMobReadInTests(self.clone(), mob_id.to_string())
    }

    /// Test builds only: hold the identity `identity` as one whose
    /// retirement the memory store recorded as private, as the read of
    /// that store does. A row then has a member the record holds only as
    /// retired, without a memory store of its own.
    #[cfg(test)]
    pub(crate) fn hold_as_retired_in_tests(&self, identity: &str) {
        self.inner
            .retired
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(MemoryHome::Launch)
            .or_default()
            .insert(identity.to_string());
    }

    /// Wait while an owed first read is still on its first attempt, at
    /// most `bound`. The waiters watch the reads' progress and start no
    /// read themselves. A read that fails is waited for no longer, and one
    /// that outlasts a waiter's bound is overdue from then on: later
    /// readers do not wait for it, and [`Self::first_read_pending`] holds
    /// until it succeeds.
    pub(crate) async fn first_attempts_settled(&self, bound: std::time::Duration) {
        let mut reads = self.inner.first_reads.0.subscribe();
        if tokio::time::timeout(bound, reads.wait_for(|reads| reads.in_flight == 0))
            .await
            .is_err()
        {
            let mut stage = self
                .inner
                .seed_stage
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if *stage == SeedStage::InFlight {
                *stage = SeedStage::Overdue;
            }
            self.inner.first_reads.0.send_modify(|reads| {
                reads.overdue += reads.in_flight;
                reads.in_flight = 0;
            });
        }
    }

    /// Note that this record owes its seed, if nothing noted it before.
    fn owe_seed(&self) {
        let mut stage = self
            .inner
            .seed_stage
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *stage == SeedStage::NotOwed {
            *stage = SeedStage::InFlight;
            self.inner
                .first_reads
                .0
                .send_modify(|reads| reads.in_flight += 1);
        }
    }

    /// Note how an attempt of the seed ended.
    fn seed_attempt_ended(&self, landed: bool) {
        let mut stage = self
            .inner
            .seed_stage
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match (*stage, landed) {
            (SeedStage::InFlight, true) => {
                *stage = SeedStage::Landed;
                self.inner
                    .first_reads
                    .0
                    .send_modify(|reads| reads.in_flight = reads.in_flight.saturating_sub(1));
            }
            (SeedStage::Overdue, true) => {
                *stage = SeedStage::Landed;
                self.inner
                    .first_reads
                    .0
                    .send_modify(|reads| reads.overdue = reads.overdue.saturating_sub(1));
            }
            (SeedStage::InFlight, false) => {
                *stage = SeedStage::Overdue;
                self.inner.first_reads.0.send_modify(|reads| {
                    reads.in_flight = reads.in_flight.saturating_sub(1);
                    reads.overdue += 1;
                });
            }
            _ => {}
        }
    }

    /// Seed in the background until the seed lands, waiting longer after
    /// each failure, up to [`SEED_RETRY_MAX`], then read each mob the mob
    /// set comes to manage later ([`Self::read_loaded_mobs_until_read`]),
    /// when the set says its mobs changed. The task holds an observer of the
    /// mob, which keeps no mob open, until the seed landed, and no mob set:
    /// it ends once nothing else holds the set.
    pub(crate) fn seed_in_background(&self, handle: &meerkat_mob::MobHandle) {
        // Owed from this moment, before the task first runs: a reader that
        // comes first finds the read in flight.
        self.owe_seed();
        // The seed reads this mob's log, the launch's own mob: a member of
        // it that the record does not hold is private to no one, as it
        // always was.
        self.note_the_seeds_mob(handle.mob_id().as_str());
        // The launch's own mob, for a launch that declared it to no mob
        // set: one that did has said it before.
        self.bind_launch_mob(handle.mob_id().as_str());
        let owners = self.clone();
        let handle = handle.observer();
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            while !owners.seeded(&handle).await.landed() {
                tokio::time::sleep(wait).await;
                wait = next_wait(wait);
            }
            drop(handle);
            owners.follow_loaded_mobs().await;
        });
    }

    /// Read, in the background and until the read succeeds, the identities
    /// whose retirement `store`, the launch's memory store, recorded as
    /// private in `realm`: they are members of the launch's memory.
    pub(crate) fn bind_private_retirements(
        &self,
        store: Arc<dyn crate::memory::capabilities::StewardStore>,
        realm: String,
    ) {
        let owners = self.clone();
        // One more first read is owed from this moment.
        self.inner
            .first_reads
            .0
            .send_modify(|reads| reads.in_flight += 1);
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            let mut failed = false;
            loop {
                match store.private_retired_identities(&realm).await {
                    Ok(identities) => {
                        owners
                            .inner
                            .retired
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .entry(MemoryHome::Launch)
                            .or_default()
                            .extend(member_names(identities));
                        owners.inner.first_reads.0.send_modify(|reads| {
                            // On its first attempt unless it failed or a
                            // reader's wait made every read in flight overdue.
                            if !failed && reads.in_flight > 0 {
                                reads.in_flight -= 1;
                            } else {
                                reads.overdue = reads.overdue.saturating_sub(1);
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        if !failed {
                            failed = true;
                            owners.inner.first_reads.0.send_modify(|reads| {
                                if reads.in_flight > 0 {
                                    reads.in_flight -= 1;
                                    reads.overdue += 1;
                                }
                            });
                        }
                        tracing::warn!(
                            %error,
                            retry_in_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                            "the private retirements the memory store recorded could not be \
                             read; trying again"
                        );
                        tokio::time::sleep(wait).await;
                        wait = next_wait(wait);
                    }
                }
            }
        });
    }

    /// [`Self::bind_private_retirements`] for `store`, the memory store of
    /// the mob `mob_id`: the identities whose retirement it recorded as
    /// private in `realm` are members of the memory of that mob
    /// ([`Self::memory_home_of`]). For the launch mob, and for a mob a member
    /// created, that is the launch's memory, and the binding is the
    /// launch's.
    ///
    /// For a mob the launch composes beside its launch mob the read is
    /// that mob's own. It is owed from this call until it has succeeded,
    /// and meanwhile a member of that mob the record does not hold is not
    /// known ([`Self::person_for_a_reader`]); it is not one of the record's
    /// first reads ([`Self::first_read_pending`]), so it keeps no reader of
    /// another mob apart. A later binding of the same mob takes its place,
    /// and so does a mob created anew under the id
    /// ([`Self::forget_the_read_of`]): a binding that is no longer the
    /// mob's stops, and what it read is dropped. Before this call nothing
    /// is owed for the mob's memory: a host binds a mob's memory where it
    /// composes it.
    ///
    /// The mob's own read holds `store` weakly: the caller keeps the mob's
    /// memory alive for as long as it is composed, and the read ends with
    /// it. A read that is still owed then stays owed, so a member of the
    /// mob the record does not hold stays not known until the mob's next
    /// composition binds its memory anew.
    pub(crate) fn bind_private_retirements_of(
        &self,
        mob_id: &meerkat_mob::MobId,
        store: Arc<dyn crate::memory::capabilities::StewardStore>,
        realm: String,
    ) {
        let mob = match self.memory_home_of(mob_id.as_str()).view() {
            Some(MemoryHome::Hosted(mob)) => mob,
            Some(_) => return self.bind_private_retirements(store, realm),
            // Its members are members of no memory: nothing to read.
            None => return,
        };
        let binding = self
            .inner
            .retirement_bindings
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.mob_reads.0.send_modify(|reads| {
            reads.retirements_owed.insert(mob.clone(), binding);
        });
        let owners = self.clone();
        // The mob's store is held weakly: the mob's composed memory owns
        // it, and this read ends with it (the mob is unloaded) instead of
        // keeping the store open and trying a store that keeps failing
        // without end. The next composition of the mob binds anew.
        let store = Arc::downgrade(&store);
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            loop {
                let Some(of_the_mob) = store.upgrade() else {
                    return;
                };
                let read = of_the_mob.private_retired_identities(&realm).await;
                drop(of_the_mob);
                let is_the_mobs = || {
                    owners.inner.mob_reads.0.borrow().retirements_owed.get(&mob) == Some(&binding)
                };
                match read {
                    Ok(identities) => {
                        {
                            // Held while the binding is looked at: a mob
                            // forgotten meanwhile takes its binding away
                            // first and what was read after it.
                            let mut retired = owners
                                .inner
                                .retired
                                .write()
                                .unwrap_or_else(PoisonError::into_inner);
                            if !is_the_mobs() {
                                return;
                            }
                            retired
                                .entry(MemoryHome::Hosted(mob.clone()))
                                .or_default()
                                .extend(member_names(identities));
                        }
                        owners.inner.mob_reads.0.send_if_modified(|reads| {
                            let landed = reads.retirements_owed.get(&mob) == Some(&binding);
                            if landed {
                                reads.retirements_owed.remove(&mob);
                            }
                            landed
                        });
                        return;
                    }
                    Err(_) if !is_the_mobs() => return,
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            mob = %mob,
                            retry_in_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                            "the private retirements a mob's memory store recorded could not \
                             be read; trying again"
                        );
                        tokio::time::sleep(wait).await;
                        wait = next_wait(wait);
                    }
                }
            }
        });
    }

    /// Read the owners the log `log` names for the members it seated.
    /// `admitted_before` is what [`Self::creates_admitted`] answered before
    /// the pass listed the mob `log` is of: the rows of a mob under whose
    /// id a create was admitted since are not kept, as nothing else the
    /// pass read of that mob is ([`Self::note_the_mob_read`]).
    async fn read_log(
        &self,
        log: &dyn MemberSpawnLog,
        admitted_before: &CreatesAdmitted,
    ) -> Result<(), String> {
        let mut logged = BTreeMap::new();
        let mut cursor = 0;
        while let Some((steps, next)) = log.spawns_after(cursor, SEED_PAGE).await? {
            cursor = next;
            for step in steps {
                self.note_a_journal_step_of(&step, admitted_before);
                if let HolderStep::Seated { owner, .. } = step.step {
                    logged.insert((step.mob_id, step.member), owner);
                }
            }
        }
        let dates = self
            .inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        logged.retain(|(mob, _), _| dates.admitted.get(mob) == admitted_before.get(mob));
        self.inner
            .logged
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(logged);
        drop(dates);
        Ok(())
    }

    /// Read every mob the agent mob tools host: its event log first (so the
    /// owners of the members that created mobs are known), then who created
    /// it (its durable creation record).
    async fn read_child_mobs(&self, state: &meerkat_mob_mcp::MobMcpState) -> Result<(), String> {
        // Before the mobs are listed: a mob created under an id from here
        // on is not the mob this pass holds.
        let admitted_before = self.creates_admitted();
        self.read_creations_of_mobs_not_loaded(state).await?;
        let mobs = state.mob_list().await.map_err(|error| error.to_string())?;
        let mut handles = Vec::with_capacity(mobs.len());
        for (mob_id, _) in mobs {
            match state.handle_for(&mob_id).await {
                Ok(handle) => handles.push((mob_id, handle)),
                // A mob destroyed or held while the seed reads: no member of
                // it spawns, so it needs no owner now.
                Err(MobError::MobNotFound(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        self.read_mobs(handles, &admitted_before).await
    }

    /// Take the creations the mob set keeps of the mobs it has not loaded
    /// (`MobInventory::dormant_creation`). A mob that runs can have been
    /// created by a member of a mob that is stored and not loaded, and
    /// [`Self::memory_home_of`] follows a mob's creation through that mob
    /// too. It loads no mob and opens no store: a mob whose creation the
    /// set has not read stays unknown to this record.
    async fn read_creations_of_mobs_not_loaded(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
    ) -> Result<(), String> {
        let inventory = state
            .mob_inventory()
            .await
            .map_err(|error| error.to_string())?;
        for (mob_id, provenance) in &inventory.dormant_creation {
            self.record_creation(mob_id.as_str(), MobCreation::of(provenance));
        }
        Ok(())
    }

    /// Read the durable records of each of `mobs`: its event log first (so
    /// the owners of the members that created mobs are known), then who
    /// created it. A mob whose records were read is noted with its
    /// incarnation, once both were read. `admitted_before` is what
    /// [`Self::creates_admitted`] answered before `mobs` was listed: a mob
    /// under whose id a create was admitted since is left unread by this
    /// pass, with nothing written from its creation record and its owed
    /// read still owed. The create changes the mobs the set manages, and
    /// the pass that follows that change reads the mob that holds the id.
    /// A mob whose creating member's owner nothing says for certain
    /// ([`MobOwner::CreatorUnknown`]) is recorded as such and not noted
    /// read: it is read again at the next reading of the mob set, when
    /// that member's mob may say it. A council's mob whose council record
    /// or convener could not be read is left as it was, to be read again
    /// likewise.
    async fn read_mobs(
        &self,
        mobs: Vec<(meerkat_mob::MobId, meerkat_mob::MobHandle)>,
        admitted_before: &CreatesAdmitted,
    ) -> Result<(), String> {
        let mut creations = Vec::with_capacity(mobs.len());
        for (mob_id, handle) in mobs {
            self.read_log(&handle, admitted_before).await?;
            if !self
                .date_the_mob(mob_id.as_str(), &handle, admitted_before)
                .await?
            {
                continue;
            }
            let provenance = handle
                .provenance()
                .await
                .map_err(|error| error.to_string())?;
            self.read_creations_above(&MobCreation::of(&provenance))
                .await;
            creations.push((mob_id, handle.incarnation(), provenance));
        }
        for (mob_id, incarnation, provenance) in creations {
            let Some(owner) = self
                .owner_by_its_records(&mob_id, incarnation, &provenance)
                .await
            else {
                continue;
            };
            self.note_the_mob_read(
                mob_id.as_str(),
                incarnation,
                owner,
                MobCreation::of(&provenance),
                admitted_before,
            );
        }
        Ok(())
    }

    /// Read who created the mobs above a mob whose creation is `creation`,
    /// where this record holds no creation of them: the creating mob it
    /// names, that mob's creating mob, and so on, at most the mobs the walk
    /// of a line reads ([`MEMORY_HOME_STEPS`]). A creating mob that is
    /// stored and not loaded can be one whose store the mob set's own sweep
    /// has not read, so the set's listing holds no creation of it
    /// ([`Self::read_creations_of_mobs_not_loaded`]). The set's typed read
    /// of one such mob answers it from that one store and loads nothing.
    /// Where the set answers nothing (it manages the mob, whose own load
    /// this record reads; the store holds no creation that can be read; no
    /// mob is stored under the id) nothing is recorded: no creation is
    /// derived for a mob.
    async fn read_creations_above(&self, creation: &MobCreation) {
        let Some(state) = self.child_mob_state() else {
            return;
        };
        let MobCreation::ByAMemberOf(creating_mob) = creation else {
            return;
        };
        let mut above = creating_mob.clone();
        for _ in 0..=MEMORY_HOME_STEPS {
            let recorded = self
                .inner
                .mob_creations
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&above)
                .cloned();
            let creation = match recorded {
                Some(creation) => creation,
                None => {
                    let Ok(Some(provenance)) = state
                        .unloaded_mob_creation(&meerkat_mob::MobId::from(above.as_str()))
                        .await
                    else {
                        return;
                    };
                    let creation = MobCreation::of(&provenance);
                    self.record_creation(&above, creation.clone());
                    creation
                }
            };
            let MobCreation::ByAMemberOf(next) = creation else {
                return;
            };
            above = next;
        }
    }

    /// A pass read the durable records of the mob it listed under `mob_id`
    /// at `incarnation`: record who the mob is private to and the mob of
    /// its creating member, and note the mob read at its incarnation.
    /// `admitted_before` is what [`Self::creates_admitted`] answered before
    /// the pass listed the mob.
    ///
    /// When a create was admitted under the id since, nothing is written:
    /// what the pass read may be an earlier mob's, and the read of the mob
    /// that holds the id is owed to the pass that lists it after the
    /// create. The comparison and the writes are under the lock the
    /// admission counts under ([`Self::record_created_mob`]), and the
    /// admission forgets what was read under the id after it counted: what
    /// a pass writes lands before the count and is forgotten, or is not
    /// written.
    fn note_the_mob_read(
        &self,
        mob_id: &str,
        incarnation: meerkat_core::service::MobIncarnationId,
        owner: MobOwner,
        creation: MobCreation,
        admitted_before: &CreatesAdmitted,
    ) {
        let dates = self
            .inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if dates.admitted.get(mob_id) != admitted_before.get(mob_id) {
            return;
        }
        // The mob the set manages under the id runs at this incarnation.
        self.meet(mob_id, incarnation);
        // A mob whose creating member's owner nothing says for certain is
        // recorded as such and not noted read: the next reading of the mob
        // set takes it again.
        let settled = owner != MobOwner::CreatorUnknown;
        self.record_mob(mob_id, owner);
        self.record_creation(mob_id, creation);
        if settled {
            self.inner
                .mobs_read
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert((mob_id.to_string(), incarnation));
        }
        // The mob's durable records are read: what the record does not
        // hold of its members from now on is private to no one, and a
        // read that was owed for it is not owed any more.
        self.inner.mob_reads.0.send_modify(|reads| {
            reads.owed.remove(mob_id);
            reads.read.insert(mob_id.to_string());
        });
        drop(dates);
    }

    /// Who the mob hosted under `mob_id` at `incarnation` is private to, as
    /// its records say: its creating member's owner, said for certain
    /// ([`Self::certain_owner_of`]), and [`MobOwner::CreatorUnknown`] when
    /// it cannot be.
    ///
    /// A temporary council's mob is created for a session of the council's
    /// own, so its creation record names no member. For a mob a council's
    /// record owns ([`Self::council_mob`]) the creating session is the one
    /// that convened the council, taken from the council's record, and it
    /// is resolved as the create admission resolves it
    /// ([`Self::council_mob_owner`]): what is read here and what the
    /// admission wrote say the same, but for a convening session no roster
    /// names. The admission takes that session for no member and records
    /// the mob as private to no one; the read takes it for a convener that
    /// left, so who the mob is private to cannot be read: the closed
    /// answer. A council no member convened is private to no one. The
    /// lookup of the convening session is the mob set's
    /// (`session_member_matches`): where the mob that session's stored
    /// binding names is not loaded, the lookup loads that one mob. `None`
    /// for a council record or a roster lookup that cannot be read: nothing
    /// is recorded, and the mob is read again at the next reading.
    async fn owner_by_its_records(
        &self,
        mob_id: &meerkat_mob::MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
        provenance: &meerkat_mob::MobProvenance,
    ) -> Option<MobOwner> {
        let state = self.child_mob_state();
        let council = match state.as_ref() {
            Some(state) => Self::council_mob(state, mob_id, incarnation).await,
            None => CouncilMob::Ordinary,
        };
        match (council, state) {
            (CouncilMob::Ordinary, state) => Some(match creating_member(provenance) {
                CreatingMember::Named(parent, creator) => {
                    let certain = match (state, creating_members_seat(provenance)) {
                        (Some(state), Some(seat)) => {
                            self.certain_owner_of(&state, parent, creator, seat).await
                        }
                        // A record that follows no mob set cannot say which
                        // mob is hosted under the creating member's id.
                        _ => None,
                    };
                    certain.map_or(MobOwner::CreatorUnknown, MobOwner::Known)
                }
                CreatingMember::NoMember => MobOwner::Known(None),
                CreatingMember::Undecidable => MobOwner::Undecidable,
            }),
            (CouncilMob::Unreadable, _) | (CouncilMob::Convened(Some(_)), None) => None,
            (CouncilMob::Convened(None), _) => Some(MobOwner::Known(None)),
            (CouncilMob::Convened(Some(session)), Some(state)) => {
                let conveners = match state.session_member_matches(&session).await {
                    Ok(found) => found
                        .into_iter()
                        .map(|found| {
                            (
                                found.mob_id,
                                found.incarnation,
                                found.identity,
                                found.generation,
                            )
                        })
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %mob_id,
                            %error,
                            "who convened a council could not be looked up; who its mob is \
                             private to is read again at the next reading of the mob set"
                        );
                        return None;
                    }
                };
                Some(self.council_mob_owner(&state, &conveners, &session).await)
            }
        }
    }

    /// The mob hosted under `mob_id` in `state` now, if one is. The lookup
    /// starts nothing: a mob that is held or quarantined is not hosted, and
    /// stays as it is (a lookup by id would try a quarantined mob's restore
    /// again).
    async fn hosted_mob(
        state: &meerkat_mob_mcp::MobMcpState,
        mob_id: &meerkat_mob::MobId,
    ) -> Option<meerkat_mob::MobHandle> {
        state
            .mob_handles_snapshot()
            .await
            .ok()?
            .into_iter()
            .find_map(|(hosted, handle)| (hosted == *mob_id).then_some(handle))
    }

    /// Whom the member a record names is private to, when that can be said
    /// for certain: `member` of the mob `mob_id`, seated as `seat` says.
    /// The mob hosted under the id must be the incarnation the seat names:
    /// a mob created under the id since is another mob. What that mob's own
    /// records hold of the member decides it
    /// ([`Self::owner_its_mobs_records_say`]): its member history and its
    /// roster row. A mob that is known and not loaded says the same from
    /// its store, read without loading it
    /// ([`Self::owner_from_a_mob_not_loaded`]). `None` when no mob is
    /// hosted or stored under the id at that incarnation, or when its
    /// records do not say: the member is then not taken to be private to
    /// no one. This function loads no mob ([`Self::hosted_mob`]). The
    /// lookup that names a council's convener to it is the mob set's, and
    /// can load one mob ([`Self::owner_by_its_records`]).
    async fn certain_owner_of(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
        mob_id: &meerkat_mob::MobId,
        member: &meerkat_mob::AgentIdentity,
        seat: CreatorsSeat<'_>,
    ) -> Option<Option<String>> {
        let hosted = Self::hosted_mob(state, mob_id)
            .await
            .filter(|hosted| hosted.incarnation() == seat.ran_at);
        let Some(handle) = hosted else {
            return self
                .owner_from_a_mob_not_loaded(state, mob_id, member, seat)
                .await;
        };
        let history = handle.member_binding_history().await.ok();
        let row = handle.get_member(member).await.ok().flatten();
        let row = row.as_ref().map(|row| SeatedRow {
            generation: row.generation,
            session: row.bridge_session_id(),
            labels: &row.labels,
        });
        self.owner_its_mobs_records_say(mob_id, member, seat, history.as_ref(), row)
    }

    /// Whom the member `member` of the mob `mob_id`, seated as `seat` says,
    /// is private to, as that mob's own records say it: its member history
    /// `history` and the roster row `row` under the member's name, where
    /// they were read. The member is the one seated at the seat's
    /// generation on the seat's session, or seated again on that session
    /// since, not whoever holds its name: its own binding in the history
    /// says who it is private to, with the labels the session's latest
    /// incarnation was seated with ([`creators_binding`]), whether it still
    /// sits there, has left, or a later member holds its name. A member
    /// this record knows as private stays private when its binding names
    /// nobody, while no later member holds its name
    /// ([`Self::owner_a_binding_says`]). Where the history holds no such
    /// binding, or was not read, the roster row says it, and only when it
    /// is the creating member's own: of the seat's generation and on the
    /// seat's session ([`SeatedRow::is_seated_as`]). `None` when neither
    /// says: nothing is taken from the member's name.
    #[allow(clippy::option_option)]
    fn owner_its_mobs_records_say(
        &self,
        mob_id: &meerkat_mob::MobId,
        member: &meerkat_mob::AgentIdentity,
        seat: CreatorsSeat<'_>,
        history: Option<&meerkat_mob::MemberBindingHistory>,
        row: Option<SeatedRow<'_>>,
    ) -> Option<Option<String>> {
        if let Some(bound) = history.and_then(|history| creators_binding(history, member, seat)) {
            return Some(self.owner_a_binding_says(mob_id, member, &bound));
        }
        let row = row.filter(|row| row.is_seated_as(seat))?;
        Some(
            crate::access::private_owner(row.labels)
                .map(str::to_string)
                .or_else(|| self.owner_of(mob_id.as_str(), member.as_str())),
        )
    }

    /// Whom the member `member` of the mob `mob_id` is private to, as its
    /// own binding `bound` says: the person its labels name. A binding that
    /// names nobody leaves a member this record knows as private private,
    /// while the member is the latest one seated under its name: what the
    /// record holds under the name is then the member's own. Once a later
    /// member holds the name, the binding alone answers.
    fn owner_a_binding_says(
        &self,
        mob_id: &meerkat_mob::MobId,
        member: &meerkat_mob::AgentIdentity,
        bound: &CreatorsBinding,
    ) -> Option<String> {
        let held_under_the_name = || {
            bound
                .holds_its_name
                .then(|| self.owner_of(mob_id.as_str(), member.as_str()))
                .flatten()
        };
        bound.owner.clone().or_else(held_under_the_name)
    }

    /// Whom the member `member` of the mob `mob_id`, seated as `seat` says,
    /// is private to, for a mob that is known and not loaded: read from the
    /// mob's store without loading the mob
    /// ([`meerkat_mob_mcp::MobMcpState::unloaded_mob_member`]). The stored
    /// mob must be the incarnation the seat names. Its stored
    /// member history decides it as a hosted mob's does
    /// ([`Self::owner_its_mobs_records_say`]), also for a member that left
    /// that mob or whose name a later member holds. No roster row is read
    /// from a store: the member's own binding answers, or nothing does (a
    /// stored mob is opened as a store that keeps a member history; where
    /// that history cannot be read, nothing is said). `None` when the store
    /// cannot be read, holds another incarnation, or its records do not
    /// say: the member is then not taken to be private to no one.
    async fn owner_from_a_mob_not_loaded(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
        mob_id: &meerkat_mob::MobId,
        member: &meerkat_mob::AgentIdentity,
        seat: CreatorsSeat<'_>,
    ) -> Option<Option<String>> {
        let stored = state.unloaded_mob_member(mob_id).await.ok().flatten()?;
        if stored.incarnation != seat.ran_at {
            return None;
        }
        self.owner_its_mobs_records_say(mob_id, member, seat, stored.history.as_ref(), None)
    }

    /// What the runtime's council store says of the mob hosted under
    /// `mob_id` at `incarnation`. A council's record owns the mob when the
    /// id is the one Core derives from the council's and the record names
    /// that incarnation as the mob its create made, or names none yet (its
    /// create is under way, or the record was written before the mob was
    /// recorded) while the council is unfinished. A record that names
    /// another incarnation, or a settled one that names none, owns no mob
    /// hosted now: a mob under its id is an ordinary mob. One read of the
    /// store, for a mob whose id has the form; this read loads no mob,
    /// member or session. For a council a member convened, the lookup of
    /// the convening session follows it: that lookup is the mob set's, and
    /// loads the one mob that session's stored binding names when it is not
    /// loaded ([`Self::owner_by_its_records`]).
    async fn council_mob(
        state: &Arc<meerkat_mob_mcp::MobMcpState>,
        mob_id: &meerkat_mob::MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) -> CouncilMob {
        use meerkat_mob::temporary_council::{TemporaryCouncilId, TemporaryCouncilMobCustody};
        let Some(council_id) = TemporaryCouncilId::of_temporary_mob_id(mob_id) else {
            return CouncilMob::Ordinary;
        };
        let record = match state.temporary_council().load(&council_id).await {
            Ok(Some(record)) if record.temporary_mob_id == *mob_id => record,
            Ok(_) => return CouncilMob::Ordinary,
            Err(error) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    %error,
                    "the council record of a mob under a council's mob id could not be read; \
                     who the mob is private to is read again at the next reading of the mob set"
                );
                return CouncilMob::Unreadable;
            }
        };
        let owns = match record.temporary_mob_custody {
            Some(TemporaryCouncilMobCustody::Created { incarnation: own }) => own == incarnation,
            Some(TemporaryCouncilMobCustody::NotCreated) => false,
            // The record does not name its mob: while the council is
            // unfinished the mob may be its own.
            Some(_) | None => record.is_unfinished(),
        };
        if owns {
            CouncilMob::Convened(record.convener.map(|convener| convener.session_id))
        } else {
            CouncilMob::Ordinary
        }
    }

    /// Who a council's mob is private to, for the members `conveners` the
    /// lookup found `session`, the session that convened the council, to
    /// be, each with its mob, the incarnation that mob runs at and the
    /// generation it is seated at. One: whoever that member is private to,
    /// said for certain ([`Self::certain_owner_of`]), and
    /// [`MobOwner::CreatorUnknown`] when nothing says it: the mob is
    /// read again at the next reading of the mob set, and is never taken to
    /// be private to no one. None: the council's record holds a convener
    /// and no roster names its session any more (the member left, or runs
    /// on another session), so who the mob is private to cannot be read.
    /// Several: the record does not say which of them convened the council,
    /// so it cannot be read either.
    async fn council_mob_owner(
        &self,
        state: &meerkat_mob_mcp::MobMcpState,
        conveners: &[Convener],
        session: &meerkat_core::types::SessionId,
    ) -> MobOwner {
        match conveners {
            [(conveners_mob, ran_at, convener, generation)] => {
                let seat = CreatorsSeat {
                    ran_at: *ran_at,
                    generation: *generation,
                    session,
                };
                self.certain_owner_of(state, conveners_mob, convener, seat)
                    .await
                    .map_or(MobOwner::CreatorUnknown, MobOwner::Known)
            }
            _ => MobOwner::Undecidable,
        }
    }

    /// Record, for the mob `mob_id`, what [`Self::council_mob_owner`]
    /// answers for `conveners` and the convening `session`, as the read of
    /// a council's mob records it: for rows outside this module, which
    /// cannot seat one session on two rosters or leave a seated member's
    /// owner unsaid.
    #[cfg(test)]
    pub(crate) async fn record_council_mob_convened_by(
        &self,
        state: &Arc<meerkat_mob_mcp::MobMcpState>,
        mob_id: &str,
        conveners: &[Convener],
        session: &meerkat_core::types::SessionId,
    ) {
        let owner = self.council_mob_owner(state, conveners, session).await;
        self.record_mob(mob_id, owner);
    }

    /// Test builds only: read `handle`'s mob as the record reads a mob the
    /// set manages ([`Self::read_mobs`]): its event log, then who created
    /// it. For a row whose mob reached the tools with no create admission.
    #[cfg(test)]
    pub(crate) async fn read_mob(&self, handle: &meerkat_mob::MobHandle) -> Result<(), String> {
        let admitted_before = self.creates_admitted();
        self.read_mobs(
            vec![(handle.mob_id().clone(), handle.clone())],
            &admitted_before,
        )
        .await
    }

    /// The mobs the mob set manages now whose durable records this record
    /// has not read at their incarnation: a mob the set created or loaded
    /// since the last reading. It asks the set for the mobs it manages and
    /// loads none.
    async fn unread_loaded_mobs(
        &self,
    ) -> Result<Vec<(meerkat_mob::MobId, meerkat_mob::MobHandle)>, String> {
        let Some(state) = self.child_mob_state() else {
            return Ok(Vec::new());
        };
        self.read_creations_of_mobs_not_loaded(&state).await?;
        let managed = state
            .mob_handles_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let mut read = self
            .inner
            .mobs_read
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let now: BTreeSet<(String, meerkat_core::service::MobIncarnationId)> = managed
            .iter()
            .map(|(mob_id, handle)| (mob_id.to_string(), handle.incarnation()))
            .collect();
        read.retain(|mob| now.contains(mob));
        // A managed mob whose read is owed is unread, whatever was read of
        // it before: a mob that was unloaded and loaded again while no
        // reading saw it gone has the incarnation this record has read,
        // and the mark the host set for the second load is cleared only by
        // a reading of the mob.
        let owed = self.inner.mob_reads.0.borrow().owed.clone();
        Ok(managed
            .into_iter()
            .filter(|(mob_id, handle)| {
                owed.contains(mob_id.as_str())
                    || !read.contains(&(mob_id.to_string(), handle.incarnation()))
            })
            .collect())
    }

    /// Test builds only: one reading of the durable records of every mob
    /// the mob set came to manage since this record's last reading, with no
    /// retry. The runtime reads through
    /// [`Self::read_loaded_mobs_until_read`].
    #[cfg(test)]
    pub(crate) async fn read_loaded_mobs(&self) -> Result<(), String> {
        let admitted_before = self.creates_admitted();
        let unread = self.unread_loaded_mobs().await?;
        self.read_mobs(unread, &admitted_before).await
    }

    /// Read the durable records of every mob the mob set came to manage
    /// since this record's last reading: the third form beside the seed,
    /// which reads the mobs loaded when it runs, and a member's build, which
    /// records the member itself. A mob loaded after the seed (a stored mob
    /// the set loads at its first use, a mob the set loads again) has its
    /// members' owners read here, from that mob's own event log, and no
    /// mob is loaded for it. The reading is tried until it succeeds,
    /// waiting longer after each failure, up to [`SEED_RETRY_MAX`]. A
    /// reading that finds a mob to read is a first read this record owes
    /// until it succeeds, as the seed is: a console keeps every caller who
    /// is not an admin apart meanwhile. It gives up once nothing holds the
    /// mob set.
    async fn read_loaded_mobs_until_read(&self) {
        let mut wait = std::time::Duration::from_millis(250);
        let (mut owed, mut failed) = (false, false);
        loop {
            // Before the mobs are listed: a mob created under an id from
            // here on is not the mob this pass holds.
            let admitted_before = self.creates_admitted();
            let read = match self.unread_loaded_mobs().await {
                Ok(unread) if unread.is_empty() => Ok(()),
                Ok(unread) => {
                    if !owed {
                        owed = true;
                        self.inner
                            .first_reads
                            .0
                            .send_modify(|reads| reads.in_flight += 1);
                    }
                    self.read_mobs(unread, &admitted_before).await
                }
                Err(error) => Err(error),
            };
            let gone = self.child_mob_state().is_none();
            if read.is_ok() || gone {
                if owed {
                    self.inner.first_reads.0.send_modify(|reads| {
                        // On its first attempt unless it failed or a
                        // reader's wait made every read in flight overdue.
                        if !failed && reads.in_flight > 0 {
                            reads.in_flight -= 1;
                        } else {
                            reads.overdue = reads.overdue.saturating_sub(1);
                        }
                    });
                }
                return;
            }
            if owed && !failed {
                failed = true;
                self.inner.first_reads.0.send_modify(|reads| {
                    if reads.in_flight > 0 {
                        reads.in_flight -= 1;
                        reads.overdue += 1;
                    }
                });
            }
            if let Err(error) = read {
                tracing::warn!(
                    %error,
                    retry_in_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    "who the members of a mob loaded since the last reading are private to \
                     could not be read from its durable records; trying again"
                );
            }
            tokio::time::sleep(wait).await;
            wait = next_wait(wait);
        }
    }

    /// Follow the mob set for as long as something holds it: one reading
    /// now, and one after each change of the mobs the set manages.
    async fn follow_loaded_mobs(&self) {
        let Some(mut changes) = self.child_mob_state().map(|state| state.mob_set_changes()) else {
            return;
        };
        loop {
            self.read_loaded_mobs_until_read().await;
            if changes.changed().await.is_err() {
                return;
            }
        }
    }

    /// The agent mob tool state whose mobs the seed reads.
    pub(crate) fn bind_child_mobs(&self, state: &Arc<meerkat_mob_mcp::MobMcpState>) {
        *self
            .inner
            .child_mobs
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::downgrade(state));
    }

    fn child_mob_state(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.inner
            .child_mobs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    /// The handle of the mob that stands under `mob_id` now in the mob set
    /// this record follows, when that mob is loaded. `None` for a mob that
    /// is not loaded, and for a record that follows no mob set or one whose
    /// set has gone. The set's snapshot of its handles is read: it loads
    /// no mob and opens no store, and it withholds a mob whose idle unload
    /// is admitted. Never the set's lookup of one mob, which loads a mob
    /// that is stored and not loaded.
    pub(crate) async fn loaded_mob(
        &self,
        mob_id: &str,
    ) -> Result<Option<meerkat_mob::MobHandle>, String> {
        let Some(state) = self.child_mob_state() else {
            return Ok(None);
        };
        let handles = state
            .mob_handles_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        Ok(handles
            .into_iter()
            .find(|(id, _)| id.as_str() == mob_id)
            .map(|(_, handle)| handle))
    }

    /// The mob set these owners were last bound to, while something still
    /// holds it: for the rows that read what a launch built of its mob set.
    #[cfg(test)]
    pub(crate) fn child_mob_state_for_tests(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.child_mob_state()
    }

    /// Record what the roster of `handle`'s mob holds for `member`, as its
    /// build would: for a member this process may not have built, before a
    /// spawn reads its owner (the source of a prompt-context fork).
    pub(crate) async fn record_roster_member(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) {
        if let Ok(Some(entry)) = handle.get_member(member).await {
            self.record_built(
                handle.mob_id().as_str(),
                member.as_str(),
                Some(&entry.labels),
                Some(handle.incarnation()),
            );
        }
    }

    /// Bind `release` as one of the releases a create asks before its mob
    /// exists ([`Self::before_create_hook`]). They are asked in the order
    /// they were bound.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "bound by the composition that owns what is released; none binds one yet"
        )
    )]
    pub(crate) fn bind_create_release(&self, release: Arc<dyn MobCreateRelease>) {
        self.inner
            .create_releases
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(release);
    }

    /// Install `follower` as what follows the end of a member private to a
    /// person in the launch's schedules ([`MemberEndSeat::Schedules`]), and
    /// hand it the ends seen before it, oldest first. A later install
    /// replaces it.
    pub(crate) async fn follow_member_ends(&self, follower: Arc<dyn PrivateMemberEndFollower>) {
        self.follow_member_ends_as(MemberEndSeat::Schedules, follower)
            .await;
    }

    /// Install `follower` in `seat`, and hand it the ends the seat held
    /// while it had no follower, oldest first. A launch installs one
    /// follower in a seat; a later install in the seat replaces it.
    pub(crate) async fn follow_member_ends_as(
        &self,
        seat: MemberEndSeat,
        follower: Arc<dyn PrivateMemberEndFollower>,
    ) {
        let held = {
            let mut ends = self
                .inner
                .ends
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let seat = ends.seats.entry(seat).or_default();
            seat.follower = Some(Arc::clone(&follower));
            std::mem::take(&mut seat.held)
        };
        for ended in &held {
            follower.member_ended(ended).await;
        }
    }

    /// Hand `ended` to the follower of each seat, and hold it for each seat
    /// that has none, for the follower a composition installs there later
    /// ([`Self::follow_member_ends_as`]). A seat's hold is this process's
    /// and bounded ([`HELD_ENDS_MAX`]): a launch that installs no follower
    /// in it keeps the newest ends there. Answers whether a follower took
    /// the end; `false` for one that every seat only holds.
    pub(crate) async fn member_ended(&self, ended: EndedPrivateMember) -> bool {
        let followers: Vec<Arc<dyn PrivateMemberEndFollower>> = {
            let mut ends = self
                .inner
                .ends
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let mut followers = Vec::new();
            for seat in MemberEndSeat::ALL {
                let seat_ends = ends.seats.entry(seat).or_default();
                match seat_ends.follower.as_ref() {
                    Some(follower) => followers.push(Arc::clone(follower)),
                    None => {
                        if seat_ends.held.len() >= HELD_ENDS_MAX {
                            let dropped = seat_ends.held.remove(0);
                            tracing::warn!(
                                mob_id = %dropped.mob_id,
                                member = %dropped.member,
                                seat = ?seat,
                                "more ends of members private to a person are held than the \
                                 hold keeps: the oldest is dropped, and what follows it is not \
                                 done"
                            );
                        }
                        seat_ends.held.push(ended.clone());
                    }
                }
            }
            followers
        };
        for follower in &followers {
            follower.member_ended(&ended).await;
        }
        !followers.is_empty()
    }

    /// Test builds only: [`Self::observe_mob_event_since`] for an event of
    /// the mob that holds its id now, as a pump that subscribed just now
    /// shows it.
    #[cfg(test)]
    pub(crate) async fn observe_mob_event(
        &self,
        handle: &meerkat_mob::MobHandle,
        event: &meerkat_mob::MobEvent,
    ) -> bool {
        self.observe_mob_event_since(handle, event, &self.creates_admitted())
            .await
    }

    /// Show the record an event of the journal of `handle`'s mob, the
    /// runtime's own: the end of a member private to a person is handed to
    /// the follower ([`Self::member_ended`]). Answers `true` when the event
    /// is such an end and no follower took it: it is only held, and a
    /// launch that installs no follower has not followed it.
    ///
    /// Only a retirement that ends the member's name is an end. A
    /// retirement start that carries a rotation (a reset, a same-session
    /// successor, a role migration, a runtime-mode restamp) or keeps the
    /// member's topology for its successor (a respawn) seats the next
    /// incarnation under the name. One that records a cause is a step of an
    /// identity entry's own lifecycle (an archive, an unlisting, a removal,
    /// a host's adoption), which the identity family follows. Neither is
    /// handed over, and nothing of the member is touched.
    ///
    /// `admitted_at_start` is what [`Self::creates_admitted`] answered
    /// before the pump that shows the event subscribed to its mob's
    /// journal. A row of a mob under whose id a create was admitted since
    /// is an earlier mob's: the pump is ended by a pass that nothing orders
    /// before that admission. Such a row is not noted, and the end of a
    /// member it removes is not followed
    /// ([`Self::follow_a_shown_journal_step`]); neither is the end of an
    /// ephemeral private member it retires handed over: the answer is
    /// `false`.
    pub(crate) async fn observe_mob_event_since(
        &self,
        handle: &meerkat_mob::MobHandle,
        event: &meerkat_mob::MobEvent,
        admitted_at_start: &CreatesAdmitted,
    ) -> bool {
        // Who holds a member's name is followed for every member: a later
        // seating under a name starts another member's time.
        if let Some(step) = MemberJournalStep::of_event(event) {
            self.follow_a_shown_journal_step(&step, event.timestamp, admitted_at_start)
                .await;
        }
        let meerkat_mob::MobEventKind::MemberRetirementStarted {
            agent_identity,
            generation,
            session_id,
            releasing,
            rotation,
            preserve_machine_topology,
            cause,
            ..
        } = &event.kind
        else {
            return false;
        };
        if rotation.is_some() || *preserve_machine_topology || cause.is_some() {
            return false;
        }
        // The end of an ephemeral private member is followed only for the
        // mob that holds the id now: a pump whose mob's id got a create
        // since it subscribed shows an earlier mob's row, and hands no end.
        if !self.no_create_admitted_since(event.mob_id.as_str(), admitted_at_start) {
            return false;
        }
        let session = session_id.as_ref().or(releasing.as_ref());
        match self
            .ended_private(
                handle,
                agent_identity,
                *generation,
                session,
                event.timestamp,
            )
            .await
        {
            Some(ended) => !self.member_ended(ended).await,
            None => false,
        }
    }

    /// The retiring incarnation `generation` of `member`, when it was an
    /// ephemeral member private to a person: a side chat, or a member, a
    /// fork or a helper such a member made. Core's member history says so:
    /// the incarnation's own binding (the one that named `session`, the
    /// session its retirement names, else the member's latest) holds the
    /// labels and the continuity intent it was spawned with, and no later
    /// member under the name changes them.
    ///
    /// A member seated as a durable identity (a long-term agent, a member
    /// of a profile that declares `durable = true`) is not such a member,
    /// whoever it is private to, unless it is a side chat (only Meta writes
    /// a side chat's label, and a side chat is closed and forked again,
    /// never rebuilt): a long-term agent's member is retired and seated
    /// again on its session by rebuilds that keep the agent (a forced
    /// compaction, a reload), and what ends with the agent is the identity
    /// family's to follow. Where the history holds no binding of that
    /// incarnation (the mob's storage keeps no history, or every read of
    /// it fails), the answer is none: nothing is handed over on a guess.
    async fn ended_private(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
        generation: meerkat_mob::Generation,
        session: Option<&meerkat_core::types::SessionId>,
        ended_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<EndedPrivateMember> {
        let history = read_member_history(handle.mob_id().as_str(), member.as_str(), || {
            handle.member_binding_history()
        })
        .await?;
        let binding = session
            .and_then(|session| history.binding_for_session(session))
            .into_iter()
            .chain(history.last_binding(member))
            .find(|binding| binding.identity() == member && binding.generation() == generation)?;
        let labels = binding.labels();
        let ephemeral = matches!(
            binding.continuity_intent(),
            meerkat_mob::SpawnContinuityIntent::Ephemeral
        ) || labels.contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL);
        (ephemeral && crate::access::private_owner(labels).is_some()).then(|| EndedPrivateMember {
            mob_id: handle.mob_id().to_string(),
            member: member.to_string(),
            session: session.or(binding.session_id()).map(ToString::to_string),
            ended_at,
        })
    }

    /// `hook`, then record the labels of the member the request builds; a
    /// member private to a person is built without WorkGraph tools. The
    /// pre-build hook of the base session layer, which every member create
    /// and rebuild passes. The request names the incarnation of the mob
    /// the member is built in where the mob runtime made the build: a
    /// build of a mob this record knows ended records nothing
    /// ([`Self::record_built`]).
    pub(crate) fn after_hook(&self, hook: PreBuildHook) -> PreBuildHook {
        let owners = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let owners = owners.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                let private = req
                    .labels
                    .as_ref()
                    .and_then(crate::access::private_owner)
                    .is_some();
                if let Some(build) = req.build.as_mut() {
                    if let Some(binding) = build.mob_member_binding.as_ref() {
                        owners.record_built(
                            &binding.mob_id,
                            &binding.member,
                            req.labels.as_ref(),
                            build
                                .mob_incarnation
                                .map(meerkat_core::service::LiveMobIncarnation::incarnation),
                        );
                    }
                    if private {
                        build.override_workgraph = meerkat_core::ToolCategoryOverride::Disable;
                    }
                }
                Ok(())
            })
        })
    }

    /// The create admission of the agent mob tools: `then` (the host's own,
    /// when there is one) decides first, then each release bound to this
    /// record is asked ([`Self::bind_create_release`]), then the mob is
    /// recorded as private to its creator's owner. A creator whose owner
    /// cannot be read refuses the create, and so does a release that fails
    /// ([`CREATE_RELEASE_FAILED`]).
    ///
    /// The record written here is the admitted mob's own: it takes the
    /// place of a record that stands under the mob's id
    /// ([`Self::record_created_mob`]). After it, each release that was
    /// asked forgets what it holds of the id for an admitted create alone
    /// ([`MobCreateRelease::forget_at_admission`]).
    ///
    /// The creator is the member whose session the create names: the owner
    /// session, or, for the mob of a temporary council, the session that
    /// convened the council (the council's owner session is a session of
    /// its own and names no member). A council no member convened (a host
    /// ran it) has no creator, like a host's own create.
    ///
    /// The record decides who a member spawned into the mob later is
    /// private to ([`Self::apply_child`]). A council participant is not
    /// such a spawn: it is a fork built with its source's labels.
    ///
    /// The record is this process's: a council's mob left over from before
    /// a restart is read from its records, with the council's own record
    /// naming the session that convened it
    /// ([`Self::owner_by_its_records`]).
    pub(crate) fn before_create_hook(
        &self,
        state: Weak<meerkat_mob_mcp::MobMcpState>,
        then: Option<meerkat_mob_mcp::MobBeforeCreate>,
    ) -> meerkat_mob_mcp::MobBeforeCreate {
        let owners = self.clone();
        Arc::new(move |request: meerkat_mob_mcp::MobCreateRequest| {
            let owners = owners.clone();
            let state = state.clone();
            let then = then.clone();
            Box::pin(async move {
                if let Some(then) = then {
                    then(request.clone()).await?;
                }
                let creator_session = match request.council.as_ref() {
                    Some(council) => council.convener_session_id.as_ref(),
                    None => request.owner_bridge_session_id.as_ref(),
                };
                let (owner, creation) = match (creator_session, state.upgrade()) {
                    (Some(session), Some(state)) => {
                        match state.member_for_bridge_session(session).await {
                            Ok(Some((mob_id, creator))) => (
                                owners.owner_of(mob_id.as_str(), creator.as_str()),
                                MobCreation::ByAMemberOf(mob_id.to_string()),
                            ),
                            Ok(None) => (None, MobCreation::ByNoMember),
                            Err(error) => {
                                return Err(format!(
                                    "who creates mob '{}' could not be read: {error}",
                                    request.mob_id
                                ));
                            }
                        }
                    }
                    // A session is named and the mob set that would say
                    // whose it is has gone: which member creates the mob is
                    // not read.
                    (Some(_), None) => (None, MobCreation::Undecidable),
                    (None, _) => (None, MobCreation::ByNoMember),
                };
                // Each bound release goes before anything of the mob
                // exists, and before the record below is written: a release
                // that fails refuses the create, and the record stays as it
                // was.
                let releases = owners.create_releases();
                for release in &releases {
                    if let Err(error) = release.release_before_create(&request.mob_id).await {
                        // The release's own words stay with the host: they
                        // can name a file or a key of a store.
                        tracing::warn!(
                            mob_id = %request.mob_id,
                            release = release.what(),
                            error = %error,
                            "a release owed before a create failed; the create is refused"
                        );
                        return Err(CREATE_RELEASE_FAILED.to_string());
                    }
                }
                // The mob is created anew: the record counts the create and
                // then forgets what it read under the id, which was an
                // earlier mob's ([`Self::record_created_mob`]).
                if request.owner_bridge_session_id.is_some() {
                    // A mob created for a session is not one the launch
                    // composes, whatever an earlier mob of the id was.
                    owners.record_a_mob_created_for_a_session(
                        request.mob_id.as_str(),
                        MobOwner::Known(owner),
                        creation,
                    );
                } else {
                    // The host's own create: the launch composes this mob
                    // and names it itself.
                    owners.record_created_mob(
                        request.mob_id.as_str(),
                        MobOwner::Known(owner),
                        creation,
                    );
                }
                // Every release passed and the create is counted: each
                // release that was asked forgets what it holds of the id
                // for an admitted create alone
                // ([`MobCreateRelease::forget_at_admission`]).
                for release in &releases {
                    release.forget_at_admission(&request.mob_id);
                }
                Ok(())
            })
        })
    }

    /// The spawn customizer of the runtime's own mob: each member a private
    /// member asks for (a spawn its own turn makes, or a prompt-context fork
    /// of it) is private to that member's owner ([`Self::apply`]).
    pub(crate) fn spawn_customizer(&self) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(PrivateOwnerPropagation {
            owners: self.clone(),
        })
    }

    /// The spawn customizer of the mobs the runtime's members create: every
    /// member of a mob a private member created is private to its owner
    /// ([`Self::apply_child`]).
    pub(crate) fn child_spawn_customizer(&self) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(ChildOwnerPropagation {
            owners: self.clone(),
        })
    }

    /// Who the members' own operator tools may know of
    /// ([`meerkat_mob::MemberToolAudience`]): a member private to a person is
    /// hidden from every member not private to that person, as far as this
    /// record knows the members.
    pub(crate) fn member_tool_audience(&self) -> Arc<dyn meerkat_mob::MemberToolAudience> {
        Arc::new(self.clone())
    }

    /// The Steward's source of the members private to a person in the
    /// memory of `handle`'s mob ([`Self::memory_owners_of`]): for the
    /// launch mob, its members and those of the mobs its members created;
    /// for a mob the launch composes beside its launch mob, that mob's
    /// members. Every one this record knows, once the seed landed and, for
    /// such a mob, once its own records were read (until then the Steward
    /// dreams over nothing). It holds an observer of the mob, which keeps
    /// no mob open.
    pub(crate) fn steward_source(
        &self,
        handle: &meerkat_mob::MobHandle,
    ) -> Arc<dyn crate::memory::steward::PrivateMemberSource> {
        Arc::new(StewardPrivateMembers {
            owners: self.clone(),
            handle: handle.observer(),
        })
    }

    /// The owner a member spawned into mob `mob_id` takes, asked for by
    /// `spawner` (a member of that mob whose turn spawns), or copying the
    /// source of a prompt-context fork: the fork source's owner, else the
    /// spawner's. A spawner private to one person may not fork a member
    /// private to another.
    fn requested_owner(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &SpawnMemberSpec,
    ) -> Result<Option<String>, MobError> {
        let spawner_owner = spawner.and_then(|spawner| self.owner_of(mob_id, spawner.as_str()));
        let source = match &spec.launch_mode {
            MemberLaunchMode::Fork {
                source_member_id, ..
            } => Some(source_member_id),
            _ => None,
        };
        let source_owner = source.and_then(|source| self.owner_of(mob_id, source.as_str()));
        match (spawner_owner, source_owner) {
            (Some(spawner_owner), Some(source_owner)) if spawner_owner != source_owner => {
                Err(crossed_owners(source.map_or("", |source| source.as_str())))
            }
            (spawner_owner, source_owner) => Ok(source_owner.or(spawner_owner)),
        }
    }

    /// The person the member `spec` spawns in the runtime's own mob will be
    /// private to once [`Self::apply`] has run, or `None` for a member
    /// private to nobody: the owner [`Self::requested_owner`] names, else
    /// the owner `spec` carries where it is kept ([`keeps_carried_owner`]);
    /// a host's own spawn keeps what it carries.
    /// For a customizer that runs before the owner is written on the spec.
    pub(crate) fn owner_a_spawn_takes(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &SpawnMemberSpec,
    ) -> Result<Option<String>, MobError> {
        let carried = carried_owner(spec).filter(|_| keeps_carried_owner(spawner, spec));
        let forks = matches!(spec.launch_mode, MemberLaunchMode::Fork { .. });
        if spawner.is_none() && !forks {
            return Ok(carried);
        }
        Ok(self.requested_owner(mob_id, spawner, spec)?.or(carried))
    }

    /// The customizer body of the runtime's own mob, factored off the trait
    /// so tests can drive it. A spawn asked for by a member, `spawner` (the
    /// member whose turn spawns), or a prompt-context fork, carries no label
    /// only Meta writes of its own (every caller label path refused them),
    /// and takes the owner [`Self::requested_owner`] names. A member private
    /// to a person is seated exempt from the definition's wiring rules. A
    /// host's own spawn is left as it is.
    pub(crate) fn apply(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let forks = matches!(spec.launch_mode, MemberLaunchMode::Fork { .. });
        if spawner.is_none() && !forks {
            return Ok(());
        }
        let owner = self.owner_a_spawn_takes(mob_id, spawner, spec)?;
        give_owner(spec, owner);
        Ok(())
    }

    /// The customizer body of a mob a member created: the mob's own owner
    /// (its creator's), else the owner [`Self::requested_owner`] names. A
    /// mob this record does not know refuses the spawn as retryable: until
    /// the seed lands, a mob restored from before a restart is not known. A
    /// mob whose creation record does not say which member created it
    /// refuses the spawn for good. A rebuild of a member already seated
    /// there is not a spawn, and is not asked (see the customizer's
    /// `customize_resume`).
    pub(crate) fn apply_child(
        &self,
        mob_id: &str,
        spawner: Option<&meerkat_mob::AgentIdentity>,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let mob_owner = self.mob_owner(mob_id)?;
        let requested = self.requested_owner(mob_id, spawner, spec)?;
        let owner = match (mob_owner, requested) {
            (Some(mob_owner), Some(requested)) if mob_owner != requested => {
                return Err(crossed_owners(mob_id));
            }
            (mob_owner, requested) => mob_owner.or(requested),
        };
        let carried = carried_owner(spec).filter(|_| keeps_carried_owner(spawner, spec));
        give_owner(spec, owner.or(carried));
        Ok(())
    }
}

/// Who held a member's name, and when: what the journals of the runtime's
/// mobs say of it, as far as the seed has read them and the event pump has
/// shown them. A record of a moment is its asking member's, whoever holds
/// the name later.
impl PrivateOwners {
    /// [`Self::note_journal_step`] for a row of a journal that may be an
    /// earlier mob's: one a reading pass paged from the log of a mob it
    /// listed, or one a mob's event pump is shown. `admitted_before` is
    /// what [`Self::creates_admitted`] answered before the mob was listed,
    /// or before the pump subscribed. A row of a mob under whose id a
    /// create was admitted since is not noted, and the answer is `false`:
    /// the journal read may be an earlier mob's of the id, whose rows take
    /// the cursors the new mob's journal starts again from. The count is
    /// compared and the row noted under the dates lock, which the
    /// admission holds while it counts the create and forgets the id's
    /// journal ([`Self::forget_the_journal_of`]).
    fn note_a_journal_step_of(
        &self,
        step: &MemberJournalStep,
        admitted_before: &CreatesAdmitted,
    ) -> bool {
        let dates = self
            .inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if dates.admitted.get(&step.mob_id) != admitted_before.get(&step.mob_id) {
            return false;
        }
        self.note_journal_step(step);
        true
    }

    /// Whether no create was admitted under the mob id `mob_id` since
    /// `admitted_before`, what [`Self::creates_admitted`] answered then:
    /// the mob that held the id then still holds it.
    fn no_create_admitted_since(&self, mob_id: &str, admitted_before: &CreatesAdmitted) -> bool {
        self.inner
            .mob_dates
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .admitted
            .get(mob_id)
            == admitted_before.get(mob_id)
    }

    /// What the event pump of a mob does with a row of its journal it is
    /// shown: the row is noted, and where it removes a long-term agent the
    /// agent's end is followed ([`Self::private_agent_ended`]).
    /// `admitted_at_start` is what [`Self::creates_admitted`] answered
    /// before the pump subscribed: a row of a mob under whose id a create
    /// was admitted since is an earlier mob's, and is neither noted
    /// ([`Self::note_a_journal_step_of`]) nor followed. The member the new
    /// mob seated under the row's name is not the one the row is of.
    async fn follow_a_shown_journal_step(
        &self,
        step: &MemberJournalStep,
        ended_at: chrono::DateTime<chrono::Utc>,
        admitted_at_start: &CreatesAdmitted,
    ) {
        if !self.note_a_journal_step_of(step, admitted_at_start) {
            return;
        }
        if step.step == HolderStep::Removed {
            self.private_agent_ended(step, ended_at).await;
        }
    }

    /// Forget the journal rows noted under the mob id `mob_id`: who held
    /// each member name of that mob and when, and its rows' times. A mob
    /// created anew under an id starts its journal again at its first row
    /// (its store is new, and a store counts its rows from one), so the
    /// rows noted under the id are an earlier mob's, and a row of the new
    /// mob at a cursor one of them holds would not be noted. The admission
    /// of a create calls it under the dates lock, after its count.
    fn forget_the_journal_of(&self, mob_id: &str) {
        let mut journals = self
            .inner
            .journal_times
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut holders = self
            .inner
            .holders
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let had_rows = journals.remove(mob_id).is_some();
        let names = holders.len();
        holders.retain(|(mob, _), _| mob != mob_id);
        if had_rows || holders.len() != names {
            // What the record hands of who held a name changed.
            self.inner
                .holders_noted
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    /// Forget the members this record holds under the mob id `mob_id`:
    /// whom each is private to, as its build recorded it and as the mob's
    /// event log named it. A mob created under an id has no member yet, so
    /// a member held under the id is an earlier mob's: the mob created
    /// under the id is told nothing of it, and each of its own members is
    /// recorded by its build and by the read of its own log. The members of
    /// every other mob stay. The admission of a create calls it under the
    /// dates lock, after its count: a read of the earlier mob's log that
    /// ends afterwards keeps no row of that mob ([`Self::read_log`]).
    ///
    /// Every incarnation this record met under the id has ended with that:
    /// they are marked so under the lock a member build records under, and
    /// a build of one of them that reaches the record afterwards records
    /// nothing ([`Self::record_built`]).
    fn forget_the_members_of(&self, mob_id: &str) {
        let mut incarnations = self
            .inner
            .incarnations
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let (ended, met): (BTreeSet<_>, BTreeSet<_>) = std::mem::take(&mut incarnations.met)
            .into_iter()
            .partition(|(mob, _)| mob == mob_id);
        incarnations.met = met;
        incarnations.ended.extend(ended);
        incarnations.destroy_asked.remove(mob_id);
        for members in [&self.inner.built, &self.inner.logged] {
            members
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(mob, _), _| mob != mob_id);
        }
    }

    /// Note what one row of a mob's journal says of who holds a name. The
    /// row's time among its journal's rows ([`JournalTimes`]) is also the
    /// floor of the times this process stamps its gating entries and
    /// question sets with from here on
    /// ([`crate::runtime::raise_entry_time_floor`]): an entry written after
    /// the runtime was shown a seating is never stamped before it, and one
    /// written before the next seating is shown is stamped before that
    /// seating, also while the clock is behind the journal's newest time.
    fn note_journal_step(&self, step: &MemberJournalStep) {
        let mut journals = self
            .inner
            .journal_times
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let times = journals.entry(step.mob_id.clone()).or_default();
        let (at_ms, moved) = times.note(step.cursor, step.at_ms);
        crate::runtime::raise_entry_time_floor(
            times.entry_floor().unwrap_or(at_ms.saturating_add(1)),
        );
        let mut holders = self
            .inner
            .holders
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let new = holders
            .entry((
                step.mob_id.clone(),
                crate::member_comms_id::logical_memory_identity(&step.member),
            ))
            .or_default()
            .note(step.cursor, step.at_ms, at_ms, step.step.clone());
        if moved {
            for ((mob_id, _), of_name) in holders.iter_mut() {
                if *mob_id == step.mob_id {
                    of_name.restamp(times);
                }
            }
        }
        if new || moved {
            self.inner
                .holders_noted
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    /// How many journal rows the record has noted of who holds its members'
    /// names: [`Self::name_seatings`] answers the same while this does.
    pub(crate) fn name_seatings_noted(&self) -> u64 {
        self.inner
            .holders_noted
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// The members seated under each name over time, in every mob of the
    /// runtime whose journal the record has been shown: for each name (as a
    /// member's memory scope spells it), one list per mob, with the mob's
    /// id.
    pub(crate) fn name_seatings(
        &self,
    ) -> BTreeMap<String, Vec<(meerkat_mob::MobId, NameSeatings)>> {
        let mut names: BTreeMap<String, Vec<(meerkat_mob::MobId, NameSeatings)>> = BTreeMap::new();
        for ((mob_id, name), holders) in self
            .inner
            .holders
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            names.entry(name.clone()).or_default().push((
                meerkat_mob::MobId::from(mob_id.as_str()),
                holders.seatings(),
            ));
        }
        names
    }

    fn with_holders<Answer>(
        &self,
        mob_id: &str,
        name: &str,
        read: impl FnOnce(Option<&NameHolders>) -> Answer,
    ) -> Answer {
        let key = (
            mob_id.to_string(),
            crate::member_comms_id::logical_memory_identity(name),
        );
        read(
            self.inner
                .holders
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key),
        )
    }

    /// When the member that held `name` in mob `mob_id` at `at_ms` ended,
    /// if it was private to a person and has ended: an ephemeral member by
    /// its retirement, a long-term agent by the end of its identity entry.
    /// The member that held a name at a moment is the one last seated under
    /// it at or before that moment; a member private to a person holds every
    /// moment from its seating up to the next seating under the name, that
    /// moment included, whether or not it ended in between: what was
    /// recorded under a name nobody held is the member's that held it last.
    pub(crate) fn private_holder_ended(&self, mob_id: &str, name: &str, at_ms: u64) -> Option<u64> {
        self.with_holders(mob_id, name, |holders| {
            let holders = holders?;
            let index = holders.seating_at(at_ms)?;
            holders.owner_of(index)?;
            holders.ended(index)
        })
    }

    /// Whether the record knows a member private to a person that has
    /// ended.
    pub(crate) fn knows_an_ended_private_member(&self) -> bool {
        self.inner
            .holders
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|holders| {
                (0..holders.0.len()).any(|index| {
                    holders.owner_of(index).is_some() && holders.ended(index).is_some()
                })
            })
    }

    /// Whether the member that held `name` in mob `mob_id` at `at_ms` holds
    /// it now: `None` for a name the record knows no seating of (it sees no
    /// journal of that mob). A respawn, a reset and a rebuild seat the same
    /// member again; the end of the member, and a later seating private to
    /// another person or to no one, do not.
    pub(crate) fn still_held_by(&self, mob_id: &str, name: &str, at_ms: u64) -> Option<bool> {
        self.with_holders(mob_id, name, |holders| {
            holders.map(|holders| holders.held_since(at_ms))
        })
    }

    /// Whether a record made under `name` in mob `mob_id` at `at_ms` ends
    /// with the member private to a person whose end at `ended_at_ms` is
    /// being followed: the member that held the name then is private to a
    /// person, and either its end is in the journal or it is the member
    /// last seated under the name and was seated no later than that end
    /// (the end is then its own; a member seated after the end is a later
    /// one, and what it recorded stays). The record's time is read against
    /// the seatings only. `None` where no seating says who held the name
    /// then: a name the record knows no seating of, and a moment before the
    /// name's first known seating.
    pub(crate) fn ends_with_a_private_member(
        &self,
        mob_id: &str,
        name: &str,
        at_ms: u64,
        ended_at_ms: u64,
    ) -> Option<bool> {
        self.with_holders(mob_id, name, |holders| {
            let holders = holders?;
            holders.seating_at(at_ms)?;
            Some(
                holders.held_by_the_last_private_member(at_ms, ended_at_ms)
                    || holders.held_by_an_ended_private_member(at_ms),
            )
        })
    }

    /// Wait until every first read this record owes has landed: the seed
    /// has read the journals. A record that owes none does not wait.
    pub(crate) async fn journals_read(&self) {
        let mut reads = self.inner.first_reads.0.subscribe();
        let _ = reads
            .wait_for(|reads| reads.in_flight + reads.overdue == 0)
            .await;
    }

    /// Hand the end of a long-term agent private to a person to the
    /// follower of the question sets' seat: `step` is the journal's row that
    /// ends the agent's identity entry, recorded at `ended_at`. Its
    /// schedules are the identity family's to follow, so no other seat is
    /// handed it, and it is held for none: a set the end leaves open is
    /// withdrawn when the runtime next serves its members' questions.
    async fn private_agent_ended(
        &self,
        step: &MemberJournalStep,
        ended_at: chrono::DateTime<chrono::Utc>,
    ) {
        let private = self.with_holders(&step.mob_id, &step.member, |holders| {
            holders.is_some_and(|holders| {
                holders
                    .seating_before(step.cursor)
                    .is_some_and(|index| holders.owner_of(index).is_some())
            })
        });
        if !private {
            return;
        }
        let follower = self
            .inner
            .ends
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .seats
            .get(&MemberEndSeat::QuestionSets)
            .and_then(|seat| seat.follower.clone());
        if let Some(follower) = follower {
            follower
                .member_ended(&EndedPrivateMember {
                    mob_id: step.mob_id.clone(),
                    member: step.member.clone(),
                    session: None,
                    ended_at,
                })
                .await;
        }
    }
}

/// The private owner the labels of `spec` carry.
fn carried_owner(spec: &SpawnMemberSpec) -> Option<String> {
    spec.labels
        .as_ref()
        .and_then(crate::access::private_owner)
        .map(str::to_string)
}

/// Whether a spawn whose owner no record names keeps the private owner its
/// spec carries: a fork does (a fork Meerkat made copies its source's
/// labels: it holds that person's conversation), and so does a spawn no
/// member asked for (a host's own). A spawn a member asks for that is no
/// fork takes its owner from the record alone: no caller path hands such a
/// spawn an owner label, and one that did would not be believed.
fn keeps_carried_owner(
    spawner: Option<&meerkat_mob::AgentIdentity>,
    spec: &SpawnMemberSpec,
) -> bool {
    spawner.is_none() || matches!(spec.launch_mode, MemberLaunchMode::Fork { .. })
}

/// Strip the labels only Meta writes from `spec` and, for a member private
/// to `owner`, write the owner and seat it exempt from the definition's
/// wiring rules.
fn give_owner(spec: &mut SpawnMemberSpec, owner: Option<String>) {
    if let Some(labels) = spec.labels.as_mut() {
        labels.retain(|key, _| {
            !crate::member_comms_id::RESERVED_AUTHORITY_LABELS.contains(&key.as_str())
        });
    }
    if let Some(owner) = owner {
        spec.labels
            .get_or_insert_with(BTreeMap::new)
            .insert(crate::access::PRIVATE_TO_LABEL.to_string(), owner);
        spec.definition_wiring = meerkat_mob::DefinitionWiring::Exempt;
    }
}

/// The spawn on a mob of a member whose name lies under a key namespace
/// agent memory reserves is refused
/// ([`crate::member_comms_id::refuse_reserved_memory_name`]) wherever the
/// mob's spawn customizer is asked about a spawn that creates a member: a
/// host's own fresh spawn, a member's spawn or fork, a respawn under the
/// name, and a member's spawn with a resume launch, which seats a member on
/// an existing session (`spawner`, the member that asked for it, is what
/// tells that spawn from a rebuild).
///
/// A `SpawnSource::Resume` request that names no spawner is not asked. It
/// is the rebuild of a member already seated (a restart's restore, a wake,
/// an explicit resume; a fork-derived member's rebuild asks through
/// `customize_spawn` with that source too), which creates nothing, or a
/// host's own spawn on the mob handle with a resume launch, which the
/// request does not tell from such a rebuild. Core admits a resume only on
/// a session whose own durable records name the same mob and the same
/// member, so that spawn can only seat again, on its old session, a member
/// that was seated under the very name before.
fn admit_member_name(
    source: meerkat_mob::SpawnSource,
    spawner: Option<&meerkat_mob::ids::AgentIdentity>,
    spec: &SpawnMemberSpec,
) -> Result<(), MobError> {
    if matches!(source, meerkat_mob::SpawnSource::Resume) && spawner.is_none() {
        return Ok(());
    }
    crate::member_comms_id::refuse_reserved_memory_name(spec.identity.as_str())
        .map_err(MobError::WiringError)
}

/// [`PrivateOwners::spawn_customizer`].
struct PrivateOwnerPropagation {
    owners: PrivateOwners,
}

/// [`PrivateOwners::child_spawn_customizer`].
struct ChildOwnerPropagation {
    owners: PrivateOwners,
}

/// [`PrivateOwners::steward_source`].
struct StewardPrivateMembers {
    owners: PrivateOwners,
    handle: meerkat_mob::MobHandle,
}

#[async_trait::async_trait]
impl crate::memory::steward::PrivateMemberSource for StewardPrivateMembers {
    async fn private_identities(&self) -> Result<BTreeSet<String>, String> {
        let mob = self.handle.mob_id().as_str();
        // The seed reads the launch's own mob first, so only a source of
        // the launch's memory starts it over its handle. A source of a mob
        // with a memory of its own waits for the seed and for that mob's
        // own read.
        let read = match self.owners.memory_home_of(mob).view() {
            Some(MemoryHome::Hosted(_)) => {
                self.owners.landed() && self.owners.has_read_the_mob(mob)
            }
            Some(_) => self.owners.seeded(&self.handle).await.landed(),
            None => {
                return Err("the members of this mob are members of no memory".to_string());
            }
        };
        if !read {
            return Err(
                "who the members are private to has not been read yet; the next dream reads it"
                    .to_string(),
            );
        }
        Ok(self.owners.memory_owners_of(mob).into_keys().collect())
    }
}

/// [`PrivateOwners::member_tool_audience`]. Whom a member is private to is
/// said by its own roster row, the labels Meerkat hands in. This record of
/// the members the process built or read adds to that on the side that can
/// only hide more: for the target it is asked whenever the row names
/// nobody or there is no row, so a member the record knows as private
/// stays hidden; for the caller it is asked only when the caller has no
/// row, so a caller whose row names nobody is never taken for a person's
/// member on the record's word.
impl meerkat_mob::MemberToolAudience for PrivateOwners {
    fn hides(
        &self,
        mob_id: &meerkat_mob::MobId,
        caller: Option<&meerkat_mob::AgentIdentity>,
        caller_labels: Option<&BTreeMap<String, String>>,
        target: &meerkat_mob::AgentIdentity,
        target_labels: Option<&BTreeMap<String, String>>,
    ) -> bool {
        let named = |labels: &BTreeMap<String, String>| {
            crate::access::private_owner(labels).map(str::to_string)
        };
        let Some(owner) = target_labels
            .and_then(named)
            .or_else(|| self.owner_of(mob_id.as_str(), target.as_str()))
        else {
            return false;
        };
        let callers = caller.and_then(|caller| match caller_labels {
            Some(labels) => named(labels),
            None => self.owner_of(mob_id.as_str(), caller.as_str()),
        });
        callers != Some(owner)
    }
}

impl SpawnMemberCustomizer for PrivateOwnerPropagation {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        admit_member_name(ctx.spawn_source, ctx.spawner_identity.as_ref(), spec)?;
        self.owners
            .apply(ctx.mob_id.as_str(), ctx.spawner_identity.as_ref(), spec)
    }
}

impl SpawnMemberCustomizer for ChildOwnerPropagation {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        admit_member_name(ctx.spawn_source, ctx.spawner_identity.as_ref(), spec)?;
        self.owners
            .apply_child(ctx.mob_id.as_str(), ctx.spawner_identity.as_ref(), spec)
    }

    /// A rebuild of a member already seated in the mob (a restart's restore,
    /// a wake, an explicit resume) repeats its seating: the roster labels it
    /// is rebuilt with are the ones [`PrivateOwners::apply_child`] gave its
    /// spawn, owner included, and its seat keeps its wiring. Nothing is
    /// decided again, so a rebuild is never refused, even before the seed
    /// lands.
    fn customize_resume(
        &self,
        _ctx: &SpawnCustomizationContext,
        _durable: &meerkat_mob::ResumedMemberView,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const OWNER: &str = "console-caller:v1:aa";

    fn private_labels() -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            OWNER.to_string(),
        )])
    }

    /// A creation record names its creating member exactly when one
    /// member's session made the create: the create a host made for it
    /// (`Member`) and the member's own `mob_create` (`OperatorMember`) both
    /// answer the member and the mob it is seated in. A record of the host,
    /// of a session no roster named (a host's create for it, or its own), or
    /// of a store from before creations were recorded names none. A record
    /// of a session several rosters named is not decidable.
    #[test]
    fn a_creation_record_names_its_creating_member() {
        use meerkat_mob::{AgentIdentity, Generation, MobId, MobProvenance};

        let parent = MobId::from("parent-mob");
        let creator = AgentIdentity::from("creator");
        let session = meerkat_core::SessionId::new();
        let incarnation = meerkat_core::service::MobIncarnationId::mint();
        let names_the_creator = [
            MobProvenance::Member {
                parent_mob_id: parent.clone(),
                parent_incarnation: incarnation,
                creator_identity: creator.clone(),
                creator_generation: Generation::INITIAL,
                owner_session: session.clone(),
            },
            MobProvenance::OperatorMember {
                parent_mob_id: parent.clone(),
                parent_incarnation: incarnation,
                creator_identity: creator.clone(),
                creator_generation: Generation::INITIAL,
                owner_session: session.clone(),
            },
        ];
        for provenance in &names_the_creator {
            assert_eq!(
                creating_member(provenance),
                CreatingMember::Named(&parent, &creator),
                "{provenance:?}"
            );
        }
        let names_no_member = [
            MobProvenance::Host,
            MobProvenance::Session {
                session_id: session.clone(),
            },
            MobProvenance::OperatorSession {
                session_id: session,
            },
            MobProvenance::Unrecorded,
        ];
        for provenance in &names_no_member {
            assert_eq!(
                creating_member(provenance),
                CreatingMember::NoMember,
                "{provenance:?}"
            );
        }
        assert_eq!(
            creating_member(&MobProvenance::Ambiguous {
                matching_rosters: 2,
            }),
            CreatingMember::Undecidable,
            "a record of several rosters does not say which member created the mob"
        );
    }

    /// The audience of the members' own operator tools reads whom a member
    /// is private to from the roster labels it is handed, with a record
    /// that knows no member: a member whose row names a person is hidden
    /// from a caller whose row names nobody, from one whose row names
    /// another person and from a call no member made, and not from a caller
    /// whose row names the same person; a member whose row names nobody is
    /// hidden from no one. The record only adds: a member it knows as
    /// private stays hidden when its row names nobody, or it has no row.
    #[test]
    fn the_member_tool_audience_reads_the_owner_from_the_roster_labels() {
        use meerkat_mob::MemberToolAudience as _;
        let mob = meerkat_mob::MobId::from("mob");
        let member = |name: &str| meerkat_mob::AgentIdentity::from(name);
        let private = private_labels();
        let shared = BTreeMap::new();
        let other = BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "console-caller:v1:bb".to_string(),
        )]);

        let unread = PrivateOwners::default();
        let hides = |caller: Option<&BTreeMap<String, String>>,
                     target: Option<&BTreeMap<String, String>>| {
            unread.hides(
                &mob,
                caller.map(|_| member("caller")).as_ref(),
                caller,
                &member("target"),
                target,
            )
        };
        assert!(hides(Some(&shared), Some(&private)), "a shared caller");
        assert!(hides(Some(&other), Some(&private)), "another person's");
        assert!(hides(None, Some(&private)), "a call no member made");
        assert!(!hides(Some(&private), Some(&private)), "the same person's");
        assert!(!hides(Some(&shared), Some(&shared)), "a shared target");
        assert!(!hides(Some(&private), Some(&shared)), "a shared target");
        assert!(!hides(Some(&shared), None), "a target with no row");

        let knows = PrivateOwners::default();
        knows.record("mob", "target", Some(&private));
        knows.record("mob", "caller", Some(&private));
        for target in [Some(&shared), None] {
            assert!(
                knows.hides(
                    &mob,
                    Some(&member("other")),
                    Some(&shared),
                    &member("target"),
                    target
                ),
                "the record adds: a member it knows as private stays hidden"
            );
        }
        assert!(
            !knows.hides(
                &mob,
                Some(&member("caller")),
                None,
                &member("target"),
                Some(&shared)
            ),
            "the record speaks for a caller that has no row"
        );
        assert!(
            knows.hides(
                &mob,
                Some(&member("caller")),
                Some(&shared),
                &member("target"),
                Some(&private)
            ),
            "a caller whose row names nobody is no person's member on the record's word"
        );
    }

    /// A spawn a private member asks for gets its owner and keeps no label
    /// only Meta writes of its own; a fork of a private member is private
    /// too; a spawn of a shared member, and a host's spawn, get none.
    #[test]
    fn a_private_members_spawns_and_forks_get_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        let spawner = meerkat_mob::AgentIdentity::from("side");

        let mut spawned = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h1"))
            .with_labels(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "alpha".to_string(),
                ),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "console-caller:v1:bb".to_string(),
                ),
            ]));
        owners
            .apply("mob", Some(&spawner), &mut spawned)
            .expect("a spawn of a private member");
        assert_eq!(
            spawned.labels,
            Some(BTreeMap::from([
                ("team".to_string(), "blue".to_string()),
                (
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    OWNER.to_string()
                ),
            ]))
        );

        let mut forked = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h2"));
        forked.launch_mode = MemberLaunchMode::Fork {
            source_member_id: meerkat_mob::AgentIdentity::from("side"),
            fork_context: meerkat_mob::launch::ForkContext::default(),
        };
        owners
            .apply("mob", None, &mut forked)
            .expect("a fork of a private member");
        assert_eq!(forked.labels, Some(private_labels()));

        let mut of_shared = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h3"));
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut of_shared,
            )
            .expect("a spawn of a shared member");
        assert_eq!(of_shared.labels, None);

        let mut hosts = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("h4"))
            .with_labels(private_labels());
        owners
            .apply("mob", None, &mut hosts)
            .expect("a host's spawn");
        assert_eq!(
            hosts.labels,
            Some(private_labels()),
            "a host's own spawn is left as it is"
        );
        assert_eq!(owners.owner_of("other-mob", "side"), None);
    }

    /// A spawn the customizer is asked about that creates a member is
    /// refused a name under a key namespace agent memory reserves: a host's
    /// fresh spawn, a member's spawn, a fork, a respawn under the name, with
    /// and without a member that asked for it, and a spawn with a resume
    /// launch that a member asked for. A resume request that names no
    /// member that asked for it (the rebuild of a member already seated, or
    /// a host's own resume spawn) is not refused. A name that holds a `/`
    /// outside those namespaces is admitted. This row tests the function,
    /// not which requests Core sends it.
    #[test]
    fn a_member_is_created_under_no_reserved_memory_name_and_a_rebuild_is_not_refused() {
        use meerkat_mob::SpawnSource;
        let named = |name: &str| {
            SpawnMemberSpec::new("worker", crate::member_comms_id::mob_member_id(name))
        };
        let reserved = [
            "private/console-caller:v1:aa/ledger-side",
            "set-aside/ledger-side",
        ];
        let asking = meerkat_mob::ids::AgentIdentity::from("alpha");
        for source in [
            SpawnSource::Consumer,
            SpawnSource::AgentSpawnMember,
            SpawnSource::HelperSpawn,
            SpawnSource::BatchItem,
            SpawnSource::FlowProvisioning,
            SpawnSource::PolicySpawn,
            SpawnSource::Respawn,
            SpawnSource::Fork,
            SpawnSource::PersistedForkResume,
        ] {
            for spawner in [None, Some(&asking)] {
                for name in reserved {
                    assert!(
                        matches!(
                            admit_member_name(source, spawner, &named(name)),
                            Err(MobError::WiringError(_))
                        ),
                        "{source:?} under '{name}' is refused"
                    );
                }
                for name in ["ledger-side", "team/ledger-side", "privateer/ledger-side"] {
                    assert!(
                        admit_member_name(source, spawner, &named(name)).is_ok(),
                        "{source:?} under '{name}' is admitted"
                    );
                }
            }
        }
        for name in reserved {
            assert!(
                matches!(
                    admit_member_name(SpawnSource::Resume, Some(&asking), &named(name)),
                    Err(MobError::WiringError(_))
                ),
                "a resume spawn a member asked for under '{name}' is refused"
            );
            assert!(
                admit_member_name(SpawnSource::Resume, None, &named(name)).is_ok(),
                "a resume request under '{name}' that no member asked for is not refused"
            );
        }
        assert!(
            admit_member_name(SpawnSource::Resume, Some(&asking), &named("ledger-side")).is_ok(),
            "a resume spawn a member asked for under a plain name is admitted"
        );
    }

    /// A console decides a nested mob's member by its mob's owner
    /// ([`PrivateOwners::give_mob_owner`]), on each answer the record can
    /// hold. The person a mob is private to owns a member with no owner of
    /// its own, and a mob private to no one leaves its member as it is.
    /// Every answer that is not certain (nothing held, a creating member of
    /// unknown owner, an undecidable creation record) withholds the member
    /// from everyone but an admin, and so does the listing's own word that
    /// the creation record is undecided, whatever the record holds. A
    /// member with an owner of its own keeps it on every answer.
    #[test]
    fn a_nested_mobs_member_takes_its_mobs_owner_on_every_answer_of_the_record() {
        const OTHER: &str = "console-caller:v1:bb";
        let given = |owners: &PrivateOwners, undecided: bool, own: Option<&str>| {
            let mut labels = BTreeMap::new();
            if let Some(own) = own {
                labels.insert(crate::access::PRIVATE_TO_LABEL.to_string(), own.to_string());
            }
            owners.give_mob_owner("crew", undecided, &mut labels);
            crate::access::private_owner(&labels).map(str::to_string)
        };
        let withheld = Some(crate::access::UNDECIDED_PRIVATE_OWNER.to_string());

        let owners = PrivateOwners::default();
        assert_eq!(given(&owners, false, None), withheld, "nothing held");
        owners.record_created_mob(
            "crew",
            MobOwner::Known(Some(OWNER.to_string())),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(
            given(&owners, false, None),
            Some(OWNER.to_string()),
            "a person's mob"
        );
        assert_eq!(
            given(&owners, true, None),
            withheld,
            "the listing says the creation record is undecided"
        );
        owners.record_created_mob(
            "crew",
            MobOwner::CreatorUnknown,
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(
            given(&owners, false, None),
            withheld,
            "a creating member of unknown owner"
        );
        owners.record_created_mob("crew", MobOwner::Undecidable, MobCreation::Undecidable);
        assert_eq!(
            given(&owners, false, None),
            withheld,
            "an undecidable creation record"
        );
        owners.record_created_mob(
            "crew",
            MobOwner::Known(None),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(given(&owners, false, None), None, "a mob private to no one");

        // A member with an owner of its own keeps it, whatever its mob's.
        let own = Some(OTHER.to_string());
        assert_eq!(given(&owners, false, Some(OTHER)), own, "a shared mob");
        assert_eq!(
            given(&owners, true, Some(OTHER)),
            own,
            "an undecided creation record"
        );
        owners.record_created_mob(
            "crew",
            MobOwner::Known(Some(OWNER.to_string())),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(
            given(&owners, false, Some(OTHER)),
            own,
            "another person's mob"
        );
        owners.record_created_mob(
            "crew",
            MobOwner::CreatorUnknown,
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(
            given(&owners, false, Some(OTHER)),
            own,
            "a creating member of unknown owner"
        );
        owners.forget_mob_in_tests("crew");
        assert_eq!(given(&owners, false, Some(OTHER)), own, "nothing held");
    }

    /// Who a member of a nested mob that has left its roster is private to:
    /// its own owner on every answer of its mob; else the person its mob is
    /// private to; shared in a mob private to no one; and not known for
    /// every answer of its mob that is not certain (nothing held, a
    /// creating member of unknown owner, an undecidable creation record).
    /// A mob read while its creating member's owner was not known is no
    /// answer yet: what a later read says takes its place, and the first
    /// certain record of a mob stands.
    #[test]
    fn an_ended_nested_member_is_a_persons_shared_or_not_known() {
        const OTHER: &str = "console-caller:v1:bb";
        let person = |owner: &str| EndedMemberOwner::Person(owner.to_string());
        let owners = PrivateOwners::default();
        // A member with an owner of its own, and one known as private to no
        // one: both built in the mob. A create under the id forgets the
        // members held under it, so the two are built again after each
        // create the row admits, as the members of a mob are.
        let built = || {
            owners.record("crew", "own", Some(&private_labels()));
            owners.record("crew", "aide", None);
        };
        built();
        let read = |name: &str| owners.ended_nested_member_owner("crew", name);

        assert_eq!(read("aide"), EndedMemberOwner::Unknown, "nothing held");
        assert_eq!(
            read("stranger"),
            EndedMemberOwner::Unknown,
            "a member the record does not know, nothing held"
        );
        assert_eq!(read("own"), person(OWNER), "its own owner, nothing held");

        owners.record_created_mob(
            "crew",
            MobOwner::Known(Some(OTHER.to_string())),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        built();
        assert_eq!(read("aide"), person(OTHER), "a person's mob");
        assert_eq!(read("stranger"), person(OTHER), "a person's mob");
        assert_eq!(
            read("own"),
            person(OWNER),
            "its own owner in a person's mob"
        );

        owners.record_created_mob(
            "crew",
            MobOwner::Known(None),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        built();
        assert_eq!(read("aide"), EndedMemberOwner::Shared, "a shared mob");
        assert_eq!(read("own"), person(OWNER), "its own owner in a shared mob");

        // A creating member of unknown owner says nothing of the mob, and a
        // later read of the mob takes its place; a certain record stands.
        owners.record_created_mob(
            "crew",
            MobOwner::CreatorUnknown,
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        built();
        assert_eq!(
            read("aide"),
            EndedMemberOwner::Unknown,
            "a creating member of unknown owner"
        );
        assert!(
            owners.mob_owner("crew").is_err(),
            "a spawn into such a mob is refused"
        );
        owners.record_mob("crew", MobOwner::Known(Some(OTHER.to_string())));
        assert_eq!(
            read("aide"),
            person(OTHER),
            "read again once its creating member's owner is known"
        );
        owners.record_mob("crew", MobOwner::Known(None));
        assert_eq!(
            read("aide"),
            person(OTHER),
            "the first certain record of a mob stands"
        );
        owners.record_mob("crew", MobOwner::CreatorUnknown);
        assert_eq!(
            read("aide"),
            person(OTHER),
            "a later read that cannot say does not take a certain record's place"
        );

        owners.record_created_mob("crew", MobOwner::Undecidable, MobCreation::Undecidable);
        built();
        assert_eq!(
            read("aide"),
            EndedMemberOwner::Unknown,
            "an undecidable creation record"
        );
        assert_eq!(read("own"), person(OWNER), "its own owner on every answer");

        // What the console records for each answer.
        assert_eq!(person(OWNER).recorded_owner().as_deref(), Some(OWNER));
        assert_eq!(EndedMemberOwner::Shared.recorded_owner(), None);
        assert_eq!(
            EndedMemberOwner::Unknown.recorded_owner().as_deref(),
            Some(crate::access::UNDECIDED_PRIVATE_OWNER)
        );

        // The members of the nested mobs the record knows, a member of the
        // runtime's own mob left out.
        owners.record("primary", "lead", None);
        let nested = owners.nested_members("primary");
        let names: Vec<(&str, &str)> = nested
            .iter()
            .map(|(mob, name, _)| (mob.as_str(), name.as_str()))
            .collect();
        assert_eq!(names, vec![("crew", "aide"), ("crew", "own")]);
        assert_eq!(nested[1].2, person(OWNER));
    }

    /// A create under an id forgets the members this record holds under
    /// that id, as their builds recorded them and as the mob's log named
    /// them: the mob created under the id is told nothing of them, and a
    /// member of it is what its own build records. A member of another mob
    /// that stands, private to a person or to no one, keeps what the
    /// record holds of it, and so do the launch mob's members.
    #[test]
    fn a_create_under_an_id_forgets_the_members_held_under_it_and_no_other_mobs() {
        use crate::mob_announcements::MemberPerson;
        const OTHER: &str = "console-caller:v1:bb";
        let private_to = |person: &str| {
            BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            )])
        };
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");
        // What builds recorded.
        owners.record("second", "desk", Some(&private_to(OWNER)));
        owners.record("second", "clerk", None);
        owners.record("third", "scout", Some(&private_to(OTHER)));
        owners.record("third", "porter", None);
        owners.record("launch", "ledger", Some(&private_to(OWNER)));
        // What the logs named, of members no build recorded.
        owners
            .inner
            .logged
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .extend([
                (
                    ("second".to_string(), "courier".to_string()),
                    Some(OWNER.to_string()),
                ),
                (
                    ("third".to_string(), "runner".to_string()),
                    Some(OTHER.to_string()),
                ),
                (
                    ("launch".to_string(), "usher".to_string()),
                    Some(OWNER.to_string()),
                ),
            ]);
        let held = |mob: &str, member: &str| owners.person_of(mob, member);
        let private = |person: &str| MemberPerson::Private(person.to_string());
        let of_the_others = || {
            [
                held("third", "scout"),
                held("third", "porter"),
                held("third", "runner"),
                held("launch", "ledger"),
                held("launch", "usher"),
            ]
        };
        let kept = [
            private(OTHER),
            MemberPerson::Shared,
            private(OTHER),
            private(OWNER),
            private(OWNER),
        ];
        assert_eq!(
            [
                held("second", "desk"),
                held("second", "clerk"),
                held("second", "courier"),
            ],
            [private(OWNER), MemberPerson::Shared, private(OWNER)],
            "the scene: the members held under the id"
        );
        assert_eq!(of_the_others(), kept, "the scene: the other mobs' members");

        owners.record_created_mob("second", MobOwner::Known(None), MobCreation::ByNoMember);
        assert_eq!(
            [
                held("second", "desk"),
                held("second", "clerk"),
                held("second", "courier"),
            ],
            [
                MemberPerson::NotKnown,
                MemberPerson::NotKnown,
                MemberPerson::NotKnown,
            ],
            "the mob created under the id is told nothing of an earlier mob's members"
        );
        assert_eq!(
            of_the_others(),
            kept,
            "a member of another mob and of the launch mob keeps what the record holds of it"
        );

        // A member of the new mob is what its own build records.
        owners.record("second", "desk", None);
        assert_eq!(held("second", "desk"), MemberPerson::Shared);
    }

    /// The members of the mobs nested under a mob are the members of the
    /// mobs made under it, by the recorded creations, and of no other mob.
    /// A launch mob has a member, a mob its member made and a mob a member
    /// of that mob made. The launch composes two mobs beside it, each with a
    /// member; a member of one made a mob, and a member of that mob made
    /// another; a member of the other made a mob too. Three more mobs have
    /// no creating member the record can read: one created by no member,
    /// one whose record does not say, and one the record holds no creation
    /// of. Asked for the launch mob, the record answers the members of the
    /// mobs made under it, at either depth, and of the three mobs it cannot
    /// attribute; asked for a mob composed beside it, the members of the
    /// mobs made under that mob alone. No answer holds a member of the mob
    /// asked for, of a mob composed beside the launch mob, or of a mob made
    /// under another one.
    #[test]
    fn the_nested_members_of_a_mob_are_the_members_of_the_mobs_made_under_it() {
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");
        let hosted = owners.hosted_mobs();
        hosted.name("hosted");
        hosted.name("other-hosted");
        for (mob, member, creating) in [
            ("launch", "lead", None),
            ("crew", "aide", Some("launch")),
            ("crew-of-crew", "deep", Some("crew")),
            ("hosted", "hosted-lead", None),
            ("hosted-crew", "hosted-aide", Some("hosted")),
            ("hosted-crew-of-crew", "hosted-deep", Some("hosted-crew")),
            ("other-hosted", "other-lead", None),
            ("other-crew", "other-aide", Some("other-hosted")),
        ] {
            owners.record(mob, member, None);
            if let Some(creating) = creating {
                owners.hold_the_creation_in_tests(mob, Some(creating));
            }
        }
        owners.record("made-by-no-member", "stray", None);
        owners.hold_the_creation_in_tests("made-by-no-member", None);
        owners.record("undecided", "undecided-aide", None);
        owners.record_creation("undecided", MobCreation::Undecidable);
        owners.record("unread", "unread-aide", None);

        let nested = |primary: &str| -> Vec<(String, String)> {
            owners
                .nested_members(primary)
                .into_iter()
                .map(|(mob, name, _)| (mob, name))
                .collect()
        };
        let named = |members: &[(&str, &str)]| -> Vec<(String, String)> {
            members
                .iter()
                .map(|(mob, name)| ((*mob).to_string(), (*name).to_string()))
                .collect()
        };
        assert_eq!(
            nested("launch"),
            named(&[
                ("crew", "aide"),
                ("crew-of-crew", "deep"),
                ("made-by-no-member", "stray"),
                ("undecided", "undecided-aide"),
                ("unread", "unread-aide"),
            ]),
            "under the launch mob"
        );
        assert_eq!(
            nested("hosted"),
            named(&[
                ("hosted-crew", "hosted-aide"),
                ("hosted-crew-of-crew", "hosted-deep"),
            ]),
            "under a mob composed beside the launch mob"
        );
        assert_eq!(
            nested("other-hosted"),
            named(&[("other-crew", "other-aide")]),
            "under the other mob composed beside the launch mob"
        );
    }

    /// A mob the host created that is stored and not loaded still ends the
    /// line of the mobs made under it, on a launch that composes the mobs
    /// its host creates. The record read such a mob's creation from the mob
    /// set (by the host) and the launch's composition has not named it: it
    /// is not built in this process. A mob its member made is that mob's:
    /// the launch mob is not answered its members, and the mob itself is
    /// once it is loaded.
    /// On a launch that does not compose its host's mobs nothing names such
    /// a mob at any load, and the line stays the launch mob's.
    #[test]
    fn a_mob_made_under_a_hosts_mob_that_is_not_loaded_is_that_mobs() {
        let record = |composes: bool| {
            let owners = PrivateOwners::default();
            owners.bind_launch_mob("launch");
            if composes {
                owners.the_launch_composes_its_hosts_mobs();
            }
            owners.record("launch", "lead", None);
            // The host's mob, as the mob set's record of a mob it has not
            // loaded says it; no build named it.
            owners.record_creation(
                "host-made",
                MobCreation::of(&meerkat_mob::MobProvenance::Host),
            );
            owners.record("crew", "aide", None);
            owners.hold_the_creation_in_tests("crew", Some("host-made"));
            owners.record("crew-of-crew", "deep", None);
            owners.hold_the_creation_in_tests("crew-of-crew", Some("crew"));
            owners
        };
        let nested = |owners: &PrivateOwners, primary: &str| -> Vec<(String, String)> {
            owners
                .nested_members(primary)
                .into_iter()
                .map(|(mob, name, _)| (mob, name))
                .collect()
        };
        let both: Vec<(String, String)> = [("crew", "aide"), ("crew-of-crew", "deep")]
            .iter()
            .map(|(mob, name)| ((*mob).to_string(), (*name).to_string()))
            .collect();

        let composing = record(true);
        assert_eq!(
            nested(&composing, "launch"),
            Vec::new(),
            "the launch mob of a launch that composes its host's mobs"
        );
        // The host's mob is loaded: its build names it, and it is answered
        // the mobs made under it.
        composing.hosted_mobs().name("host-made");
        assert_eq!(
            nested(&composing, "host-made"),
            both,
            "the host's mob once it is loaded"
        );
        assert_eq!(
            nested(&composing, "launch"),
            Vec::new(),
            "the launch mob once the host's mob is loaded"
        );
        let not_composing = record(false);
        assert_eq!(
            nested(&not_composing, "launch"),
            both,
            "the launch mob of a launch that composes no mob of its host"
        );
    }

    /// A spawn a member asks for that is no fork takes its owner from this
    /// record alone: an owner label its spec carries is not kept, in the
    /// runtime's own mob and in a mob a member created. A fork keeps the
    /// owner its spec carries (its source's), and a host's own spawn is left
    /// as it is.
    #[test]
    fn a_spawn_that_is_no_fork_keeps_no_owner_label_it_carries() {
        let owners = PrivateOwners::default();
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        owners.record_mob("child-mob", MobOwner::Known(None));
        let shared = meerkat_mob::AgentIdentity::from("shared");
        let carrying = || {
            SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from("helper"))
                .with_labels(private_labels())
        };
        let owner_of = |spec: &SpawnMemberSpec| {
            spec.labels
                .as_ref()
                .and_then(crate::access::private_owner)
                .map(str::to_string)
        };

        let mut spawned = carrying();
        owners
            .apply("mob", Some(&shared), &mut spawned)
            .expect("a spawn of a shared member");
        assert_eq!(
            owner_of(&spawned),
            None,
            "a shared member's spawn is private to nobody, whatever label it carries"
        );
        let mut spawned = carrying();
        owners
            .apply_child("child-mob", Some(&shared), &mut spawned)
            .expect("a spawn of a shared member in a mob a member created");
        assert_eq!(
            owner_of(&spawned),
            None,
            "and so in a mob a member created that is private to nobody"
        );

        let mut forked = carrying();
        forked.launch_mode = MemberLaunchMode::Fork {
            source_member_id: meerkat_mob::AgentIdentity::from("unrecorded-source"),
            fork_context: meerkat_mob::launch::ForkContext::default(),
        };
        owners
            .apply("mob", Some(&shared), &mut forked)
            .expect("a fork");
        assert_eq!(
            owner_of(&forked),
            Some(OWNER.to_string()),
            "a fork keeps the owner its source's labels carry"
        );
        for mob in ["mob", "child-mob"] {
            let mut hosts = carrying();
            let applied = if mob == "mob" {
                owners.apply(mob, None, &mut hosts)
            } else {
                owners.apply_child(mob, None, &mut hosts)
            };
            applied.expect("a host's spawn");
            assert_eq!(
                owner_of(&hosts),
                Some(OWNER.to_string()),
                "a spawn no member asked for is left as it is in {mob}"
            );
        }
    }

    /// The memory owners name each private member by its logical memory
    /// identity, and the newest build of a member wins over the log.
    #[test]
    fn memory_owners_follow_the_newest_record() {
        let owners = PrivateOwners::default();
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "retired".to_string()),
            Some(OWNER.to_string()),
        );
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "rebuilt".to_string()),
            Some(OWNER.to_string()),
        );
        owners.record("mob", "rebuilt", Some(&BTreeMap::new()));
        owners.record("mob", "live", Some(&private_labels()));
        assert_eq!(
            owners.memory_owners(),
            BTreeMap::from([
                ("live".to_string(), OWNER.to_string()),
                ("retired".to_string(), OWNER.to_string()),
            ])
        );
    }

    /// The record tells a member private to a person, a member private to
    /// no one and a member it does not hold apart, for a member that ended
    /// too; the newest build of a member wins over the log, and a member
    /// only the memory store's private retirements name is private to a
    /// person nobody is.
    #[test]
    fn the_record_tells_private_shared_and_not_known_apart() {
        use crate::mob_announcements::MemberPerson;
        let owners = PrivateOwners::default();
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "ended-private".to_string()),
            Some(OWNER.to_string()),
        );
        owners
            .inner
            .logged
            .write()
            .unwrap()
            .insert(("mob".to_string(), "ended-shared".to_string()), None);
        owners.inner.logged.write().unwrap().insert(
            ("mob".to_string(), "rebuilt".to_string()),
            Some(OWNER.to_string()),
        );
        owners.record("mob", "rebuilt", Some(&BTreeMap::new()));
        owners
            .inner
            .retired
            .write()
            .unwrap()
            .entry(MemoryHome::Launch)
            .or_default()
            .insert("retired-private".to_string());
        assert_eq!(
            owners.person_of("mob", "ended-private"),
            MemberPerson::Private(OWNER.to_string())
        );
        assert_eq!(
            owners.person_of("mob", "ended-shared"),
            MemberPerson::Shared
        );
        assert_eq!(owners.person_of("mob", "rebuilt"), MemberPerson::Shared);
        assert_eq!(
            owners.person_of("mob", "retired-private"),
            MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string())
        );
        assert_eq!(owners.person_of("mob", "nobody"), MemberPerson::NotKnown);
        assert_eq!(
            owners.person_of("other-mob", "ended-private"),
            MemberPerson::NotKnown
        );
    }
    /// A log whose first read fails and whose later reads name one private
    /// member's spawn.
    #[derive(Default)]
    struct FlakyLog {
        reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MemberSpawnLog for FlakyLog {
        async fn spawns_after(
            &self,
            cursor: u64,
            _limit: usize,
        ) -> Result<Option<MemberSpawnPage>, String> {
            if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("the event store is unavailable".to_string());
            }
            if cursor > 0 {
                return Ok(None);
            }
            Ok(Some((
                vec![MemberJournalStep::seated(
                    "mob",
                    "side",
                    &private_labels(),
                    true,
                    1,
                    0,
                )],
                1,
            )))
        }
    }

    /// A seed whose read of the event log failed is read again by the next
    /// caller, so a transient store error does not leave the owners of the
    /// members this process did not build unknown for its life.
    #[tokio::test]
    async fn the_seed_reads_the_log_again_after_a_failed_read() {
        let owners = PrivateOwners::default();
        let log = FlakyLog::default();
        owners.seeded(&log).await;
        owners.seeded(&log).await;
        assert_eq!(
            owners.owner_of("mob", "side").as_deref(),
            Some(OWNER),
            "a later seed reads the log again"
        );
    }

    /// A record owes its first read from the moment anything starts its seed
    /// until a read succeeds; one nothing reads owes none. Nobody waits for
    /// a read that has failed.
    #[tokio::test]
    async fn a_record_owes_its_first_read_until_a_read_succeeds() {
        let owners = PrivateOwners::default();
        assert!(
            !owners.first_read_pending(),
            "a record nothing reads owes no read"
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            owners.first_attempts_settled(std::time::Duration::from_hours(1)),
        )
        .await
        .expect("nothing to wait for");

        let log = FlakyLog::default();
        owners.seeded(&log).await;
        assert!(
            !owners.landed() && owners.first_read_pending(),
            "a first read that failed is still owed"
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            owners.first_attempts_settled(std::time::Duration::from_hours(1)),
        )
        .await
        .expect("nobody waits for a read that failed");
        owners.seeded(&log).await;
        assert!(owners.landed() && !owners.first_read_pending());
    }

    /// A log whose read waits until its gate opens, then holds nothing.
    struct HeldLog(tokio::sync::watch::Receiver<bool>);

    #[async_trait::async_trait]
    impl MemberSpawnLog for HeldLog {
        async fn spawns_after(
            &self,
            _cursor: u64,
            _limit: usize,
        ) -> Result<Option<MemberSpawnPage>, String> {
            let mut gate = self.0.clone();
            let _ = gate.wait_for(|open| *open).await;
            Ok(None)
        }
    }

    /// The seed of `owners` started as a background read behind a gate;
    /// returns once the read is owed.
    async fn read_in_flight(
        owners: &PrivateOwners,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let (open, gate) = tokio::sync::watch::channel(false);
        let reading = owners.clone();
        let task = tokio::spawn(async move {
            let log = HeldLog(gate);
            reading.seeded(&log).await;
        });
        while !owners.first_read_pending() {
            tokio::task::yield_now().await;
        }
        (open, task)
    }

    /// Readers wait for a first read that is on its first attempt and go on
    /// when it lands. A reader's wait ends at its bound; the read is overdue
    /// from then on, so no later reader waits for it, and it stays owed
    /// until it succeeds.
    #[tokio::test]
    async fn readers_wait_a_bounded_time_for_a_first_read_in_flight() {
        // The read lands while readers wait: each goes on, nothing is owed.
        let owners = PrivateOwners::default();
        let (open, task) = read_in_flight(&owners).await;
        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let owners = owners.clone();
                tokio::spawn(async move {
                    owners
                        .first_attempts_settled(std::time::Duration::from_hours(1))
                        .await;
                    owners.first_read_pending()
                })
            })
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
        open.send(true).expect("the read is waiting");
        task.await.expect("the read ends");
        for waiter in waiters {
            assert!(
                !waiter.await.expect("the reader goes on"),
                "a reader that waited finds the read landed"
            );
        }

        // The read does not return: a reader's wait ends at its bound.
        let owners = PrivateOwners::default();
        let (open, task) = read_in_flight(&owners).await;
        let bound = std::time::Duration::from_millis(200);
        let asked = std::time::Instant::now();
        owners.first_attempts_settled(bound).await;
        assert!(asked.elapsed() >= bound, "the reader waited its bound");
        assert!(owners.first_read_pending(), "the read is still owed");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            owners.first_attempts_settled(std::time::Duration::from_hours(1)),
        )
        .await
        .expect("nobody waits for an overdue read");
        open.send(true).expect("the read is waiting");
        task.await.expect("the read ends");
        assert!(owners.landed() && !owners.first_read_pending());
    }

    /// A member the record does not hold is private to no one for a record
    /// nothing reads, as before, and not known once the host said it is
    /// about to build the member's mob: until that mob is read. The mark is
    /// the mob's own: another mob is not touched, the record's first reads
    /// are not touched, and what a build recorded stays what it was. A wait
    /// for the mob's read lasts its bound; a wait for a mob nothing is owed
    /// for returns at once.
    #[tokio::test]
    async fn a_member_the_record_does_not_hold_is_not_known_while_its_mobs_read_is_owed() {
        use crate::mob_announcements::MemberPerson;
        let owners = PrivateOwners::default();
        assert_eq!(owners.person_of("mob", "nobody"), MemberPerson::NotKnown);
        assert_eq!(
            owners.person_for_a_reader("mob", "nobody"),
            MemberPerson::Shared,
            "a record nothing reads owes no read"
        );
        owners.record("mob", "built-apart", Some(&private_labels()));
        owners.record("mob", "built", Some(&BTreeMap::new()));

        owners.owe_the_read_of("mob");
        assert_eq!(
            owners.person_for_a_reader("mob", "nobody"),
            MemberPerson::NotKnown,
            "the mob's read is owed"
        );
        assert_eq!(
            owners.person_for_a_reader("mob", "built-apart"),
            MemberPerson::Private(OWNER.to_string()),
            "what a build recorded is held"
        );
        assert_eq!(
            owners.person_for_a_reader("mob", "built"),
            MemberPerson::Shared,
            "what a build recorded is held"
        );
        assert_eq!(
            owners.person_for_a_reader("another-mob", "nobody"),
            MemberPerson::Shared,
            "the mark is the mob's own"
        );
        assert!(
            !owners.first_read_pending(),
            "the record's first reads are not touched by a mob's mark"
        );

        let bound = std::time::Duration::from_millis(200);
        let asked = std::time::Instant::now();
        owners.read_of_the_mob_awaited("mob", bound).await;
        assert!(asked.elapsed() >= bound, "the wait lasted its bound");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            owners.read_of_the_mob_awaited("another-mob", std::time::Duration::from_hours(1)),
        )
        .await
        .expect("nothing is owed for the other mob");

        // A mob created anew under the id: that the old one was read is
        // forgotten, and the mark stays. The members held under the id are
        // not forgotten by this step: the admission forgets them where it
        // counts the create.
        owners.forget_the_read_of("mob");
        assert_eq!(
            owners.person_for_a_reader("mob", "nobody"),
            MemberPerson::NotKnown,
            "the new mob's read is still owed"
        );
        assert_eq!(
            owners.person_for_a_reader("mob", "built-apart"),
            MemberPerson::Private(OWNER.to_string()),
            "forgetting that the mob was read forgets no member"
        );
    }

    /// While the seed is owed and has not landed, a member the record does
    /// not hold is not known, in every mob but the launch's own; once it
    /// landed, a member of a mob the record has read that it does not hold
    /// is private to no one. The launch's own mob keeps the answer it had:
    /// a member of it the record does not hold is private to no one, also
    /// while the seed is owed.
    #[tokio::test]
    async fn a_member_the_record_does_not_hold_is_not_known_until_the_seed_landed() {
        use crate::mob_announcements::MemberPerson;
        let owners = PrivateOwners::default();
        owners.note_the_seeds_mob("launch");
        let (open, task) = read_in_flight(&owners).await;
        owners.record("mob", "built", Some(&BTreeMap::new()));
        assert_eq!(
            owners.person_for_a_reader("mob", "nobody"),
            MemberPerson::NotKnown
        );
        assert_eq!(
            owners.person_for_a_reader("mob", "built"),
            MemberPerson::Shared,
            "what a build recorded is held while the seed is owed"
        );
        assert_eq!(
            owners.person_for_a_reader("launch", "nobody"),
            MemberPerson::Shared,
            "the launch's own mob is answered as before while the seed is owed"
        );
        open.send(true).expect("the read is waiting");
        task.await.expect("the read ends");
        assert_eq!(
            owners.person_for_a_reader("mob", "nobody"),
            MemberPerson::Shared,
            "the seed landed, and this record follows no mob set"
        );
        assert_eq!(
            owners.person_for_a_reader("launch", "nobody"),
            MemberPerson::Shared
        );
    }

    /// A wait that marks nothing ends at its bound and leaves the read on
    /// its first attempt: a wait with a longer bound still waits for the
    /// read, and goes on when the read lands. With nothing in flight it
    /// returns at once.
    #[tokio::test]
    async fn a_wait_that_marks_nothing_leaves_a_longer_wait_waiting() {
        let owners = PrivateOwners::default();
        let (open, task) = read_in_flight(&owners).await;
        let bound = std::time::Duration::from_millis(200);
        let asked = std::time::Instant::now();
        owners.read_of_the_mob_awaited("mob", bound).await;
        assert!(asked.elapsed() >= bound, "the wait lasted its bound");
        assert!(owners.first_read_pending(), "the read is still owed");

        let longer = {
            let owners = owners.clone();
            tokio::spawn(async move {
                owners
                    .first_attempts_settled(std::time::Duration::from_hours(1))
                    .await;
                owners.first_read_pending()
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !longer.is_finished(),
            "the read is still on its first attempt for a wait with a longer bound"
        );
        open.send(true).expect("the read is waiting");
        task.await.expect("the read ends");
        assert!(
            !longer.await.expect("the longer wait goes on"),
            "the longer wait finds the read landed"
        );

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            owners.read_of_the_mob_awaited("mob", std::time::Duration::from_hours(1)),
        )
        .await
        .expect("with nothing in flight the wait returns at once");
    }

    /// A prompt-context fork of a private member takes the fork source's
    /// owner, whoever asks for it, and a spawner private to another person
    /// may not fork it at all.
    #[test]
    fn a_fork_launch_of_a_private_member_takes_its_owner() {
        let owners = PrivateOwners::default();
        owners.record("mob", "side", Some(&private_labels()));
        owners.record("mob", "shared", Some(&BTreeMap::new()));
        owners.record(
            "mob",
            "bobs",
            Some(&BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:bb".to_string(),
            )])),
        );
        let fork_of_side = |name: &str| {
            let mut spec = SpawnMemberSpec::new("worker", meerkat_mob::AgentIdentity::from(name));
            spec.launch_mode = MemberLaunchMode::Fork {
                source_member_id: meerkat_mob::AgentIdentity::from("side"),
                fork_context: meerkat_mob::launch::ForkContext::default(),
            };
            spec
        };
        let mut asked_by_shared = fork_of_side("h5");
        owners
            .apply(
                "mob",
                Some(&meerkat_mob::AgentIdentity::from("shared")),
                &mut asked_by_shared,
            )
            .expect("a shared spawner's fork of a private member");
        assert_eq!(
            asked_by_shared
                .labels
                .as_ref()
                .and_then(|labels| labels.get(crate::access::PRIVATE_TO_LABEL))
                .map(String::as_str),
            Some(OWNER),
            "the fork is private to its source's owner: {:?}",
            asked_by_shared.labels
        );
        let mut asked_by_another = fork_of_side("h6");
        assert!(
            owners
                .apply(
                    "mob",
                    Some(&meerkat_mob::AgentIdentity::from("bobs")),
                    &mut asked_by_another,
                )
                .is_err(),
            "a spawner private to another person may not fork it"
        );
    }

    /// A member private to a person is built without WorkGraph tools: the
    /// base layer's pre-build hook turns them off for its build, and leaves
    /// a shared member's build as it is.
    #[tokio::test]
    async fn a_private_member_is_built_without_workgraph_tools() {
        let owners = PrivateOwners::default();
        let noop: PreBuildHook =
            Arc::new(|_req: &mut CreateSessionRequest| Box::pin(async { Ok(()) }));
        let hook = owners.after_hook(noop);
        for (labels, private) in [(private_labels(), true), (BTreeMap::new(), false)] {
            let mut req = CreateSessionRequest {
                model: "gpt-5.5".to_string(),
                prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    mob_member_binding: Some(meerkat_core::MobMemberBinding {
                        mob_id: "mob".to_string(),
                        role: "worker".to_string(),
                        member: "side".to_string(),
                    }),
                    override_workgraph: meerkat_core::ToolCategoryOverride::Enable,
                    ..Default::default()
                }),
                labels: Some(labels),
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
                injected_context: Vec::new(),
            };
            hook(&mut req).await.expect("the hook runs");
            let workgraph = req
                .build
                .as_ref()
                .map(|build| build.override_workgraph)
                .expect("build options");
            assert_eq!(
                workgraph,
                if private {
                    meerkat_core::ToolCategoryOverride::Disable
                } else {
                    meerkat_core::ToolCategoryOverride::Enable
                },
                "private: {private}"
            );
        }
    }

    /// A follower that keeps the ends it is handed.
    #[derive(Default)]
    struct KeptEnds(std::sync::Mutex<Vec<EndedPrivateMember>>);

    #[async_trait::async_trait]
    impl PrivateMemberEndFollower for KeptEnds {
        async fn member_ended(&self, ended: &EndedPrivateMember) {
            self.0.lock().unwrap().push(ended.clone());
        }
    }

    fn ended(member: &str) -> EndedPrivateMember {
        EndedPrivateMember {
            mob_id: "mob".to_string(),
            member: member.to_string(),
            session: Some(format!("session-of-{member}")),
            ended_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    /// An end seen before a follower is installed is held and handed over
    /// at the install, oldest first; an end seen after it goes to the
    /// follower at once, and nothing is handed over twice.
    #[tokio::test]
    async fn an_end_seen_before_a_follower_is_installed_is_followed_at_the_install() {
        let owners = PrivateOwners::default();
        assert!(
            !owners.member_ended(ended("first")).await,
            "an end no follower took is answered as only held"
        );
        owners.member_ended(ended("second")).await;

        let follower = Arc::new(KeptEnds::default());
        owners.follow_member_ends(follower.clone()).await;
        assert_eq!(
            *follower.0.lock().unwrap(),
            vec![ended("first"), ended("second")],
            "the held ends are handed over at the install"
        );

        assert!(
            owners.member_ended(ended("third")).await,
            "an end the follower took is answered as taken"
        );
        assert_eq!(
            *follower.0.lock().unwrap(),
            vec![ended("first"), ended("second"), ended("third")],
            "a later end goes to the follower, and the held ones are not handed over again"
        );
    }

    /// The hold keeps the newest ends of a launch that installs no follower.
    #[tokio::test]
    async fn the_hold_of_ends_is_bounded() {
        let owners = PrivateOwners::default();
        for index in 0..=HELD_ENDS_MAX {
            owners.member_ended(ended(&format!("member-{index}"))).await;
        }
        let follower = Arc::new(KeptEnds::default());
        owners.follow_member_ends(follower.clone()).await;
        let kept = follower.0.lock().unwrap();
        assert_eq!(kept.len(), HELD_ENDS_MAX);
        assert_eq!(kept.first(), Some(&ended("member-1")));
        assert_eq!(
            kept.last(),
            Some(&ended(&format!("member-{HELD_ENDS_MAX}")))
        );
    }

    /// Each seat has its own follower and its own hold: an end goes to the
    /// follower of every seat that has one and is held for the others, a
    /// seat installed later is handed what it held, and an end is answered
    /// as taken once a follower took it.
    #[tokio::test]
    async fn each_seat_follows_and_holds_ends_on_its_own() {
        let owners = PrivateOwners::default();
        let schedules = Arc::new(KeptEnds::default());
        owners.follow_member_ends(schedules.clone()).await;
        assert!(
            owners.member_ended(ended("first")).await,
            "the schedules' follower took the end"
        );

        let question_sets = Arc::new(KeptEnds::default());
        owners
            .follow_member_ends_as(MemberEndSeat::QuestionSets, question_sets.clone())
            .await;
        assert_eq!(
            *question_sets.0.lock().unwrap(),
            vec![ended("first")],
            "the seat installed later is handed the end it held"
        );
        owners.member_ended(ended("second")).await;
        for (seat, follower) in [("schedules", &schedules), ("question sets", &question_sets)] {
            assert_eq!(
                *follower.0.lock().unwrap(),
                vec![ended("first"), ended("second")],
                "{seat}"
            );
        }

        // A later install in a seat replaces its follower and is handed
        // nothing again.
        let replaced = Arc::new(KeptEnds::default());
        owners.follow_member_ends(replaced.clone()).await;
        owners.member_ended(ended("third")).await;
        assert_eq!(*replaced.0.lock().unwrap(), vec![ended("third")]);
        assert_eq!(schedules.0.lock().unwrap().len(), 2);
        assert_eq!(question_sets.0.lock().unwrap().len(), 3);
    }

    fn seated(owner: Option<&str>, ephemeral: bool) -> HolderStep {
        HolderStep::Seated {
            owner: owner.map(str::to_string),
            ephemeral,
            seated_as: SeatedAs::default(),
        }
    }

    /// An earlier holder of a name that is private to no one is read as it
    /// was seated; the member seated last, a member private to a person and
    /// a moment no member holds are not.
    #[test]
    fn an_earlier_shared_holder_of_a_name_is_read_as_it_was_seated() {
        let as_role = |role: &str| SeatedAs {
            role: role.to_string(),
            labels: BTreeMap::from([("team".to_string(), role.to_string())]),
        };
        let name = NameSeatings::of_seated(vec![
            (100, None, as_role("planner")),
            (300, None, as_role("worker")),
        ]);
        assert_eq!(name.earlier_shared_holder_at(99), None);
        assert_eq!(
            name.earlier_shared_holder_at(100),
            Some(&as_role("planner"))
        );
        assert_eq!(
            name.earlier_shared_holder_at(299),
            Some(&as_role("planner"))
        );
        assert_eq!(
            name.earlier_shared_holder_at(300),
            None,
            "the member seated last"
        );
        assert_eq!(
            name.earlier_shared_holder_at(400),
            None,
            "the member seated last"
        );

        let after_a_private_member = NameSeatings::of_seated(vec![
            (100, Some(OWNER.to_string()), as_role("planner")),
            (300, None, as_role("worker")),
        ]);
        assert_eq!(after_a_private_member.earlier_shared_holder_at(200), None);

        // The journal's row of a seating carries the role and the labels.
        let owners = PrivateOwners::default();
        let labels = BTreeMap::from([("team".to_string(), "blue".to_string())]);
        owners.note_journal_step(&MemberJournalStep::seated_as(
            "mob", "gamma", "planner", &labels, true, 1, 100,
        ));
        owners.note_journal_step(&MemberJournalStep::seated_as(
            "mob",
            "gamma",
            "worker",
            &BTreeMap::new(),
            true,
            2,
            300,
        ));
        let seatings = owners.name_seatings();
        let gamma = seatings
            .get("gamma")
            .and_then(|lists| lists.first())
            .map(|(_, seatings)| seatings);
        assert_eq!(
            gamma.and_then(|name| name.earlier_shared_holder_at(200)),
            Some(&SeatedAs {
                role: "planner".to_string(),
                labels,
            })
        );
    }

    /// The holders of a name whose journal recorded `steps`, in that order,
    /// each at its time.
    fn holders(steps: &[(u64, HolderStep)]) -> NameHolders {
        let mut holders = NameHolders::default();
        for (cursor, (at_ms, step)) in (1..).zip(steps) {
            holders.note(cursor, *at_ms, *at_ms, step.clone());
        }
        holders
    }

    /// Who `holders` say held the name at `at_ms`: `None` for no known
    /// member, else the person the member was private to, if any.
    #[allow(clippy::option_option)]
    fn held_by(holders: &NameHolders, at_ms: u64) -> Option<Option<&str>> {
        holders
            .seating_at(at_ms)
            .map(|index| holders.owner_of(index))
    }

    /// A moment belongs to the member last seated under the name at or
    /// before it. A member private to a person holds every moment from its
    /// seating up to the next seating under the name, that moment included,
    /// ended or not; a moment before the first seating belongs to no known
    /// member.
    #[test]
    fn a_moment_is_held_by_the_member_last_seated_under_the_name() {
        let private = Some(Some(OWNER));
        let shared = Some(None);
        let name = holders(&[
            (100, seated(Some(OWNER), true)),
            (200, HolderStep::Retired),
            (300, seated(None, true)),
        ]);
        for (at_ms, expected) in [
            (99, None),
            (100, private),
            (150, private),
            (200, private),
            (250, private),
            (300, private),
            (301, shared),
        ] {
            assert_eq!(held_by(&name, at_ms), expected, "at {at_ms}");
        }
        assert_eq!(name.ended(0), Some(200), "the private member ended");
        assert_eq!(name.ended(2), None, "the later member lives");

        // A private member whose end the journal does not hold: its time
        // lasts until the next seating.
        let no_end = holders(&[(100, seated(Some(OWNER), true)), (300, seated(None, true))]);
        assert_eq!(held_by(&no_end, 299), private);
        assert_eq!(held_by(&no_end, 301), shared);
        assert_eq!(
            held_by(&holders(&[(100, seated(Some(OWNER), true))]), u64::MAX),
            private
        );

        // A shared member first: a moment at a later private seating is the
        // private member's.
        let shared_first = holders(&[(100, seated(None, true)), (300, seated(Some(OWNER), true))]);
        assert_eq!(held_by(&shared_first, 299), shared);
        assert_eq!(held_by(&shared_first, 300), private);
    }

    /// The member that held a name at a moment holds it still while no
    /// member under the name ended since and the newest seating is private
    /// to the same person, or like it to no one: a respawn and a rebuild
    /// seat the same member again, and the member of a long-term agent does
    /// not end with a retirement.
    #[test]
    fn a_name_is_held_still_until_its_member_ends_or_another_audience_takes_it() {
        let ended_then_shared = holders(&[
            (100, seated(Some(OWNER), true)),
            (200, HolderStep::Retired),
            (300, seated(None, true)),
        ]);
        assert!(!ended_then_shared.held_since(150), "it ended");
        assert!(ended_then_shared.held_since(301), "the later member");
        assert!(
            !holders(&[(100, seated(Some(OWNER), true)), (200, HolderStep::Retired)])
                .held_since(150),
            "it ended and nobody holds the name"
        );
        assert!(
            holders(&[
                (100, seated(Some(OWNER), true)),
                (150, seated(Some(OWNER), true)),
            ])
            .held_since(120),
            "a respawn seats the same member again"
        );
        assert!(
            holders(&[
                (100, seated(None, false)),
                (200, HolderStep::Retired),
                (300, seated(None, false)),
            ])
            .held_since(150),
            "a long-term agent's member is retired and seated again"
        );
        assert!(
            !holders(&[
                (100, seated(Some(OWNER), false)),
                (300, seated(None, false))
            ])
            .held_since(150),
            "a later seating private to no one"
        );
        assert!(
            !holders(&[(300, seated(None, true))]).held_since(150),
            "no member was seated then"
        );
    }

    /// A row of the journal is noted once, whether the seed's read or the
    /// event pump shows it first, and the steps keep the journal's order,
    /// also where the clock that stamped the rows stepped back: a row is
    /// never read as earlier than the row before it.
    #[test]
    fn a_journal_row_is_noted_once_in_the_journals_order() {
        let owners = PrivateOwners::default();
        let retired = |cursor: u64, at_ms: u64, step: HolderStep| MemberJournalStep {
            mob_id: "mob".to_string(),
            member: "side".to_string(),
            cursor,
            at_ms,
            step,
        };
        let rows = [
            MemberJournalStep::seated("mob", "side", &private_labels(), true, 7, 100),
            retired(9, 200, HolderStep::Retired),
            // A later seating, private to no one, stamped by a clock that
            // stepped back.
            MemberJournalStep::seated("mob", "side", &BTreeMap::new(), true, 12, 50),
        ];
        // The pump shows the end first, then the seed reads every row.
        owners.note_journal_step(&rows[1]);
        for row in &rows {
            owners.note_journal_step(row);
        }
        assert_eq!(owners.private_holder_ended("mob", "side", 150), Some(200));
        assert_eq!(owners.still_held_by("mob", "side", 150), Some(false));
        assert_eq!(owners.still_held_by("mob", "other", 150), None);
        assert_eq!(owners.private_holder_ended("other-mob", "side", 150), None);
        assert!(owners.knows_an_ended_private_member());
        let steps = owners
            .inner
            .holders
            .read()
            .unwrap()
            .get(&("mob".to_string(), "side".to_string()))
            .cloned()
            .expect("the name's steps");
        assert_eq!(
            steps
                .0
                .iter()
                .map(|noted| (noted.cursor, noted.at_ms))
                .collect::<Vec<_>>(),
            vec![(7, 100), (9, 200), (12, 202)],
            "{steps:?}"
        );
        assert_eq!(held_by(&steps, 60), None, "before the first seating");
        assert_eq!(held_by(&steps, 200), Some(Some(OWNER)));
        assert_eq!(held_by(&steps, 201), Some(Some(OWNER)), "after its end");
        assert_eq!(
            held_by(&steps, 202),
            Some(Some(OWNER)),
            "the moment of the next seating is still the private member's"
        );
        assert_eq!(held_by(&steps, 203), Some(None), "the later member");
    }

    /// A row's time is its place among ALL the member rows of its mob's
    /// journal, whatever name each is of and whatever order they are shown
    /// in: later than the row before it, also where the clock that stamped
    /// the rows stepped back. The newest such time is the floor of the
    /// times entries are stamped with, so an entry written once a seating
    /// was shown is not earlier than it, and one written before the next
    /// seating is shown is earlier than that seating.
    #[test]
    fn a_journal_rows_time_is_later_than_the_row_before_it_in_its_mobs_journal() {
        let row = |member: &str, owner: bool, cursor: u64, recorded_ms: u64| {
            let labels = if owner {
                private_labels()
            } else {
                BTreeMap::new()
            };
            MemberJournalStep::seated("mob", member, &labels, true, cursor, recorded_ms)
        };
        let end = |member: &str, cursor: u64, recorded_ms: u64| MemberJournalStep {
            mob_id: "mob".to_string(),
            member: member.to_string(),
            cursor,
            at_ms: recorded_ms,
            step: HolderStep::Retired,
        };
        // The journal: the side chat is seated; another member's row is
        // recorded much later; then the clock steps back, the side chat
        // ends and a shared member is seated under its name.
        let journal = [
            row("side", true, 1, 100),
            row("other", false, 2, 5_000),
            end("side", 3, 40),
            row("side", false, 4, 45),
        ];
        let times_of = |owners: &PrivateOwners| {
            owners
                .inner
                .holders
                .read()
                .unwrap()
                .get(&("mob".to_string(), "side".to_string()))
                .map(|steps| {
                    steps
                        .0
                        .iter()
                        .map(|noted| (noted.cursor, noted.at_ms))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let in_order = PrivateOwners::default();
        // The private seating is shown; then the wall clock steps back
        // behind it: an entry written now is not stamped before it.
        in_order.note_journal_step(&journal[0]);
        assert!(
            crate::runtime::entry_time_ms(30) >= 100,
            "a clock set back after a private seating does not move a new entry before it"
        );
        for row in &journal[1..] {
            in_order.note_journal_step(row);
        }
        assert_eq!(
            times_of(&in_order),
            vec![(1, 100), (3, 5_002), (4, 5_004)],
            "the rows after the step back follow the other member's row"
        );
        // An entry stamped at the floor while the side chat held the name
        // (the floor was the other member's row then) is the side chat's.
        assert_eq!(
            in_order.private_holder_ended("mob", "side", 5_001),
            Some(5_002)
        );
        assert!(
            crate::runtime::entry_time_floor_ms() >= 5_005
                && crate::runtime::entry_time_ms(46) >= 5_005,
            "an entry written once the runtime was shown a seating is never earlier than it"
        );
        // The same journal read again gives the same times.
        let again = PrivateOwners::default();
        for row in &journal {
            again.note_journal_step(row);
        }
        assert_eq!(times_of(&again), times_of(&in_order));
        assert_eq!(again.name_seatings(), in_order.name_seatings());
        // Shown in another order (the pump's rows before the seed's), the
        // times are the same.
        let out_of_order = PrivateOwners::default();
        for row in [&journal[3], &journal[2], &journal[0], &journal[1]] {
            out_of_order.note_journal_step(row);
        }
        assert_eq!(times_of(&out_of_order), times_of(&in_order));
        assert_eq!(
            out_of_order.name_seatings().get("side"),
            in_order.name_seatings().get("side")
        );
        // Another mob's journal is its own.
        let mut elsewhere = row("side", false, 1, 7);
        elsewhere.mob_id = "other-mob".to_string();
        in_order.note_journal_step(&elsewhere);
        assert_eq!(in_order.still_held_by("other-mob", "side", 7), Some(true));
    }

    /// A record that names a mob other than the default mob is decided by
    /// the member that held its name then in that mob: the default mob's
    /// member of that name says nothing of it. A record that names no mob
    /// is decided by the members that held the name then in every mob: a
    /// host that names no mob records the action of every member so, so
    /// where a member private to a person held the name in another mob the
    /// record stays that person's, or an administrator's alone.
    #[test]
    fn a_record_that_names_another_mob_is_decided_by_that_mobs_member() {
        const OTHER_OWNER: &str = "console-caller:v1:bb";
        const MADE_AT_MS: u64 = 500;
        let launch = meerkat_mob::MobId::from("launch");
        let second = meerkat_mob::MobId::from("second");
        let private_to = |person: &str| {
            BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            )])
        };

        let access = crate::access::AccessController::disabled();
        access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
        let owners = PrivateOwners::default();
        // Each name is held in both mobs. `alice`: private to no one in the
        // launch mob, private to a person in the second. `bea`: private to
        // that person in the launch mob, to another person in the second.
        // `carl`: private to that person in the launch mob, to no one in
        // the second.
        let members = [
            ("alice", BTreeMap::new(), private_to(OWNER)),
            ("bea", private_to(OWNER), private_to(OTHER_OWNER)),
            ("carl", private_to(OWNER), BTreeMap::new()),
        ];
        for ((cursor, at_ms), (name, in_launch, in_second)) in
            [(1, 100), (2, 150), (3, 200)].into_iter().zip(&members)
        {
            for (mob_id, labels) in [(&launch, in_launch), (&second, in_second)] {
                owners.note_journal_step(&MemberJournalStep::seated(
                    mob_id.as_str(),
                    name,
                    labels,
                    false,
                    cursor,
                    at_ms,
                ));
                access.record_agent_attributes_in(
                    mob_id,
                    crate::access::AgentResourceAttributes {
                        identity: (*name).to_string(),
                        labels: labels.clone(),
                        ..Default::default()
                    },
                );
            }
        }
        access.record_name_seatings(
            &owners,
            crate::access::NameSeatingsHandOver::OWN_ROUTE,
            owners.name_seatings_noted(),
            || owners.name_seatings(),
        );

        // Whether a caller is shown a record made under `name` after every
        // seating: one that names no mob, one that names the launch mob by
        // its id, one that names the second mob.
        let shown = |view: &crate::access::AccessView, name: &str| {
            [None, Some(&launch), Some(&second)].map(|mob_id| {
                view.in_stored_mob(mob_id)
                    .sees_origin(name, Some(MADE_AT_MS))
            })
        };
        let someone = access.view_for_subject(Some("someone@example.test"));
        let owner = access.view_for_caller(Some("owner@example.test"), Some(OWNER));
        let other_owner = access.view_for_caller(Some("other@example.test"), Some(OTHER_OWNER));

        // The second mob's `carl` was private to no one then: its record is
        // shown to every caller who may view it, whoever the launch mob's
        // `carl` was private to.
        assert_eq!(shown(&someone, "carl"), [false, false, true]);
        assert_eq!(shown(&other_owner, "carl"), [false, false, true]);
        assert_eq!(shown(&owner, "carl"), [true, true, true]);

        // The second mob's `bea` was private to a person then: its record
        // is that person's, whoever the launch mob's `bea` was private to.
        assert_eq!(shown(&other_owner, "bea"), [false, false, true]);
        assert_eq!(shown(&someone, "bea"), [false, false, false]);

        // A record that names no mob, and one that names the launch mob,
        // stay withheld where a member private to a person held the name
        // then in another mob: from the person the launch mob's `bea` was
        // private to, and from a caller who may view the launch mob's
        // `alice`, which was private to no one. The person the second mob's
        // `alice` was private to is shown them.
        assert_eq!(shown(&owner, "bea"), [false, false, false]);
        assert_eq!(shown(&someone, "alice"), [false, false, false]);
        assert_eq!(shown(&other_owner, "alice"), [false, false, false]);
        assert_eq!(shown(&owner, "alice"), [true, true, true]);
    }

    /// A record that names another mob is decided by that mob's seatings,
    /// so it is withheld while the record has not read that mob's durable
    /// records: the mob's present members are known a moment before its
    /// earlier seatings are, and a name a member private to a person held
    /// then may be held by a member private to no one now. An administrator
    /// is shown it. Once the mob's records are read, the member that held
    /// the name then decides. A record that names no mob, and one that
    /// names the launch mob, are not held back by another mob's read.
    #[test]
    fn a_record_that_names_a_mob_is_withheld_until_that_mobs_records_are_read() {
        let launch = meerkat_mob::MobId::from("launch");
        let second = meerkat_mob::MobId::from("second");
        let private_labels = private_labels();

        let mut config = crate::access::AccessControlConfig::default();
        config.admins.push("admin@example.test".to_string());
        let access = crate::access::AccessController::new(config)
            .expect("an access configuration with one administrator is valid");
        access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
        let owners = PrivateOwners::default();
        // The host is about to load the second mob: its read is owed.
        let read_of_the_second_mob = owners.owe_the_read_of_in_tests(second.as_str());
        // A request that made the host load it primed its present members:
        // `dee` is private to no one now, in both mobs.
        for mob_id in [&launch, &second] {
            access.record_agent_attributes_in(
                mob_id,
                crate::access::AgentResourceAttributes {
                    identity: "dee".to_string(),
                    ..Default::default()
                },
            );
        }
        owners.note_journal_step(&MemberJournalStep::seated(
            launch.as_str(),
            "dee",
            &BTreeMap::new(),
            false,
            1,
            100,
        ));
        let hand_over = |owners: &PrivateOwners| {
            access.record_name_seatings(
                owners,
                crate::access::NameSeatingsHandOver::OWN_ROUTE,
                owners.name_seatings_noted(),
                || owners.name_seatings(),
            );
        };
        hand_over(&owners);

        // Whether a caller is shown a record made under `dee` at `at_ms`:
        // one that names no mob, one that names the launch mob by its id,
        // one that names the second mob.
        let shown = |view: &crate::access::AccessView, at_ms: u64| {
            [None, Some(&launch), Some(&second)]
                .map(|mob_id| view.in_stored_mob(mob_id).sees_origin("dee", Some(at_ms)))
        };
        let someone = access.view_for_subject(Some("someone@example.test"));
        let owner = access.view_for_caller(Some("owner@example.test"), Some(OWNER));
        let admin = access.view_for_subject(Some("admin@example.test"));

        // The second mob's records are not read: a record that names it is
        // withheld from every caller but an administrator, whoever holds
        // the name now.
        assert_eq!(shown(&someone, 200), [true, true, false]);
        assert_eq!(shown(&owner, 200), [true, true, false]);
        assert_eq!(shown(&admin, 200), [true, true, true]);
        // The decision fence withholds it alike.
        assert!(
            !access
                .decision_fence(Some("someone@example.test"), None)
                .can_see_origin_in(Some(&second), "dee", Some(200))
        );

        // The record reads the second mob's log: a member private to a
        // person held `dee` there before the member that holds it now.
        owners.note_journal_step(&MemberJournalStep::seated(
            second.as_str(),
            "dee",
            &private_labels,
            true,
            1,
            100,
        ));
        owners.note_journal_step(&MemberJournalStep::seated(
            second.as_str(),
            "dee",
            &BTreeMap::new(),
            false,
            2,
            300,
        ));
        drop(read_of_the_second_mob);
        // The record has read the mob and the controller still holds the
        // seatings it was handed before: the record stays withheld until
        // they are handed again.
        assert_eq!(shown(&someone, 400), [true, true, false]);
        hand_over(&owners);

        // What was recorded while the private member held the name is its
        // person's; what the present member recorded is shown to a caller
        // who may view it.
        assert_eq!(shown(&someone, 200), [false, false, false]);
        assert_eq!(shown(&owner, 200), [true, true, true]);
        assert_eq!(shown(&someone, 400), [true, true, true]);
        // The decision fence decides the record that names the second mob
        // as the list does: its person may decide it, the other caller not.
        let by_fence = |subject: &str, person: Option<&str>| {
            access
                .decision_fence(Some(subject), person)
                .can_see_origin_in(Some(&second), "dee", Some(200))
        };
        assert!(by_fence("owner@example.test", Some(OWNER)));
        assert!(!by_fence("someone@example.test", None));
    }

    /// A host of one mob is not held back: a record that names no mob, and
    /// one that names the launch mob by its id, are decided as before by a
    /// controller that knows its default mob, also when the record noted a
    /// journal row after it last handed its seatings over. A controller
    /// that was never told its default mob reads the record that names the
    /// launch mob by its id as another mob's, whose agents it does not
    /// know: that record is withheld there, as a record of an unknown
    /// member is.
    #[test]
    fn a_host_of_one_mob_is_decided_as_before_when_a_row_is_noted_after_the_hand_over() {
        let launch = meerkat_mob::MobId::from("launch");
        let seated = |name: &str, cursor: u64, at_ms: u64| {
            MemberJournalStep::seated(
                launch.as_str(),
                name,
                &BTreeMap::new(),
                false,
                cursor,
                at_ms,
            )
        };
        let record_agents = |access: &crate::access::AccessController| {
            access.record_agent_attributes(crate::access::AgentResourceAttributes {
                identity: "dee".to_string(),
                ..Default::default()
            });
        };
        // Whether a caller is shown a record made under `dee`: one that
        // names no mob, one that names the launch mob by its id.
        let shown = |view: &crate::access::AccessView| {
            [None, Some(&launch)]
                .map(|mob_id| view.in_stored_mob(mob_id).sees_origin("dee", Some(200)))
        };

        for told_its_default_mob in [true, false] {
            let access = crate::access::AccessController::disabled();
            if told_its_default_mob {
                access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
            }
            record_agents(&access);
            let owners = PrivateOwners::default();
            owners.note_journal_step(&seated("dee", 1, 100));
            access.record_name_seatings(
                &owners,
                crate::access::NameSeatingsHandOver::OWN_ROUTE,
                owners.name_seatings_noted(),
                || owners.name_seatings(),
            );
            let someone = access.view_for_subject(Some("someone@example.test"));
            let as_before = if told_its_default_mob {
                [true, true]
            } else {
                [true, false]
            };
            assert_eq!(shown(&someone), as_before, "told: {told_its_default_mob}");
            // A member is seated under another name: a journal row the
            // record notes after the hand-over.
            owners.note_journal_step(&seated("eve", 2, 300));
            assert_eq!(shown(&someone), as_before, "told: {told_its_default_mob}");
        }
    }

    /// Where one controller holds the records of two runtimes, a record
    /// that names a mob waits for the read of the runtime it was made in:
    /// another runtime's record, whose names a console keys under another
    /// identity namespace, and which follows no mob set and so owes the
    /// read of no mob, does not stand for that read. A record that names
    /// no mob is not held back, and once the runtime has read the mob the
    /// record is decided.
    #[test]
    fn a_record_that_names_a_mob_waits_for_the_read_of_its_own_runtime() {
        let launch = meerkat_mob::MobId::from("launch");
        let second = meerkat_mob::MobId::from("second");
        let of_another_runtime = crate::access::NameSeatingsHandOver {
            reader: crate::access::PrivateOwnerRecordReader::ServedRuntime(
                crate::console_aggregator::ConsoleAggregatorId::mint(),
            ),
            namespace: "team",
        };

        let mut config = crate::access::AccessControlConfig::default();
        config.admins.push("admin@example.test".to_string());
        let access = crate::access::AccessController::new(config)
            .expect("an access configuration with one administrator is valid");
        access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
        for mob_id in [&launch, &second] {
            access.record_agent_attributes_in(
                mob_id,
                crate::access::AgentResourceAttributes {
                    identity: "dee".to_string(),
                    ..Default::default()
                },
            );
        }
        // The runtime the record was made in: its host is about to
        // load the second mob, whose read is owed.
        let owners = PrivateOwners::default();
        let read_of_the_second_mob = owners.owe_the_read_of_in_tests(second.as_str());
        owners.note_journal_step(&MemberJournalStep::seated(
            launch.as_str(),
            "dee",
            &BTreeMap::new(),
            false,
            1,
            100,
        ));
        // Another runtime the same controller is primed from.
        let another = PrivateOwners::default();
        let hand_over = |record: &PrivateOwners, by| {
            access.record_name_seatings(record, by, record.name_seatings_noted(), || {
                record.name_seatings()
            });
        };
        hand_over(&owners, crate::access::NameSeatingsHandOver::OWN_ROUTE);
        hand_over(&another, of_another_runtime);

        let someone = access.view_for_subject(Some("someone@example.test"));
        let shown = |mob_id: Option<&meerkat_mob::MobId>| {
            someone.in_stored_mob(mob_id).sees_origin("dee", Some(200))
        };
        let by_fence = || {
            access
                .decision_fence(Some("someone@example.test"), None)
                .can_see_origin_in(Some(&second), "dee", Some(200))
        };
        assert!(
            !shown(Some(&second)),
            "the other runtime's record does not stand for the read of the second mob"
        );
        assert!(shown(None));
        assert!(!by_fence());

        // The runtime reads the second mob, where no one else held the
        // name: the member that holds it now decides.
        drop(read_of_the_second_mob);
        hand_over(&owners, crate::access::NameSeatingsHandOver::OWN_ROUTE);
        assert!(shown(Some(&second)));
        assert!(by_fence());
    }

    /// Which runtimes a record that names a mob waits for, where one
    /// controller holds the records of two: the runtimes whose names a
    /// console keys under the most specific identity namespace that can key
    /// the record's name. The runtime the record was made in owes the read
    /// of the second mob; the other follows no mob set and so owes the read
    /// of no mob.
    ///
    /// A name under a namespace is its namespace's: a runtime handed under
    /// no namespace does not stand for the read of a mob of the runtime
    /// handed under `team`, though the empty namespace can key every name.
    ///
    /// Runtimes keyed under one namespace are not told apart, as their
    /// members' names are not: there the other runtime's record does stand
    /// for the read. A host tells its runtimes apart by giving each its own
    /// namespace.
    #[test]
    fn a_record_that_names_a_mob_waits_for_the_runtimes_of_its_names_namespace() {
        let launch = meerkat_mob::MobId::from("launch");
        let second = meerkat_mob::MobId::from("second");
        let by_a_console = |namespace: &'static str| crate::access::NameSeatingsHandOver {
            reader: crate::access::PrivateOwnerRecordReader::ServedRuntime(
                crate::console_aggregator::ConsoleAggregatorId::mint(),
            ),
            namespace,
        };
        // The namespace of the record's own runtime, the namespace of the
        // other runtime, the name the record is made under, and whether
        // the record is shown while its own runtime owes the read.
        for (own, other, name, shown_meanwhile) in [
            ("team", "", "team/dee", false),
            ("team", "team", "team/dee", true),
            ("", "", "dee", true),
        ] {
            let mut config = crate::access::AccessControlConfig::default();
            config.admins.push("admin@example.test".to_string());
            let access = crate::access::AccessController::new(config)
                .expect("an access configuration with one administrator is valid");
            access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
            for mob_id in [&launch, &second] {
                access.record_agent_attributes_in(
                    mob_id,
                    crate::access::AgentResourceAttributes {
                        identity: name.to_string(),
                        ..Default::default()
                    },
                );
            }
            let owners = PrivateOwners::default();
            let _read_of_the_second_mob = owners.owe_the_read_of_in_tests(second.as_str());
            owners.note_journal_step(&MemberJournalStep::seated(
                launch.as_str(),
                "dee",
                &BTreeMap::new(),
                false,
                1,
                100,
            ));
            let another = PrivateOwners::default();
            for (record, namespace) in [(&owners, own), (&another, other)] {
                access.record_name_seatings(
                    record,
                    by_a_console(namespace),
                    record.name_seatings_noted(),
                    || {
                        record
                            .name_seatings()
                            .into_iter()
                            .map(|(name, seatings)| {
                                if namespace.is_empty() {
                                    (name, seatings)
                                } else {
                                    (format!("{namespace}/{name}"), seatings)
                                }
                            })
                            .collect()
                    },
                );
            }

            let someone = access.view_for_subject(Some("someone@example.test"));
            assert_eq!(
                someone
                    .in_stored_mob(Some(&second))
                    .sees_origin(name, Some(200)),
                shown_meanwhile,
                "the record's runtime under {own:?}, the other under {other:?}"
            );
            assert_eq!(
                access
                    .decision_fence(Some("someone@example.test"), None)
                    .can_see_origin_in(Some(&second), name, Some(200)),
                shown_meanwhile,
                "the record's runtime under {own:?}, the other under {other:?}, by the fence"
            );
        }
    }

    /// A mob created anew under an id is not the mob that held the id
    /// before, and its journal starts again at its first row. Once the
    /// create is admitted the record holds none of the earlier mob's
    /// seatings under the id, and the new mob's first row is noted though
    /// the earlier mob's journal had a row of that cursor. A record made
    /// under a name in the new mob is decided by the member the new mob
    /// seated under it: a member private to a person where the earlier
    /// mob's was private to no one, and the other way round.
    #[test]
    fn a_mob_created_under_an_id_keeps_none_of_an_earlier_mobs_name_seatings() {
        let launch = meerkat_mob::MobId::from("launch");
        let helper = meerkat_mob::MobId::from("helper");
        let labels = |private: bool| {
            if private {
                private_labels()
            } else {
                BTreeMap::new()
            }
        };
        // Whether the earlier mob's member was private to the person, and
        // whether the new mob's is.
        for (earlier_private, new_private) in [(false, true), (true, false)] {
            let owners = PrivateOwners::default();
            // The earlier mob seats `dee` at its journal's first row.
            owners.note_journal_step(&MemberJournalStep::seated(
                helper.as_str(),
                "dee",
                &labels(earlier_private),
                false,
                1,
                100,
            ));
            // That mob goes and a create under its id is admitted: the new
            // mob seats `dee` at its own journal's first row.
            owners.admit_a_create_in_tests(helper.as_str(), launch.as_str());
            owners.note_journal_step(&MemberJournalStep::seated(
                helper.as_str(),
                "dee",
                &labels(new_private),
                false,
                1,
                500,
            ));

            let seatings = owners.name_seatings();
            let of_the_mob: Vec<&NameSeatings> = seatings
                .get("dee")
                .into_iter()
                .flatten()
                .filter(|(mob_id, _)| mob_id == &helper)
                .map(|(_, seatings)| seatings)
                .collect();
            assert_eq!(
                of_the_mob
                    .iter()
                    .map(|seatings| seatings.0.clone())
                    .collect::<Vec<_>>(),
                vec![vec![(500, new_private.then(|| OWNER.to_string()))]],
                "the mob created under the id holds its own seating alone: \
                 earlier private {earlier_private}, new private {new_private}"
            );

            let mut config = crate::access::AccessControlConfig::default();
            config.admins.push("admin@example.test".to_string());
            let access = crate::access::AccessController::new(config)
                .expect("an access configuration with one administrator is valid");
            access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
            access.record_agent_attributes_in(
                &helper,
                crate::access::AgentResourceAttributes {
                    identity: "dee".to_string(),
                    ..Default::default()
                },
            );
            access.record_name_seatings(
                &owners,
                crate::access::NameSeatingsHandOver::OWN_ROUTE,
                owners.name_seatings_noted(),
                || owners.name_seatings(),
            );
            // A record made under `dee` in the new mob, after its seating.
            let shown = |view: &crate::access::AccessView| {
                view.in_stored_mob(Some(&helper))
                    .sees_origin("dee", Some(600))
            };
            let someone = access.view_for_subject(Some("someone@example.test"));
            let owner = access.view_for_caller(Some("owner@example.test"), Some(OWNER));
            assert_eq!(
                shown(&someone),
                !new_private,
                "earlier private {earlier_private}, new private {new_private}"
            );
            assert!(
                shown(&owner),
                "earlier private {earlier_private}, new private {new_private}"
            );
        }
    }

    /// A reading pass that listed a mob before a create was admitted under
    /// its id pages the journal of the earlier mob: it notes none of that
    /// journal's rows under the id. A pass that lists the mob after the
    /// create notes the new mob's rows.
    #[tokio::test]
    async fn a_pass_that_listed_a_mob_before_a_create_under_its_id_notes_none_of_its_rows() {
        struct Journal(Vec<MemberJournalStep>);

        #[async_trait::async_trait]
        impl MemberSpawnLog for Journal {
            async fn spawns_after(
                &self,
                cursor: u64,
                _limit: usize,
            ) -> Result<Option<MemberSpawnPage>, String> {
                Ok((cursor == 0).then(|| (self.0.clone(), 1)))
            }
        }

        let owners = PrivateOwners::default();
        let of_the_earlier_mob = Journal(vec![MemberJournalStep::seated(
            "helper",
            "dee",
            &private_labels(),
            false,
            1,
            100,
        )]);
        let of_the_new_mob = Journal(vec![MemberJournalStep::seated(
            "helper",
            "dee",
            &BTreeMap::new(),
            false,
            1,
            500,
        )]);
        let seatings_of_dee = |owners: &PrivateOwners| -> Vec<Vec<(u64, Option<String>)>> {
            owners
                .name_seatings()
                .get("dee")
                .into_iter()
                .flatten()
                .map(|(_, seatings)| seatings.0.clone())
                .collect()
        };

        // A pass notes the creates, then lists the mob that holds `helper`.
        let listed_before = owners.creates_admitted();
        // That mob goes and a create under the id is admitted.
        owners.admit_a_create_in_tests("helper", "launch");
        // The pass pages the log of the mob it listed.
        owners
            .read_log(&of_the_earlier_mob, &listed_before)
            .await
            .expect("the earlier mob's log is paged");
        assert_eq!(
            seatings_of_dee(&owners),
            Vec::<Vec<(u64, Option<String>)>>::new(),
            "no row of the earlier mob's journal is noted under the id"
        );

        // A pass that lists the mob after the create reads the new mob.
        owners
            .read_log(&of_the_new_mob, &owners.creates_admitted())
            .await
            .expect("the new mob's log is paged");
        assert_eq!(seatings_of_dee(&owners), vec![vec![(500, None)]]);
    }

    /// The event pump of a mob can still show a row of its journal after a
    /// create under the mob's id was admitted: the pump is ended by a pass
    /// that nothing orders before that admission. Such a row is the earlier
    /// mob's. It is not noted under the id, and where it removes a member
    /// its end is not followed: the member the new mob seated under the
    /// name, private to a person, stands as it is, and the follower of the
    /// question sets' seat is handed nothing.
    #[tokio::test]
    async fn a_row_an_earlier_mobs_pump_shows_after_a_create_under_its_id_is_not_followed() {
        let owners = PrivateOwners::default();
        let question_sets = Arc::new(KeptEnds::default());
        owners
            .follow_member_ends_as(MemberEndSeat::QuestionSets, question_sets.clone())
            .await;
        // The earlier mob seats `dee`, private to no one. Its pump noted
        // the creates admitted so far when it was made.
        owners.note_journal_step(&MemberJournalStep::seated(
            "helper",
            "dee",
            &BTreeMap::new(),
            false,
            1,
            100,
        ));
        let noted_by_the_earlier_mobs_pump = owners.creates_admitted();
        // That mob goes and a create under the id is admitted: the new mob
        // seats `dee`, private to a person, at its journal's first row.
        owners.admit_a_create_in_tests("helper", "launch");
        owners.note_journal_step(&MemberJournalStep::seated(
            "helper",
            "dee",
            &private_labels(),
            false,
            1,
            500,
        ));
        // The earlier mob's pump shows the row that removed its `dee`: a
        // later row of the earlier journal.
        let removed = MemberJournalStep {
            mob_id: "helper".to_string(),
            member: "dee".to_string(),
            cursor: 7,
            at_ms: 400,
            step: HolderStep::Removed,
        };
        owners
            .follow_a_shown_journal_step(
                &removed,
                chrono::Utc::now(),
                &noted_by_the_earlier_mobs_pump,
            )
            .await;

        assert_eq!(
            owners.with_holders("helper", "dee", |holders| holders
                .map(|holders| holders.0.len())),
            Some(1),
            "the name's history in the new mob holds its own seating alone"
        );
        assert_eq!(
            owners
                .name_seatings()
                .get("dee")
                .into_iter()
                .flatten()
                .map(|(_, seatings)| seatings.0.clone())
                .collect::<Vec<_>>(),
            vec![vec![(500, Some(OWNER.to_string()))]]
        );
        assert!(
            question_sets.0.lock().unwrap().is_empty(),
            "the end of the new mob's member is not followed"
        );

        // A row the new mob's own pump shows is noted, and the end it
        // records is followed.
        let noted_by_the_new_mobs_pump = owners.creates_admitted();
        let removed_now = MemberJournalStep {
            mob_id: "helper".to_string(),
            member: "dee".to_string(),
            cursor: 2,
            at_ms: 600,
            step: HolderStep::Removed,
        };
        owners
            .follow_a_shown_journal_step(
                &removed_now,
                chrono::Utc::now(),
                &noted_by_the_new_mobs_pump,
            )
            .await;
        assert_eq!(
            owners.with_holders("helper", "dee", |holders| holders
                .map(|holders| holders.0.len())),
            Some(2)
        );
        assert_eq!(
            question_sets.0.lock().unwrap().len(),
            1,
            "the end the new mob's own journal records is followed"
        );
    }

    /// The moment a member private to one person is seated under a name a
    /// member private to another person held before is held by neither:
    /// what was recorded at that moment is not told to be the later
    /// member's. The moment after is the later member's.
    #[test]
    fn the_moment_one_persons_member_follows_anothers_is_held_by_neither() {
        let handed_over = holders(&[
            (100, seated(Some(OWNER), true)),
            (200, HolderStep::Retired),
            (300, seated(Some("console-caller:v1:bb"), true)),
        ]);
        assert!(!handed_over.held_since(300));
        assert!(handed_over.held_since(301));
    }

    /// A journal row's time leaves room after the row before it, and the
    /// floor of the entries' times is past the newest row: an entry written
    /// once a seating was shown is later than that seating and earlier than
    /// the next row, also while the clock that stamps both is behind the
    /// journal.
    #[test]
    fn an_entry_written_once_a_seating_was_shown_is_later_than_it_and_earlier_than_the_next_row() {
        let mut times = JournalTimes::default();
        let (seating, _) = times.note(1, 9_000);
        let (next, _) = times.note(2, 8_001);
        assert!(
            next > seating + 1,
            "the next row's time leaves a moment between: {seating} then {next}"
        );

        let owners = PrivateOwners::default();
        owners.note_journal_step(&MemberJournalStep::seated(
            "mob",
            "side",
            &private_labels(),
            true,
            1,
            9_000,
        ));
        assert!(
            crate::runtime::entry_time_ms(8_000) > 9_000,
            "an entry written by a clock behind the journal is later than the seating shown"
        );
    }

    /// A long-term agent private to a person ends with its identity entry,
    /// not with a retirement that seats it again; an archive is no end.
    #[test]
    fn a_long_term_agent_ends_with_its_identity_entry() {
        let agent = holders(&[
            (100, seated(Some(OWNER), false)),
            (200, HolderStep::Retired),
            (300, seated(Some(OWNER), false)),
            (400, HolderStep::Removed),
        ]);
        assert_eq!(agent.ended(0), None, "a rebuild seats it again");
        assert_eq!(agent.ended(2), Some(400), "its identity entry ended");
        assert!(!agent.held_since(150));
        assert_eq!(agent.seating_before(4), Some(2));
    }

    /// The log lines a row's own thread writes.
    #[derive(Clone, Default)]
    struct CapturedLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl CapturedLog {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner))
                .into_owned()
        }
    }

    impl std::io::Write for CapturedLog {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
        type Writer = CapturedLog;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// A mob store that keeps no member history answers so however often it
    /// is asked: the history is read once, the end is left unfollowed with
    /// one warning, and nothing waits. A store error that can pass is read
    /// three times.
    #[tokio::test]
    async fn a_store_that_keeps_no_member_history_is_read_once() {
        let log = CapturedLog::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log.clone())
            .with_ansi(false)
            .finish();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let logged = tracing::subscriber::set_default(subscriber);

        let mut reads = 0;
        let history = read_member_history("mob", "side", || {
            reads += 1;
            async {
                Err(meerkat_mob::MobError::from(
                    meerkat_mob::MobStoreError::IdentityMemberAtomicPersistenceUnavailable,
                ))
            }
        })
        .await;
        drop(logged);
        assert!(history.is_none(), "no history, no end to follow");
        assert_eq!(
            reads, 1,
            "a store that keeps no member history is read once"
        );
        let log = log.contents();
        let warnings: Vec<&str> = log
            .lines()
            .filter(|line| {
                line.contains("the member history of a retiring member could not be read")
            })
            .collect();
        assert_eq!(warnings.len(), 1, "{log}");
        let warning = warnings.first().copied().unwrap_or_default();
        assert!(warning.contains("WARN"), "{log}");
        assert!(warning.contains("member=side"), "{log}");

        let mut reads = 0;
        let history = read_member_history("mob", "side", || {
            reads += 1;
            async {
                Err(meerkat_mob::MobError::StorageError(Box::new(
                    std::io::Error::other("the store is busy"),
                )))
            }
        })
        .await;
        assert!(history.is_none(), "no history, no end to follow");
        assert_eq!(
            reads, HISTORY_READ_ATTEMPTS,
            "a store error that can pass is read again"
        );
    }

    /// The memory store marks as private the plain names of the private
    /// members whose end it recorded, and the key of every identity scope it
    /// keeps apart from the members of a name. Only the plain names are
    /// members to this record: a key is a scope, not a member's name.
    #[tokio::test]
    async fn the_private_rows_of_the_memory_store_name_members_by_plain_names_only() {
        use crate::identity_first::agent_memory::AgentMemoryProvider as _;
        use crate::memory::capabilities::StewardStore as _;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::memory::sqlite_store::SqliteAgentMemoryStore::open(dir.path()).unwrap(),
        );
        store
            .record_private_retirement("default", "ledger-side", None, "retired", 5)
            .await
            .unwrap();
        store
            .remember_authored(
                &crate::memory::records::MemoryScope::Identity {
                    realm: "default".to_string(),
                    identity: "private/console-caller:v1:aa/ledger-side".to_string(),
                },
                crate::memory::records::NewMemoryRecord {
                    kind: crate::memory::records::MemoryKind::Fact,
                    title: "Where the heron is".to_string(),
                    description: String::new(),
                    body: "The heron is at the north pond.".to_string(),
                    tags: Vec::new(),
                    evidence: Vec::new(),
                    verification: None,
                },
                crate::memory::records::MemoryAuthor::Application,
            )
            .await
            .unwrap();
        assert_eq!(
            store.private_retired_identities("default").await.unwrap(),
            BTreeSet::from([
                "ledger-side".to_string(),
                "private/console-caller:v1:aa/ledger-side".to_string(),
            ]),
            "the store marks the name and the key"
        );

        let owners = PrivateOwners::default();
        owners.bind_private_retirements(store, "default".to_string());
        owners
            .first_attempts_settled(std::time::Duration::from_secs(10))
            .await;
        assert_eq!(
            owners.memory_members(),
            BTreeMap::from([(
                "ledger-side".to_string(),
                Some(crate::access::UNKNOWN_PRIVATE_OWNER.to_string()),
            )])
        );
    }

    /// The binding a creation record names is the one that named the
    /// record's session, of the same member, seated at the record's
    /// generation or at a later one that the unbroken chain of same-session
    /// successor steps reaches from it (a role migration among the steps
    /// included). A later generation whose journal answers no chain (it
    /// holds a mob reset), a chain that begins after the record's
    /// generation, an earlier generation, another member on the session and
    /// a session the history never bound are none of them the creating
    /// member: nothing is said of it.
    #[test]
    fn the_creating_members_binding_is_its_sessions_at_its_generation_or_on_an_unbroken_chain_from_it()
     {
        use meerkat_mob::{Generation, MemberBinding, MemberBindingHistory, MemberBindingState};
        const FIRST: &str = "console-caller:v1:first-person";
        let member = meerkat_mob::AgentIdentity::from("creator");
        let session = meerkat_core::SessionId::new();
        let seat = CreatorsSeat {
            ran_at: meerkat_core::service::MobIncarnationId::mint(),
            generation: Generation::new(3),
            session: &session,
        };
        let bound = |identity: &str, generation: u64, chain: Option<(u64, u64)>| {
            MemberBinding::for_test(
                meerkat_mob::AgentIdentity::from(identity),
                Generation::new(generation),
                Some(session.clone()),
                MemberBindingState::Live,
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::SpawnContinuityIntent::Ephemeral,
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
            .with_same_session_lineage(chain.map(|(successors, one_role)| {
                (Generation::new(successors), Generation::new(one_role))
            }))
            .with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                FIRST.to_string(),
            )]))
        };
        let read = |binding: Option<MemberBinding>| {
            let history = MemberBindingHistory::for_test(binding.clone(), binding, [], 1);
            creators_binding(&history, &member, seat)
        };
        let its_own = Some(CreatorsBinding {
            owner: Some(FIRST.to_string()),
            holds_its_name: true,
        });
        assert_eq!(
            read(Some(bound("creator", 3, Some((3, 3))))),
            its_own,
            "the binding of the record's session at the record's generation"
        );
        assert_eq!(
            read(Some(bound("creator", 3, None))),
            its_own,
            "at the record's own generation no chain is needed"
        );
        assert_eq!(
            read(Some(bound("creator", 5, Some((3, 3))))),
            its_own,
            "seated again on its session: the chain starts at the record's generation"
        );
        assert_eq!(
            read(Some(bound("creator", 5, Some((2, 5))))),
            its_own,
            "the chain starts before the record's generation, a role migration among its steps"
        );
        assert_eq!(
            read(Some(bound("creator", 5, None))),
            None,
            "a journal that holds a mob reset answers no chain: not certain"
        );
        assert_eq!(
            read(Some(bound("creator", 5, Some((4, 4))))),
            None,
            "a chain that begins after the record's generation does not reach the creating member"
        );
        assert_eq!(
            read(Some(bound("creator", 2, Some((2, 2))))),
            None,
            "an earlier generation is not the creating member"
        );
        assert_eq!(
            read(Some(bound("another", 3, Some((3, 3))))),
            None,
            "another member on the session is not the creating member"
        );
        assert_eq!(read(None), None, "a session the history never bound");
    }

    /// The creating member holds its name only while the name's latest
    /// binding is its own: the same generation on the same session. A later
    /// member seated under the name at the same generation number on
    /// another session (a journal whose generation numbers began again) is
    /// another member, and what the record holds under the name is then not
    /// the creating member's.
    #[test]
    fn a_later_member_at_the_same_generation_number_on_another_session_holds_the_name() {
        use meerkat_mob::{Generation, MemberBinding, MemberBindingHistory, MemberBindingState};
        const LATER: &str = "console-caller:v1:later-person";
        let member = meerkat_mob::AgentIdentity::from("creator");
        let session = meerkat_core::SessionId::new();
        let another_session = meerkat_core::SessionId::new();
        let seat = CreatorsSeat {
            ran_at: meerkat_core::service::MobIncarnationId::mint(),
            generation: Generation::new(3),
            session: &session,
        };
        let bound = |session: &meerkat_core::SessionId, state: MemberBindingState| {
            MemberBinding::for_test(
                member.clone(),
                Generation::new(3),
                Some(session.clone()),
                state,
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::SpawnContinuityIntent::Ephemeral,
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
        };
        let creators = bound(&session, MemberBindingState::Retired);
        let later =
            bound(&another_session, MemberBindingState::Live).with_labels(BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                LATER.to_string(),
            )]));
        let history = MemberBindingHistory::for_test([later.clone()], [creators, later], [], 2);
        let found = creators_binding(&history, &member, seat)
            .expect("the creating member's own binding is found by its session");
        assert_eq!(
            found.owner, None,
            "the creating member's binding names nobody"
        );
        assert!(
            !found.holds_its_name,
            "the name's latest binding is on another session: the creating member does not hold \
             its name, whatever the generation number"
        );
    }

    /// What a mob's own records say of a creating member, case by case: its
    /// own binding first, whoever holds its name; then the roster row, only
    /// when it is the creating member's own (the record's generation on the
    /// record's session); else nothing.
    #[test]
    fn a_mobs_records_say_who_its_creating_member_is_private_to_by_its_binding_then_its_own_row() {
        use meerkat_mob::{Generation, MemberBinding, MemberBindingHistory, MemberBindingState};
        const FIRST: &str = "console-caller:v1:first-person";
        const SECOND: &str = "console-caller:v1:second-person";
        fn row<'a>(
            generation: u64,
            session: &'a meerkat_core::SessionId,
            labels: &'a BTreeMap<String, String>,
        ) -> SeatedRow<'a> {
            SeatedRow {
                generation: Generation::new(generation),
                session: Some(session),
                labels,
            }
        }
        let mob = meerkat_mob::MobId::from("parent");
        let member = meerkat_mob::AgentIdentity::from("creator");
        let session = meerkat_core::SessionId::new();
        let later_session = meerkat_core::SessionId::new();
        let seat = CreatorsSeat {
            ran_at: meerkat_core::service::MobIncarnationId::mint(),
            generation: Generation::new(3),
            session: &session,
        };
        let private_to = |person: &str| {
            BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person.to_string(),
            )])
        };
        let (first, second, nobody) = (private_to(FIRST), private_to(SECOND), BTreeMap::new());
        let bound = |generation: u64,
                     session: &meerkat_core::SessionId,
                     state: MemberBindingState,
                     labels: &BTreeMap<String, String>| {
            MemberBinding::for_test(
                member.clone(),
                Generation::new(generation),
                Some(session.clone()),
                state,
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::SpawnContinuityIntent::Ephemeral,
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
            .with_labels(labels.clone())
        };
        let says = |owners: &PrivateOwners,
                    history: Option<&MemberBindingHistory>,
                    row: Option<SeatedRow<'_>>| {
            owners.owner_its_mobs_records_say(&mob, &member, seat, history, row)
        };
        let unread = PrivateOwners::default();

        // The creating member left its mob: its binding stays in the history.
        let left = bound(3, &session, MemberBindingState::Retired, &first);
        let history = MemberBindingHistory::for_test([left.clone()], [left], [], 1);
        assert_eq!(
            says(&unread, Some(&history), None),
            Some(Some(FIRST.to_string())),
            "a creating member that left is answered by its own binding"
        );

        // A later member holds its name, private to another person.
        let left = bound(3, &session, MemberBindingState::Retired, &first);
        let later = bound(4, &later_session, MemberBindingState::Live, &second);
        let history = MemberBindingHistory::for_test([later.clone()], [left, later], [], 2);
        assert_eq!(
            says(
                &unread,
                Some(&history),
                Some(row(4, &later_session, &second))
            ),
            Some(Some(FIRST.to_string())),
            "the later holder of the name, and its row, are not asked"
        );

        // The binding names nobody and a later member holds the name: the
        // record's knowledge under the name is the later member's.
        let knows_the_later = PrivateOwners::default();
        knows_the_later.record(mob.as_str(), member.as_str(), Some(&second));
        let left = bound(3, &session, MemberBindingState::Retired, &nobody);
        let later = bound(4, &later_session, MemberBindingState::Live, &second);
        let history = MemberBindingHistory::for_test([later.clone()], [left, later], [], 2);
        assert_eq!(
            says(
                &knows_the_later,
                Some(&history),
                Some(row(4, &later_session, &second))
            ),
            Some(None),
            "a shared creating member whose name a private member holds later: private to no one"
        );

        // The binding names nobody, the creating member still holds its
        // name, and the record knows it as private: it stays private.
        let knows_it_private = PrivateOwners::default();
        knows_it_private.record(mob.as_str(), member.as_str(), Some(&first));
        let seated = bound(3, &session, MemberBindingState::Live, &nobody);
        let history = MemberBindingHistory::for_test([seated.clone()], [seated], [], 1);
        assert_eq!(
            says(
                &knows_it_private,
                Some(&history),
                Some(row(3, &session, &nobody))
            ),
            Some(Some(FIRST.to_string())),
            "a member the record knows as private stays private while it holds its name"
        );

        // No history: the row answers only as the creating member's own.
        assert_eq!(
            says(&unread, None, Some(row(3, &session, &first))),
            Some(Some(FIRST.to_string())),
            "the row of the record's generation on the record's session"
        );
        assert_eq!(
            says(&unread, None, Some(row(3, &later_session, &first))),
            None,
            "the record's generation number on another session is another member"
        );
        assert_eq!(
            says(&unread, None, Some(row(4, &session, &first))),
            None,
            "another generation on the record's session, with no history to follow"
        );
        assert_eq!(says(&unread, None, None), None, "nothing read");

        // A history that holds no binding of the record's session, and a
        // row of another member under the name.
        let later = bound(4, &later_session, MemberBindingState::Live, &second);
        let history = MemberBindingHistory::for_test([later.clone()], [later], [], 1);
        assert_eq!(
            says(
                &unread,
                Some(&history),
                Some(row(4, &later_session, &second))
            ),
            None,
            "a row of another generation says nothing of the creating member"
        );

        // A later generation on the record's session whose journal answers
        // no chain (it holds a mob reset): neither the binding nor its row.
        let reseated =
            bound(5, &session, MemberBindingState::Live, &first).with_same_session_lineage(None);
        let history = MemberBindingHistory::for_test([reseated.clone()], [reseated], [], 1);
        assert_eq!(
            says(&unread, Some(&history), Some(row(5, &session, &first))),
            None,
            "a chain a mob reset broke: not certain"
        );
    }

    /// Wait, ten seconds at most, until `landed` holds: a row's own wait
    /// for the reads a record makes on tasks of its own.
    async fn until(landed: impl Fn() -> bool) {
        for _ in 0..200 {
            if landed() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// A memory store in a directory of its own.
    fn memory_store(
        dir: &tempfile::TempDir,
    ) -> Arc<crate::memory::sqlite_store::SqliteAgentMemoryStore> {
        Arc::new(crate::memory::sqlite_store::SqliteAgentMemoryStore::open(dir.path()).unwrap())
    }

    /// The read of the private retirements a hosted mob's memory recorded
    /// keeps no store value of that mob alive: it holds the store weakly,
    /// so it ends when the mob's memory is dropped, as at the mob's unload.
    #[tokio::test]
    async fn a_hosted_mobs_retirement_binding_keeps_no_store_value_of_the_mob_alive() {
        let dir = tempfile::tempdir().unwrap();
        let of_the_second: Arc<dyn crate::memory::capabilities::StewardStore> = memory_store(&dir);
        let seen = Arc::downgrade(&of_the_second);
        let owners = PrivateOwners::default();
        let second = meerkat_mob::MobId::from("second");
        owners.hosted_mobs().name(second.as_str());
        owners.bind_private_retirements_of(
            &second,
            of_the_second.clone(),
            "mob.second".to_string(),
        );
        // The mob's memory is dropped before the read ran.
        drop(of_the_second);
        assert!(
            seen.upgrade().is_none(),
            "the binding holds the mob's store weakly"
        );
        // The read ends: nothing but this row holds the record.
        until(|| Arc::strong_count(&owners.inner) == 1).await;
        assert_eq!(Arc::strong_count(&owners.inner), 1);
    }

    /// The memory views of a mob the launch composes beside its launch mob
    /// hold that mob's members, and the launch's views hold the launch's:
    /// a member of one name in each mob is, in each mob's views, what it is
    /// in that mob.
    #[test]
    fn the_memory_members_of_a_mob_are_that_mobs_members() {
        let owners = PrivateOwners::default();
        owners.hosted_mobs().name("second");
        owners.record("launch", "desk", Some(&BTreeMap::new()));
        owners.record("launch", "side", Some(&private_labels()));
        owners.record("second", "desk", Some(&private_labels()));
        owners.record("second", "clerk", Some(&BTreeMap::new()));
        owners.inner.logged.write().unwrap().insert(
            ("second".to_string(), "ended".to_string()),
            Some(OWNER.to_string()),
        );

        assert_eq!(
            owners.memory_members_of("second"),
            BTreeMap::from([
                ("clerk".to_string(), None),
                ("desk".to_string(), Some(OWNER.to_string())),
                ("ended".to_string(), Some(OWNER.to_string())),
            ]),
            "the second mob's views hold the second mob's members"
        );
        assert_eq!(
            owners.memory_owners_of("second"),
            BTreeMap::from([
                ("desk".to_string(), OWNER.to_string()),
                ("ended".to_string(), OWNER.to_string()),
            ])
        );
        let of_the_launch = BTreeMap::from([
            ("desk".to_string(), None),
            ("side".to_string(), Some(OWNER.to_string())),
        ]);
        assert_eq!(
            owners.memory_members_of("launch"),
            of_the_launch,
            "the launch mob's views hold the launch mob's members"
        );
        assert_eq!(owners.memory_members(), of_the_launch);
        assert_eq!(
            owners.memory_owners(),
            BTreeMap::from([("side".to_string(), OWNER.to_string())])
        );
    }

    /// A member private to a person in one mob leaves the other mob's
    /// member of that name as it is. The second mob's memory recorded the
    /// retirement of a private member `desk`; the launch mob has an agent
    /// `desk` private to nobody, and its memory recorded the retirement of
    /// a private member `ledger`. Each mob's views and each mob's answer
    /// for a member the record does not hold name the retirements of that
    /// mob's own memory and no other's.
    #[tokio::test]
    async fn a_member_private_to_a_person_in_one_mob_leaves_the_other_mobs_member_of_that_name_as_it_is()
     {
        use crate::memory::capabilities::StewardStore as _;
        use crate::mob_announcements::MemberPerson;

        let private_to_someone =
            || MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
        let someone = || Some(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
        let (launch_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (of_the_launch, of_the_second) = (memory_store(&launch_dir), memory_store(&second_dir));
        of_the_launch
            .record_private_retirement("default", "ledger", None, "retired", 5)
            .await
            .unwrap();
        of_the_second
            .record_private_retirement("mob.second", "desk", None, "retired", 5)
            .await
            .unwrap();

        let owners = PrivateOwners::default();
        let second = meerkat_mob::MobId::from("second");
        owners.hosted_mobs().name(second.as_str());
        owners.record("launch", "desk", Some(&BTreeMap::new()));
        owners.bind_private_retirements(of_the_launch, "default".to_string());
        owners.bind_private_retirements_of(
            &second,
            of_the_second.clone(),
            "mob.second".to_string(),
        );
        owners
            .first_attempts_settled(std::time::Duration::from_secs(10))
            .await;
        until(|| {
            owners.person_of("second", "desk") == private_to_someone()
                && owners.person_of("launch", "ledger") == private_to_someone()
        })
        .await;

        assert_eq!(
            owners.person_of("second", "desk"),
            private_to_someone(),
            "the second mob's ended member is private to a person"
        );
        assert_eq!(
            owners.memory_members_of("second"),
            BTreeMap::from([("desk".to_string(), someone())]),
            "the second mob's views name the retirement its own memory recorded"
        );
        assert_eq!(
            owners.memory_members(),
            BTreeMap::from([
                ("desk".to_string(), None),
                ("ledger".to_string(), someone()),
            ]),
            "the launch mob's agent of that name is private to nobody, as it is"
        );
        assert_eq!(owners.person_of("launch", "desk"), MemberPerson::Shared);
        assert_eq!(owners.person_of("launch", "ledger"), private_to_someone());
        assert_eq!(
            owners.person_of("second", "ledger"),
            MemberPerson::NotKnown,
            "a retirement the launch's memory recorded names no member of the second mob"
        );
        // The second mob's memory lived until here: its read holds it weakly.
        drop(of_the_second);
    }

    /// The retirements a mob's own memory recorded go with the mob. Once a
    /// mob is created under its id (the record then forgets what it read
    /// of the earlier mob), a member under that id that the record does
    /// not hold is no longer private on the word of the earlier mob's
    /// memory, and the retirements the launch's memory recorded stand.
    #[tokio::test]
    async fn the_retirements_a_mobs_memory_recorded_are_forgotten_with_the_mob() {
        use crate::memory::capabilities::StewardStore as _;
        use crate::mob_announcements::MemberPerson;

        let private_to_someone =
            || MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
        let (launch_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (of_the_launch, of_the_second) = (memory_store(&launch_dir), memory_store(&second_dir));
        of_the_launch
            .record_private_retirement("default", "ledger", None, "retired", 5)
            .await
            .unwrap();
        of_the_second
            .record_private_retirement("mob.second", "desk", None, "retired", 5)
            .await
            .unwrap();
        let owners = PrivateOwners::default();
        let second = meerkat_mob::MobId::from("second");
        owners.hosted_mobs().name(second.as_str());
        owners.bind_private_retirements(of_the_launch, "default".to_string());
        owners.bind_private_retirements_of(
            &second,
            of_the_second.clone(),
            "mob.second".to_string(),
        );
        owners
            .first_attempts_settled(std::time::Duration::from_secs(10))
            .await;
        until(|| {
            owners.person_of("second", "desk") == private_to_someone()
                && owners.person_of("launch", "ledger") == private_to_someone()
        })
        .await;
        assert_eq!(owners.person_of("second", "desk"), private_to_someone());

        owners.forget_the_read_of("second");
        assert_eq!(
            owners.person_of("second", "desk"),
            MemberPerson::NotKnown,
            "the earlier mob's memory names no member of a mob under its id"
        );
        assert_eq!(owners.memory_members_of("second"), BTreeMap::new());
        assert_eq!(
            owners.person_of("launch", "ledger"),
            private_to_someone(),
            "the launch's memory's retirements stand"
        );
        // The second mob's memory lived until here: its read holds it weakly.
        drop(of_the_second);
    }

    /// On a host of one mob the memory views are as they were: every member
    /// of the launch mob and of the mobs its members created, by name, with
    /// the names its memory recorded as retired in private. Asked for any
    /// of those mobs the view is that one view, and a member the record
    /// does not hold whose retirement the memory recorded as private is
    /// private to a person in each of them.
    #[tokio::test]
    async fn the_memory_members_of_a_host_of_one_mob_are_as_before() {
        use crate::memory::capabilities::StewardStore as _;
        use crate::mob_announcements::MemberPerson;

        let dir = tempfile::tempdir().unwrap();
        let store = memory_store(&dir);
        store
            .record_private_retirement("default", "ledger", None, "retired", 5)
            .await
            .unwrap();
        let owners = PrivateOwners::default();
        owners.record("launch", "desk", Some(&BTreeMap::new()));
        owners.record("launch", "side", Some(&private_labels()));
        owners.record("made-by-a-member", "helper", Some(&private_labels()));
        owners.inner.logged.write().unwrap().insert(
            ("launch".to_string(), "ended".to_string()),
            Some(OWNER.to_string()),
        );
        owners.bind_private_retirements(store, "default".to_string());
        owners
            .first_attempts_settled(std::time::Duration::from_secs(10))
            .await;

        let as_before = BTreeMap::from([
            ("desk".to_string(), None),
            ("ended".to_string(), Some(OWNER.to_string())),
            ("helper".to_string(), Some(OWNER.to_string())),
            (
                "ledger".to_string(),
                Some(crate::access::UNKNOWN_PRIVATE_OWNER.to_string()),
            ),
            ("side".to_string(), Some(OWNER.to_string())),
        ]);
        assert_eq!(owners.memory_members(), as_before);
        for mob in ["launch", "made-by-a-member"] {
            assert_eq!(owners.memory_members_of(mob), as_before, "asked for {mob}");
            assert_eq!(
                owners.person_of(mob, "ledger"),
                MemberPerson::Private(crate::access::UNKNOWN_PRIVATE_OWNER.to_string()),
                "asked for {mob}"
            );
        }
        assert_eq!(
            owners.memory_owners_of("launch"),
            owners.memory_owners(),
            "and so are the owners"
        );
    }

    /// Which memory the members of a mob are members of is decided by what
    /// the launch said (its own mob, the mobs it composes beside it) and
    /// by recorded creations, case by case.
    #[test]
    fn the_memory_home_of_a_mob_follows_its_recorded_creation() {
        fn created_by(owners: &PrivateOwners, mob: &str, creating_mob: &str) {
            owners.record_creation(mob, MobCreation::ByAMemberOf(creating_mob.to_string()));
        }
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");
        owners.hosted_mobs().name("second");
        created_by(&owners, "of-launch", "launch");
        created_by(&owners, "of-of-launch", "of-launch");
        created_by(&owners, "of-second", "second");
        created_by(&owners, "of-of-second", "of-second");
        owners.record_creation("unowned", MobCreation::ByNoMember);
        owners.record_creation("undecided", MobCreation::Undecidable);
        created_by(&owners, "of-unowned", "unowned");
        created_by(&owners, "ring-a", "ring-b");
        created_by(&owners, "ring-b", "ring-a");
        // A line of mobs, each created by a member of the one before it:
        // `line-1` by a member of the launch mob.
        created_by(&owners, "line-1", "launch");
        for step in 2..=MEMORY_HOME_STEPS + 1 {
            created_by(
                &owners,
                &format!("line-{step}"),
                &format!("line-{}", step - 1),
            );
        }
        let within_the_bound = format!("line-{MEMORY_HOME_STEPS}");
        let past_the_bound = format!("line-{}", MEMORY_HOME_STEPS + 1);

        for (mob, home, what) in [
            ("launch", MemoryHome::Launch, "the launch mob"),
            (
                "second",
                MemoryHome::Hosted("second".to_string()),
                "a mob the launch composes beside its launch mob",
            ),
            (
                "of-launch",
                MemoryHome::Launch,
                "a mob of a launch mob's member",
            ),
            (
                "of-of-launch",
                MemoryHome::Launch,
                "a mob of a member of such a mob",
            ),
            (
                within_the_bound.as_str(),
                MemoryHome::Launch,
                "the last mob of a line the bound reaches the launch mob through",
            ),
            (
                "of-second",
                MemoryHome::Nowhere,
                "a mob of a member of a mob with a memory of its own",
            ),
            (
                "of-of-second",
                MemoryHome::Nowhere,
                "a mob of a member of such a mob",
            ),
            ("unowned", MemoryHome::NotKnown, "a mob no member created"),
            (
                "undecided",
                MemoryHome::NotKnown,
                "a mob whose creation does not say which member created it",
            ),
            (
                "of-unowned",
                MemoryHome::NotKnown,
                "a mob of a member of a mob no member created",
            ),
            (
                "never-read",
                MemoryHome::NotKnown,
                "a mob whose creation the record does not hold",
            ),
            (
                "ring-a",
                MemoryHome::NotKnown,
                "a mob whose line of creating mobs returns to itself",
            ),
            (
                past_the_bound.as_str(),
                MemoryHome::NotKnown,
                "a mob one step past the bound",
            ),
        ] {
            assert_eq!(owners.memory_home_of(mob), home, "{what}");
        }

        // A creation the record comes to hold is honoured at the next ask.
        created_by(&owners, "never-read", "of-launch");
        assert_eq!(owners.memory_home_of("never-read"), MemoryHome::Launch);

        // A mob forgotten and created anew under its id is what its new
        // creation says, and so are the mobs its members created.
        owners.forget_mob_in_tests("of-launch");
        assert_eq!(owners.memory_home_of("of-launch"), MemoryHome::NotKnown);
        owners.record_created_mob(
            "of-launch",
            MobOwner::Known(None),
            MobCreation::ByAMemberOf("second".to_string()),
        );
        for mob in ["of-launch", "of-of-launch", "never-read"] {
            assert_eq!(
                owners.memory_home_of(mob),
                MemoryHome::Nowhere,
                "{mob}, after its line was created anew by a member of the second mob"
            );
        }

        // The launch composed a mob that is gone, and a member of the
        // launch mob creates a mob under its id: the new mob is not one
        // the launch composes, and its members are in the launch's memory.
        owners.hosted_mobs().name("third");
        assert_eq!(
            owners.memory_home_of("third"),
            MemoryHome::Hosted("third".to_string())
        );
        owners.forget_mob_in_tests("third");
        owners.record_a_mob_created_for_a_session(
            "third",
            MobOwner::Known(None),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(owners.memory_home_of("third"), MemoryHome::Launch);
    }

    /// A record whose launch has not said which mob is its own decides
    /// nothing for a mob it does not name while it follows a mob set, and
    /// names every mob the launch's while it follows none: a runtime of
    /// one mob. Whether a mob has been named beside the launch mob yet
    /// does not change either answer.
    #[test]
    fn a_record_with_no_launch_mob_bound_never_takes_a_mob_for_the_launchs_because_none_is_named_yet()
     {
        for a_mob_is_named in [false, true] {
            let owners = PrivateOwners::default();
            if a_mob_is_named {
                owners.hosted_mobs().name("second");
            }
            assert_eq!(
                owners.memory_home_of("a-mob"),
                MemoryHome::Launch,
                "a record that follows no mob set (a mob named: {a_mob_is_named})"
            );
            // The record follows a mob set from here.
            owners.follow_a_mob_set_that_has_gone_in_tests();
            assert_eq!(
                owners.memory_home_of("a-mob"),
                MemoryHome::NotKnown,
                "a record that follows a mob set (a mob named: {a_mob_is_named})"
            );
            if a_mob_is_named {
                assert_eq!(
                    owners.memory_home_of("second"),
                    MemoryHome::Hosted("second".to_string())
                );
            }
            // Once the launch says which mob is its own, that mob is.
            owners.bind_launch_mob("a-mob");
            assert_eq!(owners.memory_home_of("a-mob"), MemoryHome::Launch);
        }
    }

    /// A log whose first event was recorded at a given time.
    struct LogFrom(Option<std::time::SystemTime>);

    #[async_trait::async_trait]
    impl MemberSpawnLog for LogFrom {
        async fn spawns_after(
            &self,
            _cursor: u64,
            _limit: usize,
        ) -> Result<Option<MemberSpawnPage>, String> {
            Ok(None)
        }

        async fn first_event_time(&self) -> Result<Option<std::time::SystemTime>, String> {
            Ok(self.0)
        }
    }

    /// Date the mob `mob` from `log`, as a reading pass that listed the mob
    /// now does: no create is admitted under the id since its note.
    async fn dated(owners: &PrivateOwners, mob: &str, log: &LogFrom) {
        let read = owners
            .date_the_mob(mob, log, &owners.creates_admitted())
            .await;
        assert_eq!(read, Ok(true), "{mob}");
    }

    /// The time the record keeps for a mob's id is the first event's of
    /// the mob that holds the id NOW: after a mob is created anew under
    /// the id, the later mob's, never an earlier one's. A mob this record
    /// has not dated has none.
    #[tokio::test]
    async fn the_time_the_record_keeps_for_a_mobs_id_is_the_first_event_of_the_mob_that_holds_it_now()
     {
        let at = |seconds: u64| {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
        };
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");
        owners.hold_the_creation_in_tests("helper", Some("launch"));

        // Not dated yet.
        assert_eq!(owners.first_event_of("helper"), None);

        // The mob that holds `helper` was created at 100.
        dated(&owners, "helper", &LogFrom(Some(at(100)))).await;
        assert_eq!(owners.first_event_of("helper"), Some(at(100)));

        // A mob is created anew under the id by a launch mob's member: the
        // earlier mob's time does not stand for it. The admission drops
        // the time before it writes the new creation, and says so itself
        // where it writes: the time kept here is what it is asked with.
        owners.record_created_mob(
            "helper",
            MobOwner::Known(None),
            MobCreation::ByAMemberOf("launch".to_string()),
        );
        assert_eq!(
            owners.first_event_of("helper"),
            None,
            "the earlier mob's time does not stand for the mob created under its id"
        );
        dated(&owners, "helper", &LogFrom(Some(at(200)))).await;
        assert_eq!(owners.first_event_of("helper"), Some(at(200)));

        // A read of a later mob's log that the record was not told a create
        // of takes the earlier time's place too: the later one's stands.
        dated(&owners, "helper", &LogFrom(Some(at(300)))).await;
        assert_eq!(owners.first_event_of("helper"), Some(at(300)));
        // A log that holds no event dates nothing and leaves the time kept.
        dated(&owners, "helper", &LogFrom(None)).await;
        assert_eq!(owners.first_event_of("helper"), Some(at(300)));
    }

    /// A pass that noted the creates before a mob was created under an id
    /// writes no time for the id, whichever mob it then reads: the earlier
    /// mob of the id, or the mob created since, which it cannot tell from
    /// it. It says so, so the mob is left unread, and the pass that lists
    /// the mob after the create dates it. A create under another id
    /// leaves the read's time as it is.
    #[tokio::test]
    async fn a_read_listed_before_a_create_under_the_id_does_not_date_the_mob_created_since() {
        let at = |seconds: u64| {
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
        };
        let created_by_a_launch_member = || MobCreation::ByAMemberOf("launch".to_string());
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");
        owners.hold_the_creation_in_tests("helper", Some("launch"));
        owners.hold_the_creation_in_tests("other", Some("launch"));

        // A reading pass notes the creates, then lists the mob that holds
        // `helper`, created at 100.
        let listed_before = owners.creates_admitted();
        // Meanwhile that mob is destroyed and a mob is created under each
        // of two ids.
        owners.record_created_mob(
            "helper",
            MobOwner::Known(None),
            created_by_a_launch_member(),
        );
        owners.record_created_mob(
            "elsewhere",
            MobOwner::Known(None),
            created_by_a_launch_member(),
        );
        // The pass's read of the earlier mob's log lands now.
        let of_the_earlier_mob = owners
            .date_the_mob("helper", &LogFrom(Some(at(100))), &listed_before)
            .await;
        assert_eq!(
            of_the_earlier_mob,
            Ok(false),
            "the pass is told a create was admitted under the id since its note"
        );
        assert_eq!(
            owners.first_event_of("helper"),
            None,
            "the earlier mob's time is not kept under the id of the mob created since"
        );
        // The same note with the mob created since, built before the pass
        // listed: its time is withheld too, and the pass is told so.
        let of_the_mob_created_since = owners
            .date_the_mob("helper", &LogFrom(Some(at(200))), &listed_before)
            .await;
        assert_eq!(of_the_mob_created_since, Ok(false));
        assert_eq!(owners.first_event_of("helper"), None);
        // The same pass dates a mob under whose id no create was admitted.
        let of_another_mob = owners
            .date_the_mob("other", &LogFrom(Some(at(100))), &listed_before)
            .await;
        assert_eq!(of_another_mob, Ok(true));
        assert_eq!(owners.first_event_of("other"), Some(at(100)));

        // The following pass notes after the create, lists the mob created
        // since and dates it.
        dated(&owners, "helper", &LogFrom(Some(at(200)))).await;
        assert_eq!(
            owners.first_event_of("helper"),
            Some(at(200)),
            "the time kept is the mob's created since, never the earlier mob's"
        );
    }

    /// A pass that noted the creates before a mob was created under an id
    /// does not mark that id read with the earlier mob's records: the id is
    /// not read, a member the record does not hold is not known, and a read
    /// that is owed for the id stays owed. Marks that landed before the
    /// create go with what the create forgets. A mob under whose id no
    /// create was admitted is marked read as before.
    #[test]
    fn a_read_listed_before_a_create_under_the_id_does_not_mark_the_mob_created_since_read() {
        use crate::mob_announcements::MemberPerson;
        let created_by_a_launch_member = || MobCreation::ByAMemberOf("launch".to_string());
        let incarnation = meerkat_core::service::MobIncarnationId::mint;
        let read_mark = |owners: &PrivateOwners, mob: &str| {
            let reads = owners.inner.mob_reads.0.borrow();
            (reads.read.contains(mob), reads.owed.contains(mob))
        };
        let owners = PrivateOwners::default();
        owners.bind_launch_mob("launch");

        // A reading pass notes the creates, then lists the mobs that hold
        // three ids.
        let listed_before = owners.creates_admitted();

        // Under `helper` the earlier mob goes and a mob is created: the
        // admission forgets what the record read under the id and records
        // the new mob; the host owes the read of the mob it builds.
        owners.forget_the_read_of("helper");
        owners.record_created_mob(
            "helper",
            MobOwner::Known(None),
            created_by_a_launch_member(),
        );
        owners.owe_the_read_of("helper");
        // The pass's read of the earlier mob's records lands now.
        owners.note_the_mob_read(
            "helper",
            incarnation(),
            MobOwner::Known(None),
            created_by_a_launch_member(),
            &listed_before,
        );
        assert_eq!(
            read_mark(&owners, "helper"),
            (false, true),
            "the id is not marked read, and the read owed for the mob created since stays owed"
        );
        assert!(!owners.has_read_the_mob("helper"));
        assert_eq!(
            owners.person_for_a_reader("helper", "nobody"),
            MemberPerson::NotKnown,
            "a member the record does not hold is not known until the new mob is read"
        );

        // Under `landed-first` the pass's marks land before the create is
        // admitted: the create forgets them.
        owners.note_the_mob_read(
            "landed-first",
            incarnation(),
            MobOwner::Known(None),
            created_by_a_launch_member(),
            &listed_before,
        );
        assert_eq!(read_mark(&owners, "landed-first"), (true, false));
        owners.record_created_mob(
            "landed-first",
            MobOwner::Known(None),
            created_by_a_launch_member(),
        );
        assert_eq!(
            read_mark(&owners, "landed-first"),
            (false, false),
            "what the pass marked before the create is forgotten where the create is counted"
        );

        // No create is admitted under `other`: the same pass marks it read.
        owners.note_the_mob_read(
            "other",
            incarnation(),
            MobOwner::Known(None),
            created_by_a_launch_member(),
            &listed_before,
        );
        assert_eq!(read_mark(&owners, "other"), (true, false));
    }
}
