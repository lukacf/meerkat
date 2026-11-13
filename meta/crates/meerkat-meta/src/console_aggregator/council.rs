//! Temporary council participants are never console members.
//!
//! A council seats its participants in a temporary mob of its own. Each is a
//! fork of its source member, so its session begins with a copy of the
//! source's earlier turns, while nothing links it to its source for an
//! access decision: it would be decided on its own name, its council role
//! and the labels it inherits. On every visibility policy the console
//! therefore gives a participant the path the policy that hides nested
//! members gives every nested member: it is not listed, no member action
//! resolves it, its history is never read, and a frame stored for one is not
//! served.
//!
//! Which mob is a council's is the council's own fact: the record its
//! lifecycle machine keeps in the runtime's council store names the
//! temporary mob it owns. The record is looked up under the council id Core
//! derives the mob id from; the id's form alone decides nothing for a mob
//! the runtime holds.
//!
//! The record outlives the council (it answers a replay of the same
//! request), and a mob id is free again once its mob is destroyed. A council
//! settles only after its cleanup destroyed the temporary mob, so a settled
//! record owns no mob the runtime holds now: one held under that id was
//! created since and is an ordinary mob. The record still names the settled
//! council's seats and when it settled: the frames stored for those seats
//! stay unserved, and so does every frame stored under the id before the
//! council settled, since the mob held now did not exist then.

use std::sync::Arc;

use meerkat_core::service::MobIncarnationId;
use meerkat_mob::forked_participant::ForkedParticipantRef;
use meerkat_mob::temporary_council::{
    TEMPORARY_COUNCIL_MOB_ID_PREFIX, TemporaryCouncilId, TemporaryCouncilParticipantCustody,
};

use super::{ConsoleFrame, ConsoleFrameMemberProvenance, RuntimeEntry};

/// What the runtime's council store holds for one mob id.
enum CouncilRecord {
    /// No council's record names the mob.
    Absent,
    /// A council that has not settled names the mob: the mob held under the
    /// id is that council's. A store that cannot be read answers this too,
    /// so the mob's members stay hidden rather than listed for want of the
    /// read.
    Owns,
    /// The council that named the mob settled: its cleanup destroyed the
    /// temporary mob and owes nothing more.
    Settled(CouncilSeats),
}

/// A settled council, as its record names it: each seat's name in the
/// temporary mob, the fork sessions the record holds for the seats, and when
/// the cleanup that settled it ran.
#[derive(Debug, Default)]
pub(super) struct CouncilSeats {
    names: Vec<String>,
    sessions: Vec<String>,
    /// The time of the settling cleanup, as a frame's time. The council's
    /// mob was destroyed before it, so a mob the runtime holds under the id
    /// now was created after it.
    settled_at_ms: u64,
}

impl CouncilSeats {
    fn of(participants: &[TemporaryCouncilParticipantCustody], settled_at_ms: u64) -> Self {
        let mut seats = Self {
            settled_at_ms,
            ..Self::default()
        };
        for participant in participants {
            // The console names a member by its public alias.
            seats.names.push(
                crate::member_comms_id::runtime_alias_str(participant.target_identity.as_str())
                    .into_owned(),
            );
            seats.sessions.extend(
                participant
                    .seated_session_id
                    .iter()
                    .chain(
                        participant
                            .capability_ref
                            .iter()
                            .map(ForkedParticipantRef::fork_session_id),
                    )
                    .map(ToString::to_string),
            );
        }
        seats
    }

    /// Whether the settled council wrote `frame`, rather than a mob created
    /// under its id since: the frame is no newer than the settling cleanup
    /// (the mob held under the id now did not exist then, whoever the
    /// frame's member was), or its captured member has a seat's name, or it
    /// runs on a seat's fork session. Any one is enough, so a seat whose
    /// session the record never came to hold is still decided by its name.
    fn wrote(&self, frame: &ConsoleFrame, provenance: &ConsoleFrameMemberProvenance) -> bool {
        let before_settled = frame.timestamp_ms <= self.settled_at_ms;
        let named = self.names.iter().any(|name| {
            *name == provenance.member.agent_identity
                || *name == provenance.identity.runtime_member_id
        });
        let on_seat_session = [
            frame.session_id.as_ref(),
            provenance.member.session_id.as_ref(),
            provenance.identity.session_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|session| self.sessions.contains(session));
        before_settled || named || on_seat_session
    }
}

/// The council record for `mob_id` in `state`'s council store. One read of
/// the store, for a mob whose id has the form Core derives; no mob, member
/// or session is loaded.
async fn council_record(state: &Arc<meerkat_mob_mcp::MobMcpState>, mob_id: &str) -> CouncilRecord {
    let Some(council) = mob_id.strip_prefix(TEMPORARY_COUNCIL_MOB_ID_PREFIX) else {
        return CouncilRecord::Absent;
    };
    // Core admits no council under an id it refuses, so none owns the mob.
    let Ok(council_id) = TemporaryCouncilId::new(council) else {
        return CouncilRecord::Absent;
    };
    let record = match state.temporary_council().load(&council_id).await {
        Ok(Some(record)) if record.temporary_mob_id.as_str() == mob_id => record,
        Ok(_) => return CouncilRecord::Absent,
        Err(_) => return CouncilRecord::Owns,
    };
    // Settled is the council machine's own verdict, and it is reached only
    // by a cleanup that found the temporary mob destroyed: that cleanup's
    // receipt holds when.
    let destroyed_at = record
        .cleanup
        .as_ref()
        .filter(|cleanup| cleanup.temporary_mob_destroyed)
        .map(|cleanup| cleanup.attempted_at);
    let Some(settled_at) = destroyed_at else {
        return CouncilRecord::Owns;
    };
    if record.is_unfinished() {
        return CouncilRecord::Owns;
    }
    // A time no frame time can hold reads as later than every frame.
    let settled_at_ms = u64::try_from(settled_at.timestamp_millis()).unwrap_or(u64::MAX);
    CouncilRecord::Settled(CouncilSeats::of(&record.participants, settled_at_ms))
}

/// Whether the runtime holds, now, a mob of `incarnation` under `mob_id`: a
/// mob keeps its incarnation in its creation record from its create to its
/// destroy.
async fn holds_incarnation(
    state: &Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: &str,
    incarnation: MobIncarnationId,
) -> bool {
    Box::pin(state.mob_handles_snapshot())
        .await
        .unwrap_or_default()
        .iter()
        .any(|(held_id, held)| held_id.as_str() == mob_id && held.incarnation() == incarnation)
}

/// Whether a council recorded in `state`'s council store owns the mob of
/// `incarnation`, which the runtime listed under `mob_id`, as its temporary
/// mob. The listing passes the incarnation of the handle it listed.
///
/// A record that has not settled owns the mob held under its id. A settled
/// record owns none the runtime holds now: its council destroyed its mob
/// before it settled, and a destroy and a create under one id start a new
/// incarnation. So the listed mob is an ordinary mob when the runtime still
/// holds its incarnation after the record read settled; an incarnation
/// listed before that destroy names no mob the runtime holds now, and stays
/// a council's.
pub(super) async fn council_owns_mob(
    state: &Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: &str,
    incarnation: MobIncarnationId,
) -> bool {
    match council_record(state, mob_id).await {
        CouncilRecord::Absent => false,
        CouncilRecord::Owns => true,
        CouncilRecord::Settled(_) => !holds_incarnation(state, mob_id, incarnation).await,
    }
}

/// Which of the stored frames whose captured provenance names one source
/// mob were written by a council participant.
#[derive(Debug)]
pub(super) enum CouncilFrames {
    /// None: the source mob is an ordinary mob, or the primary one.
    Ordinary,
    /// Every one.
    All,
    /// Those a settled council wrote: its seats' frames, and every frame
    /// from before it settled. An ordinary mob holds the id now, and the
    /// frames its members wrote since are no council's.
    Seats(CouncilSeats),
}

impl CouncilFrames {
    /// Whether `frame`, whose captured `provenance` names the source mob
    /// this was decided for, was written by a council participant.
    pub(super) fn wrote(
        &self,
        frame: &ConsoleFrame,
        provenance: &ConsoleFrameMemberProvenance,
    ) -> bool {
        match self {
            Self::Ordinary => false,
            Self::All => true,
            Self::Seats(seats) => seats.wrote(frame, provenance),
        }
    }
}

/// Which stored frames whose captured provenance names `source_mob_id` were
/// written by a council participant of `entry`'s runtime.
///
/// A council's record decides it while the record exists. A host whose
/// councils are process-bound loses the records at a restart, so a mob id
/// of the council form that names no mob the runtime holds is decided as a
/// council's too: only a mob the runtime holds now is an ordinary mob that
/// merely carries such an id, and keeps its frames. With no council record
/// it keeps them all; under the id of a council that settled it keeps all
/// but the frames that council wrote (its seats', and every frame from
/// before it settled).
pub(super) async fn council_frames_of_source(
    entry: &RuntimeEntry,
    source_mob_id: &str,
) -> CouncilFrames {
    if !source_mob_id.starts_with(TEMPORARY_COUNCIL_MOB_ID_PREFIX) {
        return CouncilFrames::Ordinary;
    }
    let primary_mob_id = entry.runtime.handle().mob_id().to_string();
    let Some(state) = entry.runtime.agent_mob_mcp_state() else {
        return if source_mob_id == primary_mob_id {
            CouncilFrames::Ordinary
        } else {
            CouncilFrames::All
        };
    };
    let held = match council_record(&state, source_mob_id).await {
        CouncilRecord::Owns => return CouncilFrames::All,
        CouncilRecord::Absent => CouncilFrames::Ordinary,
        CouncilRecord::Settled(seats) => CouncilFrames::Seats(seats),
    };
    let holds = source_mob_id == primary_mob_id
        || Box::pin(state.mob_handles_snapshot())
            .await
            .unwrap_or_default()
            .iter()
            .any(|(mob_id, _)| mob_id.as_str() == source_mob_id);
    if holds { held } else { CouncilFrames::All }
}
