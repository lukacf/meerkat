//! The lineage of a mob and of a member across the hosted set: who seated a
//! member, who created its mob, and whether each recorded creator is still
//! there, followed until a host's own declaration or closed.
//!
//! The read is a fact for readers to weigh: it allows nothing by itself. It
//! reads the mobs that are loaded and what the set keeps of the others, and
//! it never loads a mob: a parent that is hosted and not loaded closes the
//! walk. Everything unknown closes it. No name, label or argument of a call
//! is an input.

use meerkat_core::SessionId;
use meerkat_mob::{
    AgentIdentity, LineageClosed, MemberLineageStep, MemberLink, MemberLinkProof,
    MemberSeatCreator, MobHandle, MobId, MobLineageCause, MobProvenance, RecordedSeat, SeatNow,
    SeatSealer,
};

use crate::MobMcpState;

/// The most steps one lineage walk takes before it closes
/// ([`LineageClosed::Bound`]).
pub const LINEAGE_STEP_BOUND: usize = 16;

/// One step of a lineage walk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineageStep {
    /// The member's lineage is its mob's own.
    MobsOwn {
        mob_id: MobId,
        identity: AgentIdentity,
        cause: MobLineageCause,
    },
    /// The member's lineage continues at the member `to` of `to_mob_id`.
    /// `now` is the proof of that member's recorded seat or creation.
    Linked {
        mob_id: MobId,
        identity: AgentIdentity,
        link: MemberLink,
        to_mob_id: MobId,
        to: AgentIdentity,
        now: Option<SeatNow>,
    },
    /// The mob was created by, or for the session of, the member `creator`
    /// of `parent_mob_id`; `now` is the proof of that member's recorded seat.
    MobCreatedBy {
        mob_id: MobId,
        parent_mob_id: MobId,
        creator: AgentIdentity,
        now: SeatNow,
    },
}

/// Where a lineage walk ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LineageEnd {
    /// At a host's own declaration: a mob the host created.
    HostDeclared,
    /// Closed before one.
    Closed(LineageClosed),
}

/// A lineage: its steps in order, and where it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Lineage {
    pub steps: Vec<LineageStep>,
    pub end: LineageEnd,
}

impl Lineage {
    /// Whether the walk ended at a host's own declaration.
    #[must_use]
    pub fn is_host_declared(&self) -> bool {
        self.end == LineageEnd::HostDeclared
    }
}

/// A mob as the set holds it now, for a read that loads nothing.
enum HostedNow {
    /// An actor runs for it: its handle.
    Loaded(Box<MobHandle>),
    /// The set knows it and no actor runs for it.
    NotLoaded,
    /// The set holds no mob of the id.
    NotHosted,
}

/// Where a walk stands: at a member of a mob, or at a mob's own creation
/// record.
enum At {
    Member(MobId, AgentIdentity),
    Mob(MobId),
}

/// The steps a walk took so far.
#[derive(Default)]
struct Walk {
    steps: Vec<LineageStep>,
}

impl Walk {
    fn ends(self, end: LineageEnd) -> Lineage {
        Lineage {
            steps: self.steps,
            end,
        }
    }

    fn closes(self, why: LineageClosed) -> Lineage {
        self.ends(LineageEnd::Closed(why))
    }

    /// Take one more step; `false` when the walk is at its bound.
    fn steps_on(&mut self, step: LineageStep) -> bool {
        if self.steps.len() >= LINEAGE_STEP_BOUND {
            return false;
        }
        self.steps.push(step);
        true
    }
}

/// What a proof of a recorded seat means for a walk: `None` when the walk
/// goes on at that seat's member.
fn closed_by(now: SeatNow) -> Option<LineageClosed> {
    match now {
        SeatNow::Current | SeatNow::SameLife => None,
        SeatNow::Gone => Some(LineageClosed::CreatorGone),
        SeatNow::Unreadable => Some(LineageClosed::Unreadable),
        _ => Some(LineageClosed::UnknownKind),
    }
}

impl MobMcpState {
    /// The lineage of the mob `mob_id`: its creation record, and for a mob a
    /// member's session created (or the host created for it) that member's
    /// lineage in turn.
    ///
    /// A mob that is not loaded is read from what the set keeps of its
    /// creation record; nothing is loaded.
    pub async fn mob_lineage(&self, mob_id: &MobId) -> Lineage {
        self.walk_lineage(At::Mob(mob_id.clone())).await
    }

    /// The lineage of the member seated under `member` in the mob `mob_id`.
    ///
    /// The member's own mob must be loaded: the read loads nothing, and a
    /// mob that is hosted and not loaded closes it. The read spans every
    /// hosted mob, so it answers the owner's console only: any other
    /// console's read closes ([`LineageClosed::Unreadable`]).
    pub async fn lineage_of(&self, mob_id: &MobId, member: &AgentIdentity) -> Lineage {
        self.walk_lineage(At::Member(mob_id.clone(), member.clone()))
            .await
    }

    /// The seat of the member the session `session` serves, sealed for the
    /// seat record of a member a call of that session seats
    /// ([`meerkat_mob::SpawnMemberSpec::with_seat_creator`]).
    ///
    /// One complete lookup of the hosted rosters that name the session
    /// ([`Self::session_member_matches`]): exactly one mob, and that mob's own
    /// handle seals the member from its machine's bindings; none, and the
    /// bare session is named; several, and none is picked. A lookup that
    /// cannot be completed, a mob that is not loaded, and a seal that is not
    /// the member the lookup found seal nothing. `session` is the session
    /// the runtime runs the call for, never one the call names. The answer
    /// allows nothing: the spawn is admitted whatever it is.
    pub(crate) async fn seat_creator_of(&self, session: &SessionId) -> MemberSeatCreator {
        let Ok(matches) = self.session_member_matches(session).await else {
            return MemberSeatCreator::unavailable();
        };
        let found = match matches.as_slice() {
            [] => return MemberSeatCreator::no_roster(session.clone()),
            [found] => found,
            _ => return MemberSeatCreator::several(),
        };
        let HostedNow::Loaded(handle) = self.hosted_now(&found.mob_id).await else {
            return MemberSeatCreator::unavailable();
        };
        let sealed = handle.seal_seat_creator(session, SeatSealer::Set).await;
        match sealed.recorded() {
            Some(recorded)
                if recorded.creator_identity == found.identity
                    && recorded.creator_generation == found.generation
                    && recorded.parent_incarnation == found.incarnation =>
            {
                sealed
            }
            _ => MemberSeatCreator::unavailable(),
        }
    }

    /// The mob `mob_id` as the set holds it now. Nothing is loaded.
    async fn hosted_now(&self, mob_id: &MobId) -> HostedNow {
        let hosted = self.hosted.read().await;
        let loaded = hosted
            .loaded_handles()
            .find_map(|(id, handle)| (id == mob_id).then(|| handle.clone()));
        match loaded {
            Some(handle) => HostedNow::Loaded(Box::new(handle)),
            None if hosted.knows(mob_id) => HostedNow::NotLoaded,
            None => HostedNow::NotHosted,
        }
    }

    /// The creation record of the mob `mob_id`, read without loading it: the
    /// loaded mob's own, or what the set keeps of a mob that is not loaded.
    async fn creation_record_now(&self, mob_id: &MobId) -> Result<MobProvenance, LineageClosed> {
        match self.hosted_now(mob_id).await {
            HostedNow::Loaded(handle) => handle
                .provenance()
                .await
                .map_err(|_| LineageClosed::Unreadable),
            HostedNow::NotLoaded => self
                .hosted
                .read()
                .await
                .creation_of_unloaded(mob_id)
                .ok_or(LineageClosed::NotLoaded),
            HostedNow::NotHosted => Err(LineageClosed::NotHosted),
        }
    }

    /// The proof of `recorded` by its own mob, which must be loaded.
    async fn recorded_seat_now(&self, recorded: &RecordedSeat) -> Result<SeatNow, LineageClosed> {
        match self.hosted_now(&recorded.parent_mob_id).await {
            HostedNow::Loaded(parent) => Ok(parent.seat_now(recorded).await),
            HostedNow::NotLoaded => Err(LineageClosed::NotLoaded),
            HostedNow::NotHosted => Err(LineageClosed::NotHosted),
        }
    }

    async fn walk_lineage(&self, mut at: At) -> Lineage {
        let mut walk = Walk::default();
        // The read spans every hosted mob: it needs the console owner, as
        // the set's inventory does, and the restored set.
        if self
            .require_console_owner(meerkat_mob::ControlScope::List)
            .is_err()
            || self.ensure_restored().await.is_err()
        {
            return walk.closes(LineageClosed::Unreadable);
        }
        loop {
            at = match at {
                At::Member(mob_id, identity) => {
                    let handle = match self.hosted_now(&mob_id).await {
                        HostedNow::Loaded(handle) => handle,
                        HostedNow::NotLoaded => return walk.closes(LineageClosed::NotLoaded),
                        HostedNow::NotHosted => return walk.closes(LineageClosed::NotHosted),
                    };
                    match handle.member_lineage_step(&identity).await {
                        MemberLineageStep::Closed(why) => return walk.closes(why),
                        MemberLineageStep::Mob(cause) => {
                            if !walk.steps_on(LineageStep::MobsOwn {
                                mob_id: mob_id.clone(),
                                identity,
                                cause,
                            }) {
                                return walk.closes(LineageClosed::Bound);
                            }
                            At::Mob(mob_id)
                        }
                        MemberLineageStep::Member {
                            mob_id: to_mob_id,
                            identity: to,
                            link,
                            proof,
                        } => {
                            // No link is followed by its name alone: a seat
                            // and a creation are proven on the mob the link
                            // names.
                            let now = match &proof {
                                MemberLinkProof::Seat(recorded) => {
                                    match self.recorded_seat_now(recorded).await {
                                        Ok(now) => Some(now),
                                        Err(why) => return walk.closes(why),
                                    }
                                }
                                MemberLinkProof::Creation(creation) => {
                                    match self.hosted_now(&to_mob_id).await {
                                        HostedNow::Loaded(source) => {
                                            Some(source.creation_now(&to, *creation).await)
                                        }
                                        HostedNow::NotLoaded => {
                                            return walk.closes(LineageClosed::NotLoaded);
                                        }
                                        HostedNow::NotHosted => {
                                            return walk.closes(LineageClosed::NotHosted);
                                        }
                                    }
                                }
                                _ => return walk.closes(LineageClosed::UnknownKind),
                            };
                            if !walk.steps_on(LineageStep::Linked {
                                mob_id,
                                identity,
                                link,
                                to_mob_id: to_mob_id.clone(),
                                to: to.clone(),
                                now,
                            }) {
                                return walk.closes(LineageClosed::Bound);
                            }
                            if let Some(why) = now.and_then(closed_by) {
                                return walk.closes(why);
                            }
                            At::Member(to_mob_id, to)
                        }
                        _ => return walk.closes(LineageClosed::UnknownKind),
                    }
                }
                At::Mob(mob_id) => {
                    let provenance = match self.creation_record_now(&mob_id).await {
                        Ok(provenance) => provenance,
                        Err(why) => return walk.closes(why),
                    };
                    match provenance {
                        MobProvenance::Host => return walk.ends(LineageEnd::HostDeclared),
                        MobProvenance::Member {
                            parent_mob_id,
                            parent_incarnation,
                            creator_identity,
                            creator_generation,
                            owner_session,
                        }
                        | MobProvenance::OperatorMember {
                            parent_mob_id,
                            parent_incarnation,
                            creator_identity,
                            creator_generation,
                            owner_session,
                        } => {
                            let recorded = RecordedSeat::new(
                                parent_mob_id.clone(),
                                parent_incarnation,
                                creator_identity.clone(),
                                creator_generation,
                                owner_session,
                            );
                            let now = match self.recorded_seat_now(&recorded).await {
                                Ok(now) => now,
                                Err(why) => return walk.closes(why),
                            };
                            if !walk.steps_on(LineageStep::MobCreatedBy {
                                mob_id,
                                parent_mob_id: parent_mob_id.clone(),
                                creator: creator_identity.clone(),
                                now,
                            }) {
                                return walk.closes(LineageClosed::Bound);
                            }
                            if let Some(why) = closed_by(now) {
                                return walk.closes(why);
                            }
                            At::Member(parent_mob_id, creator_identity)
                        }
                        MobProvenance::Session { .. } | MobProvenance::OperatorSession { .. } => {
                            return walk.closes(LineageClosed::SessionNoRoster);
                        }
                        MobProvenance::Ambiguous { .. } => {
                            return walk.closes(LineageClosed::Ambiguous);
                        }
                        MobProvenance::Unrecorded => {
                            return walk.closes(LineageClosed::Unrecorded);
                        }
                        _ => return walk.closes(LineageClosed::UnknownKind),
                    }
                }
            };
        }
    }
}
