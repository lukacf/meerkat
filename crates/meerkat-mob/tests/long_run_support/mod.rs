//! Core over one store, driven through the long run, compacted long run and
//! durable worker chapters of `meerkat-store-conformance`.
//!
//! Every row runs one mob over a fresh store. Its `worker` member is a
//! durable worker (its profile sets `durable`) whose scripted model and
//! scripted tools play the row's plan; a `peer` member joins when the plan
//! kills inside a comms call. The first incarnation is killed at the plan's
//! kill point and a second one restarts over the same store:
//!
//! - over the built-in SQLite realm, the first incarnation is a child
//!   process (this test binary again) killed with SIGKILL, and the restart
//!   runs in the test process over the same files;
//! - over the reference host store, the first incarnation runs on a tokio
//!   runtime of its own that is shut down at the kill point, dropping every
//!   Core object, and the restart reopens Core over the same store
//!   instance, taking over the killed incarnation's fence.
//!
//! The restarted mob installs a harness `DurableResumeGate`. The runtime asks
//! it before it resumes the worker's run: that is where the long run rows
//! queue a second input, stop or retire the runtime "before the resume", and
//! where the durable worker rows answer with the row's verdict.
//!
//! A compaction race (`KillPoint::RebaseRace`) is played at the compaction
//! request of its round: before the model answers it, a second Core over the
//! same store takes the store over and appends one System message of its
//! own, so the run's compaction commit is refused.
//!
//! A deploy row ([`DeployConfig`]) ends its first incarnation the way a host
//! deploys instead of killing it: once every durable worker's run is in
//! flight at the plan's stop point, the incarnation gets SIGTERM (in process,
//! its stand-in) and runs the mob's terminal Shutdown inside the host's
//! budget, then ends. Its two-worker form also runs a second durable worker,
//! `worker2`, whose input is tagged `second`, and can kill the process while
//! the Shutdown drops the last worker's in-flight call.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::any::Any;
use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::service::SessionServiceControlExt as _;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentEvent, AssistantBlock, Message, ToolCallDedupeKey};
use meerkat_mob::{
    AgentIdentity, DurableResumeGate, DurableResumeRequest, DurableResumeVerdict, MobBuilder,
    MobDefinition, MobHandle, MobId, MobRuntimeMode, MobStorage, Profile, ProfileBinding,
    ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::terminal_status::InteractionSelector;
use meerkat_runtime::{
    AcceptOutcome, Input, LogicalRuntimeId, MeerkatMachine, PromptInput,
    RuntimeSessionPersistenceProfile, RuntimeStore,
};
use meerkat_store_conformance::{
    BeforeResume, KillPoint, KilledRun, LongRunPlan, RestartReport, ResumeGate, ResumePointKind,
    ResumedRun, RoundSet, UnrepeatableTool, call_id,
};

/// The realm every row opens.
const REALM: &str = "long-run-realm";
/// The durable worker that runs the plan.
pub const WORKER: &str = "worker";
/// The peer a comms call of the plan addresses.
pub const PEER: &str = "peer";
/// The input whose run the plan scripts.
pub const MAIN: &str = "main";
/// The input a `QueueInput` row queues behind the resumed run, and the
/// input a deploy row runs after the restart.
pub const QUEUED: &str = "queued";
/// The second durable worker of a two-worker deploy row.
pub const WORKER2: &str = "worker2";
/// The input whose run the plan scripts on the second worker.
pub const SECOND: &str = "second";
/// A full scripted run a restart starts on a worker's session after reading
/// it: its tool rounds take round commits.
pub const FOLLOW: &str = "follow";
/// The host's budget for its runtime shutdown on SIGTERM.
pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(75);
/// The System message the other writer of a compaction race appends.
pub const OTHER_WRITER: &str = "[lr other writer]";
/// The worker's auto-compaction threshold, and the input tokens the model
/// reports on the round before a compaction.
const COMPACTION_THRESHOLD: u64 = 50_000;
/// Bound for one step of a row (a child reaching its kill point, a restart
/// going idle); it only turns a hang into a failure.
pub const STEP: Duration = Duration::from_secs(120);
/// How long the harness gate waits for the harness before it answers:
/// under the runtime's attempt timeout, so a parked ask is never cancelled.
const PARK: Duration = Duration::from_secs(6);
/// The host store's fence timing: the shortest renew interval and lease
/// the contract admits, so a takeover waits out three seconds.
const HOST_RENEW: Duration = Duration::from_secs(1);
const HOST_LEASE: Duration = Duration::from_secs(3);
/// Environment variable naming a child incarnation's row root.
pub const ROOT_ENV: &str = "MEERKAT_LONG_RUN_ROOT";
/// The row root's record of the main input's round whose model call fails
/// ([`fail_model_call`]).
const MODEL_FAILS: &str = "model-fails";
/// The row root's marker for a whole-blob realm: every incarnation opens the
/// realm with the JSONL session store, whose runtime store keeps the
/// whole-blob session profile, instead of the SQLite one.
pub const WHOLE_BLOB: &str = "whole-blob";
/// A one-pixel PNG, the image an image round's message carries. The blob
/// store addresses it by content, so every incarnation stores the same blob.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

// ---------------------------------------------------------------------------
// The plan on disk (the child reads it)
// ---------------------------------------------------------------------------

fn rounds_of(set: RoundSet) -> Vec<u64> {
    (0..64).filter(|round| set.contains(*round)).collect()
}

fn set_of(value: &serde_json::Value) -> RoundSet {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_u64)
        .fold(RoundSet::EMPTY, RoundSet::with)
}

fn encode_plan(plan: &LongRunPlan) -> serde_json::Value {
    let (kill_point, unrepeatable) = match plan.kill_point {
        KillPoint::ModelCall => ("model_call", None),
        KillPoint::BeforeDispatch => ("before_dispatch", None),
        KillPoint::DuringTools => ("during_tools", None),
        KillPoint::DuringUnrepeatableTool(tool) => (
            "during_unrepeatable_tool",
            Some(match tool {
                UnrepeatableTool::BuiltInProcess => "built_in_process",
                UnrepeatableTool::DetachedNonResumable => "detached_non_resumable",
                UnrepeatableTool::CommsSend => "comms_send",
                UnrepeatableTool::CommsRequest => "comms_request",
            }),
        ),
        KillPoint::DuringCallbackTools => ("during_callback_tools", None),
        KillPoint::RebaseRace => ("rebase_race", None),
    };
    let before_resume = match plan.before_resume {
        BeforeResume::Nothing => "nothing",
        BeforeResume::QueueInput => "queue_input",
        BeforeResume::Stop => "stop",
        BeforeResume::Retire => "retire",
    };
    serde_json::json!({
        "rounds": plan.rounds,
        "calls_per_round": plan.calls_per_round,
        "kill_round": plan.kill_round,
        "kill_point": kill_point,
        "unrepeatable": unrepeatable,
        "compactions": rounds_of(plan.compactions),
        "image_rounds": rounds_of(plan.image_rounds),
        "before_resume": before_resume,
    })
}

fn decode_plan(value: &serde_json::Value) -> LongRunPlan {
    let unrepeatable = || match value["unrepeatable"].as_str() {
        Some("built_in_process") => UnrepeatableTool::BuiltInProcess,
        Some("detached_non_resumable") => UnrepeatableTool::DetachedNonResumable,
        Some("comms_send") => UnrepeatableTool::CommsSend,
        Some("comms_request") => UnrepeatableTool::CommsRequest,
        other => panic!("unknown unrepeatable tool {other:?}"),
    };
    LongRunPlan {
        rounds: value["rounds"].as_u64().expect("rounds"),
        calls_per_round: u32::try_from(value["calls_per_round"].as_u64().expect("calls"))
            .expect("calls fit"),
        kill_round: value["kill_round"].as_u64().expect("kill round"),
        kill_point: match value["kill_point"].as_str().expect("kill point") {
            "model_call" => KillPoint::ModelCall,
            "before_dispatch" => KillPoint::BeforeDispatch,
            "during_tools" => KillPoint::DuringTools,
            "during_unrepeatable_tool" => KillPoint::DuringUnrepeatableTool(unrepeatable()),
            "during_callback_tools" => KillPoint::DuringCallbackTools,
            "rebase_race" => KillPoint::RebaseRace,
            other => panic!("unknown kill point {other}"),
        },
        compactions: set_of(&value["compactions"]),
        image_rounds: set_of(&value["image_rounds"]),
        before_resume: match value["before_resume"].as_str().expect("before resume") {
            "nothing" => BeforeResume::Nothing,
            "queue_input" => BeforeResume::QueueInput,
            "stop" => BeforeResume::Stop,
            "retire" => BeforeResume::Retire,
            other => panic!("unknown before-resume action {other}"),
        },
    }
}

fn uses_peer(plan: &LongRunPlan) -> bool {
    matches!(
        plan.kill_point,
        KillPoint::DuringUnrepeatableTool(
            UnrepeatableTool::CommsSend | UnrepeatableTool::CommsRequest
        )
    )
}

/// Whether `tag` names an input whose run the plan scripts.
fn scripted_run(tag: &str) -> bool {
    tag == MAIN || tag == SECOND || tag == FOLLOW
}

/// The tag of a durable worker's scripted input.
pub fn worker_tag(member: &str) -> &'static str {
    if member == WORKER { MAIN } else { SECOND }
}

/// The name of a record the first incarnation keeps for `member`.
fn member_file(name: &str, member: &str) -> String {
    if member == WORKER {
        name.to_string()
    } else {
        format!("{name}-{member}")
    }
}

/// Whether this process is a child incarnation.
fn in_child() -> bool {
    std::env::var_os(ROOT_ENV).is_some()
}

// ---------------------------------------------------------------------------
// The deploy rows
// ---------------------------------------------------------------------------

/// How a deploy row ends its first incarnation: the host's graceful
/// shutdown, the mob's terminal Shutdown inside the host's budget, once every
/// durable worker's run is in flight at the plan's stop point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeployConfig {
    /// The durable workers whose runs are in flight at the stop, 1 or 2.
    pub workers: u32,
    /// `Some`: the Shutdown asks for this requeue bound; `None`: it makes no
    /// choice and takes the default.
    pub max_age: Option<Duration>,
    /// Kill the process while the Shutdown drops the last worker's
    /// in-flight call, before the Shutdown returns.
    pub kill_mid_suspend: bool,
}

impl DeployConfig {
    pub fn write(&self, root: &Path) {
        let config = serde_json::json!({
            "workers": self.workers,
            "max_age_ms": self.max_age.map(|max_age| u64::try_from(max_age.as_millis()).unwrap_or(u64::MAX)),
            "kill_mid_suspend": self.kill_mid_suspend,
        });
        std::fs::write(root.join("deploy.json"), config.to_string()).expect("write the deploy");
    }

    pub fn read(root: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(root.join("deploy.json")).ok()?;
        let config: serde_json::Value = serde_json::from_str(&text).expect("decode the deploy");
        Some(Self {
            workers: u32::try_from(config["workers"].as_u64().expect("workers"))
                .expect("workers fit"),
            max_age: config["max_age_ms"].as_u64().map(Duration::from_millis),
            kill_mid_suspend: config["kill_mid_suspend"].as_bool().unwrap_or(false),
        })
    }

    /// The durable workers of the row.
    pub fn members(&self) -> &'static [&'static str] {
        if self.workers >= 2 {
            &[WORKER, WORKER2]
        } else {
            &[WORKER]
        }
    }
}

// ---------------------------------------------------------------------------
// Markers: how the scripted model knows which input and round it is asked for
// ---------------------------------------------------------------------------

/// The prompt of input `tag`.
pub fn prompt_text(tag: &str) -> String {
    format!("[lr input={tag}] work through the plan")
}

/// The marker the model's message for round `round` of input `tag` carries.
fn round_marker(tag: &str, round: u64) -> String {
    format!("[lr input={tag} round={round}]")
}

/// Every marker in `text`, in order: its input tag and its round, if any.
fn markers(text: &str) -> Vec<(String, Option<u64>)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[lr input=") {
        let after = &rest[start + "[lr input=".len()..];
        let Some(end) = after.find(']') else {
            break;
        };
        let mut parts = after[..end].split(' ');
        let tag = parts.next().unwrap_or_default().to_string();
        let round = parts
            .find_map(|part| part.strip_prefix("round="))
            .and_then(|round| round.parse().ok());
        found.push((tag, round));
        rest = &after[end..];
    }
    found
}

/// The text a message carries markers in.
fn message_text(message: &Message) -> String {
    match message {
        Message::User(user) => user.text_content(),
        Message::BlockAssistant(assistant) => {
            assistant.text_blocks().collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

/// What one model request asks for.
struct Asked {
    /// The request summarizes the transcript for a compaction.
    compaction: bool,
    /// The input the request belongs to; `None` for a request no marker
    /// names (a member's kickoff turn).
    tag: Option<String>,
    /// The round the request asks for: one after the input's latest marked
    /// round. For a compaction, the round whose model call follows it.
    round: u64,
    /// The input's latest marked round, if any.
    latest: Option<u64>,
}

fn analyse(request: &LlmRequest) -> Asked {
    let compaction = request.messages.last().is_some_and(|message| {
        matches!(message, Message::User(user) if user.text_content().contains("CONTEXT COMPACTION"))
    });
    let found: Vec<(String, Option<u64>)> = request
        .messages
        .iter()
        .flat_map(|message| markers(&message_text(message)))
        .collect();
    let tag = found.last().map(|(tag, _)| tag.clone());
    let latest = tag.as_ref().and_then(|tag| {
        found
            .iter()
            .filter(|(found_tag, _)| found_tag == tag)
            .filter_map(|(_, round)| *round)
            .max()
    });
    Asked {
        compaction,
        tag,
        round: latest.map_or(0, |latest| latest + 1),
        latest,
    }
}

// ---------------------------------------------------------------------------
// The kill switch and the compaction race
// ---------------------------------------------------------------------------

/// Where the incarnation the harness kills stops: it records the point and
/// then never returns, and the harness kills it.
pub struct KillSwitch {
    root: PathBuf,
    fired: tokio::sync::watch::Sender<bool>,
    /// The members' comms runtimes of an in-process incarnation, whose
    /// names outlive its runtime's shutdown in this process.
    comms: Mutex<Vec<std::sync::Weak<dyn meerkat_core::agent::CommsRuntime>>>,
    /// A deploy row's stop, read from the row root.
    deploy: Option<DeployConfig>,
    /// The workers of a deploy row that reached the stop point.
    reached: AtomicU32,
    /// Set once every worker of a deploy row is in flight at the stop point.
    in_flight: tokio::sync::watch::Sender<bool>,
    /// The in-process stand-in for SIGTERM.
    terminate: tokio::sync::watch::Sender<bool>,
    /// The in-flight calls the stop of a deploy row dropped.
    dropped: AtomicU32,
}

impl KillSwitch {
    pub fn new(root: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: root.to_path_buf(),
            fired: tokio::sync::watch::channel(false).0,
            comms: Mutex::new(Vec::new()),
            deploy: DeployConfig::read(root),
            reached: AtomicU32::new(0),
            in_flight: tokio::sync::watch::channel(false).0,
            terminate: tokio::sync::watch::channel(false).0,
            dropped: AtomicU32::new(0),
        })
    }

    pub fn deploy(&self) -> Option<DeployConfig> {
        self.deploy
    }

    pub fn subscribe_in_flight(&self) -> tokio::sync::watch::Receiver<bool> {
        self.in_flight.subscribe()
    }

    /// The in-process stand-in for SIGTERM.
    pub fn terminate(&self) {
        self.terminate.send_replace(true);
    }

    pub fn subscribe_terminate(&self) -> tokio::sync::watch::Receiver<bool> {
        self.terminate.subscribe()
    }

    /// A deploy row's Shutdown returned and the incarnation dropped every
    /// Core object: record the stop's account, then end the incarnation.
    pub fn stopped(&self, account: &serde_json::Value) {
        std::fs::write(self.root.join("stopped"), account.to_string()).expect("record the stop");
        std::fs::write(self.root.join("killed"), "deploy_stop").expect("record the end");
        self.fired.send_replace(true);
    }

    /// A guard that tells this switch when its in-flight call is dropped.
    fn drop_guard(self: &Arc<Self>) -> CallDropGuard {
        CallDropGuard {
            kill: Arc::clone(self),
        }
    }

    /// The stop of a deploy row dropped one worker's in-flight call. A row
    /// that kills mid-suspend does it when the last one is dropped: a child
    /// marks the kill and holds this thread until the harness kills it; in
    /// process, the harness shuts the runtime down while this thread is
    /// briefly held.
    fn call_dropped(&self) {
        let Some(deploy) = self.deploy.filter(|deploy| deploy.kill_mid_suspend) else {
            return;
        };
        if self.dropped.fetch_add(1, Ordering::SeqCst) + 1 < deploy.workers {
            return;
        }
        if in_child() {
            let _ = std::fs::write(self.root.join("killed"), "mid_suspend");
            let held = Instant::now();
            while held.elapsed() < STEP {
                std::thread::sleep(Duration::from_millis(50));
            }
        } else {
            self.fired.send_replace(true);
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.fired.subscribe()
    }

    /// Hold a member's comms runtime, weakly, for [`Self::release_comms`].
    pub fn hold_comms(&self, runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>) {
        self.comms.lock().unwrap().push(Arc::downgrade(runtime));
    }

    /// Free the comms names of an in-process incarnation the harness shut
    /// down, as a process exit frees them. This writes nothing durable.
    pub fn release_comms(&self) {
        let held: Vec<_> = self.comms.lock().unwrap().drain(..).collect();
        for runtime in held {
            if let Some(runtime) = runtime.upgrade() {
                let _ = runtime.retire_inproc_route();
            }
        }
    }

    /// Record the kill point and signal the harness. What the harness reads
    /// back (each worker's session, input and run) must be on disk first. A
    /// deploy row instead waits until every worker reached its stop point,
    /// then marks the run in flight for the harness to send SIGTERM.
    pub async fn fire(&self, at: &str) {
        let members = self.deploy.map_or(&[WORKER][..], |deploy| deploy.members());
        if let Some(deploy) = self.deploy
            && self.reached.fetch_add(1, Ordering::SeqCst) + 1 < deploy.workers
        {
            return;
        }
        let started = Instant::now();
        while members
            .iter()
            .flat_map(|member| {
                ["session-id", "input-id", "run-id"].map(|name| member_file(name, member))
            })
            .any(|file| !self.root.join(file).exists())
            && started.elapsed() < Duration::from_secs(10)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if self.deploy.is_some() {
            std::fs::write(self.root.join("in-flight"), at).expect("record the stop point");
            self.in_flight.send_replace(true);
            return;
        }
        std::fs::write(self.root.join("killed"), at).expect("record the kill point");
        self.fired.send_replace(true);
    }

    /// Fire and never return: the incarnation is killed while it waits, or
    /// a deploy row's stop drops the wait.
    async fn hold<T>(&self, at: &str) -> T {
        self.fire(at).await;
        std::future::pending::<T>().await
    }
}

/// Dropped with a deploy row's in-flight call (see
/// [`KillSwitch::call_dropped`]).
struct CallDropGuard {
    kill: Arc<KillSwitch>,
}

impl Drop for CallDropGuard {
    fn drop(&mut self) {
        self.kill.call_dropped();
    }
}

/// The other writer of a compaction race: run before the run's compaction
/// of the race round commits.
#[async_trait::async_trait]
pub trait RaceHook: Send + Sync {
    async fn race(&self);
}

/// A child incarnation's race: ask the parent to play the other writer, and
/// wait until it did.
pub struct ChildRace {
    pub root: PathBuf,
}

#[async_trait::async_trait]
impl RaceHook for ChildRace {
    async fn race(&self) {
        std::fs::write(self.root.join("race"), "race").expect("ask for the race");
        let done = self.root.join("race-done");
        let started = Instant::now();
        while !done.exists() && started.elapsed() < STEP {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// An in-process race over the host store.
pub struct HostRace {
    pub store: Store,
}

#[async_trait::async_trait]
impl RaceHook for HostRace {
    async fn race(&self) {
        // Marked first, as a child marks it: the raced run's end then ends
        // the first incarnation.
        std::fs::write(self.store.root().join("race"), "race").expect("mark the race");
        let session = SessionId::parse(
            std::fs::read_to_string(self.store.root().join("session-id"))
                .expect("the worker's session")
                .trim(),
        )
        .expect("session id");
        other_writer(&self.store, &session).await;
    }
}

/// A second Core over `store` takes it over and appends one System message
/// of its own to `session`.
pub async fn other_writer(store: &Store, session: &SessionId) {
    let opened = open_persistence(store, true).await;
    let (service, _machine) = runtime_backed_service(store.root(), opened.persistence, None, false);
    service
        .append_system_context(
            session,
            meerkat_core::service::AppendSystemContextRequest::from_text(OTHER_WRITER),
        )
        .await
        .expect("the other writer appends its message");
    drop(service);
    drop(opened.owners);
}

// ---------------------------------------------------------------------------
// The scripted calls
// ---------------------------------------------------------------------------

/// The tool call `index` of `round` asks for: the same name and arguments
/// every time, so its dedupe key (digest included) is the same.
fn tool_call(
    plan: &LongRunPlan,
    root: &Path,
    peer_id: Option<&str>,
    round: u64,
    index: u32,
) -> (String, serde_json::Value) {
    let ordinary = || {
        (
            "lr_work".to_string(),
            serde_json::json!({ "round": round, "index": index }),
        )
    };
    if round != plan.kill_round {
        return ordinary();
    }
    match plan.kill_point {
        KillPoint::DuringCallbackTools => (
            "lr_callback".to_string(),
            serde_json::json!({ "round": round, "index": index }),
        ),
        KillPoint::DuringUnrepeatableTool(tool) if index == 0 => match tool {
            UnrepeatableTool::BuiltInProcess => (
                "shell".to_string(),
                serde_json::json!({
                    "command": format!(
                        "echo $$ > '{}'; exec sleep 600",
                        root.join("unrepeatable-started").display()
                    ),
                    "timeout_secs": 900,
                }),
            ),
            UnrepeatableTool::DetachedNonResumable => (
                "lr_detached".to_string(),
                serde_json::json!({ "round": round, "index": index }),
            ),
            UnrepeatableTool::CommsSend => (
                "send_message".to_string(),
                serde_json::json!({
                    "peer_id": peer_id.unwrap_or_default(),
                    "body": format!("round {round} note"),
                    "handling_mode": "queue",
                }),
            ),
            UnrepeatableTool::CommsRequest => (
                "send_request".to_string(),
                serde_json::json!({
                    "peer_id": peer_id.unwrap_or_default(),
                    "intent": "checksum_token",
                    "params": { "subject": format!("round {round}") },
                    "handling_mode": "queue",
                }),
            ),
        },
        _ => ordinary(),
    }
}

/// The dedupe key of call `index` of `round` of `run_id`, as the run's tool
/// sees it.
fn expected_key(
    plan: &LongRunPlan,
    root: &Path,
    peer_id: Option<&str>,
    run_id: &RunId,
    round: u64,
    index: u32,
) -> ToolCallDedupeKey {
    let (name, args) = tool_call(plan, root, peer_id, round, index);
    let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw arguments");
    ToolCallDedupeKey::new(run_id.clone(), round, call_id(round, index), &name, &raw)
        .expect("arguments are JSON")
}

// ---------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------

/// Plays the plan: round `r` of the main input makes the plan's calls (none
/// in the last round) and carries the round's marker, with an image in an
/// image round; the round before a compaction reports input tokens over the
/// compaction threshold; a compaction request is answered with a summary
/// that carries the latest marker; any other input is answered at once. The
/// round [`fail_model_call`] names, if any, is answered once with a provider
/// error instead.
pub struct ScriptedModel {
    plan: LongRunPlan,
    root: PathBuf,
    peer_id: Mutex<Option<String>>,
    kill: Option<Arc<KillSwitch>>,
    race: Option<Arc<dyn RaceHook>>,
    raced: AtomicBool,
    image: Mutex<Option<meerkat_core::BlobRef>>,
    calls: Mutex<Vec<(String, u64)>>,
    /// The main input's round whose model call fails, read when the model
    /// is opened.
    fail_round: Option<u64>,
    /// Whether that model call already failed.
    failed: AtomicBool,
}

impl ScriptedModel {
    pub fn new(
        plan: LongRunPlan,
        root: &Path,
        kill: Option<Arc<KillSwitch>>,
        race: Option<Arc<dyn RaceHook>>,
    ) -> Arc<Self> {
        let peer_id = std::fs::read_to_string(root.join("peer-id"))
            .ok()
            .map(|id| id.trim().to_string());
        let fail_round = std::fs::read_to_string(root.join(MODEL_FAILS))
            .ok()
            .and_then(|round| round.trim().parse().ok());
        Arc::new(Self {
            plan,
            root: root.to_path_buf(),
            peer_id: Mutex::new(peer_id),
            kill,
            race,
            raced: AtomicBool::new(false),
            image: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            fail_round,
            failed: AtomicBool::new(false),
        })
    }

    /// The stored blob an image round's message references.
    pub fn set_image(&self, image: meerkat_core::BlobRef) {
        *self.image.lock().unwrap() = Some(image);
    }

    pub fn set_peer_id(&self, peer_id: &str) {
        *self.peer_id.lock().unwrap() = Some(peer_id.to_string());
    }

    /// The rounds of the main input this incarnation called the model for.
    pub fn main_rounds(&self) -> Vec<u64> {
        self.rounds_of(MAIN)
    }

    /// The rounds of input `tag` this incarnation called the model for.
    pub fn rounds_of(&self, tag: &str) -> Vec<u64> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(called, _)| called == tag)
            .map(|(_, round)| *round)
            .collect()
    }

    fn usage(request: &LlmRequest, input_tokens: u64) -> LlmEvent {
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage {
                    input_tokens,
                    output_tokens: 1,
                    ..Default::default()
                },
            ),
        }
    }

    fn done(stop_reason: meerkat_core::StopReason) -> LlmEvent {
        LlmEvent::Done {
            outcome: LlmDoneOutcome::success(stop_reason),
        }
    }

    fn answer(request: &LlmRequest, text: String) -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                delta: text,
                meta: None,
            },
            Self::usage(request, 1),
            Self::done(meerkat_core::StopReason::EndTurn),
        ]
    }

    /// The image block an image round's message carries: the stored pixel,
    /// under an id fixed by the round.
    fn image(&self, round: u64) -> AssistantBlock {
        let blob_ref = self
            .image
            .lock()
            .unwrap()
            .clone()
            .expect("the incarnation stored the pixel image");
        AssistantBlock::Image {
            image_id: meerkat_core::AssistantImageId::new(uuid::Uuid::from_u128(
                u128::from(round) + 1,
            )),
            blob_ref,
            media_type: meerkat_core::MediaType::new("image/png"),
            width: 1,
            height: 1,
            revised_prompt: meerkat_core::RevisedPromptDisposition::NotRequested,
            meta: meerkat_core::ProviderImageMetadata::NotEmitted,
        }
    }

    /// The events of round `round` of the main input, or of the queued
    /// input's only round.
    fn round_events(&self, request: &LlmRequest, tag: &str, round: u64) -> Vec<LlmEvent> {
        let last = tag == QUEUED || round + 1 >= self.plan.rounds;
        let input_tokens = if scripted_run(tag) && self.plan.compactions.contains(round + 1) {
            COMPACTION_THRESHOLD + 1
        } else {
            1
        };
        let marker = round_marker(tag, round);
        if last {
            let mut events = Self::answer(request, format!("{marker} done"));
            events[1] = Self::usage(request, input_tokens);
            return events;
        }
        let peer_id = self.peer_id.lock().unwrap().clone();
        let calls: Vec<(String, String, serde_json::Value)> = (0..self.plan.calls_per_round)
            .map(|index| {
                let (name, args) =
                    tool_call(&self.plan, &self.root, peer_id.as_deref(), round, index);
                (call_id(round, index), name, args)
            })
            .collect();
        let mut events = Vec::new();
        if scripted_run(tag) && self.plan.image_rounds.contains(round) {
            // The provider's terminal output, with the image beside the
            // round's text and calls.
            let mut blocks = vec![
                AssistantBlock::Text {
                    text: marker,
                    meta: None,
                },
                self.image(round),
            ];
            blocks.extend(calls.into_iter().map(|(id, name, args)| {
                AssistantBlock::ToolUse {
                    id,
                    name,
                    args: serde_json::value::RawValue::from_string(args.to_string())
                        .expect("raw arguments"),
                    meta: None,
                }
            }));
            events.push(LlmEvent::AssistantOutput { blocks });
        } else {
            events.push(LlmEvent::TextDelta {
                delta: marker,
                meta: None,
            });
            events.extend(
                calls
                    .into_iter()
                    .map(|(id, name, args)| LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        meta: None,
                    }),
            );
        }
        events.push(Self::usage(request, input_tokens));
        events.push(Self::done(meerkat_core::StopReason::ToolUse));
        events
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedModel {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let asked = analyse(request);
        if asked.compaction {
            let summary = match (&asked.tag, asked.latest) {
                (Some(tag), Some(latest)) => format!("{} summary", round_marker(tag, latest)),
                (Some(tag), None) => format!("[lr input={tag}] summary"),
                _ => "summary".to_string(),
            };
            let events = Self::answer(request, summary);
            let race = (self.plan.kill_point == KillPoint::RebaseRace
                && asked.tag.as_deref() == Some(MAIN)
                && asked.round == self.plan.kill_round
                && !self.raced.swap(true, Ordering::SeqCst))
            .then(|| self.race.clone())
            .flatten();
            let Some(race) = race else {
                return Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
            };
            // The other writer moves the session before this compaction can
            // commit.
            return Box::pin(
                futures::stream::once(async move {
                    race.race().await;
                    futures::stream::iter(events.into_iter().map(Ok::<LlmEvent, LlmError>))
                })
                .flatten(),
            );
        }
        let Some(tag) = asked.tag.filter(|tag| scripted_run(tag) || tag == QUEUED) else {
            // A turn the plan does not script (a member's kickoff).
            let events = Self::answer(request, "ready".to_string());
            return Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
        };
        let round = asked.round;
        self.calls.lock().unwrap().push((tag.clone(), round));
        if tag == MAIN
            && self.fail_round == Some(round)
            && !self.failed.swap(true, Ordering::SeqCst)
        {
            // Not retryable, so the run fails at this model call.
            return Box::pin(futures::stream::once(async {
                Err(LlmError::InvalidRequest {
                    message: "the scripted provider refuses this request".to_string(),
                })
            }));
        }
        if scripted_run(&tag)
            && round == self.plan.kill_round
            && self.plan.kill_point == KillPoint::ModelCall
            && let Some(kill) = self.kill.clone()
        {
            return Box::pin(futures::stream::once(async move {
                kill.hold::<Result<LlmEvent, LlmError>>("model_call").await
            }));
        }
        let events = self.round_events(request, &tag, round);
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The scripted tools
// ---------------------------------------------------------------------------

/// The keys one incarnation's tools saw, in dispatch order. A child
/// incarnation also appends each to a file, which outlives the process.
pub struct DispatchLog {
    keys: Mutex<Vec<ToolCallDedupeKey>>,
    file: Option<PathBuf>,
}

impl DispatchLog {
    pub fn new(file: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            keys: Mutex::new(Vec::new()),
            file,
        })
    }

    fn record(&self, key: &ToolCallDedupeKey) {
        let mut keys = self.keys.lock().unwrap();
        keys.push(key.clone());
        if let Some(file) = &self.file {
            use std::io::Write as _;
            // The calls of a round record concurrently: each line goes out
            // whole in one write, so two keys never share a line.
            let mut line = serde_json::to_string(key).expect("encode key");
            line.push('\n');
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
                .expect("open the dispatch log")
                .write_all(line.as_bytes())
                .expect("append the dispatch log");
        }
    }

    pub fn keys(&self) -> Vec<ToolCallDedupeKey> {
        self.keys.lock().unwrap().clone()
    }

    /// The keys a killed incarnation recorded in `file`.
    pub fn read(file: &Path) -> Vec<ToolCallDedupeKey> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("decode key"))
            .collect()
    }
}

/// The plan's own tools, mounted on the worker as the `lr` Rust bundle:
/// `lr_work` (an ordinary call), `lr_callback` (a callback tool) and
/// `lr_detached` (a Detached call whose runner is not resumable).
pub struct ScriptedTools {
    plan: LongRunPlan,
    root: PathBuf,
    peer_id: Mutex<Option<String>>,
    kill: Option<Arc<KillSwitch>>,
    log: Arc<DispatchLog>,
    defs: Arc<[Arc<ToolDef>]>,
    catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
}

impl ScriptedTools {
    pub fn new(
        plan: LongRunPlan,
        root: &Path,
        kill: Option<Arc<KillSwitch>>,
        log: Arc<DispatchLog>,
    ) -> Arc<Self> {
        let def = |name: &str, kind: Option<meerkat_core::types::ToolSourceKind>| {
            Arc::new(ToolDef {
                audience: Default::default(),
                name: name.into(),
                description: format!("long run harness tool {name}"),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: kind.map(|kind| meerkat_core::types::ToolProvenance {
                    kind,
                    source_id: "lr".into(),
                    artifact: None,
                }),
            })
        };
        let work = def("lr_work", None);
        let callback = def(
            "lr_callback",
            Some(meerkat_core::types::ToolSourceKind::Callback),
        );
        let detached = def("lr_detached", None);
        let policy = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("lr.detached", "v1").expect("runner identity"),
            meerkat_core::RestartClass::NonResumable,
            meerkat_core::IdempotencyScope::ToolCall,
            Duration::from_secs(60),
        )
        .expect("detached policy");
        let contract = meerkat_core::ToolExecutionContract::new(
            BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .expect("detached contract");
        let catalog: Arc<[meerkat_core::ToolCatalogEntry]> = Arc::from([
            meerkat_core::ToolCatalogEntry::session_inline(Arc::clone(&work), true),
            meerkat_core::ToolCatalogEntry::session_inline(Arc::clone(&callback), true),
            meerkat_core::ToolCatalogEntry::session_inline(Arc::clone(&detached), true)
                .with_execution_contract(contract),
        ]);
        let peer_id = std::fs::read_to_string(root.join("peer-id"))
            .ok()
            .map(|id| id.trim().to_string());
        Arc::new(Self {
            plan,
            root: root.to_path_buf(),
            peer_id: Mutex::new(peer_id),
            kill,
            log,
            defs: Arc::from([work, callback, detached]),
            catalog,
        })
    }

    pub fn set_peer_id(&self, peer_id: &str) {
        *self.peer_id.lock().unwrap() = Some(peer_id.to_string());
    }

    pub fn log(&self) -> &Arc<DispatchLog> {
        &self.log
    }

    fn index_of(call: &ToolCallView<'_>) -> u32 {
        call.id
            .rsplit('-')
            .next()
            .and_then(|index| index.parse().ok())
            .unwrap_or(0)
    }

    /// The killed incarnation's behaviour for a call of the kill round.
    async fn kill_round_call(&self, key: &ToolCallDedupeKey, index: u32) {
        let Some(kill) = self.kill.clone() else {
            return;
        };
        if key.round != self.plan.kill_round {
            return;
        }
        match self.plan.kill_point {
            // After the round's first commit, before any of its calls did
            // anything: the call is not recorded as dispatched.
            KillPoint::BeforeDispatch => kill.hold::<()>("before_dispatch").await,
            KillPoint::DuringTools | KillPoint::DuringCallbackTools => {
                self.log.record(key);
                if index == 0 {
                    let _dropped = kill.drop_guard();
                    kill.hold::<()>("during_tools").await;
                }
                std::future::pending::<()>().await;
            }
            KillPoint::DuringUnrepeatableTool(tool) => {
                self.log.record(key);
                // Call 0 is the unrepeatable call; the kill waits until it
                // has started, and records its key as dispatched.
                let started = self.root.join("unrepeatable-started");
                let waited = Instant::now();
                while !started.exists() && waited.elapsed() < STEP {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if tool != UnrepeatableTool::DetachedNonResumable {
                    let peer_id = self.peer_id.lock().unwrap().clone();
                    self.log.record(&expected_key(
                        &self.plan,
                        &self.root,
                        peer_id.as_deref(),
                        &key.run_id,
                        key.round,
                        0,
                    ));
                }
                kill.hold::<()>("during_unrepeatable_tool").await;
            }
            KillPoint::ModelCall | KillPoint::RebaseRace => {}
        }
    }

    fn result(call: &ToolCallView<'_>) -> meerkat_core::ToolDispatchOutcome {
        ToolResult::new(
            call.id.to_string(),
            format!("[lr result {}]", call.id),
            false,
        )
        .into()
    }

    fn key_of(
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolCallDedupeKey, meerkat_core::ToolError> {
        context.tool_call_dedupe_key(call).ok_or_else(|| {
            meerkat_core::ToolError::execution_failed(format!(
                "{} was dispatched outside a run's round",
                call.id
            ))
        })
    }
}

#[async_trait::async_trait]
impl meerkat_core::agent::AgentToolDispatcher for ScriptedTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.defs)
    }

    /// The catalog is exact: it carries the detached tool's execution
    /// contract, which a composing gateway reads only from an exact catalog
    /// (it lists the tools of any other dispatcher with the default contract).
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        Arc::clone(&self.catalog)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::execution_failed(format!(
            "{} needs its dispatch context",
            call.name
        )))
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let key = Self::key_of(call, context)?;
        self.kill_round_call(&key, Self::index_of(&call)).await;
        self.log.record(&key);
        Ok(Self::result(&call))
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if plan.mode() != meerkat_core::ToolExecutionMode::Detached {
            return self.dispatch_with_context(call, context).await;
        }
        let key = Self::key_of(call, context)?;
        self.log.record(&key);
        if self.kill.is_some() && key.round == self.plan.kill_round {
            std::fs::write(self.root.join("unrepeatable-started"), "started")
                .expect("record the detached start");
        }
        Ok(Self::result(&call))
    }
}

// ---------------------------------------------------------------------------
// The harness gate
// ---------------------------------------------------------------------------

/// The restarted mob's resume gate. It records every ask, waits until the
/// harness releases it (bounded by [`PARK`]), and answers the row's verdict.
pub struct HarnessGate {
    verdict: ResumeGate,
    asked: tokio::sync::watch::Sender<u32>,
    released: tokio::sync::watch::Sender<bool>,
    answered_unavailable: AtomicBool,
    requests: Mutex<Vec<DurableResumeRequest>>,
}

impl HarnessGate {
    pub fn new(verdict: ResumeGate, released: bool) -> Arc<Self> {
        Arc::new(Self {
            verdict,
            asked: tokio::sync::watch::channel(0).0,
            released: tokio::sync::watch::channel(released).0,
            answered_unavailable: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Wait until the runtime asks, bounded by `within`; whether it asked.
    pub async fn wait_asked(&self, within: Duration) -> bool {
        let mut asked = self.asked.subscribe();
        tokio::time::timeout(within, asked.wait_for(|count| *count > 0))
            .await
            .is_ok_and(|seen| seen.is_ok())
    }

    pub fn release(&self) {
        self.released.send_replace(true);
    }

    pub fn requests(&self) -> Vec<DurableResumeRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl DurableResumeGate for HarnessGate {
    async fn check(&self, request: &DurableResumeRequest) -> DurableResumeVerdict {
        self.requests.lock().unwrap().push(request.clone());
        self.asked.send_modify(|count| *count += 1);
        let mut released = self.released.subscribe();
        let _ = tokio::time::timeout(PARK, released.wait_for(|released| *released)).await;
        match self.verdict {
            ResumeGate::Absent | ResumeGate::Continue => DurableResumeVerdict::Continue,
            ResumeGate::Superseded => DurableResumeVerdict::Superseded {
                reason: "the work item moved to another worker".to_string(),
            },
            ResumeGate::Unavailable => DurableResumeVerdict::Unavailable {
                reason: "the work record is unreachable".to_string(),
            },
            ResumeGate::UnavailableOnce => {
                if self.answered_unavailable.swap(true, Ordering::SeqCst) {
                    DurableResumeVerdict::Continue
                } else {
                    DurableResumeVerdict::Unavailable {
                        reason: "the work record is briefly unreachable".to_string(),
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The store under Core
// ---------------------------------------------------------------------------

/// The store a row runs over.
#[derive(Clone)]
pub enum Store {
    /// The built-in SQLite realm under `root`, or the JSONL one when the
    /// root holds the [`WHOLE_BLOB`] marker.
    Sqlite { root: PathBuf },
    /// The reference host store; `root` holds only the factory's files.
    /// The stores the host store does not hold (schedule, work graph, jobs,
    /// blobs, artifacts) are in-memory, one set per row, and outlive every
    /// incarnation of the row, as the host store does.
    Host {
        root: PathBuf,
        host: Arc<meerkat_store_conformance::InMemoryHostStore>,
        supplied: Arc<SuppliedStores>,
    },
}

/// One row's in-memory stores beside the host store.
pub struct SuppliedStores {
    schedule: Arc<meerkat::MemoryScheduleStore>,
    workgraph: Arc<meerkat::MemoryWorkGraphStore>,
    jobs: Arc<meerkat::MemoryDetachedJobStore>,
    blobs: Arc<meerkat::MemoryBlobStore>,
    artifacts: Arc<meerkat_store::MemoryArtifactStore>,
}

impl Store {
    pub fn root(&self) -> &Path {
        match self {
            Self::Sqlite { root } | Self::Host { root, .. } => root,
        }
    }

    /// A fresh reference host store with the shortest fence timing, on its
    /// manual clock: a killed incarnation's lease stays live until a
    /// takeover waits it out.
    pub fn fresh_host(root: PathBuf) -> Self {
        let timing = meerkat_core::host_store::FenceTiming::try_new(HOST_RENEW, HOST_LEASE)
            .expect("fence timing");
        Self::Host {
            root,
            host: Arc::new(meerkat_store_conformance::InMemoryHostStore::with_fence_timing(timing)),
            supplied: Arc::new(SuppliedStores {
                schedule: Arc::new(meerkat::MemoryScheduleStore::new()),
                workgraph: Arc::new(meerkat::MemoryWorkGraphStore::new()),
                jobs: Arc::new(meerkat::MemoryDetachedJobStore::new()),
                blobs: Arc::new(meerkat::MemoryBlobStore::new()),
                artifacts: Arc::new(meerkat_store::MemoryArtifactStore::new()),
            }),
        }
    }
}

/// A realm's persistence over a row's store, with the owners that must
/// outlive its use and the lease its open waited out.
struct OpenedPersistence {
    persistence: meerkat::PersistenceBundle,
    mob_storage: Option<MobStorage>,
    waited_out_lease: Duration,
    owners: Vec<Box<dyn Any + Send + Sync>>,
}

async fn open_persistence(store: &Store, takeover: bool) -> OpenedPersistence {
    let mob_id = std::fs::read_to_string(store.root().join("mob-id"))
        .expect("mob id")
        .trim()
        .to_string();
    match store {
        Store::Sqlite { root } => {
            let backend = if root.join(WHOLE_BLOB).exists() {
                meerkat_store::RealmBackend::Jsonl
            } else {
                meerkat_store::RealmBackend::Sqlite
            };
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                root,
                REALM,
                Some(backend),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open the realm");
            OpenedPersistence {
                persistence,
                mob_storage: None,
                waited_out_lease: Duration::ZERO,
                owners: Vec::new(),
            }
        }
        Store::Host {
            root,
            host,
            supplied: row,
        } => {
            use meerkat_core::{DurabilityDeclaration, DurabilityResolution};
            let supplied = meerkat::HostSuppliedStores {
                schedule_store: Arc::clone(&row.schedule) as Arc<_>,
                workgraph_store: Arc::clone(&row.workgraph) as Arc<_>,
                job_store: Arc::clone(&row.jobs) as Arc<_>,
                blob_store: Arc::clone(&row.blobs) as Arc<_>,
                artifact_store: Arc::clone(&row.artifacts) as Arc<_>,
                store_path: root.join("host-store"),
                projection_root: None,
                durability: ["schedule", "workgraph", "jobs", "blobs", "artifacts"]
                    .into_iter()
                    .map(|domain| {
                        DurabilityDeclaration::durable(
                            domain,
                            DurabilityResolution::DeclaredEphemeral,
                        )
                    })
                    .collect(),
            };
            let config = if takeover {
                meerkat_runtime::store::spi::SpiRealmConfig {
                    acquire: meerkat_core::host_store::AcquireMode::Takeover,
                    ..Default::default()
                }
            } else {
                meerkat_runtime::store::spi::SpiRealmConfig::default()
            };
            let provider = meerkat::HostStoreProvider::open(
                "long-run-host",
                Arc::clone(host) as Arc<dyn meerkat_core::host_store::HostStore>,
                config,
                supplied,
            )
            .await
            .expect("open the host store realm");
            let waited_out_lease = provider.realm().superseded_lease();
            let mob_host = meerkat_mob::store::SpiMobStoreHost::for_realm(provider.realm())
                .await
                .expect("open the host store's mob stores");
            let mob_storage = mob_host
                .open(&mob_id)
                .await
                .expect("open the mob's storage")
                .storage();
            let (_pin, persistence) = meerkat::open_realm_persistence_with_provider(
                &provider, root, REALM, None, None, None,
            )
            .await
            .expect("open the host store's persistence");
            OpenedPersistence {
                persistence,
                mob_storage: Some(mob_storage),
                waited_out_lease,
                owners: vec![Box::new(provider), Box::new(mob_host)],
            }
        }
    }
}

fn runtime_backed_service(
    root: &Path,
    persistence: meerkat::PersistenceBundle,
    model: Option<Arc<ScriptedModel>>,
    comms: bool,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
) {
    for directory in ["sessions", "realm", "project"] {
        std::fs::create_dir_all(root.join(directory)).expect("fixture directory");
    }
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(root.join("project"))
        .builtins(true)
        .shell(true)
        .comms(comms);
    let mut config = Config::default();
    config.shell.program = "sh".to_string();
    config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
    let mut builder = FactoryAgentBuilder::new(factory, config);
    if let Some(model) = model {
        builder.default_llm_client = Some(model as Arc<dyn LlmClient>);
    }
    let (service, machine) =
        meerkat::surface::build_runtime_backed_service(builder, 8, persistence)
            .expect("compose the runtime-backed service");
    (Arc::new(service), machine)
}

/// One incarnation of Core over a row's store: the runtime-backed session
/// service, the runtime, the mob and the plan's scripted model and tools.
pub struct Incarnation {
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub machine: Arc<MeerkatMachine>,
    pub runtime_store: Arc<dyn RuntimeStore>,
    pub mob: MobHandle,
    pub model: Arc<ScriptedModel>,
    pub tools: Arc<ScriptedTools>,
    /// The fence lease the open waited out (host store only).
    pub waited_out_lease: Duration,
    _owners: Vec<Box<dyn Any + Send + Sync>>,
}

fn profile(durable: bool, bundles: Vec<String>) -> Profile {
    Profile {
        model_fallback: None,
        model: "gpt-5.5".to_string(),
        provider: None,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: NonZeroU64::new(COMPACTION_THRESHOLD),
        resume_overrides: Vec::new(),
        skills: Vec::new(),
        tools: ToolConfig {
            builtins: true,
            shell: durable,
            comms: true,
            rust_bundles: bundles,
            ..Default::default()
        },
        peer_description: "long run harness member".to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
        durable,
    }
}

fn definition(mob_id: &str) -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from(mob_id.to_string()));
    definition.profiles.insert(
        ProfileName::from(WORKER),
        ProfileBinding::Inline(Box::new(profile(true, vec!["lr".to_string()]))),
    );
    definition.profiles.insert(
        ProfileName::from(PEER),
        ProfileBinding::Inline(Box::new(profile(false, Vec::new()))),
    );
    definition
}

/// How an incarnation is opened.
pub struct OpenOptions {
    pub plan: LongRunPlan,
    /// The first incarnation creates the mob; a restart resumes it, taking
    /// a host store over.
    pub restart: bool,
    /// Set in the incarnation the harness kills.
    pub kill: Option<Arc<KillSwitch>>,
    /// The other writer of a compaction race, in the incarnation it races.
    pub race: Option<Arc<dyn RaceHook>>,
    /// The restarted mob's resume gate, if any.
    pub gate: Option<Arc<HarnessGate>>,
    /// Append the tools' keys to `dispatched.jsonl` (a child process).
    pub log_to_file: bool,
}

impl Incarnation {
    pub async fn open(store: &Store, options: OpenOptions) -> Self {
        let root = store.root().to_path_buf();
        let mob_id = std::fs::read_to_string(root.join("mob-id"))
            .expect("mob id")
            .trim()
            .to_string();
        let model = ScriptedModel::new(
            options.plan,
            &root,
            options.kill.clone(),
            options.race.clone(),
        );
        let log = DispatchLog::new(options.log_to_file.then(|| root.join("dispatched.jsonl")));
        let tools = ScriptedTools::new(options.plan, &root, options.kill.clone(), log);

        let opened = open_persistence(store, options.restart).await;
        let runtime_store = opened.persistence.runtime_store();
        let image = opened
            .persistence
            .blob_store()
            .put_image("image/png", PIXEL_PNG)
            .await
            .expect("store the pixel image");
        model.set_image(image);
        let storage = match opened.mob_storage {
            Some(storage) => storage,
            None => MobStorage::persistent(root.join("mob.db")).expect("persistent mob storage"),
        };
        let (service, machine) =
            runtime_backed_service(&root, opened.persistence, Some(Arc::clone(&model)), true);

        let mut mob_builder = if options.restart {
            MobBuilder::for_resume(storage).notify_orchestrator_on_resume(false)
        } else {
            MobBuilder::new(definition(&mob_id), storage)
        }
        .with_session_service(Arc::clone(&service) as Arc<dyn meerkat_mob::MobSessionService>)
        .with_default_llm_client(Arc::clone(&model) as Arc<dyn LlmClient>)
        .register_tool_bundle(
            "lr",
            Arc::clone(&tools) as Arc<dyn meerkat_core::agent::AgentToolDispatcher>,
        );
        if let Some(gate) = &options.gate {
            mob_builder = mob_builder
                .with_durable_resume_gate(Arc::clone(gate) as Arc<dyn DurableResumeGate>);
        }
        let mob = if options.restart {
            tokio::time::timeout(STEP, mob_builder.resume())
                .await
                .expect("the mob resume answers")
                .expect("resume the mob")
        } else {
            mob_builder.create().await.expect("create the mob")
        };
        Self {
            service,
            machine,
            runtime_store,
            mob,
            model,
            tools,
            waited_out_lease: opened.waited_out_lease,
            _owners: opened.owners,
        }
    }

    pub async fn session_of(&self, identity: &str) -> SessionId {
        self.mob
            .resolve_bridge_session_id(&AgentIdentity::from(identity))
            .await
            .unwrap_or_else(|| panic!("{identity} is a session-backed member"))
    }

    /// Accept input `tag` on `session`; its input id.
    pub async fn accept(&self, session: &SessionId, tag: &str) -> InputId {
        self.accept_with_completion(session, tag).await.0
    }

    /// Accept input `tag` on `session`; its input id and its waiter.
    pub async fn accept_with_completion(
        &self,
        session: &SessionId,
        tag: &str,
    ) -> (
        InputId,
        Option<meerkat_runtime::completion::CompletionHandle>,
    ) {
        let (outcome, completion) = self
            .machine
            .accept_input_with_completion(
                session,
                Input::Prompt(PromptInput::new(prompt_text(tag), None)),
            )
            .await
            .expect("accept the input");
        let AcceptOutcome::Accepted { input_id, .. } = outcome else {
            panic!("input {tag} not accepted: {outcome:?}");
        };
        (input_id, completion)
    }

    /// Wait until `input` is terminal, bounded by `within`; whether it is.
    pub async fn wait_terminal(
        &self,
        session: &SessionId,
        input: &InputId,
        within: Duration,
    ) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if self.input_settled(session, input).await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    pub async fn input_settled(&self, session: &SessionId, input: &InputId) -> bool {
        self.machine
            .input_terminal_receipt(session, InteractionSelector::InputId(input.clone()))
            .await
            .ok()
            .flatten()
            .is_some_and(|read| read.report.is_resolved())
    }

    /// `input`'s terminal outcome, once it has one.
    pub async fn input_terminal(
        &self,
        session: &SessionId,
        input: &InputId,
    ) -> Option<meerkat_runtime::InputTerminalOutcome> {
        use meerkat_runtime::terminal_status::InputTerminalReceiptRead;
        let read = self
            .machine
            .input_terminal_receipt(session, InteractionSelector::InputId(input.clone()))
            .await
            .ok()
            .flatten()?;
        match read.report {
            InputTerminalReceiptRead::Finalized(receipt) => Some(receipt.terminal().clone()),
            InputTerminalReceiptRead::TerminalWithoutReceipt { terminal, .. } => Some(terminal),
            InputTerminalReceiptRead::Pending { terminal, .. } => terminal,
            _ => None,
        }
    }

    /// Whether `input` ended as a refused resume.
    pub async fn input_refused(&self, session: &SessionId, input: &InputId) -> bool {
        use meerkat_runtime::{InputAbandonReason, InputTerminalOutcome};
        matches!(
            self.input_terminal(session, input).await,
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::ResumeRefused,
            })
        )
    }

    /// How many interrupted-run notices `session`'s transcript holds: the
    /// notice a repaired tail ends with.
    pub async fn interrupted_notices(&self, session: &SessionId) -> Result<usize, String> {
        Ok(self
            .service
            .load_authoritative_session(session)
            .await
            .map_err(|error| format!("load the session: {error}"))?
            .ok_or("the session is gone")?
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::SystemNotice(notice)
                    if notice.body.as_deref().is_some_and(|body| body.contains("interrupted")))
            })
            .count())
    }

    /// How many compaction summaries `session`'s committed transcript holds.
    pub async fn compaction_summaries(&self, session: &SessionId) -> Result<usize, String> {
        Ok(self
            .service
            .load_authoritative_session(session)
            .await
            .map_err(|error| format!("load the session: {error}"))?
            .ok_or("the session is gone")?
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count())
    }

    /// How many times input `tag`'s prompt is in `session`'s transcript.
    pub async fn prompts_in_transcript(
        &self,
        session: &SessionId,
        tag: &str,
    ) -> Result<usize, String> {
        let prompt = prompt_text(tag);
        Ok(self
            .service
            .load_authoritative_session(session)
            .await
            .map_err(|error| format!("load the session: {error}"))?
            .ok_or("the session is gone")?
            .messages()
            .iter()
            .filter(|message| matches!(message, Message::User(user) if user.text_content().contains(&prompt)))
            .count())
    }

    pub async fn shut_down(self) {
        let _ = tokio::time::timeout(STEP, self.mob.shutdown()).await;
    }
}

// ---------------------------------------------------------------------------
// The killed incarnation
// ---------------------------------------------------------------------------

/// Set up the row's mob, start the main input and hold every Core object
/// until the harness kills the incarnation at the kill switch. A deploy row
/// starts every durable worker's input and ends the incarnation the way a
/// host deploys instead ([`deploy_stop`]).
pub async fn run_until_killed(
    store: &Store,
    plan: LongRunPlan,
    kill: Arc<KillSwitch>,
    race: Option<Arc<dyn RaceHook>>,
    log_to_file: bool,
) {
    let root = store.root().to_path_buf();
    let deploy = kill.deploy();
    // A child deploy row stops on SIGTERM, as the host's pod does: the
    // handler is in place before the harness can send it.
    let sigterm = (deploy.is_some() && in_child()).then(|| {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install the SIGTERM handler")
    });
    let workers = deploy.map_or(&[WORKER][..], |deploy| deploy.members());
    let incarnation = Incarnation::open(
        store,
        OpenOptions {
            plan,
            restart: false,
            kill: Some(Arc::clone(&kill)),
            race,
            gate: None,
            log_to_file,
        },
    )
    .await;
    for worker in workers {
        incarnation
            .mob
            .spawn_spec(SpawnMemberSpec::new(WORKER, AgentIdentity::from(*worker)))
            .await
            .expect("spawn a durable worker");
    }
    if uses_peer(&plan) {
        incarnation
            .mob
            .spawn_spec(SpawnMemberSpec::new(PEER, AgentIdentity::from(PEER)))
            .await
            .expect("spawn the peer");
        incarnation
            .mob
            .wire(AgentIdentity::from(WORKER), AgentIdentity::from(PEER))
            .await
            .expect("wire the worker to the peer");
        let peer_session = incarnation.session_of(PEER).await;
        let peer_id = incarnation
            .service
            .comms_runtime(&peer_session)
            .await
            .expect("the peer has a comms runtime")
            .peer_id()
            .expect("the peer has a peer id")
            .to_string();
        std::fs::write(root.join("peer-id"), &peer_id).expect("record the peer id");
        incarnation.model.set_peer_id(&peer_id);
        incarnation.tools.set_peer_id(&peer_id);
    }
    let mut sessions = Vec::with_capacity(workers.len());
    for worker in workers {
        let session = incarnation.session_of(worker).await;
        std::fs::write(
            root.join(member_file("session-id", worker)),
            session.to_string(),
        )
        .expect("record the session");
        sessions.push((*worker, session));
    }
    let session = sessions[0].1.clone();
    let mut members: Vec<SessionId> = sessions
        .iter()
        .map(|(_, session)| session.clone())
        .collect();
    if uses_peer(&plan) {
        members.push(incarnation.session_of(PEER).await);
    }
    for member in &members {
        if let Some(comms) = incarnation.service.comms_runtime(member).await {
            kill.hold_comms(&comms);
        }
    }
    if root.join("adopt").exists() {
        // An identity-first worker: its identity converges under this
        // incarnation's reconcile lease before the input starts.
        incarnation
            .mob
            .adopt_member_identity_declaration(adoption(
                incarnation.mob.mob_id(),
                &AgentIdentity::from(WORKER),
                &session,
            ))
            .await
            .expect("adopt the worker's identity");
        wait_converged(&incarnation.mob, WORKER, 0, STEP)
            .await
            .expect("the adopted identity converges");
        let epoch = converged_epoch(&incarnation.mob, WORKER).await.unwrap_or(0);
        std::fs::write(root.join("lease-epoch"), epoch.to_string()).expect("record the epoch");
    }
    // Record each worker's run, and for the first worker a comms call's
    // start (the harness cannot see it from its own tools) and a raced run's
    // end; the rows that need those run one worker.
    for (worker, worker_session) in &sessions {
        let mut events = incarnation
            .service
            .subscribe_session_events(worker_session)
            .await
            .expect("subscribe to the worker's events");
        let recorder_root = root.clone();
        let recorder_kill = Arc::clone(&kill);
        let run_file = member_file("run-id", worker);
        let marker = format!("input={}", worker_tag(worker));
        let first = *worker == WORKER;
        let unrepeatable_call = call_id(plan.kill_round, 0);
        let raced = plan.kill_point == KillPoint::RebaseRace;
        tokio::spawn(async move {
            while let Some(envelope) = events.next().await {
                match envelope.payload {
                    AgentEvent::RunStarted {
                        identity, input, ..
                    } if input
                        .prompt_text()
                        .is_some_and(|text| text.contains(&marker)) =>
                    {
                        if let Some(run_id) = identity.run_id {
                            std::fs::write(
                                recorder_root.join(&run_file),
                                serde_json::to_string(&run_id).expect("encode run id"),
                            )
                            .expect("record the run");
                        }
                    }
                    AgentEvent::ToolExecutionStarted { id, name }
                        if first
                            && id == unrepeatable_call
                            && (name == "send_message" || name == "send_request") =>
                    {
                        std::fs::write(recorder_root.join("unrepeatable-started"), name)
                            .expect("record the comms start");
                    }
                    // A raced run whose compaction was refused stops; that
                    // is where the race row ends its first incarnation.
                    AgentEvent::RunFailed { .. } | AgentEvent::RunCompleted { .. }
                        if first && raced && recorder_root.join("race").exists() =>
                    {
                        recorder_kill.fire("rebase_race").await;
                    }
                    _ => {}
                }
            }
        });
    }
    let mut waiters = Vec::with_capacity(sessions.len());
    for (worker, worker_session) in &sessions {
        let (input, waiter) = incarnation
            .accept_with_completion(worker_session, worker_tag(worker))
            .await;
        std::fs::write(
            root.join(member_file("input-id", worker)),
            serde_json::to_string(&input).expect("encode input id"),
        )
        .expect("record the input");
        waiters.push((*worker, waiter));
    }
    if let Some(deploy) = deploy {
        deploy_stop(incarnation, &kill, deploy, sigterm, waiters).await;
        return;
    }
    drop(waiters);
    // Hold every Core object until the harness kills this incarnation.
    let mut fired = kill.subscribe();
    let _ = fired.wait_for(|fired| *fired).await;
    std::future::pending::<()>().await;
    drop(incarnation);
}

/// End a deploy row's first incarnation as a host deploys: once every
/// worker's run is in flight at the stop point, SIGTERM (in process, its
/// stand-in) starts the mob's terminal Shutdown inside the host's budget,
/// with no queued-input choice unless the row names a requeue bound. The
/// incarnation then records how the Shutdown and each worker's waiter
/// answered, drops every Core object and ends.
async fn deploy_stop(
    incarnation: Incarnation,
    kill: &Arc<KillSwitch>,
    deploy: DeployConfig,
    sigterm: Option<tokio::signal::unix::Signal>,
    waiters: Vec<(
        &'static str,
        Option<meerkat_runtime::completion::CompletionHandle>,
    )>,
) {
    let mut in_flight = kill.subscribe_in_flight();
    let _ = in_flight.wait_for(|in_flight| *in_flight).await;
    match sigterm {
        Some(mut sigterm) => {
            let _ = sigterm.recv().await;
        }
        None => {
            let mut terminate = kill.subscribe_terminate();
            let _ = terminate.wait_for(|terminate| *terminate).await;
        }
    }
    let mut options = meerkat_mob::ShutdownOptions::default()
        .with_deadline(meerkat_core::time_compat::Instant::now() + SHUTDOWN_BUDGET);
    if let Some(max_age) = deploy.max_age {
        options = options.with_queued_inputs(meerkat_mob::QueuedInputsOnShutdown::Requeue(
            meerkat_mob::RequeueMaxAge::new(max_age).expect("an accepted requeue bound"),
        ));
    }
    let started = Instant::now();
    let shutdown = match tokio::time::timeout(
        SHUTDOWN_BUDGET + Duration::from_secs(5),
        incarnation.mob.shutdown_with_report(options),
    )
    .await
    {
        Ok(Ok(_)) => "ok".to_string(),
        Ok(Err(error)) => format!("failed: {error}"),
        Err(_) => "did not return".to_string(),
    };
    if deploy.kill_mid_suspend {
        // The row kills this process while the stop drops the last worker's
        // call; a stop that finishes before the kill lands records nothing,
        // so the row reads exactly what a kill mid-stop leaves.
        std::future::pending::<()>().await;
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut answers = serde_json::Map::new();
    for (worker, waiter) in waiters {
        let answer = match waiter {
            None => "no waiter".to_string(),
            Some(waiter) => match tokio::time::timeout(Duration::from_secs(5), waiter.wait()).await
            {
                Ok(Err(error)) if error.is_requeued_across_stop() => {
                    "requeued_across_stop".to_string()
                }
                Ok(Err(error)) => format!("failed: {error}"),
                Ok(Ok(outcome)) => format!("finished: {outcome:?}"),
                Err(_) => "no answer".to_string(),
            },
        };
        answers.insert(worker.to_string(), serde_json::Value::String(answer));
    }
    drop(incarnation);
    kill.stopped(&serde_json::json!({
        "shutdown": shutdown,
        "elapsed_ms": elapsed_ms,
        "waiters": answers,
    }));
}

/// Reopen a row's mob (no new member, no new input) and hold every Core
/// object until the harness kills this incarnation too: the run the earlier
/// incarnation left resumes here, and the plan's kill point, in a later
/// round, stops it again.
pub async fn run_resumed_until_killed(store: &Store, plan: LongRunPlan, kill: Arc<KillSwitch>) {
    let incarnation = Incarnation::open(
        store,
        OpenOptions {
            plan,
            restart: true,
            kill: Some(Arc::clone(&kill)),
            race: None,
            gate: None,
            log_to_file: true,
        },
    )
    .await;
    let session = incarnation.session_of(WORKER).await;
    if let Some(comms) = incarnation.service.comms_runtime(&session).await {
        kill.hold_comms(&comms);
    }
    let mut fired = kill.subscribe();
    let _ = fired.wait_for(|fired| *fired).await;
    std::future::pending::<()>().await;
    drop(incarnation);
}

/// The declaration that adopts `member`'s existing session as its identity.
pub fn adoption(
    mob_id: &MobId,
    member: &AgentIdentity,
    session_id: &SessionId,
) -> meerkat_mob::identity::AdoptMemberIdentityDeclaration {
    use meerkat_mob::identity::*;
    AdoptMemberIdentityDeclaration {
        mob_id: mob_id.clone(),
        agent_identity: member.clone(),
        request_id: IdentityAdoptionId::new(format!("adopt-{member}")).expect("adoption id"),
        precondition: IdentityAdoptionPrecondition::ExpectedAbsent,
        declaration_scope: IdentityDeclarationScopeId::new("long-run-policy")
            .expect("declaration scope"),
        declaration_revision: 1,
        session: DesiredSessionTarget {
            session_id: session_id.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(session_id),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: DesiredSessionAuthorityPolicy::RequireExisting,
        },
        member: IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from(WORKER),
            profile_override: None,
            model_override: None,
            external_addressable_override: None,
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: DesiredExecution::ControllingSession,
        },
        wiring_custody: IdentityWiringCustody::ExternalManaged,
        owned_wiring: BTreeSet::new(),
        convergence: IdentityConvergenceMode::Drain { max_wait_ms: 5_000 },
    }
}

/// The lease epoch `member`'s identity converged under, if it did.
pub async fn converged_epoch(mob: &MobHandle, member: &str) -> Option<u64> {
    use meerkat_mob::identity::{
        IdentityConvergenceStatus, IdentityReconcileDecision, IdentityStoredObservation,
    };
    match mob
        .identity_convergence_status(&AgentIdentity::from(member))
        .await
        .ok()?
    {
        IdentityStoredObservation::Valid(IdentityConvergenceStatus {
            decision: Some(IdentityReconcileDecision::Converged),
            lease_epoch: Some(epoch),
            ..
        }) => Some(epoch),
        _ => None,
    }
}

/// Wait until `member`'s identity converged under a lease epoch above
/// `after_epoch`, within `bound`.
pub async fn wait_converged(
    mob: &MobHandle,
    member: &str,
    after_epoch: u64,
    bound: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + bound;
    while Instant::now() < deadline {
        if converged_epoch(mob, member)
            .await
            .is_some_and(|epoch| epoch > after_epoch)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!(
        "{member} did not converge under a lease epoch above {after_epoch} within {bound:?}"
    ))
}

/// Wait until the supervisor route of `mob_id`, which an in-process
/// incarnation published, is released, bounded by five seconds.
pub async fn await_supervisor_route_release(mob_id: &str) -> Result<(), String> {
    let participant_name = format!("{mob_id}/__mob_supervisor__");
    let deadline = Instant::now() + Duration::from_secs(5);
    while meerkat_comms::InprocRegistry::global().contains_name(&participant_name) {
        if Instant::now() >= deadline {
            return Err(format!(
                "the supervisor route {participant_name} stayed published after the shutdown"
            ));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

/// What the killed incarnation left on disk.
#[derive(Clone)]
pub struct KilledState {
    pub session: SessionId,
    pub input: InputId,
    pub run: KilledRun,
}

/// What a deploy row's first incarnation left on disk for `member`; its
/// dispatched keys are its own run's.
pub fn read_killed_member(root: &Path, member: &str) -> Result<KilledState, String> {
    let read = |name: &str| {
        let file = member_file(name, member);
        std::fs::read_to_string(root.join(&file)).map_err(|error| format!("{file}: {error}"))
    };
    let session =
        SessionId::parse(read("session-id")?.trim()).map_err(|error| error.to_string())?;
    let input: InputId =
        serde_json::from_str(&read("input-id")?).map_err(|error| error.to_string())?;
    let run_id: RunId =
        serde_json::from_str(&read("run-id")?).map_err(|error| error.to_string())?;
    let dispatched = DispatchLog::read(&root.join("dispatched.jsonl"))
        .into_iter()
        .filter(|key| key.run_id == run_id)
        .collect();
    Ok(KilledState {
        session,
        input,
        run: KilledRun { run_id, dispatched },
    })
}

pub fn read_killed(root: &Path) -> Result<KilledState, String> {
    let read = |name: &str| {
        std::fs::read_to_string(root.join(name)).map_err(|error| format!("{name}: {error}"))
    };
    let session =
        SessionId::parse(read("session-id")?.trim()).map_err(|error| error.to_string())?;
    let input: InputId =
        serde_json::from_str(&read("input-id")?).map_err(|error| error.to_string())?;
    let run_id: RunId =
        serde_json::from_str(&read("run-id")?).map_err(|error| error.to_string())?;
    Ok(KilledState {
        session,
        input,
        run: KilledRun {
            run_id,
            dispatched: DispatchLog::read(&root.join("dispatched.jsonl")),
        },
    })
}

// ---------------------------------------------------------------------------
// The restart and its report
// ---------------------------------------------------------------------------

/// What one restart found.
pub struct Restarted {
    pub report: RestartReport,
    pub refused: bool,
    pub session: SessionId,
    /// How many times the main input's prompt appears in the final
    /// transcript.
    pub main_prompts: usize,
    /// The run of the provisional tail the store still holds for the
    /// session's runtime, if any.
    pub provisional_tail_run: Option<RunId>,
}

/// The run of the provisional tail `runtime_store` holds for `session`'s
/// runtime, if any, read in the store's own session profile: a
/// head-canonical store refuses the whole-blob read.
pub async fn provisional_tail_run(
    runtime_store: &dyn RuntimeStore,
    session: &SessionId,
) -> Result<Option<RunId>, String> {
    let runtime = LogicalRuntimeId::for_session(session);
    match runtime_store.session_persistence_profile() {
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => Ok(runtime_store
            .load_head_canonical_provisional_tail(&runtime)
            .await
            .map_err(|error| format!("read the head-canonical tail: {error}"))?
            .map(|tail| tail.run_id().clone())),
        RuntimeSessionPersistenceProfile::WholeBlobV1 => Ok(runtime_store
            .load_whole_blob_provisional_tail(&runtime)
            .await
            .map_err(|error| format!("read the whole-blob tail: {error}"))?
            .map(|tail| tail.authority().run_id().clone())),
        other => Err(format!("no provisional tail read for the {other} profile")),
    }
}

/// With no runtime running, discard the provisional tail the store holds
/// for `session`'s runtime by its own authority: the state a crash leaves
/// between a failed run's tail discard and the runtime's requeue of its
/// inputs. Whether a tail was discarded.
pub async fn discard_provisional_tail(store: &Store, session: &SessionId) -> Result<bool, String> {
    let opened = open_persistence(store, true).await;
    let runtime_store = opened.persistence.runtime_store();
    let runtime = LogicalRuntimeId::for_session(session);
    match runtime_store.session_persistence_profile() {
        RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
            match runtime_store
                .load_head_canonical_provisional_tail(&runtime)
                .await
                .map_err(|error| format!("read the head-canonical tail: {error}"))?
            {
                Some(tail) => runtime_store
                    .discard_head_canonical_provisional_tail(&runtime, &tail)
                    .await
                    .map_err(|error| format!("discard the head-canonical tail: {error}")),
                None => Ok(false),
            }
        }
        RuntimeSessionPersistenceProfile::WholeBlobV1 => {
            match runtime_store
                .load_whole_blob_provisional_tail(&runtime)
                .await
                .map_err(|error| format!("read the whole-blob tail: {error}"))?
            {
                Some(tail) => runtime_store
                    .discard_whole_blob_provisional_tail(&runtime, tail.authority())
                    .await
                    .map_err(|error| format!("discard the whole-blob tail: {error}")),
                None => Ok(false),
            }
        }
        other => Err(format!(
            "no provisional tail discard for the {other} profile"
        )),
    }
}

/// Reopen over the store with `gate`, do the plan's before-resume action at
/// the gate, run until idle, and report.
pub async fn restart(
    store: &Store,
    plan: LongRunPlan,
    gate: Option<Arc<HarnessGate>>,
    killed: &KilledState,
    fence_leases: bool,
) -> Result<Restarted, String> {
    let incarnation = Incarnation::open(
        store,
        OpenOptions {
            plan,
            restart: true,
            kill: None,
            race: None,
            gate: gate.clone(),
            log_to_file: false,
        },
    )
    .await;
    let session = killed.session.clone();
    let mut queued = None;
    // A stop or retire before the resume ends the live session; its events
    // are subscribed to while the gate still holds the resume.
    let mut held_events = None;
    if let Some(gate) = &gate {
        match plan.before_resume {
            BeforeResume::Nothing => gate.release(),
            BeforeResume::QueueInput => {
                // The resumed run goes first wherever the input is queued.
                let _ = gate.wait_asked(STEP).await;
                queued = Some(incarnation.accept(&session, QUEUED).await);
                gate.release();
            }
            BeforeResume::Stop | BeforeResume::Retire => {
                if !gate.wait_asked(STEP).await {
                    return Err("the runtime never asked before resuming the run".to_string());
                }
                held_events = Some(
                    incarnation
                        .service
                        .subscribe_session_events_from(
                            &session,
                            meerkat_core::SessionEventCursor::Earliest,
                        )
                        .await
                        .map_err(|error| format!("subscribe to the session's events: {error}"))?,
                );
                let machine = Arc::clone(&incarnation.machine);
                let target = session.clone();
                let retire = plan.before_resume == BeforeResume::Retire;
                let action = tokio::spawn(async move {
                    if retire {
                        machine
                            .retire_runtime_control_plane(&LogicalRuntimeId::for_session(&target))
                            .await
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    } else {
                        // The stop of the registration the resume runs on,
                        // awaited to its terminal completion: its cleanup
                        // may outlive a plain stop call's bounded wait.
                        let Some(registration) =
                            machine.current_session_registration_witness(&target).await
                        else {
                            return Err("the session has no registration to stop".to_string());
                        };
                        match machine
                            .stop_runtime_executor_until_terminal_if_current(
                                &registration,
                                "the long run harness stops it",
                            )
                            .await
                        {
                            Ok(true) => Ok(()),
                            Ok(false) => Err("the stopped registration was replaced".to_string()),
                            Err(error) => Err(error.to_string()),
                        }
                    }
                });
                // The stop or retire settles the resumable run while the
                // gate holds the resume; then the gate answers.
                let _ = incarnation
                    .wait_terminal(
                        &session,
                        &killed.input,
                        PARK.saturating_sub(Duration::from_secs(1)),
                    )
                    .await;
                gate.release();
                match tokio::time::timeout(STEP, action).await {
                    Ok(Ok(Ok(()))) => {}
                    Ok(Ok(Err(error))) => {
                        return Err(format!("the before-resume action failed: {error}"));
                    }
                    Ok(Err(error)) => {
                        return Err(format!("the before-resume action panicked: {error}"));
                    }
                    Err(_) => return Err("the before-resume action never answered".to_string()),
                }
            }
        }
    }
    if !incarnation
        .wait_terminal(&session, &killed.input, STEP)
        .await
    {
        return Err(format!(
            "the input {} never reached a terminal",
            killed.input
        ));
    }
    if let Some(queued) = &queued
        && !incarnation.wait_terminal(&session, queued, STEP).await
    {
        return Err(format!(
            "the queued input {queued} never reached a terminal"
        ));
    }
    let refused = incarnation.input_refused(&session, &killed.input).await;
    let report = report_from(
        &incarnation,
        MAIN,
        &session,
        killed,
        fence_leases,
        held_events,
    )
    .await?;
    let main_prompts = incarnation.prompts_in_transcript(&session, MAIN).await?;
    let provisional_tail_run =
        provisional_tail_run(incarnation.runtime_store.as_ref(), &session).await?;
    incarnation.shut_down().await;
    kill_orphaned_process(store.root());
    Ok(Restarted {
        report,
        refused,
        session,
        main_prompts,
        provisional_tail_run,
    })
}

/// Kill the process a killed incarnation's shell call left running, by the
/// pid it recorded, if that pid still runs the call's `sleep`.
pub fn kill_orphaned_process(root: &Path) {
    let Some(pid) = std::fs::read_to_string(root.join("unrepeatable-started"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
    else {
        return;
    };
    let pid = pid.to_string();
    // The pid may name another process by now: kill only the call's sleep.
    let sleeping = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid])
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .ends_with("sleep")
        });
    if sleeping {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid])
            .status();
    }
}

/// Read what the restarted incarnation did for the worker's session.
pub async fn report(
    incarnation: &Incarnation,
    session: &SessionId,
    killed: &KilledState,
    fence_leases: bool,
) -> Result<RestartReport, String> {
    report_for(incarnation, MAIN, session, killed, fence_leases).await
}

/// Read what the restarted incarnation did for the session of the worker
/// whose scripted input is `tag`.
pub async fn report_for(
    incarnation: &Incarnation,
    tag: &str,
    session: &SessionId,
    killed: &KilledState,
    fence_leases: bool,
) -> Result<RestartReport, String> {
    report_from(incarnation, tag, session, killed, fence_leases, None).await
}

/// [`report_for`], reading the session's events from `held_events` when the
/// restart subscribed to them before its session stopped being live.
async fn report_from(
    incarnation: &Incarnation,
    tag: &str,
    session: &SessionId,
    killed: &KilledState,
    fence_leases: bool,
    held_events: Option<meerkat_core::comms::SessionEventSubscription>,
) -> Result<RestartReport, String> {
    // The runs, from the events the session retains.
    let subscription = match held_events {
        Some(subscription) => subscription,
        None => incarnation
            .service
            .subscribe_session_events_from(session, meerkat_core::SessionEventCursor::Earliest)
            .await
            .map_err(|error| format!("replay the session's events: {error}"))?,
    };
    let mut stream = subscription.stream;
    let mut resumed = None;
    let mut runs: Vec<RunId> = Vec::new();
    while let Ok(Some(envelope)) =
        tokio::time::timeout(Duration::from_millis(500), stream.next()).await
    {
        match envelope.payload {
            AgentEvent::RunResumed {
                run_id,
                round,
                resume_point,
                ..
            } => {
                let resume_point = match resume_point {
                    meerkat_core::RunResumePointKind::ToolsPending => ResumePointKind::ToolsPending,
                    meerkat_core::RunResumePointKind::AwaitingModel => {
                        ResumePointKind::AwaitingModel
                    }
                    other => return Err(format!("unknown resume point {other:?}")),
                };
                if resumed.is_none() {
                    resumed = Some(ResumedRun {
                        run_id: run_id.clone(),
                        round,
                        resume_point,
                    });
                }
                if !runs.contains(&run_id) {
                    runs.push(run_id);
                }
            }
            AgentEvent::RunStarted {
                identity, input, ..
            } => {
                let scripted = input.prompt_text().is_some_and(|text| {
                    text.contains(&format!("input={tag}"))
                        || text.contains(&format!("input={QUEUED}"))
                });
                if scripted
                    && let Some(run_id) = identity.run_id
                    && !runs.contains(&run_id)
                {
                    runs.push(run_id);
                }
            }
            _ => {}
        }
    }
    let input_run = runs.first().cloned();
    let dispatched: Vec<ToolCallDedupeKey> = incarnation
        .tools
        .log()
        .keys()
        .into_iter()
        .filter(|key| Some(&key.run_id) == input_run.as_ref())
        .collect();

    // The transcript the input's runs left.
    let transcript = incarnation
        .service
        .load_authoritative_session(session)
        .await
        .map_err(|error| format!("load the session: {error}"))?
        .ok_or("the session is gone")?;
    let mut transcript_rounds = Vec::new();
    let mut transcript_results = Vec::new();
    let mut interrupted = Vec::new();
    let mut other_writer_messages = 0;
    for message in transcript.messages() {
        match message {
            Message::BlockAssistant(assistant) => {
                let text = assistant.text_blocks().collect::<Vec<_>>().join("\n");
                if let Some(round) = markers(&text)
                    .into_iter()
                    .find_map(|(found, round)| (found == tag).then_some(round).flatten())
                {
                    transcript_rounds.push(round);
                }
            }
            Message::ToolResults { results, .. } => {
                for result in results {
                    if !result.tool_use_id.starts_with("call-") {
                        continue;
                    }
                    transcript_results.push(result.tool_use_id.clone());
                    let content = meerkat_core::types::text_content(&result.content);
                    if result.is_error && content.contains("outcome_uncertain") {
                        interrupted.push(result.tool_use_id.clone());
                    }
                }
            }
            Message::System(system) if system.content.contains(OTHER_WRITER) => {
                other_writer_messages += 1;
            }
            _ => {}
        }
    }

    // The input's receipts, over the killed run and the run that carried it.
    let runtime = LogicalRuntimeId::for_session(session);
    let mut receipts = 0;
    let mut counted = BTreeSet::new();
    for run in std::iter::once(killed.run.run_id.clone()).chain(input_run.clone()) {
        if !counted.insert(run.to_string()) {
            continue;
        }
        receipts += incarnation
            .runtime_store
            .load_committed_boundary_receipts(&runtime, &run)
            .await
            .map_err(|error| format!("read the receipts of {run}: {error}"))?
            .iter()
            .filter(|receipt| receipt.contributing_input_ids.contains(&killed.input))
            .count();
    }
    let inputs_settled = incarnation.input_settled(session, &killed.input).await;
    Ok(RestartReport {
        resumed,
        waited_out_lease: fence_leases && !incarnation.waited_out_lease.is_zero(),
        runs,
        model_call_rounds: incarnation.model.rounds_of(tag),
        dispatched,
        interrupted,
        transcript_rounds,
        transcript_results,
        receipts,
        other_writer_messages,
        inputs_settled,
    })
}

/// What one worker of a deploy row's restart did.
pub struct DeployRestart {
    pub member: &'static str,
    pub report: RestartReport,
    /// The worker's input's terminal outcome.
    pub terminal: Option<meerkat_runtime::InputTerminalOutcome>,
    /// How many times its prompt is in the final transcript.
    pub prompts: usize,
    /// How many interrupted-run notices its transcript holds.
    pub notices: usize,
    /// How many compaction summaries its committed transcript holds.
    pub summaries: usize,
    /// Whether an input accepted on its session after the restart ran to
    /// its end: no stop mark or resume mark outlives the restart.
    pub follow_up_consumed: bool,
}

/// The input a restart runs on each worker's session after reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// A one-round input without tool calls.
    Queued,
    /// A full scripted run of the plan's rounds ([`FOLLOW`]), whose tool
    /// rounds take round commits.
    Scripted,
}

/// Reopen a deploy row's store, wait until every worker's input is
/// terminal, read each worker's report, then run one more input on each
/// worker's session.
pub async fn restart_deploy(
    store: &Store,
    plan: LongRunPlan,
    workers: &[(&'static str, KilledState)],
    fence_leases: bool,
) -> Result<Vec<DeployRestart>, String> {
    restart_workers(store, plan, workers, fence_leases, None, FollowUp::Queued).await
}

/// Reopen a row's store with `gate`, wait until every worker's input is
/// terminal, read each worker's report, then run `follow_up` on each
/// worker's session.
pub async fn restart_workers(
    store: &Store,
    plan: LongRunPlan,
    workers: &[(&'static str, KilledState)],
    fence_leases: bool,
    gate: Option<Arc<HarnessGate>>,
    follow_up: FollowUp,
) -> Result<Vec<DeployRestart>, String> {
    let incarnation = Incarnation::open(
        store,
        OpenOptions {
            plan,
            restart: true,
            kill: None,
            race: None,
            gate,
            log_to_file: false,
        },
    )
    .await;
    for (member, killed) in workers {
        if !incarnation
            .wait_terminal(&killed.session, &killed.input, STEP)
            .await
        {
            return Err(format!(
                "{member}'s input {} never reached a terminal",
                killed.input
            ));
        }
    }
    let mut restarted = Vec::with_capacity(workers.len());
    for (member, killed) in workers {
        let tag = worker_tag(member);
        restarted.push(DeployRestart {
            member,
            report: report_for(&incarnation, tag, &killed.session, killed, fence_leases).await?,
            terminal: incarnation
                .input_terminal(&killed.session, &killed.input)
                .await,
            prompts: incarnation
                .prompts_in_transcript(&killed.session, tag)
                .await?,
            notices: incarnation.interrupted_notices(&killed.session).await?,
            summaries: incarnation.compaction_summaries(&killed.session).await?,
            follow_up_consumed: false,
        });
    }
    let follow_up_tag = match follow_up {
        FollowUp::Queued => QUEUED,
        FollowUp::Scripted => FOLLOW,
    };
    for (restart, (_, killed)) in restarted.iter_mut().zip(workers) {
        let follow_up = incarnation.accept(&killed.session, follow_up_tag).await;
        restart.follow_up_consumed = incarnation
            .wait_terminal(&killed.session, &follow_up, STEP)
            .await
            && matches!(
                incarnation
                    .input_terminal(&killed.session, &follow_up)
                    .await,
                Some(meerkat_runtime::InputTerminalOutcome::Consumed)
            );
    }
    incarnation.shut_down().await;
    kill_orphaned_process(store.root());
    Ok(restarted)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A fresh fixture root under `base` for one row, with its mob id.
pub fn fresh_root(base: &Path, rows: &AtomicU32) -> PathBuf {
    let row = rows.fetch_add(1, Ordering::SeqCst);
    let root = base.join(format!("row-{row:02}"));
    std::fs::create_dir_all(&root).expect("row root");
    let root = root.canonicalize().expect("absolute row root");
    std::fs::write(
        root.join("mob-id"),
        format!("long-run-{}", uuid::Uuid::new_v4().simple()),
    )
    .expect("mob id");
    root
}

pub fn write_plan(root: &Path, plan: &LongRunPlan) {
    std::fs::write(root.join("plan.json"), encode_plan(plan).to_string()).expect("write the plan");
}

/// Have every scripted model opened over `root` from now on answer the
/// model call of round `round` of the main input once with a provider error
/// that is not retried.
pub fn fail_model_call(root: &Path, round: u64) {
    std::fs::write(root.join(MODEL_FAILS), round.to_string()).expect("write the failing round");
}

pub fn read_plan(root: &Path) -> LongRunPlan {
    let text = std::fs::read_to_string(root.join("plan.json")).expect("read the plan");
    decode_plan(&serde_json::from_str(&text).expect("decode the plan"))
}
