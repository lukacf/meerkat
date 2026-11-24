//! The experience over every mob a caller may address.
//!
//! `GET /console/experience?mobs=*` answers the launch mob's document, as a
//! request without the selector does, and adds what the host serves beside
//! the launch mob to a caller who may address it:
//!
//! - `topology.live_snapshot.mobs`: the launch mob and each other mob the
//!   host serves that the caller may address
//!   ([`MobsSelector::listed`]: one evaluation of the caller's view per
//!   mob), each with who created it, its state, its incarnation where one
//!   is known, and, for a mob that is loaded, the number of its member rows
//!   in this document;
//! - in the member rows: the members of every such mob that is loaded now,
//!   each read and judged with the caller's view in its own mob, keyed by
//!   its row key and naming its own mob.
//!
//! It loads nothing. The list is one read of the host's records of its
//! mobs, and a mob's members are read only from a mob that is loaded
//! ([`crate::unified_runtime::MobResolver::resolve_if_loaded`]): a mob that
//! is not loaded contributes its entry and no row, and stays not loaded. A
//! periodic read is never a use of a mob.
//!
//! A mob the caller may not address contributes nothing: no entry, no row
//! and no count, so the document is the one of a host without that mob.

use serde_json::{Value, json};

use super::row_key::key_member_row;
use super::{
    ConsoleHttpAuthContext, ConsoleJsonState, ConsoleRestJsonRequest, MobsSelector,
    console_document_of, console_launch_mob,
};
use crate::ConsoleAccessRequest;
use crate::rpc::mob_addressing::DoorCaller;
use crate::unified_runtime::{HostedMobListing, HostedMobPhase, MobResolver, ResolvedMob};

/// Where the experience lists its member rows.
const MEMBER_ROWS: &str = "/agent_sidebar/live_snapshot/agents";

/// The kind of a member's row (a module's row is of another kind and names
/// no member).
const MEMBER_ROW_KIND: &str = "mob_agent";

/// The state word of a mob that is known and not loaded.
const STATE_DORMANT: &str = "dormant";

/// The state word of a mob whose store cannot be loaded.
const STATE_UNAVAILABLE: &str = "unavailable";

/// The word for who created a mob: `host` for a mob the host created,
/// `member` for one a member created, and the creation record's own kind
/// for every other. The list is open: a reader that does not know a word
/// places the mob as one whose creator it does not know.
fn provenance_word(provenance: &meerkat_mob::MobProvenance) -> String {
    use meerkat_mob::MobProvenance;
    match provenance {
        MobProvenance::Host => "host".to_string(),
        MobProvenance::Member { .. } | MobProvenance::OperatorMember { .. } => "member".to_string(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|value| {
                value
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

/// The state word of a mob that is loaded, from whether it runs as its own
/// document answered it.
fn loaded_state_word(document: &Value) -> &'static str {
    match document
        .pointer("/health_overview/live_snapshot/running")
        .and_then(Value::as_bool)
    {
        Some(true) => "running",
        Some(false) => "stopped",
        None => "unknown",
    }
}

/// One entry of the list of mobs. `member_count` is the number of the
/// mob's member rows in the same document, and is left out for a mob that
/// is not loaded: nothing is invented for it.
fn listed_mob(
    mob_id: &meerkat_mob::MobId,
    provenance: &meerkat_mob::MobProvenance,
    default: bool,
    state: &str,
    incarnation: Option<&meerkat_core::service::MobIncarnationId>,
    member_count: Option<usize>,
) -> Value {
    let mut entry = json!({
        "mob_id": mob_id.as_str(),
        "provenance": provenance_word(provenance),
        "state": state,
    });
    if default {
        entry["default"] = json!(true);
    }
    if let Some(incarnation) = incarnation {
        entry["incarnation"] = json!(incarnation.to_string());
    }
    if let Some(member_count) = member_count {
        entry["member_count"] = json!(member_count);
    }
    entry
}

/// The member rows of `document`, when it lists any.
fn member_rows_mut(document: &mut Value) -> Option<&mut Vec<Value>> {
    document.pointer_mut(MEMBER_ROWS)?.as_array_mut()
}

/// Whether `row` is a member's row.
fn is_member_row(row: &Value) -> bool {
    row.get("kind").and_then(Value::as_str) == Some(MEMBER_ROW_KIND)
}

/// The document of the loaded mob `mob`, read for `caller` with its view
/// taken for that mob: the same document a request served for that mob
/// alone would be answered, built from the mob's own values. What the
/// console's router holds for the launch mob alone (its frame log, its
/// event store, its memory panel, its snapshot read model) is not read for
/// another mob: the mob's roster is walked once for this read.
async fn document_of_loaded_mob(
    state: &ConsoleJsonState,
    mob: &ResolvedMob,
    request: &ConsoleRestJsonRequest,
    resolved_auth: Option<&ConsoleAccessRequest>,
    caller: Option<&ConsoleHttpAuthContext>,
) -> Option<Value> {
    let mut of_the_mob = state.clone();
    of_the_mob.runtime = Some(mob.mob_runtime().clone());
    of_the_mob.identity_runtime = mob.identity_runtime();
    of_the_mob.mob_events = Some(mob.mob_events_store());
    of_the_mob.identity_roster = mob.console_identity_roster();
    of_the_mob.workgraph = mob.workgraph_service();
    of_the_mob.topology = Some(mob.topology_runtime_handle());
    of_the_mob.snapshot_read_model = Default::default();
    of_the_mob.console_events = None;
    of_the_mob.console_aggregator = None;
    of_the_mob.memory_panel = None;
    let caller = caller
        .cloned()
        .map(|caller| caller.seeing_mob(mob.mob_id()));
    let (status, axum::Json(document)) = Box::pin(console_document_of(
        &of_the_mob,
        None,
        request,
        resolved_auth,
        caller.as_ref(),
        super::CONSOLE_EXPERIENCE_PATH,
        Some(mob.mob_id()),
    ))
    .await;
    (status == axum::http::StatusCode::OK).then_some(document)
}

/// Add to `document`, the launch mob's experience as `caller` is answered
/// it, what the host serves beside the launch mob to that caller: see the
/// module. `state` is the launch mob's.
pub(super) async fn compose(
    document: &mut Value,
    state: &ConsoleJsonState,
    resolver: Option<&MobResolver>,
    request: &ConsoleRestJsonRequest,
    resolved_auth: Option<&ConsoleAccessRequest>,
    caller: Option<&ConsoleHttpAuthContext>,
) {
    // A console that knows no mob by its id lists no mobs.
    let Some(launch_mob) = console_launch_mob(state, resolver).await else {
        return;
    };
    let view = caller.map(|caller| &caller.audience);

    // The launch mob's member rows name their mob, as every member row of
    // this document does.
    let mut launch_members = 0;
    if let Some(rows) = member_rows_mut(document) {
        for row in rows.iter_mut().filter(|row| is_member_row(row)) {
            row["mob_id"] = json!(launch_mob.as_str());
            launch_members += 1;
        }
    }
    let launch_state = loaded_state_word(document);
    let launch_incarnation = state
        .runtime
        .as_ref()
        .map(|runtime| runtime.handle().incarnation());
    let mut mobs = vec![listed_mob(
        &launch_mob,
        &meerkat_mob::MobProvenance::Host,
        true,
        launch_state,
        launch_incarnation.as_ref(),
        Some(launch_members),
    )];

    // The host's other mobs, as its records know them at this one read:
    // only those the caller may address are listed.
    let hosted: Vec<HostedMobListing> = match resolver {
        Some(resolver) => resolver.hosted_mobs().await,
        None => Vec::new(),
    };
    let addressable =
        MobsSelector::EveryAddressable.listed(view, hosted.iter().map(|mob| &mob.mob_id));
    let mut rows_of_other_mobs = Vec::new();
    for mob in hosted
        .iter()
        .filter(|mob| addressable.contains(&&mob.mob_id))
    {
        // A mob's members are read only while the mob is loaded. A mob that
        // was unloaded since the list was read is listed as not loaded.
        let loaded = match (&mob.phase, resolver) {
            (HostedMobPhase::Loaded, Some(resolver)) => resolver.resolve_if_loaded(&mob.mob_id),
            _ => None,
        };
        let of_the_mob = match &loaded {
            Some(loaded) => {
                document_of_loaded_mob(state, loaded, request, resolved_auth, caller).await
            }
            None => None,
        };
        let entry = match (&mob.phase, of_the_mob) {
            (HostedMobPhase::Loaded, Some(mut of_the_mob)) => {
                let state_word = loaded_state_word(&of_the_mob);
                let mut members = 0;
                if let Some(rows) = member_rows_mut(&mut of_the_mob) {
                    for mut row in std::mem::take(rows).into_iter().filter(is_member_row) {
                        key_member_row(&mut row, &mob.mob_id);
                        withhold_actions_not_served_per_mob(&mut row);
                        rows_of_other_mobs.push(row);
                        members += 1;
                    }
                }
                listed_mob(
                    &mob.mob_id,
                    &mob.provenance,
                    false,
                    state_word,
                    mob.incarnation.as_ref(),
                    Some(members),
                )
            }
            (HostedMobPhase::CannotBeLoaded(_), _) => listed_mob(
                &mob.mob_id,
                &mob.provenance,
                false,
                STATE_UNAVAILABLE,
                mob.incarnation.as_ref(),
                None,
            ),
            (HostedMobPhase::Loaded | HostedMobPhase::NotLoaded, _) => listed_mob(
                &mob.mob_id,
                &mob.provenance,
                false,
                STATE_DORMANT,
                mob.incarnation.as_ref(),
                None,
            ),
        };
        mobs.push(entry);
    }
    if let Some(rows) = member_rows_mut(document) {
        rows.extend(rows_of_other_mobs);
    }
    if let Some(snapshot) = document
        .pointer_mut("/topology/live_snapshot")
        .and_then(Value::as_object_mut)
    {
        snapshot.insert("mobs".to_string(), Value::Array(mobs));
    }
}

/// The lifecycle actions a member row offers, each with the method that
/// acts on it. A row of a mob other than the launch mob offers an action
/// only where the door serves its method for the mob a request names
/// ([`withhold_actions_not_served_per_mob`]).
const MEMBER_ROW_ACTIONS: &[(&str, &str)] = &[
    ("can_retire", "meta/retire"),
    ("can_respawn", "meta/respawn"),
    ("can_reset", "meta/reset"),
    ("can_stop_run", "meta/stop_member_run"),
    ("can_fork_side_chat", "meta/fork_member"),
];

/// Withhold on `row`, a member row of a mob other than the launch mob, each
/// lifecycle action whose method the door does not serve per mob
/// ([`super::CONSOLE_METHODS_SERVED_PER_MOB`]): the row reads it false, so
/// the console offers no action the door would refuse for that mob. A row
/// that does not say an action is left silent on it.
fn withhold_actions_not_served_per_mob(row: &mut Value) {
    let Some(affordances) = row.get_mut("affordances").and_then(Value::as_object_mut) else {
        return;
    };
    for (affordance, method) in MEMBER_ROW_ACTIONS {
        if !super::CONSOLE_METHODS_SERVED_PER_MOB.contains(method)
            && let Some(offered) = affordances.get_mut(*affordance)
        {
            *offered = Value::Bool(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listed mob names who created it, its state and, where they are
    /// known, its incarnation and the number of its member rows. A mob
    /// that is not loaded, or cannot be loaded, carries no count; only the
    /// launch mob is marked `default`.
    #[test]
    fn a_listed_mob_carries_what_is_known_of_it_and_nothing_else() {
        let incarnation = meerkat_core::service::MobIncarnationId::mint();
        let host = meerkat_mob::MobProvenance::Host;
        assert_eq!(
            listed_mob(
                &meerkat_mob::MobId::from("launch"),
                &host,
                true,
                "running",
                Some(&incarnation),
                Some(2),
            ),
            json!({
                "mob_id": "launch",
                "provenance": "host",
                "state": "running",
                "default": true,
                "incarnation": incarnation.to_string(),
                "member_count": 2,
            })
        );
        assert_eq!(
            listed_mob(
                &meerkat_mob::MobId::from("archive"),
                &host,
                false,
                STATE_DORMANT,
                None,
                None,
            ),
            json!({ "mob_id": "archive", "provenance": "host", "state": "dormant" })
        );
        assert_eq!(
            listed_mob(
                &meerkat_mob::MobId::from("broken"),
                &host,
                false,
                STATE_UNAVAILABLE,
                None,
                None,
            ),
            json!({ "mob_id": "broken", "provenance": "host", "state": "unavailable" })
        );
        assert_eq!(provenance_word(&host), "host");
        assert_eq!(
            provenance_word(&meerkat_mob::MobProvenance::Unrecorded),
            "unrecorded"
        );
    }

    /// A loaded mob's state word is whether it runs, as its own document
    /// says; a document that does not say reads `unknown`.
    #[test]
    fn a_loaded_mobs_state_is_whether_it_runs() {
        let document = |running: Value| json!({ "health_overview": { "live_snapshot": { "running": running } } });
        assert_eq!(loaded_state_word(&document(json!(true))), "running");
        assert_eq!(loaded_state_word(&document(json!(false))), "stopped");
        assert_eq!(loaded_state_word(&document(Value::Null)), "unknown");
        assert_eq!(loaded_state_word(&json!({})), "unknown");
    }
}
