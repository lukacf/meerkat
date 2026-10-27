//! Console side chats (`meta/fork_member`): a durable fork of an agent's
//! conversation into a new member the human continues in, unwired, kept until
//! it is closed, and listed under its source from Meerkat's durable roster.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat::{AgentFactory, Config};
use meerkat_client::TestClient;
use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
use serde_json::{Value, json};

use super::*;
use crate::console_aggregator::AllowAllConsoleVisibilityPolicy;
use crate::{MobBootstrapOptions, MobBootstrapSpec};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const WORKER_PROFILES: &str = r#"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true

[profiles.lead]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.lead.tools]
comms = true
"#;

/// A console runtime whose sessions persist (SQLite session and runtime
/// stores), which a durable fork needs. `mob_header` holds the `[mob]` table
/// lines after `id`, and `tail` any `[wiring]` tables.
async fn persistent_console_runtime(
    mob_id: &str,
    mob_header: &str,
    tail: &str,
) -> Result<(tempfile::TempDir, MobRuntime), Box<dyn std::error::Error + Send + Sync>> {
    let temp_dir = tempfile::tempdir()?;
    let state = temp_dir.path().join("state");
    std::fs::create_dir_all(&state)?;
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))?,
    );
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))?,
    );
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let factory = AgentFactory::new(&state).comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, Config::default());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    builder.default_blob_store = Some(blob_store.clone());
    let machine = Arc::new(meerkat_runtime::MeerkatMachine::persistent(
        Arc::clone(&runtime_store),
        Arc::clone(&blob_store),
    )?);
    let session_service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        16,
        session_store,
        runtime_store,
        blob_store,
    ));
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n{mob_header}\n{WORKER_PROFILES}\n{tail}"
    ))?;
    let runtime = MobRuntime::bootstrap(
        MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
            .with_session_runtime_adapter(machine)?
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(TestClient::default())),
            }),
    )
    .await?;
    Ok((temp_dir, runtime))
}

fn request(method: &str, params: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    }
}

async fn call(runtime: &MobRuntime, method: &str, params: Value) -> Value {
    Box::pin(handle_console_runtime_rpc(
        runtime,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        request(method, params),
        true,
    ))
    .await
}

/// Spawn `alias` as a member of `role` and run one committed turn on it, so
/// its session holds a transcript to fork.
async fn seeded_member(runtime: &MobRuntime, role: &str, alias: &str) -> TestResult {
    let identity = AgentIdentity::from(alias);
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new(role, identity.clone())),
    )
    .await?;
    let member = runtime.handle().member(&identity).await?;
    let turn = member
        .start_turn(
            ContentInput::Text("The code word is heron.".to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?;
    turn.wait().await?;
    Ok(())
}

/// `meta/fork_member` for `source`, retried while the runtime still settles
/// the source's last turn (a typed busy refusal), as the console waits for an
/// idle agent.
async fn fork_when_idle(runtime: &MobRuntime, source: &str, side_chat: &str) -> Value {
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    for _ in 0..50 {
        let answer = call(runtime, "meta/fork_member", params.clone()).await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            return answer;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    call(runtime, "meta/fork_member", params).await
}

async fn bridge_session(runtime: &MobRuntime, alias: &str) -> Option<String> {
    runtime
        .handle()
        .get_member(&AgentIdentity::from(alias))
        .await
        .ok()
        .flatten()
        .and_then(|entry| entry.bridge_session_id().map(ToString::to_string))
}

/// The fork is a spawn of the side chat and a read of its source: the
/// console gates it on `agent.spawn` for the new member (the source's
/// `agent.view` is judged in the handler), refuses it on a read-only console
/// and on a candidate launch.
#[test]
fn fork_member_is_gated_as_a_spawn_of_the_side_chat() {
    assert_eq!(
        console_rpc_access_requirements(
            "meta/fork_member",
            &json!({ "source_member_id": "alpha", "agent_identity": "alpha-side" }),
        ),
        Some(vec![(ACTION_AGENT_SPAWN, Some("alpha-side".to_string()))])
    );
    assert!(is_console_mutating_rpc_method("meta/fork_member"));
    assert!(crate::rpc::CANDIDATE_LAUNCH_REFUSED_METHODS.contains(&"meta/fork_member"));
}

/// A side chat starts from its source's committed transcript, leaves the
/// source's session untouched, is seated without a peer edge, is projected
/// under its source from Meerkat's roster, and closes through the ordinary
/// member retire.
#[tokio::test]
async fn a_side_chat_forks_the_source_unwired_and_closes_on_retire() -> TestResult {
    let (_temp_dir, runtime) = persistent_console_runtime("console-side-chat-fork", "", "").await?;
    seeded_member(&runtime, "worker", "alpha").await?;
    let source_session = bridge_session(&runtime, "alpha").await;
    assert!(source_session.is_some(), "the source has a session");

    let capabilities = call(&runtime, "meta/capabilities", json!({})).await;
    assert!(
        capabilities["result"]["methods"]
            .as_array()
            .is_some_and(|methods| methods.contains(&json!("meta/fork_member"))),
        "a console over persistent sessions offers side chats: {capabilities:#?}"
    );

    let forked = fork_when_idle(&runtime, "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "the fork succeeds: {forked:#?}");
    let result = &forked["result"];
    assert_eq!(result["agent_identity"], json!("alpha-side"));
    assert_eq!(result["side_chat_of"], json!("alpha"));
    assert_eq!(result["source_member_id"], json!("alpha"));
    assert_eq!(result["retention"], json!("until_closed"));
    assert_eq!(result["display_name"], json!("alpha (side chat)"));
    assert_eq!(
        result["source_session_id"].as_str(),
        source_session.as_deref(),
        "the side chat names the source session it was forked from"
    );
    assert_ne!(
        result["session_id"].as_str(),
        source_session.as_deref(),
        "the side chat runs on its own session"
    );

    assert_eq!(
        bridge_session(&runtime, "alpha").await,
        source_session,
        "the source keeps its session"
    );
    let side = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-side"))
        .await?
        .expect("the side chat is seated");
    assert!(side.wired_to.is_empty(), "a side chat has no peer edge");
    assert!(side.spawned_by.is_none());
    assert_eq!(
        side.labels.get(crate::side_chat::SIDE_CHAT_OF_LABEL),
        Some(&"alpha".to_string())
    );
    let source = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha"))
        .await?
        .expect("the source stays seated");
    assert!(source.wired_to.is_empty(), "the source gained no peer");

    assert_eq!(
        crate::side_chat::verified_side_chats(&runtime.handle()).await,
        BTreeMap::from([("alpha-side".to_string(), "alpha".to_string())]),
        "the roster's fork facts name the side chat's source"
    );
    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    assert_eq!(
        snapshot.side_chats.get("alpha-side").map(String::as_str),
        Some("alpha"),
        "the console snapshot lists the side chat under its source"
    );
    let side_row = snapshot
        .members
        .iter()
        .find(|member| member.agent_identity == "alpha-side")
        .expect("the side chat is a console member");
    assert!(
        !side_row
            .labels
            .contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL),
        "the marker label is never projected; the verified relation is"
    );

    let retired = call(
        &runtime,
        "meta/retire_member",
        json!({ "member_id": "alpha-side" }),
    )
    .await;
    assert_eq!(retired["result"]["accepted"], json!(true), "{retired:#?}");
    assert_eq!(
        bridge_session(&runtime, "alpha").await,
        source_session,
        "closing the side chat leaves the source as it was"
    );
    Ok(())
}

/// A role the mob definition wires to peers cannot hold an unwired side
/// chat: Meerkat would wire the child when it seats it. The fork is refused,
/// typed, before anything is forked.
#[tokio::test]
async fn a_side_chat_of_a_wired_role_is_refused_before_anything_is_forked() -> TestResult {
    let (_temp_dir, runtime) = persistent_console_runtime(
        "console-side-chat-wired",
        "orchestrator = \"lead\"",
        "[wiring]\nauto_wire_orchestrator = true\n",
    )
    .await?;
    seeded_member(&runtime, "worker", "beta").await?;

    let refused = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "beta", "agent_identity": "beta-side" }),
    )
    .await;
    assert_eq!(
        refused["error"]["code"],
        json!(crate::side_chat::SIDE_CHAT_FORK_REFUSED_CODE),
        "{refused:#?}"
    );
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("unwired_seat_unavailable")
    );
    assert_eq!(
        refused["error"]["data"]["wiring"],
        json!({ "rule": "auto_wire_orchestrator", "orchestrator_role": "lead" })
    );
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("beta-side"))
            .await?
            .is_none(),
        "nothing was seated"
    );
    Ok(())
}

/// The fork takes the source's committed transcript end only: a prefix is
/// refused, not ignored, and the request names both members.
#[tokio::test]
async fn a_fork_request_names_both_members_and_takes_no_prefix() -> TestResult {
    let (_temp_dir, runtime) =
        persistent_console_runtime("console-side-chat-params", "", "").await?;
    let missing = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "alpha" }),
    )
    .await;
    assert_eq!(missing["error"]["code"], json!(-32602), "{missing:#?}");
    let prefix = call(
        &runtime,
        "meta/fork_member",
        json!({
            "source_member_id": "alpha",
            "agent_identity": "alpha-side",
            "message_count": 2,
        }),
    )
    .await;
    assert_eq!(prefix["error"]["code"], json!(-32602), "{prefix:#?}");
    let unknown = call(
        &runtime,
        "meta/fork_member",
        json!({ "source_member_id": "nobody", "agent_identity": "nobody-side" }),
    )
    .await;
    assert_eq!(unknown["error"]["code"], json!(-32602), "{unknown:#?}");
    Ok(())
}
