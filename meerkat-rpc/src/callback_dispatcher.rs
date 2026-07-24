//! Callback tool dispatcher — routes tool calls to an external client via JSON-RPC.
//!
//! When the agent invokes a callback tool, the dispatcher sends a `tool/execute`
//! request to the client over the RPC transport and awaits the response. This
//! enables Python/TypeScript SDKs to provide tool implementations.
//!
//! The tool list is **live**: it reads from a shared `Arc<RwLock<Vec<ToolDef>>>`
//! (the same one `tools/register` writes to), so tools registered after session
//! materialization become visible at the next turn boundary via
//! `poll_external_updates`.
//!
//! Per-session inline tools (from `session/create` `external_tools` param) are
//! held separately and take precedence on name collision with globals.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};

use serde::Serialize;
use serde_json::value::RawValue;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};

use crate::protocol::{RpcId, RpcOutboundAdmission, RpcOutboundPermit, RpcRequest, RpcResponse};

const CALLBACK_REQUEST_MAX_PARAMS_BYTES: usize = 16 * 1024 * 1024;
const CALLBACK_REQUEST_ENVELOPE_BYTES: usize = 4 * 1024;
const CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES: usize = 1024;
const CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES: usize = 256;
pub(crate) const CALLBACK_PROCESS_MAX_PENDING: usize = 64;
const CALLBACK_JOB_INITIAL_LEASE: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
struct DetachedCallbackJobRuntime {
    realm_id: String,
    store: Arc<dyn meerkat::DetachedJobStore>,
    service: meerkat::DetachedJobService,
    blob_store: Arc<dyn meerkat_core::BlobStore>,
}

impl DetachedCallbackJobRuntime {
    fn new(
        realm_id: impl Into<String>,
        store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        Self {
            realm_id: realm_id.into(),
            service: meerkat::DetachedJobService::new(Arc::clone(&store)),
            store,
            blob_store,
        }
    }
}

#[derive(Clone)]
struct CallbackInvocationAdmission {
    semaphore: Arc<Semaphore>,
    max_pending: usize,
}

impl CallbackInvocationAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<CallbackInvocationAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| Self::new(CALLBACK_PROCESS_MAX_PENDING))
            .clone()
    }

    fn new(max_pending: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_pending)),
            max_pending,
        }
    }

    fn try_acquire(&self) -> Result<OwnedSemaphorePermit, ToolError> {
        Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "callback request admission rejected at the {}-request process limit: {error}",
                    self.max_pending
                ))
            })
    }
}

/// Inbound callback response plus the process-memory/count reservations that
/// remain live while the callback consumer parses and projects the payload.
/// Rejected handoffs carry a bounded synthesized error and no reservation.
pub struct CallbackResponseHandoff {
    response: RpcResponse,
    _memory_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    _count_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl CallbackResponseHandoff {
    pub(crate) fn admitted(
        response: RpcResponse,
        memory_permit: tokio::sync::OwnedSemaphorePermit,
        count_permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            response,
            _memory_permit: Some(memory_permit),
            _count_permit: Some(count_permit),
        }
    }

    pub(crate) fn rejected(response: RpcResponse) -> Self {
        Self {
            response,
            _memory_permit: None,
            _count_permit: None,
        }
    }

    #[cfg(test)]
    fn unmetered_for_test(response: RpcResponse) -> Self {
        Self::rejected(response)
    }

    pub fn response(&self) -> &RpcResponse {
        &self.response
    }
}

/// Server→client callback request paired with the admitted response handoff.
/// The outbound permit remains attached through queueing and socket write;
/// callback invocation count admission is held separately by the dispatcher
/// until the response or timeout resolves.
pub struct CallbackRequestEnvelope {
    request: RpcRequest,
    response_tx: oneshot::Sender<CallbackResponseHandoff>,
    outbound_permit: Arc<RpcOutboundPermit>,
}

impl CallbackRequestEnvelope {
    fn new(
        request: RpcRequest,
        response_tx: oneshot::Sender<CallbackResponseHandoff>,
        outbound_permit: Arc<RpcOutboundPermit>,
    ) -> Self {
        Self {
            request,
            response_tx,
            outbound_permit,
        }
    }

    /// Split at the server transport boundary. The third tuple element must
    /// stay bound until `write_request` completes.
    pub(crate) fn into_parts(
        self,
    ) -> (
        RpcRequest,
        oneshot::Sender<CallbackResponseHandoff>,
        Arc<RpcOutboundPermit>,
    ) {
        (self.request, self.response_tx, self.outbound_permit)
    }
}

/// Timeout for waiting on a client tool response.
/// Public callback deadline. Hosts and transports add their own bounded
/// handoff margins (125s and 130s respectively).
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
struct RegisteredCallbackTool {
    tool: ToolDef,
    execution: meerkat_core::ToolExecutionContract,
}

#[derive(Default)]
struct CallbackToolRegistryState {
    tools: Vec<RegisteredCallbackTool>,
}

/// Live callback-tool registry whose semantic mutation and epoch advancement
/// share one lock/authority boundary.
pub struct CallbackToolRegistry {
    state: Arc<StdRwLock<CallbackToolRegistryState>>,
    generation: Arc<AtomicU64>,
}

impl Default for CallbackToolRegistry {
    fn default() -> Self {
        Self {
            state: Arc::new(StdRwLock::new(CallbackToolRegistryState::default())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl CallbackToolRegistry {
    pub(crate) fn replace_or_add(&self, replacements: Vec<ToolDef>) -> Result<usize, ()> {
        self.replace_or_add_with_contracts(
            replacements
                .into_iter()
                .map(|tool| (tool, meerkat_core::ToolExecutionContract::default()))
                .collect(),
        )
    }

    pub(crate) fn replace_or_add_with_contracts(
        &self,
        replacements: Vec<(ToolDef, meerkat_core::ToolExecutionContract)>,
    ) -> Result<usize, ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        let count = replacements.len();
        for (tool, execution) in replacements {
            let replacement = RegisteredCallbackTool { tool, execution };
            if let Some(existing) = state
                .tools
                .iter_mut()
                .find(|registered| registered.tool.name == replacement.tool.name)
            {
                *existing = replacement;
            } else {
                state.tools.push(replacement);
            }
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(count)
    }

    pub fn snapshot(&self) -> Vec<ToolDef> {
        self.state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .map(|registered| registered.tool.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn from_test_parts(tools: Arc<StdRwLock<Vec<ToolDef>>>, generation: Arc<AtomicU64>) -> Self {
        let tools = tools
            .read()
            .map(|tools| {
                tools
                    .iter()
                    .cloned()
                    .map(|tool| RegisteredCallbackTool {
                        tool,
                        execution: meerkat_core::ToolExecutionContract::default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            state: Arc::new(StdRwLock::new(CallbackToolRegistryState { tools })),
            generation,
        }
    }
}

/// Dispatches tool calls to an external client via the RPC callback protocol.
///
/// Holds a shared reference to the global registered-tools list so that tools
/// registered via `tools/register` after session creation are picked up
/// dynamically at each turn boundary (via `poll_external_updates`).
///
/// Per-session `inline_tools` (from `session/create` `external_tools` param) are
/// static and take precedence on name collision with globals. Most sessions have
/// no inline tools (the field is empty).
pub struct CallbackToolDispatcher {
    /// Live tool definitions — shared with `SessionRuntime::registered_tools_slot`.
    registered_tools: Arc<CallbackToolRegistry>,
    /// Per-session inline tools (static, usually empty). Win on name collision.
    inline_tools: Vec<ToolDef>,
    /// Inline tool names cached for fast collision lookup.
    inline_names: HashSet<String>,
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    detached_jobs: Option<DetachedCallbackJobRuntime>,
    /// Snapshot of global tool names from the last `poll_external_updates` (or creation).
    /// Used to detect additions/removals between polls.
    last_seen_globals: StdRwLock<HashSet<String>>,
}

impl CallbackToolDispatcher {
    /// Create a callback dispatcher backed by the live registered-tools list.
    ///
    /// `inline_tools` are per-session static tools (from `session/create`
    /// `external_tools` param). Pass empty vec for most sessions.
    #[cfg(test)]
    fn new(
        registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        Self::new_with_generation(
            registered_tools,
            Arc::new(AtomicU64::new(0)),
            callback_tx,
            id_counter,
            inline_tools,
        )
    }

    /// Create a callback dispatcher bound to the production registry epoch.
    #[cfg(test)]
    fn new_with_generation(
        registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
        registered_tools_generation: Arc<AtomicU64>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        Self::from_registry(
            Arc::new(CallbackToolRegistry::from_test_parts(
                registered_tools,
                registered_tools_generation,
            )),
            callback_tx,
            id_counter,
            inline_tools,
        )
    }

    /// Create a callback dispatcher bound to the inseparable production
    /// registry mutation/epoch authority.
    pub fn from_registry(
        registered_tools: Arc<CallbackToolRegistry>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        let inline_names: HashSet<String> =
            inline_tools.iter().map(|t| t.name.to_string()).collect();
        let last_seen_globals = registered_tools
            .state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .filter(|registered| !inline_names.contains(registered.tool.name.as_str()))
                    .map(|registered| registered.tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            registered_tools,
            inline_tools,
            inline_names,
            callback_tx,
            id_counter,
            detached_jobs: None,
            last_seen_globals: StdRwLock::new(last_seen_globals),
        }
    }

    pub fn from_registry_with_job_runtime(
        registered_tools: Arc<CallbackToolRegistry>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
        realm_id: impl Into<String>,
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        let mut dispatcher =
            Self::from_registry(registered_tools, callback_tx, id_counter, inline_tools);
        dispatcher.detached_jobs = Some(DetachedCallbackJobRuntime::new(
            realm_id, job_store, blob_store,
        ));
        dispatcher
    }

    fn next_id(&self) -> RpcId {
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        RpcId::Str(format!("srv-{n}"))
    }

    /// Current global tool names excluding inline collisions.
    fn current_global_names(&self) -> HashSet<String> {
        self.registered_tools
            .state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .filter(|registered| !self.inline_names.contains(registered.tool.name.as_str()))
                    .map(|registered| registered.tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_known(&self, name: &str) -> bool {
        if self.inline_names.contains(name) {
            return true;
        }
        self.registered_tools
            .state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .any(|registered| registered.tool.name == name)
            })
            .unwrap_or(false)
    }

    async fn submit_detached(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let runtime = self.detached_jobs.clone().ok_or_else(|| {
            ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            )
        })?;
        let origin_session_id = context.origin_session_id().cloned().ok_or_else(|| {
            ToolError::execution_failed(
                "detached callback dispatch requires runtime-owned session identity".to_string(),
            )
        })?;
        let interaction_lineage = context.interaction_lineage_id().ok_or_else(|| {
            ToolError::execution_failed(
                "detached callback dispatch requires runtime-owned interaction lineage".to_string(),
            )
        })?;
        let arguments_sha256 = plan.canonical_arguments_sha256().ok_or_else(|| {
            ToolError::execution_failed(
                "detached callback dispatch requires a root-fenced canonical argument digest"
                    .to_string(),
            )
        })?;
        let policy = match plan.kind() {
            meerkat_core::ResolvedExecutionKind::Detached(policy) => policy,
            _ => {
                return Err(ToolError::execution_failed(
                    "detached callback owner received a non-detached execution plan".to_string(),
                ));
            }
        };
        let arguments: serde_json::Value = serde_json::from_str(call.args.get())
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let arguments_hash = format_sha256(arguments_sha256);
        let lineage = interaction_lineage.to_string();
        let submission_key = callback_submission_key(
            &runtime.realm_id,
            &origin_session_id,
            &lineage,
            call,
            policy,
            &arguments_hash,
            &arguments,
        )?;
        let specification = runtime
            .blob_store
            .put_artifact(
                "application/vnd.meerkat.callback-arguments+json",
                call.args.get(),
            )
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "failed to persist detached callback specification: {error}"
                ))
            })?;
        let execution_intent_id = meerkat::ExecutionIntentId::from_string(format!(
            "intent:{lineage}:{}:{arguments_hash}",
            call.name
        ))
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let spec = meerkat::JobSpec::new(
            runtime.realm_id.clone(),
            origin_session_id,
            execution_intent_id,
            meerkat::InteractionLineageId::from_string(lineage)
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
            meerkat::ToolIdentity::new(call.name, policy.runner().version())
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
            meerkat::RunnerIdentity::new(policy.runner().name(), policy.runner().version())
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
            job_restart_class(policy.restart_class()),
            meerkat::CanonicalArgumentsHash::new(arguments_hash)
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
            meerkat::JobSubmissionKey::new(submission_key)
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        )
        .with_runner_specification_ref(
            meerkat::RunnerSpecificationRef::new(specification.blob_id.to_string())
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        )
        .with_credential_context_refs(match plan.credential_context_refs() {
            meerkat_core::ToolExecutionApplicability::Applicable(references) => references.clone(),
            meerkat_core::ToolExecutionApplicability::NotApplicable => Vec::new(),
        });
        let receipt = runtime
            .service
            .submit(spec)
            .await
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let wire_receipt = meerkat::project_job_receipt(receipt.clone());

        let callback_tx = self.callback_tx.clone();
        let id_counter = Arc::clone(&self.id_counter);
        tokio::spawn(async move {
            if let Err(error) =
                start_detached_callback_attempt(runtime, callback_tx, id_counter, receipt.job_id)
                    .await
            {
                tracing::warn!(%error, "detached callback attempt start did not complete");
            }
        });

        let content = serde_json::to_string(&wire_receipt).map_err(|error| {
            ToolError::execution_failed(format!("failed to encode detached job receipt: {error}"))
        })?;
        Ok(ToolResult::new(call.id.to_string(), content, false).into())
    }

    /// Reconcile host-visible attempts from committed authority without
    /// claiming, retrying, or otherwise advancing attempt/fence state.
    pub async fn reconcile_detached_jobs(&self) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.clone() else {
            return Ok(());
        };
        let jobs = runtime.store.list_all(10_000).await.map_err(|error| {
            format!("failed to enumerate detached callbacks for reconciliation: {error}")
        })?;
        let mut attempts = Vec::new();
        for job in jobs.into_iter().filter(|job| {
            job.spec.realm_id == runtime.realm_id && self.owns_detached_callback_spec(&job.spec)
        }) {
            match job.machine_state.lifecycle_phase {
                meerkat::JobPhase::Queued | meerkat::JobPhase::RetryScheduled => {}
                meerkat::JobPhase::Running | meerkat::JobPhase::WaitingExternal => {
                    let Some(attempt_id) = job.machine_state.current_attempt_id.as_deref() else {
                        return Err(format!(
                            "active detached callback {} has no committed attempt id",
                            job.job_id
                        ));
                    };
                    let Some(runner_handle) = job.machine_state.runner_handle.as_deref() else {
                        return Err(format!(
                            "active detached callback {} has no committed runner handle",
                            job.job_id
                        ));
                    };
                    let Some(lease_expires_at_ms) = job.machine_state.lease_expires_at_ms else {
                        return Err(format!(
                            "active detached callback {} has no committed lease",
                            job.job_id
                        ));
                    };
                    attempts.push(meerkat_contracts::CallbackJobReconcileAttempt {
                        authority: meerkat_contracts::JobAttemptAuthority {
                            job_id: job.job_id.to_string(),
                            attempt_id: attempt_id.to_string(),
                            fence: job.machine_state.current_fence,
                        },
                        runner: meerkat_contracts::JobRunner {
                            name: job.spec.runner.name().to_string(),
                            version: job.spec.runner.version().to_string(),
                        },
                        restart_class: wire_restart_class(job.spec.restart_class),
                        runner_handle: runner_handle.to_string(),
                        checkpoint_ref: job
                            .machine_state
                            .checkpoint_ref
                            .as_ref()
                            .map(ToString::to_string),
                        lease_expires_at_ms,
                    });
                }
                _ => {}
            }
        }
        if !attempts.is_empty() {
            let offered_attempts = attempts.clone();
            let offered = offered_attempts
                .iter()
                .map(|attempt| attempt.authority.clone())
                .collect::<Vec<_>>();
            let result: meerkat_contracts::CallbackJobReconcileResult = send_callback_request(
                self.callback_tx.clone(),
                Arc::clone(&self.id_counter),
                "callback/job/reconcile",
                &meerkat_contracts::CallbackJobReconcileParams { attempts },
            )
            .await?;
            if result
                .live_attempts
                .iter()
                .any(|live| !offered.contains(live))
            {
                return Err(
                    "callback/job/reconcile returned authority that was not offered".to_string(),
                );
            }
            let now_ms = unix_time_ms()?;
            for attempt in offered_attempts.iter().filter(|attempt| {
                attempt.lease_expires_at_ms < now_ms
                    && !result.live_attempts.contains(&attempt.authority)
            }) {
                let job_id =
                    meerkat::JobId::new(&attempt.authority.job_id).map_err(|e| e.to_string())?;
                let write = meerkat::AttemptWriteAuthority {
                    attempt_id: meerkat::AttemptId::new(&attempt.authority.attempt_id)
                        .map_err(|e| e.to_string())?,
                    fence: meerkat::FenceToken::new(attempt.authority.fence),
                };
                match runtime
                    .service
                    .observe_lease_expired(&job_id, write, now_ms)
                    .await
                {
                    Ok(_) => {}
                    Err(meerkat::DetachedJobError::StaleRevision { .. })
                    | Err(meerkat::DetachedJobError::InvalidTransition { .. }) => {
                        continue;
                    }
                    Err(error) => return Err(error.to_string()),
                }
                match attempt.restart_class {
                    meerkat_contracts::JobRestartClass::NonResumable => {
                        runtime
                            .service
                            .classify_worker_loss(&job_id, now_ms)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    meerkat_contracts::JobRestartClass::CheckpointResumable
                        if attempt.checkpoint_ref.is_none() =>
                    {
                        runtime
                            .service
                            .mark_needs_attention(
                                &job_id,
                                now_ms,
                                meerkat::JobFailureCode::new(
                                    "checkpoint_resume_missing_checkpoint",
                                )
                                .map_err(|error| error.to_string())?,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    meerkat_contracts::JobRestartClass::Adoptable
                    | meerkat_contracts::JobRestartClass::CheckpointResumable
                    | meerkat_contracts::JobRestartClass::Replayable => {
                        runtime
                            .service
                            .schedule_retry(&job_id, now_ms)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        self.start_due_detached_jobs().await
    }

    /// Start queued or due-retry callback jobs owned by the current callback
    /// registration. Each start still commits an explicit machine claim.
    pub async fn start_due_detached_jobs(&self) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.clone() else {
            return Ok(());
        };
        let now_ms = unix_time_ms()?;
        let jobs = runtime.store.list_all(10_000).await.map_err(|error| {
            format!("failed to enumerate detached callbacks for runnable work: {error}")
        })?;
        for job in jobs.into_iter().filter(|job| {
            job.spec.realm_id == runtime.realm_id
                && self.owns_detached_callback_spec(&job.spec)
                && (job.machine_state.lifecycle_phase == meerkat::JobPhase::Queued
                    || (job.machine_state.lifecycle_phase == meerkat::JobPhase::RetryScheduled
                        && job
                            .machine_state
                            .retry_due_at_ms
                            .is_some_and(|due_at_ms| now_ms >= due_at_ms)))
        }) {
            let runtime = runtime.clone();
            let callback_tx = self.callback_tx.clone();
            let id_counter = Arc::clone(&self.id_counter);
            tokio::spawn(async move {
                if let Err(error) =
                    start_detached_callback_attempt(runtime, callback_tx, id_counter, job.job_id)
                        .await
                {
                    tracing::warn!(%error, "runnable detached callback start did not complete");
                }
            });
        }
        Ok(())
    }

    fn owns_detached_callback_spec(&self, spec: &meerkat::JobSpec) -> bool {
        self.registered_tools
            .state
            .read()
            .map(|state| {
                state.tools.iter().any(|registered| {
                    registered.tool.name == spec.tool.name()
                        && registered
                            .execution
                            .detached_policy()
                            .is_some_and(|policy| {
                                policy.runner().name() == spec.runner.name()
                                    && policy.runner().version() == spec.runner.version()
                            })
                })
            })
            .unwrap_or(false)
    }

    /// Deliver a committed cancellation request to the host using the exact
    /// current attempt authority. This never claims work or advances a fence.
    pub async fn cancel_detached_job(&self, job_id: &meerkat::JobId) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.as_ref() else {
            return Ok(());
        };
        let Some(job) = runtime
            .store
            .get(job_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err(format!("detached callback job {job_id} does not exist"));
        };
        if job.spec.realm_id != runtime.realm_id
            || !self.owns_detached_callback_spec(&job.spec)
            || !matches!(
                job.machine_state.lifecycle_phase,
                meerkat::JobPhase::Running | meerkat::JobPhase::WaitingExternal
            )
            || !job.machine_state.cancel_requested
        {
            return Ok(());
        }
        let attempt_id = job
            .machine_state
            .current_attempt_id
            .as_deref()
            .ok_or_else(|| format!("active detached callback {job_id} has no attempt id"))?;
        let result: meerkat_contracts::CallbackJobCancelResult = send_callback_request(
            self.callback_tx.clone(),
            Arc::clone(&self.id_counter),
            "callback/job/cancel",
            &meerkat_contracts::CallbackJobCancelParams {
                authority: meerkat_contracts::JobAttemptAuthority {
                    job_id: job_id.to_string(),
                    attempt_id: attempt_id.to_string(),
                    fence: job.machine_state.current_fence,
                },
            },
        )
        .await?;
        if !result.accepted {
            return Err(format!("callback/job/cancel rejected job {job_id}"));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentToolDispatcher for CallbackToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut result: Vec<Arc<ToolDef>> = self
            .inline_tools
            .iter()
            .map(|t| Arc::new(t.clone()))
            .collect();
        if let Ok(global) = self.registered_tools.state.read() {
            for registered in &global.tools {
                if !self.inline_names.contains(registered.tool.name.as_str()) {
                    result.push(Arc::new(registered.tool.clone()));
                }
            }
        }
        result.into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut result: Vec<ToolCatalogEntry> = self
            .inline_tools
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_inline(Arc::new(tool.clone()), true)
                    .with_execution_contract(meerkat_core::ToolExecutionContract::default())
            })
            .collect();
        if let Ok(global) = self.registered_tools.state.read() {
            for registered in &global.tools {
                if !self.inline_names.contains(registered.tool.name.as_str()) {
                    result.push(callback_catalog_entry(
                        registered.tool.clone(),
                        registered.execution.clone(),
                    ));
                }
            }
        }
        result.into()
    }

    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        self.registered_tools.generation()
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        let resolved_context =
            resolution_context.with_deadline(meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::ToolInternal,
                CALLBACK_TIMEOUT,
            ))?;
        entry
            .execution
            .resolve_default(resolved_context.deadlines().clone())
            .map_err(Into::into)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.is_known(call.name) {
            return Err(ToolError::not_found(call.name));
        }

        // This permit remains local across queueing and the response timeout,
        // providing a process-global hard cap on both callback dispatch tasks
        // and entries in the server's pending-callback map.
        let _invocation_permit = CallbackInvocationAdmission::production().try_acquire()?;
        if call.id.len() > CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool_use_id exceeds the {CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES}-byte callback limit"
                ),
            ));
        }
        if call.name.len() > CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool name exceeds the {CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES}-byte callback limit"
                ),
            ));
        }
        if call.args.get().len() > CALLBACK_REQUEST_MAX_PARAMS_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool arguments exceed the {CALLBACK_REQUEST_MAX_PARAMS_BYTES}-byte callback limit"
                ),
            ));
        }

        #[derive(Serialize)]
        struct CallbackParams<'a> {
            tool_use_id: &'a str,
            name: &'a str,
            arguments: &'a RawValue,
        }

        let request_id = self.next_id();
        let params = CallbackParams {
            tool_use_id: call.id,
            name: call.name,
            arguments: call.args,
        };
        let (params_raw, outbound_permit) = RpcOutboundAdmission::production()
            .admit_json(
                &params,
                CALLBACK_REQUEST_MAX_PARAMS_BYTES,
                CALLBACK_REQUEST_ENVELOPE_BYTES,
            )
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "callback request rejected by outbound admission: {error}"
                ))
            })?;

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(request_id),
            method: "tool/execute".to_string(),
            params: Some(params_raw),
        };

        let (response_tx, response_rx) = oneshot::channel();

        self.callback_tx
            .send(CallbackRequestEnvelope::new(
                request,
                response_tx,
                outbound_permit,
            ))
            .await
            .map_err(|_| ToolError::execution_failed("Callback channel closed".to_string()))?;

        let response_handoff = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
            .await
            .map_err(|_| callback_timeout_error(call.name))?
            .map_err(|_| {
                ToolError::execution_failed("Callback response channel dropped".to_string())
            })?;

        let response = response_handoff.response();

        if let Some(error) = response.error.as_ref() {
            let message = error.message.clone();
            drop(response_handoff);
            return Err(ToolError::execution_failed(message));
        }

        let result_raw = response.result.as_deref().ok_or_else(|| {
            ToolError::execution_failed("Callback response missing result".to_string())
        })?;

        #[derive(serde::Deserialize)]
        struct CallbackResult {
            #[serde(default)]
            content: String,
            #[serde(default)]
            is_error: bool,
        }

        let parsed: CallbackResult = serde_json::from_str(result_raw.get()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse callback result: {e}"))
        })?;
        let outcome = ToolResult::new(call.id.to_string(), parsed.content, parsed.is_error).into();
        drop(response_handoff);
        Ok(outcome)
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        match plan.mode() {
            meerkat_core::ToolExecutionMode::Fast => self.dispatch(call).await,
            meerkat_core::ToolExecutionMode::Detached => {
                self.submit_detached(call, context, plan).await
            }
            meerkat_core::ToolExecutionMode::Streaming => Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            )),
        }
    }

    /// Detect global tools added/removed since last poll and emit deltas.
    /// Inline tools are static and never produce deltas.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        let current = self.current_global_names();
        let previous = self
            .last_seen_globals
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        if current == previous {
            return ExternalToolUpdate::default();
        }

        let mut notices = Vec::new();

        for name in current.difference(&previous) {
            notices.push(
                ExternalToolDelta::new(
                    format!("callback:{name}"),
                    ToolConfigChangeOperation::Add,
                    ExternalToolDeltaPhase::Applied,
                )
                .with_tool_count(Some(1)),
            );
        }

        for name in previous.difference(&current) {
            notices.push(ExternalToolDelta::new(
                format!("callback:{name}"),
                ToolConfigChangeOperation::Remove,
                ExternalToolDeltaPhase::Applied,
            ));
        }

        if let Ok(mut last) = self.last_seen_globals.write() {
            *last = current;
        }

        ExternalToolUpdate {
            notices,
            pending: Vec::new(),
        }
    }
}

fn callback_catalog_entry(
    tool: ToolDef,
    execution: meerkat_core::ToolExecutionContract,
) -> ToolCatalogEntry {
    let tool = Arc::new(tool);
    let entry = if let Some(provenance) = tool.provenance.clone() {
        ToolCatalogEntry::session_deferred(tool, true, provenance)
    } else {
        ToolCatalogEntry::session_inline(tool, true)
    };
    entry.with_execution_contract(execution)
}

pub(crate) fn execution_contract_from_wire(
    execution: Option<meerkat_contracts::CallbackToolExecution>,
) -> Result<meerkat_core::ToolExecutionContract, String> {
    let Some(execution) = execution else {
        return Ok(meerkat_core::ToolExecutionContract::default());
    };
    match execution {
        meerkat_contracts::CallbackToolExecution::Fast => {
            Ok(meerkat_core::ToolExecutionContract::default())
        }
        meerkat_contracts::CallbackToolExecution::Detached {
            runner,
            restart_class,
            idempotency_scope,
            submission_timeout_ms,
            credential_scopes,
        } => {
            let runner = meerkat_core::RunnerIdentity::new(runner.name, runner.version)
                .map_err(|error| error.to_string())?;
            let restart_class = match restart_class {
                meerkat_contracts::JobRestartClass::Adoptable => {
                    meerkat_core::RestartClass::Adoptable
                }
                meerkat_contracts::JobRestartClass::CheckpointResumable => {
                    meerkat_core::RestartClass::CheckpointResumable
                }
                meerkat_contracts::JobRestartClass::Replayable => {
                    meerkat_core::RestartClass::Replayable
                }
                meerkat_contracts::JobRestartClass::NonResumable => {
                    meerkat_core::RestartClass::NonResumable
                }
            };
            let idempotency_scope = match idempotency_scope {
                meerkat_contracts::JobIdempotencyScope::ToolCall => {
                    meerkat_core::IdempotencyScope::ToolCall
                }
                meerkat_contracts::JobIdempotencyScope::InteractionAndArguments => {
                    meerkat_core::IdempotencyScope::InteractionAndArguments
                }
                meerkat_contracts::JobIdempotencyScope::HostSemanticKey => {
                    meerkat_core::IdempotencyScope::HostSemanticKey
                }
            };
            let policy = meerkat_core::DetachedToolExecutionPolicy::new(
                runner,
                restart_class,
                idempotency_scope,
                std::time::Duration::from_millis(submission_timeout_ms),
            )
            .map_err(|error| error.to_string())?
            .with_credential_scopes(credential_scopes);
            meerkat_core::ToolExecutionContract::new(
                std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
                meerkat_core::ToolExecutionMode::Detached,
                None,
                Some(policy),
            )
            .map_err(|error| error.to_string())
        }
    }
}

fn format_sha256(digest: [u8; 32]) -> String {
    let mut encoded = String::with_capacity("sha256:".len() + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn callback_submission_key(
    realm_id: &str,
    origin_session_id: &meerkat_core::SessionId,
    interaction_lineage: &str,
    call: ToolCallView<'_>,
    policy: &meerkat_core::DetachedToolExecutionPolicy,
    arguments_hash: &str,
    arguments: &serde_json::Value,
) -> Result<String, ToolError> {
    let scope = match policy.idempotency_scope() {
        meerkat_core::IdempotencyScope::ToolCall => format!("tool-call:{}", call.id),
        meerkat_core::IdempotencyScope::InteractionAndArguments => {
            format!("interaction:{interaction_lineage}:arguments:{arguments_hash}")
        }
        meerkat_core::IdempotencyScope::HostSemanticKey => {
            let semantic_key = arguments
                .get("idempotency_key")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ToolError::invalid_arguments(
                        call.name,
                        "host_semantic_key execution requires a non-empty string idempotency_key",
                    )
                })?;
            format!("host-semantic:{semantic_key}")
        }
    };
    Ok(format!(
        "callback:{realm_id}:{origin_session_id}:{}:{}:{scope}",
        call.name,
        policy.runner().version(),
    ))
}

fn job_restart_class(class: meerkat_core::RestartClass) -> meerkat::RestartClass {
    match class {
        meerkat_core::RestartClass::Adoptable => meerkat::RestartClass::Adoptable,
        meerkat_core::RestartClass::CheckpointResumable => {
            meerkat::RestartClass::CheckpointResumable
        }
        meerkat_core::RestartClass::Replayable => meerkat::RestartClass::Replayable,
        meerkat_core::RestartClass::NonResumable => meerkat::RestartClass::NonResumable,
    }
}

fn wire_restart_class(class: meerkat::RestartClass) -> meerkat_contracts::JobRestartClass {
    match class {
        meerkat::RestartClass::Adoptable => meerkat_contracts::JobRestartClass::Adoptable,
        meerkat::RestartClass::CheckpointResumable => {
            meerkat_contracts::JobRestartClass::CheckpointResumable
        }
        meerkat::RestartClass::Replayable => meerkat_contracts::JobRestartClass::Replayable,
        meerkat::RestartClass::NonResumable => meerkat_contracts::JobRestartClass::NonResumable,
    }
}

async fn start_detached_callback_attempt(
    runtime: DetachedCallbackJobRuntime,
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    job_id: meerkat::JobId,
) -> Result<(), String> {
    let stored = runtime
        .store
        .get(&job_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("detached callback job {job_id} disappeared after submission"))?;
    let claimed_at_ms = unix_time_ms()?;
    let runnable = stored.machine_state.lifecycle_phase == meerkat::JobPhase::Queued
        || (stored.machine_state.lifecycle_phase == meerkat::JobPhase::RetryScheduled
            && stored
                .machine_state
                .retry_due_at_ms
                .is_some_and(|due_at_ms| claimed_at_ms >= due_at_ms));
    if !runnable {
        return Ok(());
    }

    let lease_expires_at_ms = claimed_at_ms
        .checked_add(u64::try_from(CALLBACK_JOB_INITIAL_LEASE.as_millis()).unwrap_or(u64::MAX))
        .ok_or_else(|| "detached callback lease timestamp overflowed".to_string())?;
    let runner_handle = format!(
        "callback:{job_id}:attempt:{}",
        stored.machine_state.attempt_count.saturating_add(1)
    );
    let claim = match runtime
        .service
        .claim_attempt(
            &job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new(format!("rpc-callback:{}", std::process::id()))
                    .map_err(|error| error.to_string())?,
                claimed_at_ms,
                lease_expires_at_ms,
                meerkat::RunnerHandleRef::new(runner_handle.clone())
                    .map_err(|error| error.to_string())?,
            ),
        )
        .await
    {
        Ok(claim) => claim,
        Err(meerkat::DetachedJobError::StaleRevision { .. })
        | Err(meerkat::DetachedJobError::InvalidTransition { .. }) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let specification_ref = stored
        .spec
        .runner_specification_ref
        .as_ref()
        .ok_or_else(|| format!("detached callback job {job_id} has no runner specification"))?;
    let specification = runtime
        .blob_store
        .get(&meerkat_core::BlobId::new(specification_ref.as_str()))
        .await
        .map_err(|error| error.to_string())?;
    let arguments: serde_json::Value =
        serde_json::from_str(&specification.data).map_err(|error| error.to_string())?;
    let credential_scopes = stored
        .spec
        .credential_context_refs
        .iter()
        .flat_map(|reference| match reference {
            meerkat_core::ToolCredentialContextRef::OwningProfile { required_scopes }
            | meerkat_core::ToolCredentialContextRef::AuthBinding {
                required_scopes, ..
            } => required_scopes.iter().cloned().collect::<Vec<_>>(),
        })
        .collect();
    let params = meerkat_contracts::CallbackJobStartParams {
        authority: meerkat_contracts::JobAttemptAuthority {
            job_id: job_id.to_string(),
            attempt_id: claim.attempt_id.to_string(),
            fence: claim.fence.get(),
        },
        runner: meerkat_contracts::JobRunner {
            name: stored.spec.runner.name().to_string(),
            version: stored.spec.runner.version().to_string(),
        },
        restart_class: wire_restart_class(stored.spec.restart_class),
        runner_handle: runner_handle.clone(),
        runner_specification_ref: Some(specification_ref.to_string()),
        arguments,
        credential_scopes,
        resume_checkpoint: claim.resume_checkpoint.as_ref().map(ToString::to_string),
    };
    let result: meerkat_contracts::CallbackJobStartResult =
        match send_callback_request(callback_tx, id_counter, "callback/job/start", &params).await {
            Ok(result) => result,
            Err(error) => {
                // A lost start response is ambiguous: the host may have
                // accepted the committed attempt. Preserve its exact
                // authority for reconciliation instead of terminalizing it.
                return Err(error);
            }
        };
    if !result.accepted || result.runner_handle != runner_handle {
        runtime
            .service
            .mark_needs_attention(
                &job_id,
                unix_time_ms().unwrap_or(claimed_at_ms),
                meerkat::JobFailureCode::new(if result.accepted {
                    "callback_runner_handle_mismatch"
                } else {
                    "callback_start_rejected"
                })
                .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn send_callback_request<P, R>(
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    method: &str,
    params: &P,
) -> Result<R, String>
where
    P: Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    let _invocation_permit = CallbackInvocationAdmission::production()
        .try_acquire()
        .map_err(|error| error.to_string())?;
    let request_id = RpcId::Str(format!(
        "srv-{}",
        id_counter.fetch_add(1, Ordering::Relaxed)
    ));
    let (params_raw, outbound_permit) = RpcOutboundAdmission::production()
        .admit_json(
            params,
            CALLBACK_REQUEST_MAX_PARAMS_BYTES,
            CALLBACK_REQUEST_ENVELOPE_BYTES,
        )
        .map_err(|error| error.to_string())?;
    let request = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(request_id),
        method: method.to_string(),
        params: Some(params_raw),
    };
    let (response_tx, response_rx) = oneshot::channel();
    callback_tx
        .send(CallbackRequestEnvelope::new(
            request,
            response_tx,
            outbound_permit,
        ))
        .await
        .map_err(|_| "callback channel closed".to_string())?;
    let handoff = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
        .await
        .map_err(|_| format!("{method} timed out"))?
        .map_err(|_| format!("{method} response channel dropped"))?;
    if let Some(error) = handoff.response().error.as_ref() {
        return Err(error.message.clone());
    }
    let result = handoff
        .response()
        .result
        .as_deref()
        .ok_or_else(|| format!("{method} response is missing result"))?;
    serde_json::from_str(result.get()).map_err(|error| error.to_string())
}

fn unix_time_ms() -> Result<u64, String> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "wall clock exceeds u64 milliseconds".to_string())
}

fn callback_timeout_error(name: &str) -> ToolError {
    ToolError::timeout(
        name,
        u64::try_from(CALLBACK_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{ToolProvenance, ToolSourceKind};
    use serde_json::value::RawValue;

    fn make_tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("Test tool {name}"),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        }
    }

    fn make_provenanced_tool_def(name: &str) -> ToolDef {
        ToolDef {
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: "callback".into(),
            }),
            ..make_tool_def(name)
        }
    }

    fn detached_execution_contract() -> meerkat_core::ToolExecutionContract {
        let policy = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("mobkit.callback", "v1").unwrap(),
            meerkat_core::RestartClass::CheckpointResumable,
            meerkat_core::IdempotencyScope::InteractionAndArguments,
            std::time::Duration::from_secs(5),
        )
        .unwrap()
        .with_credential_scopes(["device.execute"]);
        meerkat_core::ToolExecutionContract::new(
            std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn callback_dispatcher_sends_request_awaits_result() {
        let (callback_tx, mut callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let handle = tokio::spawn(async move {
            let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
            let call = ToolCallView {
                id: "tc-1",
                name: "search",
                args: &args,
            };
            dispatcher.dispatch(call).await
        });

        let (request, response_tx, _outbound_permit) =
            callback_rx.recv().await.unwrap().into_parts();
        assert_eq!(request.method, "tool/execute");
        assert_eq!(request.id, Some(RpcId::Str("srv-0".to_string())));
        let params: serde_json::Value =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params["tool_use_id"], "tc-1");
        assert_eq!(params["name"], "search");
        assert_eq!(params["arguments"], serde_json::json!({"q": "test"}));

        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        serde_json::json!({"content":"results here","is_error":false}),
                    )
                ))
                .is_ok()
        );

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.result.text_content(), "results here");
        assert!(!result.result.is_error);
    }

    #[tokio::test]
    async fn callback_dispatcher_timeout_returns_tool_error() {
        let (callback_tx, _callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("slow")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-2",
            name: "slow",
            args: &args,
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            dispatcher.dispatch(call),
        )
        .await;

        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[test]
    fn callback_catalog_and_resolver_declare_actual_internal_timeout() {
        let (callback_tx, _callback_rx) = mpsc::channel(10);
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("slow")]));
        let dispatcher =
            CallbackToolDispatcher::new(tools, callback_tx, Arc::new(AtomicU64::new(0)), vec![]);
        let catalog = dispatcher.tool_catalog();
        assert_eq!(
            catalog[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Fast
        );
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let deadlines = meerkat_core::ToolDeadlineChain::new(vec![
            meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_secs(600),
            ),
        ])
        .unwrap();
        let plan = dispatcher
            .resolve_execution_plan(
                ToolCallView {
                    id: "tc-resolve",
                    name: "slow",
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default(),
                &meerkat_core::ToolExecutionResolutionContext::new(deadlines),
            )
            .unwrap();

        assert_eq!(plan.deadlines().effective_timeout(), Some(CALLBACK_TIMEOUT));
        assert!(plan.deadlines().contributors().iter().any(|contributor| {
            contributor.owner() == meerkat_core::ToolDeadlineOwner::ToolInternal
                && contributor.timeout() == Some(CALLBACK_TIMEOUT)
        }));
    }

    #[test]
    fn registered_detached_execution_contract_survives_catalog_projection() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let (callback_tx, _callback_rx) = mpsc::channel(1);
        let dispatcher = CallbackToolDispatcher::from_registry(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
        );

        let catalog = dispatcher.tool_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Detached
        );
        let policy = catalog[0]
            .execution
            .detached_policy()
            .expect("detached callback policy");
        assert_eq!(policy.runner().name(), "mobkit.callback");
        assert_eq!(policy.runner().version(), "v1");
        assert_eq!(
            policy.restart_class(),
            meerkat_core::RestartClass::CheckpointResumable
        );
        assert_eq!(
            policy.idempotency_scope(),
            meerkat_core::IdempotencyScope::InteractionAndArguments
        );
        assert_eq!(
            policy.submission_timeout(),
            std::time::Duration::from_secs(5)
        );
        assert!(policy.credential_scopes().contains("device.execute"));
    }

    #[test]
    fn callback_wire_execution_maps_to_detached_core_contract() {
        let contract = execution_contract_from_wire(Some(
            meerkat_contracts::CallbackToolExecution::Detached {
                runner: meerkat_contracts::JobRunner {
                    name: "mobkit.callback".to_string(),
                    version: "v1".to_string(),
                },
                restart_class: meerkat_contracts::JobRestartClass::Adoptable,
                idempotency_scope: meerkat_contracts::JobIdempotencyScope::InteractionAndArguments,
                submission_timeout_ms: 2_500,
                credential_scopes: vec!["device.execute".to_string()],
            },
        ))
        .unwrap();

        assert_eq!(
            contract.default_mode(),
            meerkat_core::ToolExecutionMode::Detached
        );
        let policy = contract.detached_policy().unwrap();
        assert_eq!(policy.runner().name(), "mobkit.callback");
        assert_eq!(
            policy.restart_class(),
            meerkat_core::RestartClass::Adoptable
        );
        assert_eq!(
            policy.idempotency_scope(),
            meerkat_core::IdempotencyScope::InteractionAndArguments
        );
        assert_eq!(
            policy.submission_timeout(),
            std::time::Duration::from_millis(2_500)
        );
        assert!(policy.credential_scopes().contains("device.execute"));
    }

    #[test]
    fn callback_wire_execution_rejects_zero_submission_timeout() {
        let error = execution_contract_from_wire(Some(
            meerkat_contracts::CallbackToolExecution::Detached {
                runner: meerkat_contracts::JobRunner {
                    name: "mobkit.callback".to_string(),
                    version: "v1".to_string(),
                },
                restart_class: meerkat_contracts::JobRestartClass::Replayable,
                idempotency_scope: meerkat_contracts::JobIdempotencyScope::ToolCall,
                submission_timeout_ms: 0,
                credential_scopes: Vec::new(),
            },
        ))
        .unwrap_err();

        assert!(error.contains("greater than zero"));
    }

    #[tokio::test]
    async fn detached_callback_dispatch_commits_receipt_without_waiting_for_host_work() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let (callback_tx, mut callback_rx) = mpsc::channel(2);
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
                registry,
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
                "realm-a",
                job_store.clone(),
                blob_store,
            ));
        let args = RawValue::from_string(r#"{"target":"lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-detached",
            name: "durable",
            args: &args,
        };
        let context = meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::new(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0xfeed_beef),
            )),
        );
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(30),
                ),
            ])
            .unwrap(),
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context,
        )
        .unwrap();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan),
        )
        .await
        .expect("detached submit must not wait for host execution")
        .unwrap();
        let receipt: meerkat_contracts::JobReceipt =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(receipt.status, meerkat_contracts::JobPhase::Queued);
        assert!(!receipt.deduplicated);
        assert!(receipt.awaitable);

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(1), callback_rx.recv())
            .await
            .expect("attempt start should be queued")
            .expect("callback channel");
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/start");
        let start: meerkat_contracts::CallbackJobStartParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(start.authority.job_id, receipt.job_id);
        assert_eq!(start.runner.name, "mobkit.callback");
        assert_eq!(
            start.restart_class,
            meerkat_contracts::JobRestartClass::CheckpointResumable
        );
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobStartResult {
                            accepted: true,
                            runner_handle: start.runner_handle.clone(),
                        },
                    ),
                ))
                .is_ok()
        );

        let job_id = meerkat::JobId::new(receipt.job_id).unwrap();
        let snapshot = meerkat::DetachedJobStore::get(&*job_store, &job_id)
            .await
            .unwrap()
            .expect("durable job");
        assert!(
            snapshot
                .spec
                .interaction_lineage_id
                .as_str()
                .ends_with("feedbeef")
        );
        assert!(snapshot.spec.runner_specification_ref.is_some());
        assert_eq!(snapshot.machine_state.attempt_count, 1);
    }

    #[tokio::test]
    async fn lost_start_response_preserves_claim_for_reconciliation() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let (callback_tx, callback_rx) = mpsc::channel(1);
        drop(callback_rx);
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
                registry,
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
                "realm-a",
                job_store.clone(),
                blob_store,
            ));
        let args = RawValue::from_string(r#"{"target":"lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-lost-start",
            name: "durable",
            args: &args,
        };
        let context = meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::new(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0xfeed_f00d),
            )),
        );
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(30),
                ),
            ])
            .unwrap(),
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context,
        )
        .unwrap();
        let outcome =
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await
                .unwrap();
        let receipt: meerkat_contracts::JobReceipt =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let job_id = meerkat::JobId::new(receipt.job_id).unwrap();
        let stored = meerkat::DetachedJobStore::get(&*job_store, &job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.machine_state.lifecycle_phase,
            meerkat::JobPhase::Running
        );
        assert_eq!(stored.machine_state.attempt_count, 1);
        assert_eq!(stored.machine_state.current_fence, 1);
        assert!(stored.machine_state.terminal_kind.is_none());
    }

    #[tokio::test]
    async fn reconcile_reuses_committed_attempt_fence_lease_checkpoint_and_handle() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let session_id = meerkat_core::SessionId::new();
        let receipt = service
            .submit(
                meerkat::JobSpec::new(
                    "realm-a",
                    session_id,
                    meerkat::ExecutionIntentId::from_string("intent-reconcile").unwrap(),
                    meerkat::InteractionLineageId::from_string("lineage-reconcile").unwrap(),
                    meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                    meerkat::RunnerIdentity::new("mobkit.callback", "v1").unwrap(),
                    meerkat::RestartClass::Adoptable,
                    meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                    meerkat::JobSubmissionKey::new("reconcile-same-authority").unwrap(),
                )
                .with_runner_specification_ref(
                    meerkat::RunnerSpecificationRef::new("sha256:spec").unwrap(),
                ),
            )
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    10,
                    u64::MAX - 1,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        service
            .record_checkpoint(
                &receipt.job_id,
                meerkat::AttemptWriteAuthority::from(&claim),
                meerkat::CheckpointRef::new("checkpoint-original").unwrap(),
                11,
            )
            .await
            .unwrap();
        let before = service.get(&receipt.job_id).await.unwrap().unwrap();

        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            Arc::new(meerkat::MemoryBlobStore::new()),
        ));
        let reconcile = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            async move { dispatcher.reconcile_detached_jobs().await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/reconcile");
        let params: meerkat_contracts::CallbackJobReconcileParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params.attempts.len(), 1);
        let attempt = &params.attempts[0];
        assert_eq!(attempt.authority.attempt_id, claim.attempt_id.to_string());
        assert_eq!(attempt.authority.fence, claim.fence.get());
        assert_eq!(attempt.runner_handle, "host-handle-original");
        assert_eq!(
            attempt.checkpoint_ref.as_deref(),
            Some("checkpoint-original")
        );
        assert_eq!(attempt.lease_expires_at_ms, u64::MAX - 1);
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: vec![attempt.authority.clone()],
                        },
                    )
                ))
                .is_ok()
        );
        reconcile.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.attempt_count, before.attempt_count);
        assert_eq!(after.current_attempt_id, before.current_attempt_id);
        assert_eq!(after.current_fence, before.current_fence);
        assert_eq!(after.lease_expires_at_ms, before.lease_expires_at_ms);
        assert_eq!(after.checkpoint_ref, before.checkpoint_ref);
        assert_eq!(after.runner_handle, before.runner_handle);
    }

    #[tokio::test]
    async fn cancel_uses_committed_attempt_authority_without_advancing_fence() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let receipt = service
            .submit(meerkat::JobSpec::new(
                "realm-a",
                meerkat_core::SessionId::new(),
                meerkat::ExecutionIntentId::from_string("intent-cancel").unwrap(),
                meerkat::InteractionLineageId::from_string("lineage-cancel").unwrap(),
                meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                meerkat::RunnerIdentity::new("mobkit.callback", "v1").unwrap(),
                meerkat::RestartClass::Adoptable,
                meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                meerkat::JobSubmissionKey::new("cancel-same-authority").unwrap(),
            ))
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    10,
                    10_000,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        service.request_cancel(&receipt.job_id).await.unwrap();
        let before = service.get(&receipt.job_id).await.unwrap().unwrap();

        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            Arc::new(meerkat::MemoryBlobStore::new()),
        ));
        let cancelling = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            let job_id = receipt.job_id.clone();
            async move { dispatcher.cancel_detached_job(&job_id).await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/cancel");
        let params: meerkat_contracts::CallbackJobCancelParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params.authority.job_id, receipt.job_id.to_string());
        assert_eq!(params.authority.attempt_id, claim.attempt_id.to_string());
        assert_eq!(params.authority.fence, claim.fence.get());
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobCancelResult { accepted: true },
                    ),
                ))
                .is_ok()
        );
        cancelling.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.attempt_count, before.attempt_count);
        assert_eq!(after.current_attempt_id, before.current_attempt_id);
        assert_eq!(after.current_fence, before.current_fence);
        assert_eq!(after.lease_expires_at_ms, before.lease_expires_at_ms);
    }

    #[tokio::test]
    async fn expired_omitted_non_resumable_attempt_becomes_worker_lost_without_new_fence() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let receipt = service
            .submit(meerkat::JobSpec::new(
                "realm-a",
                meerkat_core::SessionId::new(),
                meerkat::ExecutionIntentId::from_string("intent-lost").unwrap(),
                meerkat::InteractionLineageId::from_string("lineage-lost").unwrap(),
                meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                meerkat::RunnerIdentity::new("mobkit.callback", "v1").unwrap(),
                meerkat::RestartClass::NonResumable,
                meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                meerkat::JobSubmissionKey::new("lost-no-replay").unwrap(),
            ))
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    1,
                    2,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            Arc::new(meerkat::MemoryBlobStore::new()),
        ));
        let reconciling = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            async move { dispatcher.reconcile_detached_jobs().await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: Vec::new(),
                        },
                    ),
                ))
                .is_ok()
        );
        reconciling.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.phase, meerkat::JobPhase::WorkerLost);
        assert_eq!(after.attempt_count, claim.attempt_count);
        assert_eq!(after.current_attempt_id, Some(claim.attempt_id));
        assert_eq!(after.current_fence, claim.fence);
    }

    #[test]
    fn callback_internal_expiry_is_typed_timeout() {
        assert!(matches!(
            callback_timeout_error("slow"),
            ToolError::Timeout {
                name,
                timeout_ms: 120_000,
            } if name == "slow"
        ));
    }

    #[tokio::test]
    async fn callback_identical_metadata_reregistration_requires_fresh_resolution() {
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add(vec![make_tool_def("replaceable")])
            .unwrap();
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry(
                Arc::clone(&registry),
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
            ));
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-reregister",
            name: "replaceable",
            args: &args,
        };
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(600),
                ),
            ])
            .unwrap(),
        );
        let dispatch_context = meerkat_core::ToolDispatchContext::default();
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &dispatch_context,
            &resolution_context,
        )
        .unwrap();

        registry
            .replace_or_add(vec![make_tool_def("replaceable")])
            .unwrap();

        let error = meerkat_core::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &dispatch_context,
            &plan,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));
        assert!(
            callback_rx.try_recv().is_err(),
            "a stale plan must be rejected before callback delivery"
        );
    }

    #[test]
    fn callback_invocation_admission_caps_pending_work_process_wide() {
        let admission = CallbackInvocationAdmission::new(1);
        let held = admission.try_acquire().expect("first callback is admitted");
        assert!(
            admission.try_acquire().is_err(),
            "a second pending callback must fail closed"
        );
        drop(held);
        assert!(admission.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn oversized_callback_arguments_never_enter_the_request_queue() {
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));
        let dispatcher =
            CallbackToolDispatcher::new(tools, callback_tx, Arc::new(AtomicU64::new(0)), vec![]);
        let args = RawValue::from_string(format!(
            "\"{}\"",
            "x".repeat(CALLBACK_REQUEST_MAX_PARAMS_BYTES)
        ))
        .unwrap();
        let result = dispatcher
            .dispatch(ToolCallView {
                id: "tc-oversized",
                name: "search",
                args: &args,
            })
            .await;

        assert!(result.is_err());
        assert!(matches!(
            callback_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn poll_external_updates_detects_additions_and_removals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry.replace_or_add(vec![make_tool_def("a")]).unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );

        // No change yet.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());

        // Add a tool.
        registry.replace_or_add(vec![make_tool_def("b")]).unwrap();
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:b");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);

        // Remove original tool.
        registry
            .state
            .write()
            .unwrap()
            .tools
            .retain(|registered| registered.tool.name != "a");
        registry.generation.fetch_add(1, Ordering::AcqRel);
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:a");
        assert_eq!(
            update.notices[0].operation,
            ToolConfigChangeOperation::Remove
        );

        // No change.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn tools_reflects_live_registered_list() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry.replace_or_add(vec![make_tool_def("x")]).unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );
        assert_eq!(dispatcher.tools().len(), 1);

        registry.replace_or_add(vec![make_tool_def("y")]).unwrap();
        assert_eq!(dispatcher.tools().len(), 2);

        registry.state.write().unwrap().tools.clear();
        registry.generation.fetch_add(1, Ordering::AcqRel);
        assert_eq!(dispatcher.tools().len(), 0);
    }

    #[tokio::test]
    async fn tool_catalog_reflects_late_registered_provenanced_tools() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add(vec![make_provenanced_tool_def("secret_lookup")])
            .unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );
        assert_eq!(
            dispatcher
                .tool_catalog()
                .iter()
                .map(|entry| entry.tool.name.clone())
                .collect::<Vec<_>>(),
            vec!["secret_lookup".to_string()]
        );

        registry
            .replace_or_add(vec![make_provenanced_tool_def("secret_audit")])
            .unwrap();

        let names = dispatcher
            .tool_catalog()
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect::<Vec<_>>();
        assert!(names.contains(&"secret_lookup".to_string()));
        assert!(names.contains(&"secret_audit".to_string()));
    }

    #[tokio::test]
    async fn inline_tools_take_precedence_over_globals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        // Global has "shared" and "global_only"
        let tools = Arc::new(StdRwLock::new(vec![
            make_tool_def("shared"),
            make_tool_def("global_only"),
        ]));
        // Inline has "shared" and "inline_only"
        let inline = vec![make_tool_def("shared"), make_tool_def("inline_only")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        let names: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        // Should have: shared (from inline), inline_only, global_only
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"shared".to_string()));
        assert!(names.contains(&"inline_only".to_string()));
        assert!(names.contains(&"global_only".to_string()));

        // "shared" appears exactly once (inline wins, global filtered)
        assert_eq!(names.iter().filter(|n| *n == "shared").count(), 1);
    }

    #[tokio::test]
    async fn tool_catalog_prefers_inline_winners_and_reports_exact_support() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![
            make_tool_def("shared"),
            make_provenanced_tool_def("global_only"),
        ]));
        let inline = vec![make_tool_def("shared"), make_tool_def("inline_only")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        assert!(
            dispatcher.tool_catalog_capabilities().exact_catalog,
            "callback dispatcher should expose an exact catalog"
        );

        let catalog = dispatcher.tool_catalog();
        let names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(names.len(), 3);
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "shared")
                .count(),
            1
        );
        assert!(names.contains(&"inline_only".to_string()));
        assert!(names.contains(&"global_only".to_string()));
        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "shared")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::InlineOnly
        ));
        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "global_only")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { .. }
        ));
    }

    #[tokio::test]
    async fn unprovenanced_registered_tools_stay_inline_only_in_the_catalog() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("global_only")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);
        let catalog = dispatcher.tool_catalog();

        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "global_only")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::InlineOnly
        ));
    }

    #[tokio::test]
    async fn inline_tools_do_not_produce_poll_deltas() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![]));
        let inline = vec![make_tool_def("static_tool")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        // Inline tool exists but should not produce a delta.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn dynamic_updates_work_with_inline_tools_present() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        let inline = vec![make_tool_def("my_inline")];

        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            inline,
        );
        assert_eq!(dispatcher.tools().len(), 1); // just inline

        // Register a global tool later.
        registry
            .replace_or_add(vec![make_tool_def("late_global")])
            .unwrap();
        assert_eq!(dispatcher.tools().len(), 2); // inline + late_global

        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:late_global");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);
    }
}
