//! Durable agent-created child mobs: the R1-R8 scenarios of the child-mob
//! persistence disposition that Meta owns.
//!
//! Two lifetimes in one process stand for a restart: the first runtime stands
//! its children down and is dropped, the second is composed over the same
//! state directory and must find the same child without recreating it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_client::{LlmDoneOutcome, LlmEvent};
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, ApplicationToolPolicyBinding, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    PolicyRevision, StopReason, ToolConsequenceDenial, ToolConsequenceFailure,
    ToolConsequenceNarrowingPolicy, ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot,
    ToolConsequenceRequest, ToolConsequenceVerdict, ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, MobRuntimeMode, WorkOrigin, WorkSpec};
use serde_json::json;

use super::*;
use crate::child_mob_storage::{ChildMobStorage, ChildMobStorageError, REALM_PROFILE_DATABASE};
use crate::storage_layout::MetaStorageLayout;

const DENIED: &str = "host_lookup";
const ALLOWED: &str = "host_echo";
const CHILD_WORKERS: [&str; 2] = ["worker-a", "worker-b"];
/// A member's native shutdown outcome while its unregister admission waits
/// on a held registration transaction (its settlement follows).
const ADMISSION_PENDING: &str =
    "UnregisterPending { stage: \"registration_transaction_admission\", ";
/// A member's native shutdown outcome while its admitted runtime unregister
/// is still running at the stand-down deadline (its settlement follows).
const UNREGISTER_PENDING: &str = "UnregisterPending { stage: \"shutdown_runtime_unregister\", ";

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_MOB.fetch_add(1, Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Host tools, host policy and a counting scripted model
// ---------------------------------------------------------------------------

/// Ordinary host tools offered to child mobs; records every call that reaches
/// the host.
struct HostTools(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl AgentToolDispatcher for HostTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [DENIED, ALLOWED]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "Ordinary host tool.",
                    json!({"type": "object", "properties": {}}),
                ))
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call.name.to_string());
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

/// A host policy that denies [`DENIED`] for every member it governs.
struct DenyOne;

impl ToolConsequencePolicySnapshot for DenyOne {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: PolicyRevision(1),
            digest: PolicyDigest::from_canonical_bytes(b"deny-one"),
        }
    }

    fn evaluate(&self, request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        if request.tool_name.as_str() == DENIED {
            ToolConsequenceVerdict::Deny(ToolConsequenceDenial::new(
                "host_policy_denied",
                "the host policy denies this tool",
            ))
        } else {
            ToolConsequenceVerdict::Allow
        }
    }
}

struct HostProvider(PolicyProviderId);

impl ToolConsequenceNarrowingPolicy for HostProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.0
    }

    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(1)
    }

    fn snapshot(
        &self,
        _policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        Ok(Arc::new(DenyOne))
    }
}

fn host_registry() -> Arc<ToolConsequencePolicyRegistry> {
    Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostProvider(
                PolicyProviderId::new("host").expect("provider id"),
            ))],
            PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .expect("host policy registry"),
    )
}

fn host_policy() -> ApplicationToolPolicyBinding {
    ApplicationToolPolicyBinding::Provider {
        provider_id: PolicyProviderId::new("host").expect("provider id"),
        policy_id: PolicyId::new("child-tools").expect("policy id"),
    }
}

/// Answers every turn with text, except an armed tool turn, which calls
/// [`DENIED`], then [`ALLOWED`], then answers. Counts every provider call, so
/// a restore that starts no turn is observable as an unchanged count.
#[derive(Default)]
struct ScriptedModel {
    calls: AtomicU64,
    tool_turn: AtomicBool,
    step: AtomicUsize,
    /// The newest message of every request, rendered, so a test can count the
    /// requests that carried one input.
    newest: Mutex<Vec<String>>,
}

impl ScriptedModel {
    fn arm_tool_turn(&self) {
        self.step.store(0, Ordering::SeqCst);
        self.tool_turn.store(true, Ordering::SeqCst);
    }

    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// Requests whose newest message carries `marker`.
    fn requests_carrying(&self, marker: &str) -> usize {
        self.newest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|newest| newest.contains(marker))
            .count()
    }
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.newest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{:?}", request.messages.last()));
        let tool_call = |id: &str, name: &str| LlmEvent::ToolCallComplete {
            id: id.to_string(),
            name: name.to_string(),
            args: json!({}),
            meta: None,
        };
        let text = |delta: &str| LlmEvent::TextDelta {
            delta: delta.to_string(),
            meta: None,
        };
        let (lead, stop_reason) = if self.tool_turn.load(Ordering::SeqCst) {
            match self.step.fetch_add(1, Ordering::SeqCst) {
                0 => (tool_call("call-denied", DENIED), StopReason::ToolUse),
                1 => (tool_call("call-allowed", ALLOWED), StopReason::ToolUse),
                _ => {
                    self.tool_turn.store(false, Ordering::SeqCst);
                    (text("done"), StopReason::EndTurn)
                }
            }
        } else {
            (text("ok"), StopReason::EndTurn)
        };
        let events = vec![
            Ok(lead),
            Ok(super::test_llm_usage::usage_event(
                request,
                Provider::OpenAI,
            )),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ];
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Runtime lifetimes over one durable state directory
// ---------------------------------------------------------------------------

/// What persists across lifetimes besides the state directory: the model and
/// the record of host tool calls.
struct Host {
    model: Arc<ScriptedModel>,
    dispatched: Arc<Mutex<Vec<String>>>,
}

impl Host {
    fn new() -> Self {
        Self {
            model: Arc::new(ScriptedModel::default()),
            dispatched: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

fn root_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.coordinator]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.coordinator.tools]\ncomms = true\n"
    ))
    .expect("root definition")
}

fn child_definition(mob_id: &MobId) -> MobDefinition {
    serde_json::from_value(json!({
        "id": mob_id.to_string(),
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        } }
    }))
    .expect("child definition")
}

fn durable(dir: &Path) -> ChildMobStorage {
    ChildMobStorage::Durable(MetaStorageLayout::with_injected_roots(
        dir.to_path_buf(),
        None,
    ))
}

/// One runtime lifetime: the stock persistent composition over `dir`, with
/// persistent sessions, a persistent root mob, child-available host tools and
/// whatever `configure` selects.
async fn lifetime(
    dir: &Path,
    mob_id: &str,
    host: &Host,
    configure: impl FnOnce(MobBootstrapSpec) -> MobBootstrapSpec,
) -> Result<MobRuntime, MobRuntimeError> {
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
            .expect("root mob storage");
    let tools: Arc<dyn meerkat_core::AgentToolDispatcher> =
        Arc::new(HostTools(Arc::clone(&host.dispatched)));
    let mut spec = MobBootstrapSpec::persistent(
        root_definition(mob_id),
        storage,
        provenance,
        dir.to_path_buf(),
        8,
    )
    .await
    .expect("persistent spec")
    .register_tool_bundle_with_availability(
        "host",
        tools,
        meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable,
    );
    spec = configure(spec);
    let model: Arc<dyn LlmClient> = host.model.clone();
    spec.options.default_llm_client = Some(model);
    Box::pin(MobRuntime::bootstrap(spec)).await
}

async fn coordinator_session(runtime: &MobRuntime) -> meerkat_core::types::SessionId {
    let coordinator = AgentIdentity::from("coordinator");
    if runtime
        .handle()
        .resolve_bridge_session_id(&coordinator)
        .await
        .is_none()
    {
        Box::pin(runtime.handle().spawn_spec(SpawnMemberSpec::new(
            ProfileName::from("coordinator"),
            coordinator.clone(),
        )))
        .await
        .expect("spawn the coordinator");
    }
    runtime
        .handle()
        .resolve_bridge_session_id(&coordinator)
        .await
        .expect("coordinator bridge session")
}

/// Create an owner-bridge child mob, as agent `mob_create` does, and seat its
/// turn-driven workers.
async fn create_child(
    runtime: &MobRuntime,
    child_id: &MobId,
    owner: meerkat_core::types::SessionId,
) -> MobHandle {
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");
    state
        .mob_create_definition_with_owner_bridge_session(
            child_definition(child_id),
            owner,
            true,
            false,
        )
        .await
        .expect("create the child mob");
    for worker in CHILD_WORKERS {
        state
            .mob_spawn(
                child_id,
                ProfileName::from("worker"),
                AgentIdentity::from(worker),
                Some(MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("seat a child worker");
    }
    state.handle_for(child_id).await.expect("child handle")
}

async fn worker_sessions(child: &MobHandle) -> Vec<meerkat_core::types::SessionId> {
    let mut sessions = Vec::new();
    for worker in CHILD_WORKERS {
        sessions.push(
            child
                .resolve_bridge_session_id(&AgentIdentity::from(worker))
                .await
                .expect("child worker session"),
        );
    }
    sessions
}

/// Run one armed tool turn of `worker`; return the host tools it reached.
async fn run_tool_turn(host: &Host, child: &MobHandle, worker: &str) -> Vec<String> {
    host.dispatched
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    host.model.arm_tool_turn();
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let work = child
        .start_work_for_identity_bounded(
            AgentIdentity::from(worker),
            WorkSpec::new(ContentInput::Text("go".to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("turn succeeds");
    host.dispatched
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

// ---------------------------------------------------------------------------
// Composition-level fixture (no restart): MobBootstrapSpec::new with the agent
// mob tools, as both gateway binaries compose it.
// ---------------------------------------------------------------------------

fn composed_spec(dir: &Path, storage: MobStorage) -> MobBootstrapSpec {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> = Arc::new(Base64BlobStoreAdapter::new(Arc::new(
        ObjectStoreBlobStore::memory(),
    )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("construct the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = FactoryAgentBuilder::new(
        AgentFactory::new(dir).builtins(false).comms(true),
        Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        4,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    MobBootstrapSpec::new(root_definition(&unique("composed")), storage, service)
        .with_session_runtime_adapter(machine)
        .expect("adopt the runtime owner")
        .with_runtime_delivery(MobRuntimeDelivery::new(store, jobs))
        .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
        .expect("install the agent mob tools")
}

/// Root storage without participant custody: the state then keeps its own
/// default custody, which a durable child root upgrades.
fn custody_free_storage() -> MobStorage {
    MobStorage::in_memory().with_forked_participant_store(None)
}

fn child_storage_error(result: Result<(), MobRuntimeError>) -> ChildMobStorageError {
    match result {
        Err(MobRuntimeError::ChildMobStorage(error)) => error,
        other => panic!("expected a child mob storage error, got {other:?}"),
    }
}

fn inline_profile(name: &str, model: &str) -> Profile {
    let definition = MobDefinition::from_toml(&format!(
        "[mob]\nid = \"profile-source\"\n\n[profiles.{name}]\nmodel = \"{model}\"\n"
    ))
    .expect("profile source definition");
    definition
        .profiles
        .get(&ProfileName::from(name))
        .and_then(|binding| binding.as_inline())
        .cloned()
        .expect("inline profile")
}

// ---------------------------------------------------------------------------
// Lifetimes as processes
//
// A durable child root stays owned until its process exits (Meerkat cannot yet
// close its state-owned work in-process), so a restart is a real process
// boundary: each lifetime runs as the only test of a child process of this
// test binary, and the parent test only sequences them and inspects the
// state directory in between.
// ---------------------------------------------------------------------------

const LIFETIME_ENV: &str = "MEERKAT_META_CHILD_LIFETIME";
const LIFETIME_DIR_ENV: &str = "MEERKAT_META_CHILD_LIFETIME_DIR";
const ROOT_MOB: &str = "lifetime-root";
const CHILD_MOB: &str = "lifetime-child";
const RECORD_FILE: &str = "lifetime-record.json";

/// Run `lifetime` (`<scenario>:<phase>`) over `dir` as its own process.
fn run_lifetime(lifetime: &str, dir: &Path) {
    let module = module_path!()
        .split_once("::")
        .map_or("", |(_crate_name, module)| module);
    let test = format!("{module}::lifetime_driver");
    let binary = std::env::current_exe().expect("locate the test binary");
    let output = std::process::Command::new(binary)
        .args([test.as_str(), "--exact", "--test-threads=1", "--nocapture"])
        .env(LIFETIME_ENV, lifetime)
        .env(LIFETIME_DIR_ENV, dir)
        .output()
        .expect("spawn the lifetime process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // `1 passed` also proves the filter selected the driver.
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "lifetime `{lifetime}` failed ({})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
}

/// The body of one lifetime process; a no-op in an ordinary test run.
#[test]
fn lifetime_driver() {
    let Ok(lifetime) = std::env::var(LIFETIME_ENV) else {
        return;
    };
    // A lifetime is a process of its own: its warnings (a stand-down that did
    // not settle names the unsettled work) reach the parent's failure message.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_writer(std::io::stderr)
        .try_init();
    let dir = PathBuf::from(std::env::var(LIFETIME_DIR_ENV).expect("lifetime state dir"));
    std::thread::Builder::new()
        .name("child-mob-lifetime".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(16 * 1024 * 1024)
                .enable_all()
                .build()
                .expect("lifetime runtime")
                .block_on(async {
                    tokio::time::timeout(
                        Duration::from_mins(2),
                        Box::pin(drive_lifetime(&lifetime, &dir)),
                    )
                    .await
                    .expect("the lifetime settles");
                });
        })
        .expect("spawn the lifetime thread")
        .join()
        .expect("the lifetime passes");
}

async fn drive_lifetime(lifetime: &str, dir: &Path) {
    let host = Host::new();
    let child_id = MobId::from(CHILD_MOB);
    match lifetime {
        // R1: create the child, record its sessions and profile.
        "cold:1" | "maint:1" => {
            // No selection: the stock persistent composition defaults to
            // durable child mobs, and cold:2 restores them explicitly.
            let runtime = self::lifetime(dir, ROOT_MOB, &host, |spec| spec)
                .await
                .expect("bootstrap the lifetime");
            let coordinator = coordinator_session(&runtime).await;
            let child = create_child(&runtime, &child_id, coordinator.clone()).await;
            let profile = child
                .effective_member_profile(&AgentIdentity::from(CHILD_WORKERS[0]))
                .await
                .expect("worker profile");
            write_record(
                dir,
                json!({
                    "coordinator": coordinator.to_string(),
                    "workers": worker_sessions(&child)
                        .await
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    "profile": profile,
                }),
            );
            drop(child);
            stand_down_released(runtime, 1).await;
        }
        // R1: the same child, sessions and profile, without a model turn.
        "cold:2" => {
            let record = read_record(dir);
            let runtime = lifetime_with(dir, &host, durable(dir)).await;
            let child = runtime
                .agent_mob_mcp_state()
                .expect("agent mob state")
                .handle_for(&child_id)
                .await
                .expect("the same child is found without recreating it");
            assert_eq!(
                coordinator_session(&runtime).await.to_string(),
                record["coordinator"]
                    .as_str()
                    .expect("recorded coordinator")
            );
            assert_eq!(
                json!(
                    worker_sessions(&child)
                        .await
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                ),
                record["workers"],
                "a cold restore must reuse every saved session"
            );
            assert_eq!(
                json!(
                    child
                        .effective_member_profile(&AgentIdentity::from(CHILD_WORKERS[0]))
                        .await
                        .expect("restored worker profile")
                ),
                record["profile"]
            );
            assert_eq!(
                host.model.calls(),
                0,
                "restoring and observing the child must not start a model turn"
            );
            drop(child);
            stand_down_released(runtime, 1).await;
        }
        // R1 policy half: the first lifetime under `managed` or `unmanaged`.
        "policy-managed:1" | "policy-tightened:1" => {
            let (policy, reached) = if lifetime == "policy-managed:1" {
                (host_policy(), vec![ALLOWED.to_string()])
            } else {
                (
                    ApplicationToolPolicyBinding::Unmanaged,
                    vec![DENIED.to_string(), ALLOWED.to_string()],
                )
            };
            let runtime = lifetime_under_policy(dir, &host, policy).await;
            let coordinator = coordinator_session(&runtime).await;
            let child = create_child(&runtime, &child_id, coordinator).await;
            assert_eq!(
                run_tool_turn(&host, &child, CHILD_WORKERS[0]).await,
                reached
            );
            drop(child);
            stand_down_released(runtime, 1).await;
        }
        // R1 policy half: the restored member runs under the current managed
        // policy, unchanged or tightened.
        "policy-managed:2" | "policy-tightened:2" => {
            let runtime = lifetime_under_policy(dir, &host, host_policy()).await;
            let child = runtime
                .agent_mob_mcp_state()
                .expect("agent mob state")
                .handle_for(&child_id)
                .await
                .expect("restored child");
            assert_eq!(
                run_tool_turn(&host, &child, CHILD_WORKERS[1]).await,
                vec![ALLOWED.to_string()],
                "a restored member is denied the tool the current policy denies and keeps \
                 its permitted sibling"
            );
            drop(child);
            stand_down_released(runtime, 1).await;
        }
        // R2: process-bound children are never restored.
        "bound:1" => {
            let runtime = lifetime_with(dir, &host, ChildMobStorage::ProcessBound).await;
            let coordinator = coordinator_session(&runtime).await;
            drop(create_child(&runtime, &child_id, coordinator).await);
            stand_down_settled(runtime).await;
        }
        "bound:2" | "maint:3" => {
            let process_bound = lifetime == "bound:2";
            let storage = if process_bound {
                ChildMobStorage::ProcessBound
            } else {
                durable(dir)
            };
            let runtime = lifetime_with(dir, &host, storage).await;
            assert!(matches!(
                runtime
                    .agent_mob_mcp_state()
                    .expect("agent mob state")
                    .handle_for(&child_id)
                    .await,
                Err(MobError::MobNotFound(_))
            ));
            if process_bound {
                stand_down_settled(runtime).await;
            } else {
                stand_down_released(runtime, 0).await;
            }
        }
        // R3: destroy the restored child.
        "maint:2" => {
            let runtime = lifetime_with(dir, &host, durable(dir)).await;
            runtime
                .agent_mob_mcp_state()
                .expect("agent mob state")
                .mob_destroy(&child_id)
                .await
                .expect("destroy the child");
            stand_down_released(runtime, 0).await;
        }
        other => panic!("unknown lifetime `{other}`"),
    }
}

async fn lifetime_with(dir: &Path, host: &Host, storage: ChildMobStorage) -> MobRuntime {
    lifetime(dir, ROOT_MOB, host, |spec| {
        spec.with_child_mob_storage(storage)
    })
    .await
    .expect("bootstrap the lifetime")
}

async fn lifetime_under_policy(
    dir: &Path,
    host: &Host,
    policy: ApplicationToolPolicyBinding,
) -> MobRuntime {
    let storage = durable(dir);
    lifetime(dir, ROOT_MOB, host, |spec| {
        spec.with_child_mob_storage(storage)
            .with_tool_consequence_policy_registry(host_registry())
            .with_child_application_tool_policy(policy)
    })
    .await
    .expect("bootstrap the lifetime")
}

/// The deadline of a stand-down a test expects to release the child root.
///
/// `stand_down_child_mobs` returns as soon as every child and the state's own
/// work settle; its deadline only caps the wait. These tests assert the
/// release after a proven stand-down, so the cap is a hang guard (inside a
/// lifetime's two-minute one) rather than a budget a loaded host can miss: on
/// a host at load ~50 the state's work took more than 15 s to settle, and the
/// stand-down reported `HeldUntilProcessExit`.
fn release_deadline() -> std::time::Instant {
    std::time::Instant::now() + Duration::from_secs(90)
}

/// Stand down a durable lifetime: every child and the state's own work
/// settle, and the child root is released.
async fn stand_down_released(runtime: MobRuntime, child_mobs: usize) {
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    assert_eq!(
        runtime.stand_down_child_mobs(release_deadline()).await,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs }
    );
}

/// Stand down a process-bound lifetime.
async fn stand_down_settled(runtime: MobRuntime) {
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    assert_eq!(
        runtime
            .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(15))
            .await,
        crate::child_mob_storage::ChildMobStandDownOutcome::NotConfigured
    );
}

fn write_record(dir: &Path, record: serde_json::Value) {
    std::fs::write(
        dir.join(RECORD_FILE),
        serde_json::to_vec(&record).expect("encode the lifetime record"),
    )
    .expect("write the lifetime record");
}

fn read_record(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(RECORD_FILE)).expect("read the record"))
        .expect("decode the lifetime record")
}

// ---------------------------------------------------------------------------
// R1: the child, its members and their sessions survive a cold restart
// ---------------------------------------------------------------------------

#[test]
fn a_durable_child_mob_is_restored_with_its_sessions_and_no_new_turn() {
    let dir = tempfile::tempdir().expect("state dir");
    run_lifetime("cold:1", dir.path());
    run_lifetime("cold:2", dir.path());
}

/// R1, policy half: a restored child member runs under the CURRENT host child
/// policy, with a denied operation and a permitted sibling. Needs the Core
/// resume-policy handoff (`SpawnMemberSpec::application_tool_policy` as an
/// explicit choice on resume).
#[test]
fn a_restored_managed_child_member_keeps_the_current_policy() {
    let dir = tempfile::tempdir().expect("state dir");
    run_lifetime("policy-managed:1", dir.path());
    run_lifetime("policy-managed:2", dir.path());
}

#[test]
fn a_restored_unmanaged_child_member_runs_under_a_tightened_policy() {
    let dir = tempfile::tempdir().expect("state dir");
    run_lifetime("policy-tightened:1", dir.path());
    run_lifetime("policy-tightened:2", dir.path());
}

fn assert_root_owned(dir: &Path) {
    assert!(
        matches!(
            crate::child_mob_storage::ChildMobRootClaim::acquire(
                &crate::child_mob_storage::child_mob_root(dir)
            ),
            Err(ChildMobStorageError::AlreadyClaimed { .. })
        ),
        "the durable child root must stay owned"
    );
}

/// A durable child root that holds the production predecessor's empty,
/// unversioned realm profile store boots: the composition adopts exactly
/// that shape under the root claim, and Meerkat initializes it as new.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_durable_child_root_with_the_predecessors_empty_realm_profiles_boots() {
    let dir = tempfile::tempdir().expect("state dir");
    let root = crate::child_mob_storage::child_mob_root(dir.path());
    std::fs::create_dir_all(&root).expect("child root");
    std::fs::write(
        root.join(REALM_PROFILE_DATABASE),
        include_bytes!("../tests/fixtures/legacy_empty_realm_profiles/realm_profiles.db"),
    )
    .expect("plant the predecessor's store");
    let host = Host::new();
    let mob_id = unique("legacy-realm-profiles");
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("the child root with the predecessor's empty store boots");
    assert_root_owned(dir.path());
    stand_down_released(runtime, 0).await;
}

/// A proven stand-down releases the durable child root in this process:
/// once every child and the agent mob-tool state's own work settled, a
/// successor over the same layout claims it, restores the child, and can
/// stand down and release it again. The test holds no claim of its own, so
/// the lock it observes is the runtime's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stood_down_child_root_is_released_for_a_successor_in_the_same_process() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("released-root");
    let child_id = MobId::from(unique("released-child").as_str());
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(&runtime, &child_id, coordinator).await;
    let exported_state = runtime.agent_mob_mcp_state().expect("exported state clone");
    let runtime_clone = runtime.clone();

    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let released = runtime.stand_down_child_mobs(release_deadline()).await;
    assert_eq!(
        released,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 }
    );
    assert!(released.cleanup_complete());
    // A repeat call reports the release, through any clone.
    assert_eq!(
        runtime_clone
            .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(15))
            .await,
        released
    );
    // The stood-down state admits no new mob.
    assert!(
        exported_state
            .mob_create_definition(child_definition(&MobId::from("after-release")))
            .await
            .is_err()
    );

    // The old runtime and its clones are still alive: the release does not
    // wait for them to drop.
    let successor = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("a same-process successor claims the released root");
    assert_root_owned(dir.path());
    successor
        .agent_mob_mcp_state()
        .expect("successor state")
        .handle_for(&child_id)
        .await
        .expect("the successor restores the child");
    drop(child);
    drop(exported_state);
    drop(runtime_clone);
    drop(runtime);
    stand_down_released(successor, 1).await;
    assert!(
        crate::child_mob_storage::ChildMobRootClaim::acquire(
            &crate::child_mob_storage::child_mob_root(dir.path())
        )
        .is_ok(),
        "the successor released the root too"
    );
}

/// The release is published only after the last holder of the claim is
/// gone. While another holder outlives the release boundary, concurrent
/// stand-downs through two clones both report the root held, never
/// `Released`, and the exact root still refuses a successor. Once that
/// holder drops, a concurrent retry pair both report the release, and the
/// exact root is free at that point.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_is_reported_only_after_the_last_claim_holder_is_gone() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("release-boundary");
    let child_id = MobId::from(unique("release-boundary-child").as_str());
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(&runtime, &child_id, coordinator).await;
    let runtime_clone = runtime.clone();
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");

    let holder = runtime
        .child_mob_root
        .claim_holder_for_test()
        .expect("the runtime owns the root");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let (first, retry) = tokio::join!(
        runtime.stand_down_child_mobs(deadline),
        runtime_clone.stand_down_child_mobs(deadline),
    );
    let held =
        crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs: 1 };
    assert_eq!(first, held);
    assert_eq!(
        retry, held,
        "a concurrent retry never reports Released early"
    );
    assert!(!first.cleanup_complete());
    assert!(runtime.owns_child_mob_root());
    assert_root_owned(dir.path());

    drop(holder);
    let deadline = release_deadline();
    let (first, retry) = tokio::join!(
        runtime.stand_down_child_mobs(deadline),
        runtime_clone.stand_down_child_mobs(deadline),
    );
    let released = crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 };
    assert_eq!(first, released);
    assert_eq!(retry, released);
    assert!(!runtime.owns_child_mob_root());
    let successor_claim = crate::child_mob_storage::ChildMobRootClaim::acquire(
        &crate::child_mob_storage::child_mob_root(dir.path()),
    );
    assert!(
        successor_claim.is_ok(),
        "the exact root is free once Released is reported: {:?}",
        successor_claim.err()
    );
    drop(child);
}

/// A child created while the stand-down drains is either admitted before
/// the state's admission closed, and then shut down and counted, or
/// refused; it never runs on after the root is released. Limit: nothing
/// forces an admitted create to cross the close, so this can pass with no
/// racing create admitted at all; it does not prove a late insertion by
/// retained state work is caught by the second snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_created_during_the_stand_down_is_counted_or_refused() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("drain-insert");
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");
    // The same create succeeds while admission is open.
    let admitted_id = state
        .mob_create_definition(child_definition(&MobId::from(
            unique("drain-insert-before").as_str(),
        )))
        .await
        .expect("a create before the stand-down is admitted");
    let admitted = state
        .handle_for(&admitted_id)
        .await
        .expect("the admitted child");

    let creator = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut created = Vec::new();
            for attempt in 0..10_000 {
                let child_id = MobId::from(unique(&format!("drain-insert-{attempt}")).as_str());
                match state
                    .mob_create_definition(child_definition(&child_id))
                    .await
                {
                    Ok(id) => created.push(
                        state
                            .handle_for(&id)
                            .await
                            .expect("a created child has a handle"),
                    ),
                    Err(_) => return created,
                }
                tokio::task::yield_now().await;
            }
            panic!("the state never refused a create during its stand-down");
        })
    };
    let outcome = runtime.stand_down_child_mobs(release_deadline()).await;
    let during = tokio::time::timeout(Duration::from_secs(15), creator)
        .await
        .expect("the creator stops once refused")
        .expect("creator task");
    assert_eq!(
        outcome,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released {
            child_mobs: 1 + during.len()
        },
        "every child admitted before the close is counted"
    );
    for handle in during.iter().chain(std::iter::once(&admitted)) {
        let status = handle.status_observation_snapshot();
        assert!(
            !matches!(
                status,
                meerkat_mob::MobState::Creating | meerkat_mob::MobState::Running
            ),
            "an admitted child is shut down before the release: {status:?}"
        );
    }
    assert!(
        crate::child_mob_storage::ChildMobRootClaim::acquire(
            &crate::child_mob_storage::child_mob_root(dir.path())
        )
        .is_ok()
    );
}

/// The wait for another clone's running stand-down is bounded by the
/// caller's deadline. While the test holds the real stand-down gate, a
/// short-deadline stand-down through a clone returns `InProgress` (cleanup
/// incomplete, children unobserved) instead of waiting, and the root stays
/// owned. A queued waiter that is cancelled leaves nothing behind: once the
/// gate is free, a stand-down releases the root as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stand_down_waits_for_a_running_one_only_until_its_deadline() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("gate-deadline");
    let child_id = MobId::from(unique("gate-deadline-child").as_str());
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(&runtime, &child_id, coordinator).await;
    let runtime_clone = runtime.clone();
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");

    // A running stand-down: the test owns the gate.
    let running = runtime
        .child_mob_root
        .begin_stand_down(std::time::Instant::now() + Duration::from_secs(5))
        .await
        .expect("the gate is free");

    // Blocked waiter with a short deadline: it answers by its deadline.
    let blocked = tokio::time::timeout(
        Duration::from_secs(10),
        runtime_clone.stand_down_child_mobs(std::time::Instant::now() + Duration::from_millis(200)),
    )
    .await
    .expect("the blocked stand-down returns by its own deadline");
    assert_eq!(
        blocked,
        crate::child_mob_storage::ChildMobStandDownOutcome::InProgress
    );
    assert!(!blocked.child_actors_settled());
    assert!(!blocked.cleanup_complete());
    assert!(runtime.owns_child_mob_root());
    assert_root_owned(dir.path());

    // Cancelled waiter: queued behind the gate, then dropped.
    let waiter = {
        let runtime = runtime_clone.clone();
        tokio::spawn(async move {
            runtime
                .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(15))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !waiter.is_finished(),
        "the waiter is queued behind the gate"
    );
    waiter.abort();
    assert!(
        waiter
            .await
            .expect_err("the waiter was cancelled")
            .is_cancelled(),
        "the queued waiter was cancelled, not completed"
    );
    assert_root_owned(dir.path());

    drop(running);
    let released = runtime.stand_down_child_mobs(release_deadline()).await;
    assert_eq!(
        released,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 }
    );
    assert!(
        crate::child_mob_storage::ChildMobRootClaim::acquire(
            &crate::child_mob_storage::child_mob_root(dir.path())
        )
        .is_ok(),
        "the exact root is free after the release"
    );
    drop(child);
}

/// With no child ever created, the durable child root is still claimed; a
/// stand-down releases it, and a same-process successor claims it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_root_with_no_children_is_released_for_a_successor() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("zero-children");
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    assert_root_owned(dir.path());
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let released = runtime.stand_down_child_mobs(release_deadline()).await;
    assert_eq!(
        released,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 0 }
    );
    assert!(released.cleanup_complete());
    let successor = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("a same-process successor claims the released root");
    assert_root_owned(dir.path());
    drop(runtime);
    stand_down_released(successor, 0).await;
}

/// A stand-down is terminal for the runtime's delivery owner as well: a
/// runtime torn down without `UnifiedRuntime` (the root mob shut down, then
/// the child stand-down) stops it, as `UnifiedRuntime`'s shutdown does. The
/// owner kept running after the root was released, held by the composition
/// the dropped runtime left behind, and an owner pass in flight when the
/// state directory was removed recreated it with an empty runtime store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stand_down_stops_the_runtime_delivery_owner() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("delivery-stop");
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let released = runtime.stand_down_child_mobs(release_deadline()).await;
    // Stopped, and joined: the owner's task ended before the stand-down
    // returned, so no pass of it can run past the release.
    let stopped = runtime
        .runtime_delivery
        .as_ref()
        .map(|delivery| (delivery.is_stopped(), delivery.owner_task_ended()));
    drop(runtime);
    // Work the dropped runtime still finishes may close files in the
    // directory while it goes: remove it again until it is gone.
    let removal_deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::remove_dir_all(dir.path()) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => assert!(
                std::time::Instant::now() < removal_deadline,
                "remove the state dir: {error}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The delivery owner sweeps its store every 5 s by default.
    let deadline = std::time::Instant::now() + Duration::from_secs(7);
    while !dir.path().exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        (released, stopped, dir.path().exists()),
        (
            crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 0 },
            Some((true, true)),
            false
        )
    );
}

/// A child that shut down cleanly keeps that receipt for its incarnation: a
/// second pass of the child stand-down helper over the same, now stopped,
/// incarnation (the retry a stand-down makes after unsettled state work)
/// does not ask the actor again, so its closed channel never turns the
/// known clean shutdown into unknown debt. This drives the helper directly;
/// it does not force a first full stand-down to time out on state work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_keeps_the_clean_receipt_of_a_stopped_child() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let mob_id = unique("clean-receipt");
    let child_id = MobId::from(unique("clean-receipt-child").as_str());
    let runtime = lifetime(dir.path(), &mob_id, &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("first lifetime");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(&runtime, &child_id, coordinator).await;
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let state = runtime.agent_mob_mcp_state().expect("agent mob-tool state");
    let deadline = release_deadline();
    // The first attempt shuts the child down cleanly.
    let (mut seen, mut child_mobs) = (std::collections::BTreeSet::new(), 0);
    runtime
        .stand_down_new_children(&state, deadline, &mut seen, &mut child_mobs)
        .await
        .expect("enumerate the children");
    assert_eq!(child_mobs, 1);
    // A retry enumerates the same, now stopped, incarnation again.
    let (mut seen, mut child_mobs) = (std::collections::BTreeSet::new(), 0);
    runtime
        .stand_down_new_children(&state, deadline, &mut seen, &mut child_mobs)
        .await
        .expect("enumerate the children again");
    assert_eq!(
        runtime.child_mob_root.outcome(child_mobs),
        crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs: 1 },
        "the clean receipt holds; no unknown debt"
    );
    drop(child);
    assert_eq!(
        runtime.stand_down_child_mobs(release_deadline()).await,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 }
    );
}

/// A child destroyed through its own handle stays in the agent mob-tool
/// state's managed set, and the stand-down still asks it although handle
/// snapshots withhold a destroyed mob: the answer is accounted for the
/// child instead of the stand-down passing over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_destroyed_through_its_own_handle_is_still_stood_down() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = lifetime(dir.path(), &unique("own-destroy-root"), &host, |spec| {
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let coordinator = coordinator_session(&runtime).await;
    let child_id = MobId::from(unique("own-destroy-child").as_str());
    let child = create_child(&runtime, &child_id, coordinator).await;
    child
        .destroy()
        .await
        .expect("destroy through the child's own handle");
    drop(child);
    let state = runtime.agent_mob_mcp_state().expect("agent mob-tool state");
    assert!(
        state
            .mob_handles_snapshot()
            .await
            .expect("snapshot")
            .iter()
            .all(|(mob_id, _)| mob_id != &child_id),
        "handle snapshots withhold the destroyed child"
    );
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let outcome = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(15))
        .await;
    let accounted = match &outcome {
        crate::child_mob_storage::ChildMobStandDownOutcome::Retained { unsettled } => unsettled
            .iter()
            .any(|entry| entry.mob_id == child_id.as_str()),
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs }
        | crate::child_mob_storage::ChildMobStandDownOutcome::HeldUntilProcessExit { child_mobs } => {
            *child_mobs == 1
        }
        _ => false,
    };
    assert!(
        accounted,
        "the stand-down asked the destroyed child: {outcome:?}"
    );
}

/// A child whose member's unregister admission was still held at the first
/// stand-down's deadline keeps the child root claimed. Once that exact
/// registration is free, a later stand-down in the same process settles the
/// pending unregister itself, without asking the exited child actor again
/// (its closed channel would read as unknown debt), and releases the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_unregister_admission_releases_the_root_once_it_settles() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let adapter = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&adapter);
    let runtime = lifetime(dir.path(), &unique("debt-root"), &host, |spec| {
        // The actual runtime adapter the child members register on.
        *captured.lock().expect("adapter slot") = spec.runtime_adapter.clone();
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let adapter = adapter
        .lock()
        .expect("adapter slot")
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(
        &runtime,
        &MobId::from(unique("debt-child").as_str()),
        coordinator,
    )
    .await;
    let sessions = worker_sessions(&child).await;

    // Hold the first worker's registration transaction, so its unregister
    // cannot be admitted before the stand-down deadline; its sibling is free.
    let guard = adapter
        .hold_session_registration_transaction_for_test(&sessions[0])
        .await;
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let first = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(3))
        .await;
    let crate::child_mob_storage::ChildMobStandDownOutcome::Retained { unsettled } = &first else {
        panic!("a held member must leave the first stand-down unsettled: {first:?}");
    };
    // Exactly one child carries debt.
    let [debt] = unsettled.as_slice() else {
        panic!("one child with debt expected: {unsettled:?}");
    };
    // The entry comes from the native shutdown report, not from a closed
    // channel or a failed request, and a later stand-down can clear it.
    assert_eq!(
        debt.kind,
        crate::child_mob_storage::RetainedChildMobKind::PendingUnregister,
        "{debt:?}"
    );
    assert!(
        debt.detail.starts_with("members not settled: "),
        "the debt must come from the native shutdown report: {debt:?}"
    );
    // The held member is pending exactly at its registration admission. Its
    // free sibling's unregister usually settles first; when it is still
    // running at the deadline (a loaded host), the native report truthfully
    // names it pending at that runtime unregister, never at admission and
    // never otherwise. The control after this test holds that sibling's
    // unregister deliberately.
    assert!(
        member_debt(&debt.detail, CHILD_WORKERS[0])
            .is_some_and(|entry| entry.contains(ADMISSION_PENDING)),
        "the held member is pending exactly at its registration admission: {debt:?}"
    );
    if let Some(sibling) = member_debt(&debt.detail, CHILD_WORKERS[1]) {
        assert!(
            sibling.contains(UNREGISTER_PENDING) && !sibling.contains(ADMISSION_PENDING),
            "the free member may only be pending at its runtime unregister: {debt:?}"
        );
    }
    assert_root_owned(dir.path());

    // Release the hold. The child actor's dispatch ends once its retained
    // completions settle, so a native shutdown call would then see the
    // closed channel rather than a second report. (`status()` maps a closed
    // actor to its last terminal phase, so it cannot tell.)
    drop(guard);
    tokio::time::timeout(Duration::from_secs(30), async {
        while !matches!(
            child
                .shutdown_with_report(meerkat_mob::ShutdownOptions::default())
                .await,
            Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed)
        ) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the child actor's dispatch ends after its shutdown");
    assert!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .is_some(),
        "the held member's registration is still current: only the settlement unregisters it"
    );
    let second = runtime
        .clone()
        .stand_down_child_mobs(release_deadline())
        .await;
    assert_eq!(
        second,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 },
        "the settled pending unregister clears the debt"
    );
    assert!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .is_none(),
        "the held member's registration is gone"
    );
    crate::child_mob_storage::ChildMobRootClaim::acquire(
        &crate::child_mob_storage::child_mob_root(dir.path()),
    )
    .expect("the released root is claimable in the same process");
}

/// A child member's admitted runtime unregister still running at the first
/// stand-down's deadline keeps the child root claimed. Once that exact
/// unregister settles, a later stand-down in the same process clears the
/// debt and releases the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_runtime_unregister_releases_the_root_once_it_settles() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let adapter = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&adapter);
    let runtime = lifetime(dir.path(), &unique("settle-root"), &host, |spec| {
        *captured.lock().expect("adapter slot") = spec.runtime_adapter.clone();
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let adapter = adapter
        .lock()
        .expect("adapter slot")
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(
        &runtime,
        &MobId::from(unique("settle-child").as_str()),
        coordinator,
    )
    .await;
    let sessions = worker_sessions(&child).await;

    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    // Armed after the root shutdown: the first unregister saga the child
    // stand-down starts holds before it unregisters.
    let (entered, release) = adapter.test_hold_next_unregister_saga();
    let first = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(2))
        .await;
    let crate::child_mob_storage::ChildMobStandDownOutcome::Retained { unsettled } = &first else {
        panic!("a held unregister must leave the first stand-down unsettled: {first:?}");
    };
    let [debt] = unsettled.as_slice() else {
        panic!("one child with debt expected: {unsettled:?}");
    };
    assert_eq!(
        debt.kind,
        crate::child_mob_storage::RetainedChildMobKind::PendingUnregister,
        "{debt:?}"
    );
    assert!(
        debt.detail.contains(UNREGISTER_PENDING) && !debt.detail.contains(ADMISSION_PENDING),
        "the held member is pending at its runtime unregister: {debt:?}"
    );
    assert_root_owned(dir.path());
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the unregister saga reached the hold")
        .expect("the saga hold reports its entry");

    let _ = release.send(());
    let second = runtime.stand_down_child_mobs(release_deadline()).await;
    assert_eq!(
        second,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 },
        "the settled pending unregister clears the debt"
    );
    for session in &sessions {
        assert!(
            adapter
                .current_session_registration_witness(session)
                .await
                .is_none(),
            "every child member's registration is gone"
        );
    }
    crate::child_mob_storage::ChildMobRootClaim::acquire(
        &crate::child_mob_storage::child_mob_root(dir.path()),
    )
    .expect("the released root is claimable in the same process");
}

/// The native report's entry for `worker` within one child's debt detail
/// (`members not settled: [(<identity>, <outcome>), ...]`): from its name up
/// to the next worker's entry.
fn member_debt<'a>(debt: &'a str, worker: &str) -> Option<&'a str> {
    let start = debt.find(worker)?;
    let end = CHILD_WORKERS
        .iter()
        .filter_map(|other| debt.find(other))
        .filter(|position| *position > start)
        .min()
        .unwrap_or(debt.len());
    Some(&debt[start..end])
}

/// The free sibling's pending report is truthful, not a misreport and not a
/// coupling to the held member: with its own unregister saga held before the
/// stand-down reports, the report names it pending at its runtime unregister
/// while the held member stays pending at admission; released while that
/// member is still held, the sibling settles on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_sibling_unregister_is_reported_pending_and_then_settles_independently() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let adapter = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&adapter);
    let runtime = lifetime(dir.path(), &unique("sibling-root"), &host, |spec| {
        *captured.lock().expect("adapter slot") = spec.runtime_adapter.clone();
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let adapter = adapter
        .lock()
        .expect("adapter slot")
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    let coordinator = coordinator_session(&runtime).await;
    let child = create_child(
        &runtime,
        &MobId::from(unique("sibling-child").as_str()),
        coordinator,
    )
    .await;
    let sessions = worker_sessions(&child).await;

    let guard = adapter
        .hold_session_registration_transaction_for_test(&sessions[0])
        .await;
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    // Armed after the root shutdown and before the report: the only
    // unregister saga the child stand-down starts is the sibling's, since the
    // held member's unregister cannot be admitted.
    let (entered, release) = adapter.test_hold_next_unregister_saga();
    let first = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(3))
        .await;
    let crate::child_mob_storage::ChildMobStandDownOutcome::Retained { unsettled } = &first else {
        panic!("held members must leave the stand-down unsettled: {first:?}");
    };
    let [debt] = unsettled.as_slice() else {
        panic!("one child with debt expected: {unsettled:?}");
    };
    assert_eq!(
        debt.kind,
        crate::child_mob_storage::RetainedChildMobKind::PendingUnregister,
        "{debt:?}"
    );
    assert!(
        member_debt(&debt.detail, CHILD_WORKERS[0])
            .is_some_and(|entry| entry.contains(ADMISSION_PENDING)),
        "the held member is pending exactly at its registration admission: {debt:?}"
    );
    assert!(
        member_debt(&debt.detail, CHILD_WORKERS[1])
            .is_some_and(|entry| entry.contains(UNREGISTER_PENDING)),
        "the sibling with a held saga is pending at its runtime unregister: {debt:?}"
    );
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the sibling's unregister saga reached the hold")
        .expect("the saga hold reports its entry");

    // Release only the sibling's saga; the held member stays held.
    let _ = release.send(());
    tokio::time::timeout(Duration::from_secs(30), async {
        while adapter
            .current_session_registration_witness(&sessions[1])
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the sibling settles while the other member stays held");
    assert!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .is_some(),
        "the held member is still registered: the sibling settled on its own"
    );
    drop(guard);
}

/// A later owner of a child member's registration keeps it, and a later
/// incarnation of the child is asked at the next stand-down instead of being
/// skipped as clean.
///
/// The first incarnation's stand-down leaves a member's unregister waiting
/// for admission (its registration transaction is held) at the deadline: a
/// pending unregister owed by that incarnation. That stand-down asks the
/// children directly, as a retry does, so the agent mob-tool state's
/// admission stays open. The child is then created again under the same id
/// (a new incarnation the state manages in place of the stopped one), and,
/// once the transaction is free, a later owner reuses the held member's
/// registration in place. The next stand-down asks the new incarnation and
/// follows the old debt only after that: the earlier incarnation's
/// settlement finds the registration handed over, which settles its debt
/// without unregistering it, and the root is released. The control is
/// `a_pending_unregister_admission_releases_the_root_once_it_settles`: with
/// no later owner, the settlement unregisters the member itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_earlier_incarnations_pending_unregister_leaves_a_later_owners_registration() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let adapter = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&adapter);
    let runtime = lifetime(dir.path(), &unique("handover-root"), &host, |spec| {
        *captured.lock().expect("adapter slot") = spec.runtime_adapter.clone();
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let adapter = adapter
        .lock()
        .expect("adapter slot")
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    let coordinator = coordinator_session(&runtime).await;
    let child_id = MobId::from(unique("handover-child").as_str());
    let first = create_child(&runtime, &child_id, coordinator).await;
    let sessions = worker_sessions(&first).await;
    let state = runtime.agent_mob_mcp_state().expect("agent mob-tool state");

    let guard = adapter
        .hold_session_registration_transaction_for_test(&sessions[0])
        .await;
    runtime
        .handle()
        .shutdown()
        .await
        .expect("root mob shutdown");
    let (mut seen, mut child_mobs) = (std::collections::BTreeSet::new(), 0);
    runtime
        .stand_down_new_children(
            &state,
            std::time::Instant::now() + Duration::from_secs(3),
            &mut seen,
            &mut child_mobs,
        )
        .await
        .expect("enumerate the children");
    let earlier = crate::child_mob_storage::ChildIncarnation::of(&first);
    assert!(
        runtime
            .child_mob_root
            .pending_in(child_id.as_str(), &earlier),
        "the first incarnation owes a pending unregister: {:?}",
        runtime.child_mob_root.outcome(child_mobs)
    );

    // The first incarnation's actor exits; its member's registration stays
    // current, since only its settlement would unregister it.
    drop(guard);
    tokio::time::timeout(Duration::from_secs(30), async {
        while !matches!(
            first
                .shutdown_with_report(meerkat_mob::ShutdownOptions::default())
                .await,
            Err(MobError::ActorCommandChannelClosed | MobError::ActorReplyChannelClosed)
        ) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the first incarnation's actor exits after its shutdown");
    let registration = adapter
        .current_session_registration_witness(&sessions[0])
        .await
        .expect("the held member's registration is still current");

    // The child again under the same id: a new incarnation, which the state
    // now manages in place of the stopped one.
    let model: Arc<dyn LlmClient> = host.model.clone();
    let later = MobBuilder::new(child_definition(&child_id), MobStorage::in_memory())
        .with_session_service(
            runtime
                .session_service()
                .expect("the runtime's session service")
                .clone(),
        )
        .with_default_llm_client(model)
        .create()
        .await
        .expect("the later incarnation");
    state
        .try_mob_insert_handle(child_id.clone(), later.clone())
        .await
        .expect("the state manages the later incarnation");
    // A later owner reuses the held member's registration in place.
    adapter
        .register_session(sessions[0].clone())
        .await
        .expect("a later owner reuses the registration");
    assert_eq!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .as_ref(),
        Some(&registration),
        "the reuse keeps the exact registration"
    );

    let second = runtime.stand_down_child_mobs(release_deadline()).await;
    assert_eq!(
        second,
        crate::child_mob_storage::ChildMobStandDownOutcome::Released { child_mobs: 1 },
        "the earlier incarnation's debt settles as handed over, and the later incarnation \
         shut down cleanly"
    );
    let status = later.status_observation_snapshot();
    assert!(
        !matches!(
            status,
            meerkat_mob::MobState::Creating | meerkat_mob::MobState::Running
        ),
        "the later incarnation was asked, not skipped as clean: {status:?}"
    );
    assert_eq!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .as_ref(),
        Some(&registration),
        "the earlier incarnation's debt never unregisters the later owner's registration"
    );
}

/// Unrelated ingress: worker-b's input while worker-a's cleanup is held.
const SIBLING_INPUT: &str = "acceptance-sibling-input";
/// worker-a's own input while its cleanup is held.
const HELD_MEMBER_INPUT: &str = "acceptance-held-member-input";
/// The member created while worker-a's cleanup is held.
const NEW_WORKER: &str = "worker-c";

/// Cleanup does not stall ingress. worker-a's retirement parks its runtime
/// unregister saga at the existing test hold, an awaited oneshot, so no
/// runtime worker is blocked. While it is held, worker-a's own input gets its
/// concrete native answer, worker-b's unrelated input is admitted and its turn
/// completes, and a new member is created and registered. Released, the
/// retirement completes and each admitted input reached exactly one turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingress_is_answered_while_a_member_cleanup_is_held() {
    // Run alone: ingress hops onto Core's process-wide machine cleanup
    // dispatcher. The held unregister saga is not on that dispatcher: it is
    // tokio::spawn on this test's ambient runtime (meerkat-runtime
    // session_management.rs:8390-8402), and the hold awaits there
    // (11129-11139).
    if !crate::test_process_isolation::run_body_in_isolated_process(
        module_path!(),
        "ingress_is_answered_while_a_member_cleanup_is_held",
    ) {
        return;
    }
    // A hang guard for each answer, inside the retirement's own budget.
    let answer_bound = Duration::from_secs(20);
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let adapter = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&adapter);
    let runtime = lifetime(dir.path(), &unique("ingress-root"), &host, |spec| {
        *captured.lock().expect("adapter slot") = spec.runtime_adapter.clone();
        spec.with_child_mob_storage(durable(dir.path()))
    })
    .await
    .expect("lifetime");
    let adapter = adapter
        .lock()
        .expect("adapter slot")
        .clone()
        .expect("the persistent composition carries its runtime adapter");
    let coordinator = coordinator_session(&runtime).await;
    let child_id = MobId::from(unique("ingress-child").as_str());
    let child = create_child(&runtime, &child_id, coordinator).await;
    let sessions = worker_sessions(&child).await;
    let state = runtime.agent_mob_mcp_state().expect("agent mob state");
    let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
    let input =
        |text: &str| WorkSpec::new(ContentInput::Text(text.to_string()), WorkOrigin::Internal);

    // Lengthen the plain unregister's caller grace (2 s by default) so the
    // retirement waits on the held saga instead of answering in progress
    // while the test still holds it.
    adapter.test_set_unregister_caller_wait_grace(Duration::from_mins(2));
    let (entered, release) = adapter.test_hold_next_unregister_saga();
    let retiring = {
        let child = child.clone();
        tokio::spawn(async move { child.retire(AgentIdentity::from(CHILD_WORKERS[0])).await })
    };
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .expect("worker-a's unregister saga reached the hold")
        .expect("the saga hold reports its entry");

    // Unrelated ingress succeeds: admitted, and its turn completes.
    let sibling = tokio::time::timeout(
        answer_bound,
        child.start_work_for_identity_bounded(
            AgentIdentity::from(CHILD_WORKERS[1]),
            input(SIBLING_INPUT),
            HandlingMode::Queue,
            spec.clone(),
        ),
    )
    .await
    .expect("worker-b's ingress is answered while worker-a's cleanup is held")
    .expect("worker-b's ingress is admitted while worker-a's cleanup is held");
    tokio::time::timeout(answer_bound, sibling.wait_bounded(spec.clone()))
        .await
        .expect("worker-b's turn completes while worker-a's cleanup is held")
        .expect("worker-b's turn succeeds");

    // The held member's own ingress gets its concrete native answer.
    let held_member = tokio::time::timeout(
        answer_bound,
        child.start_work_for_identity_bounded(
            AgentIdentity::from(CHILD_WORKERS[0]),
            input(HELD_MEMBER_INPUT),
            HandlingMode::Queue,
            spec.clone(),
        ),
    )
    .await
    .expect("worker-a's ingress is answered while its cleanup is held");
    // The retirement has already removed worker-a's machine-owned session
    // binding while worker-a is still Retiring, so admission refuses the work
    // at once with the mob machine's own typed refusal for a retiring member.
    assert!(
        matches!(
            &held_member,
            Err(meerkat_mob::MobError::MemberNotFound(member))
                if member == &AgentIdentity::from(CHILD_WORKERS[0])
        ),
        "worker-a's ingress during its held cleanup: {:?}",
        held_member.as_ref().map(|_| "admitted")
    );

    // A new member is created and registered.
    tokio::time::timeout(
        answer_bound,
        state.mob_spawn(
            &child_id,
            ProfileName::from("worker"),
            AgentIdentity::from(NEW_WORKER),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        ),
    )
    .await
    .expect("member creation is answered while worker-a's cleanup is held")
    .expect("a new member is created while worker-a's cleanup is held");
    let created = child
        .resolve_bridge_session_id(&AgentIdentity::from(NEW_WORKER))
        .await
        .expect("the new member's session");
    assert!(
        adapter
            .current_session_registration_witness(&created)
            .await
            .is_some(),
        "the new member is registered"
    );

    // The cleanup was genuinely held throughout.
    assert!(
        adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .is_some(),
        "worker-a's cleanup is still held: its registration stands"
    );
    assert!(
        !retiring.is_finished(),
        "worker-a's retirement is still waiting on its held cleanup"
    );

    let _ = release.send(());
    let retired = tokio::time::timeout(answer_bound, retiring)
        .await
        .expect("the retirement completes once its cleanup is released")
        .expect("the retirement task");
    assert!(
        retired.is_ok(),
        "worker-a's retirement completes once its cleanup is released: {retired:?}"
    );
    tokio::time::timeout(answer_bound, async {
        while adapter
            .current_session_registration_witness(&sessions[0])
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("worker-a's registration clears after the release");

    assert_eq!(
        host.model.requests_carrying(SIBLING_INPUT),
        1,
        "worker-b's input reached exactly one turn"
    );
    assert_eq!(
        host.model.requests_carrying(HELD_MEMBER_INPUT),
        0,
        "worker-a's refused input reached no turn"
    );
}

// ---------------------------------------------------------------------------
// R2: process-bound child mobs stay the default and are not restored
// ---------------------------------------------------------------------------

#[test]
fn process_bound_child_mobs_are_not_restored() {
    let dir = tempfile::tempdir().expect("state dir");
    run_lifetime("bound:1", dir.path());
    assert!(
        !crate::child_mob_storage::child_mob_root(dir.path()).exists(),
        "process-bound child mobs never open the child root"
    );
    run_lifetime("bound:2", dir.path());
}

// ---------------------------------------------------------------------------
// R3: maintenance enumerates the child databases; destroy removes them
// ---------------------------------------------------------------------------

fn child_event_logs(dir: &Path) -> Vec<PathBuf> {
    crate::child_mob_storage::child_mob_databases(dir)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mob--"))
        })
        .collect()
}

#[test]
fn storage_maintenance_covers_the_child_databases() {
    let dir = tempfile::tempdir().expect("state dir");
    run_lifetime("maint:1", dir.path());

    // The lifetime process has exited, so the child root is free and no
    // store is open.
    let child_root = crate::child_mob_storage::child_mob_root(dir.path());
    let children = crate::child_mob_storage::child_mob_databases(dir.path());
    assert_eq!(child_event_logs(dir.path()).len(), 1, "{children:?}");
    assert!(
        children.contains(&child_root.join(REALM_PROFILE_DATABASE)),
        "the durable realm profile store is enumerated: {children:?}"
    );
    let fenced = crate::storage_migrate::enumerate_state_dir_databases(dir.path());
    for child in &children {
        assert!(fenced.contains(child), "{} is not fenced", child.display());
    }
    let diagnosis = crate::storage_doctor::diagnose_state_dir_blocking(
        &meerkat_core::storage_diagnostics::DiagnoseScope::new(vec![dir.path().to_path_buf()]),
        None,
    );
    let inventoried: Vec<&Path> = diagnosis
        .inventory
        .iter()
        .flat_map(|entry| entry.databases.iter().map(|db| db.path.as_path()))
        .collect();
    for child in &children {
        assert!(
            inventoried.contains(&child.as_path()),
            "{} is not inventoried",
            child.display()
        );
    }
    let report = crate::storage_migrate::migrate_state_dir(
        dir.path(),
        crate::storage_migrate::MigrateMode::Apply,
        None,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for child in &children {
        assert!(
            report.ledger.iter().any(|entry| &entry.database == child
                && entry.domain == "mob"
                && entry.after.is_some()),
            "{} has no applied `mob` ledger row",
            child.display()
        );
    }

    // A destroyed child leaves no event log behind and is not restored.
    run_lifetime("maint:2", dir.path());
    assert!(
        child_event_logs(dir.path()).is_empty(),
        "a destroyed child leaves no event log"
    );
    run_lifetime("maint:3", dir.path());
}

// ---------------------------------------------------------------------------
// R5: a storage-only late selection rebuilds; explicit ProcessBound never opens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_late_storage_only_selection_rebuilds_the_agent_state() {
    let dir = tempfile::tempdir().expect("state dir");
    let mut spec = composed_spec(dir.path(), custody_free_storage());
    let initial = spec.agent_mob_mcp_state.clone().expect("initial state");
    let generation = initial
        .continuation_binding_generation()
        .expect("initial continuation binding");
    // Storage is the only changed input: empty child policy, same (absent)
    // participant store.
    spec = spec.with_child_mob_storage(durable(dir.path()));
    spec.apply_agent_mob_child_policy()
        .expect("apply the durable child root");
    let state = spec.agent_mob_mcp_state.as_ref().expect("final state");
    assert!(!Arc::ptr_eq(&initial, state));
    assert_ne!(state.continuation_binding_generation(), Some(generation));
    let child_root = crate::child_mob_storage::child_mob_root(dir.path());
    assert_eq!(
        spec.child_mob_root_claim
            .as_ref()
            .map(|claim| claim.root().to_path_buf()),
        Some(child_root.clone())
    );
    // D2: the native durable realm-profile store sits under the overlay.
    assert!(child_root.join(REALM_PROFILE_DATABASE).is_file());
    // D3: with no root custody, the state's default custody became durable.
    assert!(
        meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(dir.path())
            .is_file()
    );
    // R7: councils use Meta's canonical council slot.
    assert!(
        MetaStorageLayout::with_injected_roots(dir.path().to_path_buf(), None)
            .council_db()
            .is_file()
    );
}

/// A composed spec over a persistent root mob storage that records its Meta
/// layout and makes no child-mob selection.
fn persistent_root_spec(dir: &Path) -> MobBootstrapSpec {
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
            .expect("root mob storage");
    composed_spec(dir, storage)
        .with_mob_storage_provenance(provenance)
        .with_meta_storage_layout(MetaStorageLayout::with_injected_roots(
            dir.to_path_buf(),
            None,
        ))
}

#[tokio::test]
async fn an_absent_selection_is_durable_only_over_a_persistent_root() {
    // Persistent root, recorded layout: the default claims the durable root.
    let dir = tempfile::tempdir().expect("state dir");
    let mut spec = persistent_root_spec(dir.path());
    spec.apply_agent_mob_child_policy()
        .expect("the default durable root opens");
    assert!(spec.child_mob_root_claim.is_some());
    assert_eq!(spec.child_mob_storage, Some(durable(dir.path())));
    drop(spec);

    // No persistent root: process-bound, nothing opened.
    let dir = tempfile::tempdir().expect("state dir");
    let mut spec = composed_spec(dir.path(), custody_free_storage()).with_meta_storage_layout(
        MetaStorageLayout::with_injected_roots(dir.path().to_path_buf(), None),
    );
    spec.apply_agent_mob_child_policy()
        .expect("process-bound child mobs open nothing");
    assert!(spec.child_mob_root_claim.is_none());
    assert!(!crate::child_mob_storage::child_mob_root(dir.path()).exists());

    // Persistent root but no recorded layout: a typed refusal, never an
    // implicit process-bound fallback, and nothing is opened.
    let dir = tempfile::tempdir().expect("state dir");
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.path().join("mob.sqlite3"))
            .expect("root mob storage");
    let mut spec = composed_spec(dir.path(), storage).with_mob_storage_provenance(provenance);
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::LayoutNotRecorded
    ));
    assert!(spec.child_mob_root_claim.is_none());
    assert!(!crate::child_mob_storage::child_mob_root(dir.path()).exists());

    // A recorded layout declared ephemeral is refused, not replaced.
    let dir = tempfile::tempdir().expect("state dir");
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.path().join("mob.sqlite3"))
            .expect("root mob storage");
    let mut spec = composed_spec(dir.path(), storage)
        .with_mob_storage_provenance(provenance)
        .with_meta_storage_layout(MetaStorageLayout::declared_ephemeral(
            dir.path().join("scratch"),
        ));
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::DeclaredEphemeralLayout { .. }
    ));
    assert!(spec.child_mob_root_claim.is_none());

    // An explicit process-bound choice over a persistent root is preserved.
    let dir = tempfile::tempdir().expect("state dir");
    let mut spec =
        persistent_root_spec(dir.path()).with_child_mob_storage(ChildMobStorage::ProcessBound);
    spec.apply_agent_mob_child_policy()
        .expect("explicit process-bound is honored");
    assert!(spec.child_mob_root_claim.is_none());
    assert!(!crate::child_mob_storage::child_mob_root(dir.path()).exists());
}

#[tokio::test]
async fn an_explicit_process_bound_selection_never_opens_the_child_root() {
    let dir = tempfile::tempdir().expect("state dir");
    // An unusable child root: a regular file where the directory would be.
    let child_root = crate::child_mob_storage::child_mob_root(dir.path());
    std::fs::write(&child_root, b"not a directory").expect("occupy the child root");
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()))
        .with_child_mob_storage(ChildMobStorage::ProcessBound);
    spec.apply_agent_mob_child_policy()
        .expect("process-bound child mobs open nothing");
    assert!(spec.child_mob_root_claim.is_none());
    assert!(child_root.is_file());
}

#[tokio::test]
async fn durable_child_mobs_refuse_process_bound_participant_custody() {
    let dir = tempfile::tempdir().expect("state dir");
    // In-memory root storage carries in-memory custody and declares no
    // persistent provenance.
    let mut spec = composed_spec(dir.path(), MobStorage::in_memory())
        .with_child_mob_storage(durable(dir.path()));
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::ProcessBoundParticipantCustody
    ));
    assert!(spec.child_mob_root_claim.is_none());
    assert!(!crate::child_mob_storage::child_mob_root(dir.path()).exists());
}

// ---------------------------------------------------------------------------
// R6: authored realm profiles survive under the definition overlay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authored_realm_profiles_survive_under_the_definition_overlay() {
    let dir = tempfile::tempdir().expect("state dir");
    let authored = inline_profile("shared", "gpt-5.5");
    let inline = inline_profile("shared", "gpt-5.5-mini");
    let profile_store = |spec: &MobBootstrapSpec| {
        spec.agent_mob_mcp_state
            .as_ref()
            .and_then(|state| state.realm_profile_store().cloned())
            .expect("realm profile store")
    };

    // Lifetime A: no inline profile; an agent authors "shared".
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    spec.apply_agent_mob_child_policy().expect("lifetime A");
    profile_store(&spec)
        .create("shared", &authored)
        .await
        .expect("author a realm profile");
    drop(spec);

    // Lifetime B: the definition now carries an inline "shared". It wins on
    // read, refuses mutation, and the authored row stays underneath.
    let mut definition = root_definition(&unique("overlay"));
    definition.profiles.insert(
        ProfileName::from("shared"),
        meerkat_mob::ProfileBinding::Inline(Box::new(inline.clone())),
    );
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    spec.definition = definition;
    spec.apply_agent_mob_child_policy().expect("lifetime B");
    let store = profile_store(&spec);
    assert_eq!(
        store
            .get("shared")
            .await
            .expect("read")
            .map(|row| row.profile),
        Some(inline.clone())
    );
    assert!(store.update("shared", &inline, 1).await.is_err());
    drop(store);
    drop(spec);

    // Lifetime C: the inline definition is gone; the unchanged authored row
    // is visible again.
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    spec.apply_agent_mob_child_policy().expect("lifetime C");
    assert_eq!(
        profile_store(&spec)
            .get("shared")
            .await
            .expect("read")
            .map(|row| row.profile),
        Some(authored)
    );
}

// ---------------------------------------------------------------------------
// R8: durable composition fails closed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_owner_is_refused_until_the_first_composition_is_dropped() {
    let dir = tempfile::tempdir().expect("state dir");
    let mut first = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    first.apply_agent_mob_child_policy().expect("first owner");
    // Refused even though the child root is still empty.
    let mut second = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    assert!(matches!(
        child_storage_error(second.apply_agent_mob_child_policy()),
        ChildMobStorageError::AlreadyClaimed { .. }
    ));
    // The default is no way around the claim: an absent selection over a
    // persistent root resolves to the same durable root and is refused too.
    let mut defaulted = persistent_root_spec(dir.path());
    assert!(matches!(
        child_storage_error(defaulted.apply_agent_mob_child_policy()),
        ChildMobStorageError::AlreadyClaimed { .. }
    ));
    drop(first);
    let mut successor = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    successor.apply_agent_mob_child_policy().expect(
        "a successor is admitted once the first composition, which started nothing, is dropped",
    );
}

#[tokio::test]
async fn an_unusable_child_root_fails_composition() {
    let dir = tempfile::tempdir().expect("state dir");
    std::fs::write(
        crate::child_mob_storage::child_mob_root(dir.path()),
        b"not a directory",
    )
    .expect("occupy the child root");
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::RootUnavailable { .. }
    ));
}

#[tokio::test]
async fn an_unreadable_council_store_fails_composition() {
    let dir = tempfile::tempdir().expect("state dir");
    // A directory where the council database belongs: present, unopenable.
    std::fs::create_dir_all(
        MetaStorageLayout::with_injected_roots(dir.path().to_path_buf(), None).council_db(),
    )
    .expect("occupy the council path");
    let mut spec = composed_spec(dir.path(), custody_free_storage())
        .with_child_mob_storage(durable(dir.path()));
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::CouncilStore(_)
    ));
}

#[tokio::test]
async fn a_declared_ephemeral_layout_cannot_hold_durable_child_mobs() {
    let dir = tempfile::tempdir().expect("scratch dir");
    let mut spec = composed_spec(dir.path(), custody_free_storage()).with_child_mob_storage(
        ChildMobStorage::Durable(MetaStorageLayout::declared_ephemeral(
            dir.path().to_path_buf(),
        )),
    );
    assert!(matches!(
        child_storage_error(spec.apply_agent_mob_child_policy()),
        ChildMobStorageError::DeclaredEphemeralLayout { .. }
    ));
    assert!(!crate::child_mob_storage::child_mob_root(dir.path()).exists());
}

/// The weak identity-runtime back-link holds none of the runtime's
/// composition. With agent mob tools composed and the identity runtime
/// installed in the composition's slot, and before any shutdown,
/// downgrading the public view adds no strong reference to the composition
/// release, the identity runtime slot, the session service, the mob-tool
/// state or the identity runtime itself. A cloned `MobRuntime` observer
/// (2098603c) added to the release (`observer -> CompositionRelease -> tool
/// slot -> identity runtime slot`), and an observer `MobHandle` (204060a4)
/// still added to the session service. It still upgrades while the mob is
/// owned, and not after the idle mob's admission closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_weak_identity_runtime_retains_none_of_the_agent_tool_composition() {
    let dir = tempfile::tempdir().expect("state dir");
    let host = Host::new();
    let runtime = lifetime(dir.path(), &unique("weak-composition"), &host, |spec| spec)
        .await
        .expect("lifetime");
    assert!(
        runtime.agent_mob_mcp_state().is_some(),
        "agent mob tools are composed"
    );
    let identity = Arc::new(
        crate::identity_first::IdentityRuntime::new(crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "weak-composition".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        })
        // Like the composition: the identity runtime's own non-owning mob handle.
        .with_runtime_services(crate::identity_first::AgentRuntimeServices::new(
            runtime.handle().observer(),
        )),
    );
    runtime.install_identity_runtime_authority(Arc::clone(&identity));
    let counts = |runtime: &MobRuntime| {
        (
            runtime
                .composition_release
                .as_ref()
                .map(|release| Arc::strong_count(&release.0)),
            runtime
                .identity_runtime_slot
                .as_ref()
                .map(Arc::strong_count),
            runtime.session_service.as_ref().map(Arc::strong_count),
            runtime.agent_mob_mcp_state.as_ref().map(Arc::strong_count),
        )
    };
    let view =
        crate::identity_first::PublicIdentityRuntime::new(Arc::clone(&identity), runtime.clone());
    let before = (counts(&runtime), Arc::strong_count(&identity));
    assert!(
        before.0.0.is_some() && before.0.1.is_some(),
        "the composition release and the identity runtime slot exist"
    );
    let weak = view.downgrade();
    assert_eq!(
        (counts(&runtime), Arc::strong_count(&identity)),
        before,
        "the weak link holds no strong reference into the composition"
    );

    drop(view);
    let upgraded = weak.upgrade().expect("upgrades while the mob is owned");
    assert!(Arc::ptr_eq(upgraded.runtime(), &identity));
    drop(upgraded);

    // Releasing the last public owner lets the idle mob close its
    // admission; the weak link is not an owner, so it does not hold it open.
    let mut closed = runtime.handle().observer().admission_closed();
    runtime.clear_identity_runtime_authority();
    drop(runtime);
    tokio::time::timeout(Duration::from_secs(10), async {
        while closed.borrow_and_update().is_none() {
            if closed.changed().await.is_err() {
                return;
            }
        }
    })
    .await
    .expect("the idle mob closes once its owners are gone");
    assert!(closed.borrow().is_some(), "admission closed");
    assert!(
        weak.upgrade().is_none(),
        "no public owner after the admission closed"
    );
}

/// A bundle offered to child mobs is forwarded to child mob creation like a
/// child-available one, so a create can check its profiles' opt-ins against
/// the offer even when nothing else is forwarded. A host-only bundle alone
/// forwards nothing.
#[test]
fn an_offered_bundle_is_forwarded_to_child_mob_creation() {
    let dir = tempfile::tempdir().expect("dir");
    let forwards = |availability| {
        let tools: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(HostTools(Arc::new(Mutex::new(Vec::new()))));
        !MobBootstrapSpec::ephemeral(
            root_definition(&unique("mob-offer")),
            meerkat_mob::MobStorage::in_memory(),
            dir.path().to_path_buf(),
            4,
            None,
        )
        .expect("spec")
        .register_tool_bundle_with_availability("host", tools, availability)
        .agent_mob_child_policy()
        .is_empty()
    };
    assert!(forwards(
        meerkat_mob_mcp::ChildToolBundleAvailability::ChildOffered
    ));
    assert!(forwards(
        meerkat_mob_mcp::ChildToolBundleAvailability::ChildAvailable
    ));
    assert!(!forwards(
        meerkat_mob_mcp::ChildToolBundleAvailability::HostOnly
    ));
}
