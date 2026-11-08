//! A mob member's session forked at a chosen message boundary over RPC.
//!
//! A standalone session forks at a message boundary through `session/fork_at`
//! (exact context: the child's history is the prefix). For a mob member's
//! session that call is refused by the router ("mob-owned session
//! transcripts cannot be edited through the generic session surface"), and
//! the member counterpart is `mob/fork_member`: the mob runtime's
//! exact-prefix fork ([`meerkat_mob::MobHandle::fork_member`] with an
//! explicit `message_count`), which seats the child under a new identity.
//! `mob/fork_helper` renders `ForkContext::{FullHistory, LastMessages}` as
//! text into a fresh session's first prompt (prompt context), and
//! `mob/spawn` with `launch_mode = resume` takes a whole session. The ignored
//! test states the same-identity rewind, which no method offers yet.

#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]
#![cfg(feature = "mob")]

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{BlobStore, Config, ConfigRuntime, MemoryConfigStore, StopReason};
use meerkat_rpc::protocol::{RpcError, RpcId, RpcRequest, RpcResponse};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::MemoryBlobStore;
use serde_json::{Value, json};
use tokio::sync::Notify;

const MODEL: &str = "claude-sonnet-4-5";
const MOB_ID: &str = "tui";
const MAIN: &str = "main";
/// The user turns every source session runs.
const PROMPTS: [&str; 3] = ["first question", "second question", "third question"];
/// The boundary: the index of the second user message in the session's
/// history (`[system, user, assistant, user, ...]`), so the prefix is the
/// system prompt and the first exchange.
const BOUNDARY: usize = 3;
/// A prompt the scripted client holds until the test releases it.
const HOLD: &str = "hold this turn";

/// Answers every call with `reply to: <last user text>` and records the tool
/// names each call offers. A call whose last user message is [`HOLD`]
/// signals `entered` and waits for `release`.
#[derive(Default)]
struct ScriptedClient {
    tools: Mutex<Vec<Vec<String>>>,
    entered: Notify,
    release: Notify,
}

fn last_user_text(messages: &[meerkat_core::Message]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|message| {
            let value = serde_json::to_value(message).ok()?;
            (value["role"] == "user").then(|| text_of(&value["content"]))
        })
        .unwrap_or_default()
}

/// The text of a message's content, whatever its block shape: every string
/// under a `text` key, or the content itself when it is a string.
fn text_of(content: &Value) -> String {
    fn collect(value: &Value, out: &mut String) {
        match value {
            Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
            Value::Object(fields) => {
                for (key, field) in fields {
                    match field {
                        Value::String(text) if key == "text" || key == "content" => {
                            out.push_str(text);
                        }
                        Value::String(_) => {}
                        other => collect(other, out),
                    }
                }
            }
            _ => {}
        }
    }
    if let Value::String(text) = content {
        return text.clone();
    }
    let mut out = String::new();
    collect(content, &mut out);
    out
}

#[async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        self.tools.lock().unwrap().push(
            request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect(),
        );
        let user = last_user_text(&request.messages);
        let hold = user.contains(HOLD);
        Box::pin(async_stream::stream! {
            if hold {
                self.entered.notify_one();
                self.release.notified().await;
            }
            yield Ok(meerkat_client::LlmEvent::TextDelta {
                delta: format!("reply to: {user}"),
                meta: None,
            });
            yield Ok(meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Anthropic,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            });
            yield Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
            });
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Rig {
    router: MethodRouter,
    client: Arc<ScriptedClient>,
    _temp: tempfile::TempDir,
}

fn rig() -> Rig {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let mut config = Config::default();
    config.agent.model = MODEL.to_string();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let runtime = SessionRuntime::new(
        factory,
        config.clone(),
        10,
        meerkat::PersistenceBundle::new(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            blob_store,
        )
        .expect("construct runtime authority"),
        NotificationSink::noop(),
    )
    .expect("compose the RPC session runtime");
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
    let client = Arc::new(ScriptedClient::default());
    runtime.set_default_llm_client(Some(client.clone()));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let router = MethodRouter::new(Arc::new(runtime), config_store, NotificationSink::noop())
        .expect("construct the method router");
    Rig {
        router,
        client,
        _temp: temp,
    }
}

fn request(method: &str, params: &Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(RpcId::Num(1)),
        method: method.to_string(),
        params: Some(
            serde_json::value::RawValue::from_string(params.to_string()).expect("raw params"),
        ),
    }
}

impl Rig {
    async fn call(&self, method: &str, params: Value) -> RpcResponse {
        self.router
            .dispatch(request(method, &params))
            .await
            .expect("a request with an id is answered")
    }

    async fn ok(&self, method: &str, params: Value) -> Value {
        let response = self.call(method, params).await;
        assert!(
            response.error.is_none(),
            "{method} must succeed: {:?}",
            response.error
        );
        serde_json::from_str(response.result.as_ref().expect("a result").get()).expect("json")
    }

    /// `(role, text)` of every message of a session's history after its
    /// system prompt; a `block_assistant` row reads as `assistant`. The
    /// history must open with exactly one system row: a history that lost
    /// its system prompt fails here rather than shifting into a prefix. The
    /// system row's text is [`Rig::system_prompt`].
    async fn history(&self, session_id: &str) -> Vec<(String, String)> {
        self.rows(session_id).await.1
    }

    /// The text of a session's single system row (see [`Rig::history`]).
    async fn system_prompt(&self, session_id: &str) -> String {
        self.rows(session_id).await.0
    }

    /// Assert `child` holds exactly the [`prefix`] of `source`: the source's
    /// system prompt, copied unchanged, then the first exchange.
    async fn assert_exact_prefix(&self, child: &str, source: &str) {
        let source_prompt = self.system_prompt(source).await;
        assert!(
            !source_prompt.is_empty(),
            "setup: the source {source} has a system prompt to copy"
        );
        assert_eq!(
            self.system_prompt(child).await,
            source_prompt,
            "the child {child} carries the source's system prompt unchanged"
        );
        assert_eq!(self.history(child).await, prefix());
    }

    /// The system row's text and the `(role, text)` rows after it.
    async fn rows(&self, session_id: &str) -> (String, Vec<(String, String)>) {
        let history = self
            .ok("session/history", json!({ "session_id": session_id }))
            .await;
        let messages = history["messages"].as_array().expect("messages");
        assert_eq!(
            messages.first().map(|message| message["role"].as_str()),
            Some(Some("system")),
            "the history of {session_id} must open with its system prompt: {messages:?}"
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1,
            "the history of {session_id} must carry exactly one system row: {messages:?}"
        );
        let system = text_of(&messages[0]["content"]);
        let rows = messages[1..]
            .iter()
            .map(|message| {
                let role = message["role"].as_str().unwrap_or_default();
                let role = if role == "block_assistant" {
                    "assistant"
                } else {
                    role
                };
                let mut body = message.clone();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("role");
                }
                (role.to_string(), text_of(&body))
            })
            .collect();
        (system, rows)
    }

    /// Create mob `tui` with one turn-driven profile and seat `main` in it,
    /// optionally under a tool access policy.
    async fn seat_main(&self, tool_access_policy: Option<Value>) -> String {
        self.ok(
            "mob/create",
            json!({
                "definition": {
                    "id": MOB_ID,
                    "profiles": {
                        "main": { "model": MODEL, "tools": { "comms": true } }
                    }
                }
            }),
        )
        .await;
        let mut spawn = json!({
            "mob_id": MOB_ID,
            "profile": "main",
            "agent_identity": MAIN,
            "runtime_mode": "turn_driven"
        });
        if let Some(policy) = tool_access_policy {
            spawn["tool_access_policy"] = policy;
        }
        self.ok("mob/spawn", spawn).await;
        self.member_session(MAIN).await
    }

    async fn member_session(&self, identity: &str) -> String {
        let handle = self.mob().await;
        handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(identity))
            .await
            .expect("the member has a bridge session")
            .to_string()
    }

    async fn mob(&self) -> meerkat_mob::MobHandle {
        self.router
            .mob_state()
            .handle_for(&meerkat_mob::MobId::from(MOB_ID))
            .await
            .expect("mob handle")
    }

    async fn member_turn(&self, identity: &str, prompt: &str) {
        self.ok(
            "mob/turn_start",
            json!({ "mob_id": MOB_ID, "agent_identity": identity, "prompt": prompt }),
        )
        .await;
    }

    async fn roster(&self) -> Vec<String> {
        let mut identities: Vec<String> = self
            .mob()
            .await
            .list_all_members()
            .await
            .into_iter()
            .map(|entry| entry.agent_identity.to_string())
            .collect();
        identities.sort();
        identities
    }
}

/// The deny list the restricted tests seat `main` under.
fn deny_peers() -> Value {
    json!({ "type": "deny_list", "value": ["peers"] })
}

/// Every model call since the last clear offered the comms tools but not
/// `peers`.
fn assert_denies_peers(rig: &Rig, what: &str) {
    let offered = rig.client.tools.lock().unwrap().clone();
    assert!(
        !offered.is_empty()
            && offered.iter().all(|tools| {
                tools.iter().any(|t| t == "send_message") && !tools.iter().any(|t| t == "peers")
            }),
        "{what}: {offered:?}"
    );
}

/// A fork child of `main` under `identity`, turn-driven like its source.
fn child_spec(identity: &str) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = meerkat_mob::SpawnMemberSpec::new("main", identity);
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    spec
}

/// The source history after [`PROMPTS`]: one user message and one reply per
/// prompt.
fn three_exchanges() -> Vec<(String, String)> {
    PROMPTS
        .iter()
        .flat_map(|prompt| {
            [
                ("user".to_string(), (*prompt).to_string()),
                ("assistant".to_string(), format!("reply to: {prompt}")),
            ]
        })
        .collect()
}

/// The prefix's history after its system prompt: the first exchange.
fn prefix() -> Vec<(String, String)> {
    three_exchanges()[..BOUNDARY - 1].to_vec()
}

/// Setup proof: a standalone session with three user turns forks at its
/// second user message (message index 3, after the system prompt) through
/// `session/fork_at`, and the child's history is exactly the prefix (exact
/// context, not a rendered prompt).
#[tokio::test]
async fn a_standalone_session_forks_at_a_message_boundary() {
    let rig = rig();
    let created = rig
        .ok("session/create", json!({ "prompt": PROMPTS[0] }))
        .await;
    let source = created["session_id"].as_str().unwrap().to_string();
    for prompt in &PROMPTS[1..] {
        rig.ok(
            "turn/start",
            json!({ "session_id": &source, "prompt": prompt }),
        )
        .await;
    }
    assert_eq!(rig.history(&source).await, three_exchanges());

    let forked = rig
        .ok(
            "session/fork_at",
            json!({
                "session_id": &source,
                "message_index": BOUNDARY,
                "running_behavior": "reject"
            }),
        )
        .await;
    let child = forked["session_id"].as_str().unwrap().to_string();
    assert_ne!(child, source);
    assert_eq!(forked["source_session_id"], source.as_str());
    assert_eq!(forked["message_count"], BOUNDARY);
    rig.assert_exact_prefix(&child, &source).await;
}

/// Today: none of the RPC routes forks a member's session at a boundary.
///
/// Delete with the fix; [`a_member_session_forks_at_a_message_boundary`] is
/// the intended behaviour.
///
/// - `session/fork_at` on the member's session is refused by the router.
/// - `mob/fork_helper` with `LastMessages { count }` yields a fresh session
///   whose first user message is the source rendered as text plus the task:
///   prompt context, not the prefix.
/// - `mob/spawn` with `launch_mode = resume` on the member's session (after
///   the member is retired) brings back the whole history.
#[tokio::test]
async fn today_a_member_session_cannot_be_forked_at_a_boundary_over_rpc() {
    let rig = rig();
    let source = rig.seat_main(None).await;
    for prompt in PROMPTS {
        rig.member_turn(MAIN, prompt).await;
    }
    // Setup took effect: the member's session holds the three exchanges.
    assert_eq!(rig.history(&source).await, three_exchanges());

    let refused = rig
        .call(
            "session/fork_at",
            json!({
                "session_id": &source,
                "message_index": BOUNDARY,
                "running_behavior": "reject"
            }),
        )
        .await;
    let error = refused
        .error
        .expect("session/fork_at refuses a member session");
    assert_eq!(error.code, meerkat_rpc::error::INVALID_PARAMS);
    assert_eq!(
        error.message,
        "mob-owned session transcripts cannot be edited through the generic session surface"
    );
    assert_eq!(rig.roster().await, vec![MAIN.to_string()]);

    let helper = rig
        .ok(
            "mob/fork_helper",
            json!({
                "mob_id": MOB_ID,
                "source_member_id": MAIN,
                "agent_identity": "rewind-helper",
                "prompt": "continue from here",
                "result_label": "answer",
                "max_text_bytes": 4096,
                "fork_context": { "type": "last_messages", "count": 2 }
            }),
        )
        .await;
    let helper_session = helper["session_id"].as_str().unwrap().to_string();
    assert_ne!(helper_session, source);
    let helper_history = rig.history(&helper_session).await;
    let users: Vec<&str> = helper_history
        .iter()
        .filter(|(role, _)| role == "user")
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(
        users.len(),
        2,
        "the helper's history is one pasted context and the task, not the prefix: \
         {helper_history:?}"
    );
    let pasted = users[0];
    assert!(
        pasted.starts_with("[Forked conversation context from member 'main']")
            && pasted.ends_with("[End of forked context]"),
        "the source arrives as one rendered text message: {pasted}"
    );
    assert!(
        pasted.contains("[user]: third question") && !pasted.contains("first question"),
        "`LastMessages` counts back from the end; it names no boundary: {pasted}"
    );
    assert_eq!(users[1], "continue from here");
    assert_ne!(helper_history, prefix());

    rig.ok(
        "mob/retire",
        json!({ "mob_id": MOB_ID, "agent_identity": MAIN }),
    )
    .await;
    rig.ok(
        "mob/spawn",
        json!({
            "mob_id": MOB_ID,
            "profile": "main",
            "agent_identity": MAIN,
            "runtime_mode": "turn_driven",
            "launch_mode": { "mode": "resume", "bridge_session_id": &source }
        }),
    )
    .await;
    assert_eq!(rig.member_session(MAIN).await, source);
    assert_eq!(
        rig.history(&source).await,
        three_exchanges(),
        "resume takes the whole session"
    );
}

/// `mob/fork_member` forks a member's session at a message boundary into a
/// new member. The child is seated under its own new identity with exactly
/// the source's prefix (the system prompt and the first exchange), answers
/// `message_count`, records the source session as its fork source and keeps
/// the source's tool restrictions; the source member keeps its session and
/// history. A fork onto the seated source identity is refused, and a fork of
/// a member whose turn is in flight is refused with `SESSION_BUSY`; neither
/// seats anything.
#[tokio::test]
async fn mob_fork_member_forks_a_member_into_a_new_member_at_a_boundary() {
    const CHILD: &str = "main-branch";
    let rig = rig();
    let source = rig.seat_main(Some(deny_peers())).await;
    for prompt in PROMPTS {
        rig.member_turn(MAIN, prompt).await;
    }
    assert_denies_peers(&rig, "setup: the source's deny list took effect");
    assert_eq!(rig.history(&source).await, three_exchanges());

    let fork = |agent_identity: &'static str| {
        rig.call(
            "mob/fork_member",
            json!({
                "mob_id": MOB_ID,
                "source_member_id": MAIN,
                "agent_identity": agent_identity,
                "message_index": BOUNDARY
            }),
        )
    };

    let same_identity = fork(MAIN).await;
    assert!(
        same_identity.error.is_some(),
        "a fork onto the seated source identity is refused: {:?}",
        same_identity.result
    );
    assert_eq!(rig.roster().await, vec![MAIN.to_string()]);

    let forked = fork(CHILD).await;
    assert!(
        forked.error.is_none(),
        "mob/fork_member forks the member: {:?}",
        forked.error
    );
    let forked: Value =
        serde_json::from_str(forked.result.as_ref().expect("a result").get()).expect("json");
    let child = forked["session_id"].as_str().unwrap().to_string();
    assert_ne!(child, source);
    assert_eq!(forked["agent_identity"], CHILD);
    assert_eq!(forked["source_session_id"], source.as_str());
    assert_eq!(forked["message_count"], BOUNDARY);
    rig.assert_exact_prefix(&child, &source).await;
    assert_eq!(rig.member_session(CHILD).await, child);
    assert_eq!(rig.member_session(MAIN).await, source);
    assert_eq!(rig.history(&source).await, three_exchanges());
    assert_eq!(
        rig.roster().await,
        vec![MAIN.to_string(), CHILD.to_string()]
    );
    let entry = rig
        .mob()
        .await
        .get_member(&meerkat_mob::AgentIdentity::from(CHILD))
        .await
        .expect("roster read")
        .expect("the child is seated");
    assert_eq!(
        entry
            .fork_source
            .expect("the child records its fork source")
            .source_session_id
            .to_string(),
        source
    );

    rig.client.tools.lock().unwrap().clear();
    rig.member_turn(CHILD, "child turn").await;
    assert_denies_peers(&rig, "the fork child keeps its source's deny list");

    let held = {
        let router_turn = rig.call(
            "mob/turn_start",
            json!({ "mob_id": MOB_ID, "agent_identity": MAIN, "prompt": HOLD }),
        );
        let entered = rig.client.entered.notified();
        tokio::pin!(router_turn);
        tokio::select! {
            () = entered => {}
            response = &mut router_turn => panic!("the held turn finished early: {:?}", response.error),
        }
        let refused = fork("main-while-running").await;
        let error = refused
            .error
            .expect("a fork of a member whose turn is in flight is refused");
        assert_eq!(
            error.code,
            meerkat_rpc::error::SESSION_BUSY,
            "a running source answers SESSION_BUSY: {}",
            error.message
        );
        let data = error
            .data
            .as_ref()
            .unwrap_or_else(|| panic!("the refusal is typed: {}", error.message));
        assert_eq!(data["kind"], "fork_source_unavailable", "{data}");
        assert_eq!(data["cause"], "running", "{data}");
        assert_eq!(data["source_member_id"], MAIN, "{data}");
        assert_eq!(
            rig.roster().await,
            vec![MAIN.to_string(), CHILD.to_string()],
            "the refused fork seats nothing"
        );
        assert_eq!(rig.member_session(MAIN).await, source);
        rig.client.release.notify_one();
        tokio::time::timeout(Duration::from_secs(30), router_turn)
            .await
            .expect("the held turn finishes once released")
    };
    assert!(held.error.is_none(), "{:?}", held.error);
}

/// The identity `main` cannot be re-seated on another member's session. The
/// in-process fork child and `main` are retired, and `mob/spawn` of `main`
/// with `launch_mode = resume` on the child's session is refused: the session
/// is durably bound to the child's identity. The refusal is typed invalid
/// params naming the session and the member it is bound to, and seats
/// nothing.
#[tokio::test]
async fn main_cannot_be_reseated_on_another_members_session() {
    let rig = rig();
    let source = rig.seat_main(None).await;
    for prompt in PROMPTS {
        rig.member_turn(MAIN, prompt).await;
    }
    let handle = rig.mob().await;
    let fork = handle
        .fork_member(
            &meerkat_mob::AgentIdentity::from(MAIN),
            child_spec("main-rewound"),
            Some(BOUNDARY),
        )
        .await
        .expect("the in-process exact-prefix fork");
    let child = fork.session_id.to_string();
    rig.assert_exact_prefix(&child, &source).await;
    for identity in ["main-rewound", MAIN] {
        rig.ok(
            "mob/retire",
            json!({ "mob_id": MOB_ID, "agent_identity": identity }),
        )
        .await;
    }
    assert!(rig.roster().await.is_empty(), "both members are retired");

    let reseat = rig
        .call(
            "mob/spawn",
            json!({
                "mob_id": MOB_ID,
                "profile": "main",
                "agent_identity": MAIN,
                "runtime_mode": "turn_driven",
                "launch_mode": { "mode": "resume", "bridge_session_id": &child }
            }),
        )
        .await;
    let error = reseat
        .error
        .expect("`main` cannot be seated on the fork child's session");
    assert_eq!(error.code, meerkat_rpc::error::INVALID_PARAMS);
    let data = error
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("the refusal is typed: {}", error.message));
    assert_eq!(
        data["kind"], "resume_session_bound_to_another_member",
        "{data}"
    );
    assert_eq!(data["session_id"], child.as_str(), "{data}");
    assert_eq!(data["member_id"], MAIN, "{data}");
    assert_eq!(data["bound_mob_id"], MOB_ID, "{data}");
    assert_eq!(data["bound_member_id"], "main-rewound", "{data}");
    assert!(
        error.message.contains("main-rewound") && !error.message.contains("MobMemberBinding"),
        "the message names the member the session is bound to, without a debug rendering: {}",
        error.message
    );
    assert!(rig.roster().await.is_empty(), "the refusal seats nothing");

    // Control: `main` can be seated again on its own session.
    rig.ok(
        "mob/spawn",
        json!({
            "mob_id": MOB_ID,
            "profile": "main",
            "agent_identity": MAIN,
            "runtime_mode": "turn_driven",
            "launch_mode": { "mode": "resume", "bridge_session_id": &source }
        }),
    )
    .await;
    assert_eq!(rig.member_session(MAIN).await, source);
}

/// Intended: a member's session forked at its second user message through a
/// mob-owned RPC method becomes the identity's new session, has exactly the prefix as its
/// history (the source's system prompt and the first exchange), reports
/// `message_count`, records its lineage and keeps the source's tool
/// restrictions. A fork requested while the member's turn is in flight is
/// refused with `SESSION_BUSY` and seats nothing (the in-process fork's
/// `ForkSourceUnavailableCause::Running`, pinned by
/// [`today_the_exact_prefix_member_fork_exists_only_in_process`], mapped
/// onto the transport).
///
/// The method does not exist; [`fork_member_at`] stands in for it and fails.
#[tokio::test]
#[ignore = "no RPC method forks a member's session at a message boundary (proposed: mob/fork_member_at), see the PR"]
async fn a_member_session_forks_at_a_message_boundary() {
    let rig = rig();
    let source = rig.seat_main(Some(deny_peers())).await;
    for prompt in PROMPTS {
        rig.member_turn(MAIN, prompt).await;
    }
    assert_eq!(rig.history(&source).await, three_exchanges());
    assert_denies_peers(&rig, "setup: the source's deny list took effect");

    let forked = fork_member_at(&rig, MAIN, BOUNDARY)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "fork the member at boundary {BOUNDARY}: {} ({})",
                error.message, error.code
            )
        });
    let child = forked["session_id"].as_str().unwrap().to_string();
    assert_ne!(child, source);
    assert_eq!(forked["source_session_id"], source.as_str());
    assert_eq!(forked["message_count"], BOUNDARY);
    rig.assert_exact_prefix(&child, &source).await;
    assert_eq!(rig.history(&source).await, three_exchanges());
    assert_eq!(rig.member_session(MAIN).await, child);

    rig.client.tools.lock().unwrap().clear();
    rig.member_turn(MAIN, "after rewind").await;
    assert_denies_peers(&rig, "the child keeps the source's tool restrictions");
    let entry = rig
        .mob()
        .await
        .get_member(&meerkat_mob::AgentIdentity::from(MAIN))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        entry
            .fork_source
            .expect("lineage")
            .source_session_id
            .to_string(),
        source
    );

    // A running member is refused with SESSION_BUSY over RPC and nothing is
    // seated or rebound.
    let child_history = rig.history(&child).await;
    let held = {
        let router_turn = rig.call(
            "mob/turn_start",
            json!({ "mob_id": MOB_ID, "agent_identity": MAIN, "prompt": HOLD }),
        );
        let entered = rig.client.entered.notified();
        tokio::pin!(router_turn);
        tokio::select! {
            () = entered => {}
            response = &mut router_turn => panic!("the held turn finished early: {:?}", response.error),
        }
        let refused = fork_member_at(&rig, MAIN, BOUNDARY).await;
        let error = refused.expect_err("a fork of a running member is refused");
        assert_eq!(
            error.code,
            meerkat_rpc::error::SESSION_BUSY,
            "a running source maps to SESSION_BUSY: {}",
            error.message
        );
        assert_eq!(
            rig.roster().await,
            vec![MAIN.to_string()],
            "the refused fork seats nothing"
        );
        assert_eq!(
            rig.member_session(MAIN).await,
            child,
            "the refused fork does not rebind the identity"
        );
        rig.client.release.notify_one();
        tokio::time::timeout(Duration::from_secs(30), router_turn)
            .await
            .expect("the held turn finishes once released")
    };
    assert!(held.error.is_none(), "{:?}", held.error);
    assert!(
        rig.history(&child).await.starts_with(&child_history),
        "the refused fork leaves the member's session as it was"
    );
}

/// Stand-in for the proposed `mob/fork_member_at {mob_id, agent_identity,
/// message_index, inherited_instructions}`: a new session for the same
/// identity whose history is exactly the prefix (the source's system prompt
/// copied), lineage recorded, the source's tool restrictions kept, the
/// response carrying `session_id`, `source_session_id` and `message_count`;
/// an idle source only (`SESSION_BUSY` for a running one, nothing seated).
/// Replace the body with the call, returning the response's result or its
/// error.
async fn fork_member_at(
    _rig: &Rig,
    _identity: &str,
    _message_index: usize,
) -> Result<Value, RpcError> {
    Err(RpcError {
        code: meerkat_rpc::error::METHOD_NOT_FOUND,
        message: "no RPC method forks a member's session at a message boundary (proposed: \
                  mob/fork_member_at {mob_id, agent_identity, message_index, \
                  inherited_instructions} -> {session_id, source_session_id, message_count}; \
                  SESSION_BUSY for a running member, nothing seated)"
            .to_string(),
        data: None,
    })
}
