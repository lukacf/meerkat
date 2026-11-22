#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A console side chat keeps its own `memory` tool across a gateway restart.
//!
//! With agent memory on, a side chat of an agent is offered a `memory` tool
//! bound to the side chat's own identity. A restart leaves every agent
//! dormant until something wakes it, and the tools an agent's build
//! publishes exist in the new process only once the agent was built there.
//! A side chat does not wait for that: at its first turn after a restart,
//! and when it is forked after a restart, it is offered `memory` while its
//! source agent is still dormant, its call stores a note in its own scope,
//! and its source agent stays dormant. Once the source agent has run, the
//! side chat's `memory` is still its own: each of its verbs acts on the side
//! chat's own notes.
//!
//! Each row boots the gateway's runtime composition on a temporary state
//! directory (persistent stores, the state directory's identity substrate, a
//! roster with one agent, agent memory with the recorder on) with a model
//! that records every request it is sent and calls `memory` once when it is
//! asked to. The side chats are forked, and closed, through the Console's
//! own requests (`meta/fork_member`, `meta/retire_member`).

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, MutableRosterProvider,
};
use meerkat_meta::memory::MemoryAuthor;
use meerkat_meta::{
    AgentMemoryConfig, AgentMemoryProvider, AuthPolicy, BigQueryNaming, ConsolePolicy, MemoryKind,
    MemoryScope, NewMemoryRecord, RuntimeDecisionInputs, RuntimeOpsPolicy, SqliteAgentMemoryStore,
    TrustedOidcRuntimeConfig, UnifiedRuntimeBuilder, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::ServiceExt;

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy.
#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The agent the roster configures: the source of every side chat here.
const SOURCE: &str = "host-a";
/// A turn asked a message that holds this prefix calls `memory` once, with
/// the whole token as the note's title.
const REMEMBER: &str = "REMEMBER-";
/// A turn asked a message that holds this marker calls `memory` once, with
/// the JSON object after the marker as the call's arguments.
const CALL: &str = "CALL-MEMORY ";
const MEMORY_TOOL: &str = "memory";
const TURN_BOUND: Duration = Duration::from_secs(90);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn fresh_mob_id() -> String {
    format!(
        "side-chat-memory-restart-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

/// Turn-driven workers whose profile declares memory, as the agents of a
/// gateway with agent memory on do.
fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
memory = true
"#
    ))
    .expect("parse the definition")
}

fn source_spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(SOURCE).unwrap(),
        profile: ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "side-chat-memory-restart", "account": "remembering"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "remembering-profile".into(),
        "remembering-transport".into(),
    )
}

/// The note title a message asks the model to remember, if it asks.
fn note_title(message: &str) -> Option<String> {
    let start = message.find(REMEMBER)?;
    Some(
        message[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect(),
    )
}

/// The first string in `value` that holds `marker`.
fn text_holding(value: &Value, marker: &str) -> Option<String> {
    match value {
        Value::String(text) if text.contains(marker) => Some(text.clone()),
        Value::Array(items) => items.iter().find_map(|item| text_holding(item, marker)),
        Value::Object(fields) => fields
            .values()
            .find_map(|field| text_holding(field, marker)),
        _ => None,
    }
}

/// The arguments of the `memory` call a message asks the model to make, if
/// it asks: the JSON object after [`CALL`], or a `remember` of the note a
/// [`REMEMBER`] token names.
fn memory_call(message: &Value) -> Option<Value> {
    if let Some(text) = text_holding(message, CALL) {
        let start = text.find(CALL)? + CALL.len();
        return serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Value>()
            .next()?
            .ok();
    }
    let title = note_title(&message.to_string())?;
    Some(json!({
        "action": "remember",
        "kind": "fact",
        "title": title,
        "description": "a note made in a conversation",
        "body": format!("The note {title} was asked for."),
        "tags": [],
    }))
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(0);

/// The members' model: each request's last message (past Core's system
/// notices) is sent on the row's channel and each whole request is kept. A
/// request whose last message asks for a `memory` call ([`memory_call`])
/// answers with that one call; every other request, the one that carries
/// that call's result included, is answered in text.
#[derive(Clone)]
struct RememberingClient {
    asked: mpsc::UnboundedSender<String>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl meerkat_client::LlmClient for RememberingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_value(request).unwrap_or_default());
        let last = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)));
        let answered = matches!(last, Some(Message::ToolResults { .. }));
        let last = last
            .map(|last| serde_json::to_value(last).unwrap_or_default())
            .unwrap_or_default();
        let call = if answered { None } else { memory_call(&last) };
        let _ = self.asked.send(last.to_string());
        let [usage, done] = llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            if call.is_some() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
        );
        Box::pin(async_stream::stream! {
            if let Some(args) = call {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: format!("call-memory-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed)),
                    name: MEMORY_TOOL.to_string(),
                    args,
                    meta: None,
                });
            } else {
                yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
            }
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "side_chat_memory_dataset".to_string(),
            table: "side_chat_memory_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["root@example.test".to_string()],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// One booted gateway runtime, its Console, and what its members' model was
/// asked.
struct Rig {
    runtime: meerkat_meta::UnifiedRuntime,
    roster: Arc<MutableRosterProvider>,
    console: axum::Router,
    asked: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<Value>>>,
}

/// Boot the gateway's runtime composition on `state` for the mob `mob_id`,
/// with [`SOURCE`] as the host's configuration and agent memory on over
/// `store`.
async fn boot(state: &Path, mob_id: &str, store: &SqliteAgentMemoryStore) -> Rig {
    boot_with(state, mob_id, store, |builder| builder).await
}

/// [`boot`] with `configure` applied to the builder.
async fn boot_with(
    state: &Path,
    mob_id: &str,
    store: &SqliteAgentMemoryStore,
    configure: impl FnOnce(UnifiedRuntimeBuilder) -> UnifiedRuntimeBuilder,
) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let roster = Arc::new(MutableRosterProvider::new(vec![source_spec()]));
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(roster.clone())
        .identity_runtime_instance_id("side-chat-memory-restart")
        .agent_memory(Arc::new(store.clone()), AgentMemoryConfig::default())
        .default_llm_client(Arc::new(RememberingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
        }));
    let runtime = Box::pin(configure(builder).build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime with agent memory");
    let console = runtime.build_reference_app_router(decision_state());
    Rig {
        runtime,
        roster,
        console,
        asked,
        requests,
    }
}

fn child_root_released(outcome: &meerkat_meta::ChildMobStandDownOutcome) -> bool {
    matches!(
        outcome,
        meerkat_meta::ChildMobStandDownOutcome::Released { .. }
            | meerkat_meta::ChildMobStandDownOutcome::NotConfigured
    )
}

impl Rig {
    /// One JSON-RPC call through the dispatcher the stdio gateway serves.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let ctx = meerkat_meta::rpc::IdentityFirstContext {
            runtime: self
                .runtime
                .identity_runtime()
                .expect("identity runtime installed"),
            roster_provider: self.roster.clone(),
            topology_provider: None,
            customizer: None,
            agent_memory_provider: None,
            mob_definition: None,
            transcript_edit_service: None,
            compaction_floors: None,
        };
        let raw = meerkat_meta::handle_unified_rpc_json(
            &self.runtime,
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
            Duration::from_mins(1),
            None,
            Some(&ctx),
        )
        .await;
        serde_json::from_str(&raw).expect("json-rpc response")
    }

    /// One JSON-RPC call through the Console.
    async fn console_rpc(&self, method: &str, params: Value) -> Value {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": "test",
            "method": method,
            "params": params,
        });
        let response = self
            .console
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(payload.to_string()))
                    .expect("rpc request"),
            )
            .await
            .expect("rpc response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("rpc body");
        serde_json::from_slice(&body).expect("rpc json")
    }

    /// The Console's answer to a fork of [`SOURCE`] into the side chat
    /// `side`, asked again while the source's last run is still ending.
    async fn fork_answer(&self, side: &str) -> Value {
        self.fork_answer_of(SOURCE, side).await
    }

    /// The Console's answer to a fork of the member `source` into the side
    /// chat `side`, asked again while the source's last run is still ending.
    async fn fork_answer_of(&self, source: &str, side: &str) -> Value {
        let mut forked = Value::Null;
        for _ in 0..50 {
            forked = self
                .console_rpc(
                    "meta/fork_member",
                    json!({ "source_member_id": source, "agent_identity": side }),
                )
                .await;
            if forked["error"]["data"]["cause"] != json!("running") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        forked
    }

    /// Fork [`SOURCE`] into the side chat `side` on the Console.
    async fn fork_side_chat(&self, side: &str) {
        let forked = self.fork_answer(side).await;
        assert!(
            forked["error"].is_null(),
            "the side chat {side} is forked: {forked:#}"
        );
    }

    fn mob(&self) -> meerkat_mob::MobHandle {
        self.runtime.mob_handle()
    }

    fn member(name: &str) -> meerkat_mob::AgentIdentity {
        meerkat_meta::member_comms_id::mob_member_id(name)
    }

    /// Whether Core holds `name`'s member without a loaded session.
    fn dormant(&self, name: &str) -> bool {
        self.mob().member_unmaterialized(&Self::member(name))
    }

    /// The rows of `name`'s durable transcript, as its store holds them.
    async fn transcript(&self, name: &str) -> Vec<Value> {
        let session_id = self
            .mob()
            .resolve_bridge_session_id(&Self::member(name))
            .await
            .expect("the member has a session binding");
        let session = self
            .runtime
            .mob_runtime()
            .session_service()
            .expect("the mob's session service")
            .load_persisted_session_tolerating_tail(&session_id)
            .await
            .expect("the session reads")
            .expect("the session is stored");
        session
            .messages()
            .iter()
            .map(|message| serde_json::to_value(message).expect("a transcript row"))
            .collect()
    }

    /// Wait until a member's turn is asked `marker`.
    async fn asked(&mut self, marker: &str) {
        let asked = tokio::time::timeout(TURN_BOUND, async {
            while let Some(asked) = self.asked.recv().await {
                if asked.contains(marker) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(asked, "a turn is asked {marker}");
    }

    /// Wait until the run asked `marker` ended in `name`'s durable
    /// transcript: `answers` assistant rows after the row that asked it, the
    /// last row an assistant's (a run that calls a tool once answers twice).
    async fn run_ended(&self, name: &str, marker: &str, answers: usize) {
        let deadline = tokio::time::Instant::now() + TURN_BOUND;
        loop {
            let transcript = self.transcript(name).await;
            let asked = transcript.iter().position(|row| {
                row["role"] != "block_assistant" && row.to_string().contains(marker)
            });
            let ended = asked.is_some_and(|asked| {
                let tail = &transcript[asked..];
                tail.iter()
                    .filter(|row| row["role"] == "block_assistant")
                    .count()
                    >= answers
                    && tail
                        .last()
                        .is_some_and(|row| row["role"] == "block_assistant")
            });
            if ended {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the run asked {marker} ended in {name}'s transcript: {transcript:#?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// One turn of the agent [`SOURCE`], asked `marker`. A marker that asks
    /// it to remember makes the model call `memory` once.
    async fn source_turn(&mut self, marker: &str) {
        let sent = self
            .rpc("meta/send", json!({"identity": SOURCE, "content": marker}))
            .await;
        assert!(sent["error"].is_null(), "the send is admitted: {sent:#?}");
        self.asked(marker).await;
        let answers = if marker.contains(REMEMBER) { 2 } else { 1 };
        self.run_ended(SOURCE, marker, answers).await;
    }

    /// One turn of the side chat `side` in which its model calls `memory`
    /// once with `args`: what the tool answered. `tag` names the turn.
    async fn side_chat_call(&mut self, side: &str, tag: &str, args: Value) -> String {
        let message = self
            .rpc(
                "meta/send_message",
                json!({"member_id": side, "message": format!("{tag} {CALL}{args}")}),
            )
            .await;
        assert!(message["error"].is_null(), "{message:#?}");
        self.asked(tag).await;
        self.run_ended(side, tag, 2).await;
        self.tools_answered(side, tag).await
    }

    /// What the tools answered in the run of `name` that was asked `tag`:
    /// the rows of its durable transcript after the row that asked it that
    /// are no assistant's.
    async fn tools_answered(&self, name: &str, tag: &str) -> String {
        let transcript = self.transcript(name).await;
        let asked = transcript
            .iter()
            .position(|row| row["role"] != "block_assistant" && row.to_string().contains(tag))
            .unwrap_or_else(|| panic!("{name} was asked {tag}"));
        transcript[asked + 1..]
            .iter()
            .filter(|row| row["role"] != "block_assistant")
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One turn of the side chat `side`, asked `marker`: the names of the
    /// tools its model was offered. A marker that asks it to remember makes
    /// the model call `memory` once.
    async fn side_chat_turn(&mut self, side: &str, marker: &str) -> Vec<String> {
        let message = self
            .rpc(
                "meta/send_message",
                json!({"member_id": side, "message": marker}),
            )
            .await;
        assert!(message["error"].is_null(), "{message:#?}");
        self.asked(marker).await;
        let answers = if marker.contains(REMEMBER) { 2 } else { 1 };
        self.run_ended(side, marker, answers).await;
        self.tools_offered(marker)
    }

    /// The names of the tools offered in the first model request whose last
    /// message holds `marker`.
    fn tools_offered(&self, marker: &str) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        let request = requests
            .iter()
            .find(|request| {
                request["messages"]
                    .as_array()
                    .and_then(|messages| {
                        messages
                            .iter()
                            .rev()
                            .find(|message| message["role"] != "system_notice")
                    })
                    .is_some_and(|last| last.to_string().contains(marker))
            })
            .unwrap_or_else(|| panic!("a model request asked {marker}"));
        request["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Shut down before a reopen over the same state directory in this
    /// process: stand the durable child root down, bounded, until it is
    /// released.
    async fn shutdown_for_reopen(self) {
        let mut outcome = self.runtime.shutdown().await.child_mob_stand_down;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !child_root_released(&outcome) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            outcome = self
                .runtime
                .mob_runtime()
                .stand_down_child_mobs(deadline)
                .await;
        }
        assert!(
            child_root_released(&outcome),
            "the durable child root is released before the reopen: {outcome:?}"
        );
    }
}

/// The titles of the records `identity`'s own scope holds.
async fn identity_titles(store: &SqliteAgentMemoryStore, identity: &str) -> Vec<String> {
    store
        .manifest(
            &[MemoryScope::Identity {
                realm: AgentMemoryConfig::default().realm,
                identity: identity.to_string(),
            }],
            meerkat_meta::ManifestTier::Full,
        )
        .await
        .expect("manifest")
        .into_iter()
        .map(|record| record.title)
        .collect()
}

fn offered(tools: &[String]) -> bool {
    tools.iter().any(|tool| tool == MEMORY_TOOL)
}

/// A side chat that existed before a restart: at its first turn in the new
/// process, while its source agent is dormant, it is offered `memory`, its
/// call stores the note in its own scope, and its source agent stays
/// dormant.
///
/// RED before the fix: the side chat was offered no `memory` tool until its
/// source agent had run in the new process, and its call was answered tool
/// not found.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chat_is_offered_memory_before_its_source_runs() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();

    let mut rig = boot(state.path(), &mob, &store).await;
    rig.source_turn("SOURCE-FIRST-TURN").await;
    rig.fork_side_chat("side-kept").await;
    let before = rig
        .side_chat_turn("side-kept", "REMEMBER-BEFORE-RESTART")
        .await;
    assert!(
        offered(&before),
        "before the restart the side chat is offered `memory`: {before:?}"
    );
    assert!(
        identity_titles(&store, "side-kept")
            .await
            .contains(&"REMEMBER-BEFORE-RESTART".to_string()),
        "before the restart the side chat's note is in its own scope"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(state.path(), &mob, &store).await;
    assert!(rig.dormant(SOURCE), "the restart loads no agent");
    let after = rig
        .side_chat_turn("side-kept", "REMEMBER-AFTER-RESTART")
        .await;
    assert!(
        offered(&after),
        "at its first turn after the restart the side chat is offered `memory`: {after:?}"
    );
    assert!(
        identity_titles(&store, "side-kept")
            .await
            .contains(&"REMEMBER-AFTER-RESTART".to_string()),
        "the side chat's call stores the note in its own scope"
    );
    assert!(
        rig.dormant(SOURCE),
        "the side chat's turn woke nothing: its source agent is still dormant"
    );
    rig.shutdown_for_reopen().await;
}

/// A side chat forked after a restart, while its source agent is dormant:
/// it is offered `memory` at its first turn, its call stores the note in
/// its own scope, and its source agent stays dormant through the fork and
/// the turn.
///
/// RED before the fix: such a side chat was seated without any of its
/// source's per-spawn tools and was offered no `memory` tool for as long as
/// it stayed loaded in that process.
#[tokio::test(flavor = "multi_thread")]
async fn a_side_chat_forked_after_a_restart_before_its_source_runs_is_offered_memory() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();

    let mut rig = boot(state.path(), &mob, &store).await;
    rig.source_turn("SOURCE-FIRST-TURN").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot(state.path(), &mob, &store).await;
    assert!(rig.dormant(SOURCE), "the restart loads no agent");
    rig.fork_side_chat("side-early").await;
    assert!(
        rig.dormant(SOURCE),
        "a dormant source forks from its stored session without waking"
    );
    let tools = rig.side_chat_turn("side-early", "REMEMBER-EARLY").await;
    assert!(
        offered(&tools),
        "a side chat forked before its source ran is offered `memory`: {tools:?}"
    );
    assert!(
        identity_titles(&store, "side-early")
            .await
            .contains(&"REMEMBER-EARLY".to_string()),
        "the side chat's call stores the note in its own scope"
    );
    assert!(
        rig.dormant(SOURCE),
        "the side chat's turn woke nothing: its source agent is still dormant"
    );
    rig.shutdown_for_reopen().await;
}

/// A side chat that ran after a restart before its source agent did, once
/// its source agent has run in the new process too: it is offered `memory`,
/// and the note its call stores is kept in the side chat's own scope.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chats_memory_stays_its_own_after_its_source_runs() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();

    let mut rig = boot(state.path(), &mob, &store).await;
    rig.source_turn("SOURCE-FIRST-TURN").await;
    rig.fork_side_chat("side-kept").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot(state.path(), &mob, &store).await;
    assert!(rig.dormant(SOURCE), "the restart loads no agent");
    // The side chat is built in the new process before its source agent.
    rig.side_chat_turn("side-kept", "SIDE-TURN-BEFORE-SOURCE")
        .await;
    rig.source_turn("SOURCE-TURN-AFTER-RESTART").await;
    // A turn in between, so a change of the side chat's tools at a turn
    // boundary has been applied before it is asked to remember.
    rig.side_chat_turn("side-kept", "SIDE-TURN-AFTER-SOURCE")
        .await;
    let tools = rig.side_chat_turn("side-kept", "REMEMBER-LATE").await;
    assert!(
        offered(&tools),
        "after its source ran the side chat is offered `memory`: {tools:?}"
    );
    assert!(
        identity_titles(&store, "side-kept")
            .await
            .contains(&"REMEMBER-LATE".to_string()),
        "the side chat's note is kept in the side chat's own scope"
    );
    rig.shutdown_for_reopen().await;
}

/// Control: a side chat is offered `memory` exactly when its source agent
/// is. An agent whose profile the host switched agent memory off for is
/// offered no `memory` tool, and neither is its side chat, before the agent
/// runs after a restart and after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_side_chat_of_an_agent_without_the_memory_tool_gets_none() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let without_memory =
        |builder: UnifiedRuntimeBuilder| builder.agent_memory_for_profile("worker", false);

    let mut rig = boot_with(state.path(), &mob, &store, without_memory).await;
    rig.source_turn("SOURCE-FIRST-TURN").await;
    let source_tools = rig.tools_offered("SOURCE-FIRST-TURN");
    assert!(
        !offered(&source_tools),
        "the agent is offered no `memory` tool: {source_tools:?}"
    );
    rig.fork_side_chat("side-kept").await;
    rig.shutdown_for_reopen().await;

    let mut rig = boot_with(state.path(), &mob, &store, without_memory).await;
    assert!(rig.dormant(SOURCE), "the restart loads no agent");
    let before = rig
        .side_chat_turn("side-kept", "SIDE-TURN-BEFORE-SOURCE")
        .await;
    assert!(
        !offered(&before),
        "before its source ran the side chat is offered no `memory` tool: {before:?}"
    );
    rig.source_turn("SOURCE-TURN-AFTER-RESTART").await;
    rig.side_chat_turn("side-kept", "SIDE-TURN-AFTER-SOURCE")
        .await;
    let after = rig.side_chat_turn("side-kept", "SIDE-TURN-LAST").await;
    assert!(
        !offered(&after),
        "after its source ran the side chat is offered no `memory` tool: {after:?}"
    );
    rig.shutdown_for_reopen().await;
}

/// Control: on a fresh state an agent that never ran has no conversation to
/// fork, and the fork is refused typed, with agent memory on as without it.
/// This refusal is its own rule, not the restart's: nothing here changes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_of_an_agent_that_never_ran_is_refused_with_memory_on() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();

    let rig = boot(state.path(), &mob, &store).await;
    assert!(rig.dormant(SOURCE), "the agent never ran");
    let forked = rig.fork_answer("side-unborn").await;
    assert_eq!(
        forked["error"]["data"]["kind"],
        json!("fork_source_unavailable"),
        "{forked:#}"
    );
    assert_eq!(
        forked["error"]["data"]["cause"],
        json!("no_session"),
        "{forked:#}"
    );
    assert!(rig.dormant(SOURCE), "the refused fork woke nothing");
    assert!(
        rig.requests.lock().unwrap().is_empty(),
        "no model was asked anything"
    );
    rig.shutdown_for_reopen().await;
}

// ---------------------------------------------------------------------------
// Each `memory` verb of a side chat that was built before its source agent
// in a new process and is used after the source agent has run: the verb acts
// on the side chat's own notes, and the source agent's notes are as they
// were.
// ---------------------------------------------------------------------------

const AGENT_NOTE: &str = "REMEMBER-AGENT-NOTE";
const SIDE_NOTE: &str = "REMEMBER-SIDE-NOTE";
const SIDE: &str = "side-kept";

/// The id of the note titled `title` in `identity`'s own scope.
async fn note_id(store: &SqliteAgentMemoryStore, identity: &str, title: &str) -> String {
    store
        .manifest(
            &[MemoryScope::Identity {
                realm: AgentMemoryConfig::default().realm,
                identity: identity.to_string(),
            }],
            meerkat_meta::ManifestTier::Full,
        )
        .await
        .expect("manifest")
        .into_iter()
        .find(|record| record.title == title)
        .map(|record| record.id)
        .unwrap_or_else(|| panic!("{identity} holds the note {title}"))
}

/// The agent [`SOURCE`] with one note of its own ([`AGENT_NOTE`]) and its
/// side chat [`SIDE`] with one note of its own ([`SIDE_NOTE`]), both stored
/// before a restart; after the restart the side chat ran before the agent,
/// then the agent ran, then the side chat ran once more.
async fn side_chat_used_after_its_source_ran(
    state: &Path,
    mob: &str,
    store: &SqliteAgentMemoryStore,
) -> Rig {
    let mut rig = boot(state, mob, store).await;
    rig.source_turn(AGENT_NOTE).await;
    rig.fork_side_chat(SIDE).await;
    rig.side_chat_turn(SIDE, SIDE_NOTE).await;
    assert_eq!(
        identity_titles(store, SOURCE).await,
        vec![AGENT_NOTE.to_string()],
        "the agent holds its own note"
    );
    assert_eq!(
        identity_titles(store, SIDE).await,
        vec![SIDE_NOTE.to_string()],
        "the side chat holds its own note"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(state, mob, store).await;
    assert!(rig.dormant(SOURCE), "the restart loads no agent");
    rig.side_chat_turn(SIDE, "SIDE-TURN-BEFORE-SOURCE").await;
    rig.source_turn("SOURCE-TURN-AFTER-RESTART").await;
    rig.side_chat_turn(SIDE, "SIDE-TURN-AFTER-SOURCE").await;
    rig
}

/// `update` from such a side chat rewrites the side chat's own note, and an
/// `update` it aims at a note of its source agent changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chats_update_rewrites_its_own_note() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let mut rig = side_chat_used_after_its_source_ran(state.path(), &mob, &store).await;

    let own = note_id(&store, SIDE, SIDE_NOTE).await;
    rig.side_chat_call(
        SIDE,
        "UPDATE-OWN-NOTE",
        json!({
            "action": "update",
            "memory_id": own,
            "kind": "fact",
            "title": "SIDE-NOTE-REWRITTEN",
            "description": "the side chat's note, rewritten",
            "body": "The side chat rewrote its note.",
            "tags": [],
        }),
    )
    .await;
    assert_eq!(
        identity_titles(&store, SIDE).await,
        vec!["SIDE-NOTE-REWRITTEN".to_string()],
        "the side chat's update rewrote the side chat's own note"
    );

    let agents = note_id(&store, SOURCE, AGENT_NOTE).await;
    rig.side_chat_call(
        SIDE,
        "UPDATE-AGENT-NOTE",
        json!({
            "action": "update",
            "memory_id": agents,
            "kind": "fact",
            "title": "AGENT-NOTE-REWRITTEN",
            "description": "a note that is not the side chat's",
            "body": "The side chat asked to rewrite a note of its source agent.",
            "tags": [],
        }),
    )
    .await;
    assert_eq!(
        identity_titles(&store, SOURCE).await,
        vec![AGENT_NOTE.to_string()],
        "the source agent's notes are as they were"
    );
    assert_eq!(
        identity_titles(&store, SIDE).await,
        vec!["SIDE-NOTE-REWRITTEN".to_string()],
        "the side chat's notes are its own rewritten note"
    );
    rig.shutdown_for_reopen().await;
}

/// `forget` from such a side chat forgets the side chat's own note, and a
/// `forget` it aims at a note of its source agent changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chats_forget_forgets_its_own_note() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let mut rig = side_chat_used_after_its_source_ran(state.path(), &mob, &store).await;

    let agents = note_id(&store, SOURCE, AGENT_NOTE).await;
    rig.side_chat_call(
        SIDE,
        "FORGET-AGENT-NOTE",
        json!({ "action": "forget", "memory_id": agents }),
    )
    .await;
    assert_eq!(
        identity_titles(&store, SOURCE).await,
        vec![AGENT_NOTE.to_string()],
        "the source agent's notes are as they were"
    );

    let own = note_id(&store, SIDE, SIDE_NOTE).await;
    rig.side_chat_call(
        SIDE,
        "FORGET-OWN-NOTE",
        json!({ "action": "forget", "memory_id": own }),
    )
    .await;
    assert!(
        identity_titles(&store, SIDE).await.is_empty(),
        "the side chat's forget forgot the side chat's own note"
    );
    assert_eq!(
        identity_titles(&store, SOURCE).await,
        vec![AGENT_NOTE.to_string()],
        "the source agent's notes are as they were"
    );
    rig.shutdown_for_reopen().await;
}

/// `recall` from such a side chat answers the side chat's own notes and
/// none of its source agent's.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chats_recall_reads_its_own_notes() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let mut rig = side_chat_used_after_its_source_ran(state.path(), &mob, &store).await;

    let answered = rig
        .side_chat_call(SIDE, "RECALL-NOTES", json!({ "action": "recall" }))
        .await;
    assert!(
        answered.contains(SIDE_NOTE),
        "the side chat's recall answers the side chat's own note: {answered}"
    );
    assert!(
        !answered.contains(AGENT_NOTE),
        "the side chat's recall answers no note of its source agent: {answered}"
    );
    rig.shutdown_for_reopen().await;
}

/// `propose_to_mob` from such a side chat files a proposal the side chat
/// authored.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_side_chats_proposal_names_the_side_chat_as_its_author() {
    use meerkat_meta::StewardStore as _;

    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let mut rig = side_chat_used_after_its_source_ran(state.path(), &mob, &store).await;

    rig.side_chat_call(
        SIDE,
        "PROPOSE-TO-MOB",
        json!({
            "action": "propose_to_mob",
            "kind": "fact",
            "title": "SIDE-PROPOSAL",
            "description": "a note the side chat proposes to its mob",
            "body": "The side chat proposes this note to its mob.",
            "tags": [],
        }),
    )
    .await;
    let authors: Vec<MemoryAuthor> = store
        .pending_proposals(&AgentMemoryConfig::default().realm, 16)
        .await
        .expect("pending proposals")
        .into_iter()
        .filter(|proposal| proposal.record.title == "SIDE-PROPOSAL")
        .map(|proposal| proposal.author)
        .collect();
    assert_eq!(
        authors,
        vec![MemoryAuthor::Agent {
            identity: SIDE.to_string(),
        }],
        "the proposal the side chat filed names the side chat as its author"
    );
    rig.shutdown_for_reopen().await;
}

// ---------------------------------------------------------------------------
// Where a side chat's conversation and an agent's notes can be read.
// ---------------------------------------------------------------------------

/// A closed side chat's conversation is kept: its stored session answers
/// its history after the close, with the `memory` call's result that names
/// the note's id.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_side_chats_conversation_is_kept_with_its_memory_results() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let closed_side = "side-closed";

    let mut rig = boot(state.path(), &mob, &store).await;
    rig.source_turn("SOURCE-FIRST-TURN").await;
    rig.fork_side_chat(closed_side).await;
    rig.side_chat_turn(closed_side, "REMEMBER-CLOSED-NOTE")
        .await;
    let note = note_id(&store, closed_side, "REMEMBER-CLOSED-NOTE").await;
    let member = Rig::member(closed_side);
    let session = rig
        .mob()
        .resolve_bridge_session_id(&member)
        .await
        .expect("the side chat has a session binding")
        .to_string();

    let closed = rig
        .console_rpc("meta/retire_member", json!({ "member_id": closed_side }))
        .await;
    assert!(
        closed["error"].is_null(),
        "the side chat closes: {closed:#}"
    );
    let deadline = tokio::time::Instant::now() + TURN_BOUND;
    while rig
        .mob()
        .get_member(&member)
        .await
        .expect("the roster reads")
        .is_some()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the closed side chat leaves the roster"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let history = rig
        .runtime
        .mob_runtime()
        .read_session_history(&session, 0, None)
        .await
        .expect("the closed side chat's stored session answers its history");
    let stored = serde_json::to_string(&history.messages).expect("the history serializes");
    assert!(
        stored.contains(&note),
        "the closed side chat's stored conversation holds the `memory` result that names \
         the note's id {note}"
    );

    // Measured, not asserted: whether the Console's timeline of the closed
    // side chat's name carries that result.
    let response = rig
        .console
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/console/timeline?identity={closed_side}&mode=recent&limit=1000"
                ))
                .body(Body::empty())
                .expect("timeline request"),
        )
        .await
        .expect("timeline response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("timeline body");
    let timeline = String::from_utf8_lossy(&body);
    eprintln!(
        "MEASURED closed side chat timeline: status={status} bytes={} holds_note_id={} \
         holds_stored_memory_text={}",
        timeline.len(),
        timeline.contains(&note),
        timeline.contains("Stored memory"),
    );
    rig.shutdown_for_reopen().await;
}

/// A note under an agent's own key is listed on the agent's memory panel
/// and in its full manifest until it is forgotten. A forget by its id makes
/// it a tombstone: the full manifest and the panel's read of active notes
/// no longer list it, and the panel's unfiltered read lists it as
/// tombstoned.
#[tokio::test(flavor = "multi_thread")]
async fn a_note_under_an_agents_key_is_listed_until_it_is_forgotten() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let title = "AGENT-KEY-NOTE";
    let note = store
        .remember_authored(
            &MemoryScope::Identity {
                realm: AgentMemoryConfig::default().realm,
                identity: SOURCE.to_string(),
            },
            NewMemoryRecord {
                kind: MemoryKind::Fact,
                title: title.to_string(),
                description: "a note under the agent's key".to_string(),
                body: "A note stored under the agent's key.".to_string(),
                tags: vec![],
                evidence: vec![],
                verification: None,
            },
            MemoryAuthor::Agent {
                identity: SOURCE.to_string(),
            },
        )
        .await
        .expect("store the note")
        .memory_id;

    // The Console's memory panel reads the store a host registers for it,
    // before the Console's router is built: a gateway registers its agent
    // memory store, and so does this row.
    let mut rig = boot(state.path(), &mob, &store).await;
    rig.runtime.set_memory_panel_store(Arc::new(store.clone()));
    rig.console = rig.runtime.build_reference_app_router(decision_state());
    let listed = |answer: &Value| -> bool {
        answer["result"]["records"]
            .as_array()
            .is_some_and(|records| records.iter().any(|record| record["title"] == json!(title)))
    };
    let manifest_params = json!({ "identity": SOURCE, "tier": "full" });

    let panel = rig
        .console_rpc("meta/memory/panel/records", json!({}))
        .await;
    assert!(panel["error"].is_null(), "{panel:#}");
    assert!(listed(&panel), "the panel lists the note: {panel:#}");
    let manifest = rig
        .rpc("meta/agent_memory/manifest", manifest_params.clone())
        .await;
    assert!(manifest["error"].is_null(), "{manifest:#}");
    assert!(
        listed(&manifest),
        "the full manifest lists the note: {manifest:#}"
    );

    let forgotten = rig
        .rpc(
            "meta/agent_memory/forget",
            json!({ "identity": SOURCE, "memory_id": note }),
        )
        .await;
    assert!(
        forgotten["error"].is_null(),
        "the forget by id answers: {forgotten:#}"
    );

    // A forget makes the note a tombstone: no read of active notes lists
    // it, and the panel's unfiltered read still does, marked tombstoned.
    let manifest = rig.rpc("meta/agent_memory/manifest", manifest_params).await;
    assert!(manifest["error"].is_null(), "{manifest:#}");
    assert!(
        !listed(&manifest),
        "the full manifest no longer lists the note: {manifest:#}"
    );
    let active = rig
        .console_rpc("meta/memory/panel/records", json!({ "status": "active" }))
        .await;
    assert!(active["error"].is_null(), "{active:#}");
    assert!(
        !listed(&active),
        "the panel's read of active notes no longer lists the note: {active:#}"
    );
    let panel = rig
        .console_rpc("meta/memory/panel/records", json!({}))
        .await;
    assert!(panel["error"].is_null(), "{panel:#}");
    let statuses: Vec<Value> = panel["result"]["records"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter(|record| record["title"] == json!(title))
                .map(|record| record["status"]["status"].clone())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        statuses,
        vec![json!("tombstoned")],
        "the panel's unfiltered read lists the forgotten note as a tombstone: {panel:#}"
    );

    // Measured, not asserted: what the panel's record detail answers for
    // the forgotten note.
    let detail = rig
        .console_rpc("meta/memory/panel/record", json!({ "memory_id": note }))
        .await;
    eprintln!(
        "MEASURED forgotten note detail: error={} holds_body_text={}",
        detail["error"],
        detail
            .to_string()
            .contains("A note stored under the agent's key."),
    );
    rig.shutdown_for_reopen().await;
}

/// A side chat is offered `memory` exactly when the member it was forked
/// from is. A member that is no agent of the roster is offered none, on a
/// profile that enables agent memory too; so is its side chat, also one
/// forked before anything ran after a restart: its `memory` call is
/// refused and stores nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_side_chat_of_a_member_that_is_no_agent_forked_after_a_restart_is_offered_none() {
    let state = tempfile::tempdir().unwrap();
    let memory = tempfile::tempdir().unwrap();
    let store = SqliteAgentMemoryStore::open(memory.path()).expect("sqlite store");
    let mob = fresh_mob_id();
    let helper = "helper-a";

    let mut rig = boot(state.path(), &mob, &store).await;
    rig.runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            helper.to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn a member that is no agent of the roster");
    let helper_tools = rig.side_chat_turn(helper, "HELPER-FIRST-TURN").await;
    assert!(
        !offered(&helper_tools),
        "a member that is no agent is offered no `memory` tool: {helper_tools:?}"
    );
    rig.shutdown_for_reopen().await;

    let mut rig = boot(state.path(), &mob, &store).await;
    assert!(rig.dormant(helper), "the restart loads no member");
    let forked = rig.fork_answer_of(helper, "helper-side").await;
    assert!(
        forked["error"].is_null(),
        "the side chat is forked: {forked:#}"
    );
    let tools = rig
        .side_chat_turn("helper-side", "REMEMBER-HELPER-SIDE")
        .await;
    assert!(
        !offered(&tools),
        "the side chat is offered no `memory` tool, as its source is: {tools:?}"
    );
    assert!(
        identity_titles(&store, "helper-side").await.is_empty(),
        "the side chat's refused call stored nothing"
    );
    rig.shutdown_for_reopen().await;
}
