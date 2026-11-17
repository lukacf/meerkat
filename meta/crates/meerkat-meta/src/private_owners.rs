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
//! records its own labels (the base session layer's pre-build hook,
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
}

#[async_trait::async_trait]
impl MemberSpawnLog for meerkat_mob::MobHandle {
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

/// The owner of each member of one runtime's mobs (see the module docs).
/// Cheap to clone; clones share the record.
#[derive(Clone, Default)]
pub(crate) struct PrivateOwners {
    inner: Arc<Inner>,
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
}

#[derive(Default)]
struct Inner {
    /// Which record this is among the records of the process.
    id: PrivateOwnerRecordId,
    /// What each member build recorded: the member's owner, or `None` for a
    /// member private to no one. The newest truth about a member.
    built: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// What the event logs say each member's last spawn left it: its owner,
    /// or `None` for a member private to no one.
    logged: RwLock<BTreeMap<MemberKey, Option<String>>>,
    /// The identities, as their memory scopes name them, whose retirement
    /// the memory store recorded as private: private to a person, though
    /// not to whom when no record above names the owner.
    retired: RwLock<BTreeSet<String>>,
    /// What is known of who each mob a member created is private to
    /// ([`MobOwner`]).
    mob_owners: RwLock<BTreeMap<String, MobOwner>>,
    /// Who held each member name over time ([`NameHolders`]), keyed by the
    /// mob and the identity the member's memory scope names it by.
    holders: RwLock<BTreeMap<MemberKey, NameHolders>>,
    /// The times of each mob's journal rows `holders` were noted from
    /// ([`JournalTimes`]), by mob. Locked before `holders`.
    journal_times: RwLock<BTreeMap<String, JournalTimes>>,
    /// How many journal rows `holders` have noted: it changes with them.
    holders_noted: std::sync::atomic::AtomicU64,
    /// Set once the seed landed: every event log and creation record read.
    seeded: tokio::sync::OnceCell<()>,
    /// The seed's stage among the first reads this record owes.
    seed_stage: std::sync::Mutex<SeedStage>,
    /// How far the first reads this record owes have come.
    first_reads: FirstReadsSignal,
    /// The agent mob tool state of the runtime, whose mobs the seed reads.
    child_mobs: RwLock<Option<Weak<meerkat_mob_mcp::MobMcpState>>>,
    /// Who follows the end of a member private to a person, and the ends
    /// held for it.
    ends: std::sync::Mutex<MemberEnds>,
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

/// The refusal of a spawn into a mob whose creation record does not say
/// which member created it. The record never changes, so the refusal stands
/// for as long as the mob does.
fn undecidable_mob_owner(mob_id: &str) -> MobError {
    MobError::WiringError(format!(
        "who mob '{mob_id}' is private to cannot be read: its creation record does not say \
         which member created it (several rosters named the session that created it, or the \
         record is of a kind this release does not know), so no member can be spawned into it"
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

/// The wait before the next try of a durable read that failed, growing to
/// [`SEED_RETRY_MAX`].
fn next_wait(wait: std::time::Duration) -> std::time::Duration {
    (wait * 2).min(SEED_RETRY_MAX)
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

    /// Every member this record knows, keyed by the identity its memory
    /// scope names it by ([`crate::member_comms_id::logical_memory_identity`]),
    /// with its owner, or `None` for a member private to no one. A member
    /// two records know of is private when either says so; an identity the
    /// memory store recorded as private whose owner no record names is
    /// private to [`crate::access::UNKNOWN_PRIVATE_OWNER`].
    pub(crate) fn memory_members(&self) -> BTreeMap<String, Option<String>> {
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
            {
                let identity = crate::member_comms_id::logical_memory_identity(member);
                let entry = members.entry(identity).or_default();
                if entry.is_none() {
                    entry.clone_from(owner);
                }
            }
        }
        for identity in self
            .inner
            .retired
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            members
                .entry(identity.clone())
                .or_default()
                .get_or_insert_with(|| crate::access::UNKNOWN_PRIVATE_OWNER.to_string());
        }
        members
    }

    /// The owner of every private member this record knows, keyed by the
    /// identity its memory scope names it by.
    pub(crate) fn memory_owners(&self) -> BTreeMap<String, String> {
        self.memory_members()
            .into_iter()
            .filter_map(|(identity, owner)| owner.map(|owner| (identity, owner)))
            .collect()
    }

    /// The owner of mob `mob_id`, which a member created: `None` for a mob
    /// private to no one. A mob this record does not know yet is refused as
    /// retryable ([`unknown_mob_owner`]); one whose creation record does not
    /// say which member created it is refused for good
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
            None => Err(unknown_mob_owner(mob_id)),
        }
    }

    /// Record who mob `mob_id` is private to. The first record of a mob
    /// stands: its creator's owner does not change.
    fn record_mob(&self, mob_id: &str, owner: MobOwner) {
        self.inner
            .mob_owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(mob_id.to_string())
            .or_insert(owner);
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
                self.read_log(log).await?;
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
    /// each failure, up to [`SEED_RETRY_MAX`]. The task holds an observer of
    /// the mob, which keeps no mob open, and ends with the seed.
    pub(crate) fn seed_in_background(&self, handle: &meerkat_mob::MobHandle) {
        // Owed from this moment, before the task first runs: a reader that
        // comes first finds the read in flight.
        self.owe_seed();
        let owners = self.clone();
        let handle = handle.observer();
        tokio::spawn(async move {
            let mut wait = std::time::Duration::from_millis(250);
            while !owners.seeded(&handle).await.landed() {
                tokio::time::sleep(wait).await;
                wait = next_wait(wait);
            }
        });
    }

    /// Read, in the background and until the read succeeds, the identities
    /// whose retirement `store` recorded as private in `realm`.
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
                        // The store also marks the key of every identity
                        // scope it keeps apart from the members of a name
                        // (`crate::memory::records::IdentityScopeKey`) as
                        // private. Such a key is a scope, not a member's
                        // name: only plain names are members here.
                        owners
                            .inner
                            .retired
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend(identities.into_iter().filter(|identity| {
                                crate::memory::records::IdentityScopeKey::of(identity).is_named()
                            }));
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

    async fn read_log(&self, log: &dyn MemberSpawnLog) -> Result<(), String> {
        let mut logged = BTreeMap::new();
        let mut cursor = 0;
        while let Some((steps, next)) = log.spawns_after(cursor, SEED_PAGE).await? {
            cursor = next;
            for step in steps {
                self.note_journal_step(&step);
                if let HolderStep::Seated { owner, .. } = step.step {
                    logged.insert((step.mob_id, step.member), owner);
                }
            }
        }
        self.inner
            .logged
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(logged);
        Ok(())
    }

    /// Read every mob the agent mob tools host: its event log first (so the
    /// owners of the members that created mobs are known), then who created
    /// it (its durable creation record).
    async fn read_child_mobs(&self, state: &meerkat_mob_mcp::MobMcpState) -> Result<(), String> {
        let mobs = state.mob_list().await.map_err(|error| error.to_string())?;
        let mut creations = Vec::with_capacity(mobs.len());
        for (mob_id, _) in mobs {
            let handle = match state.handle_for(&mob_id).await {
                Ok(handle) => handle,
                // A mob destroyed or held while the seed reads: no member of
                // it spawns, so it needs no owner now.
                Err(MobError::MobNotFound(_)) => continue,
                Err(error) => return Err(error.to_string()),
            };
            self.read_log(&handle).await?;
            let provenance = handle
                .provenance()
                .await
                .map_err(|error| error.to_string())?;
            creations.push((mob_id, provenance));
        }
        for (mob_id, provenance) in creations {
            let owner = match creating_member(&provenance) {
                CreatingMember::Named(parent, creator) => {
                    MobOwner::Known(self.owner_of(parent.as_str(), creator.as_str()))
                }
                CreatingMember::NoMember => MobOwner::Known(None),
                CreatingMember::Undecidable => MobOwner::Undecidable,
            };
            self.record_mob(mob_id.as_str(), owner);
        }
        Ok(())
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

    /// Record what the roster of `handle`'s mob holds for `member`, as its
    /// build would: for a member this process may not have built, before a
    /// spawn reads its owner (the source of a prompt-context fork).
    pub(crate) async fn record_roster_member(
        &self,
        handle: &meerkat_mob::MobHandle,
        member: &meerkat_mob::AgentIdentity,
    ) {
        if let Ok(Some(entry)) = handle.get_member(member).await {
            self.record(
                handle.mob_id().as_str(),
                member.as_str(),
                Some(&entry.labels),
            );
        }
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
    pub(crate) async fn observe_mob_event(
        &self,
        handle: &meerkat_mob::MobHandle,
        event: &meerkat_mob::MobEvent,
    ) -> bool {
        // Who holds a member's name is followed for every member: a later
        // seating under a name starts another member's time.
        if let Some(step) = MemberJournalStep::of_event(event) {
            self.note_journal_step(&step);
            if step.step == HolderStep::Removed {
                self.private_agent_ended(&step, event.timestamp).await;
            }
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
    /// and rebuild passes.
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
                        owners.record(&binding.mob_id, &binding.member, req.labels.as_ref());
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
    /// when there is one) decides first, then the mob is recorded as private
    /// to its creator's owner. A creator whose owner cannot be read refuses
    /// the create.
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
    /// a restart is read as private to no one.
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
                let owner = match (creator_session, state.upgrade()) {
                    (Some(session), Some(state)) => {
                        match state.member_for_bridge_session(session).await {
                            Ok(Some((mob_id, creator))) => {
                                owners.owner_of(mob_id.as_str(), creator.as_str())
                            }
                            Ok(None) => None,
                            Err(error) => {
                                return Err(format!(
                                    "who creates mob '{}' could not be read: {error}",
                                    request.mob_id
                                ));
                            }
                        }
                    }
                    _ => None,
                };
                owners.record_mob(request.mob_id.as_str(), MobOwner::Known(owner));
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

    /// The Steward's source of the members private to a person in
    /// `handle`'s mob and the mobs its members created: every one this
    /// record knows, once the seed landed (until then the Steward dreams
    /// over nothing). It holds an observer of the mob, which keeps no mob
    /// open.
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
    /// member's memory scope spells it), one list per mob.
    pub(crate) fn name_seatings(&self) -> BTreeMap<String, Vec<NameSeatings>> {
        let mut names: BTreeMap<String, Vec<NameSeatings>> = BTreeMap::new();
        for ((_, name), holders) in self
            .inner
            .holders
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            names
                .entry(name.clone())
                .or_default()
                .push(holders.seatings());
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
        if !self.owners.seeded(&self.handle).await.landed() {
            return Err(
                "who the members are private to has not been read yet; the next dream reads it"
                    .to_string(),
            );
        }
        Ok(self.owners.memory_owners().into_keys().collect())
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
        let gamma = seatings.get("gamma").and_then(|lists| lists.first());
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
}
