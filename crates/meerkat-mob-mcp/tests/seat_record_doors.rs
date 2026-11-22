//! The seat record at the council's door: a participant the agent `council`
//! tool seats is recorded as seated for the convener that called the council,
//! through the tool's real seating. Real components throughout (the mob set,
//! real source members, the real temporary mob and its real seats); only the
//! LLM is scripted.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;

use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, SessionId, ToolDispatchOutcome, ToolError};
use meerkat_mob::{
    AgentIdentity, LineageClosed, MemberLink, MemberSeatKind, MobBackendKind, MobRuntimeMode,
    ProfileName, SeatNow, SeatRecorder, SeatSealer,
};
use meerkat_mob_mcp::lineage::{LineageEnd, LineageStep};
use meerkat_mob_mcp::{AgentMobToolSurface, DetachedCompletionDelivery, MobMcpState};
use serde_json::{Value, json};
use support::{CouncilFixture, ScriptedTurn, TurnGate, user_text};

fn agent_surface(
    state: &Arc<MobMcpState>,
    session: SessionId,
    authority: meerkat_core::service::MobToolAuthorityContext,
) -> Arc<dyn AgentToolDispatcher> {
    let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
        Arc::clone(state),
        None,
        authority,
        "claude-sonnet-4-5".to_string(),
        session.clone(),
        None,
        None,
        None,
    ));
    surface
        .bind_ops_lifecycle(
            Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
            session,
        )
        .expect("bind ops lifecycle")
        .into_dispatcher()
}

/// Create scope plus manage scope over `mob_id` as it runs at `incarnation`,
/// as a council convener holds.
fn convener_authority(
    mob_id: &str,
    incarnation: meerkat_core::service::LiveMobIncarnation,
) -> meerkat_core::service::MobToolAuthorityContext {
    let creator = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    meerkat_runtime::mob_operator_authority::grant_manage_mob(&creator, mob_id, incarnation)
        .expect("manage scope")
}

async fn dispatch(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    surface
        .dispatch(ToolCallView {
            id: "surface-call",
            name,
            args: &raw,
        })
        .await
}

/// While the council a member convened is mid-turn, every participant's seat
/// in the temporary mob records the convener's seat in its own mob, sealed by
/// the set: the host's seat for that member, never the host's own and never
/// a bare copy of the participant's source.
#[tokio::test(flavor = "multi_thread")]
async fn a_councils_participant_is_seated_for_the_convener_that_called_the_council() {
    let gate = TurnGate::new();
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new_runtime_backed(move |request| {
        let text = user_text(request);
        if text.contains("bounded plain-text summary") {
            ScriptedTurn::Text("the council agreed".to_string())
        } else if text.contains("Council topic:") {
            ScriptedTurn::Gated(gate_for_script.clone(), "gated position".to_string())
        } else {
            ScriptedTurn::Text("ok".to_string())
        }
    });
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let mob_id = fixture.source_mob_id();
    let handle = fixture
        .state
        .handle_for(&mob_id)
        .await
        .expect("source mob handle");
    let convener_identity = AgentIdentity::from("convener");
    let convener_session = handle
        .resolve_bridge_session_id(&convener_identity)
        .await
        .expect("the convener has a bridge session");
    let convener = agent_surface(
        &fixture.state,
        convener_session.clone(),
        convener_authority(mob_id.as_str(), handle.live_incarnation()),
    );
    let council_id = fixture.council_id("seat-record");
    let temporary_mob_id = council_id.temporary_mob_id();
    let args = json!({
        "topic": "Should we ship the migration this week?",
        "participants": [
            {"mob_id": mob_id.as_str(), "member_id": "alice", "role": "analyst"},
            {"mob_id": mob_id.as_str(), "member_id": "bob", "role": "critic"},
        ],
        "max_rounds": 1,
        "timeout_seconds": 120,
        "council_id": council_id.as_str(),
    });
    let running = tokio::spawn(async move { dispatch(&convener, "council", args).await });
    gate.wait_entered(1).await;

    let temporary = fixture
        .state
        .handle_for(&temporary_mob_id)
        .await
        .expect("the temporary mob is live while the council runs");
    let participants = temporary.list_members().await;
    assert_eq!(participants.len(), 2, "both participants are seated");
    for participant in &participants {
        let seat = temporary
            .member_seat(&participant.agent_identity)
            .await
            .expect("a seated participant has a seat record");
        assert_eq!(
            seat.kind(),
            MemberSeatKind::HostForMember,
            "{}",
            participant.agent_identity
        );
        assert_eq!(seat.sealed_by(), Some(SeatSealer::Set));
        assert_eq!(seat.recorded_by(), Some(SeatRecorder::Shell));
        let recorded = seat.recorded().expect("the convener's seat");
        assert_eq!(recorded.parent_mob_id, mob_id);
        assert_eq!(recorded.creator_identity, convener_identity);
        assert_eq!(recorded.owner_session, convener_session);
        assert_eq!(handle.seat_now(recorded).await, SeatNow::Current);
    }
    // A handle kept here would own the temporary mob database's observer, and
    // the council's destroy would leave its storage cleanup outstanding.
    drop(temporary);

    gate.open();
    running
        .await
        .expect("owned task joins")
        .expect("the council completes");
    fixture.teardown().await;
}

/// A copy's lineage continues at its source while the source's mob holds,
/// under that name, a member of the source's creation. Once the source is
/// gone, and once another member holds its name, the copy's lineage closes:
/// the name alone gives it nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_a_source_whose_name_another_member_took_closes() {
    let fixture =
        CouncilFixture::new_runtime_backed(|_request| ScriptedTurn::Text("ok".to_string()));
    fixture.seed_source_mob(&["source"]).await;
    let mob_id = fixture.source_mob_id();
    let source = AgentIdentity::from("source");
    let copy = AgentIdentity::from("copy");
    fixture
        .state
        .mob_fork_member(
            &mob_id,
            &source,
            copy.clone(),
            None,
            Some(MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("the host copies the source");

    let copied = fixture.state.lineage_of(&mob_id, &copy).await;
    assert!(copied.is_host_declared(), "{copied:?}");
    assert!(
        matches!(
            copied.steps.first(),
            Some(LineageStep::Linked {
                link: MemberLink::ForkOf,
                now: Some(SeatNow::SameLife),
                ..
            })
        ),
        "{copied:?}"
    );

    fixture
        .state
        .mob_retire(&mob_id, source.clone())
        .await
        .expect("the source retires");
    let handle = fixture
        .state
        .handle_for(&mob_id)
        .await
        .expect("source mob handle");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(1);
    while handle
        .get_member(&source)
        .await
        .expect("roster read")
        .is_some()
    {
        assert!(tokio::time::Instant::now() < deadline, "the source left");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        fixture.state.lineage_of(&mob_id, &copy).await.end,
        LineageEnd::Closed(LineageClosed::CreatorGone),
        "no member holds the source's name"
    );
    fixture
        .state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            source.clone(),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("the host seats another member under the source's name");
    assert_eq!(
        fixture.state.lineage_of(&mob_id, &copy).await.end,
        LineageEnd::Closed(LineageClosed::CreatorGone),
        "a later holder of the source's name"
    );
    drop(handle);
    fixture.teardown().await;
}
