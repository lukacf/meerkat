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
/// one follow-up turn that sees it right after its current turn ends. (A
/// non-live session has no mid-turn runtime input drain, so the running
/// turn's own later model calls do not see it.)
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
