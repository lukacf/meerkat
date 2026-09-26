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
#![allow(clippy::result_large_err)]
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

/// The structured result of tool call `tool_use_id` in a transcript.
fn recorded_tool_result(history: &Value, tool_use_id: &str) -> Option<Value> {
    history_messages(history).iter().find_map(|message| {
        message["results"].as_array()?.iter().find_map(|result| {
            if result["tool_use_id"].as_str() != Some(tool_use_id) {
                return None;
            }
            result["content"]
                .as_array()?
                .iter()
                .find_map(|block| block.get("data").cloned())
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
    let Ok(outcome) = tokio::time::timeout(WAIT, work.wait_bounded(spec)).await else {
        panic!("turn for {identity} ({prompt:?}) timed out");
    };
    outcome
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
    let header = completion["body"].as_str().expect("completion header");
    assert!(
        header.contains("(completed)"),
        "the record says the child completed: {completion}"
    );
    let detail = completion["blocks"]
        .as_array()
        .and_then(|blocks| blocks.iter().find_map(|block| block["detail"].as_str()))
        .expect("completion detail");
    assert!(
        detail.contains(CHILD_REPLY),
        "the record carries the child's outcome: {completion}"
    );

    // The idle forker is woken by the completion and runs exactly one turn
    // that sees it (the only requests before the follow-up that carry the
    // persisted record).
    let wake_turns = |requests: &Mutex<Vec<(String, String)>>| {
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(last_user, rendered)| {
                !last_user.contains(FOLLOW_UP_PROMPT)
                    && rendered.contains(&job_id)
                    && rendered.contains("persisted: true")
            })
            .count()
    };
    let deadline = tokio::time::Instant::now() + WAIT;
    while wake_turns(&requests) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the idle forker was never woken by its child's completion"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(wake_turns(&requests), 1, "one wake turn for one completion");

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

/// MobKit's gateway builds its mob state with `MobMcpState::new` over a
/// session service that forwards the runtime. That shape must take the
/// detached path, never the blocking one (HomeCore's fork_off would
/// otherwise block its caller for the child's whole run).
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_library_host_built_like_mobkit_delivers_detached() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(ForkOffScript {
        requests: Arc::clone(&requests),
    });
    let (_router, stack_state) = make_stack(temp.path(), client).await;
    let mobkit_shaped = MobMcpState::new(
        stack_state.session_service(),
        meerkat_mob::MobControlPrincipal::Owner,
    );
    assert_eq!(
        mobkit_shaped.detached_delivery_blocked_because(),
        None,
        "a library host whose session service carries a runtime delivers detached"
    );
}

async fn wait_for_single_record(router: &MethodRouter, session: &SessionId, job_id: &str) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let history = session_history(router, session).await;
        let records = recorded_completions(&history, job_id);
        assert!(records.len() <= 1, "recorded more than once: {history}");
        if records.len() == 1 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the owner never recorded job {job_id}: {history}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A completion for an owner that is not live in the runtime (it never ran a
/// turn, or its idle executor was retired) is still recorded once and wakes
/// the owner for one turn. Re-delivering the same job records nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_detached_completion_reaches_owners_that_are_not_live() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(ForkOffScript {
        requests: Arc::clone(&requests),
    });
    let (router, mob_state) = make_stack(temp.path(), client).await;
    let runtime = mob_state
        .session_service()
        .runtime_adapter()
        .expect("runtime-backed stack");
    let mob_id = format!("not-live-owner-{}", uuid::Uuid::new_v4().simple());
    let mob_id = mob_state
        .mob_create_definition(mob_definition(&mob_id))
        .await
        .expect("create mob");
    let handle = mob_state.handle_for(&mob_id).await.expect("mob handle");

    for (owner, tear_down) in [("never-ran-a-turn", false), ("executor-retired", true)] {
        handle
            .spawn_spec(SpawnMemberSpec::new("keeper", AgentIdentity::from(owner)))
            .await
            .expect("spawn owner");
        let session = handle
            .resolve_bridge_session_id(&AgentIdentity::from(owner))
            .await
            .expect("owner session");
        if tear_down {
            assert_eq!(
                bounded_turn(&handle, owner, FOLLOW_UP_PROMPT).await,
                FOLLOW_UP_REPLY
            );
            runtime
                .unregister_session(&session)
                .await
                .expect("the runtime retires the idle executor");
        }
        let job_id = format!("job-{owner}");
        let before = requests.lock().unwrap().len();
        let delivered = meerkat_mob_mcp::deliver_detached_completion_to_member(
            &runtime,
            &handle,
            &AgentIdentity::from(owner),
            &session,
            "fork_off",
            &job_id,
            meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            json!({"agent_identity": "some-child", "status": "completed"}),
        )
        .await;
        assert!(delivered.is_ok(), "{owner}: {delivered:?}");
        wait_for_single_record(&router, &session, &job_id).await;
        let deadline = tokio::time::Instant::now() + WAIT;
        while requests.lock().unwrap().len() == before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{owner} was never woken"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let again = meerkat_mob_mcp::deliver_detached_completion_to_member(
            &runtime,
            &handle,
            &AgentIdentity::from(owner),
            &session,
            "fork_off",
            &job_id,
            meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            json!({"agent_identity": "some-child", "status": "completed"}),
        )
        .await;
        assert!(again.is_ok(), "{owner} redelivery: {again:?}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        wait_for_single_record(&router, &session, &job_id).await;
    }
    let _ = mob_state.mob_destroy(&mob_id).await;
}

const MID_TURN_PROMPT: &str = "MIDTURN-FORK fork and keep working";

/// A forker that calls fork_off, then keeps working (a second tool call)
/// until its child's completion has been admitted, so the completion arrives
/// while the forker's own turn is still running.
struct MidTurnScript {
    requests: Arc<Mutex<Vec<(String, String, bool)>>>,
    child_replied: Arc<std::sync::atomic::AtomicBool>,
}

fn scripted_text(model: &str, text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta {
            delta: text.to_string(),
            meta: None,
        },
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                model,
                meerkat_core::Usage::default(),
            ),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::EndTurn,
            },
        },
    ]
}

fn scripted_tool_call(model: &str, id: &str, name: &str, args: Value) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolCallComplete {
            id: id.to_string(),
            name: name.to_string(),
            args,
            meta: None,
        },
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                model,
                meerkat_core::Usage::default(),
            ),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::ToolUse,
            },
        },
    ]
}

#[async_trait::async_trait]
impl LlmClient for MidTurnScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let last_user = last_user_text(request);
        let rendered = format!("{:?}", request.messages);
        let wake = matches!(request.messages.last(), Some(Message::SystemNotice(_)));
        self.requests
            .lock()
            .unwrap()
            .push((last_user.clone(), rendered.clone(), wake));
        let model = request.model.clone();
        let child_replied = Arc::clone(&self.child_replied);
        Box::pin(futures::StreamExt::flat_map(
            futures::stream::once(async move {
                if last_user.contains(CHILD_TASK) {
                    child_replied.store(true, std::sync::atomic::Ordering::SeqCst);
                    return scripted_text(&model, CHILD_REPLY);
                }
                if wake {
                    return scripted_text(&model, WAKE_REPLY);
                }
                if !last_user.contains(MID_TURN_PROMPT) {
                    return scripted_text(&model, FOLLOW_UP_REPLY);
                }
                match request_stage(&rendered) {
                    0 => scripted_tool_call(
                        &model,
                        "toolu_fork_mid",
                        "fork_off",
                        json!({"member_id": CHILD, "task": CHILD_TASK}),
                    ),
                    1 => {
                        // Keep the turn open until the child has finished and
                        // its completion has had time to be admitted.
                        while !child_replied.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        tokio::time::sleep(Duration::from_millis(400)).await;
                        scripted_tool_call(&model, "toolu_list_mid", "mob_list", json!({}))
                    }
                    _ => scripted_text(&model, FORK_DONE),
                }
            }),
            |events| futures::stream::iter(events.into_iter().map(Ok)),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// 0 before fork_off, 1 after its result, 2 after the follow-up tool result.
fn request_stage(rendered: &str) -> usize {
    if rendered.contains("toolu_list_mid") {
        2
    } else if rendered.contains("toolu_fork_mid") {
        1
    } else {
        0
    }
}

/// A completion that arrives while the forker's turn is still running is not
/// lost and not duplicated: it is recorded once and the forker runs exactly
/// one follow-up turn that sees it right after its current turn ends. (The
/// running turn's own later model calls do not see it: the input's main
/// text is empty and its only content is the typed append, which the
/// request-only mid-turn steer lane does not carry, so admission queues it
/// for the next run.)
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_detached_completion_during_a_running_turn_is_delivered_once_after_it() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(MidTurnScript {
        requests: Arc::clone(&requests),
        child_replied: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    let (router, mob_state) = make_stack(temp.path(), client).await;
    let mob_id = format!("mid-turn-fork-{}", uuid::Uuid::new_v4().simple());
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

    assert_eq!(
        bounded_turn(&handle, PARENT, MID_TURN_PROMPT).await,
        FORK_DONE
    );
    let history = session_history(&router, &parent_session).await;
    let started = recorded_fork_off_start(&history).expect("fork_off result");
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    wait_for_single_record(&router, &parent_session, &job_id).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let requests = requests.lock().unwrap().clone();
    let wake_turns = requests
        .iter()
        .filter(|(_, rendered, wake)| *wake && rendered.contains(&job_id))
        .count();
    assert_eq!(
        wake_turns, 1,
        "one follow-up turn sees the completion: {requests:#?}"
    );
    wait_for_single_record(&router, &parent_session, &job_id).await;
    let _ = mob_state.mob_destroy(&mob_id).await;
}

// ===========================================================================
// External fork admission: a source that owes a turn is refused at once
// ===========================================================================
//
// An external fork (`MobHandle::fork_member`) is a readiness check on the
// source. The source is busy from the moment the runtime admits its input,
// not only once a provider call is in flight: the runtime loop dequeues,
// stages, materializes or revives the member's session first. A fork issued
// in that window used to queue behind the whole turn on the source's turn
// boundary and then branch whatever the turn left behind. It must answer at
// once instead, and a fork of an idle member must still be accepted.

const ADMITTED_PROMPT: &str = "ADMITTED-6M hold before the reply";
const ADMITTED_REPLY: &str = "ADMITTED-ACK";
const IDLE_PROMPT: &str = "IDLE-2P say hello";
/// How long an external fork may take to answer. Generous for a loaded CI
/// host and far below the held turns, which never end on their own.
const FORK_ANSWER_BOUND: Duration = Duration::from_secs(5);

/// Holds every provider call whose last user message carries
/// [`ADMITTED_PROMPT`] while closed, and counts the calls that reached it.
#[derive(Clone)]
struct ProviderGate {
    open: tokio::sync::watch::Sender<bool>,
    entered: Arc<std::sync::atomic::AtomicUsize>,
}

impl ProviderGate {
    fn new() -> Self {
        Self {
            open: tokio::sync::watch::channel(true).0,
            entered: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn entered(&self) -> usize {
        self.entered.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// [`ForkOffScript`] plus the [`ProviderGate`] for admitted turns.
struct AdmissionScript {
    script: ForkOffScript,
    gate: ProviderGate,
}

#[async_trait::async_trait]
impl LlmClient for AdmissionScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        if !last_user_text(request).contains(ADMITTED_PROMPT) {
            let events = self.script.events(request);
            return Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
        }
        let events = vec![
            LlmEvent::TextDelta {
                delta: ADMITTED_REPLY.to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ];
        let gate = self.gate.clone();
        let released = async move {
            gate.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut open = gate.open.subscribe();
            while !*open.borrow_and_update() {
                if open.changed().await.is_err() {
                    break;
                }
            }
            events
        };
        Box::pin(futures::StreamExt::flat_map(
            futures::stream::once(released),
            |events| futures::stream::iter(events.into_iter().map(Ok)),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct AdmissionLane {
    router: MethodRouter,
    mob_state: Arc<MobMcpState>,
    mob_id: meerkat_mob::MobId,
    handle: MobHandle,
    parent_session: SessionId,
    gate: ProviderGate,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

/// The Scenario 96 stack with the scripted provider and a spawned forker.
async fn admission_lane(root: &std::path::Path) -> AdmissionLane {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let gate = ProviderGate::new();
    let client: Arc<dyn LlmClient> = Arc::new(AdmissionScript {
        script: ForkOffScript {
            requests: Arc::clone(&requests),
        },
        gate: gate.clone(),
    });
    let (router, mob_state) = make_stack(root, client).await;
    let mob_id = format!("fork-admission-{}", uuid::Uuid::new_v4().simple());
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
    AdmissionLane {
        router,
        mob_state,
        mob_id,
        handle,
        parent_session,
        gate,
        requests,
    }
}

impl AdmissionLane {
    /// Admit an [`ADMITTED_PROMPT`] turn for the forker and return as soon as
    /// the runtime accepted it.
    async fn admit_turn(&self) -> (meerkat_mob::WorkTurnHandle, BoundedResultSpec) {
        let spec = BoundedResultSpec::new("admitted", 16 * 1024).expect("bounded result spec");
        let work = tokio::time::timeout(
            WAIT,
            self.handle.start_work_for_identity_bounded(
                AgentIdentity::from(PARENT),
                WorkSpec::new(
                    ContentInput::Text(ADMITTED_PROMPT.to_string()),
                    WorkOrigin::Internal,
                ),
                HandlingMode::Queue,
                spec.clone(),
            ),
        )
        .await
        .expect("the runtime must admit the turn without waiting for it to start")
        .expect("admit the forker's turn");
        (work, spec)
    }

    /// Issue an external fork of the forker and return its answer, failing
    /// the test if the fork does not answer within [`FORK_ANSWER_BOUND`].
    async fn external_fork(
        &self,
        fork_identity: &str,
        when: &str,
    ) -> Result<meerkat_mob::ForkMemberResult, meerkat_mob::MobError> {
        tokio::time::timeout(
            FORK_ANSWER_BOUND,
            self.handle.fork_member(
                &AgentIdentity::from(PARENT),
                SpawnMemberSpec::new("keeper", AgentIdentity::from(fork_identity)),
                None,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{when}: the external fork did not answer within {FORK_ANSWER_BOUND:?}; it queued \
                 behind the source's admitted turn instead of refusing it"
            )
        })
    }

    async fn assert_refused_as_busy(&self, fork_identity: &str, when: &str) {
        let attempt = self.external_fork(fork_identity, when).await;
        match &attempt {
            Err(meerkat_mob::MobError::ForkSourceUnavailable { cause, .. }) => assert_eq!(
                *cause,
                meerkat_mob::ForkSourceUnavailableCause::Running,
                "{when}: a source that owes a turn is refused with the Running cause"
            ),
            other => panic!(
                "{when}: an external fork of a source with an admitted turn must be refused as \
                 Running, got {other:?}"
            ),
        }
        assert!(
            self.handle
                .roster()
                .await
                .get_by_identity(&AgentIdentity::from(fork_identity))
                .is_none(),
            "{when}: a refused fork leaves no roster member behind"
        );
    }

    /// An external fork of the idle forker is accepted and branches exactly
    /// the forker's committed transcript, which ends in an answer rather
    /// than an unanswered input.
    async fn assert_idle_fork_accepted(&self, fork_identity: &str, when: &str) {
        let parent = session_history(&self.router, &self.parent_session).await;
        let fork = self
            .external_fork(fork_identity, when)
            .await
            .unwrap_or_else(|error| panic!("{when}: an idle forker must be forkable: {error:?}"));
        let child = session_history(&self.router, &fork.session_id).await;
        let child_rows = history_messages(&child);
        assert_eq!(
            child_rows.len(),
            history_messages(&parent).len(),
            "{when}: the fork branches the forker's whole committed transcript: {child}"
        );
        let last_role = child_rows
            .last()
            .and_then(|row| row["role"].as_str())
            .unwrap_or_default();
        assert_ne!(
            last_role, "user",
            "{when}: the branch must not end in an unanswered input: {child}"
        );
        assert!(
            self.handle
                .roster()
                .await
                .get_by_identity(&AgentIdentity::from(fork_identity))
                .is_some(),
            "{when}: the accepted fork is seated as a roster member"
        );
    }

    /// Run the forker's fork_off turn from its own turn (a self-fork), wait
    /// for the detached child's outcome to reach the forker and for the
    /// forker to settle, then retire the forker's idle executor.
    ///
    /// The retirement goes through the runtime's own owned unregister saga,
    /// exactly what the runtime loop's idle teardown runs. It is performed
    /// explicitly because completion delivery wakes the forker instead of
    /// leaving its executor idle, so whether the runtime retires it on its
    /// own is not something this lane controls. What matters is the state
    /// after it: the member has no live executor, so its next turn goes
    /// through revival first.
    async fn self_fork_then_retire_idle_executor(&self) {
        assert_eq!(
            bounded_turn(&self.handle, PARENT, FORK_PROMPT).await,
            FORK_DONE,
            "fork_off from the forker's own turn must still work"
        );
        let history = session_history(&self.router, &self.parent_session).await;
        let started = recorded_fork_off_start(&history).unwrap_or_else(|| {
            panic!("the forker's transcript holds no fork_off result: {history}")
        });
        assert_eq!(started["status"], "running", "{started}");
        assert_eq!(started["agent_identity"], CHILD, "{started}");
        let job_id = started["job_id"].as_str().expect("job id").to_string();
        let deadline = tokio::time::Instant::now() + WAIT;
        while recorded_completions(
            &session_history(&self.router, &self.parent_session).await,
            &job_id,
        )
        .is_empty()
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the forker never received the fork_off completion"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let runtime = self
            .mob_state
            .session_service()
            .runtime_adapter()
            .expect("runtime adapter");
        // Settled: no admitted input across a few consecutive reads, so any
        // wake turn the completion caused has been answered.
        use meerkat_runtime::SessionServiceRuntimeExt as _;
        let deadline = tokio::time::Instant::now() + WAIT;
        let mut quiet_reads = 0;
        while quiet_reads < 5 {
            let settled = match runtime.list_active_inputs(&self.parent_session).await {
                Ok(active) => active.is_empty(),
                Err(_) => !runtime.contains_session(&self.parent_session).await,
            };
            quiet_reads = if settled { quiet_reads + 1 } else { 0 };
            assert!(
                tokio::time::Instant::now() < deadline,
                "the forker never settled after the fork_off completion"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if runtime.contains_session(&self.parent_session).await {
            runtime
                .unregister_session(&self.parent_session)
                .await
                .expect("retire the forker's idle executor");
        }
        assert!(
            !runtime.contains_session(&self.parent_session).await,
            "the forker's idle executor is retired"
        );
    }

    fn provider_saw(&self, prompt: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(last_user, _)| last_user.contains(prompt))
    }
}

/// An external fork issued right after the runtime admitted the source's
/// turn, before the turn can reach the provider, is refused at once.
///
/// The test holds the forker's turn-finalization boundary, which is what the
/// runtime loop needs to dequeue and start the turn, so the admitted input
/// provably waits before the provider for the whole fork attempt.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_external_fork_is_refused_at_once_while_an_admitted_turn_waits_to_start() {
    let temp = tempfile::TempDir::new().unwrap();
    let lane = admission_lane(temp.path()).await;
    assert_eq!(
        bounded_turn(&lane.handle, PARENT, IDLE_PROMPT).await,
        FOLLOW_UP_REPLY
    );
    assert!(
        lane.provider_saw(IDLE_PROMPT),
        "the forker's turns must run on the scripted provider"
    );

    let boundary = lane
        .mob_state
        .session_service()
        .acquire_runtime_turn_finalization_guard(&lane.parent_session)
        .await
        .expect("hold the forker's turn boundary");
    let (work, spec) = lane.admit_turn().await;
    lane.assert_refused_as_busy("fork-while-admitted", "an admitted turn not yet started")
        .await;
    assert_eq!(
        lane.gate.entered(),
        0,
        "the admitted turn must not have reached the provider during the fork attempt"
    );

    drop(boundary);
    let reply = tokio::time::timeout(WAIT, work.wait_bounded(spec))
        .await
        .expect("the admitted turn runs once its boundary is free")
        .expect("the admitted turn completes");
    assert_eq!(reply.result().result().text(), ADMITTED_REPLY);

    let _ = lane.mob_state.mob_destroy(&lane.mob_id).await;
}

/// The same refusal during revival: after the forker's own fork_off (a
/// self-fork from inside its turn, which keeps working) completes and its
/// idle executor is retired, the forker's next turn goes through revival
/// first, and a fork issued as soon as that turn is admitted is refused at
/// once.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_external_fork_is_refused_at_once_while_a_revived_turn_is_admitted() {
    let temp = tempfile::TempDir::new().unwrap();
    let lane = admission_lane(temp.path()).await;
    lane.self_fork_then_retire_idle_executor().await;
    assert!(
        lane.handle
            .roster()
            .await
            .get_by_identity(&AgentIdentity::from(CHILD))
            .is_some(),
        "the self-fork child is seated"
    );

    lane.gate.open.send_replace(false);
    let (work, spec) = lane.admit_turn().await;
    lane.assert_refused_as_busy("fork-while-reviving", "a turn admitted through revival")
        .await;

    lane.gate.open.send_replace(true);
    let reply = tokio::time::timeout(WAIT, work.wait_bounded(spec))
        .await
        .expect("the revived turn completes once released")
        .expect("the revived turn completes");
    assert_eq!(reply.result().result().text(), ADMITTED_REPLY);

    let _ = lane.mob_state.mob_destroy(&lane.mob_id).await;
}

/// An external fork of an idle member is accepted: right after its turn was
/// answered, and again after the runtime retired its idle executor.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_external_fork_of_an_idle_member_is_accepted() {
    let temp = tempfile::TempDir::new().unwrap();
    let lane = admission_lane(temp.path()).await;
    assert_eq!(
        bounded_turn(&lane.handle, PARENT, IDLE_PROMPT).await,
        FOLLOW_UP_REPLY
    );
    lane.assert_idle_fork_accepted("fork-of-idle", "right after an answered turn")
        .await;

    lane.self_fork_then_retire_idle_executor().await;
    lane.assert_idle_fork_accepted(
        "fork-of-retired-executor",
        "after the runtime retired the idle executor",
    )
    .await;

    let _ = lane.mob_state.mob_destroy(&lane.mob_id).await;
}

const CHECK_PROMPT: &str = "CHECK-CHILD-7Q check on your child in mob=";
const CHECK_DONE: &str = "CHECK_DONE";

/// Like [`ForkOffScript`], but the child's turn is held open until released.
struct GatedChildScript {
    inner: ForkOffScript,
    release: Arc<tokio::sync::Notify>,
    child_started: Arc<std::sync::atomic::AtomicBool>,
}

impl GatedChildScript {
    /// The forker's check turn: one `mob_check_member` call on its child,
    /// then done.
    fn check_events(request: &LlmRequest, mob_id: &str) -> Vec<LlmEvent> {
        if matches!(request.messages.last(), Some(Message::ToolResults { .. })) {
            return scripted_text(&request.model, CHECK_DONE);
        }
        scripted_tool_call(
            &request.model,
            "toolu_check_member",
            "mob_check_member",
            json!({"mob_id": mob_id, "member_id": CHILD}),
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for GatedChildScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let last_user = last_user_text(request);
        if let Some(mob_id) = last_user.strip_prefix(CHECK_PROMPT) {
            let events = Self::check_events(request, mob_id);
            return Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
        }
        let events = self.inner.events(request);
        let is_child = last_user.contains(CHILD_TASK);
        let release = Arc::clone(&self.release);
        let child_started = Arc::clone(&self.child_started);
        Box::pin(futures::StreamExt::flat_map(
            futures::stream::once(async move {
                if is_child {
                    child_started.store(true, std::sync::atomic::Ordering::SeqCst);
                    release.notified().await;
                }
                events
            }),
            |events| futures::stream::iter(events.into_iter().map(Ok)),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Observing and controlling a running child is steer, not queue:
/// member_status answers at once while the child's turn is open, with a typed
/// open run state, the owner's member list and ownership check answer too,
/// and force-cancel is accepted while the turn runs and applies to that turn
/// at its next boundary (it does not wait for the turn, and the turn does
/// not complete normally). The cancelled child's outcome reaches the forker
/// once.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_forker_observes_a_running_child_without_waiting_for_its_turn() {
    let temp = tempfile::TempDir::new().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let child_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let client: Arc<dyn LlmClient> = Arc::new(GatedChildScript {
        inner: ForkOffScript {
            requests: Arc::new(Mutex::new(Vec::new())),
        },
        release: Arc::clone(&release),
        child_started: Arc::clone(&child_started),
    });
    let (router, mob_state) = make_stack(temp.path(), client).await;
    let mob_id = format!("observe-running-{}", uuid::Uuid::new_v4().simple());
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
    assert_eq!(bounded_turn(&handle, PARENT, FORK_PROMPT).await, FORK_DONE);
    let history = session_history(&router, &parent_session).await;
    let started = recorded_fork_off_start(&history)
        .unwrap_or_else(|| panic!("the forker's transcript holds no fork_off result: {history}"));
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    let deadline = tokio::time::Instant::now() + WAIT;
    while !child_started.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let observed = tokio::time::timeout(
        Duration::from_secs(5),
        handle.member_status(&AgentIdentity::from(CHILD)),
    )
    .await
    .expect("member_status must not wait for the running child's turn")
    .expect("member_status succeeds for a busy child");
    assert_eq!(
        observed
            .progress
            .as_ref()
            .map(|progress| progress.run_state),
        Some(meerkat_mob::MemberRunState::RunOpen),
        "the status says the child is mid-turn"
    );

    // The forker's own mob_check_member call answers within its turn, with
    // the typed run state and the plain note.
    assert_eq!(
        bounded_turn(&handle, PARENT, &format!("{CHECK_PROMPT}{mob_id}")).await,
        CHECK_DONE
    );
    let history = session_history(&router, &parent_session).await;
    let checked = recorded_tool_result(&history, "toolu_check_member")
        .unwrap_or_else(|| panic!("no mob_check_member result: {history}"));
    assert_eq!(checked["progress"]["run_state"], "run_open", "{checked}");
    assert!(
        checked["note"]
            .as_str()
            .is_some_and(|note| note.contains("still running")),
        "{checked}"
    );

    let members = tokio::time::timeout(
        Duration::from_secs(5),
        handle.list_members_including_retiring(),
    )
    .await
    .expect("the member list must not wait for the running child's turn");
    assert!(
        members
            .iter()
            .any(|member| member.agent_identity.as_str() == CHILD),
        "the running child is listed"
    );
    let admission = tokio::time::timeout(
        Duration::from_secs(5),
        handle.resolve_owned_member_admission(
            false,
            Some(&AgentIdentity::from(PARENT)),
            &AgentIdentity::from(CHILD),
        ),
    )
    .await
    .expect("the ownership check must not wait for the running child's turn")
    .expect("ownership check");
    assert!(
        matches!(admission, meerkat_mob::CurrentMobAdmission::Allowed),
        "the forker owns its running child"
    );

    // The child's provider call is still gated while force-cancel runs.
    tokio::time::timeout(
        Duration::from_secs(5),
        handle.force_cancel_member(AgentIdentity::from(CHILD)),
    )
    .await
    .expect("force-cancel must not wait for the running child's turn")
    .expect("force-cancel a running child");
    release.notify_waiters();
    wait_for_single_record(&router, &parent_session, &job_id).await;
    let history = session_history(&router, &parent_session).await;
    let completion = recorded_completions(&history, &job_id)
        .into_iter()
        .next()
        .expect("one completion");
    let body = completion["body"].as_str().expect("completion body");
    assert!(
        body.contains("(failed)"),
        "the cancel applied to the running turn: {body}"
    );

    let _ = mob_state.mob_destroy(&mob_id).await;
}

// ===========================================================================
// A top-level RPC session convenes a detached council
// ===========================================================================

const CONVENE_PROMPT: &str = "CONVENE-4Q hold a council in mob=";
const CONVENE_DONE: &str = "CONVENE_DONE";
const COUNCIL_SUMMARY: &str = "COUNCIL-SUMMARY-8R the council agreed";
const CONVENER_FOLLOW_UP: &str = "CONVENER-FOLLOW-UP-2 what did the council decide?";

fn all_user_text(request: &LlmRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The council role a participant fork was seated with, from its prompt.
fn council_role(request: &LlmRequest) -> Option<String> {
    let haystack = all_user_text(request);
    let marker = "You are '";
    let start = haystack.rfind(marker)? + marker.len();
    let rest = &haystack[start..];
    Some(rest[..rest.find('\'')?].to_string())
}

fn council_call(model: &str, mob_id: &str) -> Vec<LlmEvent> {
    scripted_tool_call(
        model,
        "toolu_council",
        "council",
        json!({
            "topic": "Should we ship the migration this week?",
            "participants": [
                {"mob_id": mob_id, "member_id": "alice", "role": "analyst"},
                {"mob_id": mob_id, "member_id": "bob", "role": "critic"},
            ],
            "max_rounds": 1,
            "timeout_seconds": 120,
        }),
    )
}

/// The top-level session calls `council` once; the participants' discussion
/// turns wait on `release`; the merge answers with the summary; every other
/// turn acknowledges. Requests are recorded as (last user text, rendered).
struct ConvenerScript {
    requests: Arc<Mutex<Vec<(String, String)>>>,
    release: Arc<tokio::sync::Notify>,
    released: Arc<std::sync::atomic::AtomicBool>,
    participants_started: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl LlmClient for ConvenerScript {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let last_user = last_user_text(request);
        self.requests
            .lock()
            .unwrap()
            .push((last_user.clone(), format!("{:?}", request.messages)));
        let ready = |events: Vec<LlmEvent>| -> LlmStream<'a> {
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        };
        if let Some(mob_id) = last_user.strip_prefix(CONVENE_PROMPT) {
            // The session builds its own source mob (which gives it manage
            // scope over it), seats two participants, then convenes.
            let convene_at = request
                .messages
                .iter()
                .rposition(|message| matches!(message, Message::User(_)))
                .unwrap_or(0);
            let step = request.messages[convene_at..]
                .iter()
                .filter(|message| matches!(message, Message::ToolResults { .. }))
                .count();
            let spawn = |member: &str| {
                json!({
                    "mob_id": mob_id,
                    "profile": "keeper",
                    "member_id": member,
                    "runtime_mode": "turn_driven",
                })
            };
            return ready(match step {
                0 => scripted_tool_call(
                    &request.model,
                    "toolu_mob_create",
                    "mob_create",
                    json!({"definition": serde_json::to_value(mob_definition(mob_id)).unwrap()}),
                ),
                1 => scripted_tool_call(
                    &request.model,
                    "toolu_spawn_alice",
                    "mob_spawn_member",
                    spawn("alice"),
                ),
                2 => scripted_tool_call(
                    &request.model,
                    "toolu_spawn_bob",
                    "mob_spawn_member",
                    spawn("bob"),
                ),
                3 => council_call(&request.model, mob_id),
                _ => scripted_text(&request.model, CONVENE_DONE),
            });
        }
        if all_user_text(request).contains("bounded plain-text summary") {
            return ready(scripted_text(&request.model, COUNCIL_SUMMARY));
        }
        if let Some(role) = council_role(request) {
            let events = scripted_text(&request.model, &format!("position from {role}"));
            let release = Arc::clone(&self.release);
            let released = Arc::clone(&self.released);
            let started = Arc::clone(&self.participants_started);
            return Box::pin(futures::StreamExt::flat_map(
                futures::stream::once(async move {
                    started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    while !released.load(std::sync::atomic::Ordering::SeqCst) {
                        let notified = release.notified();
                        if released.load(std::sync::atomic::Ordering::SeqCst) {
                            break;
                        }
                        notified.await;
                    }
                    events
                }),
                |events| futures::stream::iter(events.into_iter().map(Ok)),
            ));
        }
        ready(scripted_text(&request.model, FOLLOW_UP_REPLY))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn rpc_call(router: &MethodRouter, method: &str, params: Value) -> Value {
    let response = router
        .dispatch(RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: method.to_string(),
            params: Some(serde_json::value::RawValue::from_string(params.to_string()).unwrap()),
        })
        .await
        .unwrap_or_else(|| panic!("{method} dispatch"));
    assert!(
        response.error.is_none(),
        "{method} error: {:?}",
        response.error
    );
    serde_json::from_str(response.result.as_ref().expect("result").get()).expect("result JSON")
}

/// A top-level RPC session (not a mob member) convenes a detached council,
/// and the runtime retires its idle executor while the council runs. The
/// RPC host's owner hook makes the session live again: the council's result
/// is recorded once, wakes the session for one turn, and its next turn sees
/// the summary. Without the hook it would arrive only after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_fast_top_level_rpc_convener_is_revived_for_its_council_result() {
    let temp = tempfile::TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let participants_started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let client: Arc<dyn LlmClient> = Arc::new(ConvenerScript {
        requests: Arc::clone(&requests),
        release: Arc::clone(&release),
        released: Arc::clone(&released),
        participants_started: Arc::clone(&participants_started),
    });
    let open_gate = {
        let release = Arc::clone(&release);
        let released = Arc::clone(&released);
        move || {
            released.store(true, std::sync::atomic::Ordering::SeqCst);
            release.notify_waiters();
        }
    };

    // The production RPC composition, which installs the owner hook.
    let root = temp.path();
    let project_root = root.join("project-root");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::write(project_root.join("AGENTS.md"), "# Detached council\n").unwrap();
    let runtime_root = root.join("runtime-root");
    let factory = AgentFactory::new(runtime_root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(runtime_root.clone())
        .project_root(project_root.clone())
        .context_root(project_root)
        .builtins(false)
        .shell(false)
        .comms(true)
        .mob(true);
    let config = Config::default();
    let persistence = meerkat::PersistenceBundle::new(
        Arc::new(meerkat::MemoryStore::new()),
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
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
    let runtime = Arc::new(runtime);
    let mob_state = meerkat_rpc::router::compose_rpc_mob_state(&runtime, &config_store, None);
    assert!(mob_state.detached_owner_host().is_some());
    // Councils seat on the RPC host: its session service exposes the
    // persistent service as the forked-participant source runtime.
    assert!(
        runtime
            .session_service()
            .forked_participant_source_runtime()
            .is_some()
    );
    *runtime.builder_mob_tools_slot.write().unwrap() = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));
    let (notif_tx, _notif_rx) = mpsc::channel(256);
    let router = MethodRouter::new_with_mob_state(
        Arc::clone(&runtime),
        config_store,
        NotificationSink::new(notif_tx),
        Arc::clone(&mob_state),
    );

    // A top-level session with mob tools; it builds the source mob itself.
    let mob_id = format!("convened-{}", uuid::Uuid::new_v4().simple());
    let created = rpc_call(
        &router,
        "session/create",
        json!({
            "model": "claude-sonnet-4-5",
            "prompt": "CONVENER-HELLO warm up",
            "enable_mob": true,
        }),
    )
    .await;
    let session = SessionId::parse(created["session_id"].as_str().expect("session id")).unwrap();

    let turn = rpc_call(
        &router,
        "turn/start",
        json!({"session_id": session.to_string(), "prompt": format!("{CONVENE_PROMPT}{mob_id}")}),
    )
    .await;
    assert!(turn.to_string().contains(CONVENE_DONE), "{turn}");
    let history = session_history(&router, &session).await;
    let started = recorded_tool_result(&history, "toolu_council")
        .unwrap_or_else(|| panic!("no council result: {history}"));
    assert_eq!(started["status"], "running", "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();

    // The council is running; the runtime retires the idle session.
    let deadline = tokio::time::Instant::now() + WAIT;
    while participants_started.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "no participant started: {}",
                session_history(&router, &session).await
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    runtime
        .runtime_adapter()
        .unregister_session(&session)
        .await
        .expect("the runtime retires the session's idle executor");
    assert!(!runtime.runtime_adapter().contains_session(&session).await);
    open_gate();

    wait_for_single_record(&router, &session, &job_id).await;
    // The record's header; the started note names the same job but not its
    // status.
    let marker = format!("Background council job {job_id} finished (");
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let woken = requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(last_user, rendered)| {
                rendered.contains(&marker) && !last_user.contains(CONVENER_FOLLOW_UP)
            })
            .count();
        if woken >= 1 {
            assert_eq!(woken, 1, "the convener is woken for one turn");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the revived convener was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let follow_up = rpc_call(
        &router,
        "turn/start",
        json!({"session_id": session.to_string(), "prompt": CONVENER_FOLLOW_UP}),
    )
    .await;
    assert!(
        follow_up.to_string().contains(FOLLOW_UP_REPLY),
        "{follow_up}"
    );
    let follow_up_request = requests
        .lock()
        .unwrap()
        .iter()
        .find(|(last_user, _)| last_user.contains(CONVENER_FOLLOW_UP))
        .map(|(_, rendered)| rendered.clone())
        .expect("the follow-up request");
    assert!(
        follow_up_request.contains(&marker) && follow_up_request.contains("COUNCIL-SUMMARY-8R"),
        "{follow_up_request}"
    );
    wait_for_single_record(&router, &session, &job_id).await;

    open_gate();
    let _ = mob_state
        .mob_destroy(&meerkat_mob::MobId::from(mob_id))
        .await;
}
