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

use std::sync::Arc;

use meerkat_mob::temporary_council::{TEMPORARY_COUNCIL_MOB_ID_PREFIX, TemporaryCouncilId};

use super::RuntimeEntry;

/// Whether a council recorded in `state`'s council store owns the mob
/// `mob_id` as its temporary mob. One read of the council store, for a mob
/// whose id has the form Core derives; no mob, member or session is loaded.
/// A store that cannot be read answers `true`: the mob's members stay
/// hidden rather than listed for want of the read.
pub(super) async fn council_owns_mob(
    state: &Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: &str,
) -> bool {
    let Some(council) = mob_id.strip_prefix(TEMPORARY_COUNCIL_MOB_ID_PREFIX) else {
        return false;
    };
    // Core admits no council under an id it refuses, so none owns the mob.
    let Ok(council_id) = TemporaryCouncilId::new(council) else {
        return false;
    };
    match state.temporary_council().load(&council_id).await {
        Ok(record) => record.is_some_and(|record| record.temporary_mob_id.as_str() == mob_id),
        Err(_) => true,
    }
}

/// Whether a stored frame whose captured provenance names `source_mob_id`
/// was written by a council participant of `entry`'s runtime.
///
/// A council's record decides it while the record exists. A host whose
/// councils are process-bound loses the records at a restart, so a mob id
/// of the council form that names no mob the runtime holds is decided as a
/// council's too: only a mob the runtime holds now, with no council record,
/// is an ordinary mob that merely carries such an id, and keeps its frames.
pub(super) async fn frame_source_is_council_mob(entry: &RuntimeEntry, source_mob_id: &str) -> bool {
    if !source_mob_id.starts_with(TEMPORARY_COUNCIL_MOB_ID_PREFIX) {
        return false;
    }
    let primary_mob_id = entry.runtime.handle().mob_id().to_string();
    let Some(state) = entry.runtime.agent_mob_mcp_state() else {
        return source_mob_id != primary_mob_id;
    };
    if council_owns_mob(&state, source_mob_id).await {
        return true;
    }
    if source_mob_id == primary_mob_id {
        return false;
    }
    !Box::pin(state.mob_handles_snapshot())
        .await
        .unwrap_or_default()
        .iter()
        .any(|(mob_id, _)| mob_id.as_str() == source_mob_id)
}
