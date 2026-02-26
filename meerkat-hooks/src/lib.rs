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
        description: "Writing hooks for the 7 hook points, execution modes, and decision semantics",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["hooks"],
        body: include_str!("../skills/hook-authoring/SKILL.md"),
        extensions: &[],
    }
}

// Capability registration
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::Hooks,
        description: "7 hook points, 3 runtimes (in-process/command/HTTP), deny/allow/rewrite semantics",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: None,
    }
}

use chrono::Utc;
use futures::StreamExt;
use meerkat_core::time_compat::Duration;
use meerkat_core::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookEntryConfig, HookExecutionMode,
    HookExecutionReport, HookFailurePolicy, HookId, HookInvocation, HookOutcome, HookPatch,
    HookPatchEnvelope, HookReasonCode, HookRevision, HookRunOverrides, HooksConfig,
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

/// Response returned by runtime adapters.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeHookResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    #[serde(default)]
    pub patches: Vec<HookPatch>,
}

#[derive(Debug, Clone, Deserialize)]
struct InProcessRuntimeConfig {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CommandRuntimeConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct HttpRuntimeConfig {
    url: String,
    #[serde(default = "default_http_method")]
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

fn default_http_method() -> String {
    "POST".to_string()
}

#[cfg(not(target_arch = "wasm32"))]
type HandlerFuture = Pin<Box<dyn Future<Output = Result<RuntimeHookResponse, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
type HandlerFuture = Pin<Box<dyn Future<Output = Result<RuntimeHookResponse, String>>>>;

pub type InProcessHookHandler = Arc<dyn Fn(HookInvocation) -> HandlerFuture + Send + Sync>;

/// Deterministic hook engine used by all control surfaces.
#[derive(Clone)]
pub struct DefaultHookEngine {
    config: HooksConfig,
    base_entries: Arc<Vec<HookEntryConfig>>,
    base_validation_error: Option<String>,
    http_client: Arc<OnceLock<reqwest::Client>>,
    in_process_handlers: Arc<std::sync::RwLock<HashMap<String, InProcessHookHandler>>>,
    published_patches: Arc<Mutex<Vec<HookPatchEnvelope>>>,
    background_slots: Arc<Semaphore>,
    revision: Arc<AtomicU64>,
}

impl DefaultHookEngine {
    pub fn new(config: HooksConfig) -> Self {
        let base_validation_error = config
            .entries
            .iter()
            .find_map(|entry| Self::validate_entry(entry).err())
            .map(|err| err.to_string());
        let background_max_concurrency = config.background_max_concurrency.max(1);

        Self {
            base_entries: Arc::new(config.entries.clone()),
            config,
            base_validation_error,
            http_client: Arc::new(OnceLock::new()),
            in_process_handlers: Arc::new(std::sync::RwLock::new(HashMap::new())),
            published_patches: Arc::new(Mutex::new(Vec::new())),
            background_slots: Arc::new(Semaphore::new(background_max_concurrency)),
            revision: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn with_in_process_handler(
        self,
        name: impl Into<String>,
        handler: InProcessHookHandler,
    ) -> Self {
        let next = self;
        let name = name.into();
        match next.in_process_handlers.write() {
            Ok(mut map) => {
                map.insert(name, handler);
            }
            Err(err) => {
                tracing::warn!("failed to register in-process hook handler: {}", err);
            }
        }
        next
    }

    pub async fn register_in_process_handler(
        &self,
        name: impl Into<String>,
        handler: InProcessHookHandler,
    ) {
        match self.in_process_handlers.write() {
            Ok(mut map) => {
                map.insert(name.into(), handler);
            }
            Err(err) => {
                tracing::warn!("failed to register in-process hook handler: {}", err);
            }
        }
    }

    fn next_revision(&self) -> HookRevision {
        HookRevision(self.revision.fetch_add(1, Ordering::SeqCst))
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
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 8 * 1024];

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

    fn effective_entries<'a>(
        &'a self,
        overrides: Option<&'a HookRunOverrides>,
    ) -> Result<std::borrow::Cow<'a, [HookEntryConfig]>, HookEngineError> {
        if let Some(reason) = &self.base_validation_error {
            return Err(HookEngineError::InvalidConfiguration(reason.clone()));
        }

        if let Some(overrides) = overrides {
            if overrides.disable.is_empty() && overrides.entries.is_empty() {
                return Ok(std::borrow::Cow::Borrowed(self.base_entries.as_slice()));
            }

            let mut entries = self.base_entries.as_ref().clone();
            if !overrides.disable.is_empty() {
                let disabled: HashSet<HookId> = overrides.disable.iter().cloned().collect();
                entries.retain(|entry| !disabled.contains(&entry.id));
            }
            entries.extend(overrides.entries.clone());

            for entry in &entries {
                Self::validate_entry(entry)?;
            }

            return Ok(std::borrow::Cow::Owned(entries));
        }

        Ok(std::borrow::Cow::Borrowed(self.base_entries.as_slice()))
    }

    async fn drain_published_patches(&self) -> Vec<HookPatchEnvelope> {
        let mut guard = self.published_patches.lock().await;
        std::mem::take(&mut *guard)
    }

    async fn publish_patches(&self, patches: Vec<HookPatchEnvelope>) {
        if patches.is_empty() {
            return;
        }
        let mut guard = self.published_patches.lock().await;
        guard.extend(patches);
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

        if entry.mode == HookExecutionMode::Background
            && entry.effective_failure_policy() == HookFailurePolicy::FailClosed
        {
            return Err(HookEngineError::InvalidConfiguration(format!(
                "background hooks must use fail_open policy: {}",
                entry.id
            )));
        }

        Ok(())
    }

    async fn execute_one(
        &self,
        entry: HookEntryConfig,
        registration_index: usize,
        invocation: HookInvocation,
    ) -> HookOutcome {
        let start = meerkat_core::time_compat::Instant::now();
        let timeout_ms = entry.timeout_ms.unwrap_or(self.config.default_timeout_ms);
        let policy = entry.effective_failure_policy();

        let mut outcome = HookOutcome {
            hook_id: entry.id.clone(),
            point: entry.point,
            priority: entry.priority,
            registration_index,
            decision: None,
            patches: Vec::new(),
            published_patches: Vec::new(),
            error: None,
            duration_ms: None,
        };

        let runtime_result = timeout(
            Duration::from_millis(timeout_ms),
            self.invoke_runtime(&entry, invocation.clone()),
        )
        .await;

        match runtime_result {
            Err(_) => {
                let message = format!("hook timed out after {timeout_ms}ms");
                if policy == HookFailurePolicy::FailClosed {
                    outcome.decision = Some(HookDecision::deny(
                        entry.id.clone(),
                        HookReasonCode::Timeout,
                        message,
                        None,
                    ));
                } else {
                    outcome.error = Some(message);
                }
            }
            Ok(Err(err)) => {
                let message = err.to_string();
                if policy == HookFailurePolicy::FailClosed {
                    outcome.decision = Some(HookDecision::deny(
                        entry.id.clone(),
                        HookReasonCode::RuntimeError,
                        message,
                        None,
                    ));
                } else {
                    outcome.error = Some(message);
                }
            }
            Ok(Ok(response)) => {
                outcome.decision = response.decision;
                outcome.patches = response.patches;
            }
        }

        if entry.mode == HookExecutionMode::Background {
            if entry.point.is_pre() {
                if !outcome.patches.is_empty()
                    || matches!(outcome.decision, Some(HookDecision::Deny { .. }))
                {
                    outcome.error = Some("pre_* background hooks are observe-only".to_string());
                }
                outcome.patches.clear();
                outcome.decision = None;
            } else if entry.point.is_post() {
                let mut envelopes = Vec::new();
                for patch in outcome.patches.drain(..) {
                    envelopes.push(HookPatchEnvelope {
                        revision: self.next_revision(),
                        hook_id: entry.id.clone(),
                        point: entry.point,
                        patch,
                        published_at: Utc::now(),
                    });
                }
                outcome.published_patches = envelopes;
                outcome.decision = None;
            }
        }

        outcome.duration_ms = Some(start.elapsed().as_millis() as u64);
        outcome
    }

    async fn invoke_runtime(
        &self,
        entry: &HookEntryConfig,
        invocation: HookInvocation,
    ) -> Result<RuntimeHookResponse, HookEngineError> {
        let runtime_kind = entry.runtime.kind.as_str();
        let runtime_config =
            entry
                .runtime
                .config_value()
                .map_err(|err| HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: format!("invalid runtime config payload: {err}"),
                })?;

        match runtime_kind {
            "in_process" => {
                let cfg: InProcessRuntimeConfig =
                    serde_json::from_value(runtime_config).map_err(|err| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("invalid in_process runtime config: {err}"),
                        }
                    })?;
                let handler = {
                    let handlers = self.in_process_handlers.read().map_err(|err| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("in-process handler lock poisoned: {err}"),
                        }
                    })?;
                    handlers.get(&cfg.name).cloned().ok_or_else(|| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("in-process handler '{}' not registered", cfg.name),
                        }
                    })?
                };
                (handler)(invocation)
                    .await
                    .map_err(|reason| HookEngineError::ExecutionFailed {
                        hook_id: entry.id.clone(),
                        reason,
                    })
            }
            "command" => {
                let cfg: CommandRuntimeConfig =
                    serde_json::from_value(runtime_config).map_err(|err| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("invalid command runtime config: {err}"),
                        }
                    })?;
                self.invoke_command_runtime(entry, &cfg.command, &cfg.args, &cfg.env, invocation)
                    .await
            }
            "http" => {
                let cfg: HttpRuntimeConfig =
                    serde_json::from_value(runtime_config).map_err(|err| {
                        HookEngineError::ExecutionFailed {
                            hook_id: entry.id.clone(),
                            reason: format!("invalid http runtime config: {err}"),
                        }
                    })?;
                self.invoke_http_runtime(entry, &cfg.url, &cfg.method, &cfg.headers, invocation)
                    .await
            }
            other => Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("unsupported hook runtime '{other}'"),
            }),
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

        if payload.len() > self.config.payload_max_bytes {
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!(
                    "hook payload exceeds max size: {} > {}",
                    payload.len(),
                    self.config.payload_max_bytes
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

        let max_output_bytes = self.config.payload_max_bytes;
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

        if payload.len() > self.config.payload_max_bytes {
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!(
                    "hook payload exceeds max size: {} > {}",
                    payload.len(),
                    self.config.payload_max_bytes
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
            if bytes.len() > self.config.payload_max_bytes {
                return Err(HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: format!(
                        "HTTP hook response exceeds max size: {} > {}",
                        bytes.len(),
                        self.config.payload_max_bytes
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

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl HookEngine for DefaultHookEngine {
    fn matching_hooks(
        &self,
        invocation: &HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<Vec<HookId>, HookEngineError> {
        let entries = self.effective_entries(overrides)?;
        Ok(entries
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
        let entries = self
            .effective_entries(overrides)?
            .iter()
            .filter(|entry| entry.enabled && entry.point == invocation.point)
            .cloned()
            .collect::<Vec<_>>();

        if entries.is_empty() {
            let mut report = HookExecutionReport::empty();
            report.published_patches = self.drain_published_patches().await;
            return Ok(report);
        }

        let mut foreground = Vec::new();
        let mut background = Vec::new();
        for (registration_index, entry) in entries.into_iter().enumerate() {
            if entry.mode == HookExecutionMode::Background {
                background.push((registration_index, entry));
            } else {
                foreground.push((registration_index, entry));
            }
        }

        foreground.sort_by(|(a_idx, a_entry), (b_idx, b_entry)| {
            a_entry
                .priority
                .cmp(&b_entry.priority)
                .then_with(|| a_idx.cmp(b_idx))
        });

        let mut merged = HookExecutionReport::empty();
        for (registration_index, entry) in foreground {
            let outcome = self
                .execute_one(entry, registration_index, invocation.clone())
                .await;

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
            merged.patches.extend(outcome.patches.clone());

            let should_stop = matches!(outcome.decision, Some(HookDecision::Deny { .. }));
            merged.outcomes.push(outcome);
            if should_stop {
                break;
            }
        }

        if !matches!(merged.decision, Some(HookDecision::Deny { .. })) {
            for (registration_index, entry) in background {
                let permit = match self.background_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        tracing::warn!("background hook queue full, skipping {}", entry.id);
                        continue;
                    }
                };
                let engine = self.clone();
                let invocation_cloned = invocation.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let outcome = engine
                        .execute_one(entry, registration_index, invocation_cloned)
                        .await;
                    if let Some(error) = &outcome.error {
                        tracing::warn!(
                            hook_id = %outcome.hook_id,
                            point = ?outcome.point,
                            error = %error,
                            "background hook execution produced an error"
                        );
                    }
                    if !outcome.published_patches.is_empty() {
                        engine.publish_patches(outcome.published_patches).await;
                    }
                });
            }
        }
        merged.published_patches = self.drain_published_patches().await;

        Ok(merged)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use meerkat_core::{
        HookFailurePolicy, HookLlmRequest, HookPoint, HookReasonCode, HookRuntimeConfig, SessionId,
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

    fn runtime_in_process(name: &str) -> HookRuntimeConfig {
        HookRuntimeConfig::new("in_process", Some(serde_json::json!({"name": name})))
            .unwrap_or_default()
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
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("hook-b"),
                point: HookPoint::PreLlmRequest,
                priority: 5,
                runtime: runtime_in_process("b"),
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "a",
                static_handler(RuntimeHookResponse {
                    decision: None,
                    patches: vec![HookPatch::LlmRequest {
                        max_tokens: Some(100),
                        temperature: None,
                        provider_params: None,
                    }],
                }),
            )
            .await;
        engine
            .register_in_process_handler(
                "b",
                static_handler(RuntimeHookResponse {
                    decision: None,
                    patches: vec![HookPatch::LlmRequest {
                        max_tokens: Some(50),
                        temperature: None,
                        provider_params: None,
                    }],
                }),
            )
            .await;

        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreLlmRequest,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
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

        assert_eq!(report.patches.len(), 2);
        let mut values = Vec::new();
        for patch in report.patches {
            if let HookPatch::LlmRequest { max_tokens, .. } = patch {
                values.push(max_tokens.unwrap_or_default());
            }
        }
        assert_eq!(values, vec![50, 100]);
    }

    #[tokio::test]
    async fn pre_background_rewrite_attempt_is_rejected() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("hook-pre-bg"),
            point: HookPoint::PreToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("pre-bg"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "pre-bg",
                static_handler(RuntimeHookResponse {
                    decision: None,
                    patches: vec![HookPatch::ToolArgs {
                        args: serde_json::json!({"x": 1}),
                    }],
                }),
            )
            .await;

        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(report.patches.is_empty());
    }

    #[tokio::test]
    async fn post_background_rewrite_is_published() {
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
                static_handler(RuntimeHookResponse {
                    decision: None,
                    patches: vec![HookPatch::ToolResult {
                        content: "patched".to_string(),
                        is_error: Some(false),
                    }],
                }),
            )
            .await;

        let first = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PostToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(first.patches.is_empty());
        let mut published = first.published_patches;
        for _ in 0..20 {
            if !published.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            let follow_up = engine
                .execute(
                    HookInvocation {
                        point: HookPoint::TurnBoundary,
                        session_id: SessionId::new(),
                        turn_number: Some(1),
                        prompt: None,
                        error: None,
                        llm_request: None,
                        llm_response: None,
                        tool_call: None,
                        tool_result: None,
                    },
                    None,
                )
                .await
                .unwrap();
            published.extend(follow_up.published_patches);
        }
        assert_eq!(published.len(), 1);
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
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("fast-high-priority"),
                point: HookPoint::PreLlmRequest,
                priority: 1,
                runtime: runtime_in_process("fast"),
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
        ];

        let engine = DefaultHookEngine::new(config);
        engine
            .register_in_process_handler(
                "slow",
                delayed_handler(
                    100,
                    RuntimeHookResponse {
                        decision: None,
                        patches: vec![HookPatch::LlmRequest {
                            max_tokens: Some(200),
                            temperature: None,
                            provider_params: None,
                        }],
                    },
                ),
            )
            .await;
        engine
            .register_in_process_handler(
                "fast",
                delayed_handler(
                    1,
                    RuntimeHookResponse {
                        decision: None,
                        patches: vec![HookPatch::LlmRequest {
                            max_tokens: Some(20),
                            temperature: None,
                            provider_params: None,
                        }],
                    },
                ),
            )
            .await;

        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreLlmRequest,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
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

        let values: Vec<u32> = report
            .patches
            .into_iter()
            .filter_map(|patch| match patch {
                HookPatch::LlmRequest { max_tokens, .. } => max_tokens,
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![20, 200]);
    }

    #[tokio::test]
    async fn observe_defaults_fail_open_on_runtime_error() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("observe-missing-handler"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Observe,
            runtime: runtime_in_process("missing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(report.decision.is_none());
        assert_eq!(report.outcomes.len(), 1);
        assert!(report.outcomes[0].error.is_some());
    }

    #[tokio::test]
    async fn rewrite_defaults_fail_closed_on_runtime_error() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("rewrite-missing-handler"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Rewrite,
            runtime: runtime_in_process("missing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(matches!(
            report.decision,
            Some(HookDecision::Deny {
                reason_code: HookReasonCode::RuntimeError,
                ..
            })
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
                    patches: Vec::new(),
                }),
            )
            .await;
        engine
            .register_in_process_handler(
                "observer",
                Arc::new(move |_invocation| {
                    let observer_runs = observer_runs.clone();
                    Box::pin(async move {
                        observer_runs.fetch_add(1, AtomicOrdering::SeqCst);
                        Ok(RuntimeHookResponse {
                            decision: Some(HookDecision::Allow),
                            patches: Vec::new(),
                        })
                    })
                }),
            )
            .await;

        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
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
                    prompt: Some("hi".to_string()),
                    error: None,
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
    async fn rewrite_with_fail_open_override_does_not_deny() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("rewrite-explicit-open"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Rewrite,
            failure_policy: Some(HookFailurePolicy::FailOpen),
            runtime: runtime_in_process("missing"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let report = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PreToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap();

        assert!(report.decision.is_none());
        assert_eq!(report.outcomes.len(), 1);
        assert!(report.outcomes[0].error.is_some());
    }

    #[tokio::test]
    async fn background_fail_closed_configuration_is_rejected() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("invalid-background-fail-closed"),
            point: HookPoint::PostToolExecution,
            mode: HookExecutionMode::Background,
            capability: HookCapability::Observe,
            failure_policy: Some(HookFailurePolicy::FailClosed),
            runtime: runtime_in_process("bg"),
            ..Default::default()
        }];

        let engine = DefaultHookEngine::new(config);
        let err = engine
            .execute(
                HookInvocation {
                    point: HookPoint::PostToolExecution,
                    session_id: SessionId::new(),
                    turn_number: Some(1),
                    prompt: None,
                    error: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                None,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, HookEngineError::InvalidConfiguration(_)));
    }

    #[tokio::test]
    async fn command_runtime_hook_executes() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("command-hook"),
            point: HookPoint::PreToolExecution,
            runtime: HookRuntimeConfig::new(
                "command",
                Some(serde_json::json!({
                    "command": "sh",
                    "args": ["-c", "cat >/dev/null; printf '{\"patches\":[]}'"],
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
                    prompt: None,
                    error: None,
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
            report.outcomes[0].error.is_none(),
            "command runtime error: {:?}",
            report.outcomes[0].error
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
                    let body = r#"{"patches":[]}"#;
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
            runtime: HookRuntimeConfig::new(
                "http",
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
                    prompt: None,
                    error: None,
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
            report.outcomes[0].error.is_none(),
            "http runtime error: {:?}",
            report.outcomes[0].error
        );
    }
}
