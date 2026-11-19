//! A WorkGraph attention binding and the tools its target was built with.
//!
//! A turn of a bound member carries the binding's overlay: an allow-list of
//! the WorkGraph tools of the binding's mode. The binding does not know
//! what the member was built with. These rows bind a goal to a member
//! built without the WorkGraph tools (its profile has none, or it is
//! private to a person) and read, at the model, that the member still
//! takes its next turn, with its own tools.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
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

/// Two profiles: `worker` has the WorkGraph tools, `plain` does not.
fn definition(mob_id: &str) -> MobDefinition {
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

[profiles.plain]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.plain.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

async fn runtime_of(
    mob_id: &str,
    state_dir: &std::path::Path,
    client: &CaptureClient,
) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(definition(mob_id))
            .persistent_state(state_dir)
            .default_llm_client(Arc::new(client.clone()))
            .build(),
    )
    .await
    .expect("the runtime builds")
}

async fn teardown(runtime: UnifiedRuntime) {
    let teardown = runtime.stop_mob_for_teardown().await;
    assert!(
        teardown.teardown_may_proceed(),
        "teardown must not be blocked by a member's runtime readiness: {teardown:?}"
    );
}

/// The host binds a goal to a member whose profile has no WorkGraph tools,
/// in each of two modes. The binding's turn overlay allows only WorkGraph
/// tools, none of which the member was built with. The member still takes
/// its next turn, as an ordinary turn with its own tools.
#[tokio::test(flavor = "multi_thread")]
async fn a_bound_member_built_without_workgraph_tools_takes_its_next_turn() {
    const MOB: &str = "workgraph-attention-tools-plain-member";
    let state_dir = tempfile::tempdir().expect("state dir");
    let client = CaptureClient::default();
    let runtime = runtime_of(MOB, state_dir.path(), &client).await;
    let mut turns = 0;
    for (member, mode) in [
        ("plain-pursuer", "pursue"),
        ("plain-coordinator", "coordinate"),
    ] {
        runtime
            .spawn_many(vec![SpawnMemberSpec::from_wire(
                "plain".to_string(),
                member.to_string(),
                None,
                None,
                None,
            )])
            .await
            .expect("spawn a member without the WorkGraph tools");
        let bound = rpc(
            &runtime,
            "meta/workgraph/goal/create",
            json!({
                "title": "work for a member without the tools",
                "target": { "kind": "identity", "identity": member },
                "mode": mode,
            }),
        )
        .await;
        assert!(bound["error"].is_null(), "{bound:#?}");

        meerkat_meta::send_message_on_mob(
            &runtime.mob_handle(),
            member,
            "are you there?".to_string(),
        )
        .await
        .expect("send");
        turns += 1;
        let captured = client.wait_for_request(turns).await;
        let tools = &captured[turns - 1];
        assert!(
            has_tool(tools, "send_message"),
            "the member bound in mode {mode} takes its turn with its own tools: {tools:?}"
        );
        assert!(
            !tools.iter().any(|tool| tool.starts_with("workgraph_")),
            "it was built without the WorkGraph tools: {tools:?}"
        );
    }

    teardown(runtime).await;
}

/// A member private to a person is built without the WorkGraph tools,
/// whatever its profile says. A binding that names it all the same (the
/// WorkGraph itself knows no private member) does not stop its turns.
#[tokio::test(flavor = "multi_thread")]
async fn a_bound_member_private_to_a_person_takes_its_next_turn() {
    const MOB: &str = "workgraph-attention-tools-private-member";
    let state_dir = tempfile::tempdir().expect("state dir");
    let client = CaptureClient::default();
    let runtime = runtime_of(MOB, state_dir.path(), &client).await;
    runtime
        .mob_handle()
        .spawn_spec(
            SpawnMemberSpec::new("worker", "private-helper").with_labels(BTreeMap::from([(
                meerkat_meta::access::PRIVATE_TO_LABEL.to_string(),
                "console-caller:v1:someone".to_string(),
            )])),
        )
        .await
        .expect("spawn a member private to a person");
    let service = runtime
        .workgraph_service()
        .expect("the runtime has a WorkGraph");
    let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
        "title": "a goal bound past the host's methods",
        "target": {
            "kind": "owner",
            "owner_key": { "kind": "agent", "id": format!("mob/{MOB}/agent/private-helper") },
        },
    }))
    .expect("goal request");
    service.create_goal(request).await.expect("create goal");

    meerkat_meta::send_message_on_mob(
        &runtime.mob_handle(),
        "private-helper",
        "are you there?".to_string(),
    )
    .await
    .expect("the owner writes to the private member");
    let captured = client.wait_for_request(1).await;
    assert!(
        !captured[0]
            .iter()
            .any(|tool| tool.starts_with("workgraph_")),
        "the private member was built without the WorkGraph tools: {:?}",
        captured[0]
    );

    teardown(runtime).await;
}
