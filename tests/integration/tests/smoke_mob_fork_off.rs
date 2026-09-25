#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Turbo S scenario 96: mob `fork_off` live vertical.
//!
//! One durable mob member builds a large cached prefix, then forks itself
//! three ways: through the real agent-facing `fork_off` tool from inside its
//! own running turn (the branch is cut at its last committed boundary), as an
//! external fork attempted while its turn is running (must be refused with
//! the typed `Running` cause), and at an explicit earlier prefix through
//! `MobHandle::fork_member_then_run_bounded`. Both children are retired and
//! the parent answers again. Every assertion reads authoritative state (mob
//! roster, persisted sessions, per-call provider usage rows, the tool result
//! and the detached child's completion recorded in the parent transcript),
//! never model narration.
//!
//! The child usage counters are the measured answer to "does a fork re-bill
//! the whole parent prefix": the child's first request must report
//! `cache_read_tokens > 0` against the prefix the parent already paid for.
//!
//! Run with:
//!   ANTHROPIC_API_KEY=... cargo test -p meerkat-integration-tests \
//!     --test smoke_mob_fork_off --features integration-real-tests \
//!     -- --ignored e2e_smoke_s96_mob_fork_off_vertical --nocapture

use meerkat::{AgentFactory, Config};
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use meerkat_core::{ConfigRuntime, ContentInput, HandlingMode, MemoryConfigStore, Usage};
use meerkat_mob::{
    AgentIdentity, AttributedEvent, BoundedResultSpec, MobDefinition, MobEventRouterConfig,
    MobHandle, MobId, SpawnMemberSpec, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_rpc::protocol::{RpcId, RpcRequest};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::session_runtime::SessionRuntime;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep};

const VAULT_PHRASE: &str = "quartz-heron-nineteen";
const CODE_WORD: &str = "TANGERINE-88";
const LEDGER_ENTRIES: usize = 120;
const PARENT: &str = "ledger-keeper";
const FORK_ONE: &str = "fork-tool-child";
const FORK_TWO: &str = "fork-prefix-child";

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

struct SmokePaths {
    user_config_root: PathBuf,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: PathBuf,
}

impl SmokePaths {
    fn new(root: &Path) -> Self {
        Self {
            user_config_root: root.join("user-config"),
            runtime_root: root.join("runtime-root"),
            project_root: root.join("project-root"),
            context_root: root.join("context-root"),
        }
    }
}

fn materialize_project_context(paths: &SmokePaths) {
    for root in [&paths.project_root, &paths.context_root] {
        std::fs::create_dir_all(root).expect("create smoke project/context root");
        std::fs::write(
            root.join("AGENTS.md"),
            "# Fork Vertical Smoke\n\nAnswer concisely and follow tool instructions exactly.\n",
        )
        .expect("write smoke AGENTS.md");
    }
}

fn smoke_factory(paths: &SmokePaths) -> AgentFactory {
    materialize_project_context(paths);
    AgentFactory::new(paths.runtime_root.join("factory-store"))
        .user_config_root(paths.user_config_root.clone())
        .runtime_root(paths.runtime_root.clone())
        .project_root(paths.project_root.clone())
        .context_root(paths.context_root.clone())
        .builtins(false)
        .shell(false)
        .comms(true)
        .mob(true)
}

/// Full in-process RPC stack with the agent mob tool surface wired, so mob
/// members receive `fork_off` from the same `MobMcpState` the test inspects.
async fn make_smoke_stack(paths: &SmokePaths) -> (MethodRouter, Arc<MobMcpState>) {
    let factory = smoke_factory(paths);
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
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        paths.runtime_root.join("config_state.json"),
    )));
    let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
        runtime.session_service(),
        Some(runtime.runtime_adapter()),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    *runtime.builder_mob_tools_slot.write().unwrap() = Some(Arc::new(
        meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));
    let runtime = Arc::new(runtime);
    let (notif_tx, _notif_rx) = mpsc::channel(256);
    let sink = NotificationSink::new(notif_tx);
    let router =
        MethodRouter::new_with_mob_state(runtime, config_store, sink, Arc::clone(&mob_state));
    (router, mob_state)
}

fn rpc_request(method: &str, params: impl Serialize) -> RpcRequest {
    let params_raw =
        serde_json::value::RawValue::from_string(serde_json::to_string(&params).unwrap()).unwrap();
    RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(RpcId::Num(1)),
        method: method.to_string(),
        params: Some(params_raw),
    }
}

async fn session_history(router: &MethodRouter, session_id: &SessionId) -> Value {
    let response = router
        .dispatch(rpc_request(
            "session/history",
            json!({ "session_id": session_id.to_string(), "limit": 200 }),
        ))
        .await
        .expect("session/history dispatch");
    assert!(
        response.error.is_none(),
        "session/history error: {:?}",
        response.error
    );
    let raw = response.result.as_ref().expect("session/history result");
    serde_json::from_str(raw.get()).expect("session/history result JSON")
}

/// Every string payload carried by a `tool_results` message, whether the
/// wire content is a legacy string or a block array.
fn tool_result_texts(message: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    let Some(results) = message.get("results").and_then(Value::as_array) else {
        return texts;
    };
    for result in results {
        match result.get("content") {
            Some(Value::String(text)) => texts.push(text.clone()),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        texts.push(text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    texts
}

/// Structured (non-text) block payloads carried by a `tool_results` message.
fn tool_result_json_blocks(message: &Value) -> Vec<&Value> {
    let mut payloads = Vec::new();
    let Some(results) = message.get("results").and_then(Value::as_array) else {
        return payloads;
    };
    for result in results {
        if let Some(blocks) = result.get("content").and_then(Value::as_array) {
            for block in blocks {
                if let Some(data) = block.get("data").filter(|data| data.is_object()) {
                    payloads.push(data);
                }
            }
        }
    }
    payloads
}

/// The `fork_off` tool result the parent model received, parsed from the
/// parent's persisted transcript. The tool encodes its result as a structured
/// JSON block; a legacy text encoding is accepted too. Errors surface as
/// `Err(text)` so a refused fork fails the scenario with the tool's own
/// message.
fn recorded_fork_off_result(history: &Value) -> Option<Result<Value, String>> {
    for message in history_messages(history) {
        for data in tool_result_json_blocks(message) {
            if data.get("fork_session_id").is_some() {
                return Some(Ok(data.clone()));
            }
        }
        for text in tool_result_texts(message) {
            if text.contains("fork_session_id") {
                return Some(
                    serde_json::from_str(&text)
                        .map_err(|error| format!("fork_off result is not JSON ({error}): {text}")),
                );
            }
            if text.contains("fork_off") && text.contains("failed") {
                return Some(Err(text));
            }
        }
    }
    None
}

/// The detached `fork_off` completion recorded in the forker's own
/// transcript for `job_id`: the durable `BackgroundJob` system notice whose
/// block is `persisted` for this job. Returned as
/// `{"job_id", "status", "outcome"}`, where `outcome` is the typed fork_off
/// completion carried in the block's detail. The record must exist at most
/// once.
fn recorded_fork_off_completion(history: &Value, job_id: &str) -> Option<Value> {
    let blocks: Vec<&Value> = history_messages(history)
        .iter()
        .filter(|message| {
            message["role"].as_str() == Some("system_notice")
                && message["kind"].as_str() == Some("background_job")
        })
        .filter_map(|message| {
            message["blocks"].as_array()?.iter().find(|block| {
                block["job_id"].as_str() == Some(job_id)
                    && block["persisted"].as_bool() == Some(true)
            })
        })
        .collect();
    assert!(
        blocks.len() <= 1,
        "the fork_off completion must be recorded once, found {}: {history}",
        blocks.len()
    );
    let block = blocks.first()?;
    let detail = block["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("fork_off completion record without detail: {block}"));
    let outcome: Value = serde_json::from_str(detail).unwrap_or_else(|error| {
        panic!("fork_off completion detail is not JSON ({error}): {detail}")
    });
    Some(json!({"job_id": job_id, "status": block["status"], "outcome": outcome}))
}

/// One short line per transcript message (role and a prefix of its JSON),
/// for failure diagnostics.
fn history_summary(history: &Value) -> Vec<String> {
    history_messages(history)
        .iter()
        .map(|message| {
            let role = message["role"].as_str().unwrap_or("?");
            let body: String = message.to_string().chars().take(160).collect();
            format!("{role}: {body}")
        })
        .collect()
}

fn history_messages(history: &Value) -> &Vec<Value> {
    history
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("session/history without messages: {history}"))
}

fn synthetic_ledger() -> String {
    let mut ledger = String::with_capacity(LEDGER_ENTRIES * 96);
    ledger.push_str(
        "OPERATIONS LEDGER (confidential). Read it once and keep it in mind for later questions.\n\n",
    );
    for index in 0..LEDGER_ENTRIES {
        let account = 4000 + (index * 37) % 5000;
        let amount = 100 + (index * 53) % 9000;
        let region = ["north", "east", "south", "west", "central"][index % 5];
        let status = ["settled", "pending", "disputed", "reversed"][index % 4];
        ledger.push_str(&format!(
            "Entry {index:04}: account {account} moved {amount} units via the {region} corridor; status {status}; reviewer initials {}{}.\n",
            char::from(b'A' + (index % 26) as u8),
            char::from(b'A' + ((index * 7) % 26) as u8),
        ));
        if index == LEDGER_ENTRIES / 4 {
            ledger.push_str(&format!(
                "Entry {index:04}-NOTE: the vault phrase for this ledger is \"{VAULT_PHRASE}\".\n"
            ));
        }
        if index == LEDGER_ENTRIES * 3 / 4 {
            ledger.push_str(&format!(
                "Entry {index:04}-NOTE: the code word for this ledger is \"{CODE_WORD}\".\n"
            ));
        }
    }
    ledger.push_str(
        "\nEND OF LEDGER. Acknowledge with exactly the text LEDGER_LOADED and nothing else.\n",
    );
    ledger
}

fn mob_definition(model: &str) -> MobDefinition {
    serde_json::from_value(json!({
        "id": "s96-fork-off-vertical",
        "profiles": {
            "keeper": {
                "model": model,
                "tools": { "comms": true, "mob": true },
                "peer_description": "Ledger keeper for the fork_off live vertical",
                "runtime_mode": "turn_driven"
            }
        },
        "wiring": { "auto_wire_orchestrator": false, "role_wiring": [] }
    }))
    .expect("scenario 96 mob definition")
}

async fn bounded_turn(
    handle: &MobHandle,
    identity: &str,
    text: impl Into<String>,
    label: &str,
) -> meerkat_mob::BoundedTurnResult {
    let spec = BoundedResultSpec::new(label, 16 * 1024).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(identity),
            WorkSpec::new(ContentInput::Text(text.into()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("start bounded work for {identity}: {error}"));
    let result = match tokio::time::timeout(Duration::from_secs(240), work.wait_bounded(spec)).await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => panic!("bounded turn for {identity} ({label}) failed: {error:?}"),
        Err(_) => panic!("bounded turn for {identity} ({label}) timed out"),
    };
    result.result().clone()
}

/// Collected mob-bus events. Per-call cache counters live only on
/// `turn_completed` rows; every session total deliberately blanks them.
type EventLog = Arc<Mutex<Vec<AttributedEvent>>>;

async fn tap_mob_events(handle: &MobHandle) -> (EventLog, tokio::task::JoinHandle<()>) {
    let mut bus = handle
        .subscribe_mob_events_with_config(MobEventRouterConfig {
            poll_interval: Duration::from_millis(100),
            channel_capacity: 4096,
        })
        .await
        .expect("subscribe mob event bus");
    let log: EventLog = Arc::default();
    let sink = Arc::clone(&log);
    let task = tokio::spawn(async move {
        while let Some(event) = bus.event_rx.recv().await {
            sink.lock().unwrap().push(event);
        }
    });
    (log, task)
}

/// Per-call provider usage rows for one member, in emission order.
async fn turn_usages(log: &EventLog, identity: &str) -> Vec<Usage> {
    // Let the bus flush the tail of the turn that just completed.
    sleep(Duration::from_millis(750)).await;
    log.lock()
        .unwrap()
        .iter()
        .filter(|event| event.source.identity.as_str() == identity)
        .filter_map(|event| match &event.envelope.payload {
            AgentEvent::TurnCompleted {
                usage: Some(usage), ..
            } => Some(usage.as_usage().clone()),
            _ => None,
        })
        .collect()
}

fn cached_tokens(usage: &Usage) -> (u64, u64) {
    (
        usage.cache_creation_tokens.unwrap_or(0),
        usage.cache_read_tokens.unwrap_or(0),
    )
}

async fn wait_until_retired(handle: &MobHandle, identity: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let live = handle
            .list_members_observation_snapshot()
            .await
            .into_iter()
            .any(|entry| entry.agent_identity.as_str() == identity && !entry.is_final);
        if !live {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {identity} to retire"
        );
        sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "lane:e2e-smoke"]
async fn e2e_smoke_s96_mob_fork_off_vertical() {
    let model = smoke_model();
    if anthropic_api_key().is_none() {
        eprintln!("Skipping scenario 96: RKAT_ANTHROPIC_API_KEY/ANTHROPIC_API_KEY is not set");
        return;
    }
    let started = Instant::now();
    let temp = TempDir::new().unwrap();
    let paths = SmokePaths::new(temp.path());
    let (router, mob_state) = make_smoke_stack(&paths).await;

    let mob_id: MobId = mob_state
        .mob_create_definition(mob_definition(&model))
        .await
        .expect("create scenario 96 mob");
    let handle = mob_state.handle_for(&mob_id).await.expect("mob handle");
    handle
        .spawn_spec(SpawnMemberSpec::new("keeper", AgentIdentity::from(PARENT)))
        .await
        .expect("spawn ledger keeper");
    let parent_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(PARENT))
        .await
        .expect("ledger keeper session id");
    let (events, bus_task) = tap_mob_events(&handle).await;

    // Phase A: large prefix, provider cache write.
    let phase_a = Instant::now();
    let ack = bounded_turn(&handle, PARENT, synthetic_ledger(), "ledger-ack").await;
    let parent_rows = turn_usages(&events, PARENT).await;
    let first_call = parent_rows
        .first()
        .unwrap_or_else(|| panic!("parent first call must emit a turn_completed usage row"));
    let (a_created, a_read) = cached_tokens(first_call);
    eprintln!(
        "S96 A ledger loaded in {:?}: input={} cache_creation={} cache_read={} session_total_input={} reply={:?}",
        phase_a.elapsed(),
        first_call.input_tokens,
        a_created,
        a_read,
        ack.usage().input_tokens,
        ack.result().text()
    );
    assert!(
        ack.result().text().contains("LEDGER_LOADED"),
        "parent must acknowledge the ledger: {:?}",
        ack.result().text()
    );
    assert!(
        a_created + a_read > 0,
        "the ledger prefix must be provider-cached on the parent's first call (usage {first_call:?})"
    );
    let prefix_after_ledger =
        history_messages(&session_history(&router, &parent_session).await).len();
    assert!(
        prefix_after_ledger >= 2,
        "parent transcript must hold the ledger exchange, got {prefix_after_ledger} messages"
    );

    // Phase B: the parent calls the real `fork_off` tool from inside its own
    // running turn. The durable fork owner admits the caller's active turn and
    // cuts the branch at the parent's last committed boundary (the ledger
    // exchange); the running fork turn itself is not part of the child.
    let phase_b = Instant::now();
    let fork_instruction = format!(
        "Call the fork_off tool exactly once with member_id \"{FORK_ONE}\", task \"Report the \
         vault phrase recorded in the ledger you already hold. Reply with only the phrase, \
         nothing else.\" and expected_output \"Only the phrase.\" After the tool returns, reply \
         with exactly the text FORK_DONE and nothing else."
    );
    let fork_turn = bounded_turn(&handle, PARENT, fork_instruction, "fork-off-turn").await;
    let parent_history = session_history(&router, &parent_session).await;
    let fork_result = match recorded_fork_off_result(&parent_history) {
        Some(Ok(result)) => result,
        Some(Err(text)) => panic!("fork_off tool call failed inside the parent turn: {text}"),
        None => panic!(
            "parent transcript holds no fork_off tool result; reply={:?} history={}",
            fork_turn.result().text(),
            parent_history
        ),
    };
    // fork_off returns once the child's turn is admitted. The child's result
    // must then reach the FORKER itself: its completion is recorded once in
    // the forker's own durable transcript, keyed by the job id fork_off
    // returned, and that recorded outcome is what the forker's later model
    // calls read. Reading the child's session directly would not show that.
    assert_eq!(
        fork_result["status"].as_str(),
        Some("running"),
        "fork_off must return promptly with the running child: {fork_result}"
    );
    let job_id = fork_result["job_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("fork_off must name the background job that reports the child: {fork_result}")
        })
        .to_string();
    let completion = {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let history = session_history(&router, &parent_session).await;
            if let Some(completion) = recorded_fork_off_completion(&history, &job_id) {
                break completion;
            }
            assert!(
                Instant::now() < deadline,
                "the forker's transcript never received the fork_off completion for job \
                 {job_id}: {history}"
            );
            sleep(Duration::from_millis(500)).await;
        }
    };
    assert_eq!(
        completion["outcome"]["status"].as_str(),
        Some("completed"),
        "the forker must receive the child's completed outcome: {completion}"
    );
    assert_eq!(
        completion["outcome"]["agent_identity"].as_str(),
        Some(FORK_ONE),
        "the completion must name the child: {completion}"
    );
    let child_text = completion["outcome"]["bounded_result"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("fork_off completion without bounded_result text: {completion}"))
        .to_string();
    let child_rows = turn_usages(&events, FORK_ONE).await;
    let child_usage = child_rows
        .first()
        .unwrap_or_else(|| panic!("fork child must emit a turn_completed usage row"));
    let (child_created, child_read) = cached_tokens(child_usage);
    let fork_session = SessionId::parse(
        fork_result["fork_session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("fork_off result without fork_session_id: {fork_result}")),
    )
    .expect("fork_session_id must parse");
    eprintln!(
        "S96 B fork_off tool in {:?}: child input={} cache_creation={} cache_read={} inheritance={} reply={:?} parent_reply={:?}",
        phase_b.elapsed(),
        child_usage.input_tokens,
        child_created,
        child_read,
        fork_result["cache_inheritance"],
        child_text,
        fork_turn.result().text()
    );
    assert_eq!(
        fork_result["agent_identity"].as_str(),
        Some(FORK_ONE),
        "fork_off result must name the child: {fork_result}"
    );
    assert_eq!(
        fork_result["mob_id"].as_str(),
        Some(mob_id.as_str()),
        "fork_off result must name the mob: {fork_result}"
    );
    assert_ne!(fork_session, parent_session, "fork must mint a new session");
    assert!(
        child_read > 0,
        "the fork child's first request must read the parent's cached prefix, usage {child_usage:?}"
    );
    assert!(
        child_text.contains(VAULT_PHRASE),
        "fork child must answer from the inherited ledger: {child_text:?}"
    );
    let first_len = history_messages(&session_history(&router, &fork_session).await).len();
    // Ordinarily exactly the ledger exchange plus the child's own exchange. A
    // compaction checkpoint inside the parent's fork turn may advance the
    // committed snapshot, so the branch is bounded below, never above, by the
    // committed prefix.
    assert!(
        first_len >= prefix_after_ledger + 2,
        "a fork from the running turn must carry at least the parent's committed transcript \
         (the ledger exchange) plus its own exchange, got {first_len} rows"
    );
    assert!(
        first_len <= prefix_after_ledger + 2 + 2,
        "a fork from the running turn must not carry the parent's in-flight fork turn beyond a \
         compaction checkpoint boundary, got {first_len} rows"
    );
    assert!(
        handle
            .roster()
            .await
            .get_by_identity(&AgentIdentity::from(FORK_ONE))
            .is_some(),
        "fork child must be a roster member"
    );
    assert!(
        fork_turn.result().text().contains("FORK_DONE"),
        "parent must finish its own turn after the tool call: {:?}",
        fork_turn.result().text()
    );

    // Phase C: the parent keeps working after the fork, and an EXTERNAL fork
    // attempted while the parent's turn is running is still refused with the
    // typed cause: only the running turn itself may branch its committed
    // transcript.
    let phase_c = Instant::now();
    let code_spec = BoundedResultSpec::new("code-word", 16 * 1024).expect("bounded result spec");
    let code_work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(PARENT),
            WorkSpec::new(
                ContentInput::Text(
                    "What is the code word recorded in the ledger? Reply with only the code word."
                        .to_string(),
                ),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            code_spec.clone(),
        )
        .await
        .expect("start code-word turn");
    sleep(Duration::from_millis(300)).await;
    let racing = SpawnMemberSpec::new("keeper", AgentIdentity::from("fork-while-running"));
    let racing_result = handle
        .fork_member(&AgentIdentity::from(PARENT), racing, None)
        .await;
    match &racing_result {
        Err(meerkat_mob::MobError::ForkSourceUnavailable { cause, .. }) => assert_eq!(
            *cause,
            meerkat_mob::ForkSourceUnavailableCause::Running,
            "running-source refusal must carry the Running cause"
        ),
        other => {
            let parent_rows = history_summary(&session_history(&router, &parent_session).await);
            panic!(
                "an external fork of a running source must be refused with a typed cause, got \
                 {other:?}; parent transcript after the attempt: {parent_rows:#?}"
            )
        }
    }
    let code_turn =
        match tokio::time::timeout(Duration::from_secs(240), code_work.wait_bounded(code_spec))
            .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => panic!("code-word turn failed: {error:?}"),
            Err(_) => panic!("code-word turn timed out"),
        };
    let code_text = code_turn.result().result().text().to_string();
    eprintln!(
        "S96 C parent code word in {:?}: last_row={:?} racing_fork=refused(Running) reply={:?}",
        phase_c.elapsed(),
        turn_usages(&events, PARENT)
            .await
            .last()
            .map(|row| (row.input_tokens, cached_tokens(row))),
        code_text
    );
    assert!(
        code_text.contains(CODE_WORD),
        "parent must still answer from its own transcript: {code_text:?}"
    );
    assert!(
        handle
            .roster()
            .await
            .get_by_identity(&AgentIdentity::from("fork-while-running"))
            .is_none(),
        "a refused fork must leave no roster member behind"
    );

    // Phase D: explicit-prefix fork cut before the code-word turn.
    let phase_d = Instant::now();
    let mut second = SpawnMemberSpec::new("keeper", AgentIdentity::from(FORK_TWO));
    second.initial_message = Some(ContentInput::Text(
        "Reply with only two lines. Line 1: the vault phrase from the ledger. Line 2: YES if \
         anyone in this conversation has asked about a code word so far, otherwise NO."
            .to_string(),
    ));
    let second_outcome = handle
        .fork_member_then_run_bounded(
            &AgentIdentity::from(PARENT),
            second,
            Some(prefix_after_ledger),
            "prefix-fork",
            16 * 1024,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
        )
        .await
        .unwrap_or_else(|error| panic!("explicit-prefix fork failed: {error:?}"));
    let second_text = second_outcome.turn.result().result().text().to_string();
    let second_rows = turn_usages(&events, FORK_TWO).await;
    let second_usage = second_rows
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("prefix fork child must emit a turn_completed usage row"));
    let second_history = session_history(&router, &second_outcome.fork.session_id).await;
    let second_len = history_messages(&second_history).len();
    eprintln!(
        "S96 D prefix fork in {:?}: prefix={} child_messages={} input={} cache_read={} inheritance={:?} reply={:?}",
        phase_d.elapsed(),
        prefix_after_ledger,
        second_len,
        second_usage.input_tokens,
        cached_tokens(&second_usage).1,
        second_outcome.fork.cache_inheritance,
        second_text
    );
    assert_eq!(second_outcome.fork.agent_identity.as_str(), FORK_TWO);
    assert!(
        second_text.contains(VAULT_PHRASE),
        "prefix fork must inherit the ledger: {second_text:?}"
    );
    assert!(
        second_text.to_ascii_uppercase().contains("NO") && !second_text.contains(CODE_WORD),
        "prefix fork must not see the later code-word exchange: {second_text:?}"
    );
    assert_eq!(
        second_len,
        prefix_after_ledger + 2,
        "prefix fork transcript must be the requested prefix plus its own exchange"
    );
    assert!(
        cached_tokens(&second_usage).1 > 0,
        "prefix fork first request must read the cached ledger prefix: {second_usage:?}"
    );

    // Phase E: retire both children, parent still answers.
    let phase_e = Instant::now();
    for child in [FORK_ONE, FORK_TWO] {
        handle
            .retire(AgentIdentity::from(child))
            .await
            .unwrap_or_else(|error| panic!("retire {child}: {error}"));
        wait_until_retired(&handle, child).await;
    }
    let alive = bounded_turn(
        &handle,
        PARENT,
        "Reply with exactly the text PARENT_ALIVE and nothing else.",
        "alive",
    )
    .await;
    eprintln!(
        "S96 E retired both children and parent replied in {:?}: {:?}",
        phase_e.elapsed(),
        alive.result().text()
    );
    assert!(
        alive.result().text().contains("PARENT_ALIVE"),
        "parent must survive child retirement: {:?}",
        alive.result().text()
    );
    bus_task.abort();
    eprintln!("S96 PASS total {:?} model={model}", started.elapsed());
}
