//! Hook runtimes (in-process, command, HTTP) and deterministic default engine.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use meerkat_core::time_compat::wasm as time;
    // Keep one canonical task route in this private facade, even when unused.
    #[allow(unused_imports)]
    pub use meerkat_core::tokio::{spawn, task};
    pub use tokio_with_wasm::alias::*;
}

// Keep this anchor in the root module with the hook inventory
// declarations so optimized linking retains their archive member.
#[doc(hidden)]
#[inline(never)]
pub fn link_embedded_registrations() {}

// Skill registration
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "hook-authoring",
        name: "Hook Authoring",
        description: "Writing hooks for the 8 hook points, execution modes, and decision semantics",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["hooks"],
        body: include_str!("../skills/hook-authoring/SKILL.md"),
        extensions: &[],
    }
}

// Capability registration
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Hooks,
        description: "8 hook points, 3 runtimes (in-process/command/HTTP), observe/allow/deny semantics",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: None,
    }
}

use futures::StreamExt;
use meerkat_core::HookFailureReason;
use meerkat_core::config::{CommandRuntimeConfig, HookAdapterConfig};
use meerkat_core::hooks::{
    HookBackgroundAttribution, HookBackgroundCompletion, HookBackgroundResult,
    HookBackgroundSessionStatus, HookBackgroundSkip, HookBackgroundSkipReason,
    HookBackgroundSourceId,
};
use meerkat_core::time_compat::Duration;
use meerkat_core::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookEntryConfig, HookExecutionMode,
    HookExecutionReport, HookId, HookInvocation, HookLaunchRefusal, HookOutcome, HookRunOverrides,
    HooksConfig,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_sandbox::ProcessChild;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio::process::Command;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;

pub use meerkat_core::config::HookInProcessHandlerId as InProcessHookHandlerId;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod command_confinement_tests;

/// Durable process custody for command hooks, supplied by the host.
///
/// meerkat-hooks cannot depend on the host's custody implementation, so the
/// host adapts it to this seam. With custody installed, a command hook is
/// reserved durably before it is spawned, runs behind a spawn gate until its
/// leader is recorded, and stays in custody until its process group is proven
/// exited, so a host that dies mid-hook leaves the hook to the next
/// incarnation's recovery instead of running on unowned.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
pub trait CommandHookProcessCustody: Send + Sync {
    /// Spawn the final command through the host's retained launch boundary.
    ///
    /// The adapter configures all streams and establishes custody before it
    /// returns the exclusive child. No mutable command crosses this boundary.
    /// Every `Err` guarantees the target never entered. After entry, return the
    /// owned child and report later outcomes through its custody lifecycle.
    /// Existing implementations must adopt this method explicitly: falling
    /// back to `prepare` cannot establish required confinement.
    async fn spawn(
        &self,
        _hook_id: &HookId,
        _run_id: Option<&meerkat_core::RunId>,
        _command: &CommandRuntimeConfig,
    ) -> Result<ProcessChild, HookFailureReason> {
        Err(HookFailureReason::ConfinementRefused {
            refusal: meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
        })
    }

    /// Reserve custody for a hook process that will run `program args...`
    /// (inside run `run_id`, when the invocation belongs to one) and return
    /// the gated command to configure and spawn. The command must stay in the
    /// process group it was given.
    async fn prepare(
        &self,
        hook_id: &HookId,
        run_id: Option<&meerkat_core::RunId>,
        program: &std::ffi::OsStr,
        args: &[std::ffi::OsString],
    ) -> Result<(Box<dyn CommandHookCustodySpawn>, Command), CommandHookCustodyError>;
}

/// One reserved, gated command-hook spawn.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
pub trait CommandHookCustodySpawn: Send {
    /// Record the spawned leader and release the gate. Custody then holds the
    /// hook's process group until it is proven exited. On error the gate stays
    /// closed and the command never ran.
    async fn spawned(
        self: Box<Self>,
        child: &tokio::process::Child,
    ) -> Result<(), CommandHookCustodyError>;
}

/// Command-hook custody could not be established; the hook does not run.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct CommandHookCustodyError {
    pub reason: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for CommandHookCustodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "command hook process custody failed: {}", self.reason)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::error::Error for CommandHookCustodyError {}

/// Observer told the leader pid of every command-hook process group right
/// after spawn (the group id equals the leader pid).
#[cfg(unix)]
static COMMAND_HOOK_PROCESS_GROUP_OBSERVER: OnceLock<fn(i32)> = OnceLock::new();

/// Install the process-wide observer of command-hook process groups.
///
/// Command hooks run in their own process group, and members a hook starts
/// in the background can outlive the hook. A host that settles process
/// groups left by an earlier incarnation of itself (durable shell process
/// custody) installs an observer that registers each hook group as live, so
/// such recovery never mistakes a running hook group for an earlier
/// incarnation's tool. The first installation wins; returns whether this
/// call installed `observer`.
#[cfg(unix)]
pub fn set_command_hook_process_group_observer(observer: fn(i32)) -> bool {
    COMMAND_HOOK_PROCESS_GROUP_OBSERVER.set(observer).is_ok()
}

#[cfg(unix)]
fn observe_command_hook_process_group(child: &ProcessChild) {
    if let (Some(observer), Some(pid)) = (
        COMMAND_HOOK_PROCESS_GROUP_OBSERVER.get(),
        child.id().and_then(|pid| i32::try_from(pid).ok()),
    ) {
        observer(pid);
    }
}

#[cfg(unix)]
async fn terminate_child_process_group(child: &mut ProcessChild) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if let Some(pid) = child.id() {
        let pgid = Pid::from_raw(pid as i32);
        let _ = killpg(pgid, Signal::SIGTERM);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                let _ = killpg(pgid, Signal::SIGKILL);
                let _ = child.wait().await;
            }
            _ = child.wait() => {}
        }
    }
}

#[cfg(all(not(unix), not(target_arch = "wasm32")))]
async fn terminate_child_process_group(child: &mut ProcessChild) {
    let _ = child.kill().await;
}

#[cfg(not(target_arch = "wasm32"))]
fn remaining_until(deadline: tokio::time::Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        None
    } else {
        Some(remaining)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn abort_command_reader_tasks(
    stdout_task: &tokio::task::JoinHandle<Result<Vec<u8>, String>>,
    stderr_task: &tokio::task::JoinHandle<Result<Vec<u8>, String>>,
) {
    stdout_task.abort();
    stderr_task.abort();
}

/// Response returned by runtime adapters.
///
/// `deny_unknown_fields` keeps this contract fail-closed: a runtime that
/// emits retired vocabulary (e.g. the deleted semantic `patches` machinery)
/// gets an explicit deserialization error instead of silent acceptance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RuntimeHookResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
}

/// Typed owner for hook execution policy (ordering, timeout, and background
/// queue-pressure).
///
/// Ordering/timeout/queue-pressure used to live as loose fields read directly
/// off `HooksConfig` inside the engine execution path (remediation row #35).
/// They are resolved once into this typed owner at engine construction so the
/// execution path reads a single policy value rather than re-deriving defaults.
#[derive(Debug, Clone, Copy)]
pub struct HookExecutionPolicy {
    default_timeout_ms: u64,
    payload_max_bytes: usize,
    background_max_concurrency: usize,
}

impl HookExecutionPolicy {
    /// Resolve the typed policy from a [`HooksConfig`], normalizing the
    /// background concurrency to at least one slot.
    fn from_config(config: &HooksConfig) -> Self {
        Self {
            default_timeout_ms: config.default_timeout_ms,
            payload_max_bytes: config.payload_max_bytes,
            background_max_concurrency: config.background_max_concurrency.max(1),
        }
    }

    /// Resolve the effective per-invocation timeout for one entry.
    fn timeout_ms_for(&self, entry: &HookEntryConfig) -> u64 {
        entry.timeout_ms.unwrap_or(self.default_timeout_ms)
    }

    /// Maximum serialized payload / response size in bytes.
    pub fn payload_max_bytes(&self) -> usize {
        self.payload_max_bytes
    }

    /// Maximum number of background hook tasks allowed to run concurrently.
    pub fn background_max_concurrency(&self) -> usize {
        self.background_max_concurrency
    }
}

// Every scheduled task reserves one completion slot. Completed record ownership
// is bounded separately from the invocation being executed: variable attribution
// bytes <=1024, a repeated outcome hook ID <=1024, diagnostic bytes <=1024.
// Attribution is refused intact before scheduling; diagnostic truncation is explicit.
const BACKGROUND_COMPLETION_CAPACITY: usize = 64;
const BACKGROUND_ATTRIBUTION_BYTES: usize = 1024;
const BACKGROUND_DIAGNOSTIC_BYTES: usize = 1024;
// Conservative serialized ceiling from the capped attribution and diagnostic
// strings plus fixed-size coordinates. Retained strings are allocated at the cap.
#[cfg(test)]
const BACKGROUND_RECORD_BYTES: usize = 32 * 1024;
const BACKGROUND_TAKE_LIMIT: usize = 32;

/// Legacy ledger view. New records carry the complete typed completion; legacy
/// skip/drop variants remain readable for callers migrating their match arms.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundDispatchSignal {
    Skipped {
        hook_id: HookId,
    },
    Dropped {
        hook_id: HookId,
        reason: String,
    },
    Completed {
        completion: HookBackgroundCompletion,
    },
}

#[derive(Debug)]
struct RetainedBackgroundCompletion {
    completion: HookBackgroundCompletion,
    _capacity: OwnedSemaphorePermit,
}

#[derive(Debug, Default)]
struct BackgroundLedgerState {
    ready: Vec<RetainedBackgroundCompletion>,
    // Exact engine invocations and scheduled tasks, not an inference from the
    // ready queue or global JoinSet. Removed by the actual future's drop.
    running: Vec<meerkat_core::SessionId>,
}

struct BackgroundScopeGuard {
    ledger: BackgroundDispatchLedger,
    session_id: meerkat_core::SessionId,
}

impl Drop for BackgroundScopeGuard {
    fn drop(&mut self) {
        let mut state = self
            .ledger
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = state.running.iter().position(|id| id == &self.session_id) {
            state.running.swap_remove(index);
        }
        drop(state);
        self.ledger.ready.notify_waiters();
    }
}

/// The engine's existing process-local completion ledger. A slot is reserved
/// before task scheduling and held until transfer. A full ledger refuses new
/// scheduling through the current execution report, never evicts ready facts.
/// Neither snapshots, readiness, nor transfer prove model or durable delivery.
#[derive(Debug, Clone)]
pub struct BackgroundDispatchLedger {
    source_id: HookBackgroundSourceId,
    inner: Arc<std::sync::Mutex<BackgroundLedgerState>>,
    capacity: Arc<Semaphore>,
    next_ordinal: Arc<AtomicU64>,
    ready: Arc<Notify>,
}

impl Default for BackgroundDispatchLedger {
    fn default() -> Self {
        Self {
            source_id: HookBackgroundSourceId::new(),
            inner: Arc::new(std::sync::Mutex::new(BackgroundLedgerState::default())),
            capacity: Arc::new(Semaphore::new(BACKGROUND_COMPLETION_CAPACITY)),
            next_ordinal: Arc::new(AtomicU64::new(1)),
            ready: Arc::new(Notify::new()),
        }
    }
}

impl BackgroundDispatchLedger {
    fn new() -> Self {
        Self::default()
    }

    fn enter_scope(&self, session_id: &meerkat_core::SessionId) -> BackgroundScopeGuard {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running
            .push(session_id.clone());
        BackgroundScopeGuard {
            ledger: self.clone(),
            session_id: session_id.clone(),
        }
    }

    fn session_status(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<HookBackgroundSessionStatus> {
        let Ok(state) = self.inner.try_lock() else {
            return None;
        };
        Some(HookBackgroundSessionStatus {
            running: state.running.iter().filter(|id| *id == session_id).count(),
            ready: state
                .ready
                .iter()
                .filter(|item| &item.completion.attribution.session_id == session_id)
                .count(),
        })
    }

    fn reserve(&self) -> Result<(u64, OwnedSemaphorePermit), HookBackgroundSkipReason> {
        let capacity = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| HookBackgroundSkipReason::RetentionFull)?;
        let ordinal = self
            .next_ordinal
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| HookBackgroundSkipReason::OrdinalExhausted)?;
        Ok((ordinal, capacity))
    }

    async fn record(&self, completion: HookBackgroundCompletion, capacity: OwnedSemaphorePermit) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ready
            .push(RetainedBackgroundCompletion {
                completion,
                _capacity: capacity,
            });
        self.ready.notify_waiters();
    }

    /// Non-consuming typed ready records. This does not wait for running hooks.
    pub async fn completion_snapshot(&self) -> Vec<HookBackgroundCompletion> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ready
            .iter()
            .map(|item| item.completion.clone())
            .collect()
    }

    /// Compatibility view, now including successful and typed failed completions.
    pub async fn snapshot(&self) -> Vec<BackgroundDispatchSignal> {
        self.completion_snapshot()
            .await
            .into_iter()
            .map(|completion| BackgroundDispatchSignal::Completed { completion })
            .collect()
    }

    /// Explicit host transfer of all ready records. Session owners should use
    /// the scoped trait method instead. No acknowledgement or replay is implied.
    pub async fn drain(&self) -> Vec<BackgroundDispatchSignal> {
        std::mem::take(
            &mut self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ready,
        )
        .into_iter()
        .map(|item| BackgroundDispatchSignal::Completed {
            completion: item.completion,
        })
        .collect()
    }

    fn take_ready(
        &self,
        session_id: &meerkat_core::SessionId,
        run_id: Option<&meerkat_core::RunId>,
        limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        let limit = limit.min(BACKGROUND_TAKE_LIMIT);
        if limit == 0 {
            return Vec::new();
        }
        let Ok(mut state) = self.inner.try_lock() else {
            return Vec::new();
        };
        let ready = &mut state.ready;
        let mut transferred = Vec::new();
        let mut index = 0;
        while index < ready.len() && transferred.len() < limit {
            if ready[index]
                .completion
                .attribution
                .matches_scope(session_id, run_id)
            {
                transferred.push(ready.remove(index).completion);
            } else {
                index += 1;
            }
        }
        transferred
    }

    /// Readiness only, scoped to original ownership. Cancellation changes no
    /// record. Register before checking to avoid losing a racing completion.
    pub async fn wait_for_completion(
        &self,
        session_id: &meerkat_core::SessionId,
        run_id: Option<&meerkat_core::RunId>,
    ) {
        loop {
            let notified = self.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ready
                .iter()
                .any(|item| {
                    item.completion
                        .attribution
                        .matches_scope(session_id, run_id)
                })
            {
                return;
            }
            notified.await;
        }
    }
    fn take_session_ready(
        &self,
        session_id: &meerkat_core::SessionId,
        limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        let limit = limit.min(BACKGROUND_TAKE_LIMIT);
        if limit == 0 {
            return Vec::new();
        }
        let Ok(mut state) = self.inner.try_lock() else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut index = 0;
        while index < state.ready.len() && result.len() < limit {
            if &state.ready[index].completion.attribution.session_id == session_id {
                result.push(state.ready.remove(index).completion);
            } else {
                index += 1;
            }
        }
        result
    }

    async fn wait_for_session_completion(&self, session_id: &meerkat_core::SessionId) {
        loop {
            let notified = self.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .ready
                .iter()
                .any(|item| &item.completion.attribution.session_id == session_id)
            {
                return;
            }
            notified.await;
        }
    }
}

fn background_attribution_fits(hook_id: &HookId, invocation: &HookInvocation) -> bool {
    let call = invocation
        .tool_call
        .as_ref()
        .map(|call| call.tool_use_id.as_str())
        .or_else(|| {
            invocation
                .tool_result
                .as_ref()
                .map(|result| result.tool_use_id.as_str())
        });
    let request = match invocation.observation.as_ref() {
        Some(meerkat_core::HookObservation::PeerIngressCommitted(value)) => {
            value.request_id.as_deref()
        }
        _ => None,
    };
    hook_id
        .0
        .len()
        .saturating_add(call.map_or(0, str::len))
        .saturating_add(request.map_or(0, str::len))
        <= BACKGROUND_ATTRIBUTION_BYTES
}

fn bound_background_reason(reason: &mut HookFailureReason) -> bool {
    if let HookFailureReason::ExecutionFailed { message }
    | HookFailureReason::ConfigInvalid { message } = reason
        && message.len() > BACKGROUND_DIAGNOSTIC_BYTES
    {
        let mut end = BACKGROUND_DIAGNOSTIC_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        *message = message[..end].to_owned();
        return true;
    }
    false
}

/// Outcome of registering an in-process hook handler against a stable id.
///
/// In-process handler registration used to be a silent `HashMap::insert`
/// overwrite (remediation row #289), so swapping executable behavior under a
/// stable id produced no observable signal. Registration now returns this typed
/// outcome: a fresh id is `Registered`, and re-registering an existing id is a
/// `Revised` event carrying the new handler revision so the behavior change is
/// observable rather than silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerRegistrationOutcome {
    /// The id was previously unregistered; this is a fresh registration.
    Registered { revision: HookHandlerRevision },
    /// The id was already registered; the handler was replaced and its
    /// revision bumped to the carried value.
    Revised {
        previous: HookHandlerRevision,
        revision: HookHandlerRevision,
    },
}

/// Monotonic revision of an in-process hook handler registration.
///
/// Engine-local observable for handler re-registration (remediation row
/// #289); unrelated to the deleted semantic hook-patch machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookHandlerRevision(pub u64);

impl HandlerRegistrationOutcome {
    /// The handler revision in effect after this registration.
    pub fn revision(&self) -> HookHandlerRevision {
        match self {
            Self::Registered { revision } | Self::Revised { revision, .. } => *revision,
        }
    }

    /// Whether this registration replaced an already-registered handler.
    pub fn is_revision(&self) -> bool {
        matches!(self, Self::Revised { .. })
    }
}

#[cfg(not(target_arch = "wasm32"))]
type HandlerFuture = Pin<Box<dyn Future<Output = Result<RuntimeHookResponse, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
type HandlerFuture = Pin<Box<dyn Future<Output = Result<RuntimeHookResponse, String>>>>;

pub type InProcessHookHandler = Arc<dyn Fn(HookInvocation) -> HandlerFuture + Send + Sync>;

/// Registered in-process handler with its observable revision.
///
/// The revision is bumped every time a handler is (re-)registered under an
/// existing id, so dispatch resolves a canonical, revisioned handler authority
/// rather than an anonymous map slot that can be silently overwritten.
struct RegisteredHandler {
    handler: InProcessHookHandler,
    revision: HookHandlerRevision,
}

/// Effective hook entries plus their typed adapters for one run.
///
/// Either borrows the engine's construction-time base resolution or owns a
/// freshly-layered resolution when run overrides are present. Adapters are
/// resolved at this boundary so the execution path reads typed variants.
enum ResolvedEntries<'a> {
    Base {
        entries: &'a [HookEntryConfig],
        adapters: &'a HashMap<HookId, HookAdapterConfig>,
    },
    Owned {
        entries: Vec<HookEntryConfig>,
        adapters: HashMap<HookId, HookAdapterConfig>,
    },
}

impl<'a> ResolvedEntries<'a> {
    fn base(engine: &'a DefaultHookEngine) -> Self {
        Self::Base {
            entries: engine.base_entries.as_slice(),
            adapters: engine.base_adapters.as_ref(),
        }
    }

    fn owned(entries: Vec<HookEntryConfig>, adapters: HashMap<HookId, HookAdapterConfig>) -> Self {
        Self::Owned { entries, adapters }
    }

    fn entries(&self) -> &[HookEntryConfig] {
        match self {
            Self::Base { entries, .. } => entries,
            Self::Owned { entries, .. } => entries.as_slice(),
        }
    }

    fn adapter(&self, hook_id: &HookId) -> Option<&HookAdapterConfig> {
        match self {
            Self::Base { adapters, .. } => adapters.get(hook_id),
            Self::Owned { adapters, .. } => adapters.get(hook_id),
        }
    }
}

/// Deterministic hook engine used by all control surfaces.
#[derive(Clone)]
pub struct DefaultHookEngine {
    base_entries: Arc<Vec<HookEntryConfig>>,
    /// Typed adapter resolved once per base entry at construction, keyed by
    /// hook id. The execution path reads the typed variant directly so no
    /// `serde_json::from_value` runs in `invoke_runtime` (remediation row #231).
    base_adapters: Arc<HashMap<HookId, HookAdapterConfig>>,
    base_validation_error: Option<String>,
    /// Typed execution policy (ordering/timeout/queue-pressure) resolved once
    /// at construction (remediation row #35).
    policy: HookExecutionPolicy,
    http_client: Arc<OnceLock<reqwest::Client>>,
    in_process_handlers: Arc<std::sync::RwLock<HashMap<InProcessHookHandlerId, RegisteredHandler>>>,
    /// Bounded retained completions. Scheduling pressure is returned in the
    /// current execution report and is never misrepresented as target entry.
    background_dispatch_ledger: BackgroundDispatchLedger,
    background_slots: Arc<Semaphore>,
    /// Existing task tracking on native targets. Reaping completed tasks does
    /// not establish per-session shutdown or process quiescence: an in-flight
    /// task retains an engine clone. Wasm retains its existing spawn route.
    #[cfg(not(target_arch = "wasm32"))]
    inflight_background: Arc<Mutex<tokio::task::JoinSet<()>>>,
    revision: Arc<AtomicU64>,
    /// Durable custody for command-hook processes, when the host supplies it.
    #[cfg(not(target_arch = "wasm32"))]
    process_custody: Option<Arc<dyn CommandHookProcessCustody>>,
}

impl DefaultHookEngine {
    pub fn new(config: HooksConfig) -> Self {
        // Validate entries (including registry uniqueness, row #280) and
        // resolve typed adapters (row #231) once at the construction boundary.
        // The first failure (entry validation, duplicate id, or malformed
        // command/HTTP adapter payload) becomes the engine-wide validation
        // error so every subsequent `execute`/`matching_hooks` call fails
        // closed with a typed `InvalidConfiguration` rather than re-parsing JSON
        // on the execution path.
        let (base_adapters, base_validation_error) = match Self::resolve_base(&config.entries) {
            // Adapters are the typed `HookAdapterConfig` already deserialized at
            // the config boundary (row #231), so building the lookup table is
            // an infallible projection — no JSON re-parse, no failure mode here.
            Ok(()) => (Self::resolve_adapters(&config.entries), None),
            // Construction still succeeds but the engine fails closed on
            // first use; an empty adapter map is never read past the
            // validation-error short-circuit.
            Err(err) => (HashMap::new(), Some(err.to_string())),
        };
        let policy = HookExecutionPolicy::from_config(&config);

        Self {
            base_entries: Arc::new(config.entries),
            base_adapters: Arc::new(base_adapters),
            base_validation_error,
            policy,
            http_client: Arc::new(OnceLock::new()),
            in_process_handlers: Arc::new(std::sync::RwLock::new(HashMap::new())),
            background_dispatch_ledger: BackgroundDispatchLedger::new(),
            background_slots: Arc::new(Semaphore::new(policy.background_max_concurrency())),
            #[cfg(not(target_arch = "wasm32"))]
            inflight_background: Arc::new(Mutex::new(tokio::task::JoinSet::new())),
            revision: Arc::new(AtomicU64::new(1)),
            #[cfg(not(target_arch = "wasm32"))]
            process_custody: None,
        }
    }

    /// Non-consuming readiness and explicit transfer of retained task facts.
    pub fn background_dispatch_ledger(&self) -> &BackgroundDispatchLedger {
        &self.background_dispatch_ledger
    }

    /// Validate an entry set: per-entry rules plus registry-level id
    /// uniqueness (remediation row #280). Duplicate ids are rejected so hook
    /// identity is registry-owned, not list-ordinal.
    fn resolve_base(entries: &[HookEntryConfig]) -> Result<(), HookEngineError> {
        let mut seen: HashSet<&HookId> = HashSet::with_capacity(entries.len());
        for entry in entries {
            Self::validate_entry(entry)?;
            if !seen.insert(&entry.id) {
                return Err(HookEngineError::InvalidConfiguration(format!(
                    "duplicate hook id: {}",
                    entry.id
                )));
            }
        }
        Ok(())
    }

    /// Project the typed adapter for every entry into a by-id lookup table.
    ///
    /// `entry.runtime` is already the typed [`HookAdapterConfig`] deserialized
    /// once at the config boundary (row #231), so this is a pure clone-into-map
    /// projection — no JSON parse, no failure mode.
    fn resolve_adapters(entries: &[HookEntryConfig]) -> HashMap<HookId, HookAdapterConfig> {
        entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.runtime.clone()))
            .collect()
    }

    /// Run command hooks under the host's durable process custody (see
    /// [`CommandHookProcessCustody`]).
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_command_process_custody(
        mut self,
        custody: Arc<dyn CommandHookProcessCustody>,
    ) -> Self {
        self.process_custody = Some(custody);
        self
    }

    pub fn with_in_process_handler(
        self,
        name: impl Into<InProcessHookHandlerId>,
        handler: InProcessHookHandler,
    ) -> Self {
        let next = self;
        if let Err(err) = next.insert_in_process_handler(name.into(), handler) {
            tracing::warn!("failed to register in-process hook handler: {}", err);
        }
        next
    }

    /// Register an in-process hook handler, returning a typed outcome.
    ///
    /// Re-registering an already-registered id is a typed `Revised` event that
    /// bumps the handler revision (remediation row #289), not a silent
    /// overwrite. A poisoned registry lock fails closed with a typed
    /// [`HookEngineError`].
    pub async fn register_in_process_handler(
        &self,
        name: impl Into<InProcessHookHandlerId>,
        handler: InProcessHookHandler,
    ) -> Result<HandlerRegistrationOutcome, HookEngineError> {
        self.insert_in_process_handler(name.into(), handler)
    }

    fn insert_in_process_handler(
        &self,
        name: InProcessHookHandlerId,
        handler: InProcessHookHandler,
    ) -> Result<HandlerRegistrationOutcome, HookEngineError> {
        let mut map = self.in_process_handlers.write().map_err(|err| {
            HookEngineError::InvalidConfiguration(format!(
                "in-process handler registry lock poisoned: {err}"
            ))
        })?;
        let revision = self.next_revision();
        match map.get(&name) {
            Some(existing) => {
                let previous = existing.revision;
                map.insert(name, RegisteredHandler { handler, revision });
                Ok(HandlerRegistrationOutcome::Revised { previous, revision })
            }
            None => {
                map.insert(name, RegisteredHandler { handler, revision });
                Ok(HandlerRegistrationOutcome::Registered { revision })
            }
        }
    }

    fn next_revision(&self) -> HookHandlerRevision {
        HookHandlerRevision(self.revision.fetch_add(1, Ordering::SeqCst))
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn read_stream_limited<R>(
        mut stream: R,
        byte_limit: usize,
        stream_name: &str,
    ) -> Result<Vec<u8>, String>
    where
        R: AsyncRead + Unpin,
    {
        let mut out = Vec::with_capacity(byte_limit.min(8 * 1024));
        let mut chunk = [0u8; 8 * 1024];

        loop {
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|err| format!("failed reading {stream_name}: {err}"))?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..read]);
            if out.len() > byte_limit {
                return Err(format!(
                    "{} exceeds max size: {} > {}",
                    stream_name,
                    out.len(),
                    byte_limit
                ));
            }
        }

        Ok(out)
    }

    /// Resolve the effective entries and their typed adapters for a run.
    ///
    /// Without overrides this borrows the construction-time base entries and
    /// adapter map. With overrides it re-layers the entries, re-validates
    /// registry uniqueness (row #280), and re-resolves typed adapters
    /// (row #231) once at this config-layering boundary so the execution path
    /// never re-parses adapter JSON.
    fn effective_entries(
        &self,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<ResolvedEntries<'_>, HookEngineError> {
        if let Some(reason) = &self.base_validation_error {
            return Err(HookEngineError::InvalidConfiguration(reason.clone()));
        }

        if let Some(overrides) = overrides {
            if overrides.disable.is_empty() && overrides.entries.is_empty() {
                return Ok(ResolvedEntries::base(self));
            }

            let mut entries = Vec::with_capacity(self.base_entries.len() + overrides.entries.len());
            if overrides.disable.is_empty() {
                entries.extend(self.base_entries.iter().cloned());
            } else {
                let disabled: HashSet<HookId> = overrides.disable.iter().cloned().collect();
                entries.extend(
                    self.base_entries
                        .iter()
                        .filter(|entry| !disabled.contains(&entry.id))
                        .cloned(),
                );
            }
            entries.extend(overrides.entries.clone());

            Self::resolve_base(&entries)?;
            let adapters = Self::resolve_adapters(&entries);

            return Ok(ResolvedEntries::owned(entries, adapters));
        }

        Ok(ResolvedEntries::base(self))
    }

    /// Reap background hook tasks that have already finished.
    ///
    /// Non-blocking `try_join_next` (not `join_next().await`) deliberately:
    /// it removes completed tasks without blocking on still-running ones, so
    /// it cannot deadlock against a background hook that is itself awaiting
    /// this same engine. Called before dispatching new background work so the
    /// `JoinSet` does not accumulate completed task handles. This is not a
    /// per-session cancellation or child-exit guarantee.
    #[cfg(not(target_arch = "wasm32"))]
    async fn reap_finished_background_tasks(&self) {
        let mut set = self.inflight_background.lock().await;
        while set.try_join_next().is_some() {}
    }

    fn validate_entry(entry: &HookEntryConfig) -> Result<(), HookEngineError> {
        if entry.id.0.trim().is_empty() {
            return Err(HookEngineError::InvalidConfiguration(
                "hook id cannot be empty".to_string(),
            ));
        }

        if entry.mode == HookExecutionMode::Background
            && entry.capability != HookCapability::Observe
        {
            return Err(HookEngineError::InvalidConfiguration(format!(
                "background hooks must be observe-only: {}",
                entry.id
            )));
        }

        if entry.point.is_observe_only() && entry.capability != HookCapability::Observe {
            return Err(HookEngineError::InvalidConfiguration(format!(
                "post-commit hook points are observe-only: {}",
                entry.id
            )));
        }

        Ok(())
    }

    async fn execute_one(
        &self,
        entry: HookEntryConfig,
        adapter: HookAdapterConfig,
        registration_index: usize,
        invocation: HookInvocation,
    ) -> Result<HookOutcome, HookEngineError> {
        let start = meerkat_core::time_compat::Instant::now();
        let timeout_ms = self.policy.timeout_ms_for(&entry);

        let mut outcome = HookOutcome {
            hook_id: entry.id.clone(),
            point: entry.point,
            priority: entry.priority,
            registration_index,
            decision: None,
            failure_reason: None,
            duration_ms: None,
        };

        let response = match &adapter {
            #[cfg(not(target_arch = "wasm32"))]
            HookAdapterConfig::Command(cfg) => {
                self.invoke_command_runtime(&entry, cfg, invocation.clone(), timeout_ms)
                    .await?
            }
            #[cfg(target_arch = "wasm32")]
            HookAdapterConfig::Command(cfg) => {
                self.invoke_command_runtime(&entry, cfg, invocation.clone(), timeout_ms)
                    .await?
            }
            _ => {
                let runtime_result = timeout(
                    Duration::from_millis(timeout_ms),
                    self.invoke_runtime(&entry, &adapter, invocation.clone()),
                )
                .await;

                match runtime_result {
                    Err(_) => {
                        return Err(HookEngineError::Timeout {
                            hook_id: entry.id.clone(),
                            timeout_ms,
                        });
                    }
                    Ok(response) => response?,
                }
            }
        };
        outcome.decision = runtime_decision_with_configured_hook_id(response.decision, &entry.id);

        if entry.mode == HookExecutionMode::Background {
            outcome.decision = None;
        }

        outcome.duration_ms = Some(start.elapsed().as_millis() as u64);
        Ok(outcome)
    }

    async fn invoke_runtime(
        &self,
        entry: &HookEntryConfig,
        adapter: &HookAdapterConfig,
        invocation: HookInvocation,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        // The adapter was parsed once at the config-layering boundary; the
        // execution path reads the typed variant directly (no
        // `serde_json::from_value` here — remediation row #231).
        match adapter {
            HookAdapterConfig::InProcess(cfg) => {
                let handler = {
                    let handlers = self.in_process_handlers.read().map_err(|err| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("in-process handler lock poisoned: {err}"),
                        }
                    })?;
                    handlers
                        .get(&cfg.handler)
                        .map(|h| h.handler.clone())
                        .ok_or_else(|| HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("in-process handler '{}' not registered", cfg.handler),
                        })?
                };
                (handler)(invocation)
                    .await
                    .map_err(|reason| HookEngineError::ExecutionFailed {
                        hook_id: entry.id.clone(),
                        reason,
                    })
            }
            HookAdapterConfig::Command(cfg) => Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!(
                    "command hook '{}' was routed outside the subprocess owner",
                    cfg.command
                ),
            }),
            HookAdapterConfig::Http(cfg) => {
                self.invoke_http_runtime(entry, &cfg.url, &cfg.method, &cfg.headers, invocation)
                    .await
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn invoke_command_runtime(
        &self,
        entry: &HookEntryConfig,
        configuration: &CommandRuntimeConfig,
        invocation: HookInvocation,
        timeout_ms: u64,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        let payload =
            serde_json::to_vec(&invocation).map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed to encode invocation payload: {err}"),
            })?;

        if payload.len() > self.policy.payload_max_bytes() {
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!(
                    "hook payload exceeds max size: {} > {}",
                    payload.len(),
                    self.policy.payload_max_bytes()
                ),
            });
        }

        let mut child = match self.process_custody.as_ref() {
            Some(custody) => custody
                .spawn(&entry.id, invocation.run_id.as_ref(), configuration)
                .await
                .map_err(|reason| HookEngineError::LaunchRefused {
                    hook_id: entry.id.clone(),
                    reason,
                })?,
            None => {
                // Absent host configuration preserves legacy trusted-host
                // execution. A Required factory always installs an adapter.
                let mut command = Command::new(&configuration.command);
                command
                    .args(&configuration.args)
                    .envs(&configuration.env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true);
                #[cfg(unix)]
                command.process_group(0);
                let child: ProcessChild = command
                    .spawn()
                    .map_err(|_| HookEngineError::LaunchRefused {
                        hook_id: entry.id.clone(),
                        reason: HookFailureReason::execution_failed("command hook spawn failed"),
                    })?
                    .into();
                #[cfg(unix)]
                observe_command_hook_process_group(&child);
                child
            }
        };

        let mut stdin = match child.take_stdin() {
            Some(stdin) => stdin,
            None => {
                terminate_child_process_group(&mut child).await;
                return Err(HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: "command hook stdin pipe unavailable".to_string(),
                });
            }
        };
        let stdout = match child.take_stdout() {
            Some(stdout) => stdout,
            None => {
                terminate_child_process_group(&mut child).await;
                return Err(HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: "command hook stdout pipe unavailable".to_string(),
                });
            }
        };
        let stderr = match child.take_stderr() {
            Some(stderr) => stderr,
            None => {
                terminate_child_process_group(&mut child).await;
                return Err(HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: "command hook stderr pipe unavailable".to_string(),
                });
            }
        };

        let max_output_bytes = self.policy.payload_max_bytes();
        let mut stdout_task = tokio::spawn(Self::read_stream_limited(
            stdout,
            max_output_bytes,
            "command stdout",
        ));
        let mut stderr_task = tokio::spawn(Self::read_stream_limited(
            stderr,
            max_output_bytes,
            "command stderr",
        ));

        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let Some(write_timeout) = remaining_until(deadline) else {
            terminate_child_process_group(&mut child).await;
            abort_command_reader_tasks(&stdout_task, &stderr_task);
            return Err(HookEngineError::Timeout {
                hook_id: entry.id.clone(),
                timeout_ms,
            });
        };
        tokio::select! {
            write_result = stdin.write_all(&payload) => {
                if let Err(err) = write_result {
                    terminate_child_process_group(&mut child).await;
                    let _ = stdout_task.await;
                    let _ = stderr_task.await;
                    return Err(HookEngineError::ExecutionFailed {
                        hook_id: entry.id.clone(),
                        reason: format!("failed to write command hook stdin: {err}"),
                    });
                }
            }
            () = tokio::time::sleep(write_timeout) => {
                terminate_child_process_group(&mut child).await;
                abort_command_reader_tasks(&stdout_task, &stderr_task);
                return Err(HookEngineError::Timeout {
                    hook_id: entry.id.clone(),
                    timeout_ms,
                });
            }
        }
        drop(stdin);

        let Some(wait_timeout) = remaining_until(deadline) else {
            terminate_child_process_group(&mut child).await;
            abort_command_reader_tasks(&stdout_task, &stderr_task);
            return Err(HookEngineError::Timeout {
                hook_id: entry.id.clone(),
                timeout_ms,
            });
        };
        let status = tokio::select! {
            status = child.wait() => status.map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed waiting for command hook: {err}"),
            })?,
            () = tokio::time::sleep(wait_timeout) => {
                terminate_child_process_group(&mut child).await;
                abort_command_reader_tasks(&stdout_task, &stderr_task);
                return Err(HookEngineError::Timeout {
                    hook_id: entry.id.clone(),
                    timeout_ms,
                });
            }
        };

        let Some(stdout_timeout) = remaining_until(deadline) else {
            abort_command_reader_tasks(&stdout_task, &stderr_task);
            return Err(HookEngineError::Timeout {
                hook_id: entry.id.clone(),
                timeout_ms,
            });
        };
        let stdout = tokio::select! {
            stdout = &mut stdout_task => stdout,
            () = tokio::time::sleep(stdout_timeout) => {
                abort_command_reader_tasks(&stdout_task, &stderr_task);
                return Err(HookEngineError::Timeout {
                    hook_id: entry.id.clone(),
                    timeout_ms,
                });
            }
        }
        .map_err(|err| HookEngineError::ExecutionFailed {
            hook_id: entry.id.clone(),
            reason: format!("command stdout task join failed: {err}"),
        })?
        .map_err(|reason| HookEngineError::ExecutionFailed {
            hook_id: entry.id.clone(),
            reason,
        })?;
        let Some(stderr_timeout) = remaining_until(deadline) else {
            stderr_task.abort();
            return Err(HookEngineError::Timeout {
                hook_id: entry.id.clone(),
                timeout_ms,
            });
        };
        let stderr = tokio::select! {
            stderr = &mut stderr_task => stderr,
            () = tokio::time::sleep(stderr_timeout) => {
                stderr_task.abort();
                return Err(HookEngineError::Timeout {
                    hook_id: entry.id.clone(),
                    timeout_ms,
                });
            }
        }
        .map_err(|err| HookEngineError::ExecutionFailed {
            hook_id: entry.id.clone(),
            reason: format!("command stderr task join failed: {err}"),
        })?
        .map_err(|reason| HookEngineError::ExecutionFailed {
            hook_id: entry.id.clone(),
            reason,
        })?;

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr).to_string();
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("command hook exited with {status}: {stderr}"),
            });
        }

        serde_json::from_slice::<RuntimeHookResponse>(&stdout).map_err(|err| {
            HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("invalid command hook response: {err}"),
            }
        })
    }

    #[cfg(target_arch = "wasm32")]
    async fn invoke_command_runtime(
        &self,
        entry: &HookEntryConfig,
        _configuration: &CommandRuntimeConfig,
        _invocation: HookInvocation,
        _timeout_ms: u64,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        Err(HookEngineError::ExecutionFailed {
            hook_id: entry.id.clone(),
            reason: "command hooks are not supported on wasm32".to_string(),
        })
    }

    async fn invoke_http_runtime(
        &self,
        entry: &HookEntryConfig,
        url: &str,
        method: &str,
        headers: &HashMap<String, String>,
        invocation: HookInvocation,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        let payload =
            serde_json::to_vec(&invocation).map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed to encode invocation payload: {err}"),
            })?;

        if payload.len() > self.policy.payload_max_bytes() {
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!(
                    "hook payload exceeds max size: {} > {}",
                    payload.len(),
                    self.policy.payload_max_bytes()
                ),
            });
        }

        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|err| {
            HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("invalid HTTP method '{method}': {err}"),
            }
        })?;

        let http_client = self.http_client.get_or_init(reqwest::Client::new);
        let mut req = http_client
            .request(method, url)
            .header("content-type", "application/json")
            .body(payload);

        for (name, value) in headers {
            req = req.header(name, value);
        }

        let response = req
            .send()
            .await
            .map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("HTTP hook request failed: {err}"),
            })?;

        let status = response.status();
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed reading HTTP hook response body: {err}"),
            })?;
            bytes.extend_from_slice(&chunk);
            if bytes.len() > self.policy.payload_max_bytes() {
                return Err(HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: format!(
                        "HTTP hook response exceeds max size: {} > {}",
                        bytes.len(),
                        self.policy.payload_max_bytes()
                    ),
                });
            }
        }

        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("HTTP hook returned {status}: {body}"),
            });
        }

        serde_json::from_slice::<RuntimeHookResponse>(&bytes).map_err(|err| {
            HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("invalid HTTP hook response: {err}"),
            }
        })
    }
}

fn runtime_decision_with_configured_hook_id(
    decision: Option<HookDecision>,
    hook_id: &HookId,
) -> Option<HookDecision> {
    match decision {
        Some(HookDecision::Deny {
            reason_code,
            message,
            payload,
            ..
        }) => Some(HookDecision::Deny {
            hook_id: hook_id.clone(),
            reason_code,
            message,
            payload,
        }),
        decision => decision,
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl HookEngine for DefaultHookEngine {
    fn background_completion_source_id(&self) -> Option<HookBackgroundSourceId> {
        Some(self.background_dispatch_ledger.source_id)
    }

    fn take_background_completions(
        &self,
        session_id: &meerkat_core::SessionId,
        run_id: Option<&meerkat_core::RunId>,
        limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        self.background_dispatch_ledger
            .take_ready(session_id, run_id, limit)
    }

    fn background_session_status(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<HookBackgroundSessionStatus> {
        self.background_dispatch_ledger.session_status(session_id)
    }

    fn take_session_background_completions(
        &self,
        session_id: &meerkat_core::SessionId,
        limit: usize,
    ) -> Vec<HookBackgroundCompletion> {
        self.background_dispatch_ledger
            .take_session_ready(session_id, limit)
    }

    async fn wait_for_session_background_completion(&self, session_id: &meerkat_core::SessionId) {
        self.background_dispatch_ledger
            .wait_for_session_completion(session_id)
            .await;
    }

    fn matching_hooks(
        &self,
        invocation: &HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<Vec<HookId>, HookEngineError> {
        let entries = self.effective_entries(overrides)?;
        // Registry uniqueness is enforced at resolution, so each id is
        // canonical here — one match per id (remediation row #280).
        Ok(entries
            .entries()
            .iter()
            .filter(|entry| entry.enabled && entry.point == invocation.point)
            .map(|entry| entry.id.clone())
            .collect())
    }

    async fn execute(
        &self,
        invocation: HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        let _invocation_scope = self
            .background_dispatch_ledger
            .enter_scope(&invocation.session_id);
        let mut foreground: Vec<(usize, HookEntryConfig, HookAdapterConfig)> = Vec::new();
        let mut background: Vec<(usize, HookEntryConfig, HookAdapterConfig)> = Vec::new();
        let resolved = self.effective_entries(overrides)?;
        for (registration_index, entry) in resolved
            .entries()
            .iter()
            .filter(|entry| entry.enabled && entry.point == invocation.point)
            .cloned()
            .enumerate()
        {
            // Every resolved entry has a typed adapter (resolved at the config
            // boundary). A missing adapter is a pre-execution config-resolution
            // invariant break (the hook never starts), so fail closed with an
            // InvalidConfiguration error rather than ExecutionFailed. This keeps
            // the rule "a HookEngineError carrying a hook_id() means that hook
            // actually began executing" — Timeout and adapter-runtime
            // ExecutionFailed both originate from execute_one after the hook
            // started — which Agent::execute_hooks relies on to emit a
            // HookStarted before the terminal HookFailed on the error path.
            let adapter = resolved.adapter(&entry.id).cloned().ok_or_else(|| {
                HookEngineError::InvalidConfiguration(format!(
                    "no resolved adapter for hook id '{}'",
                    entry.id
                ))
            })?;
            if entry.mode == HookExecutionMode::Background {
                background.push((registration_index, entry, adapter));
            } else {
                foreground.push((registration_index, entry, adapter));
            }
        }
        drop(resolved);

        if foreground.is_empty() && background.is_empty() {
            return Ok(HookExecutionReport::empty());
        }

        foreground.sort_by(|(a_idx, a_entry, _), (b_idx, b_entry, _)| {
            a_entry
                .priority
                .cmp(&b_entry.priority)
                .then_with(|| a_idx.cmp(b_idx))
        });

        let mut merged = HookExecutionReport::empty();
        for (registration_index, entry, adapter) in foreground {
            let capability = entry.capability;
            let hook_id = entry.id.clone();
            let point = entry.point;
            let outcome = match self
                .execute_one(entry, adapter, registration_index, invocation.clone())
                .await
            {
                Ok(outcome) => outcome,
                Err(HookEngineError::LaunchRefused {
                    reason: meerkat_core::HookFailureReason::ConfinementRefused { refusal },
                    ..
                }) if capability == HookCapability::Observe => {
                    // An optional observer did not enter. Retain that fact and
                    // still run every later mandatory hook in priority order.
                    merged.launch_refusals.push(HookLaunchRefusal {
                        hook_id,
                        point,
                        refusal,
                    });
                    continue;
                }
                Err(error) => {
                    return Err(if merged.launch_refusals.is_empty() {
                        error
                    } else {
                        HookEngineError::WithReport {
                            report: Box::new(merged),
                            error: Box::new(error),
                        }
                    });
                }
            };
            // Only an entered hook earns a start/outcome. The error projection
            // owns the later failing hook and must not emit its start twice.
            merged.started.push(hook_id);

            if let Some(decision) = outcome.decision.clone() {
                match &decision {
                    HookDecision::Deny { .. } => {
                        merged.decision = Some(decision);
                    }
                    HookDecision::Allow => {
                        if !matches!(merged.decision, Some(HookDecision::Deny { .. })) {
                            merged.decision = Some(HookDecision::Allow);
                        }
                    }
                }
            }
            let should_stop = matches!(outcome.decision, Some(HookDecision::Deny { .. }));
            merged.outcomes.push(outcome);
            if should_stop {
                break;
            }
        }

        if !matches!(merged.decision, Some(HookDecision::Deny { .. })) {
            #[cfg(not(target_arch = "wasm32"))]
            if !background.is_empty() {
                self.reap_finished_background_tasks().await;
            }
            for (registration_index, entry, adapter) in background {
                let skip = |reason| HookBackgroundSkip {
                    hook_id: entry.id.clone(),
                    point: entry.point,
                    reason,
                };
                if !background_attribution_fits(&entry.id, &invocation) {
                    merged
                        .background_skips
                        .push(skip(HookBackgroundSkipReason::AttributionTooLarge));
                    continue;
                }
                let (ordinal, capacity) = match self.background_dispatch_ledger.reserve() {
                    Ok(reservation) => reservation,
                    Err(reason) => {
                        merged.background_skips.push(skip(reason));
                        continue;
                    }
                };
                let permit = match self.background_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        merged
                            .background_skips
                            .push(skip(HookBackgroundSkipReason::ConcurrencyFull));
                        continue;
                    }
                };
                let attribution =
                    HookBackgroundAttribution::from_invocation(entry.id.clone(), &invocation);
                // Scheduling is not target entry. Completion is retained before
                // the concurrency permit is released and is never merged as a decision.
                let engine = self.clone();
                let invocation_cloned = invocation.clone();
                let scope = self
                    .background_dispatch_ledger
                    .enter_scope(&invocation.session_id);
                let task = async move {
                    let _scope = scope;
                    let _permit = permit;
                    let mut diagnostic_truncated = false;
                    let result = match engine
                        .execute_one(entry, adapter, registration_index, invocation_cloned)
                        .await
                    {
                        Ok(mut outcome) => {
                            if let Some(reason) = &mut outcome.failure_reason {
                                diagnostic_truncated = bound_background_reason(reason);
                            }
                            HookBackgroundResult::Completed(outcome)
                        }
                        Err(error) => {
                            let mut reason = HookFailureReason::from_engine_error(&error);
                            diagnostic_truncated = bound_background_reason(&mut reason);
                            if matches!(error, HookEngineError::LaunchRefused { .. }) {
                                HookBackgroundResult::LaunchRefused(reason)
                            } else {
                                HookBackgroundResult::Failed(reason)
                            }
                        }
                    };
                    engine
                        .background_dispatch_ledger
                        .record(
                            HookBackgroundCompletion {
                                ordinal,
                                attribution,
                                result,
                                diagnostic_truncated,
                            },
                            capacity,
                        )
                        .await;
                };
                #[cfg(not(target_arch = "wasm32"))]
                self.inflight_background.lock().await.spawn(task);
                #[cfg(target_arch = "wasm32")]
                tokio::spawn(task);
            }
        }

        Ok(merged)
    }

    async fn execute_post_commit(
        &self,
        invocation: HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        debug_assert!(invocation.point.is_observe_only());
        let resolved = self.effective_entries(overrides)?;
        let mut entries: Vec<(usize, HookEntryConfig, HookAdapterConfig)> = resolved
            .entries()
            .iter()
            .filter(|entry| entry.enabled && entry.point == invocation.point)
            .cloned()
            .enumerate()
            .map(|(registration_index, entry)| {
                let adapter = resolved.adapter(&entry.id).cloned().ok_or_else(|| {
                    HookEngineError::InvalidConfiguration(format!(
                        "no resolved adapter for hook id '{}'",
                        entry.id
                    ))
                })?;
                Ok((registration_index, entry, adapter))
            })
            .collect::<Result<_, HookEngineError>>()?;
        drop(resolved);
        entries.sort_by(|(a_idx, a_entry, _), (b_idx, b_entry, _)| {
            a_entry
                .priority
                .cmp(&b_entry.priority)
                .then_with(|| a_idx.cmp(b_idx))
        });

        let mut report = HookExecutionReport::empty();
        for (registration_index, entry, adapter) in entries {
            let hook_id = entry.id.clone();
            let point = entry.point;
            let capability = entry.capability;
            let outcome = match self
                .execute_one(entry, adapter, registration_index, invocation.clone())
                .await
            {
                Ok(outcome) => outcome,
                Err(HookEngineError::LaunchRefused {
                    reason: meerkat_core::HookFailureReason::ConfinementRefused { refusal },
                    ..
                }) if capability == HookCapability::Observe => {
                    // The observed fact has committed. A refused observer did
                    // not enter and must not suppress later observers.
                    report.launch_refusals.push(HookLaunchRefusal {
                        hook_id,
                        point,
                        refusal,
                    });
                    continue;
                }
                Err(error) => {
                    return Err(if report.launch_refusals.is_empty() {
                        error
                    } else {
                        HookEngineError::WithReport {
                            report: Box::new(report),
                            error: Box::new(error),
                        }
                    });
                }
            };
            report.started.push(hook_id);
            if let Some(decision) = outcome.decision.clone() {
                report.decision = Some(decision);
            }
            report.outcomes.push(outcome);
        }
        Ok(report)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use meerkat_core::config::HookRuntimeKind;
    use meerkat_core::{
        ContentInput, HookLlmRequest, HookPoint, HookReasonCode, RunInput, SessionId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn static_handler(response: RuntimeHookResponse) -> InProcessHookHandler {
        Arc::new(move |_invocation| {
            let response = response.clone();
            Box::pin(async move { Ok(response) })
        })
    }

    fn delayed_handler(delay_ms: u64, response: RuntimeHookResponse) -> InProcessHookHandler {
        Arc::new(move |_invocation| {
            let response = response.clone();
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                Ok(response)
            })
        })
    }

    fn runtime_in_process(name: &str) -> HookAdapterConfig {
        HookAdapterConfig::in_process(name)
    }

    #[test]
    fn in_process_handler_id_preserves_string_wire_shape() {
        let id: InProcessHookHandlerId = serde_json::from_value(serde_json::json!("handler-a"))
            .expect("handler id should deserialize from string");

        assert_eq!(id.as_str(), "handler-a");
        assert_eq!(
            serde_json::to_value(&id).expect("handler id should serialize"),
            serde_json::json!("handler-a")
        );
    }

    #[tokio::test]
    async fn deterministic_merge_by_priority_then_registration() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("hook-a"),
                point: HookPoint::PreLlmRequest,
                priority: 10,
                runtime: runtime_in_process("a"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("hook-b"),
                point: HookPoint::PreLlmRequest,
                priority: 5,
                runtime: runtime_in_process("b"),
                capability: HookCapability::Guardrail,
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "a",
                static_handler(RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("in-process handler registration must succeed");
        engine
            .register_in_process_handler(
                "b",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::Allow),
                }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreLlmRequest,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: Some(HookLlmRequest {
                        max_tokens: 256,
                        temperature: None,
                        provider_params: None,
                        message_count: 1,
                    }),
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        let hook_ids: Vec<_> = report
            .outcomes
            .iter()
            .map(|outcome| &outcome.hook_id)
            .collect();
        assert_eq!(
            hook_ids,
            vec![&HookId::new("hook-b"), &HookId::new("hook-a")]
        );
        assert!(matches!(report.decision, Some(HookDecision::Allow)));
    }

    #[tokio::test]
    async fn background_hooks_are_observation_only_and_publish_no_patches()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = DefaultHookEngine::new(HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("hook-post-bg"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("post-bg"),
                ..Default::default()
            }],
            ..Default::default()
        });
        engine
            .register_in_process_handler(
                "post-bg",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::deny(
                        HookId::new("ignored-background-decision"),
                        meerkat_core::HookReasonCode::PolicyViolation,
                        "private-decision-canary",
                        Some(serde_json::json!({"private": "x".repeat(BACKGROUND_RECORD_BYTES)})),
                    )),
                }),
            )
            .await?;
        let session_id = SessionId::new();
        let report = engine
            .execute(
                invocation(HookPoint::PostToolExecution, session_id.clone()),
                None,
            )
            .await?;
        assert!(report.started.is_empty());
        assert!(report.outcomes.is_empty());
        assert!(report.decision.is_none());
        timeout(
            Duration::from_secs(5),
            engine
                .background_dispatch_ledger()
                .wait_for_completion(&session_id, None),
        )
        .await?;
        let completions = engine.take_background_completions(&session_id, None, 8);
        assert_eq!(completions.len(), 1);
        assert!(
            matches!(&completions[0].result, HookBackgroundResult::Completed(outcome)
            if outcome.decision.is_none() && outcome.failure_reason.is_none())
        );
        assert!(!serde_json::to_string(&completions)?.contains("private-decision-canary"));
        timeout(Duration::from_secs(5), async {
            while let Some(result) = engine.inflight_background.lock().await.join_next().await {
                result?;
            }
            Ok::<(), tokio::task::JoinError>(())
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn background_post_hook_task_is_tracked_and_reaped_not_leaked()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = DefaultHookEngine::new(HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("tracked-post-bg"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("tracked-post-bg"),
                ..Default::default()
            }],
            ..Default::default()
        });
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let release = Arc::new(Mutex::new(Some(release_rx)));
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        engine
            .register_in_process_handler(
                "tracked-post-bg",
                Arc::new(move |_| {
                    let release = Arc::clone(&release);
                    let entered = entered_tx.clone();
                    Box::pin(async move {
                        let receiver =
                            release.lock().await.take().ok_or("release already taken")?;
                        entered.send(()).map_err(|error| error.to_string())?;
                        receiver.await.map_err(|error| error.to_string())?;
                        Ok(RuntimeHookResponse { decision: None })
                    })
                }),
            )
            .await?;
        let session_id = SessionId::new();
        let report = engine
            .execute(
                invocation(HookPoint::PostToolExecution, session_id.clone()),
                None,
            )
            .await?;
        timeout(Duration::from_secs(5), entered_rx.recv())
            .await?
            .ok_or("missing entry")?;
        assert!(report.started.is_empty());
        assert!(report.outcomes.is_empty());
        assert_eq!(engine.inflight_background.lock().await.len(), 1);
        assert!(
            engine
                .take_background_completions(&session_id, None, 8)
                .is_empty()
        );
        release_tx.send(()).map_err(|()| "release receiver gone")?;
        timeout(Duration::from_secs(5), async {
            while !engine.inflight_background.lock().await.is_empty() {
                engine.reap_finished_background_tasks().await;
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(
            engine
                .background_dispatch_ledger()
                .completion_snapshot()
                .await
                .len(),
            1
        );
        // Actual completion and reaping, not a per-session shutdown or process-exit proof.
        Ok(())
    }

    #[tokio::test]
    async fn legacy_semantic_patch_payload_from_command_runtime_fails_closed() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("legacy-patch-command"),
            point: HookPoint::PreLlmRequest,
            capability: HookCapability::Guardrail,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::Command,
                Some(serde_json::json!({
                    "command": "sh",
                    "args": [
                        "-c",
                        "cat >/dev/null; printf '%s' '{\"patches\":[{\"patch_type\":\"llm_request\",\"max_tokens\":1}]}'"
                    ],
                    "env": {}
                })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreLlmRequest,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: Some(HookLlmRequest {
                        max_tokens: 256,
                        temperature: None,
                        provider_params: None,
                        message_count: 1,
                    }),
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .expect_err("legacy semantic patch payloads must fail closed");

        assert!(matches!(
            err,
            HookEngineError::ExecutionFailed { ref hook_id, .. }
                if hook_id == &HookId::new("legacy-patch-command")
        ));
    }

    #[tokio::test]
    async fn deterministic_merge_ignores_completion_order() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("slow-low-priority"),
                point: HookPoint::PreLlmRequest,
                priority: 100,
                runtime: runtime_in_process("slow"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("fast-high-priority"),
                point: HookPoint::PreLlmRequest,
                priority: 1,
                runtime: runtime_in_process("fast"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "slow",
                delayed_handler(100, RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("in-process handler registration must succeed");
        engine
            .register_in_process_handler(
                "fast",
                delayed_handler(1, RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreLlmRequest,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: Some(HookLlmRequest {
                        max_tokens: 256,
                        temperature: None,
                        provider_params: None,
                        message_count: 1,
                    }),
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        let hook_ids: Vec<_> = report
            .outcomes
            .iter()
            .map(|outcome| &outcome.hook_id)
            .collect();
        assert_eq!(
            hook_ids,
            vec![
                &HookId::new("fast-high-priority"),
                &HookId::new("slow-low-priority")
            ]
        );
    }

    #[tokio::test]
    async fn observe_runtime_error_returns_typed_engine_failure() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("observe-missing-handler"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("missing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .expect_err("hook runtime errors must fail closed through typed engine errors");

        assert!(matches!(
            err,
            HookEngineError::ExecutionFailed { ref hook_id, .. }
                if hook_id == &HookId::new("observe-missing-handler")
        ));
    }

    #[tokio::test]
    async fn guardrail_runtime_error_returns_typed_engine_failure() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("guardrail-missing-handler"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Guardrail,
            runtime: runtime_in_process("missing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .expect_err("hook runtime errors must not be converted into hook-local denials");

        assert!(matches!(
            err,
            HookEngineError::ExecutionFailed { ref hook_id, .. }
                if hook_id == &HookId::new("guardrail-missing-handler")
        ));
    }

    #[tokio::test]
    async fn deny_short_circuits_lower_priority_hooks() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("guardrail-deny"),
                point: HookPoint::PreToolExecution,
                priority: 1,
                capability: HookCapability::Guardrail,
                runtime: runtime_in_process("guardrail"),
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("observer-late"),
                point: HookPoint::PreToolExecution,
                priority: 100,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("observer"),
                ..Default::default()
            },
        ];

        let observed_runs = Arc::new(AtomicUsize::new(0));
        let observer_runs = observed_runs.clone();

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "guardrail",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::deny(
                        HookId::new("guardrail-deny"),
                        HookReasonCode::PolicyViolation,
                        "deny",
                        None,
                    )),
                }),
            )
            .await
            .expect("in-process handler registration must succeed");
        engine
            .register_in_process_handler(
                "observer",
                Arc::new(move |_invocation| {
                    let observer_runs = observer_runs.clone();
                    Box::pin(async move {
                        observer_runs.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(RuntimeHookResponse {
                            decision: Some(HookDecision::Allow),
                        })
                    })
                }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(matches!(report.decision, Some(HookDecision::Deny { .. })));
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(observed_runs.load(AtomicOrdering::SeqCst), 0);
        // A foreground deny short-circuits the loop, so only the hook that
        // actually ran is reported as started — the skipped lower-priority
        // observer must not appear (it never began execution).
        assert_eq!(report.started, vec![HookId::new("guardrail-deny")]);
    }

    #[tokio::test]
    async fn deny_decision_uses_configured_hook_id_over_runtime_payload() {
        let configured_hook_id = HookId::new("configured-guardrail");
        let returned_hook_id = HookId::new("runtime-payload-identity");

        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: configured_hook_id.clone(),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Guardrail,
            runtime: runtime_in_process("guardrail"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "guardrail",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::deny(
                        returned_hook_id,
                        HookReasonCode::PolicyViolation,
                        "deny",
                        None,
                    )),
                }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        match report.decision {
            Some(HookDecision::Deny { hook_id, .. }) => {
                assert_eq!(hook_id, configured_hook_id);
            }
            decision => panic!("expected deny decision, got {decision:?}"),
        }

        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.outcomes[0].hook_id, configured_hook_id);
        match &report.outcomes[0].decision {
            Some(HookDecision::Deny { hook_id, .. }) => {
                assert_eq!(hook_id, &configured_hook_id);
            }
            decision => panic!("expected outcome deny decision, got {decision:?}"),
        }
    }

    #[tokio::test]
    async fn background_guardrail_is_rejected() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("run-start-bg-guardrail"),
            point: HookPoint::RunStarted,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Guardrail,
            runtime: runtime_in_process("guardrail"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::RunStarted,
                    session_id: SessionId::new(),
                    turn_number: Some(0),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .expect_err("invalid background guardrail hook must be rejected");

        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
    }

    #[tokio::test]
    async fn post_background_guardrail_is_rejected_before_deny_can_be_erased() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("post-bg-guardrail"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Guardrail,
            runtime: runtime_in_process("guardrail"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PostToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .expect_err("invalid post background guardrail hook must be rejected");

        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
    }

    #[tokio::test]
    async fn post_commit_point_rejects_foreground_guardrail_capability() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("accepted-input-guardrail"),
            point: HookPoint::RuntimeInputAccepted,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Guardrail,
            runtime: runtime_in_process("guardrail"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                invocation(HookPoint::RuntimeInputAccepted, SessionId::new()),
                None,
            )
            .await
            .expect_err("post-commit hook points must reject guardrail capability");

        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
    }

    #[cfg(unix)]
    static OBSERVED_HOOK_GROUP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    #[cfg(unix)]
    fn record_hook_group(pid: i32) {
        OBSERVED_HOOK_GROUP.store(pid, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_hook_process_groups_are_reported_to_the_observer() {
        assert!(set_command_hook_process_group_observer(record_hook_group));
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("observed-command-hook"),
            point: HookPoint::PreToolExecution,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::Command,
                Some(serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "cat >/dev/null; printf '{}'"],
                    "env": {}
                })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }];

        let report = DefaultHookEngine::new(config)
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(report.outcomes[0].failure_reason.is_none());
        assert!(
            OBSERVED_HOOK_GROUP.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "the spawned hook group leader must be reported"
        );
        assert!(
            !set_command_hook_process_group_observer(record_hook_group),
            "the first installed observer wins"
        );
    }

    #[tokio::test]
    async fn command_runtime_hook_executes() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("command-hook"),
            point: HookPoint::PreToolExecution,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::Command,
                Some(serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "cat >/dev/null; printf '{}'"],
                    "env": {}
                })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.outcomes.len(), 1);
        assert!(
            report.outcomes[0].failure_reason.is_none(),
            "command runtime error: {:?}",
            report.outcomes[0].failure_reason
        );
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn command_runtime_timeout_covers_stdin_write() {
        let mut config = HooksConfig {
            payload_max_bytes: 1024 * 1024,
            ..Default::default()
        };
        config.entries = vec![HookEntryConfig {
            id: HookId::new("command-hook-stdin-timeout"),
            point: HookPoint::RunStarted,
            timeout_ms: Some(25),
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::Command,
                Some(serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "sleep 5"],
                    "env": {}
                })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let mut invocation = invocation(HookPoint::RunStarted, SessionId::new());
        invocation.prompt_input = Some(RunInput::from(ContentInput::Text("x".repeat(256 * 1024))));

        let err = engine
            .execute(invocation, None)
            .await
            .expect_err("non-reading command must time out while stdin write is pending");
        assert!(
            matches!(err, HookEngineError::Timeout { .. }),
            "expected timeout, got {err:?}"
        );
    }

    #[tokio::test]
    #[ignore = "integration-real: binds TCP port and makes real HTTP request"]
    async fn http_runtime_hook_executes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Bind a mock HTTP server. The listener is ready immediately after bind
        // (no sleep race needed).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Accept connections in a loop so retries/keep-alive don't break.
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0_u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let body = "{}";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        let mut config = HooksConfig::default();
        // Use an explicit generous timeout so this test doesn't flake under
        // parallel test load (the default 5 000 ms is borderline on CI).
        config.default_timeout_ms = 30_000;
        config.entries = vec![HookEntryConfig {
            id: HookId::new("http-hook"),
            point: HookPoint::PreToolExecution,
            runtime: HookAdapterConfig::from_kind_and_value(
                HookRuntimeKind::Http,
                Some(serde_json::json!({
                    "url": format!("http://{}/hook", addr),
                    "method": "POST",
                    "headers": {}
                })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let report = engine
            .execute(
                HookInvocation {
                    run_id: None,
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                    observation: None,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.outcomes.len(), 1);
        assert!(
            report.outcomes[0].failure_reason.is_none(),
            "http runtime error: {:?}",
            report.outcomes[0].failure_reason
        );
    }

    fn invocation(point: HookPoint, session_id: SessionId) -> HookInvocation {
        HookInvocation {
            run_id: None,
            point,
            session_id,
            turn_number: Some(1),
            prompt_input: None,
            error_report: None,
            error_class: None,
            llm_request: None,
            llm_response: None,
            tool_call: None,
            tool_result: None,
            observation: None,
        }
    }

    fn erroring_handler() -> InProcessHookHandler {
        Arc::new(move |_invocation| {
            Box::pin(async move { Result::<RuntimeHookResponse, String>::Err("boom".to_string()) })
        })
    }

    // Gate for remediation row #289: re-registering an already-registered
    // in-process handler id must be an observable typed revision event, not a
    // silent `HashMap::insert` overwrite of executable behavior.
    #[tokio::test]
    async fn reregistering_in_process_handler_id_is_observable_revision() {
        let engine = DefaultHookEngine::new(HooksConfig::default());

        let first = engine
            .register_in_process_handler(
                "shared-id",
                static_handler(RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("first registration must succeed");
        assert!(
            matches!(first, HandlerRegistrationOutcome::Registered { .. }),
            "fresh id must register, got {first:?}"
        );

        let second = engine
            .register_in_process_handler(
                "shared-id",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::Allow),
                }),
            )
            .await
            .expect("re-registration must succeed");

        match second {
            HandlerRegistrationOutcome::Revised { previous, revision } => {
                assert_eq!(
                    previous,
                    first.revision(),
                    "revision event must carry the prior revision"
                );
                assert_ne!(
                    revision,
                    first.revision(),
                    "re-registration must bump the handler revision (observable change)"
                );
            }
            other => panic!("re-registration must be a typed Revised event, got {other:?}"),
        }
        assert!(second.is_revision());
    }

    // Gate for remediation row #280 (part 1): two hook entries sharing an id
    // are rejected at validation, so identity is registry-owned not
    // list-ordinal.
    #[tokio::test]
    async fn duplicate_hook_ids_are_rejected_at_validation() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("dup"),
                point: HookPoint::PreToolExecution,
                runtime: runtime_in_process("a"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("dup"),
                point: HookPoint::PreToolExecution,
                runtime: runtime_in_process("b"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                invocation(HookPoint::PreToolExecution, SessionId::new()),
                None,
            )
            .await
            .expect_err("duplicate hook ids must be rejected at validation");
        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
    }

    // Gate for remediation row #280 (part 2): disabling a hook by id affects
    // exactly one canonical hook, and a hook is executed once per id.
    #[tokio::test]
    async fn disable_by_id_affects_one_canonical_hook_executed_once() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("keep"),
                point: HookPoint::PreToolExecution,
                runtime: runtime_in_process("keep"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("drop"),
                point: HookPoint::PreToolExecution,
                runtime: runtime_in_process("drop"),
                capability: HookCapability::Observe,
                ..Default::default()
            },
        ];

        let keep_runs = Arc::new(AtomicUsize::new(0));
        let keep_counter = keep_runs.clone();

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "keep",
                Arc::new(move |_invocation| {
                    let keep_counter = keep_counter.clone();
                    Box::pin(async move {
                        keep_counter.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(RuntimeHookResponse {
                            decision: Some(HookDecision::Allow),
                        })
                    })
                }),
            )
            .await
            .expect("register keep handler");
        engine
            .register_in_process_handler(
                "drop",
                static_handler(RuntimeHookResponse {
                    decision: Some(HookDecision::Allow),
                }),
            )
            .await
            .expect("register drop handler");

        let overrides = HookRunOverrides {
            entries: Vec::new(),
            disable: vec![HookId::new("drop")],
        };
        let report = engine
            .execute(
                invocation(HookPoint::PreToolExecution, SessionId::new()),
                Some(&overrides),
            )
            .await
            .expect("execute with disable override");

        let ids: Vec<_> = report
            .outcomes
            .iter()
            .map(|outcome| outcome.hook_id.clone())
            .collect();
        assert_eq!(
            ids,
            vec![HookId::new("keep")],
            "disabling 'drop' must leave exactly the one canonical 'keep' hook"
        );
        assert_eq!(
            keep_runs.load(AtomicOrdering::SeqCst),
            1,
            "the surviving canonical hook must execute exactly once"
        );
    }

    // Gate for remediation row #231 (part 1): a malformed command adapter
    // config is rejected at the config-deserialization boundary — the single
    // typed owner `HookAdapterConfig` is parsed once at config load, so a
    // missing required field fails closed there rather than being deferred to a
    // per-execution JSON re-parse.
    #[test]
    fn malformed_command_adapter_config_rejected_at_config_load() {
        // Missing the required `command` field for the command adapter.
        let err = serde_json::from_value::<HookAdapterConfig>(serde_json::json!({
            "type": "command",
            "args": ["x"]
        }))
        .expect_err("malformed command adapter must fail closed at deserialize");
        assert!(
            err.to_string().contains("command"),
            "unexpected error: {err}"
        );

        // The same failure surfaces when the whole entry is deserialized from a
        // config document (the real config-load path).
        let entry_err = serde_json::from_value::<HookEntryConfig>(serde_json::json!({
            "id": "bad-command",
            "point": "pre_tool_execution",
            "runtime": { "type": "command", "args": ["x"] }
        }))
        .expect_err("malformed command adapter entry must fail closed at deserialize");
        assert!(
            entry_err.to_string().contains("command"),
            "unexpected error: {entry_err}"
        );
    }

    // Gate for remediation row #231 (part 2): a malformed HTTP adapter config is
    // likewise rejected at the config-deserialization boundary.
    #[test]
    fn malformed_http_adapter_config_rejected_at_config_load() {
        // Missing the required `url` field for the HTTP adapter.
        let err = serde_json::from_value::<HookAdapterConfig>(serde_json::json!({
            "type": "http",
            "method": "POST"
        }))
        .expect_err("malformed http adapter must fail closed at deserialize");
        assert!(err.to_string().contains("url"), "unexpected error: {err}");
    }

    // Queue pressure is a typed scheduling fact, not target entry.
    #[tokio::test]
    async fn background_queue_full_records_typed_skip_signal()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = DefaultHookEngine::new(HooksConfig {
            background_max_concurrency: 1,
            entries: ["bg-first", "bg-second"]
                .into_iter()
                .map(|id| HookEntryConfig {
                    id: HookId::new(id),
                    point: HookPoint::PostToolExecution,
                    mode: HookExecutionMode::Background,
                    capability: HookCapability::Observe,
                    runtime: runtime_in_process("bg-held"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        let release = Arc::new(Notify::new());
        let handler_release = Arc::clone(&release);
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        engine
            .register_in_process_handler(
                "bg-held",
                Arc::new(move |_| {
                    let release = Arc::clone(&handler_release);
                    let entered = entered_tx.clone();
                    Box::pin(async move {
                        let notified = release.notified();
                        tokio::pin!(notified);
                        notified.as_mut().enable();
                        entered.send(()).map_err(|error| error.to_string())?;
                        notified.await;
                        Ok(RuntimeHookResponse { decision: None })
                    })
                }),
            )
            .await?;
        let session_id = SessionId::new();
        let report = engine
            .execute(
                invocation(HookPoint::PostToolExecution, session_id.clone()),
                None,
            )
            .await?;
        timeout(Duration::from_secs(5), entered_rx.recv())
            .await?
            .ok_or("missing entry")?;
        assert!(report.started.is_empty());
        assert!(report.outcomes.is_empty());
        assert_eq!(
            report.background_skips,
            vec![HookBackgroundSkip {
                hook_id: HookId::new("bg-second"),
                point: HookPoint::PostToolExecution,
                reason: HookBackgroundSkipReason::ConcurrencyFull,
            }]
        );
        assert!(
            engine
                .background_dispatch_ledger()
                .completion_snapshot()
                .await
                .is_empty()
        );
        release.notify_waiters();
        timeout(
            Duration::from_secs(5),
            engine
                .background_dispatch_ledger()
                .wait_for_completion(&session_id, None),
        )
        .await?;
        let completion = engine.take_background_completions(&session_id, None, 8);
        assert_eq!(completion.len(), 1);
        assert_eq!(completion[0].attribution.hook_id, HookId::new("bg-first"));
        timeout(Duration::from_secs(5), async {
            while let Some(result) = engine.inflight_background.lock().await.join_next().await {
                result?;
            }
            Ok::<(), tokio::task::JoinError>(())
        })
        .await??;
        Ok(())
    }

    // Actual failure is retained as a typed completion rather than a string-only drop.
    #[tokio::test]
    async fn background_failure_records_typed_dropped_signal()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = DefaultHookEngine::new(HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("bg-failing"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("bg-failing"),
                ..Default::default()
            }],
            ..Default::default()
        });
        engine
            .register_in_process_handler("bg-failing", erroring_handler())
            .await?;
        let session_id = SessionId::new();
        let report = engine
            .execute(
                invocation(HookPoint::PostToolExecution, session_id.clone()),
                None,
            )
            .await?;
        assert!(report.started.is_empty());
        timeout(
            Duration::from_secs(5),
            engine
                .background_dispatch_ledger()
                .wait_for_completion(&session_id, None),
        )
        .await?;
        let signals = engine.background_dispatch_ledger().snapshot().await;
        assert_eq!(signals.len(), 1);
        assert!(
            matches!(&signals[0], BackgroundDispatchSignal::Completed { completion }
            if completion.attribution.hook_id == HookId::new("bg-failing")
                && matches!(&completion.result, HookBackgroundResult::Failed(HookFailureReason::ExecutionFailed { .. })))
        );
        assert_eq!(engine.background_dispatch_ledger().drain().await, signals);
        assert!(
            engine
                .background_dispatch_ledger()
                .snapshot()
                .await
                .is_empty()
        );
        timeout(Duration::from_secs(5), async {
            while let Some(result) = engine.inflight_background.lock().await.join_next().await {
                result?;
            }
            Ok::<(), tokio::task::JoinError>(())
        })
        .await??;
        Ok(())
    }

    // These tests use the actual default background engine and dispatcher. They
    // establish process-local mechanical retention, not native work authority.
    async fn held_late_background_engine() -> (
        Arc<DefaultHookEngine>,
        tokio::sync::oneshot::Sender<()>,
        Arc<Notify>,
    ) {
        let engine = Arc::new(DefaultHookEngine::new(HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("same-late-observer"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                timeout_ms: Some(60_000),
                runtime: runtime_in_process("same-late-handler"),
                ..Default::default()
            }],
            ..Default::default()
        }));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let release = Arc::new(Mutex::new(Some(release_rx)));
        let entered = Arc::new(Notify::new());
        let handler_entered = Arc::clone(&entered);
        engine
            .register_in_process_handler(
                "same-late-handler",
                Arc::new(move |_| {
                    let release = Arc::clone(&release);
                    let entered = Arc::clone(&handler_entered);
                    Box::pin(async move {
                        let receiver = release
                            .lock()
                            .await
                            .take()
                            .ok_or("duplicate late observer")?;
                        entered.notify_one();
                        receiver.await.map_err(|error| error.to_string())?;
                        Ok(RuntimeHookResponse { decision: None })
                    })
                }),
            )
            .await
            .expect("register actual held observer");
        (engine, release_tx, entered)
    }

    struct LateSessionNoticeWriter {
        session: Mutex<meerkat_core::Session>,
        received: Mutex<Vec<meerkat_core::types::SystemNoticeRecord>>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorTranscriptNoticeHandle for LateSessionNoticeWriter {
        async fn append_system_notice_under_turn_finalization_boundary(
            &self,
            record: meerkat_core::types::SystemNoticeRecord,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            assert!(
                record.requests.is_empty(),
                "a fixed completion cannot introduce user work"
            );
            self.received.lock().await.push(record.clone());
            self.session.lock().await.append_system_notice_once(record);
            Ok(())
        }
    }

    fn late_notice_payload(record: &meerkat_core::types::SystemNoticeRecord) -> &serde_json::Value {
        assert_eq!(record.notice.blocks.len(), 1);
        let meerkat_core::types::SystemNoticeBlock::RuntimeNotice {
            category,
            payload: Some(payload),
            ..
        } = &record.notice.blocks[0]
        else {
            panic!("expected fixed background completion");
        };
        assert_eq!(category, "background_hook_completion");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &record.notice.blocks[0].model_projection_text()
            )
            .unwrap(),
            *payload
        );
        payload
    }

    #[tokio::test]
    async fn reconfigured_dispatcher_retains_old_engine_and_namespaces_colliding_ordinals()
    -> Result<(), Box<dyn std::error::Error>> {
        // The None case deliberately makes both passive scopes identical. The
        // registration namespace, not current run or local ordinal, separates them.
        for distinct_runs in [true, false] {
            let session = meerkat_core::Session::new();
            let session_id = session.id().clone();
            let old_run = distinct_runs.then(meerkat_core::RunId::new);
            let new_run = distinct_runs.then(meerkat_core::RunId::new);
            let dispatcher = meerkat_core::PostCommitHookDispatcher::new(session_id.clone());
            let (engine_a, release_a, entered_a) = held_late_background_engine().await;
            let ledger_a = engine_a.background_dispatch_ledger().clone();
            dispatcher.configure(Some(engine_a.clone()), HookRunOverrides::default())?;
            let mut invocation_a = invocation(HookPoint::PostToolExecution, session_id.clone());
            invocation_a.run_id = old_run.clone();
            invocation_a.turn_number = Some(7);
            let scheduled_a = engine_a.execute(invocation_a, None).await?;
            assert!(scheduled_a.started.is_empty());
            timeout(Duration::from_secs(5), entered_a.notified()).await?;

            let (engine_b, release_b, entered_b) = held_late_background_engine().await;
            let ledger_b = engine_b.background_dispatch_ledger().clone();
            dispatcher.configure(Some(engine_b.clone()), HookRunOverrides::default())?;
            let mut invocation_b = invocation(HookPoint::PostToolExecution, session_id.clone());
            invocation_b.run_id = new_run.clone();
            invocation_b.turn_number = Some(7);
            let scheduled_b = engine_b.execute(invocation_b, None).await?;
            assert!(scheduled_b.started.is_empty());
            timeout(Duration::from_secs(5), entered_b.notified()).await?;
            assert!(ledger_a.completion_snapshot().await.is_empty());

            // B has actually entered before old A is allowed to complete.
            release_b.send(()).map_err(|()| "B receiver disappeared")?;
            timeout(
                Duration::from_secs(5),
                ledger_b.wait_for_completion(&session_id, new_run.as_ref()),
            )
            .await?;
            release_a.send(()).map_err(|()| "A receiver disappeared")?;
            timeout(
                Duration::from_secs(5),
                ledger_a.wait_for_completion(&session_id, old_run.as_ref()),
            )
            .await?;
            let actual_a = ledger_a.completion_snapshot().await;
            let actual_b = ledger_b.completion_snapshot().await;
            assert_eq!(actual_a.len(), 1);
            assert_eq!(actual_b.len(), 1);
            assert_eq!(actual_a[0].ordinal, 1);
            assert_eq!(
                actual_a[0].ordinal, actual_b[0].ordinal,
                "fresh engines intentionally collide in their local ordinal space"
            );
            assert_eq!(actual_a[0].attribution.run_id, old_run);
            assert_eq!(actual_b[0].attribution.run_id, new_run);
            // Only the dispatcher's retained registration may discover A now.
            drop(engine_a);
            drop(ledger_a);
            let writer = LateSessionNoticeWriter {
                session: Mutex::new(session),
                received: Mutex::new(Vec::new()),
            };
            assert_eq!(
                timeout(
                    Duration::from_secs(5),
                    dispatcher
                        .flush_background_completions_under_turn_finalization_boundary(&writer)
                )
                .await??,
                2
            );
            assert_eq!(
                dispatcher
                    .flush_background_completions_under_turn_finalization_boundary(&writer)
                    .await?,
                0
            );
            let records = writer.received.lock().await;
            assert_eq!(
                records.len(),
                2,
                "neither old registration nor colliding ordinal may be lost"
            );
            let payloads: Vec<_> = records.iter().map(late_notice_payload).collect();
            for payload in &payloads {
                assert!(
                    payload["attribution"] == serde_json::to_value(&actual_a[0].attribution)?
                        || payload["attribution"]
                            == serde_json::to_value(&actual_b[0].attribution)?,
                    "the entire original attribution must survive reconfiguration unchanged"
                );
                assert_eq!(payload["ordinal"], serde_json::json!(actual_a[0].ordinal));
                assert_eq!(
                    payload["attribution"]["session_id"],
                    serde_json::json!(session_id)
                );
                assert_eq!(payload["attribution"]["hook_id"], "same-late-observer");
                assert_eq!(
                    payload["attribution"]["point"],
                    serde_json::json!(HookPoint::PostToolExecution)
                );
                assert_eq!(payload["attribution"]["turn_number"], 7);
                assert_eq!(payload["disposition"], "completed");
                assert!(
                    payload["registration_id"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty())
                );
            }
            assert_ne!(
                payloads[0]["registration_id"],
                payloads[1]["registration_id"]
            );
            if distinct_runs {
                assert_eq!(
                    payloads
                        .iter()
                        .filter(|payload| payload["attribution"]["run_id"]
                            == serde_json::json!(old_run))
                        .count(),
                    1
                );
                assert_eq!(
                    payloads
                        .iter()
                        .filter(|payload| payload["attribution"]["run_id"]
                            == serde_json::json!(new_run))
                        .count(),
                    1
                );
            } else {
                assert!(
                    payloads
                        .iter()
                        .all(|payload| payload["attribution"]["run_id"].is_null())
                );
            }
            assert_ne!(
                records[0].notice.blocks, records[1].notice.blocks,
                "existing Session dedup compares kind and blocks"
            );
            let transcript = writer.session.lock().await;
            assert_eq!(transcript.messages().len(), 2);
            assert!(
                transcript
                    .messages()
                    .iter()
                    .all(|message| matches!(message, meerkat_core::Message::SystemNotice(_))),
                "this fixed-fact writer records only typed notices"
            );
            dispatcher.shutdown();
        }
        Ok(())
    }
}
