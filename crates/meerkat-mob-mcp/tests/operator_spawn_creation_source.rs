//! A member spawning through the mob operator tools records its own session
//! as the child's creation source, end to end over the production session
//! service.
//!
//! The operator tools are mounted in the member's external tool surface. When
//! that surface also carries an external tool source, the composed external
//! dispatcher is shared between the session request and the agent build, so
//! owner identity must not depend on an exclusive late bind of the dispatcher.
//! The calling member is the session that dispatched the tool call.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::{ContentInput, HandlingMode, Message, ToolCallView, ToolDef, ToolError};
use meerkat_mob::{
    AgentIdentity, MemberCreationProvenance, MobBackendKind, MobDefinition, MobId, ProfileBinding,
    ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use support::{CouncilFixture, ScriptedTurn, last_user_text, participant_profile};

const PROBE: &str = "OPERATOR-SPAWN-PROBE";
const PARENT: &str = "operator-parent";
const CHILD: &str = "operator-child";
const EXTERNAL_TOOL: &str = "lookup_record";

fn operator_definition(mob_id: &MobId) -> MobDefinition {
    let mut profile = participant_profile("member with mob operator tools");
    profile.tools.mob = true;
    // The test drives the member's own turn from outside the mob.
    profile.external_addressable = true;
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles = profiles;
    definition
}

/// One host tool, standing in for an external tool source composed next to
/// the operator tools.
struct ExternalSource;

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for ExternalSource {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef::new(
            EXTERNAL_TOOL,
            "look up a record",
            serde_json::json!({ "type": "object", "properties": {} }),
        ))])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        Ok(meerkat_core::ToolResult::new(call.id.to_string(), "record".to_string(), false).into())
    }
}

fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

type SpawnResult = Arc<Mutex<Option<String>>>;

/// The parent's model: on the probe turn, call `spawn_member` once, then
/// record the result it saw.
fn spawn_script(
    spawn_result: SpawnResult,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        if !last_user_text(request).contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        match tool_results(&request.messages).get("call-spawn") {
            None => ScriptedTurn::ToolCall {
                id: "call-spawn".to_string(),
                name: "spawn_member".to_string(),
                args: serde_json::json!({ "profile": "participant", "member_id": CHILD }),
            },
            Some(result) => {
                *spawn_result.lock().unwrap() = Some(result.clone());
                ScriptedTurn::Text("spawned".to_string())
            }
        }
    }
}

async fn assert_operator_spawn_records_parent_as_source(
    state: &Arc<MobMcpState>,
    spawn_result: &SpawnResult,
) {
    let mob_id = MobId::from(format!("operator-spawn-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(operator_definition(&mob_id))
        .await
        .expect("create the mob");
    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from(PARENT),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the parent member");
    let handle = state.handle_for(&mob_id).await.expect("mob handle");
    let parent_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(PARENT))
        .await
        .expect("parent session");
    let turn = handle
        .member(&AgentIdentity::from(PARENT))
        .await
        .expect("parent member handle")
        .start_turn(
            ContentInput::Text(PROBE.to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("probe turn admitted");
    tokio::time::timeout(Duration::from_secs(60), turn.wait())
        .await
        .expect("the probe turn completes")
        .expect("the probe turn succeeds");

    let result = spawn_result
        .lock()
        .unwrap()
        .clone()
        .expect("the probe turn saw the spawn_member result");
    assert!(
        !result.contains("\"error\""),
        "spawn_member succeeds: {result}"
    );
    let child_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(CHILD))
        .await
        .expect("child session");
    let creation = handle
        .member_creation_for_session(&child_session)
        .await
        .expect("read the child's creation facts")
        .expect("the child has creation facts");
    match &creation.creation.provenance {
        MemberCreationProvenance::Spawn { source } => assert_eq!(
            source.session_id, parent_session,
            "the child's creation source is the calling member's session"
        ),
        other => panic!("operator spawn_member must record Spawn {{ source }}, got {other:?}"),
    }
    let _ = state.mob_destroy(&mob_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_spawn_with_an_external_tool_source_records_the_parent_as_source() {
    let spawn_result = SpawnResult::default();
    let fixture = CouncilFixture::new_runtime_backed_with(
        spawn_script(Arc::clone(&spawn_result)),
        |state, _root| {
            state.with_external_tools_provider(Some(Arc::new(|| {
                Some(Arc::new(ExternalSource) as Arc<dyn meerkat_core::AgentToolDispatcher>)
            })))
        },
    );
    assert_operator_spawn_records_parent_as_source(&fixture.state, &spawn_result).await;
    fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_spawn_without_external_tools_records_the_parent_as_source() {
    let spawn_result = SpawnResult::default();
    let fixture = CouncilFixture::new_runtime_backed(spawn_script(Arc::clone(&spawn_result)));
    assert_operator_spawn_records_parent_as_source(&fixture.state, &spawn_result).await;
    fixture.teardown().await;
}
