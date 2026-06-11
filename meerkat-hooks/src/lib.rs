//! Hook runtimes (in-process, command, HTTP) and deterministic default engine.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use tokio_with_wasm::alias::*;
}

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
use meerkat_core::config::HookAdapterConfig;
use meerkat_core::time_compat::Duration;
use meerkat_core::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookEntryConfig, HookExecutionMode,
    HookExecutionReport, HookFailureReason, HookId, HookInvocation, HookOutcome, HookRunOverrides,
    HooksConfig,
};
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
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

pub use meerkat_core::config::HookInProcessHandlerId as InProcessHookHandlerId;

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

/// Typed reason a background hook publish was not delivered.
///
/// Background hook execution is fire-and-forget by design, but the engine must
/// not silently lose work: a full queue and a failed/observed-error background
/// run are both recorded as typed signals against the engine's
/// [`BackgroundDispatchLedger`] (remediation row #35) instead of a bare
/// `continue` / `tracing::warn!` with no observable trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundDispatchSignal {
    /// The background concurrency queue was full; the hook was not dispatched.
    Skipped { hook_id: HookId },
    /// A dispatched background hook failed to execute and its publish (if any)
    /// was dropped.
    Dropped { hook_id: HookId, reason: String },
}

/// Observable ledger of background dispatch skip/drop signals.
///
/// Cloned engine handles share the same ledger via the inner `Arc`, so a skip
/// or drop recorded by a background task is observable through any engine
/// clone (which is how the cross-task delivery queue is already shared).
#[derive(Debug, Clone, Default)]
pub struct BackgroundDispatchLedger {
    inner: Arc<Mutex<Vec<BackgroundDispatchSignal>>>,
}

impl BackgroundDispatchLedger {
    fn new() -> Self {
        Self::default()
    }

    async fn record(&self, signal: BackgroundDispatchSignal) {
        self.inner.lock().await.push(signal);
    }

    /// Snapshot the recorded signals without clearing them.
    pub async fn snapshot(&self) -> Vec<BackgroundDispatchSignal> {
        self.inner.lock().await.clone()
    }

    /// Drain and return all recorded signals.
    pub async fn drain(&self) -> Vec<BackgroundDispatchSignal> {
        std::mem::take(&mut *self.inner.lock().await)
    }
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
    /// Typed, observable ledger of background-dispatch skip/drop signals so a
    /// full queue or a failed background run is never silently lost
    /// (remediation row #35).
    background_dispatch_ledger: BackgroundDispatchLedger,
    background_slots: Arc<Semaphore>,
    /// In-flight background hook tasks owned by the engine lifecycle.
    ///
    /// On non-wasm targets the previously-detached background `tokio::spawn`
    /// is tracked here so (a) dropping the engine aborts the `JoinSet` instead
    /// of leaking a zombie task, and (b) `execute` reaps finished tasks before
    /// dispatching new background work, keeping the set bounded. Wasm has no
    /// detached-task-teardown hazard, so the field is gated out and the wasm
    /// path keeps the existing spawn.
    #[cfg(not(target_arch = "wasm32"))]
    inflight_background: Arc<Mutex<tokio::task::JoinSet<()>>>,
    revision: Arc<AtomicU64>,
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
        }
    }

    /// Typed, observable view of background-dispatch skip/drop signals.
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
    /// `JoinSet` stays bounded and engine drop only has to abort
    /// genuinely-in-flight work.
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
            && entry.point.is_pre()
            && entry.capability != HookCapability::Observe
        {
            return Err(HookEngineError::InvalidConfiguration(format!(
                "pre_* background hooks must be observe-only: {}",
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

        let runtime_result = timeout(
            Duration::from_millis(timeout_ms),
            self.invoke_runtime(&entry, &adapter, invocation.clone()),
        )
        .await;

        let response = match runtime_result {
            Err(_) => {
                return Err(HookEngineError::Timeout {
                    hook_id: entry.id.clone(),
                    timeout_ms,
                });
            }
            Ok(response) => response?,
        };
        outcome.decision = runtime_decision_with_configured_hook_id(response.decision, &entry.id);

        if entry.mode == HookExecutionMode::Background {
            if entry.point.is_pre() && matches!(outcome.decision, Some(HookDecision::Deny { .. })) {
                outcome.failure_reason = Some(HookFailureReason::ObserveOnlyViolation);
            }
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
            HookAdapterConfig::Command(cfg) => {
                self.invoke_command_runtime(entry, &cfg.command, &cfg.args, &cfg.env, invocation)
                    .await
            }
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
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        invocation: HookInvocation,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed to spawn command hook: {err}"),
            })?;

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

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&payload)
                .await
                .map_err(|err| HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: format!("failed to write command hook stdin: {err}"),
                })?;
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: "command hook stdout pipe unavailable".to_string(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: "command hook stderr pipe unavailable".to_string(),
            })?;

        let max_output_bytes = self.policy.payload_max_bytes();
        let stdout_task = tokio::spawn(Self::read_stream_limited(
            stdout,
            max_output_bytes,
            "command stdout",
        ));
        let stderr_task = tokio::spawn(Self::read_stream_limited(
            stderr,
            max_output_bytes,
            "command stderr",
        ));

        let status = child
            .wait()
            .await
            .map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed waiting for command hook: {err}"),
            })?;

        let stdout = stdout_task
            .await
            .map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("command stdout task join failed: {err}"),
            })?
            .map_err(|reason| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason,
            })?;
        let stderr = stderr_task
            .await
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
        _command: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _invocation: HookInvocation,
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
            // boundary). A missing adapter is an internal invariant break, not
            // a recoverable runtime state, so fail closed with a typed error
            // rather than parsing JSON here.
            let adapter = resolved.adapter(&entry.id).cloned().ok_or_else(|| {
                HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: "no resolved adapter for hook id".to_string(),
                }
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
            let outcome = self
                .execute_one(entry, adapter, registration_index, invocation.clone())
                .await?;

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
                let permit = match self.background_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        // Queue full: record a typed skip signal against the
                        // observable ledger rather than a silent `continue`
                        // (remediation row #35).
                        self.background_dispatch_ledger
                            .record(BackgroundDispatchSignal::Skipped {
                                hook_id: entry.id.clone(),
                            })
                            .await;
                        tracing::warn!("background hook queue full, skipping {}", entry.id);
                        continue;
                    }
                };
                let engine = self.clone();
                let invocation_cloned = invocation.clone();
                let task = async move {
                    let _permit = permit;
                    let hook_id = entry.id.clone();
                    let outcome = match engine
                        .execute_one(entry, adapter, registration_index, invocation_cloned)
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(error) => {
                            // A failed background run drops its publish; record
                            // a typed dropped signal so the loss is observable
                            // (remediation row #35).
                            let reason = error.to_string();
                            engine
                                .background_dispatch_ledger
                                .record(BackgroundDispatchSignal::Dropped {
                                    hook_id,
                                    reason: reason.clone(),
                                })
                                .await;
                            tracing::warn!(
                                error = %reason,
                                "background hook execution failed"
                            );
                            return;
                        }
                    };
                    if let Some(failure_reason) = &outcome.failure_reason {
                        // A produced-error background outcome drops its publish;
                        // record a typed dropped signal (remediation row #35).
                        // The ledger carries the derived display string.
                        let reason = failure_reason.to_string();
                        engine
                            .background_dispatch_ledger
                            .record(BackgroundDispatchSignal::Dropped {
                                hook_id: outcome.hook_id.clone(),
                                reason: reason.clone(),
                            })
                            .await;
                        tracing::warn!(
                            hook_id = %outcome.hook_id,
                            point = ?outcome.point,
                            error = %reason,
                            "background hook execution produced an error"
                        );
                    }
                };
                // Non-wasm: bind the task to the engine lifecycle via the owned
                // `JoinSet` so engine drop aborts it and `execute` reaps
                // finished tasks. Wasm: keep the existing detached spawn (no
                // detached-task-teardown hazard there).
                #[cfg(not(target_arch = "wasm32"))]
                self.inflight_background.lock().await.spawn(task);
                #[cfg(target_arch = "wasm32")]
                tokio::spawn(task);
            }
        }

        Ok(merged)
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
    use meerkat_core::{HookLlmRequest, HookPoint, HookReasonCode, SessionId};
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
    async fn background_hooks_are_observation_only_and_publish_no_patches() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("hook-post-bg"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("post-bg"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "post-bg",
                static_handler(RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let session_id = SessionId::new();
        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PostToolExecution,
                    session_id: session_id.clone(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            report
                .outcomes
                .iter()
                .all(|outcome| outcome.decision.is_none()),
            "background hooks are observation-only"
        );
    }

    #[tokio::test]
    async fn background_post_hook_task_is_tracked_and_reaped_not_leaked() {
        // R2 regression: the background post-hook task used to be a detached
        // `tokio::spawn` tied to nothing, so on engine/runtime teardown an
        // in-flight task would vanish with no signal. The task is now owned by
        // the engine's `JoinSet`, so it is (a) observable as in-flight after
        // `execute()` and (b) reaped by `reap_finished_background_tasks` once
        // it completes.
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("tracked-post-bg"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("tracked-post-bg"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        // A handler with a short delay so the spawned task is genuinely
        // in-flight when we observe the JoinSet immediately after `execute()`.
        engine
            .register_in_process_handler(
                "tracked-post-bg",
                delayed_handler(20, RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("in-process handler registration must succeed");

        let session_id = SessionId::new();
        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PostToolExecution,
                    session_id: session_id.clone(),
                    turn_number: Some(1),
                    prompt_input: None,
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            report.outcomes.is_empty(),
            "background outcomes are not merged"
        );

        // The background task is owned by the engine's JoinSet, not detached:
        // it is tracked as in-flight right after `execute()` returns.
        assert_eq!(
            engine.inflight_background.lock().await.len(),
            1,
            "background post-hook task must be tracked in the engine JoinSet, not detached"
        );

        // The reap seam removes the finished task (non-blocking
        // `try_join_next`). Reap repeatedly until the task has run and been
        // reaped, proving the task is lifecycle-bound rather than leaked.
        for _ in 0..200 {
            engine.reap_finished_background_tasks().await;
            if engine.inflight_background.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            engine.inflight_background.lock().await.is_empty(),
            "finished background task must be reaped from the engine JoinSet, not leaked"
        );
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
                },
                None,
            )
            .await
            .unwrap();

        assert!(matches!(report.decision, Some(HookDecision::Deny { .. })));
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(observed_runs.load(AtomicOrdering::SeqCst), 0);
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
    async fn run_started_background_guardrail_is_rejected() {
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
                },
                None,
            )
            .await
            .expect_err("invalid background pre hook must be rejected");

        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
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

    // Gate for remediation row #35 (part 1): a full background queue produces a
    // typed Skipped signal against the observable ledger, not a silent
    // `continue`.
    #[tokio::test]
    async fn background_queue_full_records_typed_skip_signal() {
        let mut config = HooksConfig::default();
        // Single background slot so the second background hook cannot be
        // dispatched while the first holds the only permit.
        config.background_max_concurrency = 1;
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("bg-first"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("bg-slow"),
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("bg-second"),
                point: HookPoint::PostToolExecution,
                mode: HookExecutionMode::Background,
                capability: HookCapability::Observe,
                runtime: runtime_in_process("bg-slow"),
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "bg-slow",
                delayed_handler(500, RuntimeHookResponse { decision: None }),
            )
            .await
            .expect("register bg-slow handler");

        engine
            .execute(
                invocation(HookPoint::PostToolExecution, SessionId::new()),
                None,
            )
            .await
            .expect("execute background hooks");

        // The first hook took the only permit at dispatch time; the second was
        // queue-full and recorded a typed Skipped signal (not a silent skip).
        let signals = engine.background_dispatch_ledger().snapshot().await;
        assert_eq!(
            signals,
            vec![BackgroundDispatchSignal::Skipped {
                hook_id: HookId::new("bg-second"),
            }],
            "queue-full must record a typed Skipped signal, got {signals:?}"
        );
    }

    // Gate for remediation row #35 (part 2): a failed background hook records a
    // typed Dropped signal against the observable ledger instead of a
    // warn-only silent loss.
    #[tokio::test]
    async fn background_failure_records_typed_dropped_signal() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("bg-failing"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("bg-failing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler("bg-failing", erroring_handler())
            .await
            .expect("register failing handler");

        engine
            .execute(
                invocation(HookPoint::PostToolExecution, SessionId::new()),
                None,
            )
            .await
            .expect("execute failing background hook");

        // Drain the ledger until the background task has run and recorded its
        // typed Dropped signal (the task is reaped/lifecycle-bound, so this
        // converges quickly).
        let mut signals = Vec::new();
        for _ in 0..200 {
            signals = engine.background_dispatch_ledger().snapshot().await;
            if !signals.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            signals.len(),
            1,
            "expected one dropped signal, got {signals:?}"
        );
        match &signals[0] {
            BackgroundDispatchSignal::Dropped { hook_id, reason } => {
                assert_eq!(hook_id, &HookId::new("bg-failing"));
                assert!(
                    reason.contains("bg-failing"),
                    "dropped reason must carry the typed engine failure, got {reason}"
                );
            }
            other => panic!("expected a typed Dropped signal, got {other:?}"),
        }
    }
}
