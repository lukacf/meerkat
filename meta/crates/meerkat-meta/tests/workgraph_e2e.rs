//! WorkGraph end-to-end: a persistent UnifiedRuntime whose member profile
//! opts into `tools.workgraph` builds with the full `workgraph_*` tool
//! surface, and a goal created over RPC against the member's IDENTITY makes
//! the apply-time attention overlay reach a real member turn — observed at
//! the LLM boundary, where the turn-scoped tool overlay hard-filters the
//! provider-visible tool list down to the binding mode's allow-set.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
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

/// Profile whose member also mounts a host tool bundle with one deferrable
/// tool over the deferred catalog's byte budget.
const WORKGRAPH_OVER_BUDGET_TOML: &str = r#"
[mob]
id = "workgraph-over-budget-mob"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
workgraph = true
rust_bundles = ["large"]
"#;

const LARGE_HOST_TOOL: &str = "host_lookup";
const CATALOG_CONTROL_TOOLS: [&str; 2] = [
    meerkat_core::tool_catalog::TOOL_CATALOG_SEARCH_NAME,
    meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME,
];
const ATTENTION_TOOLS: [&str; 2] = ["workgraph_attention_reassign", "workgraph_policy_escalate"];

/// A host bundle with an exact catalog and one deferrable tool whose
/// definition alone is over the deferred catalog's byte budget. It asks for
/// the catalog's control tools on demand, so a request carries them only
/// while a tool waits behind the catalog.
struct LargeHostTools;

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for LargeHostTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        vec![Arc::new(meerkat_core::types::ToolDef::new(
            LARGE_HOST_TOOL,
            "x".repeat(meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD),
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn tool_catalog_control_plane(&self) -> meerkat_core::ToolCatalogControlPlane {
        meerkat_core::ToolCatalogControlPlane::OnDemand
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.tools()
            .iter()
            .map(|tool| {
                meerkat_core::ToolCatalogEntry::session_deferred(
                    Arc::clone(tool),
                    true,
                    meerkat_core::types::ToolProvenance {
                        kind: meerkat_core::types::ToolSourceKind::RustBundle,
                        source_id: "large".into(),
                        artifact: None,
                    },
                )
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(call.id.to_string(), "{}".to_string(), false),
        ))
    }
}

/// One model request as [`CallingClient`] saw it.
#[derive(Clone, Debug)]
struct Seen {
    /// The names of the tools the request offers.
    tools: Vec<String>,
    /// Every tool result the request carries: text, and whether the result
    /// is an error.
    answers: Vec<(String, bool)>,
    /// The tool this request's answer calls, if it calls one.
    calls: Option<String>,
}

/// A model that makes the one call the row scripts, in the first request
/// that offers the tool, and records every request. It reads no message
/// text: what a member's turn carries as user content is the runtime's.
#[derive(Clone, Default)]
struct CallingClient {
    seen: Arc<std::sync::Mutex<Vec<Seen>>>,
    next_call: Arc<std::sync::Mutex<Option<(String, Value)>>>,
}

impl CallingClient {
    /// Have the model call `tool` with `arguments` once, in the first
    /// request from now on that offers `tool`.
    fn call_when_offered(&self, tool: &str, arguments: Value) {
        *self.next_call.lock().unwrap() = Some((tool.to_owned(), arguments));
    }

    /// Wait until `found` answers for the requests seen so far; a wait that
    /// runs out prints them.
    async fn wait_for<T>(&self, what: &str, found: impl Fn(&[Seen]) -> Option<T>) -> T {
        let waited = tokio::time::timeout(Duration::from_mins(1), async {
            loop {
                let seen = self.seen.lock().unwrap().clone();
                if let Some(value) = found(&seen) {
                    return value;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
        match waited {
            Ok(value) => value,
            Err(_) => {
                let seen = self.seen.lock().unwrap().clone();
                panic!(
                    "no model request shows {what}; {} request(s) so far: {seen:#?}",
                    seen.len()
                )
            }
        }
    }
}

impl LlmClient for CallingClient {
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
        let mut answers = Vec::new();
        for message in &request.messages {
            if let meerkat_core::Message::ToolResults { results, .. } = message {
                answers.extend(
                    results
                        .iter()
                        .map(|result| (result.text_content(), result.is_error)),
                );
            }
        }
        let tools: Vec<String> = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let call = {
            let mut next_call = self.next_call.lock().unwrap();
            let offered = next_call
                .as_ref()
                .is_some_and(|(tool, _)| has_tool(&tools, tool));
            if offered { next_call.take() } else { None }
        };
        self.seen.lock().unwrap().push(Seen {
            tools,
            answers,
            calls: call.as_ref().map(|(name, _)| name.clone()),
        });
        let (lead, stop) = match call {
            Some((name, args)) => (
                LlmEvent::ToolCallComplete {
                    id: "fc_0".to_string(),
                    name,
                    args,
                    meta: None,
                },
                StopReason::ToolUse,
            ),
            None => (
                LlmEvent::TextDelta {
                    delta: "ok".to_string(),
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

/// A member whose other tools take it over the deferred catalog's byte
/// budget: WorkGraph's two attention tools, the one first-party family whose
/// tools may wait, wait behind the catalog with the host's tool on an
/// ordinary turn. In an attention turn the turn's overlay makes the tool its
/// binding's authority allows callable for that turn: the request offers
/// `workgraph_policy_escalate` with no load, and a call of it runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_attention_turn_of_a_member_over_the_catalog_budget_offers_and_runs_its_attention_tool()
{
    let state_dir = tempfile::tempdir().expect("state dir");
    let client = CallingClient::default();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(
                MobDefinition::from_toml(WORKGRAPH_OVER_BUDGET_TOML)
                    .expect("parse over-budget definition"),
            )
            .persistent_state(state_dir.path())
            .default_llm_client(Arc::new(client.clone()))
            .register_tool_bundle("large", Arc::new(LargeHostTools))
            .build(),
    )
    .await
    .expect("persistent workgraph runtime builds");
    runtime
        .spawn_many(vec![SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "helper".to_string(),
            None,
            None,
            None,
        )])
        .await
        .expect("spawn the over-budget member");

    // The member's first request, before any binding exists (it is the
    // request of the member's spawn turn or of the sent text's turn: either
    // is an ordinary turn): the member is over the budget, so what may wait
    // waits: the host's tool and WorkGraph's two attention tools. The two
    // control tools are in the request because a tool waits (the host bundle
    // asks for them on demand, not always); the other WorkGraph tools are
    // inline.
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "hello".to_string())
        .await
        .expect("first send");
    let unbound = client
        .wait_for("an ordinary turn", |seen| {
            seen.first().map(|request| request.tools.clone())
        })
        .await;
    for control_tool in CATALOG_CONTROL_TOOLS {
        assert!(
            has_tool(&unbound, control_tool),
            "an over-budget member's request carries {control_tool}: {unbound:?}"
        );
    }
    for waiting in ATTENTION_TOOLS.into_iter().chain([LARGE_HOST_TOOL]) {
        assert!(
            !has_tool(&unbound, waiting),
            "{waiting} waits behind the catalog on an ordinary turn: {unbound:?}"
        );
    }
    assert!(
        has_tool(&unbound, "workgraph_get"),
        "the other WorkGraph tools never wait: {unbound:?}"
    );

    // Bind a goal to the member (default mode: pursue, whose authority
    // allows `workgraph_policy_escalate` and not `workgraph_attention_reassign`).
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
    let item_id = response["result"]["item"]["id"]
        .as_str()
        .expect("item id")
        .to_string();
    let item_revision = response["result"]["item"]["revision"]
        .as_u64()
        .expect("item revision");

    // The attention turn: the model calls the attention tool in the first
    // request that offers it. No load came before: only the turn's overlay
    // can have put the tool into that request.
    client.call_when_offered(
        "workgraph_policy_escalate",
        json!({
            "id": item_id,
            "expected_revision": item_revision,
            "completion_policy": { "kind": "host_confirmed" },
        }),
    );
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "helper", "tighten it".to_string())
        .await
        .expect("second send");
    let (calling, bound) = client
        .wait_for(
            "the attention tool offered with no load (the scripted call is made there)",
            |seen| {
                seen.iter()
                    .position(|request| request.calls.is_some())
                    .map(|index| (index, seen[index].tools.clone()))
            },
        )
        .await;
    assert!(
        !has_tool(&bound, "workgraph_attention_reassign"),
        "the pursue binding's authority does not allow a reassign: {bound:?}"
    );
    // The attention turn does not offer the host's tool. This cannot tell
    // which of two reasons holds (it still waits behind the catalog; the
    // turn's overlay allows the binding's tools only): the ordinary turn
    // above is what shows it waiting.
    assert!(
        !has_tool(&bound, LARGE_HOST_TOOL),
        "the attention turn does not offer the host's tool: {bound:?}"
    );

    // The call's result reaches the model in the turn's next request.
    let answers = client
        .wait_for("the result of the attention tool's call", |seen| {
            seen.iter()
                .skip(calling + 1)
                .find(|request| !request.answers.is_empty())
                .map(|request| request.answers.clone())
        })
        .await;
    assert_eq!(answers.len(), 1, "one call, one result: {answers:?}");
    let (answer, is_error) = &answers[0];
    assert!(
        !is_error,
        "the call runs: it is not refused as a tool that waits behind the catalog: {answer}"
    );

    let teardown = runtime.stop_mob_for_teardown().await;
    assert!(
        teardown.teardown_may_proceed(),
        "teardown must not be blocked by a member's runtime readiness: {teardown:?}"
    );
}
