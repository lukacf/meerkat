//! Hook runtimes (in-process, command, HTTP) and deterministic default engine.

use chrono::Utc;
use futures::future::join_all;
use meerkat_core::{
    HookCapability, HookDecision, HookEngine, HookEngineError, HookEntryConfig, HookExecutionMode,
    HookExecutionReport, HookFailurePolicy, HookId, HookInvocation, HookOutcome, HookPatch,
    HookPatchEnvelope, HookReasonCode, HookRevision, HookRunOverrides, HookRuntimeConfig,
    HooksConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};

/// Response returned by runtime adapters.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RuntimeHookResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    #[serde(default)]
    pub patches: Vec<HookPatch>,
}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<RuntimeHookResponse, String>> + Send>>;

pub type InProcessHookHandler = Arc<dyn Fn(HookInvocation) -> HandlerFuture + Send + Sync>;

/// Deterministic hook engine used by all control surfaces.
#[derive(Clone)]
pub struct DefaultHookEngine {
    config: HooksConfig,
    http_client: reqwest::Client,
    in_process_handlers: Arc<RwLock<HashMap<String, InProcessHookHandler>>>,
    revision: Arc<AtomicU64>,
}

impl DefaultHookEngine {
    pub fn new(config: HooksConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
            in_process_handlers: Arc::new(RwLock::new(HashMap::new())),
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
        if let Ok(mut map) = next.in_process_handlers.try_write() {
            map.insert(name, handler);
        }
        next
    }

    pub async fn register_in_process_handler(
        &self,
        name: impl Into<String>,
        handler: InProcessHookHandler,
    ) {
        self.in_process_handlers
            .write()
            .await
            .insert(name.into(), handler);
    }

    fn next_revision(&self) -> HookRevision {
        HookRevision(self.revision.fetch_add(1, Ordering::Relaxed))
    }

    fn effective_entries(
        &self,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<Vec<HookEntryConfig>, HookEngineError> {
        let mut entries = self.config.entries.clone();
        if let Some(overrides) = overrides {
            if !overrides.disable.is_empty() {
                let disabled: HashSet<HookId> = overrides.disable.iter().cloned().collect();
                entries.retain(|entry| !disabled.contains(&entry.id));
            }
            entries.extend(overrides.entries.clone());
        }

        for entry in &entries {
            Self::validate_entry(entry)?;
        }

        Ok(entries)
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
        let start = std::time::Instant::now();
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
                let message = format!("hook timed out after {}ms", timeout_ms);
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
        match &entry.runtime {
            HookRuntimeConfig::InProcess { name, .. } => {
                let handlers = self.in_process_handlers.read().await;
                let Some(handler) = handlers.get(name) else {
                    return Err(HookEngineError::ExecutionFailed {
                        hook_id: entry.id.clone(),
                        reason: format!("in-process handler '{}' not registered", name),
                    });
                };
                (handler)(invocation)
                    .await
                    .map_err(|reason| HookEngineError::ExecutionFailed {
                        hook_id: entry.id.clone(),
                        reason,
                    })
            }
            HookRuntimeConfig::Command { command, args, env } => {
                self.invoke_command_runtime(entry, command, args, env, invocation)
                    .await
            }
            HookRuntimeConfig::Http {
                url,
                method,
                headers,
            } => {
                self.invoke_http_runtime(entry, url, method, headers, invocation)
                    .await
            }
        }
    }

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
                reason: format!("failed to spawn command hook: {}", err),
            })?;

        let payload =
            serde_json::to_vec(&invocation).map_err(|err| HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("failed to encode invocation payload: {}", err),
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
                    reason: format!("failed to write command hook stdin: {}", err),
                })?;
        }

        let output =
            child
                .wait_with_output()
                .await
                .map_err(|err| HookEngineError::ExecutionFailed {
                    hook_id: entry.id.clone(),
                    reason: format!("failed to read command hook output: {}", err),
                })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("command hook exited with {}: {}", output.status, stderr),
            });
        }

        serde_json::from_slice::<RuntimeHookResponse>(&output.stdout).map_err(|err| {
            HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("invalid command hook response: {}", err),
            }
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
                reason: format!("failed to encode invocation payload: {}", err),
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
                reason: format!("invalid HTTP method '{}': {}", method, err),
            }
        })?;

        let mut req = self
            .http_client
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
                reason: format!("HTTP hook request failed: {}", err),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("HTTP hook returned {}: {}", status, body),
            });
        }

        response.json::<RuntimeHookResponse>().await.map_err(|err| {
            HookEngineError::ExecutionFailed {
                hook_id: entry.id.clone(),
                reason: format!("invalid HTTP hook response: {}", err),
            }
        })
    }
}

#[async_trait::async_trait]
impl HookEngine for DefaultHookEngine {
    async fn execute(
        &self,
        invocation: HookInvocation,
        overrides: Option<&HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        let entries = self
            .effective_entries(overrides)?
            .into_iter()
            .filter(|entry| entry.enabled && entry.point == invocation.point)
            .collect::<Vec<_>>();

        if entries.is_empty() {
            return Ok(HookExecutionReport::empty());
        }

        let futures = entries
            .into_iter()
            .enumerate()
            .map(|(registration_index, entry)| {
                self.execute_one(entry, registration_index, invocation.clone())
            })
            .collect::<Vec<_>>();

        let mut outcomes = join_all(futures).await;

        outcomes.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.registration_index.cmp(&b.registration_index))
        });

        let mut merged = HookExecutionReport::empty();
        for outcome in outcomes {
            if let Some(decision) = outcome.decision.clone() {
                merged.decision = Some(decision);
            }
            merged.patches.extend(outcome.patches.clone());
            merged
                .published_patches
                .extend(outcome.published_patches.clone());
            merged.outcomes.push(outcome);
        }

        Ok(merged)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{HookFailurePolicy, HookLlmRequest, HookPoint, HookReasonCode, SessionId};

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

    #[tokio::test]
    async fn deterministic_merge_by_priority_then_registration() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("hook-a"),
                point: HookPoint::PreLlmRequest,
                priority: 10,
                runtime: HookRuntimeConfig::InProcess {
                    name: "a".to_string(),
                    config: None,
                },
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("hook-b"),
                point: HookPoint::PreLlmRequest,
                priority: 5,
                runtime: HookRuntimeConfig::InProcess {
                    name: "b".to_string(),
                    config: None,
                },
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
            runtime: HookRuntimeConfig::InProcess {
                name: "pre-bg".to_string(),
                config: None,
            },
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
            runtime: HookRuntimeConfig::InProcess {
                name: "post-bg".to_string(),
                config: None,
            },
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

        let report = engine
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

        assert!(report.patches.is_empty());
        assert_eq!(report.published_patches.len(), 1);
    }

    #[tokio::test]
    async fn deterministic_merge_ignores_completion_order() {
        let mut config = HooksConfig::default();
        config.entries = vec![
            HookEntryConfig {
                id: HookId::new("slow-low-priority"),
                point: HookPoint::PreLlmRequest,
                priority: 100,
                runtime: HookRuntimeConfig::InProcess {
                    name: "slow".to_string(),
                    config: None,
                },
                capability: HookCapability::Rewrite,
                ..Default::default()
            },
            HookEntryConfig {
                id: HookId::new("fast-high-priority"),
                point: HookPoint::PreLlmRequest,
                priority: 1,
                runtime: HookRuntimeConfig::InProcess {
                    name: "fast".to_string(),
                    config: None,
                },
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
            runtime: HookRuntimeConfig::InProcess {
                name: "missing".to_string(),
                config: None,
            },
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
            runtime: HookRuntimeConfig::InProcess {
                name: "missing".to_string(),
                config: None,
            },
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
    async fn rewrite_with_fail_open_override_does_not_deny() {
        let mut config = HooksConfig::default();
        config.entries = vec![HookEntryConfig {
            id: HookId::new("rewrite-explicit-open"),
            point: HookPoint::PreToolExecution,
            capability: HookCapability::Rewrite,
            failure_policy: Some(HookFailurePolicy::FailOpen),
            runtime: HookRuntimeConfig::InProcess {
                name: "missing".to_string(),
                config: None,
            },
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
            runtime: HookRuntimeConfig::InProcess {
                name: "bg".to_string(),
                config: None,
            },
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
}
