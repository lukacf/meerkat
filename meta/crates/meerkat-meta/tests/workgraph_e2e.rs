//! WorkGraph end-to-end: a persistent UnifiedRuntime whose member profile
//! opts into `tools.workgraph` builds with the full `workgraph_*` tool
//! surface, and a goal created over RPC against the member's IDENTITY makes
//! the apply-time attention overlay reach a real member turn — observed at
//! the LLM boundary, where the turn-scoped tool overlay hard-filters the
//! provider-visible tool list down to the binding mode's allow-set.
//!
//! The rows after it are about a member's OWN WorkGraph calls: the member's
//! model calls a `workgraph_*` tool on its turn, through the tools the
//! member was built with, and the row reads what the tool answered and what
//! the WorkGraph then holds. They pin that a member's call is refused where
//! the host's `meta/workgraph/*` method with the same target is refused.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::{UnifiedRuntime, handle_unified_rpc_json};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};
use serde_json::{Value, json};

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

/// Profile opted into workgraph tools; comms on so console sends deliver.
const WORKGRAPH_E2E_TOML: &str = r#"
[mob]
id = "workgraph-e2e-mob"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
workgraph = true
"#;

/// LLM stub recording every request's tool names, so tests can assert on the
/// member's provider-visible tool surface per turn.
#[derive(Clone, Default)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    notify: Arc<tokio::sync::Notify>,
}

impl CaptureClient {
    fn captured(&self) -> Vec<Vec<String>> {
        self.requests.lock().unwrap().clone()
    }

    async fn wait_for_request(&self, minimum: usize) -> Vec<Vec<String>> {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let captured = self.captured();
                if captured.len() >= minimum {
                    return captured;
                }
                self.notify.notified().await;
            }
        })
        .await
        .expect("member turn should reach the LLM client")
    }
}

impl LlmClient for CaptureClient {
    // Custom test LlmClients must override this or turns fail before
    // stream() on replayed transcripts.
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let tool_names = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        self.requests.lock().unwrap().push(tool_names);
        self.notify.notify_waiters();
        // meerkat 0.8.22 rejects a turn whose stream carried no normalized
        // provider accounting, so the terminal `Done` never travels alone.
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn rpc(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": "wg-e2e",
        "method": method,
        "params": params,
    })
    .to_string();
    let response =
        handle_unified_rpc_json(runtime, &request, Duration::from_secs(5), None, None).await;
    serde_json::from_str(&response).expect("rpc response json")
}

fn has_tool(tools: &[String], name: &str) -> bool {
    tools.iter().any(|tool| tool == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn member_tools_and_attention_overlay_reach_live_turns() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let client = CaptureClient::default();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(MobDefinition::from_toml(WORKGRAPH_E2E_TOML).expect("parse e2e definition"))
            .persistent_state(state_dir.path())
            .default_llm_client(Arc::new(client.clone()))
            .build(),
    )
    .await
    .expect("persistent workgraph runtime builds");

    // The persistent builder path opens the durable store beside the
    // runtime DB.
    assert!(
        state_dir
            .path()
            .join(meerkat_meta::workgraph_wiring::WORKGRAPH_STORE_FILE)
            .exists(),
        "persistent runtimes must open workgraph.sqlite3"
    );

    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn workgraph-enabled member");

    // Turn 1 — no attention binding: the profile gate composes the FULL
    // workgraph tool surface into the member build.
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("first send");
    let captured = client.wait_for_request(1).await;
    let unbound_tools = &captured[0];
    for tool in [
        "workgraph_create",
        "workgraph_get",
        "workgraph_claim",
        "workgraph_close",
        "workgraph_attention_reassign",
    ] {
        assert!(
            has_tool(unbound_tools, tool),
            "profile tools.workgraph=true must compose {tool}: {unbound_tools:?}"
        );
    }
    assert!(
        has_tool(unbound_tools, "send_message"),
        "comms tools present on unbound turns: {unbound_tools:?}"
    );
    // The stock tool `ask_user` is in the member's list once, and mounting
    // it leaves the member's catalog as it was: the whole WorkGraph surface
    // is offered inline, and the catalog's control tools are not composed.
    assert_eq!(
        unbound_tools
            .iter()
            .filter(|tool| tool.as_str() == "ask_user")
            .count(),
        1,
        "the member is offered ask_user once: {unbound_tools:?}"
    );
    for control in ["tool_catalog_search", "tool_catalog_load"] {
        assert!(
            !has_tool(unbound_tools, control),
            "mounting ask_user must not move the member's tools behind the tool catalog \
             ({control} is offered): {unbound_tools:?}"
        );
    }

    // Bind a goal to the member IDENTITY over RPC (default mode: pursue).
    let response = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "watch the queue",
            "target": { "kind": "identity", "identity": "helper" },
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    let binding_id = response["result"]["attention"]["binding_id"]
        .as_str()
        .expect("binding id")
        .to_string();
    assert_eq!(
        response["result"]["attention"]["target"]["owner_key"]["id"],
        json!("mob/workgraph-e2e-mob/agent/helper"),
        "identity target lowers against the runtime's mob id"
    );

    // goal/status + attention/list expose the binding for diagnosability
    // (MultipleActiveBindings is a hard per-turn error upstream).
    let status = rpc(
        &runtime,
        "meta/workgraph/goal/status",
        json!({ "binding_id": binding_id }),
    )
    .await;
    assert_eq!(
        status["result"]["attention"]["status"]["state"],
        json!("active")
    );
    let listed = rpc(&runtime, "meta/workgraph/attention/list", json!({})).await;
    assert_eq!(
        listed["result"]["attention"].as_array().unwrap().len(),
        1,
        "{listed:#?}"
    );

    // Turn 2 — the apply-time overlay reaches the member turn: the
    // provider-visible tool list is hard-filtered to the pursue-mode
    // allow-set (workgraph_get yes; create/claim/reassign no; comms tools
    // filtered too — the turn is scoped to the attention binding).
    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "helper",
        "how is the queue?".to_string(),
    )
    .await
    .expect("second send");
    let captured = client.wait_for_request(2).await;
    let bound_tools = &captured[1];
    assert!(
        has_tool(bound_tools, "workgraph_get"),
        "pursue allow-set must include workgraph_get: {bound_tools:?}"
    );
    assert!(
        has_tool(bound_tools, "workgraph_add_evidence"),
        "pursue allow-set must include workgraph_add_evidence: {bound_tools:?}"
    );
    for blocked in [
        "workgraph_create",
        "workgraph_claim",
        "workgraph_attention_reassign",
        "send_message",
    ] {
        assert!(
            !has_tool(bound_tools, blocked),
            "attention-scoped turns must not expose {blocked}: {bound_tools:?}"
        );
    }
    assert!(
        bound_tools.len() < unbound_tools.len(),
        "the overlay must narrow the tool surface: {} -> {}",
        unbound_tools.len(),
        bound_tools.len()
    );

    // Pausing the binding lifts the overlay: the next turn sees the full
    // surface again.
    let binding_revision = status["result"]["attention"]["machine_state"]["revision"]
        .as_u64()
        .expect("binding revision");
    let paused = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({ "binding_id": binding_id, "expected_revision": binding_revision }),
    )
    .await;
    assert!(paused["error"].is_null(), "{paused:#?}");

    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "helper",
        "back to normal".to_string(),
    )
    .await
    .expect("third send");
    let captured = client.wait_for_request(3).await;
    let unpaused_tools = &captured[2];
    assert!(
        has_tool(unpaused_tools, "workgraph_create") && has_tool(unpaused_tools, "send_message"),
        "pausing the binding must restore the full tool surface: {unpaused_tools:?}"
    );

    // Teardown is not this test's subject - the tool surface is - so it goes
    // through the degrading teardown path. Raw `MobHandle::stop()` here made
    // this test fail intermittently on a readiness state it cannot influence
    // (upstream P1: provisioner `interrupt_member` refuses with
    // `Runtime not ready: attached` while a member's runtime session is still
    // mid-kickoff). The raw path is still exercised deliberately, in
    // `unified_runtime::lifecycle::stop_degrade_tests`; do not convert that one.
    // Bind the outcome so a refusal PRINTS the error it carries.
    // `MobStopOutcome::Failed` holds the underlying error, and asserting on the
    // bare predicate discarded it: an exact-main failure here reported only
    // "teardown must not be blocked", which names the expectation and not the
    // cause, so the next occurrence starts from nothing.
    let teardown = runtime.stop_mob_for_teardown().await;
    assert!(
        teardown.teardown_may_proceed(),
        "teardown must not be blocked by a member's runtime readiness: {teardown:?}"
    );
}

// ---------------------------------------------------------------------------
// A member's own WorkGraph calls
// ---------------------------------------------------------------------------

/// The definition of the mob `mob_id`: one profile with the WorkGraph tools.
fn member_calls_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
workgraph = true
"#
    ))
    .expect("parse the member calls definition")
}

/// One tool result as a model request carried it.
#[derive(Debug, Clone)]
struct ToolAnswer {
    content: Value,
    is_error: bool,
}

/// One model request.
#[derive(Debug, Clone)]
struct Asked {
    /// The text of every user message of the request, in order.
    said: Vec<String>,
    /// The tool results that follow the newest message that names a call.
    answers: Vec<ToolAnswer>,
    /// Whether the model ended the turn on this request.
    ended: bool,
}

/// A call a message asks the model to make: `[[call TOOL]]ARGS[[end]]` makes
/// the model call `TOOL` with `ARGS` once.
struct Directive {
    tool: String,
    arguments: Value,
}

fn directive(text: &str) -> Option<Directive> {
    let (_, rest) = text.rsplit_once("[[call ")?;
    let (tool, rest) = rest.split_once("]]")?;
    let (arguments, _) = rest.split_once("[[end]]")?;
    Some(Directive {
        tool: tool.trim().to_owned(),
        arguments: serde_json::from_str(arguments).ok()?,
    })
}

/// Every text a message carries, in the order it serializes them.
fn texts_of(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) => texts.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| texts_of(item, texts)),
        Value::Object(fields) => fields.values().for_each(|field| texts_of(field, texts)),
        _ => {}
    }
}

/// The message, marked `mark`, that makes a member's model call `tool` with
/// `arguments`.
fn call_message(mark: &str, tool: &str, arguments: &Value) -> String {
    format!("{mark} [[call {tool}]]{arguments}[[end]]")
}

/// The members' model: it makes the call its newest message names, once, and
/// otherwise answers in words. It records every request.
#[derive(Clone, Default)]
struct CallingModel {
    asked: Arc<std::sync::Mutex<Vec<Asked>>>,
    notify: Arc<tokio::sync::Notify>,
}

impl CallingModel {
    /// The request on which the turn of the message marked `mark` ended.
    async fn turn_ended(&self, mark: &str) -> Asked {
        tokio::time::timeout(Duration::from_mins(1), async {
            loop {
                let notified = self.notify.notified();
                let found = self
                    .asked
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|asked| asked.ended && asked.said.iter().any(|text| text.contains(mark)))
                    .cloned();
                if let Some(asked) = found {
                    return asked;
                }
                notified.await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the turn of the message marked {mark} did not end at the model; its requests: \
                 {:#?}",
                self.asked.lock().unwrap()
            )
        })
    }
}

impl LlmClient for CallingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let mut said = Vec::new();
        let mut call = None;
        let mut answers = Vec::new();
        for message in &request.messages {
            match message {
                // What a member is told arrives as a user message (its
                // kickoff) or as a notice (a message sent to it).
                Message::User(_) | Message::SystemNotice(_) => {
                    let mut texts = Vec::new();
                    texts_of(
                        &serde_json::to_value(message).expect("message json"),
                        &mut texts,
                    );
                    for text in texts {
                        if let Some(named) = directive(&text) {
                            call = Some(named);
                            answers.clear();
                        }
                        said.push(text);
                    }
                }
                Message::ToolResults { results, .. } => {
                    answers.extend(results.iter().map(|result| {
                        ToolAnswer {
                            content: serde_json::from_str(&result.text_content())
                                .unwrap_or_else(|_| Value::String(result.text_content())),
                            is_error: result.is_error,
                        }
                    }));
                }
                _ => {}
            }
        }
        let call = call.filter(|_| answers.is_empty());
        self.asked.lock().unwrap().push(Asked {
            said,
            answers,
            ended: call.is_none(),
        });
        self.notify.notify_waiters();
        let (lead, stop) = match call {
            Some(named) => (
                LlmEvent::ToolCallComplete {
                    id: "fc_0".to_string(),
                    name: named.tool,
                    args: named.arguments,
                    meta: None,
                },
                StopReason::ToolUse,
            ),
            None => (
                LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                StopReason::EndTurn,
            ),
        };
        let [usage, done] = llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, stop);
        Box::pin(async_stream::stream! {
            yield Ok(lead);
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// A persistent runtime of the mob `mob_id` whose members all run `model`.
async fn member_calls_runtime(
    mob_id: &str,
    state_dir: &Path,
    model: &CallingModel,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(member_calls_definition(mob_id))
            .persistent_state(state_dir)
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("the member calls runtime builds")
}

async fn spawn_worker(runtime: &UnifiedRuntime, name: &str) {
    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            name.to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn a member with the WorkGraph tools");
}

/// A member private to a person: the label a side chat carries.
async fn spawn_private_worker(runtime: &UnifiedRuntime, name: &str) {
    runtime
        .mob_handle()
        .spawn_spec(
            SpawnMemberSpec::new("worker", name).with_labels(BTreeMap::from([(
                meerkat_meta::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:someone".to_string(),
            )])),
        )
        .await
        .expect("spawn a member private to a person");
}

async fn session_of(runtime: &UnifiedRuntime, name: &str) -> String {
    runtime
        .mob_handle()
        .resolve_bridge_session_id_observation(&meerkat_mob::AgentIdentity::from(name))
        .await
        .unwrap_or_else(|| panic!("member {name} has a session"))
        .to_string()
}

fn owner_key(mob_id: &str, name: &str) -> Value {
    json!({ "kind": "agent", "id": format!("mob/{mob_id}/agent/{name}") })
}

fn owner_target(mob_id: &str, name: &str) -> Value {
    json!({ "kind": "owner", "owner_key": owner_key(mob_id, name) })
}

/// A goal the host binds to `member`, in `mode`. Returns the goal's result.
async fn host_goal(runtime: &UnifiedRuntime, title: &str, member: &str, mode: &str) -> Value {
    let response = rpc(
        runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": title,
            "target": { "kind": "identity", "identity": member },
            "mode": mode,
        }),
    )
    .await;
    assert!(response["error"].is_null(), "{response:#?}");
    response["result"].clone()
}

fn binding_id(goal: &Value) -> String {
    goal["attention"]["binding_id"]
        .as_str()
        .expect("binding id")
        .to_string()
}

fn binding_revision(goal: &Value) -> u64 {
    goal["attention"]["machine_state"]["revision"]
        .as_u64()
        .expect("binding revision")
}

/// The arguments of a member's `workgraph_attention_reassign` of `goal`'s
/// binding onto `target`.
fn reassign_onto(goal: &Value, target: Value) -> Value {
    json!({
        "binding_id": binding_id(goal),
        "expected_revision": binding_revision(goal),
        "target": target,
    })
}

/// Sends `member` the message that makes its model call `tool` with
/// `arguments`, and returns what the tool answered the member.
async fn member_calls(
    runtime: &UnifiedRuntime,
    model: &CallingModel,
    member: &str,
    mark: &str,
    tool: &str,
    arguments: &Value,
) -> ToolAnswer {
    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        member,
        call_message(mark, tool, arguments),
    )
    .await
    .expect("send the member its message");
    let turn = model.turn_ended(mark).await;
    turn.answers
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the model of {member} made no call on its turn: {turn:?}"))
}

/// The attention bindings the host lists.
async fn bindings(runtime: &UnifiedRuntime) -> Vec<Value> {
    let listed = rpc(runtime, "meta/workgraph/attention/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed:#?}");
    listed["result"]["attention"]
        .as_array()
        .expect("attention list")
        .clone()
}

fn binding<'a>(bindings: &'a [Value], id: &str) -> &'a Value {
    bindings
        .iter()
        .find(|binding| binding["binding_id"] == json!(id))
        .unwrap_or_else(|| panic!("binding {id} is listed: {bindings:#?}"))
}

/// The bindings that occupy a target: active or paused.
fn occupying(bindings: &[Value]) -> Vec<&Value> {
    bindings
        .iter()
        .filter(|binding| {
            matches!(
                binding["status"]["state"].as_str(),
                Some("active" | "paused")
            )
        })
        .collect()
}

/// Everything a reader of the WorkGraph is shown: the attention list, the
/// snapshot and the events.
async fn everything_readers_see(runtime: &UnifiedRuntime) -> String {
    let mut seen = String::new();
    for method in [
        "meta/workgraph/attention/list",
        "meta/workgraph/snapshot",
        "meta/workgraph/events",
    ] {
        let response = rpc(runtime, method, json!({})).await;
        assert!(response["error"].is_null(), "{method}: {response:#?}");
        seen.push_str(&response.to_string());
    }
    seen
}

/// The one answer a member's tools give for a key of the mob's member form
/// that names no member they may bind: a policy denial that says no more
/// than that.
fn is_no_bindable_member_denial(answer: &ToolAnswer) -> bool {
    answer.is_error
        && answer.content["error"] == json!("policy_denied")
        && answer.content["data"]["code"] == json!("workgraph_member_not_bindable")
}

fn is_workgraph_conflict(answer: &ToolAnswer) -> bool {
    answer.is_error && answer.content["data"]["kind"] == json!("workgraph_conflict")
}

async fn teardown(runtime: UnifiedRuntime) {
    let teardown = runtime.stop_mob_for_teardown().await;
    assert!(
        teardown.teardown_may_proceed(),
        "teardown must not be blocked by a member's runtime readiness: {teardown:?}"
    );
}

/// A member bound to a goal may move the goal's attention with its own
/// `workgraph_attention_reassign`. Onto a member private to a person the
/// host's `meta/workgraph/attention/reassign` is refused
/// (`private_member_workgraph_target`), and so is the member's own call, by
/// the member's owner key and by its session: the tool answers a policy
/// denial, the binding stays where it was, no reader of the WorkGraph is
/// shown anything that names the private member, and the private member
/// still answers its owner.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_reassign_onto_a_member_private_to_a_person_is_refused() {
    const MOB: &str = "workgraph-member-calls-private-target";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_private_worker(&runtime, "private-helper").await;
    let private_session = session_of(&runtime, "private-helper").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;

    let by_owner_key = member_calls(
        &runtime,
        &model,
        "coord",
        "MOVE-BY-OWNER-KEY",
        "workgraph_attention_reassign",
        &reassign_onto(&goal, owner_target(MOB, "private-helper")),
    )
    .await;
    assert!(
        is_no_bindable_member_denial(&by_owner_key),
        "a member's reassign onto a member private to a person, named by its owner key, is a \
         policy denial: {by_owner_key:?}"
    );
    let by_session = member_calls(
        &runtime,
        &model,
        "coord",
        "MOVE-BY-SESSION",
        "workgraph_attention_reassign",
        &reassign_onto(
            &goal,
            json!({ "kind": "session", "session_id": private_session }),
        ),
    )
    .await;
    assert!(
        is_no_bindable_member_denial(&by_session),
        "a member's reassign onto a member private to a person, named by its session, is a \
         policy denial: {by_session:?}"
    );

    let listed = bindings(&runtime).await;
    assert_eq!(listed.len(), 1, "no binding was made: {listed:#?}");
    let kept = binding(&listed, &binding_id(&goal));
    assert_eq!(kept["status"]["state"], json!("active"), "{kept:#?}");
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "the binding stays on the member the host bound: {kept:#?}"
    );
    let seen = everything_readers_see(&runtime).await;
    assert!(
        !seen.contains("private-helper") && !seen.contains(&private_session),
        "nothing a reader of the WorkGraph is shown names the private member: {seen}"
    );

    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "private-helper",
        "OWNER-ASKS are you there?".to_string(),
    )
    .await
    .expect("the owner writes to the private member");
    model.turn_ended("OWNER-ASKS").await;

    teardown(runtime).await;
}

/// A member bound to a goal may escalate the goal's completion policy with
/// its own `workgraph_policy_escalate`. A supervisor that is a member
/// private to a person is refused to the host's
/// `meta/workgraph/policy/escalate`, and so it is to the member's own call:
/// the tool answers a policy denial and the goal's policy names no private
/// member.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_escalation_to_a_supervisor_private_to_a_person_is_refused() {
    const MOB: &str = "workgraph-member-calls-private-supervisor";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_private_worker(&runtime, "private-helper").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;

    let escalated = member_calls(
        &runtime,
        &model,
        "coord",
        "ESCALATE",
        "workgraph_policy_escalate",
        &json!({
            "id": goal["item"]["id"],
            "expected_revision": goal["item"]["revision"],
            "completion_policy": {
                "kind": "supervisor",
                "owner_key": owner_key(MOB, "private-helper"),
            },
        }),
    )
    .await;
    assert!(
        is_no_bindable_member_denial(&escalated),
        "a member's escalation to a supervisor private to a person is a policy denial: \
         {escalated:?}"
    );
    let seen = everything_readers_see(&runtime).await;
    assert!(
        !seen.contains("private-helper"),
        "the goal's policy names no private member: {seen}"
    );

    teardown(runtime).await;
}

/// A member that is the target of an attention binding takes no second one:
/// the host's reassign onto it answers `workgraph_conflict`. A member's own
/// reassign onto it answers the same when it names the occupied member by
/// its SESSION while the occupying binding names it by its owner key.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_reassign_onto_an_occupied_member_by_its_session_is_a_conflict() {
    const MOB: &str = "workgraph-member-calls-occupied-session";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_worker(&runtime, "helper").await;
    let helper_session = session_of(&runtime, "helper").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;
    host_goal(&runtime, "watch the queue", "helper", "pursue").await;

    let moved = member_calls(
        &runtime,
        &model,
        "coord",
        "MOVE-ONTO-OCCUPIED",
        "workgraph_attention_reassign",
        &reassign_onto(
            &goal,
            json!({ "kind": "session", "session_id": helper_session }),
        ),
    )
    .await;
    assert!(
        is_workgraph_conflict(&moved),
        "a member's reassign onto an occupied member, named by its session, is a conflict: \
         {moved:?}"
    );
    let listed = bindings(&runtime).await;
    assert_eq!(
        occupying(&listed).len(),
        2,
        "each member keeps its one binding: {listed:#?}"
    );
    let kept = binding(&listed, &binding_id(&goal));
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "{kept:#?}"
    );

    teardown(runtime).await;
}

/// A paused binding still occupies its target (a pause ends, and the
/// binding is active again): the host's reassign onto that member answers
/// `workgraph_conflict`, and so does a member's own.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_reassign_onto_a_member_whose_binding_is_paused_is_a_conflict() {
    const MOB: &str = "workgraph-member-calls-paused-occupant";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_worker(&runtime, "helper").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;
    let occupant = host_goal(&runtime, "watch the queue", "helper", "pursue").await;
    let paused = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({
            "binding_id": binding_id(&occupant),
            "expected_revision": binding_revision(&occupant),
        }),
    )
    .await;
    assert!(paused["error"].is_null(), "{paused:#?}");

    let moved = member_calls(
        &runtime,
        &model,
        "coord",
        "MOVE-ONTO-PAUSED",
        "workgraph_attention_reassign",
        &reassign_onto(&goal, owner_target(MOB, "helper")),
    )
    .await;
    assert!(
        is_workgraph_conflict(&moved),
        "a member's reassign onto a member whose binding is paused is a conflict: {moved:?}"
    );
    let listed = bindings(&runtime).await;
    let kept = binding(&listed, &binding_id(&goal));
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "{kept:#?}"
    );
    assert_eq!(occupying(&listed).len(), 2, "{listed:#?}");

    teardown(runtime).await;
}

/// One session has two spellings as an attention target: the session
/// target, and an owner key of kind `session`. A session that a binding
/// occupies under one spelling takes no second binding under the other:
/// the host's reassign answers `workgraph_conflict`, and so does a member's
/// own.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_reassign_onto_the_other_spelling_of_an_occupied_session_is_a_conflict() {
    const MOB: &str = "workgraph-member-calls-session-spellings";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;
    // A session no member of the mob holds.
    let session = meerkat_core::SessionId::new().to_string();
    let occupant = rpc(
        &runtime,
        "meta/workgraph/goal/create",
        json!({
            "title": "a goal of a session outside the mob",
            "target": { "kind": "session", "session_id": session },
        }),
    )
    .await;
    assert!(occupant["error"].is_null(), "{occupant:#?}");

    let moved = member_calls(
        &runtime,
        &model,
        "coord",
        "MOVE-ONTO-OTHER-SPELLING",
        "workgraph_attention_reassign",
        &reassign_onto(
            &goal,
            json!({ "kind": "owner", "owner_key": { "kind": "session", "id": session } }),
        ),
    )
    .await;
    assert!(
        is_workgraph_conflict(&moved),
        "a member's reassign onto an occupied session under its other spelling is a conflict: \
         {moved:?}"
    );
    let listed = bindings(&runtime).await;
    let kept = binding(&listed, &binding_id(&goal));
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "{kept:#?}"
    );
    assert_eq!(occupying(&listed).len(), 2, "{listed:#?}");

    teardown(runtime).await;
}

/// A member's tools answer one way for every key of the mob's member form
/// that names no member they may bind: a member private to a person and a
/// name no member holds get the same policy denial, byte for byte, on a
/// reassign and on an escalation, and nothing is written. So the answer
/// does not tell a hidden name from a free one. The host's own reassign
/// onto a name no member holds is taken, as before (a goal may be bound to
/// an agent before its member exists).
#[tokio::test(flavor = "multi_thread")]
async fn a_members_tools_answer_alike_for_a_private_name_and_a_name_no_member_holds() {
    const MOB: &str = "workgraph-member-calls-absent-name";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_worker(&runtime, "helper").await;
    spawn_private_worker(&runtime, "private-helper").await;
    let members_goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;
    let hosts_goal = host_goal(&runtime, "coordinate the backlog", "helper", "coordinate").await;

    let by_host = rpc(
        &runtime,
        "meta/workgraph/attention/reassign",
        json!({
            "binding_id": binding_id(&hosts_goal),
            "expected_revision": binding_revision(&hosts_goal),
            "target": { "kind": "identity", "identity": "not-yet-a-member" },
        }),
    )
    .await;
    assert!(
        by_host["error"].is_null(),
        "the host binds an agent no member is: {by_host:#?}"
    );

    let mut answers = Vec::new();
    for (mark, tool, arguments) in [
        (
            "MOVE-ONTO-PRIVATE",
            "workgraph_attention_reassign",
            reassign_onto(&members_goal, owner_target(MOB, "private-helper")),
        ),
        (
            "MOVE-ONTO-ABSENT",
            "workgraph_attention_reassign",
            reassign_onto(&members_goal, owner_target(MOB, "not-a-member-either")),
        ),
        (
            "ESCALATE-TO-PRIVATE",
            "workgraph_policy_escalate",
            json!({
                "id": members_goal["item"]["id"],
                "expected_revision": members_goal["item"]["revision"],
                "completion_policy": {
                    "kind": "supervisor",
                    "owner_key": owner_key(MOB, "private-helper"),
                },
            }),
        ),
        (
            "ESCALATE-TO-ABSENT",
            "workgraph_policy_escalate",
            json!({
                "id": members_goal["item"]["id"],
                "expected_revision": members_goal["item"]["revision"],
                "completion_policy": {
                    "kind": "supervisor",
                    "owner_key": owner_key(MOB, "not-a-member-either"),
                },
            }),
        ),
    ] {
        let answer = member_calls(&runtime, &model, "coord", mark, tool, &arguments).await;
        assert!(
            is_no_bindable_member_denial(&answer),
            "{mark}: a key that names no member the tools may bind is a policy denial: \
             {answer:?}"
        );
        let said = answer.content.to_string();
        assert!(
            !said.contains("private")
                && !said.contains("private-helper")
                && !said.contains("not-a-member-either"),
            "{mark}: the denial names no member and does not say why it may not be bound: \
             {said}"
        );
        answers.push((tool, answer.content));
    }
    assert_eq!(
        answers[0], answers[1],
        "a reassign onto a private name and onto a free name answer alike"
    );
    assert_eq!(
        answers[2], answers[3],
        "an escalation to a private name and to a free name answer alike"
    );

    let listed = bindings(&runtime).await;
    let kept = binding(&listed, &binding_id(&members_goal));
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "the member's binding stays where it was: {kept:#?}"
    );
    let seen = everything_readers_see(&runtime).await;
    assert!(
        !seen.contains("private-helper") && !seen.contains("not-a-member-either"),
        "nothing was written under either name: {seen}"
    );

    teardown(runtime).await;
}

/// The same holds where the name already carries a binding: a member's
/// reassign onto an occupied private name and onto an occupied free name
/// answer alike, with the one denial and not with a conflict that names the
/// occupying binding, whether that binding is active or paused.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_tools_answer_alike_for_an_occupied_private_name_and_an_occupied_free_name() {
    const MOB: &str = "workgraph-member-calls-occupied-names";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_private_worker(&runtime, "private-helper").await;
    let members_goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;
    // Each name carries a binding: the free one through the host, the
    // private one written to the WorkGraph itself.
    let on_free = host_goal(&runtime, "work for a later member", "free-name", "pursue").await;
    let service = runtime
        .workgraph_service()
        .expect("the runtime has a WorkGraph");
    let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal bound past the host's methods",
        "target": owner_target(MOB, "private-helper"),
    }))
    .expect("goal request");
    let on_private = service.create_goal(request).await.expect("create goal");
    let on_private_id = serde_json::to_value(&on_private.attention.binding_id)
        .expect("binding id json")
        .as_str()
        .expect("a binding id is a string")
        .to_string();

    for state in ["active", "paused"] {
        if state == "paused" {
            for (id, revision) in [
                (binding_id(&on_free), json!(binding_revision(&on_free))),
                (
                    on_private_id.clone(),
                    json!(on_private.attention.machine_state.revision),
                ),
            ] {
                let paused = rpc(
                    &runtime,
                    "meta/workgraph/attention/pause",
                    json!({ "binding_id": id, "expected_revision": revision }),
                )
                .await;
                assert!(paused["error"].is_null(), "{paused:#?}");
            }
        }
        let onto_private = member_calls(
            &runtime,
            &model,
            "coord",
            &format!("MOVE-ONTO-OCCUPIED-PRIVATE-{state}"),
            "workgraph_attention_reassign",
            &reassign_onto(&members_goal, owner_target(MOB, "private-helper")),
        )
        .await;
        let onto_free = member_calls(
            &runtime,
            &model,
            "coord",
            &format!("MOVE-ONTO-OCCUPIED-FREE-{state}"),
            "workgraph_attention_reassign",
            &reassign_onto(&members_goal, owner_target(MOB, "free-name")),
        )
        .await;
        assert!(
            is_no_bindable_member_denial(&onto_private),
            "{state}: {onto_private:?}"
        );
        assert_eq!(
            onto_private.content, onto_free.content,
            "{state}: an occupied private name and an occupied free name answer alike"
        );
    }

    teardown(runtime).await;
}

/// The guard does not depend on how a call spells its scope. A reassign or
/// an escalation of a bound member that carries a scope argument of a
/// kind Meerkat does not read as a scope (a non-boolean `all_namespaces`,
/// a `realm_id` or a `namespace` that is no string) is refused like the
/// same call without it, and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_call_that_spells_a_scope_value_meets_the_same_refusal() {
    const MOB: &str = "workgraph-member-calls-spelled-scope";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;
    spawn_private_worker(&runtime, "private-helper").await;
    let goal = host_goal(&runtime, "coordinate the queue", "coord", "coordinate").await;

    for (mark, key, value) in [
        ("SPELLED-ALL-NAMESPACES", "all_namespaces", json!("yes")),
        ("SPELLED-REALM", "realm_id", json!(7)),
        ("SPELLED-NAMESPACE", "namespace", json!(["default"])),
    ] {
        let mut arguments = reassign_onto(&goal, owner_target(MOB, "private-helper"));
        arguments[key] = value;
        let answer = member_calls(
            &runtime,
            &model,
            "coord",
            mark,
            "workgraph_attention_reassign",
            &arguments,
        )
        .await;
        assert!(
            is_no_bindable_member_denial(&answer),
            "{mark}: a reassign onto a member private to a person is refused however the \
             call spells its scope: {answer:?}"
        );
    }
    let escalated = member_calls(
        &runtime,
        &model,
        "coord",
        "SPELLED-ESCALATION",
        "workgraph_policy_escalate",
        &json!({
            "id": goal["item"]["id"],
            "expected_revision": goal["item"]["revision"],
            "all_namespaces": 1,
            "completion_policy": {
                "kind": "supervisor",
                "owner_key": owner_key(MOB, "private-helper"),
            },
        }),
    )
    .await;
    assert!(
        is_no_bindable_member_denial(&escalated),
        "an escalation to a supervisor private to a person is refused however the call \
         spells its scope: {escalated:?}"
    );

    let listed = bindings(&runtime).await;
    assert_eq!(listed.len(), 1, "no binding was made: {listed:#?}");
    let kept = binding(&listed, &binding_id(&goal));
    assert_eq!(
        kept["target"]["owner_key"]["id"],
        json!(format!("mob/{MOB}/agent/coord")),
        "{kept:#?}"
    );
    let seen = everything_readers_see(&runtime).await;
    assert!(
        !seen.contains("private-helper"),
        "nothing names the private member: {seen}"
    );

    teardown(runtime).await;
}

/// A key that names a member of another mob is no key of this mob's member
/// form: a member's tools pass the call on as it was made, and what answers
/// is the same for a member of that mob private to a person, for a shared
/// member of it and for a name no member of it holds. A reassign onto such
/// a key is refused, by the key's text alone; an escalation to such a
/// supervisor is taken and binds no attention.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_tools_answer_alike_for_every_name_of_another_mob() {
    const MOB: &str = "workgraph-member-calls-other-mob-keys";
    const OTHER: &str = "workgraph-member-calls-the-other-mob";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;

    // The other mob, with a shared member and a member private to a person.
    let mobs = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime hosts the mobs its members create");
    let other = mobs
        .mob_create_definition(member_calls_definition(OTHER))
        .await
        .expect("create the other mob");
    let there = mobs
        .handle_for(&other)
        .await
        .expect("the other mob's handle");
    there
        .spawn_spec(SpawnMemberSpec::new("worker", "shared-there"))
        .await
        .expect("spawn a shared member of the other mob");
    there
        .spawn_spec(
            SpawnMemberSpec::new("worker", "private-there").with_labels(BTreeMap::from([(
                meerkat_meta::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:someone".to_string(),
            )])),
        )
        .await
        .expect("spawn a member of the other mob private to a person");
    let private_there = there
        .get_member(&meerkat_mob::AgentIdentity::from("private-there"))
        .await
        .expect("read the member of the other mob")
        .expect("the other mob has the member");
    assert!(
        private_there
            .labels
            .contains_key(meerkat_meta::access::PRIVATE_TO_LABEL),
        "the member of the other mob is private to a person: {:?}",
        private_there.labels
    );

    // One coordinator and one goal for each name, so that every call meets
    // a goal in the same state.
    let mut reassigned = Vec::new();
    let mut escalated = Vec::new();
    for (coord, name) in [
        ("coord-a", "private-there"),
        ("coord-b", "shared-there"),
        ("coord-c", "absent-there"),
    ] {
        spawn_worker(&runtime, coord).await;
        let goal = host_goal(&runtime, "coordinate the queue", coord, "coordinate").await;
        let item = goal["item"]["id"].as_str().expect("item id").to_string();
        let binding = binding_id(&goal);
        let session = session_of(&runtime, coord).await;
        // What a call's answer holds of its own: the name it gave, and the
        // caller's own goal, binding, name and session.
        let own = [
            (name, "NAME"),
            (coord, "CALLER"),
            (item.as_str(), "ITEM"),
            (binding.as_str(), "BINDING"),
            (session.as_str(), "SESSION"),
        ];
        let moved = member_calls(
            &runtime,
            &model,
            coord,
            &format!("MOVE-ONTO-OTHER-MOB-{name}"),
            "workgraph_attention_reassign",
            &reassign_onto(&goal, owner_target(OTHER, name)),
        )
        .await;
        reassigned.push((moved.is_error, without_own_words(&moved.content, &own)));
        let raised = member_calls(
            &runtime,
            &model,
            coord,
            &format!("ESCALATE-TO-OTHER-MOB-{name}"),
            "workgraph_policy_escalate",
            &json!({
                "id": goal["item"]["id"],
                "expected_revision": goal["item"]["revision"],
                "completion_policy": {
                    "kind": "supervisor",
                    "owner_key": owner_key(OTHER, name),
                },
            }),
        )
        .await;
        escalated.push((raised.is_error, without_own_words(&raised.content, &own)));
    }

    for (call, answers) in [("reassign", &reassigned), ("escalation", &escalated)] {
        for other_answer in &answers[1..] {
            assert_eq!(
                answers[0].0, other_answer.0,
                "{call}: one name of the other mob is refused and another is not: {answers:#?}"
            );
            let mut found = Vec::new();
            differences(&answers[0].1, &other_answer.1, "", &mut found);
            assert!(
                found.is_empty(),
                "{call}: the answers for two names of the other mob differ at {found:?}: \
                 {answers:#?}"
            );
        }
    }
    assert!(
        reassigned.iter().all(|(is_error, _)| *is_error),
        "a reassign onto a member key of another mob is refused: {reassigned:#?}"
    );
    assert!(
        escalated.iter().all(|(is_error, _)| !*is_error),
        "an escalation to a supervisor key of another mob is taken: {escalated:#?}"
    );
    let listed = bindings(&runtime).await;
    assert_eq!(
        occupying(&listed).len(),
        3,
        "no binding was made or moved: {listed:#?}"
    );
    assert!(
        !serde_json::to_string(&listed)
            .expect("bindings json")
            .contains(OTHER),
        "no binding names the other mob: {listed:#?}"
    );

    teardown(runtime).await;
}

/// `answer` with each word of `own` (a value the call or its caller brought)
/// replaced by its tag, so that two answers can be compared.
fn without_own_words(answer: &Value, own: &[(&str, &str)]) -> Value {
    let mut text = answer.to_string();
    for (word, tag) in own {
        text = text.replace(word, tag);
    }
    serde_json::from_str(&text).expect("an answer without its own words is still JSON")
}

/// Where two answers differ, apart from the bookkeeping every write has of
/// its own: a revision and a time.
fn differences(a: &Value, b: &Value, path: &str, found: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for key in keys {
                match (a.get(key), b.get(key)) {
                    (Some(a), Some(b)) => differences(a, b, &format!("{path}/{key}"), found),
                    _ => found.push(format!("{path}/{key} (in one answer only)")),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                differences(a, b, &format!("{path}/{index}"), found);
            }
        }
        (a, b) if a == b => {}
        (Value::Number(_), Value::Number(_)) if path.ends_with("revision") => {}
        (Value::String(a), Value::String(b)) if is_a_time(a) && is_a_time(b) => {}
        _ => found.push(path.to_string()),
    }
}

/// Whether `text` has the shape of an RFC 3339 time.
fn is_a_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 20 && bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b'T'
}

/// A member's WorkGraph tools work in the member's own mob's scope. A call
/// that names another namespace is refused, and nothing is written; the
/// same call without a namespace is answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_members_own_call_that_names_another_namespace_stays_refused() {
    const MOB: &str = "workgraph-member-calls-other-namespace";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_worker(&runtime, "coord").await;

    let elsewhere = member_calls(
        &runtime,
        &model,
        "coord",
        "CREATE-ELSEWHERE",
        "workgraph_create",
        &json!({ "title": "work elsewhere", "namespace": "another-namespace" }),
    )
    .await;
    assert!(
        elsewhere.is_error,
        "a member's call that names another namespace is refused: {elsewhere:?}"
    );
    let seen = everything_readers_see(&runtime).await;
    assert!(
        !seen.contains("work elsewhere"),
        "the refused call wrote nothing: {seen}"
    );
    let here = member_calls(
        &runtime,
        &model,
        "coord",
        "CREATE-HERE",
        "workgraph_create",
        &json!({ "title": "work here" }),
    )
    .await;
    assert!(
        !here.is_error,
        "the same call without a namespace is answered: {here:?}"
    );

    teardown(runtime).await;
}

/// The host makes no binding onto a member private to a person: its
/// `goal/create` and `attention/reassign` refuse one
/// (`private_member_workgraph_target`). A binding that names such a member
/// all the same and is paused is not resumed either: the resume is refused
/// the same way and the binding stays paused.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_does_not_resume_a_binding_whose_target_is_private_to_a_person() {
    const MOB: &str = "workgraph-member-calls-resume-private";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_private_worker(&runtime, "private-helper").await;
    // The WorkGraph itself knows no private member: a binding written to it
    // directly names one.
    let service = runtime
        .workgraph_service()
        .expect("the runtime has a WorkGraph");
    let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal bound past the host's methods",
        "target": owner_target(MOB, "private-helper"),
    }))
    .expect("goal request");
    let stored = service.create_goal(request).await.expect("create goal");
    let stored_binding = serde_json::to_value(&stored.attention.binding_id)
        .expect("binding id json")
        .as_str()
        .expect("a binding id is a string")
        .to_string();
    let paused = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({
            "binding_id": stored_binding,
            "expected_revision": stored.attention.machine_state.revision,
        }),
    )
    .await;
    assert!(paused["error"].is_null(), "{paused:#?}");
    let listed = bindings(&runtime).await;
    let paused_binding = binding(&listed, &stored_binding);
    assert_eq!(
        paused_binding["status"]["state"],
        json!("paused"),
        "{paused_binding:#?}"
    );
    let paused_revision = paused_binding["machine_state"]["revision"]
        .as_u64()
        .expect("the paused binding's revision");

    let resumed = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({ "binding_id": stored_binding, "expected_revision": paused_revision }),
    )
    .await;
    assert_eq!(
        resumed["error"]["data"]["kind"],
        json!("private_member_workgraph_target"),
        "the host does not resume a binding onto a member private to a person: {resumed:#?}"
    );
    let listed = bindings(&runtime).await;
    assert_eq!(
        binding(&listed, &stored_binding)["status"]["state"],
        json!("paused"),
        "the binding stays paused: {listed:#?}"
    );

    teardown(runtime).await;
}

/// A store may hold an active binding whose target is a member private to
/// a person, from a release whose member tools did not refuse one. While it
/// is active that member's turns take the binding's tools, which it was
/// built without. The launch that speaks for the mob does not keep such a
/// binding active: at its start, before a member takes a turn, the binding
/// is paused, the runtime's storage summary names it, and the private
/// member answers its owner.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_pauses_a_stored_binding_onto_a_member_private_to_a_person() {
    const MOB: &str = "workgraph-member-calls-start-pause";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();

    // The first launch: the private member, and a binding written to the
    // WorkGraph itself, which knows no private member.
    let stored_binding = {
        let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
        spawn_private_worker(&runtime, "private-helper").await;
        let service = runtime
            .workgraph_service()
            .expect("the runtime has a WorkGraph");
        let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
            "title": "a goal bound past the host's methods",
            "target": owner_target(MOB, "private-helper"),
        }))
        .expect("goal request");
        let stored = service.create_goal(request).await.expect("create goal");
        let stored_binding = serde_json::to_value(&stored.attention.binding_id)
            .expect("binding id json")
            .as_str()
            .expect("a binding id is a string")
            .to_string();
        let listed = bindings(&runtime).await;
        assert_eq!(
            binding(&listed, &stored_binding)["status"]["state"],
            json!("active"),
            "the stored binding is active before the restart: {listed:#?}"
        );
        runtime.shutdown().await;
        stored_binding
    };

    // The second launch, on the same state.
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    assert!(
        runtime
            .mob_handle()
            .get_member(&meerkat_mob::AgentIdentity::from("private-helper"))
            .await
            .expect("read the member")
            .is_some(),
        "the restarted runtime has the private member"
    );
    let listed = bindings(&runtime).await;
    assert_eq!(
        binding(&listed, &stored_binding)["status"]["state"],
        json!("paused"),
        "a start pauses a stored binding onto a member private to a person: {listed:#?}"
    );
    let capabilities = rpc(&runtime, "meta/capabilities", json!({})).await;
    assert!(capabilities["error"].is_null(), "{capabilities:#?}");
    assert!(
        capabilities["result"]["storage"]
            .to_string()
            .contains(&stored_binding),
        "the storage summary names the binding the start holds paused: {capabilities:#?}"
    );
    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "private-helper",
        "OWNER-ASKS-AFTER-START are you there?".to_string(),
    )
    .await
    .expect("the owner writes to the private member");
    model.turn_ended("OWNER-ASKS-AFTER-START").await;

    teardown(runtime).await;
}

/// A paused binding with a deadline becomes active again when the deadline
/// passes: Meerkat's attention machine decides that, and no method of the
/// host is asked. For a binding onto a member private to a person that
/// would stop the member's turns again, so a start clears the deadline:
/// the binding is paused with none, until it is reassigned to another
/// member or its goal is closed, and the storage summary names it.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_clears_the_deadline_of_a_paused_binding_onto_a_member_private_to_a_person() {
    const MOB: &str = "workgraph-member-calls-start-deadline";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();

    // The first launch: the private member, and a binding written to the
    // WorkGraph itself and paused there with a deadline, as a store may
    // hold one (the host's own pause refuses a deadline for it).
    let stored_binding = {
        let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
        spawn_private_worker(&runtime, "private-helper").await;
        let service = runtime
            .workgraph_service()
            .expect("the runtime has a WorkGraph");
        let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
            "title": "a goal bound past the host's methods",
            "target": owner_target(MOB, "private-helper"),
        }))
        .expect("goal request");
        let stored = service.create_goal(request).await.expect("create goal");
        let stored_binding = serde_json::to_value(&stored.attention.binding_id)
            .expect("binding id json")
            .as_str()
            .expect("a binding id is a string")
            .to_string();
        let pause: meerkat::AttentionPauseRequest = serde_json::from_value(json!({
            "binding_id": stored_binding,
            "expected_revision": stored.attention.machine_state.revision,
            "until": "2099-01-01T00:00:00Z",
        }))
        .expect("pause request");
        service
            .pause_attention(pause)
            .await
            .expect("pause with a deadline");
        let listed = bindings(&runtime).await;
        let before = binding(&listed, &stored_binding);
        assert_eq!(before["status"]["state"], json!("paused"), "{before:#?}");
        assert!(
            before["status"]["until"].is_string(),
            "the binding is paused with a deadline before the restart: {before:#?}"
        );
        runtime.shutdown().await;
        stored_binding
    };

    // The second launch, on the same state.
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    let listed = bindings(&runtime).await;
    let after = binding(&listed, &stored_binding);
    assert_eq!(after["status"]["state"], json!("paused"), "{after:#?}");
    assert!(
        after["status"]["until"].is_null(),
        "a start clears the deadline of a paused binding onto a member private to a person: \
         {after:#?}"
    );
    let capabilities = rpc(&runtime, "meta/capabilities", json!({})).await;
    assert!(capabilities["error"].is_null(), "{capabilities:#?}");
    assert!(
        capabilities["result"]["storage"]
            .to_string()
            .contains(&stored_binding),
        "the storage summary names the binding the start holds paused: {capabilities:#?}"
    );

    teardown(runtime).await;
}

/// A pause with a deadline ends by itself: when the deadline passes the
/// binding is active again, and no method of the host is asked. For a
/// binding whose target is a member private to a person that is a resume
/// by another name, so the host's pause refuses a deadline there
/// (`private_member_workgraph_target`) and leaves the binding as it was.
/// A pause with no deadline is taken.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_does_not_pause_with_a_deadline_a_binding_whose_target_is_private_to_a_person() {
    const MOB: &str = "workgraph-member-calls-pause-deadline";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    spawn_private_worker(&runtime, "private-helper").await;
    // The WorkGraph itself knows no private member: a binding written to it
    // directly names one.
    let service = runtime
        .workgraph_service()
        .expect("the runtime has a WorkGraph");
    let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal bound past the host's methods",
        "target": owner_target(MOB, "private-helper"),
    }))
    .expect("goal request");
    let stored = service.create_goal(request).await.expect("create goal");
    let stored_binding = serde_json::to_value(&stored.attention.binding_id)
        .expect("binding id json")
        .as_str()
        .expect("a binding id is a string")
        .to_string();

    let with_a_deadline = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({
            "binding_id": stored_binding,
            "expected_revision": stored.attention.machine_state.revision,
            "until": "2099-01-01T00:00:00Z",
        }),
    )
    .await;
    assert_eq!(
        with_a_deadline["error"]["data"]["kind"],
        json!("private_member_workgraph_target"),
        "the host does not pause with a deadline a binding onto a member private to a person: \
         {with_a_deadline:#?}"
    );
    let listed = bindings(&runtime).await;
    let unchanged = binding(&listed, &stored_binding);
    assert_eq!(
        unchanged["machine_state"]["revision"],
        json!(stored.attention.machine_state.revision),
        "the refused pause wrote nothing: {unchanged:#?}"
    );

    let without_one = rpc(
        &runtime,
        "meta/workgraph/attention/pause",
        json!({
            "binding_id": stored_binding,
            "expected_revision": stored.attention.machine_state.revision,
        }),
    )
    .await;
    assert!(
        without_one["error"].is_null(),
        "a pause with no deadline is taken: {without_one:#?}"
    );
    let listed = bindings(&runtime).await;
    let paused = binding(&listed, &stored_binding);
    assert_eq!(paused["status"]["state"], json!("paused"), "{paused:#?}");
    assert!(paused["status"]["until"].is_null(), "{paused:#?}");

    teardown(runtime).await;
}

/// The host may bind a goal to a name before its member exists. When the
/// member then created under that name is private to a person, the binding
/// does not stay active: the member's build holds it paused, with no
/// deadline, the host's resume refuses it, and the private member answers
/// its owner.
#[tokio::test(flavor = "multi_thread")]
async fn a_binding_made_before_its_member_is_held_when_the_member_is_private_to_a_person() {
    const MOB: &str = "workgraph-member-calls-bound-before-member";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    let goal = host_goal(
        &runtime,
        "work for a later member",
        "later-helper",
        "pursue",
    )
    .await;
    let listed = bindings(&runtime).await;
    assert_eq!(
        binding(&listed, &binding_id(&goal))["status"]["state"],
        json!("active"),
        "the host binds a name no member holds yet: {listed:#?}"
    );

    spawn_private_worker(&runtime, "later-helper").await;

    let listed = bindings(&runtime).await;
    let held = binding(&listed, &binding_id(&goal));
    assert_eq!(
        held["status"]["state"],
        json!("paused"),
        "the build of a member private to a person holds a binding that names it: {held:#?}"
    );
    assert!(
        held["status"]["until"].is_null(),
        "the hold has no deadline: {held:#?}"
    );
    let resumed = rpc(
        &runtime,
        "meta/workgraph/attention/resume",
        json!({
            "binding_id": binding_id(&goal),
            "expected_revision": held["machine_state"]["revision"],
        }),
    )
    .await;
    assert_eq!(
        resumed["error"]["data"]["kind"],
        json!("private_member_workgraph_target"),
        "the host does not resume it: {resumed:#?}"
    );
    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "later-helper",
        "OWNER-ASKS-AFTER-CREATION are you there?".to_string(),
    )
    .await
    .expect("the owner writes to the private member");
    model.turn_ended("OWNER-ASKS-AFTER-CREATION").await;

    teardown(runtime).await;
}

/// A start over a WorkGraph that holds no attention binding has nothing to
/// hold and takes no admission, so it makes no admission lock file in the
/// state directory. The first call that takes an admission makes the file.
#[tokio::test(flavor = "multi_thread")]
async fn a_start_with_no_attention_binding_makes_no_admission_lock_file() {
    const MOB: &str = "workgraph-member-calls-start-no-lock-file";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    let lock_file =
        meerkat_meta::workgraph_admission::workgraph_admission_sidecar_path(state_dir.path());
    assert!(
        !lock_file.exists(),
        "a start with no binding to hold makes no admission lock file: {}",
        lock_file.display()
    );

    host_goal(&runtime, "the first goal", "later-helper", "pursue").await;
    assert!(
        lock_file.exists(),
        "the first admission makes the lock file: {}",
        lock_file.display()
    );

    teardown(runtime).await;
}

/// The build of a member private to a person holds the bindings that name
/// it, and that hold always takes the admission: a start that built no
/// such member leaves no admission lock file, and the first build of one
/// makes it.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_build_of_a_private_member_makes_the_admission_lock_file() {
    const MOB: &str = "workgraph-member-calls-private-build-lock-file";
    let state_dir = tempfile::tempdir().expect("state dir");
    let model = CallingModel::default();
    let runtime = member_calls_runtime(MOB, state_dir.path(), &model).await;
    let lock_file =
        meerkat_meta::workgraph_admission::workgraph_admission_sidecar_path(state_dir.path());
    assert!(
        !lock_file.exists(),
        "a start that built no member private to a person makes no admission lock file: {}",
        lock_file.display()
    );

    spawn_private_worker(&runtime, "private-helper").await;
    assert!(
        lock_file.exists(),
        "the build of a member private to a person takes the admission, which makes the lock \
         file: {}",
        lock_file.display()
    );

    teardown(runtime).await;
}
