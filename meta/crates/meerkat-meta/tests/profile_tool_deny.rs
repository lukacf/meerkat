//! A workspace profile's deny set through Meta's production runtime
//! (meerkat `[profiles.*.tools] deny`).
//!
//! A workspace identity agent's profile enables the mob tools but denies the
//! ones that would let it spawn or rewire broader same-mob members, keeping
//! fork_off, council, mob_check_member and mob_retire_member. The member is
//! built through `UnifiedRuntime` from the definition TOML and drives its own
//! turn: every denied tool is hidden from its visible tools (meerkat 0.8.52),
//! every kept tool it has mounted runs past the gate, and the deny set itself
//! builds (a known name the member does not mount is inert, not a build
//! failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
use serde_json::json;

#[path = "support/llm_usage.rs"]
mod llm_usage;

static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const PROBE: &str = "WORKSPACE-DENY-PROBE";

/// The workspace deny set: the agent mob tools and the mob operator tools that
/// spawn or rewire members.
const DENIED: &[&str] = &[
    "spawn_member",
    "spawn_many_members",
    "mob_spawn_member",
    "wire_members",
    "unwire_members",
    "mob_wire",
    "mob_unwire",
    "mob_create",
    "mob_destroy",
];

const KEPT: &[&str] = &[
    "fork_off",
    "council",
    "mob_check_member",
    "mob_retire_member",
];

fn definition() -> MobDefinition {
    definition_denying(DENIED)
}

fn definition_denying(names: &[&str]) -> MobDefinition {
    let deny = names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "workspace-deny-mob-{}"

[profiles.workspace]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.workspace.tools]
comms = true
mob = true
deny = [{deny}]
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
    .expect("parse the workspace definition")
}

/// What the probe turn saw: the tools the member had mounted, and each probe
/// call's tool-result text by tool name.
#[derive(Default)]
struct ProbeRecord {
    mounted: Vec<String>,
    results: Option<BTreeMap<String, String>>,
}

/// Calls each mounted denied and kept tool once, in order, then ends the turn
/// and records every call's result.
#[derive(Clone, Default)]
struct ProbeClient {
    record: Arc<Mutex<ProbeRecord>>,
    done: Arc<tokio::sync::Notify>,
}

fn probe_call_id(tool: &str) -> String {
    format!("call-{tool}")
}

/// Tool-result text per probe call id in `messages`.
fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

fn is_probe_turn(request: &LlmRequest) -> bool {
    // The probe text reaches the member wrapped as its runtime delivers it;
    // look for it anywhere in the transcript.
    serde_json::to_string(&request.messages).is_ok_and(|messages| messages.contains(PROBE))
}

impl LlmClient for ProbeClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let provider = LlmClient::provider(self);
        let next_call = if is_probe_turn(request) {
            let mounted: Vec<String> = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            let probes: Vec<&'static str> = DENIED
                .iter()
                .chain(KEPT)
                .copied()
                .filter(|tool| mounted.iter().any(|name| name == tool))
                .collect();
            let results = tool_results(&request.messages);
            let next = probes
                .iter()
                .find(|tool| !results.contains_key(&probe_call_id(tool)))
                .copied();
            let mut record = self.record.lock().unwrap();
            record.mounted = mounted;
            if next.is_none() {
                record.results = Some(results);
                self.done.notify_waiters();
            }
            next
        } else {
            None
        };
        let stop = if next_call.is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        let [usage, done] = llm_usage::usage_then_done(request, provider, stop);
        Box::pin(async_stream::stream! {
            match next_call {
                Some(tool) => {
                    yield Ok(LlmEvent::ToolCallComplete {
                        id: probe_call_id(tool),
                        name: tool.to_string(),
                        args: json!({}),
                        meta: None,
                    });
                }
                None => {
                    yield Ok(LlmEvent::TextDelta { delta: "probed".to_string(), meta: None });
                }
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
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn runtime_for(
    definition: MobDefinition,
    client: &ProbeClient,
    store_dir: &std::path::Path,
) -> UnifiedRuntime {
    let spec = MobBootstrapSpec::ephemeral(
        definition,
        MobStorage::in_memory(),
        store_dir.to_path_buf(),
        16,
        None,
    )
    .expect("build the profile tool deny ephemeral spec")
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(client.clone())),
    });
    Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: String::new(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(Duration::from_secs(30))
            .build(),
    )
    .await
    .expect("runtime builds")
}

/// A deny entry no vocabulary knows (a typo) fails the member build. The
/// gateway answers `meta/ensure_member` with -32602 whose message names the
/// entry, so a caller can fix the definition without reading logs.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_deny_entry_fails_ensure_member_with_invalid_params_naming_it() {
    let store_dir = tempfile::tempdir().expect("store dir");
    let client = ProbeClient::default();
    let runtime = runtime_for(
        definition_denying(&["spawn_member", "spawn_membr"]),
        &client,
        store_dir.path(),
    )
    .await;
    let request = json!({
        "jsonrpc": "2.0",
        "id": "unknown-deny-entry",
        "method": "meta/ensure_member",
        "params": { "role": "workspace", "agent_identity": "assistant" },
    })
    .to_string();
    let response: serde_json::Value = serde_json::from_str(
        &meerkat_meta::handle_unified_rpc_json(
            &runtime,
            &request,
            Duration::from_secs(30),
            None,
            None,
        )
        .await,
    )
    .expect("json-rpc response");
    assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
    let message = response["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("spawn_membr"),
        "the -32602 names the unknown entry: {response:#?}"
    );
    let _ = runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_deny_set_builds_and_hides_denied_mob_tools() {
    let store_dir = tempfile::tempdir().expect("store dir");
    let client = ProbeClient::default();
    let runtime = runtime_for(definition(), &client, store_dir.path()).await;

    // The deny set names agent mob tools as well as operator tools; a known
    // name the member does not mount is inert, so the member builds.
    runtime
        .spawn_many(vec![SpawnMemberSpec::new("workspace", "assistant")])
        .await
        .expect("a member builds with the workspace deny set");

    let done = client.done.notified();
    tokio::pin!(done);
    done.as_mut().enable();
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), "assistant", PROBE.to_string())
        .await
        .expect("send the probe turn");
    tokio::time::timeout(Duration::from_mins(1), done)
        .await
        .expect("the probe turn reaches its final request");

    let (mounted, results) = {
        let record = client.record.lock().unwrap();
        (
            record.mounted.clone(),
            record
                .results
                .clone()
                .expect("the probe turn recorded its results"),
        )
    };
    // A `mob = true` member mounts the mob operator tools; the probe is
    // meaningless without them. `retire_member` and `list_members` are not in
    // the deny set, so they stay mounted.
    for operator_tool in ["retire_member", "list_members"] {
        assert!(
            mounted.iter().any(|name| name == operator_tool),
            "a mob-enabled member mounts {operator_tool}: {mounted:?}"
        );
    }
    // An unconditional profile deny hides the tool from the member's visible
    // set at build time (meerkat 0.8.52); the execution gate still refuses
    // any denied call that reaches it.
    for tool in DENIED {
        assert!(
            !mounted.iter().any(|name| name == tool),
            "{tool} is denied by the profile and hidden: {mounted:?}"
        );
    }
    for tool in KEPT
        .iter()
        .filter(|tool| mounted.iter().any(|name| name == *tool))
    {
        let text = results
            .get(&probe_call_id(tool))
            .unwrap_or_else(|| panic!("{tool} was called"));
        // Empty arguments fail the kept tools' own validation; what matters
        // is that the execution gate let them through.
        assert!(
            !text.contains("access_denied"),
            "{tool} is kept and runs past the gate: {text}"
        );
    }
    let report = runtime.shutdown().await;
    assert!(report.mob_stop.is_ok(), "{:?}", report.mob_stop);
}
