//! Detached `fork_off` over the full in-process RPC stack, with a scripted
//! provider: the deterministic twin of live Scenario 96.
//!
//! The forker calls the real `fork_off` tool from inside its own turn, so the
//! tool is bound to the forker's own operation registry by the factory and
//! the job is a real detached background operation of the forker's session.
//! The test then pins the whole detached lifecycle from the forker's side:
//! the call returns `running` with a job id, the child's outcome lands once in
//! the forker's durable transcript, and the forker can run its next turn,
//! whose model request carries that outcome.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{AgentFactory, Config};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::SessionId;
use meerkat_core::{ConfigRuntime, ContentInput, HandlingMode, MemoryConfigStore, Message};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobDefinition, MobHandle, SpawnMemberSpec, WorkOrigin,
    WorkSpec,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_rpc::protocol::{RpcId, RpcRequest};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use tokio::sync::mpsc;

const PARENT: &str = "detached-forker";
const CHILD: &str = "detached-child";
const FORK_PROMPT: &str = "FORK-NOW fork a child for the token";
const CHILD_TASK: &str = "CHILD-TASK-5R reply with the token";
const CHILD_REPLY: &str = "DETACHED-RESULT-3V";
const FORK_DONE: &str = "FORK_DONE";
const FOLLOW_UP_PROMPT: &str = "FOLLOW-UP-9 what did your fork report?";
const FOLLOW_UP_REPLY: &str = "FOLLOW-UP-ACK";
const WAIT: Duration = Duration::from_secs(60);
const WAKE_REPLY: &str = "WAKE-ACK";

// ===========================================================================
// Scripted provider
// ===========================================================================

fn last_user_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The forker calls `fork_off` once, then finishes its turn; the child
/// replies with its token; every other turn acknowledges. Every request is
/// recorded as (last user text, rendered messages).
struct ForkOffScript {
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl ForkOffScript {
    fn events(&self, request: &LlmRequest) -> Vec<LlmEvent> {
        let last_user = last_user_text(request);
        self.requests
            .lock()
            .unwrap()
            .push((last_user.clone(), format!("{:?}", request.messages)));
        let usage = LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        };
        let text = |text: &str| {
            vec![
                LlmEvent::TextDelta {
                    delta: text.to_string(),
                    meta: None,
                },
                usage.clone(),
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ]
        };
        if last_user.contains(CHILD_TASK) {
            return text(CHILD_REPLY);
        }
        // The forker woken by its child's completion record.
        if matches!(request.messages.last(), Some(Message::SystemNotice(_))) {
            return text(WAKE_REPLY);
        }
        if last_user.contains(FORK_PROMPT) {
            let tool_result_seen =
                matches!(request.messages.last(), Some(Message::ToolResults { .. }));
            if tool_result_seen {
                return text(FORK_DONE);
            }
            return vec![
                LlmEvent::ToolCallComplete {
                    id: "toolu_fork_off".to_string(),
                    name: "fork_off".to_string(),
                    args: json!({"member_id": CHILD, "task": CHILD_TASK}),
                    meta: None,
                },
                usage,
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ];
        }
        text(FOLLOW_UP_REPLY)
    }
}

#[async_trait::async_trait]
impl LlmClient for ForkOffScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let events = self.events(request);
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

// ===========================================================================
// RPC stack (the Scenario 96 composition, with the scripted provider)
// ===========================================================================

async fn make_stack(
    root: &std::path::Path,
    client: Arc<dyn LlmClient>,
) -> (MethodRouter, Arc<MobMcpState>) {
    let project_root = root.join("project-root");
    let context_root = root.join("context-root");
    for dir in [&project_root, &context_root] {
        std::fs::create_dir_all(dir).expect("create project/context root");
        std::fs::write(dir.join("AGENTS.md"), "# Detached fork_off\n").expect("write AGENTS.md");
    }
    let runtime_root = root.join("runtime-root");
    let factory = AgentFactory::new(runtime_root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(runtime_root.clone())
        .project_root(project_root)
        .context_root(context_root)
        .builtins(false)
        .shell(false)
        .comms(true)
        .mob(true);
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let persistence = meerkat::PersistenceBundle::new(
        store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        blob_store,
    );
    let runtime = SessionRuntime::new(
        factory,
        config.clone(),
        64,
        persistence,
        NotificationSink::noop(),
    );
    runtime.set_default_llm_client(Some(Arc::clone(&client)));
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        runtime_root.join("config_state.json"),
    )));
    // Mob members (the forker and its fork child) are built by the mob, not
    // by the RPC session path: the scripted provider must be injected here
    // too, or they would reach a real provider.
    let mob_state = Arc::new(
        MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(runtime.runtime_adapter()),
            meerkat_mob::MobControlPrincipal::Owner,
        )
        .with_default_llm_client(Some(client)),
    );
    *runtime.builder_mob_tools_slot.write().unwrap() = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));
    let runtime = Arc::new(runtime);
    let (notif_tx, _notif_rx) = mpsc::channel(256);
    let router = MethodRouter::new_with_mob_state(
        runtime,
        config_store,
        NotificationSink::new(notif_tx),
        Arc::clone(&mob_state),
    );
    (router, mob_state)
}

fn mob_definition(mob_id: &str) -> MobDefinition {
    serde_json::from_value(json!({
        "id": mob_id,
        "profiles": {
            "keeper": {
                "model": "claude-sonnet-4-5",
                "tools": { "comms": true, "mob": true },
                "peer_description": "Forker for the detached fork_off lane",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    }))
    .expect("detached fork_off mob definition")
}

async fn session_history(router: &MethodRouter, session_id: &SessionId) -> Value {
    let params = serde_json::value::RawValue::from_string(
        json!({ "session_id": session_id.to_string(), "limit": 200 }).to_string(),
    )
    .unwrap();
    let response = router
        .dispatch(RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/history".to_string(),
            params: Some(params),
        })
        .await
        .expect("session/history dispatch");
    assert!(
        response.error.is_none(),
        "session/history error: {:?}",
        response.error
    );
    serde_json::from_str(response.result.as_ref().expect("history result").get())
        .expect("history JSON")
}

fn history_messages(history: &Value) -> &Vec<Value> {
    history["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("session/history without messages: {history}"))
}

/// The `fork_off` tool result recorded in the forker's transcript.
fn recorded_fork_off_start(history: &Value) -> Option<Value> {
    history_messages(history).iter().find_map(|message| {
        message["results"].as_array()?.iter().find_map(|result| {
            let content = &result["content"];
            let texts: Vec<String> = match content {
                Value::String(text) => vec![text.clone()],
                Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|block| {
                        block["text"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| block.get("data").map(Value::to_string))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            texts.iter().find_map(|text| {
                serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(|value| value.get("job_id").is_some())
            })
        })
    })
}

/// The durable completion records for `job_id` in the forker's transcript:
/// `BackgroundJob` system notices whose block is `persisted` for this job.
fn recorded_completions(history: &Value, job_id: &str) -> Vec<Value> {
    history_messages(history)
        .iter()
        .filter(|message| {
            message["role"].as_str() == Some("system_notice")
                && message["kind"].as_str() == Some("background_job")
                && message["blocks"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["job_id"].as_str() == Some(job_id)
                            && block["persisted"].as_bool() == Some(true)
                    })
                })
        })
        .cloned()
        .collect()
}

async fn bounded_turn(handle: &MobHandle, identity: &str, prompt: &str) -> String {
    let spec = BoundedResultSpec::new("turn", 16 * 1024).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(identity),
            WorkSpec::new(ContentInput::Text(prompt.to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("start a turn for {identity} ({prompt:?}): {error:?}"));
    tokio::time::timeout(WAIT, work.wait_bounded(spec))
        .await
        .unwrap_or_else(|_| panic!("turn for {identity} ({prompt:?}) timed out"))
        .unwrap_or_else(|error| panic!("turn for {identity} ({prompt:?}) failed: {error:?}"))
        .result()
        .result()
        .text()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_detached_fork_off_reaches_the_forker_and_its_next_turn() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(ForkOffScript {
        requests: Arc::clone(&requests),
    });
    let (router, mob_state) = make_stack(temp.path(), client).await;
    let mob_id = format!("detached-fork-off-{}", uuid::Uuid::new_v4().simple());
    let mob_id = mob_state
        .mob_create_definition(mob_definition(&mob_id))
        .await
        .expect("create mob");
    let handle = mob_state.handle_for(&mob_id).await.expect("mob handle");
    handle
        .spawn_spec(SpawnMemberSpec::new("keeper", AgentIdentity::from(PARENT)))
        .await
        .expect("spawn forker");
    let parent_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(PARENT))
        .await
        .expect("forker session");

    // The forker's own turn calls fork_off; the call returns before the child
    // finishes, so the forker completes its turn.
    assert_eq!(bounded_turn(&handle, PARENT, FORK_PROMPT).await, FORK_DONE);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|(last_user, _)| last_user.contains(FORK_PROMPT)),
        "the forker's turn must run on the scripted provider"
    );
    let history = session_history(&router, &parent_session).await;
    let started = recorded_fork_off_start(&history)
        .unwrap_or_else(|| panic!("the forker's transcript holds no fork_off result: {history}"));
    assert_eq!(started["status"], "running", "{started}");
    assert_eq!(started["agent_identity"], CHILD, "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();

    // The child's outcome reaches the forker's own durable transcript once.
    let deadline = tokio::time::Instant::now() + WAIT;
    let completion = loop {
        let history = session_history(&router, &parent_session).await;
        let completions = recorded_completions(&history, &job_id);
        assert!(completions.len() <= 1, "recorded more than once: {history}");
        if let Some(completion) = completions.into_iter().next() {
            break completion;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the forker never received the fork_off completion: {history}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let content = completion["body"].as_str().expect("completion body");
    assert!(
        content.contains(CHILD_REPLY) && content.contains("completed"),
        "the completion carries the child's completed outcome: {content}"
    );

    // The idle forker is woken by the completion and runs exactly one turn
    // that sees it.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let wake_turns = requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, rendered)| {
                rendered.contains(&job_id) && !rendered.contains(FOLLOW_UP_PROMPT)
            })
            .filter(|(last_user, _)| last_user.contains(FORK_PROMPT))
            .count();
        if wake_turns >= 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the idle forker was never woken by its child's completion"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let wake_turns = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(last_user, rendered)| {
            last_user.contains(FORK_PROMPT) && rendered.contains(&job_id)
        })
        .count();
    assert_eq!(wake_turns, 1, "one wake turn for one completion");

    // The forker keeps working after the detached job ended, and its next
    // model request carries the child's outcome.
    assert_eq!(
        bounded_turn(&handle, PARENT, FOLLOW_UP_PROMPT).await,
        FOLLOW_UP_REPLY
    );
    let follow_up: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(last_user, _)| last_user.contains(FOLLOW_UP_PROMPT))
        .map(|(_, rendered)| rendered.clone())
        .collect();
    assert_eq!(follow_up.len(), 1, "one follow-up request: {follow_up:#?}");
    assert!(
        follow_up[0].contains(&job_id) && follow_up[0].contains(CHILD_REPLY),
        "the forker's next request carries the child's outcome: {}",
        follow_up[0]
    );

    let _ = mob_state.mob_destroy(&mob_id).await;
}
