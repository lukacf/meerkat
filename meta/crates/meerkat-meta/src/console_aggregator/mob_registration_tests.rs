//! The registration of a mob's console events, as what writes for it is
//! held to it.
//!
//! A registration answers a token. An event handed with a token is written
//! for the registration that answered it, or not at all; the end of a
//! registration by its token leaves another registration of the mob as it
//! is; a mob of another incarnation is not registered over frames that were
//! not removed; and a frame that names a mob and comes with no registration
//! is refused at the one write point.
//!
//! The rows call the aggregator's own functions over the in-memory log. The
//! gate's own rows are in `mob_writers`.

#![allow(clippy::expect_used)]

use super::*;
use crate::console_contracts::ConsoleIdentityEventEnvelope;
use crate::member_key::DefaultMob;

/// A console over a new in-memory log that knows its host's default mob.
fn console() -> MetaConsoleAggregator {
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.tell_default_mob(DefaultMob::new(meerkat_mob::MobId::from(
        "writer-rows-launch",
    )));
    aggregator
}

fn second_mob() -> meerkat_mob::MobId {
    meerkat_mob::MobId::from(super::tests::unique_console_mob_id("writer-rows-second"))
}

/// Register the console events of `runtime`'s mob, under `mob`.
fn register(
    aggregator: &MetaConsoleAggregator,
    runtime: &UnifiedRuntime,
    mob: &meerkat_mob::MobId,
) -> ConsoleLogResult<uuid::Uuid> {
    aggregator.register_mob_console_events(
        mob,
        runtime.mob_runtime().clone(),
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    )
}

/// A console event of the mob's `alice` that carries `text`.
fn event(text: &str) -> ConsoleIdentityEventEnvelope {
    ConsoleIdentityEventEnvelope {
        event_id: format!("writer-rows-{text}"),
        interaction_id: None,
        identity: "alice".to_string(),
        event_type: "identity_respawned".to_string(),
        timestamp_ms: 1,
        data: json!({ "text": text }),
    }
}

/// The texts of the frames the log holds under `mob`, sorted.
async fn texts(aggregator: &MetaConsoleAggregator, mob: &meerkat_mob::MobId) -> Vec<String> {
    let mut texts: Vec<String> = aggregator
        .inner
        .store
        .query_windowed_frames_of_mob(ConsoleTimelineWindowQuery {
            mob_id: Some(mob.clone()),
            ..Default::default()
        })
        .await
        .expect("the read of one mob")
        .frames
        .iter()
        .filter_map(|frame| frame.payload.get("text").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    texts.sort();
    texts
}

/// An event handed with the token of a registration that ended is written
/// for no registration, also while the mob is registered again: the later
/// registration under the key is not written for with the earlier one's
/// token. The same event with the standing registration's token is stored.
#[tokio::test]
async fn an_event_handed_with_an_ended_registrations_token_is_stored_for_no_registration() {
    let second = second_mob();
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let aggregator = console();
    let earlier = register(&aggregator, &runtime, &second).expect("the first registration");
    let later = register(&aggregator, &runtime, &second).expect("the same mob again");
    assert_ne!(
        earlier, later,
        "each registration answers a token of its own"
    );

    aggregator
        .project_mob_console_events(&second, earlier, vec![event("with-the-ended-token")])
        .await
        .expect("an ended registration's event is dropped without an error");
    assert_eq!(
        texts(&aggregator, &second).await,
        Vec::<String>::new(),
        "nothing is written for the registration that stands with the ended one's token"
    );

    aggregator
        .project_mob_console_events(&second, later, vec![event("with-the-standing-token")])
        .await
        .expect("the standing registration's event is stored");
    assert_eq!(
        texts(&aggregator, &second).await,
        ["with-the-standing-token"]
    );
    runtime.shutdown().await;
}

/// The end of a registration by its token leaves a later registration of
/// the mob as it is: an end that comes late ends the registration it names
/// and no other.
#[tokio::test]
async fn the_end_of_a_registration_by_its_token_leaves_a_later_one_standing() {
    let second = second_mob();
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let aggregator = console();
    let earlier = register(&aggregator, &runtime, &second).expect("the first registration");
    let later = register(&aggregator, &runtime, &second).expect("the same mob again");

    aggregator.end_mob_console_events(&second, earlier);
    assert_eq!(
        mob_registered_under(&aggregator.inner, &format!("mob/{}", second.as_str())),
        Some(second.clone()),
        "the later registration is still under the key"
    );
    aggregator
        .project_mob_console_events(&second, later, vec![event("after-the-late-end")])
        .await
        .expect("the standing registration's event is stored");
    assert_eq!(texts(&aggregator, &second).await, ["after-the-late-end"]);

    aggregator.end_mob_console_events(&second, later);
    assert_eq!(
        mob_registered_under(&aggregator.inner, &format!("mob/{}", second.as_str())),
        None,
        "its own end ends it"
    );
    aggregator
        .project_mob_console_events(&second, later, vec![event("after-its-own-end")])
        .await
        .expect("an ended registration's event is dropped without an error");
    assert_eq!(texts(&aggregator, &second).await, ["after-the-late-end"]);
    runtime.shutdown().await;
}

/// A mob is not registered under an id while the registry of gates holds,
/// under that id, a gate of ANOTHER incarnation: the frames under the id
/// are that incarnation's and were not removed, whether its registration
/// stands or ended. The refusal is read by its type, nothing of the refused
/// registration is kept, and the earlier registration is left as it is.
/// Once the host says the frames under the id were removed, the
/// registration is made.
///
/// The earlier incarnation's gate is opened in the registry itself, as a
/// registration with a projection the host replaced leaves it there (the
/// registry is the host's and outlives a projection): one process cannot
/// run two mobs of one id. That is weaker than two running mobs: the
/// earlier incarnation is a gate and writes no frame of its own here.
#[tokio::test]
async fn a_mob_of_another_incarnation_is_not_registered_over_frames_that_were_not_removed() {
    let second = second_mob();
    let key = format!("mob/{}", second.as_str());
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let aggregator = console();
    let earlier = uuid::Uuid::new_v4();
    let earlier_incarnation = meerkat_core::service::MobIncarnationId::mint();
    assert_ne!(
        earlier_incarnation,
        runtime.mob_runtime().handle().incarnation()
    );
    mob_writers_of(&aggregator.inner)
        .open(&key, earlier, earlier_incarnation)
        .expect("the earlier incarnation's gate");

    let refused = register(&aggregator, &runtime, &second)
        .expect_err("refused while the earlier incarnation's registration stands");
    assert_eq!(
        refused
            .downcast_ref::<mob_writers::AnotherIncarnationStands>()
            .map(|refusal| refusal.runtime_key.as_str()),
        Some(key.as_str()),
        "the refusal is read by its type and names the key: {refused:?}"
    );
    assert_eq!(
        mob_registered_under(&aggregator.inner, &key),
        None,
        "a refused registration is not kept"
    );
    assert!(
        mob_writers_of(&aggregator.inner)
            .permit(&key, earlier)
            .is_some(),
        "the earlier registration is left as it is: it still writes"
    );

    mob_writers_of(&aggregator.inner).end(&key, earlier);
    assert!(
        register(&aggregator, &runtime, &second).is_err(),
        "refused after the earlier registration's end too: its frames are still under the id"
    );
    assert_eq!(mob_registered_under(&aggregator.inner, &key), None);

    // The host removed the frames under the id and says so.
    mob_writers_of(&aggregator.inner).released(&key);
    let later = register(&aggregator, &runtime, &second).expect("made once they were removed");
    aggregator
        .project_mob_console_events(&second, later, vec![event("of-the-later-incarnation")])
        .await
        .expect("the later incarnation's event is stored");
    assert_eq!(
        texts(&aggregator, &second).await,
        ["of-the-later-incarnation"]
    );
    runtime.shutdown().await;
}

/// A frame that names a mob and comes with no registration of that mob's
/// console events is refused at the one write point, and nothing is stored.
/// The product's one writer for no registration is the default mob's gap
/// marker, which names no mob; the row appends a frame that names a mob
/// through the test seam that writes as that writer does, and holds the
/// refusal.
#[tokio::test]
async fn a_frame_that_names_a_mob_and_no_registration_is_refused() {
    let second = second_mob();
    let runtime = super::tests::build_empty_runtime(second.as_str()).await;
    let aggregator = console();
    register(&aggregator, &runtime, &second).expect("the mob's registration");
    let key = format!("mob/{}", second.as_str());
    let frame = NewConsoleFrame {
        mob_id: Some(second.clone()),
        id: None,
        dedupe_key: "writer-rows-frame-of-no-registration".to_string(),
        timestamp_ms: 1,
        runtime_key: key,
        identity: "alice".to_string(),
        conversation_id: Some("alice".to_string()),
        session_id: None,
        kind: "note".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": "of-no-registration" }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Synthetic,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    let refused = append_and_emit_with_policy(
        &aggregator.inner,
        frame,
        Arc::new(AllowAllConsoleVisibilityPolicy),
    )
    .await
    .expect_err("a frame that names a mob is not written for no registration");
    assert!(
        refused.to_string().contains("comes with no registration"),
        "the refusal says why: {refused}"
    );
    assert_eq!(texts(&aggregator, &second).await, Vec::<String>::new());
    runtime.shutdown().await;
}

/// The gap marker of a runtime's failed lag recovery is appended whatever
/// the registry holds: it is the default mob's, names no mob, and needs no
/// registration. A console no runtime is registered with recovers a
/// store's events from a cursor the store does not hold: the marker stands
/// in the log under the runtime's key.
///
/// The row holds a console with no registration at all. It does not hold a
/// runtime that is registered for the default mob (its marker is written
/// the same way), nor a key that holds a mob's registration at that moment
/// (a mob's own recovery writes its marker for its registration).
#[tokio::test]
async fn the_default_mobs_gap_marker_needs_no_registration() {
    const RUNTIME: &str = "writer-rows-runtime";
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .inner
        .store
        .record_source_watermark(
            RUNTIME,
            ConsoleFrameSourceKind::ConsoleEvent,
            "a-cursor-the-store-does-not-hold",
        )
        .await
        .expect("the watermark");
    let events = ConsoleEventStore::new();
    recover_lagged_source_events(aggregator.inner.clone(), RUNTIME, &events)
        .await
        .expect("the recovery");
    let markers: Vec<String> = aggregator
        .inner
        .store
        .query_frames(ConsoleTimelineQuery::default())
        .await
        .expect("the log")
        .frames
        .iter()
        .filter(|frame| frame.kind == "replay_unavailable")
        .map(|frame| frame.runtime_key.clone())
        .collect();
    assert_eq!(markers, [RUNTIME], "the gap is marked");
}
