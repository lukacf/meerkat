//! The member witness of console timeline frames. A frame is witnessed by the
//! session it names or by the console send it names, never by whoever holds
//! its member's name when it is written or projected; and a frame stored
//! with no witness is shown to an administrator only by every console that
//! tells its people apart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_human_admission_tests::BOB;
use super::console_private_side_chat_tests::{
    ALICE, PRIVATE_TO, ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console,
    everyone_drives_every_agent, fork_on_console, root_claims,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::console_token_lapse_tests::console_rpc_as;
use super::*;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrame, ConsoleFrameSource, ConsoleFrameSourceKind,
    ConsoleFrameStatus, ConsoleSendRequest, ConsoleTimelineQuery, MetaConsoleAggregator,
    NewConsoleFrame, timeline_frame_visible_to,
};

const RUNTIME_KEY: &str = "witness";

/// The key of the person the private members of these rows are private to.
const ALICE_KEY: &str = "alice-key";

type RigResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A unified runtime with persistent sessions and one worker `alpha` that
/// has had a turn, and an aggregator over it that reads its console events.
async fn runtime_and_aggregator(
    mob_id: &str,
) -> RigResult<(
    tempfile::TempDir,
    Arc<crate::UnifiedRuntime>,
    MetaConsoleAggregator,
)> {
    let (state_dir, runtime) = witness_runtime(mob_id).await?;
    let aggregator = MetaConsoleAggregator::in_memory();
    register_runtime(&runtime, &aggregator)?;
    Ok((state_dir, runtime, aggregator))
}

/// Register `runtime` with `aggregator` now: the aggregator projects every
/// console event the runtime retains, then the live ones. A second
/// registration projects the retained events again.
fn register_runtime(
    runtime: &Arc<crate::UnifiedRuntime>,
    aggregator: &MetaConsoleAggregator,
) -> RigResult<()> {
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
    aggregator.register_runtime_handles_with_policy(
        RUNTIME_KEY,
        "",
        runtime.mob_runtime().clone(),
        Some(identity_runtime),
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    Ok(())
}

/// The labels of a member that is private to Alice.
fn private_to_alice() -> BTreeMap<String, String> {
    BTreeMap::from([(PRIVATE_TO.to_string(), ALICE_KEY.to_string())])
}

/// The people who read: Alice (the owner of what is private to
/// [`ALICE_KEY`]), Bob (another signed-in person) and the administrator, on a
/// console where every person may drive every agent.
struct Readers {
    alice: crate::access::AccessView,
    bob: crate::access::AccessView,
    root: crate::access::AccessView,
}

fn readers() -> Readers {
    let controller = everyone_drives_every_agent();
    Readers {
        alice: controller.view_for_caller(Some(ALICE), Some(ALICE_KEY)),
        bob: controller.view_for_caller(Some(BOB), Some("bob-key")),
        root: controller.view_for_caller(Some(ROOT), None),
    }
}

/// The session `member` runs on.
async fn session_of(runtime: &crate::UnifiedRuntime, member: &str) -> RigResult<String> {
    runtime
        .mob_handle()
        .get_member(&AgentIdentity::from(member))
        .await?
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
        .ok_or_else(|| format!("{member} has a session").into())
}

/// Retire `member` and wait until it has left the roster.
async fn leaves(runtime: &crate::UnifiedRuntime, member: &str) -> RigResult<()> {
    let identity = AgentIdentity::from(member);
    let handle = runtime.mob_runtime().handle();
    handle.retire(identity.clone()).await?;
    for _ in 0..100 {
        if handle.get_member(&identity).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("{member} is still on the roster").into())
}

/// Whether the runtime retains a console event that holds `canary`.
async fn retains_event(runtime: &crate::UnifiedRuntime, canary: &str) -> bool {
    for _ in 0..100 {
        if runtime
            .console_events()
            .replay_all(None)
            .await
            .is_ok_and(|events| {
                events
                    .iter()
                    .any(|event| event.data.to_string().contains(canary))
            })
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Every frame the aggregator has stored under `identity`.
async fn frames_of(aggregator: &MetaConsoleAggregator, identity: &str) -> Vec<ConsoleFrame> {
    aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            identity: Some(identity.to_string()),
            limit: 1_000,
            ..Default::default()
        })
        .await
        .expect("stored frames")
        .frames
}

/// The stored frame of `kind` under `identity` whose payload holds `canary`,
/// once the aggregator has written it.
async fn stored_frame_of_kind(
    aggregator: &MetaConsoleAggregator,
    identity: &str,
    kind: &str,
    canary: &str,
) -> Option<ConsoleFrame> {
    for _ in 0..100 {
        if let Some(frame) = frames_of(aggregator, identity)
            .await
            .into_iter()
            .find(|frame| frame.kind == kind && frame.payload.to_string().contains(canary))
        {
            return Some(frame);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// A console send of `content` to `identity`, as the send route reserves it
/// for a target whose status names no session.
fn send_request(identity: &str, content: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: identity.to_string(),
        content: json!(content),
        origin: "console".to_string(),
        idempotency_key: format!("witness-{content}"),
        handling_mode: Some("queue".to_string()),
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

/// A unified runtime with persistent sessions and one worker `alpha` that
/// has had a turn. No console reads it yet.
async fn witness_runtime(
    mob_id: &str,
) -> RigResult<(tempfile::TempDir, Arc<crate::UnifiedRuntime>)> {
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
    Ok((state_dir, runtime))
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

/// Run one turn of `member` on `text`.
async fn run_turn(runtime: &crate::UnifiedRuntime, member: &str, text: &str) -> RigResult<()> {
    runtime
        .mob_handle()
        .member(&AgentIdentity::from(member))
        .await?
        .start_turn(
            meerkat_core::types::ContentInput::Text(text.to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;
    Ok(())
}

/// A console event of `event_type` under `identity` whose payload is `data`.
fn console_event(
    event_id: &str,
    identity: &str,
    event_type: &str,
    data: Value,
) -> crate::console_contracts::ConsoleIdentityEventEnvelope {
    crate::console_contracts::ConsoleIdentityEventEnvelope {
        event_id: event_id.to_string(),
        interaction_id: None,
        identity: identity.to_string(),
        event_type: event_type.to_string(),
        timestamp_ms: 3_000,
        data,
    }
}

/// What each writer of a member frame stores as the frame's witness. A
/// record that names its member's session (a member memory event, a live
/// image record, a spawn's kickoff) carries that session's member, and a
/// send reserved for a member with no session carries the member it was
/// sent to. A record that names neither a session nor a console send (a
/// lifecycle record, a memory harvest, an image record with no session, a
/// failure of an interaction this console holds no send for) carries none.
/// A spawn's kickoff is written only for the session the spawn's own outcome
/// names, and only when that session's member can be read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_writer_stores_the_witness_its_record_names() -> TestResult {
    use crate::memory::events::MemoryTimelineEvent;

    let (_state, runtime, aggregator) = runtime_and_aggregator("witness-writers").await?;
    let session = session_of(&runtime, "alpha").await?;
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
        .append_envelope(console_event(
            "witness-image-1",
            "alpha",
            "assistant_image",
            json!({ "image_id": "image-without-session", "media_type": "image/png" }),
        ))
        .await;
    events
        .append_envelope(console_event(
            "witness-image-2",
            "alpha",
            "assistant_image",
            json!({
                "image_id": "image-with-session",
                "media_type": "image/png",
                "session_id": session,
            }),
        ))
        .await;
    let reserved = aggregator
        .reserve_identity_first_interaction(send_request("alpha", "sessionless-send"), None)
        .await;
    assert!(reserved.is_ok(), "the send is reserved with no session");

    // Four spawn outcomes, in this order: one that names no session, one
    // that names a session of another member, one that names a member and a
    // session nobody holds, and one that names the member it made and that
    // member's session. Only the last leaves a kickoff.
    runtime
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from("beta")))
        .await?;
    run_turn(&runtime, "beta", "The code word is egret.").await?;
    let beta_session = session_of(&runtime, "beta").await?;
    let spawn_sink = runtime
        .mob_runtime()
        .console_spawn_sink()
        .ok_or("the console spawn sink")?;
    let spawner = "witness-writers/worker/alpha";
    let outcomes = [
        (
            "mob_spawn_member",
            json!({ "member_id": "beta", "profile": "worker", "initial_message": "kickoff-no-session" }),
            json!({ "agent_identity": "beta", "member_ref": "opaque" }),
        ),
        (
            "delegate",
            json!({ "task": "kickoff-other-session" }),
            json!({ "agent_identity": "beta", "member_ref": "opaque", "session_id": session }),
        ),
        (
            "fork_off",
            json!({ "member_id": "ghost", "task": "kickoff-unseated" }),
            json!({
                "agent_identity": "ghost",
                "member_ref": "opaque",
                "source_member_id": "alpha",
                "fork_session_id": "00000000-0000-4000-8000-00000000dead",
            }),
        ),
        (
            "delegate",
            json!({ "task": "kickoff-record" }),
            json!({ "agent_identity": "beta", "member_ref": "opaque", "session_id": beta_session }),
        ),
    ];
    for (tool, args, outcome) in &outcomes {
        for seed in crate::console_spawn::console_spawn_seeds(
            tool,
            args,
            &outcome.to_string(),
            Some(spawner),
        ) {
            spawn_sink.project_spawned_member(&seed).await;
        }
    }

    for (identity, canary, witnessed) in [
        ("alpha", "memory-with-session", true),
        ("alpha", "memory-without-session", false),
        ("alpha", "lifecycle-record", false),
        ("alpha", "failure-record", false),
        ("alpha", "image-without-session", false),
        ("alpha", "image-with-session", true),
        ("alpha", "sessionless-send", true),
        ("beta", "kickoff-record", true),
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
            witnessed.then_some(identity),
            "the witness of the {canary} frame ({})",
            frame.kind
        );
        if canary == "memory-with-session" {
            assert_eq!(
                frame.session_id.as_deref(),
                Some(session.as_str()),
                "a memory event that names its session names it on its frame"
            );
        }
        if canary == "kickoff-record" {
            assert_eq!(
                frame.session_id.as_deref(),
                Some(beta_session.as_str()),
                "a kickoff names the session its spawn's outcome names"
            );
        }
    }
    // The kickoffs are projected in order: the three before the stored one
    // were decided before it.
    for (identity, canary) in [
        ("beta", "kickoff-no-session"),
        ("beta", "kickoff-other-session"),
        ("ghost", "kickoff-unseated"),
    ] {
        assert!(
            !frames_of(&aggregator, identity)
                .await
                .iter()
                .any(|frame| frame.payload.to_string().contains(canary)),
            "no {canary} frame is written under {identity}"
        );
    }
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A record is never witnessed by a later holder of its name. Records about
/// a member that is private to one person are retained by the runtime; the
/// member leaves and a shared member takes its name; then a console starts
/// to read the runtime and projects the retained records. A record that
/// names no session, and one that names the session of the member that left,
/// are stored with no witness: no person but an administrator reads them. A
/// failure record whose send this console holds no frame for is one of
/// them. A kickoff that names no session is not written at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_is_never_witnessed_by_a_later_holder_of_its_name() -> TestResult {
    use crate::memory::events::MemoryTimelineEvent;

    let (_state, runtime) = witness_runtime("witness-reuse").await?;
    // Seated by the host itself, as Meta seats a member private to a person:
    // a caller's spawn may not name the private owner.
    Box::pin(runtime.mob_runtime().handle().spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from("side")).with_labels(private_to_alice()),
    ))
    .await?;
    run_turn(&runtime, "side", "The code word is plover.").await?;
    let old_session = session_of(&runtime, "side").await?;
    let events = runtime.console_events();
    let sink = runtime.memory_event_sink();

    // A pending send that a lifecycle record closes: its failure record
    // holds the send's content.
    events
        .reserve_interaction_value(
            "side",
            None,
            "pending-send-1",
            "console",
            json!("pending-send-content"),
        )
        .await?;
    events
        .record_lifecycle(
            "side",
            "identity_reset",
            json!({ "note": "lifecycle-record" }),
        )
        .await;
    events
        .append_envelope(console_event(
            "reuse-kickoff-1",
            "side",
            "user_input",
            json!({
                "content": [{ "type": "text", "text": "kickoff-text" }],
                "source_event_type": "spawn_initial_message",
                "member_id": "side",
            }),
        ))
        .await;
    events
        .append_envelope(console_event(
            "reuse-image-1",
            "side",
            "assistant_image",
            json!({ "image_id": "image-record", "media_type": "image/png" }),
        ))
        .await;
    sink.emit(MemoryTimelineEvent::HarvestCompleted {
        realm: "harvest-record".to_string(),
        identity: "side".to_string(),
        promoted: 0,
        tombstoned: 0,
    });
    sink.emit(MemoryTimelineEvent::DistillationTimedOut {
        identity: "side".to_string(),
        session_key: old_session.clone(),
        cause: "stamped-record".to_string(),
    });
    assert!(retains_event(&runtime, "harvest-record").await);
    assert!(retains_event(&runtime, "stamped-record").await);

    // The private member leaves; a shared member takes its name.
    leaves(&runtime, "side").await?;
    runtime
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from("side")))
        .await?;
    run_turn(&runtime, "side", "The code word is avocet.").await?;
    assert_ne!(
        session_of(&runtime, "side").await?,
        old_session,
        "the member that holds the name now is another member"
    );

    // A console starts to read now: it projects the retained records.
    let aggregator = MetaConsoleAggregator::in_memory();
    register_runtime(&runtime, &aggregator)?;

    let readers = readers();
    for canary in [
        "pending-send-content",
        "lifecycle-record",
        "image-record",
        "harvest-record",
        "stamped-record",
    ] {
        let frame = stored_frame(&aggregator, "side", canary)
            .await
            .ok_or_else(|| format!("the {canary} frame is stored"))?;
        assert!(
            frame.source.member_provenance.is_none(),
            "the {canary} frame ({}) of the member that left carries no witness, and not the \
             witness of the member that holds its name now",
            frame.kind
        );
        assert!(
            !timeline_frame_visible_to(&readers.bob, &frame),
            "another person is not served the {canary} frame"
        );
        assert!(
            !timeline_frame_visible_to(&readers.alice, &frame),
            "no person is served the {canary} frame"
        );
        assert!(
            timeline_frame_visible_to(&readers.root, &frame),
            "an administrator is served the {canary} frame"
        );
    }
    // The kickoff was projected before the image record that is stored.
    assert!(
        !frames_of(&aggregator, "side")
            .await
            .iter()
            .any(|frame| frame.payload.to_string().contains("kickoff-text")),
        "a kickoff that names no session is not written"
    );
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A send to a member that has no session yet is witnessed by the member it
/// was sent to, captured at the send. The sender and every person who may
/// see a shared member are served her send to it; a send to a member that is
/// private to her is served to her and to an administrator only. A record
/// that names that send (the failure record of a pending send a lifecycle
/// record closed) carries the send's own witness, also when it is projected
/// after the member left and a shared member took its name, and when the
/// retained records are projected again: one frame for the send, one for its
/// failure, both still private. A record that names no send and no session
/// carries no witness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_to_a_member_with_no_session_carries_the_member_it_was_sent_to() -> TestResult {
    let (_state, runtime, aggregator) = runtime_and_aggregator("witness-send").await?;
    let mut shared = SpawnMemberSpec::new("worker", AgentIdentity::from("resting"));
    shared.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let mut own = SpawnMemberSpec::new("worker", AgentIdentity::from("resting-own"))
        .with_labels(private_to_alice());
    own.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    let seated = Box::pin(
        runtime
            .mob_runtime()
            .handle()
            .seat_dormant_members(vec![shared, own]),
    )
    .await?;
    assert!(
        seated.len() == 2
            && seated
                .iter()
                .all(|outcome| matches!(outcome, meerkat_mob::DormantSeatOutcome::Seated { .. })),
        "both members are seated and not woken: {seated:?}"
    );
    for (identity, content) in [
        ("resting", "send-to-shared"),
        ("resting-own", "send-to-own"),
    ] {
        let reserved = aggregator
            .reserve_identity_first_interaction(send_request(identity, content), None)
            .await;
        assert!(
            reserved.is_ok(),
            "the send to {identity} is reserved with no session"
        );
    }

    let readers = readers();
    let to_shared = stored_frame(&aggregator, "resting", "send-to-shared")
        .await
        .ok_or("the send to the shared member is stored")?;
    for (reader, view) in [
        ("its sender", &readers.alice),
        ("another person", &readers.bob),
        ("an administrator", &readers.root),
    ] {
        assert!(
            timeline_frame_visible_to(view, &to_shared),
            "a send to a shared member with no session is served to {reader}"
        );
    }
    let to_own = stored_frame(&aggregator, "resting-own", "send-to-own")
        .await
        .ok_or("the send to the private member is stored")?;
    let private_to_its_owner = |frame: &ConsoleFrame, what: &str| {
        assert!(
            timeline_frame_visible_to(&readers.alice, frame),
            "{what} is served to the person the member is private to"
        );
        assert!(
            !timeline_frame_visible_to(&readers.bob, frame),
            "{what} is not served to another person"
        );
        assert!(
            timeline_frame_visible_to(&readers.root, frame),
            "{what} is served to an administrator"
        );
    };
    private_to_its_owner(&to_own, "a send to a private member with no session");

    // The send is pending when the private member leaves and a shared
    // member takes its name; then a lifecycle record closes the send.
    let interaction = to_own
        .interaction_id
        .clone()
        .ok_or("the send names its interaction")?;
    let events = runtime.console_events();
    events
        .reserve_interaction_value(
            "resting-own",
            None,
            &interaction,
            "console",
            json!("send-to-own"),
        )
        .await?;
    leaves(&runtime, "resting-own").await?;
    runtime
        .spawn(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from("resting-own"),
        ))
        .await?;
    events
        .record_lifecycle(
            "resting-own",
            "identity_retired",
            json!({ "note": "retire-record" }),
        )
        .await;
    let failure = stored_frame_of_kind(
        &aggregator,
        "resting-own",
        "interaction_failed",
        "send-to-own",
    )
    .await
    .ok_or("the failure record of the send is stored")?;
    private_to_its_owner(&failure, "the failure record of that send");
    let retired = stored_frame(&aggregator, "resting-own", "retire-record")
        .await
        .ok_or("the lifecycle record is stored")?;
    assert!(
        retired.source.member_provenance.is_none(),
        "a record that names no session and no send carries no witness"
    );
    assert!(!timeline_frame_visible_to(&readers.bob, &retired));
    assert!(timeline_frame_visible_to(&readers.root, &retired));

    // The retained records are projected again (a registration replays
    // them), and a failure record arrives for a send this console holds no
    // frame for.
    register_runtime(&runtime, &aggregator)?;
    events
        .record_interaction_failure(
            "resting-own",
            "an-interaction-of-no-stored-send",
            json!({ "error": "failure-without-send" }),
        )
        .await;
    let orphan = stored_frame(&aggregator, "resting-own", "failure-without-send")
        .await
        .ok_or("the failure record with no send is stored")?;
    assert!(
        orphan.source.member_provenance.is_none(),
        "a failure record whose send this console holds no frame for carries no witness"
    );
    assert!(!timeline_frame_visible_to(&readers.bob, &orphan));
    assert!(timeline_frame_visible_to(&readers.root, &orphan));
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let frames = frames_of(&aggregator, "resting-own").await;
    for (kind, what) in [
        ("user_input", "the send"),
        ("interaction_failed", "the failure record of the send"),
    ] {
        let stored = frames
            .iter()
            .filter(|frame| frame.kind == kind && frame.payload.to_string().contains("send-to-own"))
            .collect::<Vec<_>>();
        assert_eq!(
            stored.len(),
            1,
            "{what} is one frame after the records were projected again"
        );
        private_to_its_owner(stored[0], what);
    }
    let _ = runtime.shutdown().await;
    Ok(())
}

/// A member a spawn made shows its first message on its own page from its
/// own session history, with its own witness, on a console log that holds
/// nothing else of it: no kickoff record is needed for it. And every frame
/// an ordinary spawn and its first turn leave that names a session carries
/// that session's witness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawned_member_shows_its_first_message_from_its_own_history() -> TestResult {
    let (_state, runtime, aggregator) = runtime_and_aggregator("witness-history").await?;
    runtime
        .spawn(
            SpawnMemberSpec::new("worker", AgentIdentity::from("gamma")).with_initial_message(
                meerkat_core::types::ContentInput::Text("first-message-of-gamma".to_string()),
            ),
        )
        .await?;
    let first = stored_frame_of_kind(&aggregator, "gamma", "user_input", "first-message-of-gamma")
        .await
        .ok_or("the member's first message is on its page")?;
    let session = session_of(&runtime, "gamma").await?;
    assert_eq!(
        first.session_id.as_deref(),
        Some(session.as_str()),
        "the first message is read from the member's own session"
    );
    assert_ne!(
        first.payload["source_event_type"],
        json!("spawn_initial_message"),
        "it is not a kickoff record"
    );
    assert_eq!(
        first
            .source
            .member_provenance
            .as_ref()
            .map(|witness| witness.identity.identity.as_str()),
        Some("gamma"),
        "it carries the member's own witness"
    );
    let readers = readers();
    assert!(timeline_frame_visible_to(&readers.bob, &first));
    assert!(timeline_frame_visible_to(&readers.root, &first));

    tokio::time::sleep(Duration::from_secs(2)).await;
    let unwitnessed = frames_of(&aggregator, "gamma")
        .await
        .iter()
        .filter(|frame| frame.session_id.is_some() && frame.source.member_provenance.is_none())
        .map(|frame| frame.kind.clone())
        .collect::<Vec<_>>();
    assert!(
        unwitnessed.is_empty(),
        "every frame of an ordinary spawn that names its session carries that session's \
         witness: {unwitnessed:?}"
    );
    let _ = runtime.shutdown().await;
    Ok(())
}

/// Both RPC timeline pages (`meta/console/query_timeline` of a console with
/// its own runtime, and of one that only aggregates) decide each frame by the
/// caller: a frame with no witness is on an administrator's page and on no
/// other person's.
#[tokio::test]
async fn both_rpc_timeline_pages_refuse_a_frame_with_no_witness() -> TestResult {
    let state_dir = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state_dir.path(), "witness-rpc-pages", "", "").await?;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(frame_with("ghost", "rpc-page-record", None))
        .await?;
    let bob = bearer(bob_claims());
    let root = bearer(root_claims());
    for (console, own_runtime) in [
        ("a console with its own runtime", true),
        ("a console that only aggregates", false),
    ] {
        let mut state = enforced_console(&runtime, Some(everyone_drives_every_agent()));
        if !own_runtime {
            state.runtime = None;
        }
        state.console_aggregator = Some(aggregator.clone());
        let app = console_json_router_with_state(state);
        for (reader, token, served) in [
            ("another person", &bob, false),
            ("an administrator", &root, true),
        ] {
            let answer = console_rpc_as(
                &app,
                Some(token),
                "meta/console/query_timeline",
                json!({ "identity": "ghost", "limit": 500 }),
            )
            .await;
            assert!(answer["error"].is_null(), "{console}: {answer:#}");
            assert_eq!(
                answer["result"]["frames"]
                    .to_string()
                    .contains("rpc-page-record"),
                served,
                "the RPC timeline page of {console}, read by {reader}"
            );
        }
    }
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
