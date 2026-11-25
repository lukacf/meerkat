//! Who seated a member, and whether that seat still stands.
//!
//! A member's seat record says who put the member in its place: the host,
//! the mob's own definition, another member's call, a schedule. It is a
//! journal fact of the member's life: the runtime's own successor of a
//! member (a respawn, a rotation) keeps it, and a member seated later under
//! the same name has its own. The proof ([`MobHandle::seat_now`]) answers
//! whether a recorded seat is still the one its mob holds.
//!
//! These are facts, not permissions: nothing in this module allows or
//! refuses anything. Every answer is closed where a fact is missing: a seat
//! whose row says nothing reads [`MemberSeatKind::Unrecorded`], and a seat
//! that cannot be proven reads [`SeatNow::Gone`] or [`SeatNow::Unreadable`],
//! never a guess. No name, label or argument of a call is an input: the
//! inputs are what the journal's rows and MobMachine's state hold.

use meerkat_core::SessionId;
use meerkat_core::service::MobIncarnationId;

use super::MobHandle;
use crate::MobError;
use crate::ids::{AgentIdentity, Generation, MobId};

/// Who seated a member's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MemberSeatKind {
    /// The host's own call seated it.
    Host,
    /// The host seated it for a caller session that a member of a hosted mob
    /// holds: the host decided, and [`MemberSeat::recorded`] names that
    /// member.
    HostForMember,
    /// The host seated it for a caller session that no hosted roster names.
    HostForSession,
    /// A member's own call seated it; [`MemberSeat::recorded`] names that
    /// member's seat as it stood then.
    Member,
    /// A call of a session that no hosted roster names seated it.
    Session,
    /// Several hosted rosters named the calling session: none is picked.
    Ambiguous,
    /// The mob's own definition seated it (a flow step, a spawn policy).
    Definition,
    /// A copy of another member that no member's own turn asked for.
    ForkOf,
    /// A schedule's fire seated it.
    Scheduled,
    /// Who seated it could not be shown.
    Unproven,
    /// The seat's row holds no record, and its older members do not say.
    Unrecorded,
}

/// The seat of the member that seated another, as it stood at that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecordedSeat {
    /// The mob the creating member sat in.
    pub parent_mob_id: MobId,
    /// That mob's incarnation.
    pub parent_incarnation: MobIncarnationId,
    /// The creating member.
    pub creator_identity: AgentIdentity,
    /// The generation it sat at.
    pub creator_generation: Generation,
    /// The session it acted on.
    pub owner_session: SessionId,
    /// The creating identity's life marker in its mob, when the record holds
    /// one ([`super::IdentityLife`]): what [`SeatNow::SameLife`] compares.
    pub creator_life: Option<super::IdentityLife>,
    /// The creating member's creation id, when the record holds one: a key
    /// for what a reader keeps under that member. The proof never compares
    /// it.
    pub creator_creation: Option<crate::MemberCreationId>,
}

impl RecordedSeat {
    /// A recorded seat of its five parts, with no life marker and no creation
    /// id: what a mob's creation record holds of its creating member.
    #[must_use]
    pub fn new(
        parent_mob_id: MobId,
        parent_incarnation: MobIncarnationId,
        creator_identity: AgentIdentity,
        creator_generation: Generation,
        owner_session: SessionId,
    ) -> Self {
        Self {
            parent_mob_id,
            parent_incarnation,
            creator_identity,
            creator_generation,
            owner_session,
            creator_life: None,
            creator_creation: None,
        }
    }

    /// This recorded seat with the creating identity's life marker.
    #[must_use]
    pub(crate) fn with_creator_life(mut self, life: super::IdentityLife) -> Self {
        self.creator_life = Some(life);
        self
    }
}

/// Who wrote a seat record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SeatRecorder {
    /// The runtime's seat step wrote it from what it observed.
    Shell,
    /// MobMachine recorded it.
    Machine,
}

/// A member's seat record ([`MobHandle::member_seat`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberSeat {
    kind: MemberSeatKind,
    recorded: Option<RecordedSeat>,
    own_turn: bool,
    recorded_by: Option<SeatRecorder>,
    sealed_by: Option<SeatSealer>,
    session: Option<SessionId>,
}

impl MemberSeat {
    /// A seat of `kind` read from a row that holds no record: it names no
    /// creating seat.
    pub(crate) fn from_older_row(kind: MemberSeatKind, own_turn: bool) -> Self {
        Self {
            kind,
            recorded: None,
            own_turn,
            recorded_by: None,
            sealed_by: None,
            session: None,
        }
    }

    /// A record the runtime's seat step writes for a seat of `kind` that
    /// names no creating seat.
    pub(crate) fn shell(kind: MemberSeatKind) -> Self {
        Self {
            recorded_by: Some(SeatRecorder::Shell),
            ..Self::from_older_row(kind, false)
        }
    }

    /// A record the runtime's seat step writes for a seat of `kind` whose
    /// creating member's seat `recorded` was sealed by `sealed_by`.
    pub(crate) fn shell_with_creator(
        kind: MemberSeatKind,
        recorded: RecordedSeat,
        sealed_by: SeatSealer,
        own_turn: bool,
    ) -> Self {
        Self {
            recorded: Some(recorded),
            sealed_by: Some(sealed_by),
            own_turn,
            ..Self::shell(kind)
        }
    }

    /// A record the runtime's seat step writes for a seat of `kind` asked
    /// for by `session`, a session no hosted roster names.
    pub(crate) fn shell_for_session(kind: MemberSeatKind, session: SessionId) -> Self {
        Self {
            session: Some(session),
            ..Self::shell(kind)
        }
    }

    /// The session that asked for the seat, for a kind whose caller is a
    /// session no hosted roster names.
    pub(crate) fn caller_session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// Who seated the member.
    #[must_use]
    pub fn kind(&self) -> MemberSeatKind {
        self.kind
    }

    /// The creating member's seat, for a kind that has one. A seat read from
    /// a row that holds no record names none: its creator is not proven.
    #[must_use]
    pub fn recorded(&self) -> Option<&RecordedSeat> {
        self.recorded.as_ref()
    }

    /// Whether the creating member's own turn made this member as a copy of
    /// itself.
    #[must_use]
    pub fn own_turn(&self) -> bool {
        self.own_turn
    }

    /// Who wrote the record; `None` for a seat whose row holds no record.
    #[must_use]
    pub fn recorded_by(&self) -> Option<SeatRecorder> {
        self.recorded_by
    }
}

/// Whether a recorded seat is still the one its mob holds
/// ([`MobHandle::seat_now`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SeatNow {
    /// The recorded session serves the recorded identity at the recorded
    /// generation, or at a later one that same-session successor steps alone
    /// reached. A session's own grant accepts this answer alone.
    Current,
    /// A member is seated now under the recorded identity, and the
    /// identity's life marker is the recorded one: the runtime's own
    /// successor steps alone lie between (a respawn or a rotation onto
    /// another session).
    SameLife,
    /// The recorded seat is not there: another incarnation of the mob, an
    /// identity whose life ended, no member seated under it.
    Gone,
    /// A fact the answer needs could not be read.
    Unreadable,
}

/// Why a lineage read closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LineageClosed {
    /// Who seated the member could not be shown.
    Unproven,
    /// The member's seat row holds no record.
    Unrecorded,
    /// Several hosted rosters named the calling session.
    Ambiguous,
    /// A session that no hosted roster names.
    SessionNoRoster,
    /// A schedule's fire seated the member.
    Scheduled,
    /// The seat names a creating member that its row cannot prove.
    CreatorNotProven,
    /// The creating member's seat is gone.
    CreatorGone,
    /// A fact the read needs could not be read.
    Unreadable,
    /// The mob is hosted and not loaded; the read loads nothing.
    NotLoaded,
    /// No hosted mob of that id and incarnation.
    NotHosted,
    /// A kind this build does not know.
    UnknownKind,
    /// The identity's record names a requester that is not verified.
    UnverifiedRequester,
    /// No member is seated under the name.
    NoMember,
    /// The walk reached its bound.
    Bound,
}

/// Why a member's lineage is its mob's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MobLineageCause {
    /// The identity is a declared entry of its mob. The entry's origin does
    /// not record WHO declared it: a declaration made through any surface
    /// that ensures a member reads the same. A reader must not take an
    /// entry as declared by the host from this cause alone.
    DeclaredEntry,
    /// The identity was created at the host's or a person's request, or with
    /// their recorded consent.
    AuthorizedEntry,
    /// The host's own call seated the member.
    HostSeat,
    /// The mob's definition seated the member.
    DefinitionSeat,
}

/// How a member's lineage continues at another member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MemberLink {
    /// That member's own call seated it.
    SeatedBy,
    /// The host seated it for that member's session.
    SeatedFor,
    /// It is a copy of that member.
    ForkOf,
}

/// Where a member's lineage goes next, read in the member's own mob
/// ([`MobHandle::member_lineage_step`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberLineageStep {
    /// It is its mob's own lineage.
    Mob(MobLineageCause),
    /// It continues at the member `identity` of the mob `mob_id`, once the
    /// reader has proven `proof` on that mob: a name alone is never followed.
    Member {
        mob_id: MobId,
        identity: AgentIdentity,
        link: MemberLink,
        proof: MemberLinkProof,
    },
    /// It closes here.
    Closed(LineageClosed),
}

/// What a reader proves, on the mob a member link names, before it follows
/// the link ([`MemberLineageStep::Member`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberLinkProof {
    /// That member's seat as recorded: [`MobHandle::seat_now`] answers
    /// [`SeatNow::Current`] or [`SeatNow::SameLife`].
    Seat(RecordedSeat),
    /// The member that mob holds under the name is of this creation:
    /// [`MobHandle::creation_now`] answers [`SeatNow::SameLife`].
    Creation(crate::MemberCreationId),
}

/// The surface name a host records for an identity change that came through
/// its Console, where the caller may be no verified person.
const CONSOLE_REQUESTER_SURFACE: &str = "console";

/// What an identity's last seat row read as, with the incarnation and the
/// session that row seated: the roster keeps one per identity, and a
/// successor row reads it only when its own step names that incarnation.
#[derive(Debug, Clone)]
pub(crate) struct SeatReading {
    pub(crate) generation: Generation,
    pub(crate) session: Option<SessionId>,
    pub(crate) seat: MemberSeat,
}

/// What the runtime observed of who asks for a seat, carried on the spawn
/// spec. Crate-private: no caller of the crate builds or reads one, and no
/// argument of a call fills it.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeatObservations {
    /// The host attested the seat as its own
    /// ([`crate::SpawnMemberSpec::attest_host_seat`]). It stands only while
    /// no spawn entry observed a call that acts for an agent or runs under
    /// an operation owner.
    pub(crate) host_seat: bool,
    /// A spawn entry observed that the call acts for an agent.
    pub(crate) acts_for_agent: bool,
    /// A schedule's fire asked for the seat: set only by the handle's own
    /// entries for the schedule host's helper calls.
    pub(crate) scheduled: bool,
    /// The seat of the member the call acts for, as the runtime sealed it
    /// ([`crate::SpawnMemberSpec::with_seat_creator`]); `None` when no door
    /// sealed one.
    pub(crate) creator: Option<MemberSeatCreator>,
    /// What this seat keeps of the seat it follows: set only by the runtime
    /// for its own successor of a member (a respawn, a rotation), whose
    /// life's record stands unchanged.
    pub(crate) carried: Option<CarriedSeat>,
}

/// What a runtime successor's seat keeps of the seat it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CarriedSeat {
    /// The record the seat it follows holds: the successor's row stores it
    /// unchanged.
    Kept(MemberSeat),
    /// The seat it follows holds no record (its row is an older one): the
    /// successor's row stores none either, and reads as the seat it follows.
    OlderRow,
}

impl CarriedSeat {
    /// What a successor keeps of a seat the roster read as `seat`; a seat
    /// with no reading is followed by a record that shows nothing.
    pub(crate) fn of(seat: Option<&MemberSeat>) -> Self {
        match seat {
            Some(seat) if seat.recorded_by().is_some() => Self::Kept(seat.clone()),
            Some(_) => Self::OlderRow,
            None => Self::Kept(MemberSeat::shell(MemberSeatKind::Unproven)),
        }
    }
}

impl SeatObservations {
    /// The observations of a runtime successor's seat: nothing of a call,
    /// only what it keeps of the seat it follows.
    pub(crate) fn successor(carried: CarriedSeat) -> Self {
        Self {
            carried: Some(carried),
            ..Self::default()
        }
    }

    /// Record what a spawn entry of the handle observes of its call:
    /// whether its authority acts for an agent, and whether it runs under
    /// an operation owner. Either drops the host's attestation.
    pub(crate) fn at_entry(&mut self, acts_for_agent: bool, owner_bound: bool) {
        self.acts_for_agent |= acts_for_agent;
        if acts_for_agent || owner_bound {
            self.host_seat = false;
        }
    }
}

/// Who sealed the creating member's parts of a seat record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SeatSealer {
    /// The mob set, after its complete lookup of the hosted rosters that
    /// name the calling session found exactly this mob.
    Set,
    /// The creating member's own mob, from MobMachine's own bindings, at the
    /// mob's own member tools: no other hosted roster was asked.
    Mob,
}

/// The seat of the member a call acts for, sealed by the runtime for the seat
/// record of what the call seats ([`crate::SpawnMemberSpec::with_seat_creator`]).
///
/// Like the creation source witness, it cannot be built from arguments: a
/// mob's handle seals a member's seat from MobMachine's own bindings
/// ([`MobHandle::seal_seat_creator`]), and the other forms say only that no
/// such seat could be named.
#[derive(Debug, Clone)]
pub struct MemberSeatCreator {
    pub(crate) seal: SeatCreatorSeal,
}

#[derive(Debug, Clone)]
pub(crate) enum SeatCreatorSeal {
    /// The lookup of the calling session could not be completed.
    Unavailable,
    /// No hosted roster names the calling session, `session`.
    NoRoster { session: SessionId },
    /// Several hosted rosters name the calling session.
    Several,
    /// Exactly one member holds the calling session: its seat now.
    One {
        recorded: RecordedSeat,
        sealed_by: SeatSealer,
    },
}

impl MemberSeatCreator {
    /// The lookup of the calling session could not be completed: the seat is
    /// recorded closed.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            seal: SeatCreatorSeal::Unavailable,
        }
    }

    /// A complete lookup found no hosted roster that names the calling
    /// session, `session`: the session the runtime runs the call for, never
    /// one the call names.
    #[must_use]
    pub fn no_roster(session: SessionId) -> Self {
        Self {
            seal: SeatCreatorSeal::NoRoster { session },
        }
    }

    /// A complete lookup found several hosted rosters that name the calling
    /// session: none is picked.
    #[must_use]
    pub fn several() -> Self {
        Self {
            seal: SeatCreatorSeal::Several,
        }
    }

    /// The sealed seat, when one member holds the calling session.
    #[must_use]
    pub fn recorded(&self) -> Option<&RecordedSeat> {
        match &self.seal {
            SeatCreatorSeal::One { recorded, .. } => Some(recorded),
            _ => None,
        }
    }
}

/// What the seat step observed, from which the seat's kind is composed
/// ([`compose_seat`]). Raw observations only: the runtime's own facts, never
/// an argument, a name or a label of the call.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeatStepFacts {
    /// The session that owns the spawn's operation, when the spawn runs
    /// under an operation owner.
    pub(crate) owner_session: Option<SessionId>,
    /// The mob's own definition asked for the seat (a flow step, a spawn
    /// policy).
    pub(crate) definition_seat: bool,
    /// The member is seated as a copy of another member.
    pub(crate) fork_seat: bool,
    /// The seat's row names the member it is a copy of (a durable copy's
    /// source), which a read of the seat can continue at.
    pub(crate) fork_source: bool,
    /// The copy is the calling member's own, made in its own turn.
    pub(crate) own_turn: bool,
    /// The member is placed on another host, whose carrier keeps nothing of
    /// who asked.
    pub(crate) placed: bool,
    /// For a sealed creator of this same mob: whether MobMachine's own
    /// bindings (the session and the runtime id) bear the seal out. `None`
    /// for a creator of another mob.
    pub(crate) same_mob_bindings_agree: Option<bool>,
}

/// The seat record of a seat, composed from what the runtime observed;
/// `None` only for a runtime successor of a seat whose row holds no record.
pub(crate) fn compose_seat(
    observations: &SeatObservations,
    facts: &SeatStepFacts,
) -> Option<MemberSeat> {
    // A runtime successor keeps what the seat it follows holds, whatever
    // its own step observed.
    match &observations.carried {
        Some(CarriedSeat::Kept(seat)) => Some(seat.clone()),
        Some(CarriedSeat::OlderRow) => None,
        None => Some(compose_new_seat(observations, facts)),
    }
}

/// The record of a seat that follows no other. The order is the rule: a
/// schedule's seat first, whatever else holds; a placed seat shows nothing;
/// the host's own seat needs its attestation as it stands after every entry
/// (no call for an agent, no operation owner), no sealed caller and no
/// copy; a seat made for a caller (an agent's call, a spawn under an
/// operation owner, or a seat its door sealed a caller for) is read from
/// the sealed creator alone; then the mob's own definition and a copy whose
/// row names its source. Everything else shows nothing.
fn compose_new_seat(observations: &SeatObservations, facts: &SeatStepFacts) -> MemberSeat {
    use MemberSeatKind as Kind;
    let unproven = || MemberSeat::shell(Kind::Unproven);
    if observations.scheduled {
        return MemberSeat::shell(Kind::Scheduled);
    }
    if facts.placed {
        return unproven();
    }
    let for_agent = observations.acts_for_agent;
    let owner = facts.owner_session.as_ref();
    let seal = observations.creator.as_ref().map(|creator| &creator.seal);
    let for_caller = for_agent || owner.is_some() || seal.is_some();
    if observations.host_seat && !for_caller && !facts.fork_seat {
        return MemberSeat::shell(Kind::Host);
    }
    if for_caller {
        // A sealed caller is taken only for the session that owns the
        // operation, when the spawn runs under one.
        let owns = |session: &SessionId| owner.is_none_or(|owner| owner == session);
        return match seal {
            Some(SeatCreatorSeal::One {
                recorded,
                sealed_by,
            }) => {
                if !owns(&recorded.owner_session) || facts.same_mob_bindings_agree == Some(false) {
                    unproven()
                } else if for_agent {
                    MemberSeat::shell_with_creator(
                        Kind::Member,
                        recorded.clone(),
                        *sealed_by,
                        facts.fork_seat && facts.own_turn,
                    )
                } else {
                    MemberSeat::shell_with_creator(
                        Kind::HostForMember,
                        recorded.clone(),
                        *sealed_by,
                        false,
                    )
                }
            }
            Some(SeatCreatorSeal::NoRoster { session }) if owns(session) => {
                let kind = if for_agent {
                    Kind::Session
                } else {
                    Kind::HostForSession
                };
                MemberSeat::shell_for_session(kind, session.clone())
            }
            Some(SeatCreatorSeal::Several) => MemberSeat::shell(Kind::Ambiguous),
            Some(SeatCreatorSeal::NoRoster { .. } | SeatCreatorSeal::Unavailable) | None => {
                unproven()
            }
        };
    }
    if facts.definition_seat {
        return MemberSeat::shell(Kind::Definition);
    }
    if facts.fork_seat && facts.fork_source {
        return MemberSeat::shell(Kind::ForkOf);
    }
    unproven()
}

/// The member MobMachine's own bindings hold for `session` in `state`: its
/// identity and generation. `None` unless exactly one identity is bound to
/// the session, stands active and holds a runtime.
pub(crate) fn machine_member_of_session(
    state: &crate::machines::mob_machine::MobMachineState,
    session: &SessionId,
) -> Option<(AgentIdentity, Generation)> {
    use crate::machines::mob_machine as mob_dsl;
    let dsl_session = mob_dsl::SessionId::from_domain(session);
    let mut bound = state
        .member_session_bindings
        .iter()
        .filter(|(_, bound)| **bound == dsl_session);
    let (identity, _) = bound.next()?;
    if bound.next().is_some() {
        return None;
    }
    if !super::mob_member_lifecycle_projection::MobMemberLifecycleProjection::is_active_machine_lifecycle(
        &state.member_lifecycle_for_identity(identity),
    ) {
        return None;
    }
    let agent_identity = AgentIdentity::from(identity.0.as_str());
    let (runtime_id, _) = state
        .member_runtime_material_for_identity(identity)?
        .to_domain_for_identity(&agent_identity);
    Some((agent_identity, runtime_id.generation))
}

/// Whether MobMachine's own bindings in `state` bear out a sealed creator of
/// the mob `mob_id` at `incarnation`; `None` for a creator of another mob,
/// which this mob's machine holds nothing of.
pub(crate) fn sealed_creator_bindings_agree(
    creator: Option<&MemberSeatCreator>,
    mob_id: &MobId,
    incarnation: MobIncarnationId,
    state: &crate::machines::mob_machine::MobMachineState,
) -> Option<bool> {
    let recorded = creator?.recorded()?;
    if &recorded.parent_mob_id != mob_id {
        return None;
    }
    Some(
        recorded.parent_incarnation == incarnation
            && machine_member_of_session(state, &recorded.owner_session)
                == Some((
                    recorded.creator_identity.clone(),
                    recorded.creator_generation,
                )),
    )
}

impl MemberSeatKind {
    /// The word a stored record holds for the kind; a seat read from a row
    /// that holds no record has none.
    fn stored_word(self) -> Option<&'static str> {
        Some(match self {
            Self::Host => "host",
            Self::HostForMember => "host_for_member",
            Self::HostForSession => "host_for_session",
            Self::Member => "member",
            Self::Session => "session",
            Self::Ambiguous => "ambiguous",
            Self::Definition => "definition",
            Self::ForkOf => "fork_of",
            Self::Scheduled => "scheduled",
            Self::Unproven => "unproven",
            Self::Unrecorded => return None,
        })
    }

    fn from_stored_word(word: &str) -> Option<Self> {
        Some(match word {
            "host" => Self::Host,
            "host_for_member" => Self::HostForMember,
            "host_for_session" => Self::HostForSession,
            "member" => Self::Member,
            "session" => Self::Session,
            "ambiguous" => Self::Ambiguous,
            "definition" => Self::Definition,
            "fork_of" => Self::ForkOf,
            "scheduled" => Self::Scheduled,
            "unproven" => Self::Unproven,
            _ => return None,
        })
    }
}

impl SeatRecorder {
    fn stored_word(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Machine => "machine",
        }
    }

    fn from_stored_word(word: &str) -> Option<Self> {
        match word {
            "shell" => Some(Self::Shell),
            "machine" => Some(Self::Machine),
            _ => None,
        }
    }
}

impl SeatSealer {
    fn stored_word(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Mob => "mob",
        }
    }

    fn from_stored_word(word: &str) -> Option<Self> {
        match word {
            "set" => Some(Self::Set),
            "mob" => Some(Self::Mob),
            _ => None,
        }
    }
}

/// The stored form of a seat record: one internal member of the stored seat
/// row, kept exactly as stored. A row a later release wrote is written back
/// unchanged, and no stored form fails a row's decode: what this build
/// cannot read whole reads closed ([`MemberSeat::from_stored`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct StoredSeatLineage(serde_json::Value);

impl StoredSeatLineage {
    /// The kind word the record stores, when it stores one.
    #[cfg(test)]
    pub(crate) fn kind_word(&self) -> Option<&str> {
        self.0.get("kind").and_then(serde_json::Value::as_str)
    }

    fn recorder(&self) -> Option<SeatRecorder> {
        self.0
            .get("recorded_by")
            .and_then(serde_json::Value::as_str)
            .and_then(SeatRecorder::from_stored_word)
    }

    fn parts(&self) -> Option<StoredSeatParts> {
        serde_json::from_value(self.0.clone()).ok()
    }
}

/// The members of a stored seat record this build reads.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSeatParts {
    recorded_by: String,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creator: Option<StoredSeatCreator>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    own_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed_by: Option<String>,
    /// The calling session, for a kind whose caller is a session no hosted
    /// roster names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<SessionId>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSeatCreator {
    parent_mob_id: MobId,
    /// The incarnation's canonical text.
    parent_incarnation: String,
    creator_identity: AgentIdentity,
    creator_generation: Generation,
    owner_session: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creator_life: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creator_creation: Option<crate::MemberCreationId>,
}

impl MemberSeat {
    /// Who sealed the creating member's parts, for a record that names one.
    #[must_use]
    pub fn sealed_by(&self) -> Option<SeatSealer> {
        self.sealed_by
    }

    /// The stored form of this record; `None` for a seat read from a row
    /// that holds no record.
    pub(crate) fn to_stored(&self) -> Option<StoredSeatLineage> {
        let recorded_by = self.recorded_by?.stored_word();
        let kind = self.kind.stored_word()?;
        let parts = StoredSeatParts {
            recorded_by: recorded_by.to_string(),
            kind: kind.to_string(),
            creator: self.recorded.as_ref().map(|recorded| StoredSeatCreator {
                parent_mob_id: recorded.parent_mob_id.clone(),
                parent_incarnation: recorded.parent_incarnation.to_string(),
                creator_identity: recorded.creator_identity.clone(),
                creator_generation: recorded.creator_generation,
                owner_session: recorded.owner_session.clone(),
                creator_life: recorded.creator_life.map(|life| life.token()),
                creator_creation: recorded.creator_creation,
            }),
            own_turn: self.own_turn,
            sealed_by: self
                .sealed_by
                .map(|sealed_by| sealed_by.stored_word().to_string()),
            session: self.caller_session().cloned(),
        };
        serde_json::to_value(parts).ok().map(StoredSeatLineage)
    }

    /// The record a stored form holds. A word this build does not know reads
    /// closed: who seated the member could not be shown.
    pub(crate) fn from_stored(stored: &StoredSeatLineage) -> Self {
        stored
            .parts()
            .and_then(|parts| Self::read_stored(&parts))
            .unwrap_or_else(|| Self {
                recorded_by: stored.recorder(),
                ..Self::from_older_row(MemberSeatKind::Unproven, false)
            })
    }

    /// The record a stored form holds, when this build reads every word of
    /// it and it holds exactly the parts its kind has.
    fn read_stored(stored: &StoredSeatParts) -> Option<Self> {
        use MemberSeatKind as Kind;
        let recorded_by = SeatRecorder::from_stored_word(&stored.recorded_by)?;
        let kind = Kind::from_stored_word(&stored.kind)?;
        let has_creator = matches!(kind, Kind::Member | Kind::HostForMember);
        let has_session = matches!(kind, Kind::Session | Kind::HostForSession);
        if stored.creator.is_some() != has_creator
            || stored.sealed_by.is_some() != has_creator
            || stored.session.is_some() != has_session
            || (stored.own_turn && kind != Kind::Member)
        {
            return None;
        }
        let recorded = match &stored.creator {
            Some(creator) => Some(RecordedSeat {
                parent_mob_id: creator.parent_mob_id.clone(),
                parent_incarnation: creator.parent_incarnation.parse().ok()?,
                creator_identity: creator.creator_identity.clone(),
                creator_generation: creator.creator_generation,
                owner_session: creator.owner_session.clone(),
                creator_life: match &creator.creator_life {
                    Some(token) => Some(super::IdentityLife::from_token(token)?),
                    None => None,
                },
                creator_creation: creator.creator_creation,
            }),
            None => None,
        };
        let sealed_by = match &stored.sealed_by {
            Some(word) => Some(SeatSealer::from_stored_word(word)?),
            None => None,
        };
        Some(Self {
            kind,
            recorded,
            own_turn: stored.own_turn,
            recorded_by: Some(recorded_by),
            sealed_by,
            session: stored.session.clone(),
        })
    }
}

/// Who seated the member of a seat row that holds no record, read from the
/// row's older members: the creation it carries and the runtime step that
/// made it.
///
/// `snapshot` is what the journal's creation projection holds for the row's
/// session once the row is folded, and `cursor` the row's own cursor. The
/// creation alone says nothing: a member seated again on a kept session
/// carries that session's earlier creation whole. So the creation is read
/// only on the row that first carried it (the snapshot's birth cursor). A
/// row the runtime's own successor step made reads as the seat it follows:
/// `last`, the identity's last seat row's reading, and only when the step
/// names exactly that row's incarnation (the row's seat rotation names the
/// generation it follows). A row of an earlier release that has no seat
/// rotation and carries a successor creation the projection accepted names
/// the session it follows: a seat resumed on a kept session carries that
/// session's earlier creation whole, a successor creation included, so this
/// arm takes `last` only when `last` was read from that very session, never
/// by the creation alone. A later member under the name is never such a
/// successor, so it never reads an earlier member's seat. Every other row
/// says nothing.
pub(crate) fn seat_of_older_row(
    spawned: &crate::event::MemberSpawnedEvent,
    snapshot: Option<&crate::MemberCreationSnapshot>,
    cursor: u64,
    last: Option<&SeatReading>,
) -> MemberSeat {
    use super::SeatRotation;
    use crate::MemberCreationProvenance as Provenance;
    let unrecorded = || MemberSeat::from_older_row(MemberSeatKind::Unrecorded, false);
    let carried = snapshot.filter(|snapshot| snapshot.creation == spawned.creation);
    // Whether a runtime successor step made the row, and of which seat.
    let follows_last = match (&spawned.seat_rotation, &spawned.creation.provenance) {
        (
            Some(
                SeatRotation::Successor { predecessor, .. }
                | SeatRotation::RespawnAfter { predecessor },
            ),
            _,
        ) => Some(last.is_some_and(|last| last.generation == *predecessor)),
        (
            None,
            Provenance::Successor {
                predecessor_session_id,
                ..
            },
        ) if carried.is_some_and(|snapshot| snapshot.birth_cursor < cursor) => {
            Some(last.is_some_and(|last| last.session.as_ref() == Some(predecessor_session_id)))
        }
        _ => None,
    };
    match follows_last {
        Some(true) => return last.map_or_else(unrecorded, |last| last.seat.clone()),
        Some(false) => return unrecorded(),
        None => {}
    }
    if carried.is_none_or(|snapshot| snapshot.birth_cursor != cursor) {
        return unrecorded();
    }
    match &spawned.creation.provenance {
        Provenance::Root => MemberSeat::from_older_row(MemberSeatKind::Host, false),
        Provenance::Spawn { .. } => MemberSeat::from_older_row(MemberSeatKind::Member, false),
        Provenance::Fork { .. } if spawned.spawned_by.is_some() => {
            MemberSeat::from_older_row(MemberSeatKind::Member, true)
        }
        Provenance::Fork { .. } => MemberSeat::from_older_row(MemberSeatKind::ForkOf, false),
        Provenance::Successor { .. } | Provenance::Unproven | Provenance::LegacyUnknown => {
            unrecorded()
        }
    }
}

/// The creating member a mob's creation record names
/// (`MobProvenance::OperatorMember`): the identity, the generation it
/// created the mob at, and the session it created it on.
#[derive(Debug, Clone, Copy)]
struct RecordedCreator<'a> {
    identity: &'a AgentIdentity,
    generation: crate::Generation,
    owner_session: &'a SessionId,
}

/// How the member that serves a session now stands to a mob's recorded
/// creator ([`RecordedCreator::seat`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatorSeat {
    /// The session serves the recorded identity at the recorded generation.
    Recorded,
    /// The session serves the recorded identity at this later generation:
    /// the record proves it only if that generation is a same-session
    /// successor of the recorded one ([`RecordedCreator::is_succeeded_by`]).
    Later(crate::Generation),
    /// Anything else: another session, no member, another identity, or an
    /// earlier generation. The record proves nothing.
    Other,
}

/// Whether a same-session successor seated under ANOTHER ROLE (a declared
/// role migration) keeps its creator's manage grant over the mobs the
/// session created. It does: the grant is the session's, over the mobs it
/// created, and the current role's own tools and capabilities still decide
/// each call (a successor whose role mounts no mob tools, or whose operator
/// removed its create capability, reaches none of them). This is the one
/// place that decides it: `false` makes a role migration end the grant.
const fn a_role_migration_successor_keeps_the_creators_grant() -> bool {
    true
}

/// The steps the creator's grant follows through the parent's journal.
fn creator_lineage_steps() -> crate::LineageSteps {
    crate::LineageSteps::same_session_successors()
        .with_role_migrations(a_role_migration_successor_keeps_the_creators_grant())
}

impl RecordedCreator<'_> {
    /// How the member `seated` on `session` (the parent roster's member of
    /// that session now, with its generation) stands to this creator.
    fn seat(
        &self,
        session: &SessionId,
        seated: Option<&(AgentIdentity, crate::Generation)>,
    ) -> CreatorSeat {
        if session != self.owner_session {
            return CreatorSeat::Other;
        }
        match seated {
            Some((identity, generation)) if identity == self.identity => {
                match generation.cmp(&self.generation) {
                    std::cmp::Ordering::Equal => CreatorSeat::Recorded,
                    std::cmp::Ordering::Greater => CreatorSeat::Later(*generation),
                    std::cmp::Ordering::Less => CreatorSeat::Other,
                }
            }
            _ => CreatorSeat::Other,
        }
    }

    /// Whether the member that serves the creator's session at the later
    /// `generation` is a same-session successor of this creator, by
    /// `binding`: the binding of that session in the parent's member binding
    /// history (`None` when the history could not be read or names no such
    /// binding).
    ///
    /// It is when the journal's binding agrees with the roster (the session's
    /// binding is live, of the same identity, at the same generation) and the
    /// unbroken chain of same-session successor steps that seated it starts
    /// at or before the recorded generation: every step from the creator's
    /// incarnation to this one replaced the member by its successor on this
    /// session. A binding that answers no chain (the parent's journal holds
    /// a mob reset) proves nothing. Every failure answers `false`.
    fn is_succeeded_by(
        &self,
        generation: crate::Generation,
        binding: Option<&crate::MemberBinding>,
    ) -> bool {
        let Some(binding) = binding else {
            return false;
        };
        let Some(chain_start) = binding.same_session_lineage_start(creator_lineage_steps()) else {
            return false;
        };
        binding.is_live()
            && binding.identity() == self.identity
            && binding.generation() == generation
            && chain_start <= self.generation
    }
}

/// The binding of `session` in the member binding history `history` of the
/// mob `parent`, for the creator's lineage. A history that could not be read
/// answers none, so the lineage proves nothing.
fn lineage_binding(
    history: Result<crate::MemberBindingHistory, MobError>,
    parent: &MobId,
    session: &SessionId,
) -> Option<crate::MemberBinding> {
    match history {
        Ok(history) => history.binding_for_session(session).cloned(),
        Err(error) => {
            // Asked once for each mob a successor's session created, at each
            // listing: a line an operator's log takes at debug level.
            tracing::debug!(
                mob_id = %parent,
                %error,
                "a mob's member history could not be read to follow its creator's successors"
            );
            None
        }
    }
}

impl MobHandle {
    /// The seat record of the member seated under `identity` now: who seated
    /// its life. `None` when no member is seated under the name.
    ///
    /// Read from this handle's projection of the journal's seat rows, with no
    /// actor command; reading it never wakes a member. It is a fact of the
    /// journal and allows nothing by itself.
    ///
    /// Never read the seat alone for an identity entry's member: an entry's
    /// seat is the host's whoever asked for the identity, and an adoption
    /// makes it the host's too. Where an entry's lineage goes is answered by
    /// [`Self::member_lineage_step`], from the entry's own record.
    pub async fn member_seat(&self, identity: &AgentIdentity) -> Option<MemberSeat> {
        self.roster.read().await.member_seat(identity)
    }

    /// Seal the seat, as this mob holds it now, of the member whose session
    /// is `session`, for the seat record of a member a call of that session
    /// seats ([`crate::SpawnMemberSpec::with_seat_creator`]).
    ///
    /// The parts are MobMachine's own: the identity and generation its
    /// bindings hold for the session, the identity's life marker, and the
    /// creation the journal records for the session. Nothing of the call is
    /// read. `sealed_by` says who asks: the mob set after its complete lookup
    /// of the hosted rosters found exactly this mob, or this mob's own member
    /// tools. A session this mob's bindings do not hold for exactly one
    /// member seals nothing ([`MemberSeatCreator::unavailable`]).
    pub async fn seal_seat_creator(
        &self,
        session: &SessionId,
        sealed_by: SeatSealer,
    ) -> MemberSeatCreator {
        let creation = self
            .roster
            .read()
            .await
            .member_creation_for_session(session)
            .ok()
            .flatten()
            .and_then(|snapshot| snapshot.creation.creation_id);
        let state = self.machine_state_watch_rx.borrow();
        let Some((identity, generation)) = machine_member_of_session(&state, session) else {
            return MemberSeatCreator::unavailable();
        };
        let life = super::IdentityLife::of(&state, &identity);
        let mut recorded = RecordedSeat::new(
            self.mob_id().clone(),
            self.incarnation(),
            identity,
            generation,
            session.clone(),
        )
        .with_creator_life(life);
        recorded.creator_creation = creation;
        MemberSeatCreator {
            seal: SeatCreatorSeal::One {
                recorded,
                sealed_by,
            },
        }
    }

    /// Whether `recorded` is still a seat this mob holds ([`SeatNow`]).
    ///
    /// `recorded` names this mob at this handle's incarnation, or the answer
    /// is [`SeatNow::Gone`]. The history is read only when the recorded
    /// session serves the recorded identity at a later generation, and the
    /// identity's life marker only when the seat is not the current one;
    /// both need the handle's principal to hold
    /// [`crate::ControlScope::List`], and a read it is refused answers
    /// [`SeatNow::Unreadable`]. It allows nothing by itself: each reader says
    /// which answers it accepts.
    pub async fn seat_now(&self, recorded: &RecordedSeat) -> SeatNow {
        if &recorded.parent_mob_id != self.mob_id()
            || recorded.parent_incarnation != self.incarnation()
        {
            return SeatNow::Gone;
        }
        let creator = RecordedCreator {
            identity: &recorded.creator_identity,
            generation: recorded.creator_generation,
            owner_session: &recorded.owner_session,
        };
        let seated = self.member_of_session(&recorded.owner_session).await;
        // Whether a fact the current seat's proof needed could not be read.
        let mut unread = false;
        match creator.seat(&recorded.owner_session, seated.as_ref()) {
            CreatorSeat::Recorded => return SeatNow::Current,
            CreatorSeat::Other => {}
            CreatorSeat::Later(generation) => {
                let history = self.member_binding_history().await;
                unread = history.is_err();
                let binding =
                    lineage_binding(history, &recorded.parent_mob_id, &recorded.owner_session);
                if creator.is_succeeded_by(generation, binding.as_ref()) {
                    return SeatNow::Current;
                }
            }
        }
        let closed = if unread {
            SeatNow::Unreadable
        } else {
            SeatNow::Gone
        };
        let Some(recorded_life) = recorded.creator_life else {
            return closed;
        };
        // The same life: a member is seated under the identity now, and the
        // identity's life marker, which MobMachine moves at every end of the
        // identity and at nothing else, is the recorded one.
        let dsl_identity =
            crate::machines::mob_machine::AgentIdentity::from_domain(&recorded.creator_identity);
        let life_now = self.read_published_machine_state(crate::ControlScope::List, |state| {
            state
                .identity_to_runtime
                .contains_key(&dsl_identity)
                .then(|| super::IdentityLife::of(state, &recorded.creator_identity))
        });
        match life_now {
            Ok(Some(now)) if now == recorded_life => SeatNow::SameLife,
            Ok(_) => closed,
            Err(_) => SeatNow::Unreadable,
        }
    }

    /// Where the lineage of the member seated under `identity` goes next
    /// ([`MemberLineageStep`]): asked first of MobMachine's record of the
    /// identity's entry, then of the member's seat record.
    ///
    /// A declared identity's is its mob's own. A created identity's lineage
    /// is its identity record's requester and consent, never its seat: the
    /// host's or a person's request, or their recorded consent, makes it its
    /// mob's own, and an agent's request that the host's rule alone let
    /// through closes, because the record names the agent by name only. Any
    /// other member's is its seat record. A name no member holds, and
    /// everything that cannot be read, closes.
    pub async fn member_lineage_step(&self, identity: &AgentIdentity) -> MemberLineageStep {
        use super::{EntryStanding, IdentityEntryOrigin, IdentityEntryRead};
        match self.identity_entry(identity) {
            Err(_) | Ok(IdentityEntryRead::Unreadable { .. }) => {
                MemberLineageStep::Closed(LineageClosed::Unreadable)
            }
            Ok(IdentityEntryRead::Removed | IdentityEntryRead::NotAnEntry { worker: None }) => {
                MemberLineageStep::Closed(LineageClosed::NoMember)
            }
            Ok(IdentityEntryRead::NotAnEntry { worker: Some(_) }) => {
                self.seat_lineage_step(identity).await
            }
            Ok(IdentityEntryRead::Entry(entry)) => match (entry.standing, entry.origin) {
                (EntryStanding::Unseated(_), _) => {
                    MemberLineageStep::Closed(LineageClosed::NoMember)
                }
                (_, IdentityEntryOrigin::Declared) => {
                    MemberLineageStep::Mob(MobLineageCause::DeclaredEntry)
                }
                (_, IdentityEntryOrigin::Created) => {
                    self.created_entry_lineage_step(identity).await
                }
            },
        }
    }

    /// Whether the member this mob holds under `identity` now is of the
    /// creation `creation`: the same member, or a runtime successor of it
    /// (a respawn and a rotation keep a member's creation). The proof of a
    /// copy's link to its source ([`MemberLinkProof::Creation`]).
    ///
    /// [`SeatNow::SameLife`] when the session of the member seated under the
    /// name records that creation; [`SeatNow::Gone`] for no member, and for
    /// a member of any other creation; [`SeatNow::Unreadable`] when the
    /// journal's creation record cannot be read. Read from this handle's
    /// projection of the journal, with no actor command.
    pub async fn creation_now(
        &self,
        identity: &AgentIdentity,
        creation: crate::MemberCreationId,
    ) -> SeatNow {
        let roster = self.roster.read().await;
        let Some(session) = roster
            .get(identity)
            .and_then(|entry| entry.bridge_session_id())
        else {
            return SeatNow::Gone;
        };
        match roster.member_creation_for_session(session) {
            Ok(Some(snapshot))
                if snapshot.creation.creation_id == Some(creation)
                    && snapshot.member_binding.member == identity.as_str() =>
            {
                SeatNow::SameLife
            }
            Ok(_) => SeatNow::Gone,
            Err(_) => SeatNow::Unreadable,
        }
    }

    /// The step of a created identity's member, from its identity record.
    async fn created_entry_lineage_step(&self, identity: &AgentIdentity) -> MemberLineageStep {
        use crate::identity::{
            CreatedIdentityObservation, IdentityChangeConsent, IdentityChangeRequester,
        };
        let Ok(Some(CreatedIdentityObservation::Created(record))) =
            self.identity_record(identity).await
        else {
            return MemberLineageStep::Closed(LineageClosed::Unreadable);
        };
        match (&record.provenance.requester, &record.provenance.consent) {
            (IdentityChangeRequester::Host { surface }, _)
                if surface == CONSOLE_REQUESTER_SURFACE =>
            {
                MemberLineageStep::Closed(LineageClosed::UnverifiedRequester)
            }
            (IdentityChangeRequester::Human { .. } | IdentityChangeRequester::Host { .. }, _)
            | (
                IdentityChangeRequester::Agent { .. },
                IdentityChangeConsent::Decided { .. } | IdentityChangeConsent::Imported { .. },
            ) => MemberLineageStep::Mob(MobLineageCause::AuthorizedEntry),
            // The record names the agent that asked by name only, and a
            // name is never followed: a member that left frees its name for
            // whoever is named next. The host's rule alone let the create
            // through, so nobody's recorded consent stands for it either.
            (IdentityChangeRequester::Agent { .. }, IdentityChangeConsent::Policy { .. }) => {
                MemberLineageStep::Closed(LineageClosed::CreatorNotProven)
            }
        }
    }

    /// The step of a member that is no identity entry, from its seat record.
    async fn seat_lineage_step(&self, identity: &AgentIdentity) -> MemberLineageStep {
        let (seat, fork_source, creation) = {
            let roster = self.roster.read().await;
            let entry = roster.get(identity);
            (
                roster.member_seat(identity),
                entry.and_then(|entry| entry.fork_source.clone()),
                // The creation the member's own session records.
                entry
                    .and_then(|entry| entry.bridge_session_id())
                    .and_then(|session| roster.member_creation_for_session(session).ok())
                    .flatten()
                    .map(|snapshot| snapshot.creation.provenance),
            )
        };
        let Some(seat) = seat else {
            return MemberLineageStep::Closed(LineageClosed::NoMember);
        };
        let linked = |link| match seat.recorded() {
            Some(recorded) => MemberLineageStep::Member {
                mob_id: recorded.parent_mob_id.clone(),
                identity: recorded.creator_identity.clone(),
                link,
                proof: MemberLinkProof::Seat(recorded.clone()),
            },
            None => MemberLineageStep::Closed(LineageClosed::CreatorNotProven),
        };
        match seat.kind() {
            MemberSeatKind::Host => MemberLineageStep::Mob(MobLineageCause::HostSeat),
            MemberSeatKind::Definition => MemberLineageStep::Mob(MobLineageCause::DefinitionSeat),
            MemberSeatKind::Member => linked(MemberLink::SeatedBy),
            MemberSeatKind::HostForMember => linked(MemberLink::SeatedFor),
            // A copy goes on at its source only by the source's creation,
            // which the copy's own creation records: the source's name alone
            // is never followed.
            MemberSeatKind::ForkOf => match (fork_source, creation) {
                (
                    Some(source),
                    Some(crate::MemberCreationProvenance::Fork { source_creation_id }),
                ) => MemberLineageStep::Member {
                    mob_id: MobId::from(source.source_member.mob_id.as_str()),
                    identity: AgentIdentity::from(source.source_member.member.as_str()),
                    link: MemberLink::ForkOf,
                    proof: MemberLinkProof::Creation(source_creation_id),
                },
                (Some(_), _) => MemberLineageStep::Closed(LineageClosed::CreatorNotProven),
                (None, _) => MemberLineageStep::Closed(LineageClosed::Unproven),
            },
            MemberSeatKind::HostForSession | MemberSeatKind::Session => {
                MemberLineageStep::Closed(LineageClosed::SessionNoRoster)
            }
            MemberSeatKind::Ambiguous => MemberLineageStep::Closed(LineageClosed::Ambiguous),
            MemberSeatKind::Scheduled => MemberLineageStep::Closed(LineageClosed::Scheduled),
            MemberSeatKind::Unproven => MemberLineageStep::Closed(LineageClosed::Unproven),
            MemberSeatKind::Unrecorded => MemberLineageStep::Closed(LineageClosed::Unrecorded),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::event::{
        MemberRef, MemberRetiredFacts, MemberSessionBindingRecoveredEvent, MemberSpawnedEvent,
        MobEvent, MobEventKind,
    };
    use crate::ids::{AgentRuntimeId, FenceToken, ProfileName};
    use crate::roster::Roster;
    use crate::runtime::{RotationTarget, SeatRotation};
    use crate::{
        MemberCreationId, MemberCreationProvenance, MemberCreationRecord, MemberCreationSource,
    };
    use meerkat_core::{ForkBuildSource, MobMemberBinding};

    const MOB: &str = "seats";

    fn binding(member: &str) -> MobMemberBinding {
        MobMemberBinding {
            mob_id: MOB.to_string(),
            role: "worker".to_string(),
            member: member.to_string(),
        }
    }

    /// A seat row as a release without the seat record wrote it.
    fn seat_row(
        cursor: u64,
        member: &str,
        generation: u64,
        session: &SessionId,
        creation_id: MemberCreationId,
        provenance: MemberCreationProvenance,
    ) -> MobEvent {
        let identity = AgentIdentity::from(member);
        let generation = Generation::new(generation);
        let mut spawned = MemberSpawnedEvent::new(
            identity.clone(),
            generation,
            FenceToken::new(cursor),
            AgentRuntimeId::new(identity, generation),
            ProfileName::from("worker"),
        )
        .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(session.clone())));
        spawned.creation = MemberCreationRecord {
            creation_id: Some(creation_id),
            provenance,
        };
        MobEvent {
            cursor,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from(MOB),
            kind: MobEventKind::MemberSpawned(spawned),
        }
    }

    fn with_spawned(mut event: MobEvent, change: impl FnOnce(&mut MemberSpawnedEvent)) -> MobEvent {
        if let MobEventKind::MemberSpawned(spawned) = &mut event.kind {
            change(spawned);
        }
        event
    }

    fn retired(cursor: u64, member: &str, generation: u64) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from(MOB),
            kind: MobEventKind::MemberRetired {
                agent_identity: AgentIdentity::from(member),
                generation: Generation::new(generation),
                role: ProfileName::from("worker"),
                session_archived: false,
                facts: MemberRetiredFacts::default(),
            },
        }
    }

    fn seat_of(roster: &Roster, member: &str) -> Option<(MemberSeatKind, bool, bool)> {
        roster.seat(&AgentIdentity::from(member)).map(|seat| {
            (
                seat.kind(),
                seat.own_turn(),
                seat.recorded().is_some() || seat.recorded_by().is_some(),
            )
        })
    }

    /// A root row is the host's seat on its life's first row.
    #[test]
    fn an_old_root_row_reads_host_on_its_lifes_first_row() {
        let session = SessionId::new();
        let roster = Roster::project(&[seat_row(
            1,
            "lead",
            0,
            &session,
            MemberCreationId::new(),
            MemberCreationProvenance::Root,
        )]);
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Host, false, false))
        );
    }

    /// A member retired and seated again on its kept session takes that
    /// session's earlier creation whole. Its row is not the life's first and
    /// no runtime step made it: it reads as a row without the record, also
    /// when the creation it carries says root.
    #[test]
    fn an_old_seat_on_a_kept_session_after_a_retirement_reads_unrecorded() {
        let session = SessionId::new();
        let creation = MemberCreationId::new();
        let roster = Roster::project(&[
            seat_row(
                1,
                "lead",
                0,
                &session,
                creation,
                MemberCreationProvenance::Root,
            ),
            retired(2, "lead", 0),
            seat_row(
                3,
                "lead",
                1,
                &session,
                creation,
                MemberCreationProvenance::Root,
            ),
        ]);
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Unrecorded, false, false))
        );
    }

    /// An agent's spawn reads as a member's seat. The row holds no record,
    /// so it names no creating seat: its creator is not proven.
    #[test]
    fn an_old_spawn_row_reads_member_and_names_no_creator() {
        let session = SessionId::new();
        let roster = Roster::project(&[seat_row(
            1,
            "helper",
            0,
            &session,
            MemberCreationId::new(),
            MemberCreationProvenance::Spawn {
                source: MemberCreationSource {
                    session_id: SessionId::new(),
                    member_binding: binding("lead"),
                    creation_id: MemberCreationId::new(),
                },
            },
        )]);
        assert_eq!(
            seat_of(&roster, "helper"),
            Some((MemberSeatKind::Member, false, false))
        );
    }

    fn fork_row(cursor: u64, member: &str, session: &SessionId, own_turn: bool) -> MobEvent {
        let source_session = SessionId::new();
        with_spawned(
            seat_row(
                cursor,
                member,
                0,
                session,
                MemberCreationId::new(),
                MemberCreationProvenance::Fork {
                    source_creation_id: MemberCreationId::new(),
                },
            ),
            |spawned| {
                spawned.fork_source = Some(ForkBuildSource::new(binding("lead"), source_session));
                if own_turn {
                    spawned.spawned_by = Some(AgentIdentity::from("lead"));
                }
            },
        )
    }

    /// The caller's own fork reads as its seat, made in its own turn.
    #[test]
    fn an_old_callers_fork_reads_member_own_turn() {
        let roster = Roster::project(&[fork_row(1, "copy", &SessionId::new(), true)]);
        assert_eq!(
            seat_of(&roster, "copy"),
            Some((MemberSeatKind::Member, true, false))
        );
    }

    /// A fork no member's own turn asked for reads as a copy of its source.
    #[test]
    fn an_old_fork_alone_reads_fork_of() {
        let roster = Roster::project(&[fork_row(1, "copy", &SessionId::new(), false)]);
        assert_eq!(
            seat_of(&roster, "copy"),
            Some((MemberSeatKind::ForkOf, false, false))
        );
    }

    /// The runtime's own successor reads as the seat it follows: onto
    /// another session (its row names its predecessor), and on the kept
    /// session (its row names the step that seated it).
    #[test]
    fn an_old_successor_row_reads_as_its_predecessor() {
        let first = SessionId::new();
        let second = SessionId::new();
        let creation = MemberCreationId::new();
        let mut events = vec![
            seat_row(
                1,
                "lead",
                0,
                &first,
                creation,
                MemberCreationProvenance::Root,
            ),
            retired(2, "lead", 0),
            seat_row(
                3,
                "lead",
                1,
                &second,
                creation,
                MemberCreationProvenance::Successor {
                    predecessor_session_id: first.clone(),
                    predecessor_member_binding: binding("lead"),
                    predecessor_creation_id: creation,
                },
            ),
        ];
        assert_eq!(
            seat_of(&Roster::project(&events), "lead"),
            Some((MemberSeatKind::Host, false, false)),
            "a successor on another session"
        );
        events.push(retired(4, "lead", 1));
        events.push(with_spawned(
            seat_row(
                5,
                "lead",
                2,
                &second,
                creation,
                MemberCreationProvenance::Successor {
                    predecessor_session_id: first,
                    predecessor_member_binding: binding("lead"),
                    predecessor_creation_id: creation,
                },
            ),
            |spawned| {
                spawned.seat_rotation = Some(SeatRotation::successor(
                    Generation::new(1),
                    RotationTarget::SameSession,
                ));
            },
        ));
        assert_eq!(
            seat_of(&Roster::project(&events), "lead"),
            Some((MemberSeatKind::Host, false, false)),
            "a successor on the kept session"
        );
    }

    /// A successor of a seat that reads without a record reads so too.
    #[test]
    fn an_old_successor_of_an_unrecorded_seat_reads_unrecorded() {
        let session = SessionId::new();
        let creation = MemberCreationId::new();
        let roster = Roster::project(&[
            seat_row(
                1,
                "lead",
                0,
                &session,
                creation,
                MemberCreationProvenance::Unproven,
            ),
            retired(2, "lead", 0),
            with_spawned(
                seat_row(
                    3,
                    "lead",
                    1,
                    &session,
                    creation,
                    MemberCreationProvenance::Unproven,
                ),
                |spawned| {
                    spawned.seat_rotation = Some(SeatRotation::respawn_after(Generation::new(0)));
                },
            ),
        ]);
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Unrecorded, false, false))
        );
    }

    /// A row whose creation is unproven or predates creation records says
    /// nothing of who seated the member: a scheduled helper's row of either
    /// form is such a row.
    #[test]
    fn an_old_unproven_or_legacy_row_reads_unrecorded() {
        for provenance in [
            MemberCreationProvenance::Unproven,
            MemberCreationProvenance::LegacyUnknown,
        ] {
            let mut row = seat_row(
                1,
                "helper",
                0,
                &SessionId::new(),
                MemberCreationId::new(),
                provenance.clone(),
            );
            if provenance == MemberCreationProvenance::LegacyUnknown
                && let MobEventKind::MemberSpawned(spawned) = &mut row.kind
            {
                spawned.creation = MemberCreationRecord::default();
            }
            assert_eq!(
                seat_of(&Roster::project(&[row]), "helper"),
                Some((MemberSeatKind::Unrecorded, false, false)),
                "{provenance:?}"
            );
        }
    }

    /// The adoption of a member into the identity store makes its seat the
    /// host's: its former creator no longer reaches it.
    #[test]
    fn an_adoption_makes_the_seat_host() {
        let session = SessionId::new();
        let mut roster = Roster::project(&[fork_row(1, "copy", &session, true)]);
        let identity = AgentIdentity::from("copy");
        let mut adopted = MemberSessionBindingRecoveredEvent::new(
            identity.clone(),
            AgentRuntimeId::new(identity, Generation::new(0)),
            session,
        );
        adopted.identity_entry = Some(crate::runtime::IdentityEntryOrigin::Created);
        roster.apply(&MobEvent {
            cursor: 2,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from(MOB),
            kind: MobEventKind::MemberSessionBindingRecovered(adopted),
        });
        assert_eq!(
            seat_of(&roster, "copy"),
            Some((MemberSeatKind::Host, false, false))
        );
    }

    /// No member seated under a name: no seat. A mob reset forgets every
    /// seat, and a member seated after it is read from its own row.
    #[test]
    fn a_seat_is_read_only_for_a_member_seated_now() {
        let session = SessionId::new();
        let creation = MemberCreationId::new();
        let mut roster = Roster::project(&[
            seat_row(
                1,
                "lead",
                0,
                &session,
                creation,
                MemberCreationProvenance::Root,
            ),
            retired(2, "lead", 0),
        ]);
        assert_eq!(seat_of(&roster, "lead"), None, "retired");
        assert_eq!(seat_of(&roster, "nobody"), None);
        roster.apply(&MobEvent {
            cursor: 3,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from(MOB),
            kind: MobEventKind::MobReset,
        });
        roster.apply(&with_spawned(
            seat_row(
                4,
                "lead",
                0,
                &SessionId::new(),
                MemberCreationId::new(),
                MemberCreationProvenance::Unproven,
            ),
            |spawned| {
                spawned.seat_rotation = Some(SeatRotation::respawn_after(Generation::new(0)));
            },
        ));
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Unrecorded, false, false)),
            "a seat after a mob reset follows nothing before it"
        );
    }

    /// A name reads no seat once its member retired, and a later member
    /// under the name reads its own row: never the earlier member's seat,
    /// also when its row carries a successor step's mark, which names
    /// another incarnation than the one that retired.
    #[test]
    fn a_later_seat_under_a_retired_members_name_reads_its_own_row_never_the_earlier_members() {
        let earlier = [
            seat_row(
                1,
                "lead",
                0,
                &SessionId::new(),
                MemberCreationId::new(),
                MemberCreationProvenance::Root,
            ),
            retired(2, "lead", 0),
        ];
        let later = |cursor: u64| {
            seat_row(
                cursor,
                "lead",
                1,
                &SessionId::new(),
                MemberCreationId::new(),
                MemberCreationProvenance::Spawn {
                    source: MemberCreationSource {
                        session_id: SessionId::new(),
                        member_binding: binding("someone"),
                        creation_id: MemberCreationId::new(),
                    },
                },
            )
        };

        let mut roster = Roster::project(&earlier);
        assert_eq!(seat_of(&roster, "lead"), None, "after the retirement");
        roster.apply(&later(3));
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Member, false, false)),
            "the later member's own row"
        );

        let mut roster = Roster::project(&earlier);
        roster.apply(&with_spawned(later(3), |spawned| {
            spawned.seat_rotation = Some(SeatRotation::respawn_after(Generation::new(7)));
        }));
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Unrecorded, false, false)),
            "a successor step's mark that names another incarnation"
        );
    }

    /// The host attests a seat as its own with the spec's own method, and a
    /// host-root spec carries the attestation too. It stands only off an
    /// agent's call and with no operation owner: a spawn entry that observes
    /// either drops it.
    #[test]
    fn a_host_attests_its_own_seat_and_an_agents_call_or_an_owner_drops_it() {
        use crate::SpawnMemberSpec;
        assert!(
            SpawnMemberSpec::new("worker", "attested")
                .attest_host_seat()
                .seat
                .host_seat,
            "the spec's own method"
        );
        assert!(
            SpawnMemberSpec::host_root("worker", "root").seat.host_seat,
            "a host-root spec"
        );
        assert!(
            !SpawnMemberSpec::new("worker", "plain").seat.host_seat,
            "a plain spec attests nothing"
        );
        for (acts_for_agent, owner_bound, kept) in [
            (false, false, true),
            (true, false, false),
            (false, true, false),
            (true, true, false),
        ] {
            let mut spec = SpawnMemberSpec::new("worker", "entered").attest_host_seat();
            spec.seat.at_entry(acts_for_agent, owner_bound);
            assert_eq!(
                spec.seat.host_seat, kept,
                "acts for an agent: {acts_for_agent}, under an owner: {owner_bound}"
            );
            assert_eq!(spec.seat.acts_for_agent, acts_for_agent);
        }
        // What one entry observed, a later one cannot undo.
        let mut spec = SpawnMemberSpec::new("worker", "twice").attest_host_seat();
        spec.seat.at_entry(true, false);
        spec.seat.at_entry(false, false);
        assert!(!spec.seat.host_seat && spec.seat.acts_for_agent);
    }

    /// The attestation changes nothing of what a spec says of the member's
    /// creation.
    #[test]
    fn attesting_a_host_seat_leaves_the_creation_origin_as_it_was() {
        use crate::SpawnMemberSpec;
        use crate::member_creation::MemberCreationOrigin;
        assert!(matches!(
            SpawnMemberSpec::new("worker", "plain")
                .attest_host_seat()
                .creation_origin,
            MemberCreationOrigin::Unproven
        ));
        assert!(matches!(
            SpawnMemberSpec::host_root("worker", "root")
                .attest_host_seat()
                .creation_origin,
            MemberCreationOrigin::HostRoot
        ));
    }

    /// The binding of `session` a parent's member history answers for the
    /// creator's lineage: `identity` at `generation`, standing at `state`,
    /// with the chain of same-session successor steps that seated it
    /// starting at `chain` (with role migrations followed, and under one
    /// role), or answering none.
    fn lineage_binding_of(
        identity: &str,
        generation: u64,
        session: &SessionId,
        state: crate::MemberBindingState,
        chain: Option<(u64, u64)>,
    ) -> crate::MemberBinding {
        crate::MemberBinding::for_test(
            AgentIdentity::from(identity),
            crate::Generation::new(generation),
            Some(session.clone()),
            state,
            ProfileName::from("worker"),
            crate::SpawnContinuityIntent::Ephemeral,
            crate::MemberIdentityManagement::CommandDriven,
        )
        .with_same_session_lineage(chain.map(|(followed, one_role)| {
            (
                crate::Generation::new(followed),
                crate::Generation::new(one_role),
            )
        }))
    }

    /// How the member that serves a session stands to a mob's recorded
    /// creator: only the creator's own session is asked, at the recorded
    /// generation the record proves it, at a later generation of the same
    /// identity the lineage decides, and everything else proves nothing.
    #[test]
    fn the_recorded_creator_is_proven_only_on_its_session_by_its_identity() {
        let owner_session = SessionId::new();
        let creator_identity = AgentIdentity::from("creator");
        let creator = RecordedCreator {
            identity: &creator_identity,
            generation: crate::Generation::new(2),
            owner_session: &owner_session,
        };
        let seated = |identity: &str, generation: u64| {
            Some((
                AgentIdentity::from(identity),
                crate::Generation::new(generation),
            ))
        };
        let verdict = |session: &SessionId, seated: Option<(AgentIdentity, _)>| {
            creator.seat(session, seated.as_ref())
        };
        assert_eq!(
            verdict(&owner_session, seated("creator", 2)),
            CreatorSeat::Recorded
        );
        assert_eq!(
            verdict(&owner_session, seated("creator", 3)),
            CreatorSeat::Later(crate::Generation::new(3))
        );
        assert_eq!(
            verdict(&owner_session, seated("creator", 7)),
            CreatorSeat::Later(crate::Generation::new(7))
        );
        for (what, session, member) in [
            (
                "an earlier generation of the creator",
                owner_session.clone(),
                seated("creator", 1),
            ),
            (
                "another identity at the recorded generation",
                owner_session.clone(),
                seated("other", 2),
            ),
            (
                "another identity at a later generation",
                owner_session.clone(),
                seated("other", 3),
            ),
            ("no member on the session", owner_session.clone(), None),
            (
                "the creator at the recorded generation on another session",
                SessionId::new(),
                seated("creator", 2),
            ),
            (
                "the creator at a later generation on another session",
                SessionId::new(),
                seated("creator", 3),
            ),
        ] {
            assert_eq!(verdict(&session, member), CreatorSeat::Other, "{what}");
        }
    }

    /// A later generation of the creator on its session is its same-session
    /// successor only when the parent's journal says so: the session's
    /// binding is live, of the creator's identity, at the roster's
    /// generation, and the chain of same-session successor steps that seated
    /// it starts at or before the recorded generation. A chain that starts
    /// later (a step between the two was no same-session successor step), a
    /// binding that answers no chain (a mob reset), a binding that disagrees
    /// with the roster, and a history that names no binding or could not be
    /// read each prove nothing.
    #[test]
    fn a_later_generation_is_the_creators_successor_only_by_an_unbroken_chain() {
        use crate::MemberBindingState::{Live, Moved, Retired};
        let owner_session = SessionId::new();
        let creator_identity = AgentIdentity::from("creator");
        let creator = RecordedCreator {
            identity: &creator_identity,
            generation: crate::Generation::new(2),
            owner_session: &owner_session,
        };
        let current = crate::Generation::new(4);
        let binding = |identity: &str, generation: u64, state, chain| {
            lineage_binding_of(identity, generation, &owner_session, state, chain)
        };
        for (what, chain) in [
            ("the chain starts at the recorded generation", (2, 2)),
            ("the chain starts before the recorded generation", (0, 0)),
            (
                "a role migration after the recorded generation is followed",
                (2, 4),
            ),
            ("a role migration before the recorded generation", (0, 1)),
        ] {
            assert!(
                creator.is_succeeded_by(current, Some(&binding("creator", 4, Live, Some(chain)))),
                "{what}"
            );
        }
        for (what, refused) in [
            (
                "the chain starts one generation after the recorded one",
                Some(binding("creator", 4, Live, Some((3, 3)))),
            ),
            (
                "the chain starts at the current generation",
                Some(binding("creator", 4, Live, Some((4, 4)))),
            ),
            (
                "the binding answers no chain",
                Some(binding("creator", 4, Live, None)),
            ),
            (
                "the binding is retired",
                Some(binding("creator", 4, Retired, Some((2, 2)))),
            ),
            (
                "the binding moved to another session",
                Some(binding("creator", 4, Moved, Some((2, 2)))),
            ),
            (
                "the binding is another identity's",
                Some(binding("other", 4, Live, Some((2, 2)))),
            ),
            (
                "the journal's binding is behind the roster",
                Some(binding("creator", 3, Live, Some((2, 2)))),
            ),
            (
                "the journal's binding is ahead of the roster",
                Some(binding("creator", 5, Live, Some((2, 2)))),
            ),
            ("the history names no binding of the session", None),
        ] {
            assert!(
                !creator.is_succeeded_by(current, refused.as_ref()),
                "{what}"
            );
        }

        // The history read's own answers: the session's binding when it
        // reads, none when it names no such binding, and none when it could
        // not be read.
        let parent = MobId::from("lineage-parent");
        let read = crate::MemberBindingHistory::for_test(
            [],
            [binding("creator", 4, Live, Some((2, 2)))],
            [],
            7,
        );
        let found = lineage_binding(Ok(read.clone()), &parent, &owner_session);
        assert!(creator.is_succeeded_by(current, found.as_ref()));
        assert!(lineage_binding(Ok(read), &parent, &SessionId::new()).is_none());
        let unread = lineage_binding(
            Err(MobError::Internal("the journal cannot be read".to_string())),
            &parent,
            &owner_session,
        );
        assert!(unread.is_none());
        assert!(!creator.is_succeeded_by(current, unread.as_ref()));

        // The one decision about a role migration, as it is ruled: followed.
        // The chain a reader that follows role migrations reads starts at 2,
        // and the one under one role at 4.
        assert_eq!(
            binding("creator", 4, Live, Some((2, 4)))
                .same_session_lineage_start(creator_lineage_steps()),
            Some(crate::Generation::new(2))
        );
    }

    /// An entry at another generation than the reading's row reads no seat:
    /// here a member reset moved the entry's generation with no seat row.
    #[test]
    fn an_entry_at_another_generation_than_its_reading_reads_no_seat() {
        let identity = AgentIdentity::from("lead");
        let mut roster = Roster::project(&[seat_row(
            1,
            "lead",
            0,
            &SessionId::new(),
            MemberCreationId::new(),
            MemberCreationProvenance::Root,
        )]);
        assert_eq!(
            seat_of(&roster, "lead"),
            Some((MemberSeatKind::Host, false, false))
        );
        roster.apply(&MobEvent {
            cursor: 2,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from(MOB),
            kind: MobEventKind::MemberReset {
                agent_identity: identity.clone(),
                previous_generation: Generation::new(0),
                new_generation: Generation::new(1),
                fence_token: FenceToken::new(2),
                agent_runtime_id: AgentRuntimeId::new(identity, Generation::new(1)),
            },
        });
        assert_eq!(seat_of(&roster, "lead"), None);
    }

    fn creator_seat(mob: &str, member: &str, session: &SessionId) -> RecordedSeat {
        let mut recorded = RecordedSeat::new(
            MobId::from(mob),
            meerkat_core::service::MobIncarnationId::mint(),
            AgentIdentity::from(member),
            Generation::new(3),
            session.clone(),
        )
        .with_creator_life(crate::runtime::IdentityLife::from_token("2.5").expect("a marker"));
        recorded.creator_creation = Some(MemberCreationId::new());
        recorded
    }

    fn sealed(recorded: &RecordedSeat, sealed_by: SeatSealer) -> Option<MemberSeatCreator> {
        Some(MemberSeatCreator {
            seal: SeatCreatorSeal::One {
                recorded: recorded.clone(),
                sealed_by,
            },
        })
    }

    fn observed(change: impl FnOnce(&mut SeatObservations)) -> SeatObservations {
        let mut observations = SeatObservations::default();
        change(&mut observations);
        observations
    }

    fn under_owner(session: &SessionId) -> SeatStepFacts {
        SeatStepFacts {
            owner_session: Some(session.clone()),
            ..SeatStepFacts::default()
        }
    }

    fn kind_of(observations: &SeatObservations, facts: &SeatStepFacts) -> Option<MemberSeatKind> {
        compose_seat(observations, facts).map(|seat| seat.kind())
    }

    /// Each kind of seat, from what the runtime observed of it.
    #[test]
    fn a_seat_records_its_kind_from_what_the_runtime_observed() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let plain = SeatStepFacts::default();
        let owner = under_owner(&session);
        let for_caller =
            |creator: Option<MemberSeatCreator>| observed(|seat| seat.creator = creator);
        let agent = |creator: Option<MemberSeatCreator>| {
            observed(|seat| {
                seat.acts_for_agent = true;
                seat.creator = creator;
            })
        };
        let rows: Vec<(&str, SeatObservations, &SeatStepFacts, Kind)> = vec![
            (
                "the host's own attested seat",
                observed(|seat| seat.host_seat = true),
                &plain,
                Kind::Host,
            ),
            (
                "a seat nobody attested",
                SeatObservations::default(),
                &plain,
                Kind::Unproven,
            ),
            (
                "a host's seat under an owner, for a member",
                for_caller(sealed(&creator, SeatSealer::Set)),
                &owner,
                Kind::HostForMember,
            ),
            (
                "a host's seat under an owner, for a session no roster names",
                for_caller(Some(MemberSeatCreator::no_roster(session.clone()))),
                &owner,
                Kind::HostForSession,
            ),
            (
                "a host's seat under an owner, several rosters",
                for_caller(Some(MemberSeatCreator::several())),
                &owner,
                Kind::Ambiguous,
            ),
            (
                "a host's seat under an owner, the lookup unavailable",
                for_caller(Some(MemberSeatCreator::unavailable())),
                &owner,
                Kind::Unproven,
            ),
            (
                "a host's seat under an owner, nothing sealed",
                for_caller(None),
                &owner,
                Kind::Unproven,
            ),
            (
                "a seat its door sealed a calling member for, with no owner",
                for_caller(sealed(&creator, SeatSealer::Set)),
                &plain,
                Kind::HostForMember,
            ),
            (
                "an attested seat its door sealed a calling member for",
                observed(|seat| {
                    seat.host_seat = true;
                    seat.creator = sealed(&creator, SeatSealer::Set);
                }),
                &plain,
                Kind::HostForMember,
            ),
            (
                "a seat its door sealed a caller for, the lookup unavailable",
                observed(|seat| {
                    seat.host_seat = true;
                    seat.creator = Some(MemberSeatCreator::unavailable());
                }),
                &plain,
                Kind::Unproven,
            ),
            (
                "a member's call, sealed by the set",
                agent(sealed(&creator, SeatSealer::Set)),
                &owner,
                Kind::Member,
            ),
            (
                "a member's call at its mob's own tools, with no owner",
                agent(sealed(&creator, SeatSealer::Mob)),
                &plain,
                Kind::Member,
            ),
            (
                "a call of a session no roster names",
                agent(Some(MemberSeatCreator::no_roster(session.clone()))),
                &owner,
                Kind::Session,
            ),
            (
                "a call of a session no roster names, with no operation owner",
                agent(Some(MemberSeatCreator::no_roster(session.clone()))),
                &plain,
                Kind::Session,
            ),
            (
                "a call of a session no roster names, under another session's operation",
                agent(Some(MemberSeatCreator::no_roster(SessionId::new()))),
                &owner,
                Kind::Unproven,
            ),
            (
                "an agent's call, several rosters",
                agent(Some(MemberSeatCreator::several())),
                &owner,
                Kind::Ambiguous,
            ),
            (
                "an agent's call, the lookup unavailable",
                agent(Some(MemberSeatCreator::unavailable())),
                &owner,
                Kind::Unproven,
            ),
            (
                "an agent's call, nothing sealed",
                agent(None),
                &owner,
                Kind::Unproven,
            ),
        ];
        for (what, observations, facts, kind) in rows {
            assert_eq!(kind_of(&observations, facts), Some(kind), "{what}");
        }
        let definition = SeatStepFacts {
            definition_seat: true,
            ..SeatStepFacts::default()
        };
        assert_eq!(
            kind_of(&SeatObservations::default(), &definition),
            Some(Kind::Definition),
            "the mob's own definition"
        );
        let fork = SeatStepFacts {
            fork_seat: true,
            fork_source: true,
            ..SeatStepFacts::default()
        };
        assert_eq!(
            kind_of(&SeatObservations::default(), &fork),
            Some(Kind::ForkOf),
            "a copy nobody's call made for an agent"
        );
        let unnamed_copy = SeatStepFacts {
            fork_seat: true,
            ..SeatStepFacts::default()
        };
        for host_seat in [false, true] {
            assert_eq!(
                kind_of(&observed(|seat| seat.host_seat = host_seat), &unnamed_copy),
                Some(Kind::Unproven),
                "a copy whose row names no source shows nothing, attested or not"
            );
        }
        assert_eq!(
            kind_of(&observed(|seat| seat.host_seat = true), &fork),
            Some(Kind::ForkOf),
            "an attested copy is a copy still"
        );
        // A schedule's seat is its own kind whatever else holds.
        let scheduled = observed(|seat| {
            seat.scheduled = true;
            seat.host_seat = true;
            seat.creator = sealed(&creator, SeatSealer::Set);
        });
        for facts in [&plain, &owner, &definition, &fork] {
            assert_eq!(kind_of(&scheduled, facts), Some(Kind::Scheduled));
        }
        // A member placed on another host: nothing of who asked is kept.
        let placed = SeatStepFacts {
            placed: true,
            ..SeatStepFacts::default()
        };
        assert_eq!(
            kind_of(&observed(|seat| seat.host_seat = true), &placed),
            Some(Kind::Unproven)
        );
    }

    /// What a record holds beside its kind: the sealed creator and who sealed
    /// it on the two kinds that have one, the calling session on the two a
    /// bare session asked for, and the caller's own turn only on its own
    /// copy. Every record the seat step composes is shell recorded.
    #[test]
    fn a_seat_record_holds_the_sealed_creator_and_who_sealed_it() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let owner = under_owner(&session);
        let member = compose_seat(
            &observed(|seat| {
                seat.acts_for_agent = true;
                seat.creator = sealed(&creator, SeatSealer::Set);
            }),
            &owner,
        )
        .expect("a record");
        assert_eq!(member.kind(), Kind::Member);
        assert_eq!(member.recorded(), Some(&creator));
        assert_eq!(member.sealed_by(), Some(SeatSealer::Set));
        assert_eq!(member.recorded_by(), Some(SeatRecorder::Shell));
        assert!(!member.own_turn());
        assert_eq!(member.caller_session(), None);

        let own_copy = |fork_seat: bool, own_turn: bool| {
            compose_seat(
                &observed(|seat| {
                    seat.acts_for_agent = true;
                    seat.creator = sealed(&creator, SeatSealer::Mob);
                }),
                &SeatStepFacts {
                    fork_seat,
                    own_turn,
                    ..SeatStepFacts::default()
                },
            )
            .expect("a record")
        };
        let copy = own_copy(true, true);
        assert_eq!((copy.kind(), copy.own_turn()), (Kind::Member, true));
        assert_eq!(copy.sealed_by(), Some(SeatSealer::Mob));
        // An agent's call that copies another member: its creator is the
        // caller, and it is not the caller's own copy.
        let others_copy = own_copy(true, false);
        assert_eq!(
            (others_copy.kind(), others_copy.own_turn()),
            (Kind::Member, false)
        );
        assert_eq!(others_copy.recorded(), Some(&creator));
        assert!(!own_copy(false, true).own_turn(), "no copy, no own turn");

        let for_member = compose_seat(
            &observed(|seat| seat.creator = sealed(&creator, SeatSealer::Set)),
            &owner,
        )
        .expect("a record");
        assert_eq!(for_member.kind(), Kind::HostForMember);
        assert_eq!(for_member.recorded(), Some(&creator));
        assert_eq!(for_member.sealed_by(), Some(SeatSealer::Set));

        for (acts_for_agent, kind) in [(true, Kind::Session), (false, Kind::HostForSession)] {
            let seat = compose_seat(
                &observed(|seat| {
                    seat.acts_for_agent = acts_for_agent;
                    seat.creator = Some(MemberSeatCreator::no_roster(session.clone()));
                }),
                &owner,
            )
            .expect("a record");
            assert_eq!(seat.kind(), kind);
            assert_eq!(seat.caller_session(), Some(&session));
            assert_eq!(seat.recorded(), None);
            assert_eq!(seat.sealed_by(), None);
        }

        let host = compose_seat(
            &observed(|seat| seat.host_seat = true),
            &SeatStepFacts::default(),
        )
        .expect("a record");
        assert_eq!(host, MemberSeat::shell(Kind::Host));
        assert_eq!(host.recorded_by(), Some(SeatRecorder::Shell));
    }

    /// Over every combination of what the seat step observes: the seat is the
    /// host's own only with the host's attestation, off an agent's call, with
    /// no operation owner, no sealed caller, no copy, no schedule and no
    /// placement; and every record holds exactly the parts its kind has.
    #[test]
    fn no_agents_owners_schedules_or_copied_seat_is_ever_the_hosts() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let creators = [
            None,
            Some(MemberSeatCreator::unavailable()),
            Some(MemberSeatCreator::no_roster(session.clone())),
            Some(MemberSeatCreator::several()),
            sealed(&creator, SeatSealer::Set),
        ];
        let mut hosts = 0;
        for bits in 0_u32..256 {
            let bit = |n: u32| bits & (1 << n) != 0;
            for creator in &creators {
                let observations = observed(|seat| {
                    seat.host_seat = bit(0);
                    seat.acts_for_agent = bit(1);
                    seat.scheduled = bit(2);
                    seat.creator = creator.clone();
                });
                let facts = SeatStepFacts {
                    owner_session: bit(3).then(|| session.clone()),
                    definition_seat: bit(4),
                    fork_seat: bit(5),
                    fork_source: bit(5),
                    own_turn: bit(6),
                    placed: bit(7),
                    same_mob_bindings_agree: None,
                };
                let seat = compose_seat(&observations, &facts).expect("a record");
                let host = bit(0)
                    && !bit(1)
                    && !bit(2)
                    && !bit(3)
                    && !bit(5)
                    && !bit(7)
                    && creator.is_none();
                assert_eq!(
                    seat.kind() == Kind::Host,
                    host,
                    "{observations:?} {facts:?}"
                );
                hosts += usize::from(host);
                assert_eq!(seat.recorded_by(), Some(SeatRecorder::Shell));
                let has_creator = matches!(seat.kind(), Kind::Member | Kind::HostForMember);
                assert_eq!(seat.recorded().is_some(), has_creator);
                assert_eq!(seat.sealed_by().is_some(), has_creator);
                assert_eq!(
                    seat.caller_session().is_some(),
                    matches!(seat.kind(), Kind::Session | Kind::HostForSession)
                );
                assert!(!seat.own_turn() || seat.kind() == Kind::Member);
            }
        }
        assert_eq!(hosts, 4, "the host's seat is reached");
    }

    /// A sealed creator of the seat's own mob is taken only when MobMachine's
    /// own bindings bear it out, and a sealed creator is taken only for the
    /// session that owns the operation.
    #[test]
    fn a_sealed_creator_the_runtime_cannot_bear_out_is_unproven() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat(MOB, "lead", &session);
        for acts_for_agent in [true, false] {
            let observations = observed(|seat| {
                seat.acts_for_agent = acts_for_agent;
                seat.creator = sealed(&creator, SeatSealer::Mob);
            });
            let with_bindings = |agree: bool| SeatStepFacts {
                same_mob_bindings_agree: Some(agree),
                ..under_owner(&session)
            };
            assert_eq!(
                kind_of(&observations, &with_bindings(false)),
                Some(Kind::Unproven),
                "the machine's bindings disagree"
            );
            let agreed = kind_of(&observations, &with_bindings(true));
            assert!(
                matches!(agreed, Some(Kind::Member | Kind::HostForMember)),
                "{agreed:?}"
            );
            assert_eq!(
                kind_of(&observations, &under_owner(&SessionId::new())),
                Some(Kind::Unproven),
                "another session owns the operation"
            );
        }
    }

    /// A runtime successor's seat keeps the record of the seat it follows
    /// whatever its own step observed, and stores none after a seat whose
    /// row holds none.
    #[test]
    fn a_successor_keeps_the_record_of_the_seat_it_follows() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let followed =
            MemberSeat::shell_with_creator(Kind::Member, creator.clone(), SeatSealer::Set, false);
        assert_eq!(
            CarriedSeat::of(Some(&followed)),
            CarriedSeat::Kept(followed.clone())
        );
        assert_eq!(
            CarriedSeat::of(Some(&MemberSeat::from_older_row(Kind::Host, false))),
            CarriedSeat::OlderRow
        );
        assert_eq!(
            CarriedSeat::of(None),
            CarriedSeat::Kept(MemberSeat::shell(Kind::Unproven)),
            "a seat with no reading is followed by a record that shows nothing"
        );
        let mut observations = SeatObservations::successor(CarriedSeat::Kept(followed.clone()));
        observations.host_seat = true;
        observations.scheduled = true;
        let facts = SeatStepFacts {
            placed: true,
            ..under_owner(&session)
        };
        assert_eq!(compose_seat(&observations, &facts), Some(followed));
        assert_eq!(
            compose_seat(
                &SeatObservations::successor(CarriedSeat::OlderRow),
                &SeatStepFacts::default()
            ),
            None
        );
    }

    /// The stored form of a record: its words, and the record it reads back
    /// as. A seat read from a row that holds no record has no stored form.
    #[test]
    fn a_seat_record_round_trips_its_stored_form() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let records = [
            MemberSeat::shell(Kind::Host),
            MemberSeat::shell(Kind::Definition),
            MemberSeat::shell(Kind::ForkOf),
            MemberSeat::shell(Kind::Scheduled),
            MemberSeat::shell(Kind::Ambiguous),
            MemberSeat::shell(Kind::Unproven),
            MemberSeat::shell_for_session(Kind::Session, session.clone()),
            MemberSeat::shell_for_session(Kind::HostForSession, session.clone()),
            MemberSeat::shell_with_creator(Kind::Member, creator.clone(), SeatSealer::Set, false),
            MemberSeat::shell_with_creator(Kind::Member, creator.clone(), SeatSealer::Mob, true),
            MemberSeat::shell_with_creator(
                Kind::HostForMember,
                creator.clone(),
                SeatSealer::Set,
                false,
            ),
        ];
        for record in &records {
            let stored = record.to_stored().expect("a record has a stored form");
            let text = serde_json::to_string(&stored).expect("encode");
            let read: StoredSeatLineage = serde_json::from_str(&text).expect("decode");
            assert_eq!(&MemberSeat::from_stored(&read), record, "{text}");
        }
        assert_eq!(
            serde_json::to_value(records[0].to_stored().expect("stored")).expect("encode"),
            serde_json::json!({ "recorded_by": "shell", "kind": "host" })
        );
        assert_eq!(
            serde_json::to_value(records[9].to_stored().expect("stored")).expect("encode"),
            serde_json::json!({
                "recorded_by": "shell",
                "kind": "member",
                "creator": {
                    "parent_mob_id": "elsewhere",
                    "parent_incarnation": creator.parent_incarnation.to_string(),
                    "creator_identity": "lead",
                    "creator_generation": 3,
                    "owner_session": session,
                    "creator_life": "2.5",
                    "creator_creation": creator.creator_creation,
                },
                "own_turn": true,
                "sealed_by": "mob",
            })
        );
        let words = |record: &MemberSeat| {
            let stored = serde_json::to_value(record.to_stored().expect("stored")).expect("encode");
            (
                stored["kind"].as_str().unwrap_or_default().to_string(),
                stored["sealed_by"].as_str().map(str::to_string),
            )
        };
        assert_eq!(words(&records[6]).0, "session");
        assert_eq!(words(&records[7]).0, "host_for_session");
        assert_eq!(
            words(&records[10]),
            ("host_for_member".to_string(), Some("set".to_string()))
        );
        assert_eq!(words(&records[1]).0, "definition");
        assert_eq!(words(&records[2]).0, "fork_of");
        assert_eq!(words(&records[3]).0, "scheduled");
        assert_eq!(words(&records[4]).0, "ambiguous");
        assert_eq!(words(&records[5]).0, "unproven");
        assert_eq!(
            MemberSeat::from_older_row(Kind::Host, false).to_stored(),
            None
        );
    }

    /// A stored record this build cannot read whole reads closed: who seated
    /// the member could not be shown. What was stored is kept as stored.
    #[test]
    fn a_stored_record_this_build_cannot_read_whole_reads_unproven_and_is_kept() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let member = MemberSeat::shell_with_creator(Kind::Member, creator, SeatSealer::Set, false)
            .to_stored()
            .expect("stored");
        let member = serde_json::to_value(member).expect("encode");
        let changed = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value = member.clone();
            change(&mut value);
            value
        };
        let unreadable = [
            serde_json::json!({ "recorded_by": "shell", "kind": "a_later_kind", "later": [1] }),
            serde_json::json!({ "recorded_by": "a_later_recorder", "kind": "host" }),
            changed(&|value| value["sealed_by"] = "a_later_sealer".into()),
            changed(&|value| value["creator"]["parent_incarnation"] = "not-an-incarnation".into()),
            changed(&|value| value["creator"]["creator_life"] = "1".into()),
            changed(&|value| {
                value.as_object_mut().expect("an object").remove("creator");
            }),
            changed(&|value| value["kind"] = "host".into()),
            changed(&|value| value["kind"] = "host_for_session".into()),
            serde_json::json!({ "recorded_by": "shell", "kind": "session" }),
            serde_json::json!({ "recorded_by": "shell", "kind": "host", "own_turn": true }),
            changed(&|value| value["creator"] = 5.into()),
            serde_json::json!({ "kind": "host" }),
            serde_json::json!("no record at all"),
        ];
        for value in unreadable {
            let stored: StoredSeatLineage =
                serde_json::from_value(value.clone()).expect("the stored form decodes");
            let read = MemberSeat::from_stored(&stored);
            assert_eq!(read.kind(), Kind::Unproven, "{value}");
            assert_eq!(read.recorded(), None, "{value}");
            assert!(!read.own_turn(), "{value}");
            assert_eq!(
                serde_json::to_value(&stored).expect("encode"),
                value,
                "written back as stored"
            );
        }
        let later = changed(&|value| {
            value["a_later_member"] = "kept".into();
            value["creator"]["a_later_part"] = 7.into();
        });
        let stored: StoredSeatLineage = serde_json::from_value(later.clone()).expect("decode");
        assert_eq!(MemberSeat::from_stored(&stored).kind(), Kind::Member);
        assert_eq!(serde_json::to_value(&stored).expect("encode"), later);
    }

    /// A seat row that stores a record is read from it, whatever its older
    /// members would read as; a row that stores none reads by the older
    /// rows' rule.
    #[test]
    fn a_row_that_stores_a_seat_record_is_read_from_it() {
        use MemberSeatKind as Kind;
        let session = SessionId::new();
        let creator = creator_seat("elsewhere", "lead", &session);
        let record =
            MemberSeat::shell_with_creator(Kind::Member, creator.clone(), SeatSealer::Set, false);
        let row = |record: Option<&MemberSeat>| {
            with_spawned(
                seat_row(
                    1,
                    "worker",
                    0,
                    &SessionId::new(),
                    MemberCreationId::new(),
                    MemberCreationProvenance::Root,
                ),
                |spawned| spawned.seat_lineage = record.and_then(MemberSeat::to_stored),
            )
        };
        let roster = Roster::project(&[row(Some(&record))]);
        assert_eq!(roster.seat(&AgentIdentity::from("worker")), Some(&record));
        let roster = Roster::project(&[row(Some(&MemberSeat::shell(Kind::Unproven)))]);
        assert_eq!(
            roster.seat(&AgentIdentity::from("worker")),
            Some(&MemberSeat::shell(Kind::Unproven))
        );
        let roster = Roster::project(&[row(None)]);
        assert_eq!(
            roster.seat(&AgentIdentity::from("worker")),
            Some(&MemberSeat::from_older_row(Kind::Host, false))
        );
    }
}
