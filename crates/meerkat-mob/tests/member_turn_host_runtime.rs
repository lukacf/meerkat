//! A runtime-backed member's turns run on the runtime that provisions the
//! member, which is the host's, and not on Core's process-wide machine
//! cleanup runtime.
//!
//! The mob actor provisions a member from its own runtime, and the member's
//! session actor is materialized there, so the member's agent loop, its tool
//! calls and whatever those spawn run on the host's workers, with their
//! stack. Before, the actor was materialized from a task on the cleanup
//! runtime (`meerkat-machine-cleanup`, two workers), its session task was
//! spawned on that runtime, and every member turn ran there: an embedding
//! host whose own workers had more stack than those threads aborted with a
//! stack overflow on `meerkat-machine-cleanup` once a member's tool needed
//! more than the cleanup threads had.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, Message, ToolDispatchOutcome, ToolError};
use meerkat_mob::{
    AgentIdentity, MemberTurnOptions, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};

/// Bound for one step of a scenario; it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(60);
/// The host tool the scripted model calls once.
const PROBE_TOOL: &str = "host_probe";
/// The name of the host runtime's threads in these tests.
const HOST_WORKER_THREAD: &str = "member-host-worker";
/// The name of the threads of Core's process-wide machine cleanup runtime.
const MACHINE_CLEANUP_THREAD: &str = "meerkat-machine-cleanup";
/// The worker stack of the host runtime in [`member_tool_stack_probe_child`]:
/// 32 MiB, as an embedding host may give its own workers.
const HOST_WORKER_STACK_BYTES: usize = 32 * 1024 * 1024;
/// The frame the stack probe touches: more than the 16 MiB the machine
/// cleanup runtime's threads take without a stack knob, and less than the
/// host's 32 MiB workers.
const STACK_PROBE_FRAME_BYTES: usize = 24 * 1024 * 1024;
/// Arms [`member_tool_stack_probe_child`]. Without it the fixture does
/// nothing, so a run of ignored tests stays harmless.
const STACK_CHILD_ENV: &str = "MEERKAT_TEST_MEMBER_HOST_STACK_CHILD";
/// The child-process fixture's test name, as libtest's `--exact` matches it.
const STACK_CHILD_TEST: &str = "member_tool_stack_probe_child";
/// The stack of the child's driver thread, which builds the scenario and
/// waits on it. The member's turn, and its tool, run on the workers.
const STACK_CHILD_DRIVER_STACK_BYTES: usize = 8 * 1024 * 1024;
/// How long the parent waits for the child test process before killing it.
const STACK_CHILD_DEADLINE: Duration = Duration::from_secs(300);

/// First request calls [`PROBE_TOOL`]; the request carrying its result ends
/// the turn.
struct ProbeThenDoneClient;

fn done_events(
    request: &LlmRequest,
    mut events: Vec<LlmEvent>,
    stop: meerkat_core::StopReason,
) -> Vec<LlmEvent> {
    events.push(LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        ),
    });
    events.push(LlmEvent::Done {
        outcome: LlmDoneOutcome::success(stop),
    });
    events
}

#[async_trait::async_trait]
impl LlmClient for ProbeThenDoneClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let has_tool_result = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::ToolResults { .. }));
        let events = if has_tool_result {
            done_events(
                request,
                vec![LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            )
        } else {
            done_events(
                request,
                vec![LlmEvent::ToolCallComplete {
                    id: "call-host-probe".to_string(),
                    name: PROBE_TOOL.to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                }],
                meerkat_core::StopReason::ToolUse,
            )
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Reserve and touch a [`STACK_PROBE_FRAME_BYTES`] frame on the calling
/// thread's stack, as a deep host tool does; `black_box` keeps the array in
/// any build profile.
#[inline(never)]
#[allow(clippy::large_stack_arrays)]
fn touch_stack_probe_frame() -> u8 {
    let mut frame = [0_u8; STACK_PROBE_FRAME_BYTES];
    std::hint::black_box(&mut frame);
    frame[STACK_PROBE_FRAME_BYTES - 1]
}

/// A host tool that records the name of the thread each of its calls runs
/// on and, with `touch_frame`, first touches the stack probe frame there.
struct ProbeTool {
    touch_frame: bool,
    threads: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for ProbeTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            PROBE_TOOL,
            "Host probe.",
            serde_json::json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if self.touch_frame {
            assert_eq!(touch_stack_probe_frame(), 0);
        }
        self.threads
            .lock()
            .unwrap()
            .push(std::thread::current().name().map(str::to_owned));
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }
}

async fn build_service(
    root: &Path,
    client: Arc<ProbeThenDoneClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "member-host-runtime-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client);
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
    .expect("compose the runtime-backed service")
}

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("peer"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "gpt-5.5".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "team peer".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "member-host-runtime-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// Build a runtime-backed mob from the calling runtime, spawn one member
/// whose host tool is [`ProbeTool`], and run one scripted turn that calls it.
/// Returns the names of the threads the tool's calls ran on.
async fn run_one_probe_turn(touch_frame: bool) -> Vec<Option<String>> {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = Arc::new(ProbeThenDoneClient);
    let (service, adapter) = build_service(temp.path(), Arc::clone(&client)).await;
    let storage = MobStorage::persistent(temp.path().join("mob.db")).expect("mob storage");
    let handle = MobBuilder::new(mob_definition(), storage)
        .with_session_service(service)
        .with_runtime_adapter(adapter)
        .with_default_llm_client(client)
        .create()
        .await
        .expect("create mob");
    let threads = Arc::new(Mutex::new(Vec::new()));
    let peer = AgentIdentity::from("probe");
    let mut spec = SpawnMemberSpec::new("peer", peer.clone());
    spec.external_tools = Some(Arc::new(ProbeTool {
        touch_frame,
        threads: Arc::clone(&threads),
    }));
    tokio::time::timeout(STEP, handle.spawn_spec(spec))
        .await
        .expect("the spawn completes")
        .expect("spawn the member");
    let member = handle.member(&peer).await.expect("member handle");
    let turn = member
        .start_turn(
            "call the host probe",
            HandlingMode::Queue,
            MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("start the turn");
    tokio::time::timeout(STEP, turn.wait())
        .await
        .expect("the turn completes")
        .expect("the turn succeeds");
    let recorded = threads.lock().unwrap().clone();
    let _ = tokio::time::timeout(STEP, handle.shutdown()).await;
    recorded
}

/// The host tool a runtime-backed member calls in its turn runs on a worker
/// of the runtime that provisioned the member, never on
/// `meerkat-machine-cleanup`.
#[test]
fn a_runtime_backed_members_tool_runs_on_the_runtime_that_provisioned_it() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(HOST_WORKER_THREAD)
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("host runtime");
    let threads = runtime.block_on(run_one_probe_turn(false));
    runtime.shutdown_timeout(Duration::from_secs(10));
    assert_eq!(threads.len(), 1, "the probe tool ran once: {threads:?}");
    let thread = threads[0].as_deref();
    assert_ne!(
        thread,
        Some(MACHINE_CLEANUP_THREAD),
        "the member's tool ran on Core's machine cleanup runtime instead of the runtime \
         that provisioned the member"
    );
    assert_eq!(
        thread,
        Some(HOST_WORKER_THREAD),
        "the member's tool runs on a worker of the runtime that provisioned the member"
    );
}

/// Whether `output` reports a stack overflow on a thread named `thread` (std
/// prints the thread's id between the two).
fn reports_stack_overflow(output: &str, thread: &str) -> bool {
    let prefix = format!("thread '{thread}'");
    output
        .lines()
        .any(|line| line.starts_with(&prefix) && line.ends_with("has overflowed its stack"))
}

/// The last lines of the child test process's output, where a stack overflow
/// names its thread.
fn output_tail(path: &Path) -> String {
    let output = std::fs::read_to_string(path).unwrap_or_default();
    let lines = output.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(80)..].join("\n")
}

/// An embedding host whose runtime gives its workers 32 MiB, with neither
/// `RKAT_WORKER_STACK_BYTES` nor `RUST_MIN_STACK` set, runs a runtime-backed
/// member whose host tool touches a 24 MiB frame, through one scripted turn,
/// and the turn completes on the host's workers. While member turns ran on
/// the machine cleanup runtime, whose threads take 16 MiB without a knob
/// (they took 8 MiB before the process runtimes' own default rose), the
/// process aborted with a stack overflow on `meerkat-machine-cleanup` once a
/// tool's frame was deeper than that. A stack overflow aborts the whole
/// process, so the scenario runs in a child process, and this test asserts
/// that the child exits successfully.
#[test]
fn a_member_tool_frame_the_hosts_workers_hold_completes_without_a_stack_knob() {
    let output_dir = tempfile::tempdir().expect("child output dir");
    let output_path = output_dir.path().join("child-output.txt");
    let stdout = std::fs::File::create(&output_path).expect("child output file");
    let stderr = stdout.try_clone().expect("child output file");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            STACK_CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(STACK_CHILD_ENV, "1")
        .env_remove(meerkat_runtime::host_stack::HOST_WORKER_STACK_ENV)
        .env_remove("RUST_MIN_STACK")
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start the child test process");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the child test process") {
            break status;
        }
        if started.elapsed() > STACK_CHILD_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the child test process did not finish within {STACK_CHILD_DEADLINE:?}\n{}",
                output_tail(&output_path)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = std::fs::read_to_string(&output_path).unwrap_or_default();
    let overflowed = reports_stack_overflow(&output, MACHINE_CLEANUP_THREAD);
    assert!(
        status.success() && output.contains("test result: ok. 1 passed"),
        "a member tool's {STACK_PROBE_FRAME_BYTES}-byte frame must complete on a host runtime \
         with {HOST_WORKER_STACK_BYTES}-byte workers and no stack knob; the child exited with \
         {status} (stack overflow on {MACHINE_CLEANUP_THREAD}: {overflowed})\n{}",
        output_tail(&output_path)
    );
}

/// The child process of
/// [`a_member_tool_frame_the_hosts_workers_hold_completes_without_a_stack_knob`]:
/// a host runtime with 32 MiB workers runs the scenario, and the member's
/// tool touches the stack probe frame.
#[test]
#[ignore = "child-process fixture of a_member_tool_frame_the_hosts_workers_hold_completes_without_a_stack_knob"]
fn member_tool_stack_probe_child() {
    if std::env::var_os(STACK_CHILD_ENV).is_none() {
        return;
    }
    let driver = std::thread::Builder::new()
        .name("member-host-driver".into())
        .stack_size(STACK_CHILD_DRIVER_STACK_BYTES)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name(HOST_WORKER_THREAD)
                .thread_stack_size(HOST_WORKER_STACK_BYTES)
                .enable_all()
                .build()
                .expect("host runtime");
            let threads = runtime.block_on(run_one_probe_turn(true));
            runtime.shutdown_timeout(Duration::from_secs(10));
            threads
        })
        .expect("start the driver thread");
    let threads = match driver.join() {
        Ok(threads) => threads,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    assert_eq!(
        threads,
        vec![Some(HOST_WORKER_THREAD.to_string())],
        "the member's tool ran once, on a worker of the host's runtime"
    );
}
