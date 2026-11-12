//! The member witness of console timeline frames: every writer that can name
//! its member at the write stores the member's captured attributes with the
//! frame, and a frame stored with none is shown to an administrator only by
//! every console that tells its people apart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_human_admission_tests::BOB;
use super::console_private_side_chat_tests::{
    ROOT, TestResult, alice_claims, bearer, enforced_console, everyone_drives_every_agent,
    fork_on_console,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::*;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrame, ConsoleFrameSource, ConsoleFrameSourceKind,
    ConsoleFrameStatus, ConsoleSendRequest, ConsoleTimelineQuery, MetaConsoleAggregator,
    NewConsoleFrame, timeline_frame_visible_to,
};

const RUNTIME_KEY: &str = "witness";

/// A unified runtime with persistent sessions and one worker `alpha` that
/// has had a turn, and an aggregator over it that reads its console events.
async fn runtime_and_aggregator(
    mob_id: &str,
) -> Result<
    (
        tempfile::TempDir,
        Arc<crate::UnifiedRuntime>,
        MetaConsoleAggregator,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let state_dir = tempfile::tempdir()?;
    let runtime = Arc::new(
        crate::UnifiedRuntime::builder()
            .definition(meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = '{mob_id}'\n[profiles.worker]\nmodel = 'gpt-5.5'\n\
                 runtime_mode = 'turn_driven'\nexternal_addressable = true\n\
                 [profiles.worker.tools]\ncomms = true\n"
            ))?)
            .persistent_state(state_dir.path())
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await?,
    );
    runtime
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from("alpha")))
        .await?;
    runtime
        .mob_handle()
        .member(&AgentIdentity::from("alpha"))
        .await?
        .start_turn(
            meerkat_core::types::ContentInput::Text("The code word is heron.".to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;
    let identity_runtime = Arc::new(crate::identity_first::IdentityRuntime::new(
        crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(crate::identity_first::LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "witness-rows".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        },
    ));
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        RUNTIME_KEY,
        "",
        runtime.mob_runtime().clone(),
        Some(identity_runtime),
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    Ok((state_dir, runtime, aggregator))
}

/// The stored frame under `identity` whose payload holds `canary`, once the
/// aggregator has written it.
async fn stored_frame(
    aggregator: &MetaConsoleAggregator,
    identity: &str,
    canary: &str,
) -> Option<ConsoleFrame> {
    for _ in 0..100 {
        let page = aggregator
            .store()
            .query_frames(ConsoleTimelineQuery {
                identity: Some(identity.to_string()),
                limit: 1_000,
                ..Default::default()
            })
            .await
            .expect("stored frames");
        if let Some(frame) = page.frames.into_iter().find(|frame| {
            frame.kind != "frame_updated" && frame.payload.to_string().contains(canary)
        }) {
            return Some(frame);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// A stored frame under `identity` that holds `canary`, with `provenance`
/// as its witness.
fn frame_with(
    identity: &str,
    canary: &str,
    provenance: Option<crate::ConsoleFrameMemberProvenance>,
) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: format!("witness-row:{identity}:{canary}"),
        timestamp_ms: 2_000,
        runtime_key: RUNTIME_KEY.to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: None,
        kind: "user_input".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "content": canary }),
        source: ConsoleFrameSource {
            member_provenance: provenance,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some(format!("witness-row:{identity}:{canary}")),
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// Every writer that can name its member at the write stores that member's
/// witness with the frame: a member memory event that names its session and
/// one that names none, a lifecycle record, an interaction failure, a live
/// image record, a spawn's kickoff, and a send reserved for a member with no
/// session. A spawn kickoff under a name no member holds is not written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_writer_that_can_name_its_member_stores_its_witness() -> TestResult {
    use crate::memory::events::MemoryTimelineEvent;

    let (_state, runtime, aggregator) = runtime_and_aggregator("witness-writers").await?;
    let session = runtime
        .mob_handle()
        .get_member(&AgentIdentity::from("alpha"))
        .await?
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
        .ok_or("alpha has a session")?;
    let events = runtime.console_events();
    let sink = runtime.memory_event_sink();

    sink.emit(MemoryTimelineEvent::DistillationTimedOut {
        identity: "alpha".to_string(),
        session_key: session.clone(),
        cause: "memory-with-session".to_string(),
    });
    sink.emit(MemoryTimelineEvent::HarvestCompleted {
        realm: "memory-without-session".to_string(),
        identity: "alpha".to_string(),
        promoted: 0,
        tombstoned: 0,
    });
    events
        .record_lifecycle(
            "alpha",
            "identity_reset",
            json!({ "note": "lifecycle-record" }),
        )
        .await;
    events
        .record_interaction_failure(
            "alpha",
            "interaction-1",
            json!({ "error": "failure-record" }),
        )
        .await;
    events
        .append_envelope(crate::console_contracts::ConsoleIdentityEventEnvelope {
            event_id: "witness-image-1".to_string(),
            interaction_id: None,
            identity: "alpha".to_string(),
            event_type: "assistant_image".to_string(),
            timestamp_ms: 3_000,
            data: json!({ "image_id": "image-record", "media_type": "image/png" }),
        })
        .await;
    let reserved = aggregator
        .reserve_identity_first_interaction(
            ConsoleSendRequest {
                identity: "alpha".to_string(),
                content: json!("sessionless-send"),
                origin: "console".to_string(),
                idempotency_key: "witness-send-1".to_string(),
                handling_mode: Some("queue".to_string()),
                origin_kind: None,
                skill_refs: Vec::new(),
            },
            None,
        )
        .await;
    assert!(reserved.is_ok(), "the send is reserved with no session");

    // A kickoff aimed at a seated member by a spawner its lineage does not
    // name, then two from the spawn sink: one under a name no member holds,
    // one for a member a spawn seated.
    events
        .append_envelope(crate::console_contracts::ConsoleIdentityEventEnvelope {
            event_id: "witness-kickoff-foreign".to_string(),
            interaction_id: None,
            identity: "alpha".to_string(),
            event_type: "user_input".to_string(),
            timestamp_ms: 3_500,
            data: json!({
                "content": [{ "type": "text", "text": "kickoff-foreign" }],
                "source_event_type": "spawn_initial_message",
                "parent_identity": "someone-else",
                "member_id": "alpha",
            }),
        })
        .await;
    runtime
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from("beta")))
        .await?;
    let spawn_sink = runtime
        .mob_runtime()
        .console_spawn_sink()
        .ok_or("the console spawn sink")?;
    for (member, text) in [("ghost", "kickoff-unseated"), ("beta", "kickoff-record")] {
        spawn_sink
            .project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
                mob_id: Some("witness-writers".to_string()),
                member_id: member.to_string(),
                identity: member.to_string(),
                initial_message: Some(json!(text)),
                labels: BTreeMap::new(),
                spawned_by: Some("alpha".to_string()),
                spawned_by_session: Some(session.clone()),
                via_tool: "mob_spawn_member".to_string(),
            })
            .await;
    }

    for (identity, canary) in [
        ("alpha", "memory-with-session"),
        ("alpha", "memory-without-session"),
        ("alpha", "lifecycle-record"),
        ("alpha", "failure-record"),
        ("alpha", "image-record"),
        ("alpha", "sessionless-send"),
        ("beta", "kickoff-record"),
    ] {
        let frame = stored_frame(&aggregator, identity, canary)
            .await
            .ok_or_else(|| format!("the {canary} frame is stored"))?;
        assert_eq!(
            frame
                .source
                .member_provenance
                .as_ref()
                .map(|witness| witness.identity.identity.as_str()),
            Some(identity),
            "the {canary} frame ({}) carries its member's witness",
            frame.kind
        );
        if canary == "memory-with-session" {
            assert_eq!(
                frame.session_id.as_deref(),
                Some(session.as_str()),
                "a memory event that names its session names it on its frame"
            );
        }
    }
    // A kickoff that names a spawner is written only for a member whose
    // lineage names that spawner: the one aimed at `alpha`, which the host
    // seated, is not stored.
    let of_alpha = aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            identity: Some("alpha".to_string()),
            limit: 1_000,
            ..Default::default()
        })
        .await?;
    assert!(
        !of_alpha
            .frames
            .iter()
            .any(|frame| frame.payload.to_string().contains("kickoff-foreign")),
        "no kickoff is written for a member another spawn did not make"
    );
    // The kickoffs are projected in order: the one under the unseated name
    // was decided before the one that is stored.
    let unseated = aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            identity: Some("ghost".to_string()),
            limit: 1_000,
            ..Default::default()
        })
        .await?;
    assert!(
        unseated.frames.is_empty(),
        "no kickoff is written under a name no member holds: {:?}",
        unseated
            .frames
            .iter()
            .map(|frame| frame.kind.as_str())
            .collect::<Vec<_>>()
    );
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A frame with no witness is decided without the owner cache: right after a
/// restart, before the record of who each member is private to has read the
/// mob's event log, the cache does not know a private side chat that ended
/// before the restart, and the frame is still shown to no person but an
/// administrator. It stays so once the record is read.
#[tokio::test]
async fn a_frame_with_no_witness_is_refused_before_the_owner_record_is_read() -> TestResult {
    const MOB: &str = "witness-before-seed";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(everyone_drives_every_agent()),
    ));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    drop(app);
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().retire(side_chat.clone()).await?;
    for _ in 0..100 {
        if runtime.handle().get_member(&side_chat).await?.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(90))
        .await;
    drop(runtime);

    // The restart. Nothing is awaited between the bootstrap's return and
    // the decision below, so the background read has not run.
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    assert!(
        !runtime.private_owners().landed(),
        "the record of who each member is private to has not been read yet"
    );
    let known = runtime.private_owners().memory_members();
    assert!(
        !known.contains_key("alpha-side"),
        "before it is read, the record holds nothing of the ended side chat: {known:?}"
    );
    // What a request's priming records for ended members at this moment.
    let controller = everyone_drives_every_agent();
    record_live_and_registered_attributes(
        &controller,
        Vec::new(),
        &BTreeMap::new(),
        &BTreeSet::new(),
        &known,
        "",
    );
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let root = controller.view_for_caller(Some(ROOT), None);
    assert!(!bob.knows_agent("alpha-side"), "the cache does not know it");
    let frame = MetaConsoleAggregator::in_memory()
        .store()
        .append_if_absent(frame_with("alpha-side", "ended-side-chat-record", None))
        .await?
        .frame;
    assert!(
        !timeline_frame_visible_to(&bob, &frame),
        "a frame with no witness is refused to another person while the cache is empty"
    );
    assert!(
        timeline_frame_visible_to(&root, &frame),
        "an administrator reads it"
    );

    // Once the record is read it names the side chat's owner; the decision
    // on the frame is the same.
    let handle = runtime.handle();
    assert!(runtime.private_owners().seeded(&handle).await.landed());
    let known = runtime.private_owners().memory_members();
    assert!(
        known.get("alpha-side").is_some_and(Option::is_some),
        "the record read names the ended side chat's owner: {known:?}"
    );
    record_live_and_registered_attributes(
        &controller,
        Vec::new(),
        &BTreeMap::new(),
        &BTreeSet::new(),
        &known,
        "",
    );
    assert!(!timeline_frame_visible_to(&bob, &frame));
    assert!(timeline_frame_visible_to(&root, &frame));
    Ok(())
}

/// The thread read decides a kept reply as the timeline decides its frame:
/// a reply kept for a request with no witness is served to an administrator
/// only, one kept for a request of a shared member to everyone who may view
/// that member. With no runtime registered, a frame whose captured member sat
/// in a nested mob is not served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_thread_read_and_a_console_with_no_runtime_decide_by_the_witness() -> TestResult {
    let (_state, runtime, aggregator) = runtime_and_aggregator("witness-thread").await?;
    let session = runtime
        .mob_handle()
        .get_member(&AgentIdentity::from("alpha"))
        .await?
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
        .ok_or("alpha has a session")?;
    let witness = aggregator
        .capture_member_provenance_for_test(RUNTIME_KEY, "alpha", &session)
        .await
        .ok_or("the owner captures alpha")?;
    let controller = everyone_drives_every_agent();
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let root = controller.view_for_caller(Some(ROOT), None);

    for (view, reader, reads_unwitnessed) in [
        (&bob, "another person", false),
        (&root, "an administrator", true),
    ] {
        let mut serving = aggregator.kept_reply_serving(view).await;
        assert_eq!(
            serving
                .serve("kept-1", frame_with("ghost", "kept-without-witness", None))
                .await
                .is_some(),
            reads_unwitnessed,
            "a reply kept for a request with no witness, read by {reader}"
        );
        assert!(
            serving
                .serve(
                    "kept-2",
                    frame_with("alpha", "kept-with-witness", Some(witness.clone()))
                )
                .await
                .is_some(),
            "a reply kept for a shared member's request is served to {reader}"
        );
    }

    // No runtime registered: the stored frames of a member of the runtime's
    // own mob stay queryable, those of a member of a nested mob do not.
    let detached = MetaConsoleAggregator::in_memory();
    let mut nested = witness.clone();
    nested.source_mob_id = format!("{}-child", witness.primary_mob_id);
    detached
        .store()
        .append_if_absent(frame_with("alpha", "own-mob-member", Some(witness)))
        .await?;
    detached
        .store()
        .append_if_absent(frame_with("alpha", "nested-mob-member", Some(nested)))
        .await?;
    let served = detached
        .query_timeline(ConsoleTimelineQuery {
            identity: Some("alpha".to_string()),
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames
        .iter()
        .map(|frame| frame.payload["content"].clone())
        .collect::<Vec<Value>>();
    assert_eq!(
        served,
        vec![json!("own-mob-member")],
        "with no runtime registered only the own mob's member is served"
    );
    let _ = runtime.shutdown().await;
    Ok(())
}
