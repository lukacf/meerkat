//! Host-registered Rust tool bundles reach mob members through meerkat-mob's
//! existing `tools.rust_bundles` wiring on every build: spawn, resume (which
//! revives the persisted member) and respawn. A profile naming a bundle that
//! was never registered is refused.
//!
//! Each assertion reads the tool names of the member's actual model request.
//! Waiting for that request is a typed watch on the recorded-request count,
//! bounded only by a named failure timeout.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{StopReason, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_meta::UnifiedRuntime;
use meerkat_mob::{AgentIdentity, MobDefinition, SpawnMemberSpec};

/// How long a member turn may take to reach the model before the test fails.
const MODEL_REQUEST_FAILURE_BOUND: Duration = Duration::from_secs(30);
const BUNDLE_TOOL: &str = "bundle_probe";

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
rust_bundles = ["probe"]

[profiles.misconfigured]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.misconfigured.tools]
comms = true
rust_bundles = ["absent"]
"#
    ))
    .expect("parse mob definition")
}

/// One tool, so its presence in a model request shows the bundle was wired.
struct ProbeBundle;

#[async_trait::async_trait]
impl AgentToolDispatcher for ProbeBundle {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            BUNDLE_TOOL,
            "Host-registered probe tool.",
            serde_json::json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

#[derive(Clone)]
struct CaptureClient {
    requests: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    count: Arc<tokio::sync::watch::Sender<usize>>,
}

impl CaptureClient {
    fn new() -> Self {
        Self {
            requests: Arc::default(),
            count: Arc::new(tokio::sync::watch::channel(0).0),
        }
    }

    /// Tool names of the `nth` (1-based) model request, once it exists.
    async fn request_tools(&self, nth: usize) -> Vec<String> {
        let mut count = self.count.subscribe();
        tokio::time::timeout(
            MODEL_REQUEST_FAILURE_BOUND,
            count.wait_for(|seen| *seen >= nth),
        )
        .await
        .expect("member turn reached the model within the failure bound")
        .expect("capture client alive");
        self.requests.lock().unwrap()[nth - 1].clone()
    }
}

impl LlmClient for CaptureClient {
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
        let names = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let seen = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(names);
            requests.len()
        };
        self.count.send_replace(seen);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            });
            yield Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            });
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

async fn runtime(mob_id: &str, state: &Path, client: &CaptureClient) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntime::builder()
            .definition(definition(mob_id))
            .persistent_state(state)
            .default_llm_client(Arc::new(client.clone()))
            .register_tool_bundle("probe", Arc::new(ProbeBundle))
            .build(),
    )
    .await
    .expect("runtime builds")
}

/// Durable mob storage plus durable sessions, so a restart resumes the mob
/// through `MobBuilder::resume` (the builder's `persistent_state` path keeps
/// only sessions durable). The bundle is registered on the spec.
async fn durable_runtime(state: &Path, client: &CaptureClient) -> UnifiedRuntime {
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(state.join("mob.sqlite3"))
            .expect("open persistent mob storage");
    let spec = meerkat_meta::MobBootstrapSpec::persistent(
        definition("bundle-resume"),
        storage,
        provenance,
        state.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent stores")
    .with_options(meerkat_meta::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(client.clone())),
    })
    .register_tool_bundle("probe", Arc::new(ProbeBundle));
    Box::pin(UnifiedRuntime::bootstrap(
        spec,
        meerkat_meta::MetaConfig {
            modules: vec![],
            discovery: meerkat_meta::DiscoverySpec {
                namespace: "bundle-resume".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        Duration::from_secs(5),
    ))
    .await
    .expect("bootstrap durable runtime")
}

fn worker(identity: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire("worker".to_string(), identity.to_string(), None, None, None)
}

async fn send(runtime: &UnifiedRuntime, identity: &str) {
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), identity, "hello".to_string())
        .await
        .expect("send message");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spawned_member_sees_its_profile_bundle() {
    let state = tempfile::tempdir().unwrap();
    let client = CaptureClient::new();
    let runtime = runtime("bundle-spawn", state.path(), &client).await;
    runtime.spawn(worker("helper")).await.expect("spawn");
    send(&runtime, "helper").await;
    assert!(
        client
            .request_tools(1)
            .await
            .iter()
            .any(|name| name == BUNDLE_TOOL)
    );
    runtime.shutdown().await;
}

/// The first boot runs in a child copy of this test binary: in-process comms
/// names outlive a stopped mob, so a same-process restart cannot model a
/// process restart. Exiting the child releases them; its state stays on disk.
const FIRST_BOOT_STATE: &str = "META_BUNDLE_RESUME_FIRST_BOOT_STATE";

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_mob_revives_its_member_with_the_bundle() {
    if let Some(state) = std::env::var_os(FIRST_BOOT_STATE) {
        // Child: spawn the member, stop the mob (keeping the roster) and exit.
        let client = CaptureClient::new();
        let first = durable_runtime(Path::new(&state), &client).await;
        first.spawn(worker("helper")).await.expect("spawn");
        first.mob_handle().stop().await.expect("stop");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "a_resumed_mob_revives_its_member_with_the_bundle",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(FIRST_BOOT_STATE, state.path())
        .output()
        .expect("run the first boot in a child process");
    assert!(
        output.status.success(),
        "first boot failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // A new runtime over the same state resumes the mob; the member is revived
    // by meerkat-mob's resume reconciliation, not spawned again, and its first
    // model request is built by that revival.
    let client = CaptureClient::new();
    let resumed = durable_runtime(state.path(), &client).await;
    assert!(
        resumed
            .mob_handle()
            .list_members_including_retiring()
            .await
            .iter()
            .any(|member| member.agent_identity.as_str() == "helper"),
        "the persisted member is part of the resumed mob"
    );
    // The first boot stopped the mob; resuming it brings the member back.
    resumed
        .mob_handle()
        .resume()
        .await
        .expect("resume the stopped mob");
    send(&resumed, "helper").await;
    assert!(
        client
            .request_tools(1)
            .await
            .iter()
            .any(|name| name == BUNDLE_TOOL)
    );
    resumed.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_respawned_member_keeps_its_bundle() {
    let state = tempfile::tempdir().unwrap();
    let client = CaptureClient::new();
    let runtime = runtime("bundle-respawn", state.path(), &client).await;
    runtime.spawn(worker("helper")).await.expect("spawn");
    runtime
        .mob_handle()
        .respawn(AgentIdentity::from("helper"), None)
        .await
        .expect("respawn");
    send(&runtime, "helper").await;
    assert!(
        client
            .request_tools(1)
            .await
            .iter()
            .any(|name| name == BUNDLE_TOOL)
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_naming_an_unregistered_bundle_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let client = CaptureClient::new();
    let runtime = runtime("bundle-missing", state.path(), &client).await;
    let error = runtime
        .spawn(SpawnMemberSpec::from_wire(
            "misconfigured".to_string(),
            "orphan".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect_err("an unregistered bundle refuses the member build");
    // meerkat 0.8.51 refuses it with the typed `ToolBundleUnavailable`.
    assert!(
        matches!(
            &error,
            meerkat_meta::MobRuntimeError::Mob(
                meerkat_mob::MobError::ToolBundleUnavailable { bundle }
            ) if bundle == "absent"
        ),
        "{error:?}"
    );
    assert!(
        runtime
            .mob_handle()
            .list_members_including_retiring()
            .await
            .iter()
            .all(|member| member.agent_identity.as_str() != "orphan"),
        "no member exists for the refused build"
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_registered_on_both_builder_and_spec_is_refused() {
    let state = tempfile::tempdir().unwrap();
    let spec = meerkat_meta::MobBootstrapSpec::new(
        definition("bundle-conflict"),
        meerkat_mob::MobStorage::in_memory(),
        Arc::new(meerkat::build_ephemeral_service(
            meerkat::AgentFactory::new(state.path()),
            meerkat::Config::default(),
            4,
        )),
    )
    .register_tool_bundle("probe", Arc::new(ProbeBundle));
    let result = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(meerkat_meta::MetaConfig {
                modules: vec![],
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "bundle-conflict".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(5))
            .register_tool_bundle("probe", Arc::new(ProbeBundle))
            .build(),
    )
    .await;
    let Err(error) = result else {
        panic!("a duplicate bundle registration must be refused");
    };
    assert!(
        format!("{error:?}").contains("tool bundle 'probe'"),
        "{error:?}"
    );
}
