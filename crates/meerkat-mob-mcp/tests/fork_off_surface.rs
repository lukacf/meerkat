//! fork_off and council through the agent-facing surface, over the production
//! persistent session service with a scripted provider.
//!
//! These pin the headline paths of the detached contract: a fork_off returns
//! promptly and its outcome reaches the forker as a durable transcript entry
//! plus a completed background job; one-shot hosts block for the result; the
//! forker can observe and retire its own children without manage scope.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::ops_lifecycle::{OperationStatus, OpsLifecycleRegistry};
use meerkat_core::types::ToolCallView;
use meerkat_mob::AgentIdentity;
use meerkat_mob_mcp::{AgentMobToolSurface, DetachedCompletionDelivery};
use serde_json::json;
use support::{CouncilFixture, ScriptedTurn};

const CHILD_REPLY: &str = "FORKED-RESULT-7Q";

fn forker_authority(mob_id: &str) -> meerkat_core::service::MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    let authority =
        meerkat_runtime::mob_operator_authority::set_create_authority(&authority, false)
            .expect("no create scope");
    meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &authority,
        mob_id,
        "participant",
    )
    .expect("spawnable participant profile, no manage scope")
}

struct Forker {
    fixture: CouncilFixture,
    surface: Arc<dyn AgentToolDispatcher>,
    registry: Arc<meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    session: meerkat_core::SessionId,
}

async fn forker() -> Forker {
    let fixture = CouncilFixture::new(|_| ScriptedTurn::Text(CHILD_REPLY.to_string()));
    fixture.seed_source_mob(&["forker", "bystander"]).await;
    let mob_id = fixture.source_mob_id();
    let handle = fixture.state.handle_for(&mob_id).await.expect("handle");
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from("forker"))
        .await
        .expect("forker session");
    let registry = Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new());
    let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
        Arc::clone(&fixture.state),
        None,
        forker_authority(mob_id.as_str()),
        "claude-sonnet-4-5".to_string(),
        session.clone(),
        None,
        None,
        None,
    ));
    let surface = surface
        .bind_ops_lifecycle(registry.clone(), session.clone())
        .expect("bind")
        .into_dispatcher();
    Forker {
        fixture,
        surface,
        registry,
        session,
    }
}

async fn call(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: serde_json::Value,
) -> Result<serde_json::Value, meerkat_core::ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    let outcome = surface
        .dispatch(ToolCallView {
            id: "surface-call",
            name,
            args: &raw,
        })
        .await?;
    Ok(serde_json::from_str(&outcome.result.text_content()).expect("json tool result"))
}

async fn owner_transcript_text(
    fixture: &CouncilFixture,
    session: &meerkat_core::SessionId,
) -> String {
    let persisted = <meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder> as meerkat_mob::MobSessionService>::load_persisted_session(
        fixture.service.as_ref(),
        session,
    )
    .await
    .expect("load persisted forker session")
    .expect("forker session exists");
    format!("{:?}", persisted.messages())
}

#[tokio::test]
async fn detached_fork_off_delivers_a_durable_completion_to_the_forker() {
    let Forker {
        fixture,
        surface,
        registry,
        session,
    } = forker().await;

    let started = call(
        &surface,
        "fork_off",
        json!({"member_id": "surface-child", "task": "reply with the token"}),
    )
    .await
    .expect("fork_off starts");
    assert_eq!(started["status"], "running", "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    assert_eq!(started["agent_identity"], "surface-child");

    // The job completes with the child's typed outcome.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let content = loop {
        let completed = registry
            .list_operations()
            .expect("operations")
            .into_iter()
            .find(|operation| operation.id.to_string() == job_id);
        if let Some(operation) = completed
            && operation.status == OperationStatus::Completed
        {
            break format!("{operation:?}");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fork job never completed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(content.contains(&job_id));

    // The outcome is recorded durably in the forker's own transcript, so it
    // outlives the refresh notice: a reload from the store still has it.
    let transcript = owner_transcript_text(&fixture, &session).await;
    assert!(
        transcript.contains(&job_id) && transcript.contains(CHILD_REPLY),
        "the forker's durable transcript must hold the completion: {transcript}"
    );

    // The completed child stays seated and belongs to the forker.
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    let child = handle
        .get_member(&AgentIdentity::from("surface-child"))
        .await
        .unwrap()
        .expect("completed child stays seated");
    assert_eq!(child.spawned_by, Some(AgentIdentity::from("forker")));
    fixture.teardown().await;
}

#[tokio::test]
async fn one_shot_hosts_block_for_the_fork_off_result() {
    let Forker {
        fixture, surface, ..
    } = forker().await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);

    let result = call(
        &surface,
        "fork_off",
        json!({"member_id": "blocking-child", "task": "reply with the token", "max_run_secs": 120}),
    )
    .await
    .expect("blocking fork_off returns the result");
    assert_eq!(result["agent_identity"], "blocking-child");
    assert_eq!(result["bounded_result"]["text"], CHILD_REPLY, "{result}");
    fixture.teardown().await;
}

#[tokio::test]
async fn forker_checks_lists_and_retires_its_own_children_on_the_agent_surface() {
    let Forker {
        fixture, surface, ..
    } = forker().await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let mob_id = fixture.source_mob_id().to_string();
    call(
        &surface,
        "fork_off",
        json!({"member_id": "owned-child", "task": "reply with the token"}),
    )
    .await
    .expect("fork_off");

    call(
        &surface,
        "mob_check_member",
        json!({"mob_id": mob_id, "member_id": "owned-child"}),
    )
    .await
    .expect("the forker checks its own child");
    assert!(matches!(
        call(
            &surface,
            "mob_check_member",
            json!({"mob_id": mob_id, "member_id": "bystander"}),
        )
        .await,
        Err(meerkat_core::ToolError::AccessDenied { .. })
    ));
    let listed = call(&surface, "mob_list_members", json!({"mob_id": mob_id}))
        .await
        .expect("owner view of the member list");
    let listed = listed["members"].as_array().expect("members").clone();
    assert_eq!(listed.len(), 1, "only the forker's own child: {listed:?}");
    assert!(format!("{listed:?}").contains("owned-child"));

    assert!(matches!(
        call(
            &surface,
            "mob_retire_member",
            json!({"mob_id": mob_id, "member_id": "bystander"}),
        )
        .await,
        Err(meerkat_core::ToolError::AccessDenied { .. })
    ));
    call(
        &surface,
        "mob_retire_member",
        json!({"mob_id": mob_id, "member_id": "owned-child"}),
    )
    .await
    .expect("the forker retires its own child");
    let handle = fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .unwrap();
    assert!(
        handle
            .get_member(&AgentIdentity::from("owned-child"))
            .await
            .unwrap()
            .is_none()
    );
    fixture.teardown().await;
}
