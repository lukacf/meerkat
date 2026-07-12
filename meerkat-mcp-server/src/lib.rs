//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

#![allow(dead_code, unused_imports, clippy::expect_used, clippy::large_futures)]

mod runtime_ingress;
mod schedule_host;

#[cfg(test)]
use meerkat::SessionStore;
use meerkat::surface::{
    RequestContext, install_prepared_runtime_interrupt_handle_for_actor_slot,
    prepare_surface_session, request_action, split_runtime_backed_eager_create_request,
};
use meerkat::{
    AgentFactory, FactoryAgentBuilder, MachineServiceTurnCommitProtocol,
    MachineSessionArchiveProtocol, OutputSchema, PersistenceBundle, PersistentSessionService,
    ScheduleService, ScheduleToolDispatcher, ToolError, ToolResult,
};
use meerkat_contracts::{RequestLifecycle, SkillsParams};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions, SessionError, SessionService, SessionServiceHistoryExt, StartTurnRequest,
};
use meerkat_core::{
    AgentEvent, BlobId, Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy,
    ConfigRuntimeError, ConfigStore, EventEnvelope, FileConfigStore, HookRunOverrides, Provider,
    RealmSelection, RuntimeBootstrap, ToolCallView, ToolCategoryOverride, format_verbose_event,
};
use meerkat_mcp::{McpReloadTarget, McpRouter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::mpsc;

use futures::StreamExt;

fn runtime_driver_error_to_session_error(err: meerkat_runtime::RuntimeDriverError) -> SessionError {
    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
        err.to_string(),
    ))
}

fn surface_materialize_error_to_session_error(
    err: meerkat::surface::SurfaceRuntimeMaterializeError,
) -> SessionError {
    match err {
        meerkat::surface::SurfaceRuntimeMaterializeError::Session(err) => err,
        meerkat::surface::SurfaceRuntimeMaterializeError::RuntimeDriver(err) => {
            runtime_driver_error_to_session_error(err)
        }
        other => SessionError::Agent(meerkat_core::error::AgentError::InternalError(
            other.to_string(),
        )),
    }
}

/// Minimal MCP-owned actor materialization shell.
///
/// The machine owns the exact claim and the session service mints the actor
/// witness. MCP retains neither the service turn boundary nor a post-commit
/// rollback receipt: once durable actor creation succeeds, cancellation may
/// leave a discoverable session that an explicit resume can reconstruct.
pub(crate) struct McpPreparedActorMaterialization {
    prepared: meerkat_runtime::PreparedSessionMaterialization,
    actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
    replaces_predecessor: bool,
}

impl McpPreparedActorMaterialization {
    fn new(
        prepared: meerkat_runtime::PreparedSessionMaterialization,
        actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
        replaces_predecessor: bool,
    ) -> Self {
        Self {
            prepared,
            actor_witness_slot,
            replaces_predecessor,
        }
    }

    pub(crate) fn actor_witness_slot(&self) -> &meerkat::LiveSessionActorWitnessSlot {
        &self.actor_witness_slot
    }

    pub(crate) fn replaces_predecessor(&self) -> bool {
        self.replaces_predecessor
    }
}

impl std::ops::Deref for McpPreparedActorMaterialization {
    type Target = meerkat_runtime::PreparedSessionMaterialization;

    fn deref(&self) -> &Self::Target {
        &self.prepared
    }
}

impl std::ops::DerefMut for McpPreparedActorMaterialization {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.prepared
    }
}

/// Explicitly replace the current MCP actor/executor incarnation.
///
/// Replacement uses the machine's exact unregister seam. The old actor is
/// discarded only through the witness captured before retirement; a
/// same-session successor is never removed by this operation.
async fn prepare_mcp_actor_materialization(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
) -> Result<McpPreparedActorMaterialization, ToolCallError> {
    let old_actor = match service
        .acquire_live_session_actor_turn_boundary_lease(session_id)
        .await
    {
        Ok(lease) => Some(lease.witness().clone()),
        Err(SessionError::NotFound { .. }) => None,
        Err(error) => {
            return Err(ToolCallError::internal(format!(
                "failed to inspect exact live actor before resuming {session_id}: {error}"
            )));
        }
    };

    // Explicit resume always establishes a new attachment incarnation. Even
    // after a process restart, when no live witness remains to unregister,
    // durable active inputs may still belong to the vanished attachment.
    // Replacement commit terminalizes those inputs before the new executor
    // can serve, so work is never silently transferred from A to B.
    let replaces_predecessor = true;
    if let Some(witness) = runtime_adapter
        .current_executor_attachment_witness(session_id)
        .await
    {
        let retired = runtime_adapter
            .unregister_executor_attachment_if_current(&witness)
            .await
            .map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to retire exact runtime attachment before resuming {session_id}: {error}"
                ))
            })?;
        if !retired {
            return Err(ToolCallError::internal(format!(
                "runtime attachment changed before explicit resume of {session_id}; retry resume"
            )));
        }
    } else if runtime_adapter
        .session_has_executor(session_id)
        .await
        .map_err(|error| ToolCallError::internal(error.to_string()))?
    {
        return Err(runtime_ingress::resume_required_tool_error(
            session_id,
            "runtime executor is transitional and has no exact attachment witness",
        ));
    }

    if let Some(old_actor) = old_actor
        && let Some(lease) = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&old_actor)
            .await
            .map_err(|error| ToolCallError::internal(error.to_string()))?
    {
        service
            .discard_live_session_actor(&lease)
            .await
            .map_err(|error| ToolCallError::internal(error.to_string()))?;
    }

    prepare_mcp_actor_materialization_claim(
        service,
        runtime_adapter,
        session_id,
        replaces_predecessor,
    )
    .await
}

async fn prepare_new_mcp_actor_materialization(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
) -> Result<McpPreparedActorMaterialization, ToolCallError> {
    let has_live_actor = match service
        .acquire_live_session_actor_turn_boundary_lease(session_id)
        .await
    {
        Ok(_lease) => true,
        Err(SessionError::NotFound { .. }) => false,
        Err(error) => {
            return Err(ToolCallError::internal(format!(
                "failed to inspect exact live actor before creating {session_id}: {error}"
            )));
        }
    };
    if has_live_actor
        || runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .is_some()
        || runtime_adapter
            .session_has_executor(session_id)
            .await
            .map_err(|error| ToolCallError::internal(error.to_string()))?
    {
        return Err(runtime_ingress::resume_required_tool_error(
            session_id,
            "new materialization raced an existing live incarnation",
        ));
    }
    prepare_mcp_actor_materialization_claim(service, runtime_adapter, session_id, false).await
}

async fn prepare_mcp_actor_materialization_claim(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
    replaces_predecessor: bool,
) -> Result<McpPreparedActorMaterialization, ToolCallError> {
    let mut prepared = runtime_adapter
        .prepare_session_materialization(session_id.clone())
        .await
        .map_err(|error| {
            ToolCallError::internal(format!(
                "failed to prepare exact actor materialization for {session_id}: {error}"
            ))
        })?;
    let actor_witness_slot = meerkat::LiveSessionActorWitnessSlot::default();
    if let Err(error) = install_prepared_runtime_interrupt_handle_for_actor_slot(
        service,
        runtime_adapter,
        prepared.bindings(),
        actor_witness_slot.clone(),
    )
    .await
    {
        let rollback = prepared.rollback_now().await.err();
        let rollback = rollback
            .map(|error| format!("; exact rollback also failed: {error}"))
            .unwrap_or_default();
        return Err(ToolCallError::internal(format!(
            "failed to install prepared runtime handles for {session_id}: {error}{rollback}"
        )));
    }
    Ok(McpPreparedActorMaterialization::new(
        prepared,
        actor_witness_slot,
        replaces_predecessor,
    ))
}

/// Release an uncommitted machine claim without trying to erase durable work.
/// If the actor already published, retain it as actor-unattached so discovery
/// and the explicit resume seam can recover it.
async fn release_mcp_actor_materialization(
    prepared: &mut Option<McpPreparedActorMaterialization>,
    session_id: &meerkat::SessionId,
    context: &str,
) -> Result<(), ToolCallError> {
    let Some(mut prepared) = prepared.take() else {
        return Ok(());
    };
    let result = if prepared.actor_witness_slot().witness().is_some() {
        prepared.commit_actor_unattached().await.map(|()| false)
    } else {
        prepared.rollback_now().await
    };
    result.map(|_| ()).map_err(|error| {
        ToolCallError::internal(format!(
            "{context} for session {session_id}; exact materialization release failed: {error}"
        ))
    })
}

#[derive(Debug)]
enum McpRuntimeBackedCreateError {
    Session {
        error: SessionError,
        preserves_runtime_session: bool,
    },
    CreatedSessionIdentityMismatch(SessionError),
    InitialTurnIdentityMismatch(SessionError),
}

async fn create_runtime_backed_session_and_run_initial_turn_prepared(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
    req: CreateSessionRequest,
    admission: Option<meerkat::RuntimeContextAdmissionGuard>,
    transaction: &McpPreparedActorMaterialization,
) -> Result<meerkat::RunResult, McpRuntimeBackedCreateError> {
    if transaction.session_id() != session_id {
        return Err(McpRuntimeBackedCreateError::Session {
            error: SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "MCP actor transaction belongs to {}, not {session_id}",
                transaction.session_id()
            ))),
            preserves_runtime_session: false,
        });
    }
    let (req, initial_turn) = split_runtime_backed_eager_create_request(req);
    let create_attempt = match admission {
        Some(admission) => {
            service
                .create_session_with_reserved_admission_and_actor_witness(
                    req,
                    admission,
                    transaction.actor_witness_slot(),
                )
                .await
        }
        None => {
            let admission = service.reserve_create_session_admission().await;
            match admission {
                Ok(admission) => {
                    service
                        .create_session_with_reserved_admission_and_actor_witness(
                            req,
                            admission,
                            transaction.actor_witness_slot(),
                        )
                        .await
                }
                Err(error) => Err(error),
            }
        }
    };
    let create_result = match create_attempt {
        Ok(result) => result,
        Err(error) => {
            let preserves_runtime_session = service
                .service_turn_error_requires_machine_terminal_receipt(session_id, &error)
                .await;
            return Err(McpRuntimeBackedCreateError::Session {
                error,
                preserves_runtime_session,
            });
        }
    };
    if &create_result.session_id != session_id {
        let returned_session_id = create_result.session_id.clone();
        return Err(McpRuntimeBackedCreateError::CreatedSessionIdentityMismatch(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "runtime-backed actor materialization returned session {returned_session_id}, expected {session_id}"
            ))),
        ));
    }

    let result = match initial_turn {
        Some(initial_turn) => match meerkat::surface::run_runtime_backed_initial_turn_with_machine(
            service,
            runtime_adapter,
            session_id,
            initial_turn,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                let error = surface_materialize_error_to_session_error(error);
                let preserves_runtime_session = service
                    .service_turn_error_requires_machine_terminal_receipt(session_id, &error)
                    .await;
                return Err(McpRuntimeBackedCreateError::Session {
                    error,
                    preserves_runtime_session,
                });
            }
        },
        None => create_result,
    };
    if &result.session_id != session_id {
        let returned_session_id = result.session_id;
        return Err(McpRuntimeBackedCreateError::InitialTurnIdentityMismatch(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "runtime-backed initial turn returned session {returned_session_id}, expected {session_id}"
            ))),
        ));
    }
    Ok(result)
}

struct McpCallLocalActorCreateOutcome {
    result: Result<meerkat::RunResult, McpRuntimeBackedCreateError>,
    transaction: McpPreparedActorMaterialization,
}

/// Call-local actor construction. There is deliberately no MCP-owned detached
/// rollback saga: the shared machine claim still performs its normal exact
/// provisional cleanup on drop, but it never archives durable creation.
/// Cancellation can lose the response while leaving the session discoverable;
/// explicit resume reconstructs any missing runtime pieces.
async fn create_runtime_backed_session_and_run_initial_turn_call_local(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
    req: CreateSessionRequest,
    admission: Option<meerkat::RuntimeContextAdmissionGuard>,
    transaction: McpPreparedActorMaterialization,
) -> Result<McpCallLocalActorCreateOutcome, ToolCallError> {
    let result = create_runtime_backed_session_and_run_initial_turn_prepared(
        service,
        runtime_adapter,
        session_id,
        req,
        admission,
        &transaction,
    )
    .await;
    Ok(McpCallLocalActorCreateOutcome {
        result,
        transaction,
    })
}

#[derive(Clone, Copy)]
pub(crate) enum McpActorMaterializationMode {
    Fresh,
    Resume,
}

pub(crate) struct McpActorMaterializationPlan {
    pub request: CreateSessionRequest,
    pub callback_config: runtime_ingress::McpCallbackConfig,
    pub router_to_publish: Option<Arc<meerkat_mcp::McpRouterAdapter>>,
}

pub(crate) struct McpActorMaterializationOutcome {
    pub result: Result<meerkat::RunResult, SessionError>,
    pub committed: bool,
}

/// The single MCP actor materialization transaction used by ordinary create,
/// explicit resume, and trusted autonomous owners. Product-specific callers
/// build the request/router plan, while this seam alone owns
/// prepare -> create -> exact attachment commit (or pre-commit release).
pub(crate) async fn materialize_mcp_actor_transaction<F, Fut>(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    ingress: &runtime_ingress::McpRuntimeIngressContext,
    session_id: &meerkat::SessionId,
    mode: McpActorMaterializationMode,
    admission: Option<meerkat::RuntimeContextAdmissionGuard>,
    build_plan: F,
) -> Result<McpActorMaterializationOutcome, ToolCallError>
where
    F: FnOnce(
        meerkat_core::SessionRuntimeBindings,
        runtime_ingress::McpLogicalConfigSnapshot,
    ) -> Fut,
    Fut: Future<Output = Result<McpActorMaterializationPlan, ToolCallError>>,
{
    let prepared = match mode {
        McpActorMaterializationMode::Fresh => {
            prepare_new_mcp_actor_materialization(service, runtime_adapter, session_id).await?
        }
        McpActorMaterializationMode::Resume => {
            prepare_mcp_actor_materialization(service, runtime_adapter, session_id).await?
        }
    };
    let logical_config = ingress
        .logical_config_for_prepared_actor(session_id, &prepared)
        .await
        .map_err(|error| {
            ToolCallError::internal(format!(
                "failed to snapshot MCP configuration for exact materialization {session_id}: {error}"
            ))
        });
    let logical_config = match logical_config {
        Ok(config) => config,
        Err(error) => {
            let mut prepared = Some(prepared);
            release_mcp_actor_materialization(
                &mut prepared,
                session_id,
                "exact MCP configuration snapshot failed",
            )
            .await?;
            return Err(error);
        }
    };
    let plan = match build_plan(prepared.bindings_clone(), logical_config.clone()).await {
        Ok(plan) => plan,
        Err(error) => {
            let mut prepared = Some(prepared);
            release_mcp_actor_materialization(
                &mut prepared,
                session_id,
                "MCP materialization plan failed",
            )
            .await?;
            return Err(error);
        }
    };
    let McpActorMaterializationPlan {
        request,
        callback_config,
        router_to_publish,
    } = plan;
    let create = create_runtime_backed_session_and_run_initial_turn_call_local(
        service,
        runtime_adapter,
        session_id,
        request,
        admission,
        prepared,
    )
    .await?;
    let McpCallLocalActorCreateOutcome {
        result,
        transaction,
    } = create;
    let (result, error_preserves_runtime_session) = match result {
        Ok(result) => (Ok(result), false),
        Err(McpRuntimeBackedCreateError::Session {
            error,
            preserves_runtime_session,
        }) => (Err(error), preserves_runtime_session),
        Err(McpRuntimeBackedCreateError::CreatedSessionIdentityMismatch(error)) => {
            let mut prepared = Some(transaction);
            release_mcp_actor_materialization(
                &mut prepared,
                session_id,
                "MCP materialization returned a mismatched session identity",
            )
            .await?;
            return Err(ToolCallError::internal(format!("Agent error: {error}")));
        }
        Err(McpRuntimeBackedCreateError::InitialTurnIdentityMismatch(error)) => {
            let mut prepared = Some(transaction);
            release_mcp_actor_materialization(
                &mut prepared,
                session_id,
                "MCP materialization initial turn returned a mismatched session identity",
            )
            .await?;
            return Err(ToolCallError::internal(format!("Agent error: {error}")));
        }
    };

    if result.is_err() && !error_preserves_runtime_session {
        let mut prepared = Some(transaction);
        release_mcp_actor_materialization(
            &mut prepared,
            session_id,
            "MCP materialization failed before a durable actor commit",
        )
        .await?;
        return Ok(McpActorMaterializationOutcome {
            result,
            committed: false,
        });
    }

    ingress
        .commit_prepared_session(session_id, transaction, callback_config, router_to_publish)
        .await
        .map_err(|error| {
            runtime_ingress::session_error_to_tool_error(
                error,
                format!("failed to commit exact MCP materialization for {session_id}"),
            )
        })?;
    Ok(McpActorMaterializationOutcome {
        result,
        committed: true,
    })
}

/// Legacy test-fixture helper for actors built without an MCP prepared claim.
/// Production paths must use the call-local transaction above.
#[cfg(test)]
async fn create_runtime_backed_session_and_run_initial_turn(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &meerkat::SessionId,
    req: CreateSessionRequest,
    admission: Option<meerkat::RuntimeContextAdmissionGuard>,
    actor_witness_slot: &meerkat::LiveSessionActorWitnessSlot,
) -> Result<meerkat::RunResult, McpRuntimeBackedCreateError> {
    let (req, initial_turn) = split_runtime_backed_eager_create_request(req);
    let admission = match admission {
        Some(admission) => Ok(admission),
        None => service.reserve_create_session_admission().await,
    }
    .map_err(|error| McpRuntimeBackedCreateError::Session {
        error,
        preserves_runtime_session: false,
    })?;
    let create_result = service
        .create_session_with_reserved_admission_and_actor_witness(
            req,
            admission,
            actor_witness_slot,
        )
        .await
        .map_err(|error| McpRuntimeBackedCreateError::Session {
            preserves_runtime_session: false,
            error,
        })?;
    if &create_result.session_id != session_id {
        return Err(McpRuntimeBackedCreateError::CreatedSessionIdentityMismatch(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "runtime-backed actor materialization returned session {}, expected {session_id}",
                create_result.session_id
            ))),
        ));
    }
    let result = match initial_turn {
        Some(initial_turn) => meerkat::surface::run_runtime_backed_initial_turn_with_machine(
            service,
            runtime_adapter,
            session_id,
            initial_turn,
        )
        .await
        .map_err(|error| McpRuntimeBackedCreateError::Session {
            error: surface_materialize_error_to_session_error(error),
            preserves_runtime_session: false,
        })?,
        None => create_result,
    };
    if &result.session_id != session_id {
        return Err(McpRuntimeBackedCreateError::InitialTurnIdentityMismatch(
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "runtime-backed initial turn returned session {}, expected {session_id}",
                result.session_id
            ))),
        ));
    }
    Ok(result)
}

/// Wrap the REAL source-identity record (projected from the skill runtime's
/// `SourceIdentityRegistry`) as wire provenance. The MCP surface must never
/// synthesize identity from display data; the registry record is the single
/// source of truth (mirrors `meerkat-rpc`'s `skills` handler).
fn skill_source_provenance(
    identity: meerkat_core::skills::SourceIdentityRecord,
) -> meerkat_contracts::SkillSourceProvenance {
    meerkat_contracts::SkillSourceProvenance { identity }
}

/// Project a registry-backed introspection entry into the wire `SkillEntry`,
/// failing closed when the typed source identity is absent rather than
/// fabricating a synthetic provenance record.
fn skill_entry_from_introspection(
    entry: &meerkat_core::skills::SkillIntrospectionEntry,
) -> Result<meerkat_contracts::SkillEntry, String> {
    let source_identity = entry.source_identity.clone().ok_or_else(|| {
        format!(
            "skill {} missing typed source identity",
            entry.descriptor.key
        )
    })?;
    Ok(meerkat_contracts::SkillEntry {
        key: entry.descriptor.key.clone(),
        name: entry.descriptor.name.clone(),
        description: entry.descriptor.description.clone(),
        scope: entry.descriptor.scope,
        source: skill_source_provenance(source_identity),
        is_active: entry.is_active,
        shadowed_by: entry
            .shadowed_by_identity
            .clone()
            .map(skill_source_provenance),
    })
}
use meerkat_client::TestClient;
use tokio::sync::Mutex;

/// Tool definition provided by the MCP client
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpToolDef {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: Value,
    /// Handler type: "callback" means the tool result will be provided via meerkat_resume
    #[serde(default)]
    pub handler: Option<String>,
}

impl McpToolDef {
    pub fn handler_kind(&self) -> &str {
        self.handler.as_deref().unwrap_or("callback")
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderInput {
    Anthropic,
    Openai,
    Gemini,
    Other,
}

impl ProviderInput {
    pub fn to_provider(self) -> Provider {
        match self {
            ProviderInput::Anthropic => Provider::Anthropic,
            ProviderInput::Openai => Provider::OpenAI,
            ProviderInput::Gemini => Provider::Gemini,
            ProviderInput::Other => Provider::Other,
        }
    }
}

fn resolve_mcp_create_session_model(
    config: &Config,
    model: Option<String>,
    provider: Option<ProviderInput>,
    auth_binding: Option<meerkat_core::AuthBindingRef>,
) -> Result<meerkat::CreateSessionModelResolution, meerkat::CreateSessionModelResolutionError> {
    meerkat::resolve_create_session_model(
        config,
        meerkat::CreateSessionModelResolutionRequest {
            model,
            provider: provider.map(ProviderInput::to_provider),
            auth_binding,
        },
    )
}

fn create_session_model_resolution_error_to_tool_error(
    error: meerkat::CreateSessionModelResolutionError,
) -> ToolCallError {
    if error.is_configuration_fault() {
        ToolCallError::internal(error.to_string())
    } else {
        ToolCallError::invalid_params(error.to_string())
    }
}

/// Input schema for meerkat_run tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatRunInput {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub model: Option<meerkat_core::lifecycle::run_primitive::ModelId>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation.
    /// Omit to use the product default.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Enable built-in tools (task management, shell, etc.).
    /// Omit to use the product default.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    /// Requires comms_name when enabled.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (name, description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable schedule tools.
    #[serde(default)]
    pub enable_schedule: Option<bool>,
    /// Enable WorkGraph tools.
    #[serde(default)]
    pub enable_workgraph: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default)]
    pub enable_web_search: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Explicit budget limits for this run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt — typed `SkillKey`s
    /// (source_uuid + skill_name).
    #[serde(default)]
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Key-value labels attached at session creation (e.g. workflow tagging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional system-level instructions prepended to the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context forwarded to the agent build pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Route this session's LLM calls through a realm-scoped provider
    /// binding. Typed `WireAuthBindingRef` referencing a
    /// `[realm.<realm>.binding.<binding>]` entry in the active Config.
    /// Pre-wave-c this was `Option<String>` parsed as `"realm:binding"`
    /// — the string form is now rejected at the deserialization
    /// boundary (dogma #5: no untyped joins on the ingress seam).
    /// When set, the provider runtime registry resolves the binding's
    /// auth profile and backend profile through the standard
    /// `ProviderRuntime::resolve` pipeline (Phase 4d.mcp.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<meerkat_contracts::WireAuthBindingRef>,
}

fn mcp_resume_requires_rebuild(input: &MeerkatResumeInput) -> bool {
    !input.tool_results.is_empty()
        || !input.tools.is_empty()
        || input.model.is_some()
        || input.provider.is_some()
        || input.max_tokens.is_some()
        || input.system_prompt.is_some()
        || input.output_schema.is_some()
        || input.structured_output_retries.is_some()
        || input.provider_params.is_some()
        || input.hooks_override.is_some()
        || input.enable_builtins.is_some()
        || input
            .builtin_config
            .as_ref()
            .is_some_and(|cfg| cfg.enable_shell.is_some())
        || input.enable_memory.is_some()
        || input.enable_schedule.is_some()
        || input.enable_workgraph.is_some()
        || input.enable_mob.is_some()
        || input.enable_web_search.is_some()
        || input.budget_limits.is_some()
        || input.preload_skills.is_some()
        || input.comms_name.is_some()
        || input.peer_meta.is_some()
}

/// Configuration options for built-in tools
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct BuiltinConfigInput {
    /// Enable shell tools. Omit to inherit/default.
    #[serde(default)]
    pub enable_shell: Option<bool>,
    /// Default timeout for shell commands in seconds (default: 30)
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
}

/// Actions supported by the config tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAction {
    Get,
    Set,
    Patch,
}

/// Input schema for meerkat_config tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatConfigInput {
    pub action: ConfigAction,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub patch: Option<Value>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
}

/// Structured MCP tool-call error payload.
#[derive(Debug, Clone)]
pub struct ToolCallError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl ToolCallError {
    fn new(code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message, None)
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message, None)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message, None)
    }

    fn internal_with_data(message: impl Into<String>, data: Value) -> Self {
        Self::new(-32603, message, Some(data))
    }
}

fn map_config_runtime_error(err: ConfigRuntimeError) -> ToolCallError {
    match err {
        ConfigRuntimeError::GenerationConflict { expected, current } => ToolCallError::new(
            -32602,
            format!("generation conflict: expected {expected}, current {current}"),
            Some(json!({
                "type": "generation_conflict",
                "expected_generation": expected,
                "current_generation": current
            })),
        ),
        other => ToolCallError::new(
            -32603,
            other.to_string(),
            Some(json!({ "type": "config_runtime_error" })),
        ),
    }
}

/// Shared filesystem realm-config source for the MCP server.
///
/// Mirrors REST: maps the reserved `global` realm to the home-rooted doc (or a
/// never-existent path when no HOME-rooted path exists, so `global` yields None
/// and behaves like a leaf realm) and every other realm to its per-realm config.
/// Surfaces inject this rather than re-deriving the projection.
fn mcp_realm_config_source(
    realms_root: &std::path::Path,
) -> Arc<dyn meerkat_core::RealmConfigSource> {
    let global_doc = meerkat_core::Config::global_config_path()
        .unwrap_or_else(|| realms_root.join("__no_global__").join("config.toml"));
    Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        realms_root.to_path_buf(),
        global_doc,
        meerkat_models::canonical(),
    ))
}

async fn load_config_async(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
    source: &Arc<dyn meerkat_core::RealmConfigSource>,
) -> Config {
    let store = match realm_config_store(
        realm_id,
        realms_root,
        backend_hint,
        origin_hint,
        instance_id,
    )
    .await
    {
        Ok(store) => store,
        Err(_) => return Config::default(),
    };
    let head_config = store.get().await.unwrap_or_else(|_| Config::default());
    // Fold the head realm's parent chain (head ⊕ ancestors ⊕ `global` tail) into
    // the effective config so top-level fields inherit before env overrides win.
    // A compose failure (e.g. malformed parent edge) falls back to the head
    // realm's own config rather than nuking everything to defaults.
    let mut config = match meerkat_core::EffectiveConfigReader::new(Arc::clone(source))
        .effective_config_over_head(realm_id, head_config.clone())
        .await
    {
        Ok(effective) => effective,
        Err(err) => {
            tracing::warn!("Failed to compose realm inheritance; using head config: {err}");
            head_config
        }
    };
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    if let Err(err) = config.validate(meerkat_models::canonical()) {
        tracing::warn!("Invalid realm config; using defaults: {}", err);
        return Config::default();
    }
    config
}

/// Resolve an explicit keep_alive override. Returns None when input is None (inherit).
fn resolve_keep_alive(requested: Option<bool>) -> Result<Option<bool>, String> {
    match requested {
        Some(true) => {
            #[cfg(feature = "comms")]
            {
                meerkat::surface::resolve_keep_alive(true).map(Some)
            }
            #[cfg(not(feature = "comms"))]
            {
                Err("keep_alive requires comms support (build with --features comms)".to_string())
            }
        }
        other => Ok(other), // None (inherit) or Some(false) (disable) pass through
    }
}

fn validate_public_peer_meta(peer_meta: Option<&meerkat_core::PeerMeta>) -> Result<(), String> {
    meerkat::surface::validate_public_peer_meta(peer_meta)
}

fn tagged_realm_config_store(
    realms_root: &std::path::Path,
    realm_id: &meerkat_core::connection::RealmId,
    backend: meerkat_store::RealmBackend,
    instance_id: Option<&str>,
) -> Arc<dyn ConfigStore> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id.as_str());
    let base: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
        paths.config_path.clone(),
        meerkat_models::canonical(),
    ));
    let tagged = meerkat_core::TaggedConfigStore::new(
        base,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(realm_id.to_string()),
            instance_id: instance_id.map(ToOwned::to_owned),
            backend: Some(backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: paths.config_path.display().to_string(),
                sessions_sqlite_path: Some(paths.sessions_sqlite_path.display().to_string()),
                sessions_jsonl_dir: paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    Arc::new(tagged)
}

async fn realm_config_store(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
) -> Result<Arc<dyn ConfigStore>, String> {
    let manifest = meerkat_store::ensure_realm_manifest_in(
        realms_root,
        realm_id.as_str(),
        backend_hint,
        origin_hint,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(tagged_realm_config_store(
        realms_root,
        realm_id,
        manifest.backend,
        instance_id,
    ))
}

fn realm_store_path(
    realms_root: &std::path::Path,
    realm_id: &meerkat_core::connection::RealmId,
    backend: meerkat_store::RealmBackend,
) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id.as_str());
    match backend {
        meerkat_store::RealmBackend::Jsonl => paths.sessions_jsonl_dir,
        meerkat_store::RealmBackend::Memory => paths.root,
        meerkat_store::RealmBackend::Sqlite => paths.root,
    }
}

/// Shared state for the MCP server, holding the session service.
///
/// The service is configured once with max-permissive factory flags
/// (`builtins: true`, `shell: true`). Per-request tool configuration is
/// controlled via `override_builtins` / `override_shell` in `SessionBuildOptions`.
pub struct MeerkatMcpState {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    schedule_service: ScheduleService,
    workgraph_service: meerkat::WorkGraphService,
    realm_id: meerkat_core::connection::RealmId,
    backend: String,
    instance_id: Option<String>,
    expose_paths: bool,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    /// Realm config source for composing the effective config on model-resolution
    /// read paths (create default-model), so an inherited (ancestor-realm) default
    /// model / self-hosted alias resolves like the agent build path.
    realm_config_source: Arc<dyn meerkat_core::RealmConfigSource>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    sidecars: runtime_ingress::SharedMcpSidecars,
    runtime_registration_locks: runtime_ingress::SharedMcpRuntimeRegistrationLocks,
    session_event_streams: Arc<Mutex<HashMap<McpStreamId, Arc<SessionEventStreamHandle>>>>,
    #[cfg(feature = "mob")]
    mob_event_streams: Arc<Mutex<HashMap<McpStreamId, Arc<MobEventStreamInner>>>>,
    schedule_host: StdMutex<Option<meerkat::surface::ScheduleHostHandle>>,
    /// Runtime adapter for comms drain lifecycle and runtime operations.
    #[allow(dead_code)] // Only used with `comms` feature
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    #[cfg(feature = "comms")]
    _realm_lease: Option<meerkat_store::RealmLeaseGuard>,
}

struct SessionEventStreamHandle {
    stream: Mutex<meerkat_core::EventStream>,
}

/// Typed owner of an MCP event-stream identity.
///
/// Stream identity is its own domain fact — it is NOT a session id (the
/// previous code minted stream ids through `SessionId::new()`, conflating two
/// identity spaces). A stream id is minted as a fresh UUID at stream open and
/// parsed fail-closed from the untrusted wire `stream_id` on read/close,
/// mirroring the RPC `StreamRef` ownership shape. The stream registries are
/// keyed by this type — there is no bare-string stream key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct McpStreamId(uuid::Uuid);

impl McpStreamId {
    fn mint() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Parse an untrusted wire `stream_id`, failing closed on malformed input.
    fn parse(raw: &str) -> Result<Self, String> {
        uuid::Uuid::parse_str(raw)
            .map(Self)
            .map_err(|e| format!("invalid stream_id '{raw}': {e}"))
    }
}

impl std::fmt::Display for McpStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(feature = "mob")]
enum MobEventStreamInner {
    /// Per-member agent event stream.
    Member(Mutex<meerkat_core::comms::EventStream>),
    /// Mob-wide attributed event stream.
    MobWide(Mutex<meerkat_mob::MobEventRouterHandle>),
}

impl MeerkatMcpState {
    pub(crate) fn runtime_ingress_context(&self) -> runtime_ingress::McpRuntimeIngressContext {
        runtime_ingress::McpRuntimeIngressContext::new(
            runtime_ingress::McpRuntimeIngressResources {
                service: Arc::clone(&self.service),
                runtime_adapter: Arc::clone(&self.runtime_adapter),
                workgraph_service: self.workgraph_service.clone(),
                config_runtime: Arc::clone(&self.config_runtime),
                realm_id: self.realm_id.clone(),
                instance_id: self.instance_id.clone(),
                backend: self.backend.clone(),
                sidecars: Arc::clone(&self.sidecars),
                runtime_registration_locks: Arc::clone(&self.runtime_registration_locks),
            },
        )
    }

    pub(crate) async fn clear_surface_bindings(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<(), SessionError> {
        self.runtime_ingress_context()
            .clear_session(session_id)
            .await
    }

    async fn cleanup_archived_session_runtime(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<(), SessionError> {
        self.clear_surface_bindings(session_id).await?;
        #[cfg(feature = "mob")]
        self.mob_state
            .destroy_bridge_session_mobs(&session_id.to_string())
            .await
            .map_err(|error| error.into_session_error("archived-session mob cleanup incomplete"))?;
        Ok(())
    }

    /// Create a new MCP state with a session service backed by `AgentFactory`.
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(RuntimeBootstrap::default(), false).await
    }

    pub async fn new_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(bootstrap, false).await
    }

    pub async fn new_with_bootstrap_and_options(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(bootstrap, expose_paths, None).await
    }

    #[doc(hidden)]
    pub async fn new_with_bootstrap_and_test_client(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(
            bootstrap,
            expose_paths,
            Some(Arc::new(TestClient::default())),
        )
        .await
    }

    async fn new_with_bootstrap_options_and_llm(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let locator = bootstrap.realm.resolve_locator()?;
        // Locator now carries the typed `RealmId` directly; no need to
        // reparse at the mcp-server boundary. Downstream `meerkat_store::*_in`
        // call sites obtain `&str` via `RealmId::as_str`.
        let realm_id = locator.realm;
        let realms_root = locator.state_root;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint);
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let realm_config_source = mcp_realm_config_source(&realms_root);
        let config = load_config_async(
            &realm_id,
            &realms_root,
            backend_hint,
            origin_hint,
            bootstrap.realm.instance_id.as_deref(),
            &realm_config_source,
        )
        .await;
        let (manifest, persistence) = meerkat::open_realm_persistence_in(
            &realms_root,
            realm_id.as_str(),
            backend_hint,
            origin_hint,
        )
        .await?;
        let store_path = persistence
            .store_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| realm_store_path(&realms_root, &realm_id, manifest.backend));
        let runtime_store = persistence.runtime_store();
        let session_store = persistence.session_store();
        let blob_store = persistence.blob_store();
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let workgraph_service = meerkat::WorkGraphService::with_scope(
            persistence.workgraph_store(),
            realm_id.as_str().to_owned(),
            meerkat::WorkNamespace::default(),
        );
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm_id.as_str());
        let conventions_context_root = bootstrap.context.context_root.clone();
        let project_root = conventions_context_root
            .clone()
            .unwrap_or_else(|| realm_paths.root.clone());
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            manifest.backend,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));
        let _lease = meerkat_store::start_realm_lease_in(
            &realms_root,
            realm_id.as_str(),
            bootstrap.realm.instance_id.as_deref(),
            "rkat-mcp",
        )
        .await?;

        // Create factory with max-permissive flags; per-request overrides
        // in SessionBuildOptions control what tools are actually enabled.
        let mut factory = AgentFactory::new(store_path)
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .project_root(project_root)
            .builtins(true)
            .shell(true)
            .workgraph(true)
            .schedule(true);
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await?;

        let max_sessions = config.max_sessions();
        let mut builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store)
            .with_realm_inheritance(Arc::clone(&realm_config_source), realm_id.clone());
        builder.default_llm_client = default_llm_client;
        #[cfg(feature = "mob")]
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        meerkat::surface::set_default_workgraph_tools(
            &builder,
            Some(Arc::new(meerkat::WorkGraphToolSurface::new(
                workgraph_service.clone(),
            ))),
        );
        let (service, runtime_adapter) = meerkat::surface::build_runtime_backed_service(
            builder,
            max_sessions,
            PersistenceBundle::new(session_store, runtime_store, blob_store),
        );
        let service = Arc::new(service);
        #[cfg(feature = "mob")]
        let mob_state = {
            let persistent_mob_root = realm_paths.root.clone();
            let state = Arc::new(
                meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                    service.clone(),
                    Some(runtime_adapter.clone()),
                    // A16: the local MCP console is the owning operator
                    // (phase 5 explicit mint, DEC-P5E-8).
                    meerkat_mob::MobControlPrincipal::Owner,
                )
                .with_persistent_storage_root(Some(persistent_mob_root))
                .with_workgraph_service(Some(workgraph_service.clone())),
            );
            *mob_tools_slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
                meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&state)),
            ));
            state
        };

        let state = Self {
            service,
            schedule_service,
            workgraph_service,
            realm_id,
            backend: manifest.backend.as_str().to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths,
            config_runtime,
            realm_config_source,
            skill_runtime,
            #[cfg(feature = "mob")]
            mob_state,
            sidecars: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "mob")]
            mob_event_streams: Arc::new(Mutex::new(HashMap::new())),
            schedule_host: StdMutex::new(None),
            runtime_adapter,
            #[cfg(feature = "comms")]
            _realm_lease: Some(_lease),
        };
        if let Err(error) = state.start_schedule_host() {
            tracing::warn!("schedule host failed to start: {error}");
        }
        Ok(state)
    }

    /// Test constructor that accepts an injected store (avoids opening redb at platform data dir).
    #[cfg(test)]
    pub(crate) async fn new_with_store(store: Arc<dyn SessionStore>) -> Self {
        Self::new_with_store_options(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_runtime_store(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
    ) -> Self {
        Self::new_with_store_options(store, runtime_store, None).await
    }

    #[cfg(test)]
    pub(crate) async fn new_with_store_and_max_sessions(
        store: Arc<dyn SessionStore>,
        max_sessions_override: Option<usize>,
    ) -> Self {
        Self::new_with_store_options(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            max_sessions_override,
        )
        .await
    }

    #[cfg(test)]
    async fn new_with_store_options(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
        max_sessions_override: Option<usize>,
    ) -> Self {
        Self::new_with_store_options_and_llm(store, runtime_store, max_sessions_override, None)
            .await
    }

    /// Test-only constructor with an optional explicit deterministic client.
    ///
    /// Persisted-session fixtures that exercise resume and attachment
    /// ownership can supply the client at the factory seam, keeping those
    /// tests hermetic while still allowing ownership tests to observe exactly
    /// when the first LLM turn starts. Other tests retain the production-like
    /// provider-resolution path by passing `None`.
    #[cfg(test)]
    async fn new_with_store_options_and_llm(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
        max_sessions_override: Option<usize>,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
    ) -> Self {
        let bootstrap = RuntimeBootstrap::default();
        let locator = match bootstrap.realm.resolve_locator() {
            Ok(locator) => locator,
            Err(_) => meerkat_core::RealmLocator {
                state_root: meerkat_core::default_state_root(),
                realm: meerkat_core::connection::RealmId::parse(meerkat_core::generate_realm_id())
                    .expect("generate_realm_id emits a valid slug by construction"),
            },
        };
        let realm_id = locator.realm.clone();
        let realms_root = locator.state_root;
        let realm_config_source = mcp_realm_config_source(&realms_root);
        let mut config = load_config_async(
            &realm_id,
            &realms_root,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
            bootstrap.realm.instance_id.as_deref(),
            &realm_config_source,
        )
        .await;
        if let Some(max_sessions) = max_sessions_override {
            config.limits.max_sessions = Some(max_sessions);
        }
        let store_path =
            realm_store_path(&realms_root, &realm_id, meerkat_store::RealmBackend::Sqlite);
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm_id.as_str());
        let project_root = realm_paths.root.clone();
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            meerkat_store::RealmBackend::Sqlite,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));

        let runtime_root = realm_paths.root.clone();
        let mut factory = AgentFactory::new(store_path)
            .runtime_root(runtime_root)
            .project_root(project_root)
            .builtins(true)
            .shell(true)
            .workgraph(true)
            .schedule(true);
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let max_sessions = config.max_sessions();
        let mut builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store)
            .with_realm_inheritance(Arc::clone(&realm_config_source), realm_id.clone());
        builder.default_llm_client = default_llm_client;
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(meerkat::MemoryScheduleStore::default()),
            )))),
        );
        let workgraph_service = meerkat::WorkGraphService::with_scope(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            realm_id.as_str().to_owned(),
            meerkat::WorkNamespace::default(),
        );
        meerkat::surface::set_default_workgraph_tools(
            &builder,
            Some(Arc::new(meerkat::WorkGraphToolSurface::new(
                workgraph_service.clone(),
            ))),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(
            meerkat_store::FsBlobStore::new(realm_paths.root.join("blobs")),
        );
        let (service, runtime_adapter) = meerkat::surface::build_runtime_backed_service(
            builder,
            max_sessions,
            PersistenceBundle::new(store, runtime_store, blob_store),
        );
        let service = Arc::new(service);

        let state = Self {
            service,
            schedule_service: ScheduleService::new(Arc::new(
                meerkat::MemoryScheduleStore::default(),
            )),
            workgraph_service,
            realm_id,
            backend: "sqlite".to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths: false,
            config_runtime,
            realm_config_source,
            skill_runtime: None,
            #[cfg(feature = "mob")]
            mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
            sidecars: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            runtime_registration_locks: Arc::new(StdMutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "mob")]
            mob_event_streams: Arc::new(Mutex::new(HashMap::new())),
            schedule_host: StdMutex::new(None),
            runtime_adapter,
            #[cfg(feature = "comms")]
            _realm_lease: None,
        };
        if let Err(error) = state.start_schedule_host() {
            tracing::warn!("schedule host failed to start: {error}");
        }
        state
    }

    pub fn realm_id(&self) -> &meerkat_core::connection::RealmId {
        &self.realm_id
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn expose_paths(&self) -> bool {
        self.expose_paths
    }

    async fn live_mcp_router_lease(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<runtime_ingress::McpLiveRouterLease, ToolCallError> {
        if self
            .service
            .load_authoritative_session(session_id)
            .await
            .map_err(|error| {
                runtime_ingress::session_error_to_tool_error(
                    error,
                    format!("failed to load session {session_id}"),
                )
            })?
            .is_none()
        {
            return Err(ToolCallError::new(
                meerkat_contracts::ErrorCode::SessionNotFound.jsonrpc_code(),
                format!("session not found: {session_id}"),
                Some(json!({ "session_id": session_id.to_string() })),
            ));
        }
        self.runtime_ingress_context()
            .live_router_lease(session_id)
            .await
            .map_err(|error| {
                runtime_ingress::session_error_to_tool_error(
                    error,
                    format!("failed to acquire live MCP router for {session_id}"),
                )
            })
    }
}

fn parse_backend_hint(raw: &str) -> Option<meerkat_store::RealmBackend> {
    match raw {
        "jsonl" => Some(meerkat_store::RealmBackend::Jsonl),
        "memory" => Some(meerkat_store::RealmBackend::Memory),
        "sqlite" => Some(meerkat_store::RealmBackend::Sqlite),
        _ => None,
    }
}

fn realm_origin_from_selection(selection: &RealmSelection) -> meerkat_store::RealmOrigin {
    match selection {
        RealmSelection::Explicit { .. } => meerkat_store::RealmOrigin::Explicit,
        RealmSelection::Isolated => meerkat_store::RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => meerkat_store::RealmOrigin::Workspace,
    }
}

/// Input schema for meerkat_resume tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatResumeInput {
    pub session_id: String,
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use (should match the original run)
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Tool results to provide for pending tool calls
    #[serde(default)]
    pub tool_results: Vec<ToolResultInput>,
    /// Enable built-in tools (task management, shell, etc.).
    /// Omit to inherit the resumed session's setting.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional model override for resume.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub model: Option<meerkat_core::lifecycle::run_primitive::ModelId>,
    /// Optional max_tokens override for resume.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional provider override for resume.
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation.
    /// Omit to inherit the resumed session's setting.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable schedule tools.
    #[serde(default)]
    pub enable_schedule: Option<bool>,
    /// Enable WorkGraph tools.
    #[serde(default)]
    pub enable_workgraph: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default)]
    pub enable_web_search: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Explicit budget limits for this resumed run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt — typed `SkillKey`s
    /// (source_uuid + skill_name).
    #[serde(default)]
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Optional per-turn tool overlay.
    #[serde(default)]
    pub turn_tool_overlay: Option<TurnToolOverlayInput>,
    /// Additional system-level instructions prepended to the prompt for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionIdInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionListInput {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// Filter sessions by labels (all specified k/v pairs must match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionHistoryInput {
    pub session_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatBlobGetInput {
    pub blob_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeerkatMcpAddInput {
    pub session_id: String,
    pub server_config: meerkat_core::McpServerConfig,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpRemoveInput {
    pub session_id: String,
    pub server_name: String,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpReloadInput {
    pub session_id: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeerkatSkillsAction {
    List,
    Inspect,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSkillsInput {
    pub action: MeerkatSkillsAction,
    /// Typed skill identity for the `inspect` action.
    #[serde(default)]
    pub skill_key: Option<meerkat_core::skills::SkillKey>,
    /// Optional source selector for inspect action.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamOpenInput {
    pub session_id: String,
}

/// Tri-state read-timeout policy for an event-stream read.
///
/// Owns the single semantic distinction that drives which timeout branch runs:
/// a finite caller timeout, an unbounded wait, or the surface default. The
/// legacy two-field wire form (`timeout_ms` + `no_timeout`) is collapsed into
/// this enum once at the deserialization boundary
/// ([`StreamReadTimeoutWire`]); consumers read [`StreamReadTimeout::duration`]
/// rather than re-deriving the policy by field precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamReadTimeout {
    /// No `timeout_ms` / `no_timeout` supplied: apply the surface default.
    #[default]
    Default,
    /// `no_timeout` requested: wait indefinitely for the next event.
    Infinite,
    /// `timeout_ms` supplied: apply this finite per-read timeout.
    Fixed { ms: u64 },
}

impl StreamReadTimeout {
    /// Resolve the policy to an optional wait duration. `None` means
    /// "wait indefinitely"; `Some` is the finite timeout for this read.
    #[must_use]
    pub fn duration(self) -> Option<std::time::Duration> {
        match self {
            Self::Infinite => None,
            Self::Fixed { ms } => Some(std::time::Duration::from_millis(ms)),
            Self::Default => Some(std::time::Duration::from_millis(
                DEFAULT_STREAM_READ_TIMEOUT_MS,
            )),
        }
    }
}

/// Legacy two-field wire form for [`StreamReadTimeout`]. Kept so the MCP tool
/// input schema and existing clients (which send `timeout_ms`/`no_timeout`)
/// continue to work unchanged; the precedence collapse happens here, once.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StreamReadTimeoutWire {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Disable timeout and wait indefinitely for the next event.
    #[serde(default)]
    pub no_timeout: bool,
}

impl From<StreamReadTimeoutWire> for StreamReadTimeout {
    fn from(wire: StreamReadTimeoutWire) -> Self {
        // Precedence (preserved from the prior consumer logic): an explicit
        // `no_timeout` wins over any `timeout_ms`; otherwise a supplied
        // `timeout_ms` is finite; otherwise the surface default applies.
        if wire.no_timeout {
            Self::Infinite
        } else {
            match wire.timeout_ms {
                Some(ms) => Self::Fixed { ms },
                None => Self::Default,
            }
        }
    }
}

/// `#[serde(flatten)]` carrier that deserializes the legacy
/// `timeout_ms`/`no_timeout` fields into the typed [`StreamReadTimeout`] policy
/// without changing the wire form. Consumers read `.policy.duration()`.
#[derive(Debug, Default)]
pub struct StreamReadTimeoutFlat {
    pub policy: StreamReadTimeout,
}

impl<'de> Deserialize<'de> for StreamReadTimeoutFlat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StreamReadTimeoutWire::deserialize(deserializer).map(|wire| Self {
            policy: wire.into(),
        })
    }
}

impl JsonSchema for StreamReadTimeoutFlat {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        StreamReadTimeoutWire::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        StreamReadTimeoutWire::json_schema(generator)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamReadInput {
    pub stream_id: String,
    /// Read-timeout policy. Deserialized from the legacy `timeout_ms` /
    /// `no_timeout` fields and flattened so the wire form is unchanged.
    #[serde(flatten)]
    #[schemars(flatten)]
    pub timeout: StreamReadTimeoutFlat,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamOpenInput {
    /// The mob to subscribe to.
    pub mob_id: String,
    /// Optional member ID. If provided, subscribes to that member's agent events only.
    /// If absent, subscribes to all mob-wide attributed events.
    #[serde(default)]
    pub member_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamReadInput {
    pub stream_id: String,
    /// Read-timeout policy. Deserialized from the legacy `timeout_ms` /
    /// `no_timeout` fields and flattened so the wire form is unchanged.
    #[serde(flatten)]
    #[schemars(flatten)]
    pub timeout: StreamReadTimeoutFlat,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMobEventStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsPeersInput {
    pub session_id: String,
}

pub type MeerkatCommsSendInput = meerkat_contracts::CommsSendParams;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BudgetLimitsInput {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
}

impl From<BudgetLimitsInput> for meerkat_core::BudgetLimits {
    fn from(value: BudgetLimitsInput) -> Self {
        meerkat_core::BudgetLimits {
            max_tokens: value.max_tokens,
            max_duration: value.max_duration_secs.map(std::time::Duration::from_secs),
            max_tool_calls: value.max_tool_calls,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TurnToolOverlayInput {
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
}

impl From<TurnToolOverlayInput> for meerkat_core::service::TurnToolOverlay {
    fn from(value: TurnToolOverlayInput) -> Self {
        Self {
            allowed_tools: value.allowed_tools.map(|names| {
                names
                    .into_iter()
                    .map(meerkat_core::ToolName::from)
                    .collect()
            }),
            blocked_tools: value.blocked_tools.map(|names| {
                names
                    .into_iter()
                    .map(meerkat_core::ToolName::from)
                    .collect()
            }),
            ..Default::default()
        }
    }
}

/// Tool result provided by the MCP client
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ToolResultInput {
    /// ID of the tool call this is a result for
    pub tool_use_id: String,
    /// Result content (or error message)
    pub content: String,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

pub type EventNotifier = Arc<dyn Fn(&str, &EventEnvelope<AgentEvent>) + Send + Sync>;
type EventEnvelopeSender = mpsc::Sender<EventEnvelope<AgentEvent>>;
type EventEnvelopeReceiver = mpsc::Receiver<EventEnvelope<AgentEvent>>;

fn spawn_event_forwarder(
    mut rx: EventEnvelopeReceiver,
    session_id: String,
    verbose: bool,
    notifier: Option<EventNotifier>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(ref notify) = notifier {
                notify(&session_id, &event);
            }

            if !verbose {
                continue;
            }

            if let Some(line) = format_verbose_event(&event.payload) {
                tracing::info!("{}", line);
            }
        }
    })
}

fn maybe_event_channel(
    verbose: bool,
    stream: bool,
) -> (Option<EventEnvelopeSender>, Option<EventEnvelopeReceiver>) {
    if !verbose && !stream {
        return (None, None);
    }
    let (tx, rx) = mpsc::channel(100);
    (Some(tx), Some(rx))
}

/// Format an agent run result (success, callback pending, or error) into an MCP response.
///
/// The `session_id` parameter is the pre-claimed ID, used as fallback when the
/// result is a `CallbackPending` error (which doesn't carry a session ID).
fn format_agent_result(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, String> {
    match result {
        Ok(result) => {
            let payload = json!({
                "content": [{
                    "type": "text",
                    "text": result.text
                }],
                "session_id": result.session_id.to_string(),
                "turns": result.turns,
                "tool_calls": result.tool_calls,
                "structured_output": result.structured_output,
                "extraction_error": result.extraction_error,
                "schema_warnings": result.schema_warnings,
                "skill_diagnostics": result.skill_diagnostics
            });
            wrap_tool_payload(payload)
        }
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending { tool_name, args })) => {
            // Pending state is a typed terminal-control fact, not a success
            // envelope. Serialize the canonical `WireCallbackPending` contract
            // rather than hand-building a success-looking JSON object so the
            // `status` discriminant + pending-tool list stay schema-aligned
            // across surfaces. The MCP human-readable `content` rides alongside
            // the contract fields.
            let pending = meerkat_contracts::WireCallbackPending::single(
                session_id.clone(),
                None,
                false,
                true,
                tool_name,
                args,
            );
            let mut payload = serde_json::to_value(&pending)
                .map_err(|err| format!("Failed to serialize callback pending contract: {err}"))?;
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "content".to_string(),
                    json!([{
                        "type": "text",
                        "text": "Agent is waiting for tool results"
                    }]),
                );
            }
            wrap_tool_payload(payload)
        }
        Err(e) => Err(format!("Agent error: {e}")),
    }
}

fn format_agent_result_tool(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, ToolCallError> {
    match result {
        Err(SessionError::Agent(meerkat::AgentError::Cancelled)) => {
            Err(request_cancelled_tool_error())
        }
        result => format_agent_result(result, session_id).map_err(ToolCallError::internal),
    }
}

fn build_callback_dispatcher(tools: &[McpToolDef]) -> Option<Arc<dyn AgentToolDispatcher>> {
    if tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(tools)) as Arc<dyn AgentToolDispatcher>)
    }
}

async fn compose_run_external_tool_dispatchers(
    state: &MeerkatMcpState,
    session_id: &meerkat::SessionId,
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, ToolCallError> {
    match compose_external_tool_dispatchers(primary, secondary) {
        Ok(tools) => Ok(tools),
        Err(error) => match state.clear_surface_bindings(session_id).await {
            Ok(()) => Err(ToolCallError::internal(error)),
            Err(cleanup) => Err(ToolCallError::internal(format!(
                "{error}; additionally failed to clear MCP surface bindings: {cleanup}"
            ))),
        },
    }
}

fn recoverable_callback_tool_defs(tools: &[McpToolDef]) -> Vec<ToolDef> {
    tools
        .iter()
        .filter(|tool| tool.handler_kind() == "callback")
        .map(|tool| ToolDef {
            name: tool.name.clone().into(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            provenance: Some(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Builtin,
                source_id: "mcp-server".into(),
            }),
        })
        .collect()
}

fn post_commit_session_created_error(
    session_id: &meerkat::SessionId,
    err: &SessionError,
) -> ToolCallError {
    if matches!(err, SessionError::Agent(meerkat::AgentError::Cancelled)) {
        return ToolCallError::new(
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code(),
            "request cancelled",
            Some(json!({
                "session_id": session_id.to_string(),
                "session_ref": session_id.to_string(),
                "session_created": true,
                "resumable": true,
            })),
        );
    }

    ToolCallError::internal_with_data(
        format!("Agent error: {err}"),
        json!({
            "session_id": session_id.to_string(),
            "session_ref": session_id.to_string(),
            "session_created": true,
            "resumable": true,
        }),
    )
}

fn request_cancelled_tool_error() -> ToolCallError {
    ToolCallError::new(
        meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code(),
        "request cancelled",
        None,
    )
}

async fn reject_if_cancelled_before_mcp_service_admission<F>(
    request_context: Option<&RequestContext>,
    cleanup: F,
) -> Result<(), ToolCallError>
where
    F: Future<Output = Result<(), ToolCallError>>,
{
    if request_context.is_some_and(RequestContext::cancel_already_requested) {
        cleanup.await?;
        Err(request_cancelled_tool_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
const DEFAULT_STREAM_READ_TIMEOUT_MS: u64 = 5;
#[cfg(not(test))]
const DEFAULT_STREAM_READ_TIMEOUT_MS: u64 = 5_000;

/// Typed descriptor for a Meerkat MCP tool.
///
/// The descriptor is the single owner of every fact about a base tool:
/// its wire `name`, the human-readable `description`, the `input_schema`
/// builder, and — co-located here rather than in a separate string-keyed
/// catalog — its [`RequestLifecycle`] classification. The rendered
/// `tools/list` JSON (see [`base_tools_list`]) and the lifecycle lookup (see
/// [`mcp_tool_request_lifecycle`]) are both read-only projections of this
/// single typed table, so a tool's lifecycle can never drift from where its
/// name/description/schema are declared.
pub struct BaseToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> Value,
    pub request_lifecycle: RequestLifecycle,
}

/// Empty input schema for tools that take no arguments.
fn empty_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "required": []
    })
}

/// The typed descriptor table for the base (always-present) MCP tools.
///
/// This is the authority for both the advertised descriptor JSON and the
/// per-tool request lifecycle. Tools contributed by other crates
/// (`schedule_tools_list`, `unscoped_workgraph_tools_list`, the mob/comms surfaces)
/// declare their lifecycle in [`contributed_tool_lifecycles`], keyed by the
/// surface's own advertised list.
fn base_tool_descriptors() -> Vec<BaseToolDescriptor> {
    vec![
        BaseToolDescriptor {
            name: "meerkat_run",
            description: "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            input_schema: || meerkat_tools::schema_for::<MeerkatRunInput>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_resume",
            description: "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            input_schema: || meerkat_tools::schema_for::<MeerkatResumeInput>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_help",
            description: "Ask how to use Meerkat. Runs a help session with the embedded meerkat-platform skill and returns the answer.",
            input_schema: || meerkat_tools::schema_for::<meerkat_contracts::HelpRequest>(),
            request_lifecycle: RequestLifecycle::LongRunningPublishOnSuccess,
        },
        BaseToolDescriptor {
            name: "meerkat_config",
            description: "Get or update Meerkat config for this MCP server instance.",
            input_schema: || meerkat_tools::schema_for::<MeerkatConfigInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_capabilities",
            description: "Get the list of capabilities available in this Meerkat runtime.",
            input_schema: empty_input_schema,
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_models_catalog",
            description: "Get the catalog of supported LLM models with provider grouping, tiers, and parameter schemas.",
            input_schema: empty_input_schema,
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_skills",
            description: "List or inspect available skills. Use action 'list' to see all skills, or 'inspect' with a typed skill_key and optional source UUID selector to see full content.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSkillsInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_read",
            description: "Read current session state.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_sessions",
            description: "List sessions in the active realm.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionListInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_history",
            description: "Read a session's full history.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionHistoryInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_blob_get",
            description: "Fetch raw blob bytes and metadata by blob id.",
            input_schema: || meerkat_tools::schema_for::<MeerkatBlobGetInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_interrupt",
            description: "Interrupt an in-flight turn for a session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_archive",
            description: "Archive (remove) a session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionIdInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_add",
            description: "Stage a live MCP server add operation on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpAddInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_remove",
            description: "Stage a live MCP server removal on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpRemoveInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_mcp_reload",
            description: "Stage a live MCP server reload on an active session.",
            input_schema: || meerkat_tools::schema_for::<MeerkatMcpReloadInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_open",
            description: "Open a session-level agent event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamOpenInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_read",
            description: "Read the next item from an open session-level event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamReadInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
        BaseToolDescriptor {
            name: "meerkat_event_stream_close",
            description: "Close a previously opened session-level event stream.",
            input_schema: || meerkat_tools::schema_for::<MeerkatSessionEventStreamCloseInput>(),
            request_lifecycle: RequestLifecycle::LongRunningObservation,
        },
    ]
}

fn base_tools_list() -> Vec<Value> {
    base_tool_descriptors()
        .into_iter()
        .map(|descriptor| {
            json!({
                "name": descriptor.name,
                "description": descriptor.description,
                "inputSchema": (descriptor.input_schema)(),
            })
        })
        .collect()
}

/// Feature-owned lifecycle declarations for contributed tool surfaces.
///
/// Each contributed surface composed into [`tools_list`] (schedule,
/// workgraph, mob host tools, mob event streams, comms) declares exactly one
/// [`RequestLifecycle`] for the tools it advertises. The name set is read
/// from the surface's own advertised list — the same projection
/// [`tools_list`] extends — so the lifecycle key set can never drift from
/// advertisement, and no name-string fall-through exists.
fn contributed_tool_lifecycles() -> Vec<(Vec<String>, RequestLifecycle)> {
    fn advertised_names(tools: Vec<Value>) -> Vec<String> {
        tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
            .collect()
    }
    #[allow(unused_mut)]
    let mut surfaces = vec![
        (
            advertised_names(meerkat::schedule_tools_list()),
            RequestLifecycle::LongRunningObservation,
        ),
        (
            advertised_names(meerkat::unscoped_workgraph_tools_list()),
            RequestLifecycle::LongRunningObservation,
        ),
    ];
    #[cfg(feature = "mob")]
    surfaces.push((
        advertised_names(mob_host_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    #[cfg(feature = "mob")]
    surfaces.push((
        advertised_names(mob_event_stream_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    #[cfg(feature = "comms")]
    surfaces.push((
        advertised_names(comms_tools_list()),
        RequestLifecycle::LongRunningObservation,
    ));
    surfaces
}

/// Resolve the [`RequestLifecycle`] for a tools/call by name.
///
/// Owner-of-record for MCP tool lifecycle: base tools read the
/// classification straight off the typed [`BaseToolDescriptor`] co-located
/// with the tool's name/description/schema; contributed tools read the
/// lifecycle their owning surface declares in
/// [`contributed_tool_lifecycles`]. Unknown names resolve to `None` — the
/// caller must fail closed (reject the call as an unknown tool) instead of
/// classifying an unadvertised name under a default lifecycle.
pub fn mcp_tool_request_lifecycle(tool_name: &str) -> Option<RequestLifecycle> {
    if let Some(descriptor) = base_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == tool_name)
    {
        return Some(descriptor.request_lifecycle);
    }
    contributed_tool_lifecycles()
        .into_iter()
        .find(|(names, _)| names.iter().any(|name| name == tool_name))
        .map(|(_, lifecycle)| lifecycle)
}

#[cfg(feature = "mob")]
fn mob_event_stream_tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_mob_event_stream_open",
            "description": "Open a mob-level event stream. If member_id is provided, streams that member's agent events. Otherwise streams all mob-wide attributed events.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_mob_event_stream_read",
            "description": "Read the next event from an open mob event stream (optional timeout).",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamReadInput>()
        }),
        json!({
            "name": "meerkat_mob_event_stream_close",
            "description": "Close a previously opened mob event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMobEventStreamCloseInput>()
        }),
    ]
}

#[cfg(feature = "mob")]
fn mob_host_tools_list() -> Vec<Value> {
    meerkat_mob_mcp::public_tools_list()
}

#[cfg(feature = "comms")]
fn comms_tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_comms_send",
            "description": "Send a canonical comms command to a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsSendInput>()
        }),
        json!({
            "name": "meerkat_comms_peers",
            "description": "List peers visible to a session's comms runtime.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsPeersInput>()
        }),
    ]
}

impl MeerkatMcpState {
    /// Advertised tools for THIS runtime instance.
    ///
    /// The static composition ([`tools_list`]) is filtered by the runtime
    /// capability seam: `meerkat_skills` advertises only when the resolved
    /// skill runtime exists, so the advertised availability can never split
    /// from the handler that would otherwise reject the call with
    /// "skills not enabled".
    pub fn advertised_tools_list(&self) -> Vec<Value> {
        let mut tools = tools_list();
        if self.skill_runtime.is_none() {
            tools.retain(|tool| tool.get("name").and_then(Value::as_str) != Some("meerkat_skills"));
        }
        tools
    }
}

/// Returns the full static list of tools composable by this MCP server.
///
/// Runtime-conditional availability (e.g. skills) is applied by
/// [`MeerkatMcpState::advertised_tools_list`]; serving this static catalog
/// directly would advertise tools the runtime cannot dispatch.
pub fn tools_list() -> Vec<Value> {
    let mut tools = base_tools_list();
    tools.extend(meerkat::schedule_tools_list());
    tools.extend(meerkat::unscoped_workgraph_tools_list());

    #[cfg(feature = "mob")]
    tools.extend(mob_host_tools_list());

    #[cfg(feature = "mob")]
    tools.extend(mob_event_stream_tools_list());

    #[cfg(feature = "comms")]
    tools.extend(comms_tools_list());

    tools
}

/// Handle a tools/call request
pub async fn handle_tools_call(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, ToolCallError> {
    Box::pin(handle_tools_call_with_notifier(
        state, tool_name, arguments, None, None,
    ))
    .await
}

/// Handle a tools/call request with optional event notifications.
pub async fn handle_tools_call_with_notifier(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    #[cfg(feature = "mob")]
    if meerkat_mob_mcp::public_tool_names().contains(&tool_name) {
        let payload =
            meerkat_mob_mcp::handle_public_tools_call(&state.mob_state, tool_name, arguments)
                .await
                .map_err(|err| ToolCallError {
                    code: err.code,
                    message: err.message,
                    data: err.data,
                })?;
        return wrap_tool_payload(payload).map_err(ToolCallError::internal);
    }

    match tool_name {
        "meerkat_help" => {
            let input: meerkat_contracts::HelpRequest = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            Box::pin(handle_meerkat_help(state, input, notifier, request_context)).await
        }
        "meerkat_run" => {
            let input: MeerkatRunInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            Box::pin(handle_meerkat_run(state, input, notifier, request_context)).await
        }
        "meerkat_resume" => {
            let input: MeerkatResumeInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            Box::pin(handle_meerkat_resume(
                state,
                input,
                notifier,
                request_context,
            ))
            .await
        }
        "meerkat_config" => {
            let input: MeerkatConfigInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_config(state, input)
                .await
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        "meerkat_capabilities" => handle_meerkat_capabilities(state)
            .await
            .map_err(ToolCallError::internal)
            .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal)),
        "meerkat_models_catalog" => handle_meerkat_models_catalog(state)
            .await
            .map_err(ToolCallError::internal)
            .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal)),
        "meerkat_skills" => {
            let input: MeerkatSkillsInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_skills(state, input)
                .await
                .map_err(ToolCallError::internal)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        "meerkat_read" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_sessions" => {
            let input: MeerkatSessionListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_sessions(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_history" => {
            let input: MeerkatSessionHistoryInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_history(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_blob_get" => {
            let input: MeerkatBlobGetInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_blob_get(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_interrupt" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_interrupt(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_archive" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_archive(state, input).await
        }
        "meerkat_mcp_add" => {
            let input: MeerkatMcpAddInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_add(state, input).await
        }
        "meerkat_mcp_remove" => {
            let input: MeerkatMcpRemoveInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_remove(state, input).await
        }
        "meerkat_mcp_reload" => {
            let input: MeerkatMcpReloadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_reload(state, input).await
        }
        "meerkat_event_stream_open" => {
            let input: MeerkatSessionEventStreamOpenInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_open(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_read" => {
            let input: MeerkatSessionEventStreamReadInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_close" => {
            let input: MeerkatSessionEventStreamCloseInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        name if name.starts_with("meerkat_schedule_") => {
            state
                .start_schedule_host()
                .map_err(|error| ToolCallError::internal(error.to_string()))?;
            meerkat::handle_schedule_tools_call(&state.schedule_service, name, arguments)
                .await
                .map_err(map_schedule_tool_error)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        name if name.starts_with("workgraph_") => {
            meerkat::handle_unscoped_workgraph_tools_call(&state.workgraph_service, name, arguments)
                .await
                .map_err(map_workgraph_tool_error)
                .and_then(|payload| wrap_tool_payload(payload).map_err(ToolCallError::internal))
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_open" => {
            let input: MeerkatMobEventStreamOpenInput = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            handle_meerkat_mob_event_stream_open(state, input).await
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_read" => {
            let input: MeerkatMobEventStreamReadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
            })?;
            handle_meerkat_mob_event_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "mob")]
        "meerkat_mob_event_stream_close" => {
            let input: MeerkatMobEventStreamCloseInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mob_event_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_send" => {
            let input: MeerkatCommsSendInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_send(state, input).await
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_peers" => {
            let input: MeerkatCommsPeersInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_peers(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
    }
}

async fn handle_meerkat_skills(
    state: &MeerkatMcpState,
    input: MeerkatSkillsInput,
) -> Result<Value, String> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| "skills not enabled".to_string())?;

    match input.action {
        MeerkatSkillsAction::List => {
            let entries = runtime
                .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
                .await
                .map_err(|e| format!("skill list failed: {e}"))?;
            let wire: Vec<meerkat_contracts::SkillEntry> = entries
                .iter()
                .map(skill_entry_from_introspection)
                .collect::<Result<_, _>>()?;
            serde_json::to_value(meerkat_contracts::SkillListResponse { skills: wire })
                .map_err(|e| format!("serialization failed: {e}"))
        }
        MeerkatSkillsAction::Inspect => {
            let skill_key = input
                .skill_key
                .as_ref()
                .ok_or_else(|| "missing 'skill_key' for inspect action".to_string())?;
            // Apply the identity registry remap chain before dispatch
            // so legacy source_uuids still resolve to the canonical
            // backing skill (C-4 invariant — registry-backed engines
            // apply remaps; others identity-project).
            let canonical = runtime
                .canonical_skill_key(skill_key)
                .await
                .map_err(|e| format!("skill canonicalization failed: {e}"))?;
            let doc = runtime
                .load_from_source(&canonical, input.source.as_deref())
                .await
                .map_err(|e| format!("skill inspect failed: {e}"))?;
            // Project the REAL provenance from the registry-backed introspection
            // listing (matching the canonical key) rather than fabricating one
            // from display data. Fail closed if the registry does not surface a
            // typed identity for this key.
            let provenance_entries = runtime
                .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
                .await
                .map_err(|e| format!("skill provenance lookup failed: {e}"))?;
            let source_identity = provenance_entries
                .into_iter()
                .find(|entry| entry.descriptor.key == doc.descriptor.key)
                .and_then(|entry| entry.source_identity)
                .ok_or_else(|| {
                    format!("skill {} missing typed source identity", doc.descriptor.key)
                })?;
            serde_json::to_value(meerkat_contracts::SkillInspectResponse {
                key: doc.descriptor.key.clone(),
                name: doc.descriptor.name.clone(),
                description: doc.descriptor.description.clone(),
                scope: doc.descriptor.scope,
                source: skill_source_provenance(source_identity),
                body: doc.body,
            })
            .map_err(|e| format!("serialization failed: {e}"))
        }
    }
}

async fn handle_meerkat_capabilities(state: &MeerkatMcpState) -> Result<Value, String> {
    // A ConfigError is a TRUE fault (benign absence is already Ok(default) inside the
    // store), so it must surface as an error, never as a phantom default-config payload.
    // Compose the realm chain so a capability gated on an inherited config field
    // reports the same status as the other surfaces.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| format!("Failed to read config: {e}"))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| format!("Failed to compose realm config chain: {e}"))?;
    let response = meerkat::surface::build_capabilities_response(&config);
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_models_catalog(state: &MeerkatMcpState) -> Result<Value, String> {
    // A ConfigError is a TRUE fault: an empty/default catalog must not masquerade
    // as success. Compose the realm chain so an inherited (ancestor-realm)
    // self-hosted alias / per-provider default is listed identically to the other
    // surfaces and matches what a session build resolves.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| format!("Failed to read config: {e}"))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| format!("Failed to compose realm config chain: {e}"))?;
    let response =
        meerkat::surface::build_models_catalog_response(&config).map_err(|e| e.to_string())?;
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_config(
    state: &MeerkatMcpState,
    input: MeerkatConfigInput,
) -> Result<Value, ToolCallError> {
    match input.action {
        ConfigAction::Get => {
            let snapshot = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Set => {
            let config_value = input.config.ok_or_else(|| {
                ToolCallError::invalid_params("config is required for action=set")
            })?;
            let config: Config = serde_json::from_value(config_value.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
            reject_config_payload_defaulting(&config_value, &config)?;
            validate_config_for_commit(&config)?;
            let snapshot = state
                .config_runtime
                .set(config, input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Patch => {
            let patch = input.patch.ok_or_else(|| {
                ToolCallError::invalid_params("patch is required for action=patch")
            })?;
            let current = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            let preview = apply_patch_preview(&current.config, patch.clone())?;
            validate_config_for_commit(&preview)?;
            let snapshot = state
                .config_runtime
                .patch(ConfigDelta(patch), input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
    }
}

fn config_envelope_value(
    snapshot: meerkat_core::ConfigSnapshot,
    expose_paths: bool,
) -> Result<Value, ToolCallError> {
    let policy = if expose_paths {
        ConfigEnvelopePolicy::Diagnostic
    } else {
        ConfigEnvelopePolicy::Public
    };
    serde_json::to_value(ConfigEnvelope::from_snapshot(snapshot, policy))
        .map_err(|e| ToolCallError::internal(e.to_string()))
}

fn validate_config_for_commit(config: &Config) -> Result<(), ToolCallError> {
    config
        .validate(meerkat_models::canonical())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
    config
        .skills
        .build_source_identity_registry()
        .map_err(|e| {
            ToolCallError::invalid_params(format!("Invalid skills source-identity config: {e}"))
        })?;
    Ok(())
}

fn reject_config_payload_defaulting(raw: &Value, config: &Config) -> Result<(), ToolCallError> {
    let typed = serde_json::to_value(config).map_err(|e| {
        ToolCallError::internal(format!(
            "Failed to serialize typed config for validation: {e}"
        ))
    })?;
    compare_config_payload_shape(raw, &typed, "config")
}

fn compare_config_payload_shape(
    raw: &Value,
    typed: &Value,
    path: &str,
) -> Result<(), ToolCallError> {
    match (raw, typed) {
        (Value::Object(raw_obj), Value::Object(typed_obj)) => {
            for key in raw_obj.keys() {
                if !typed_obj.contains_key(key) {
                    return Err(ToolCallError::invalid_params(format!(
                        "Invalid config: unknown field `{path}.{key}`"
                    )));
                }
            }
            for (key, typed_value) in typed_obj {
                let Some(raw_value) = raw_obj.get(key) else {
                    return Err(ToolCallError::invalid_params(format!(
                        "Invalid config: missing field `{path}.{key}`; action=set requires a complete typed config payload"
                    )));
                };
                compare_config_payload_shape(raw_value, typed_value, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
        (Value::Array(raw_items), Value::Array(typed_items)) => {
            for (index, (raw_item, typed_item)) in
                raw_items.iter().zip(typed_items.iter()).enumerate()
            {
                compare_config_payload_shape(raw_item, typed_item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn map_schedule_tool_error(error: meerkat::ScheduleToolError) -> ToolCallError {
    ToolCallError::new(error.code, error.message, error.data)
}

fn map_workgraph_tool_error(error: meerkat::WorkGraphToolError) -> ToolCallError {
    use meerkat::WorkGraphToolErrorCode;
    let code = match error.code {
        WorkGraphToolErrorCode::InvalidArguments
        | WorkGraphToolErrorCode::Conflict
        | WorkGraphToolErrorCode::InvalidTransition => {
            meerkat_contracts::ErrorCode::InvalidParams.jsonrpc_code()
        }
        WorkGraphToolErrorCode::NotFound => -32601,
        WorkGraphToolErrorCode::CapabilityUnavailable => {
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        }
        WorkGraphToolErrorCode::StoreError | WorkGraphToolErrorCode::InternalError => {
            meerkat_contracts::ErrorCode::InternalError.jsonrpc_code()
        }
    };
    ToolCallError::new(code, error.message, None)
}

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, ToolCallError> {
    // Single owner of RFC-7386 patch semantics: meerkat-core.
    meerkat_core::apply_config_patch_preview(config, patch)
        .map_err(|e| ToolCallError::invalid_params(format!("{e}")))
}

fn canonical_skill_keys(
    _config: &Config,
    skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    skill_references: Option<Vec<String>>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, String> {
    // Wave-b retypes removed the legacy stringly `skill_references` path:
    // `SkillsParams` now flattens only typed `skill_refs` into `SkillKey`
    // via `canonical_skill_keys()` (no registry lookup at wire boundary).
    // Reject any caller still supplying the legacy string form.
    if skill_references
        .as_ref()
        .is_some_and(|refs| !refs.is_empty())
    {
        return Err(
            "legacy `skill_references` string form is no longer supported; \
             pass typed `skill_refs` (source_uuid + skill_name) instead"
                .to_string(),
        );
    }
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
    };
    Ok(params.canonical_skill_keys())
}

async fn handle_meerkat_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let view = state
        .service
        .read(&session_id)
        .await
        .map_err(|e| format!("Failed to read session: {e}"))?;
    let payload = json!({
        "session_id": session_id.to_string(),
        "session_ref": meerkat_contracts::format_session_ref(&state.realm_id, &session_id),
        "state": view.state,
        "billing": view.billing
    });
    wrap_tool_payload(payload)
}

async fn handle_meerkat_interrupt(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    state
        .service
        .read(&session_id)
        .await
        .map_err(|e| format!("Failed to interrupt session: {e}"))?;
    match state
        .runtime_adapter
        .hard_cancel_current_run(&session_id, "MCP session interrupt")
        .await
    {
        Ok(()) => {}
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state })
            if interrupt_not_ready_is_noop(state) => {}
        Err(
            meerkat_runtime::RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Destroyed,
            }
            | meerkat_runtime::RuntimeDriverError::Destroyed,
        ) => {}
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => {
            return Err(format!(
                "Failed to interrupt session: runtime is not interruptible while {state}"
            ));
        }
        Err(e) => return Err(format!("Failed to interrupt session: {e}")),
    }
    wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "interrupted": true
    }))
}

fn interrupt_not_ready_is_noop(state: meerkat_runtime::RuntimeState) -> bool {
    matches!(
        state,
        meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
    )
}

async fn handle_meerkat_sessions(
    state: &MeerkatMcpState,
    input: MeerkatSessionListInput,
) -> Result<Value, String> {
    let query = meerkat_core::service::SessionQuery {
        limit: input.limit,
        offset: input.offset,
        labels: input.labels,
    };
    let sessions = state
        .service
        .list(query)
        .await
        .map_err(|e| format!("Failed to list sessions: {e}"))?;
    let wire_sessions: Vec<meerkat_contracts::WireSessionSummary> = sessions
        .into_iter()
        .map(|s| {
            let session_ref = meerkat_contracts::format_session_ref(&state.realm_id, &s.session_id);
            let mut wire = meerkat_contracts::WireSessionSummary::from(s);
            wire.session_ref = Some(session_ref);
            wire
        })
        .collect();
    let payload = json!({ "sessions": wire_sessions });
    wrap_tool_payload(payload)
}

async fn handle_meerkat_history(
    state: &MeerkatMcpState,
    input: MeerkatSessionHistoryInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let history = state
        .service
        .read_history(
            &session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: input.offset.unwrap_or(0),
                limit: input.limit,
            },
        )
        .await
        .map_err(|e| format!("Failed to read session history: {e}"))?;
    let mut payload: meerkat_contracts::WireSessionHistory = history.into();
    payload.session_ref = Some(meerkat_contracts::format_session_ref(
        &state.realm_id,
        &session_id,
    ));
    let payload_value = serde_json::to_value(payload)
        .map_err(|e| format!("Failed to serialize session history: {e}"))?;
    wrap_tool_payload(payload_value)
}

async fn handle_meerkat_blob_get(
    state: &MeerkatMcpState,
    input: MeerkatBlobGetInput,
) -> Result<Value, String> {
    let blob_id = BlobId::new(input.blob_id);
    let payload = state
        .service
        .blob_store()
        .get(&blob_id)
        .await
        .map_err(|err| err.to_string())?;
    let payload_value = serde_json::to_value(payload)
        .map_err(|e| format!("Failed to serialize blob payload: {e}"))?;
    wrap_tool_payload(payload_value)
}

#[derive(Clone)]
struct McpArchiveCleanup {
    ingress: runtime_ingress::McpRuntimeIngressContext,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
}

impl McpArchiveCleanup {
    async fn clear_surface_session(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<(), SessionError> {
        self.ingress.clear_session(session_id).await
    }

    async fn run(&self, session_id: &meerkat::SessionId) -> Result<(), SessionError> {
        self.clear_surface_session(session_id).await?;
        #[cfg(feature = "mob")]
        if let Err(error) = self
            .mob_state
            .destroy_bridge_session_mobs(&session_id.to_string())
            .await
        {
            return Err(error.into_session_error("mob cleanup during archive incomplete"));
        }
        Ok(())
    }
}

async fn archive_with_mcp_machine_authority(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    runtime_adapter: &meerkat_runtime::MeerkatMachine,
    session_id: &meerkat::SessionId,
) -> Result<(), SessionError> {
    service
        .archive_with_machine_protocol(
            session_id,
            MachineSessionArchiveProtocol::from_machine(runtime_adapter),
        )
        .await
}

async fn archive_session_with_runtime_cleanup(
    state: &MeerkatMcpState,
    session_id: meerkat::SessionId,
) -> Result<(), SessionError> {
    let service = Arc::clone(&state.service);
    let runtime_adapter = Arc::clone(&state.runtime_adapter);
    let cleanup = McpArchiveCleanup {
        ingress: state.runtime_ingress_context(),
        #[cfg(feature = "mob")]
        mob_state: Arc::clone(&state.mob_state),
    };
    let result_session_id = session_id.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        #[cfg(feature = "mob")]
        match cleanup
            .mob_state
            .archive_mob_owned_bridge_session_with_cleanup(
                &session_id,
                "mob cleanup during mob-owned archive incomplete",
            )
            .await
        {
            Ok(true) => {
                let result = cleanup.clear_surface_session(&session_id).await;
                let _ = result_tx.send(result);
                return;
            }
            Ok(false) => {}
            Err(error) => {
                let _ = result_tx.send(Err(error));
                return;
            }
        }

        let result = archive_with_mcp_machine_authority(
            service.as_ref(),
            runtime_adapter.as_ref(),
            &session_id,
        )
        .await;
        if matches!(result, Ok(()) | Err(SessionError::NotFound { .. }))
            && let Err(error) = cleanup.run(&session_id).await
        {
            let _ = result_tx.send(Err(error));
            return;
        }
        let _ = result_tx.send(result);
    });
    result_rx.await.map_err(|_| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "MCP archive task ended before reporting a result for {result_session_id}"
        )))
    })?
}

async fn handle_meerkat_archive(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, ToolCallError> {
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;
    archive_session_with_runtime_cleanup(state, session_id.clone())
        .await
        .map_err(archive_session_error_to_tool_error)?;
    wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "archived": true
    }))
    .map_err(ToolCallError::internal)
}

fn archive_session_error_to_tool_error(error: SessionError) -> ToolCallError {
    match error {
        SessionError::FailedWithData { message, data } => {
            ToolCallError::internal_with_data(format!("Failed to archive session: {message}"), data)
        }
        other => ToolCallError::internal(format!("Failed to archive session: {other}")),
    }
}

async fn handle_meerkat_mcp_add(
    state: &MeerkatMcpState,
    input: MeerkatMcpAddInput,
) -> Result<Value, ToolCallError> {
    let server_name = input.server_config.name.clone();
    if server_name.trim().is_empty() {
        return Err(ToolCallError::invalid_params("server_name cannot be empty"));
    }
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|error| ToolCallError::invalid_params(invalid_session_id_message(error)))?;
    let router_lease = state.live_mcp_router_lease(&session_id).await?;
    let adapter = router_lease.router();

    adapter
        .stage_add(input.server_config)
        .await
        .map_err(|e| ToolCallError::internal(format!("failed to stage add: {e}")))?;
    drop(router_lease);

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Add,
        Some(server_name),
        false,
    )
    .and_then(wrap_tool_payload)
    .map_err(ToolCallError::internal)
}

async fn handle_meerkat_mcp_remove(
    state: &MeerkatMcpState,
    input: MeerkatMcpRemoveInput,
) -> Result<Value, ToolCallError> {
    if input.server_name.trim().is_empty() {
        return Err(ToolCallError::invalid_params("server_name cannot be empty"));
    }
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|error| ToolCallError::invalid_params(invalid_session_id_message(error)))?;
    let router_lease = state.live_mcp_router_lease(&session_id).await?;
    let adapter = router_lease.router();
    adapter
        .stage_remove(input.server_name.clone())
        .await
        .map_err(|e| ToolCallError::internal(format!("failed to stage remove: {e}")))?;
    drop(router_lease);

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Remove,
        Some(input.server_name),
        false,
    )
    .and_then(wrap_tool_payload)
    .map_err(ToolCallError::internal)
}

async fn handle_meerkat_mcp_reload(
    state: &MeerkatMcpState,
    input: MeerkatMcpReloadInput,
) -> Result<Value, ToolCallError> {
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|error| ToolCallError::invalid_params(invalid_session_id_message(error)))?;
    let router_lease = state.live_mcp_router_lease(&session_id).await?;
    let adapter = router_lease.router();

    match input.server_name.clone() {
        Some(name) => {
            if name.trim().is_empty() {
                return Err(ToolCallError::invalid_params("server_name cannot be empty"));
            }
            adapter
                .stage_reload(McpReloadTarget::ServerName(name))
                .await
                .map_err(|e| ToolCallError::internal(format!("failed to stage reload: {e}")))?;
        }
        None => {
            // K14: the lifecycle owner's reload-all primitive stages the whole
            // roster under one router lock and returns the typed per-server
            // report; a non-clean report is an explicit fault naming both the
            // staged servers (whose reloads WILL still apply at the next
            // boundary) and the rejected ones.
            let report = adapter
                .stage_reload_all()
                .await
                .map_err(|e| ToolCallError::internal(format!("failed to stage reload-all: {e}")))?;
            if !report.is_clean() {
                let failed = report
                    .failed
                    .iter()
                    .map(|failure| format!("{}: {}", failure.server, failure.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ToolCallError::internal(format!(
                    "failed to stage reload for [{failed}]; reload already staged for [{}] and will still apply at the next boundary",
                    report.staged.join(", ")
                )));
            }
        }
    }
    drop(router_lease);

    mcp_live_response_value(
        input.session_id,
        meerkat_contracts::McpLiveOperation::Reload,
        input.server_name,
        false,
    )
    .and_then(wrap_tool_payload)
    .map_err(ToolCallError::internal)
}

fn mcp_live_response_value(
    session_id: String,
    operation: meerkat_contracts::McpLiveOperation,
    server_name: Option<String>,
    persisted: bool,
) -> Result<Value, String> {
    serde_json::to_value(meerkat_contracts::McpLiveOpResponse {
        session_id,
        operation,
        server_name,
        status: meerkat_contracts::McpLiveOpStatus::Staged,
        persisted,
        applied_at_turn: None,
    })
    .map_err(|error| format!("failed to serialize MCP live response: {error}"))
}

async fn handle_meerkat_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamOpenInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let stream = state
        .service
        .subscribe_session_events(&session_id)
        .await
        .map_err(|e| format!("Failed to open session event stream: {e}"))?;
    let stream_id = McpStreamId::mint();
    state.session_event_streams.lock().await.insert(
        stream_id,
        Arc::new(SessionEventStreamHandle {
            stream: Mutex::new(stream),
        }),
    );

    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "session_id": session_id.to_string()
    }))
}

async fn handle_meerkat_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamReadInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let stream_id_str = stream_id.to_string();
    let handle = state
        .session_event_streams
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| format!("Stream not found: {stream_id}"))?;

    let read_timeout = input.timeout.policy.duration();
    let next_event = {
        let mut stream = handle.stream.lock().await;
        match read_timeout {
            None => stream.next().await,
            Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    return stream_read_payload(
                        &stream_id_str,
                        meerkat_contracts::StreamReadStatus::Timeout,
                    );
                }
            },
        }
    };

    match next_event {
        Some(envelope) => {
            let status = stream_read_event_status(&envelope)?;
            stream_read_payload(&stream_id_str, status)
        }
        None => {
            state.session_event_streams.lock().await.remove(&stream_id);
            stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
        }
    }
}

async fn handle_meerkat_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamCloseInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let removed = state.session_event_streams.lock().await.remove(&stream_id);
    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "closed": removed.is_some()
    }))
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamOpenInput,
) -> Result<Value, ToolCallError> {
    let mob_id = meerkat_mob::MobId::from(input.mob_id.as_str());
    let stream_id = McpStreamId::mint();

    let inner = if let Some(member_id) = &input.member_id {
        let identity = meerkat_mob::AgentIdentity::from(member_id.as_str());
        let stream = state
            .mob_state
            .subscribe_agent_events(&mob_id, &identity)
            .await
            .map_err(|error| {
                let mapped = meerkat_mob_mcp::McpToolError::from_mob(&error);
                ToolCallError::new(mapped.code, mapped.message, mapped.data)
            })?;
        MobEventStreamInner::Member(Mutex::new(stream))
    } else {
        let router_handle = state
            .mob_state
            .subscribe_mob_events(&mob_id)
            .await
            .map_err(|error| {
                let mapped = meerkat_mob_mcp::McpToolError::from_mob(&error);
                ToolCallError::new(mapped.code, mapped.message, mapped.data)
            })?;
        MobEventStreamInner::MobWide(Mutex::new(router_handle))
    };

    state
        .mob_event_streams
        .lock()
        .await
        .insert(stream_id, Arc::new(inner));

    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "mob_id": input.mob_id,
        "member_id": input.member_id,
    }))
    .map_err(ToolCallError::internal)
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamReadInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let stream_id_str = stream_id.to_string();
    let handle = state
        .mob_event_streams
        .lock()
        .await
        .get(&stream_id)
        .cloned()
        .ok_or_else(|| format!("Mob event stream not found: {stream_id}"))?;

    let read_timeout = input.timeout.policy.duration();

    match handle.as_ref() {
        MobEventStreamInner::Member(stream_mutex) => {
            let next_event = {
                let mut stream = stream_mutex.lock().await;
                match read_timeout {
                    None => stream.next().await,
                    Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
                        Ok(item) => item,
                        Err(_) => {
                            return stream_read_payload(
                                &stream_id_str,
                                meerkat_contracts::StreamReadStatus::Timeout,
                            );
                        }
                    },
                }
            };
            match next_event {
                Some(envelope) => {
                    let status = stream_read_event_status(&envelope)?;
                    stream_read_payload(&stream_id_str, status)
                }
                None => {
                    state.mob_event_streams.lock().await.remove(&stream_id);
                    stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
                }
            }
        }
        MobEventStreamInner::MobWide(router_mutex) => {
            let next_event = {
                let mut router_handle = router_mutex.lock().await;
                match read_timeout {
                    None => router_handle.event_rx.recv().await,
                    Some(timeout) => {
                        match tokio::time::timeout(timeout, router_handle.event_rx.recv()).await {
                            Ok(item) => item,
                            Err(_) => {
                                return stream_read_payload(
                                    &stream_id_str,
                                    meerkat_contracts::StreamReadStatus::Timeout,
                                );
                            }
                        }
                    }
                }
            };
            match next_event {
                Some(attributed) => {
                    let status = stream_read_event_status(&attributed)?;
                    stream_read_payload(&stream_id_str, status)
                }
                None => {
                    state.mob_event_streams.lock().await.remove(&stream_id);
                    stream_read_payload(&stream_id_str, meerkat_contracts::StreamReadStatus::Closed)
                }
            }
        }
    }
}

#[cfg(feature = "mob")]
async fn handle_meerkat_mob_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatMobEventStreamCloseInput,
) -> Result<Value, String> {
    let stream_id = McpStreamId::parse(&input.stream_id)?;
    let removed = state.mob_event_streams.lock().await.remove(&stream_id);
    wrap_tool_payload(json!({
        "stream_id": stream_id.to_string(),
        "closed": removed.is_some()
    }))
}

#[cfg(feature = "comms")]
fn comms_send_tool_payload(receipt: meerkat_core::comms::SendReceipt) -> Result<Value, String> {
    wrap_tool_payload(json!({
        "receipt": meerkat_contracts::CommsSendResult::from(receipt),
    }))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_send(
    state: &MeerkatMcpState,
    input: MeerkatCommsSendInput,
) -> Result<Value, ToolCallError> {
    let session_id = meerkat::SessionId::parse(input.session_id())
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| {
            ToolCallError::new(
                -32603,
                format!("Session not found or comms not enabled: {session_id}"),
                Some(json!({
                    "code": "session_not_found_or_comms_disabled",
                    "session_id": session_id.to_string(),
                })),
            )
        })?;
    let peer_name = input.peer_label();
    let cmd = input
        .into_command()
        .into_command(&session_id)
        .map_err(|err| {
            ToolCallError::new(
                -32602,
                "Invalid comms command",
                Some(json!({
                    "code": "invalid_comms_command",
                    "message": err.to_string(),
                })),
            )
        })?;
    let receipt = comms
        .send(cmd)
        .await
        .map_err(|e| normalize_mcp_comms_send_error(peer_name.as_deref(), &e))?;
    comms_send_tool_payload(receipt).map_err(ToolCallError::internal)
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_peers(
    state: &MeerkatMcpState,
    input: MeerkatCommsPeersInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| format!("Session not found or comms not enabled: {session_id}"))?;
    comms_peers_tool_payload(comms.peers().await)
}

#[cfg(feature = "comms")]
fn comms_peers_tool_payload(
    peers: Vec<meerkat_core::comms::PeerDirectoryEntry>,
) -> Result<Value, String> {
    wrap_tool_payload(json!(meerkat_contracts::CommsPeersResult::from_entries(
        &peers
    )))
}

#[cfg(feature = "comms")]
fn normalize_mcp_comms_send_error(
    peer_name: Option<&str>,
    error: &meerkat_core::comms::SendError,
) -> ToolCallError {
    match error {
        meerkat_core::comms::SendError::PeerNotFound(peer) => ToolCallError::new(
            -32603,
            format!("peer_not_found_or_not_trusted: peer '{peer}' is not found or not trusted"),
            Some(json!({
                "code": "peer_not_found_or_not_trusted",
                "peer": peer,
            })),
        ),
        meerkat_core::comms::SendError::PeerOffline => ToolCallError::new(
            -32603,
            format!(
                "peer_unreachable: peer '{}' is unreachable: offline_or_no_ack",
                peer_name.unwrap_or("<unknown>")
            ),
            Some(json!({
                "code": "peer_unreachable",
                "peer": peer_name.unwrap_or("<unknown>"),
                "reason": "offline_or_no_ack",
            })),
        ),
        meerkat_core::comms::SendError::Transport(details) if peer_name.is_some() => {
            ToolCallError::new(
                -32603,
                format!(
                    "peer_unreachable: peer '{}' is unreachable: transport_error ({details})",
                    peer_name.unwrap_or("<unknown>")
                ),
                Some(json!({
                    "code": "peer_unreachable",
                    "peer": peer_name.unwrap_or("<unknown>"),
                    "reason": "transport_error",
                    "details": details,
                })),
            )
        }
        meerkat_core::comms::SendError::AdmissionDropped { reason } => {
            let peer = peer_name.unwrap_or("<unknown>");
            ToolCallError::new(
                -32603,
                format!(
                    "peer_admission_dropped: peer '{peer}' rejected envelope at ingress: {}",
                    reason.as_code()
                ),
                Some(json!({
                    "code": "peer_admission_dropped",
                    "peer": peer,
                    "reason": reason,
                    "message": format!(
                        "peer '{peer}' rejected envelope at ingress: {}",
                        reason.as_code()
                    ),
                })),
            )
        }
        other => ToolCallError::new(
            -32603,
            other.to_string(),
            Some(json!({
                "code": "send_failed",
                "message": other.to_string(),
            })),
        ),
    }
}

fn help_provider_to_mcp_provider(
    raw: Option<String>,
) -> Result<Option<ProviderInput>, ToolCallError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match meerkat_core::Provider::parse_strict(raw.as_str()) {
        Some(meerkat_core::Provider::Anthropic) => Ok(Some(ProviderInput::Anthropic)),
        Some(meerkat_core::Provider::OpenAI) => Ok(Some(ProviderInput::Openai)),
        Some(meerkat_core::Provider::Gemini) => Ok(Some(ProviderInput::Gemini)),
        Some(meerkat_core::Provider::SelfHosted) => Ok(Some(ProviderInput::Other)),
        Some(meerkat_core::Provider::Other) | None => Err(ToolCallError::invalid_params(format!(
            "invalid help provider `{raw}`"
        ))),
    }
}

async fn handle_meerkat_help(
    state: &MeerkatMcpState,
    input: meerkat_contracts::HelpRequest,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    let prompt = meerkat::help::render_help_prompt(&input)
        .map_err(|err| ToolCallError::invalid_params(err.to_string()))?;
    let provider = help_provider_to_mcp_provider(input.provider.clone())?;
    let run_input = MeerkatRunInput {
        prompt,
        system_prompt: Some(meerkat::help::help_system_prompt().to_string()),
        model: input
            .model
            .map(meerkat_core::lifecycle::run_primitive::ModelId::new),
        max_tokens: input.max_tokens,
        provider,
        output_schema: None,
        structured_output_retries: None,
        stream: false,
        verbose: false,
        tools: Vec::new(),
        enable_builtins: Some(false),
        builtin_config: Some(BuiltinConfigInput {
            enable_shell: Some(false),
            shell_timeout_secs: None,
        }),
        keep_alive: Some(false),
        comms_name: None,
        peer_meta: None,
        hooks_override: None,
        enable_memory: Some(false),
        enable_schedule: Some(false),
        enable_workgraph: Some(false),
        enable_mob: Some(false),
        enable_web_search: Some(false),
        provider_params: None,
        budget_limits: None,
        preload_skills: Some(meerkat::help::platform_preload_skills()),
        skill_refs: None,
        skill_references: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
        auth_binding: None,
    };

    Box::pin(handle_meerkat_run(
        state,
        run_input,
        notifier,
        request_context,
    ))
    .await
}

async fn handle_meerkat_run(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    validate_public_peer_meta(input.peer_meta.as_ref()).map_err(ToolCallError::invalid_params)?;
    let ingress = state.runtime_ingress_context();
    let keep_alive_override =
        resolve_keep_alive(input.keep_alive).map_err(ToolCallError::invalid_params)?;
    // Create: no persisted session to inherit from, so None → false.
    let keep_alive = keep_alive_override.unwrap_or(false);
    if keep_alive
        && input
            .comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires comms_name",
        ));
    }
    // A ConfigError is a TRUE fault and must not be laundered into a default config
    // that silently picks a phantom default model. Compose the realm chain so an
    // inherited (ancestor-realm) default model / self-hosted alias resolves like
    // the agent build path.
    let head_config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to read config: {e}")))?
        .config;
    let config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
        .effective_config_over_head(&state.realm_id, head_config)
        .await
        .map_err(|e| {
            ToolCallError::internal(format!("Failed to compose realm config chain: {e}"))
        })?;
    let model_resolution = resolve_mcp_create_session_model(
        &config,
        input.model.as_ref().map(|model| model.as_str().to_string()),
        input.provider,
        input.auth_binding.clone().map(Into::into),
    )
    .map_err(create_session_model_resolution_error_to_tool_error)?;
    let model = model_resolution.model.clone();

    // Build callback tools supplied by the MCP client. The per-session live
    // MCP router is created after runtime bindings are prepared so its surface
    // owner is session-canonical from construction.
    let callback_tools = build_callback_dispatcher(&input.tools);

    let enable_shell_override = input.builtin_config.as_ref().and_then(|c| c.enable_shell);
    let preload_skills = input.preload_skills.clone();
    let skill_references = canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    )
    .map_err(ToolCallError::invalid_params)?;

    // Parse output schema if provided
    let output_schema =
        match input.output_schema.clone() {
            Some(schema) => Some(OutputSchema::from_json_value(schema).map_err(|e| {
                ToolCallError::invalid_params(format!("Invalid output_schema: {e}"))
            })?),
            None => None,
        };

    // Pre-create a session to claim a stable session_id.
    let create_admission = state
        .service
        .reserve_create_session_admission()
        .await
        .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?;
    let session = meerkat::Session::new();
    let session_id = session.id().clone();

    if let Some(context) = request_context.as_ref() {
        // Before the runtime returns an exact accepted input identity, this
        // request owns no authority to cancel a session-current run. Request
        // cancellation may drop the response while durable creation remains
        // discoverable; it must never target a replacement incarnation.
        let install = context
            .install_cancel_action_or_cancelled(meerkat::surface::noop_request_action())
            .await;
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            return Err(request_cancelled_tool_error());
        }
    }

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let realm_id = state.realm_id.clone();
    let instance_id = state.instance_id.clone();
    let backend = meerkat_core::RecoveryBackendKind::parse(&state.backend);
    let callback_tools_for_plan = callback_tools.clone();
    let event_tx_for_plan = event_tx.clone();
    let request_context_for_plan = request_context.clone();
    let materialization = materialize_mcp_actor_transaction(
        &state.service,
        &state.runtime_adapter,
        &ingress,
        &session_id,
        McpActorMaterializationMode::Fresh,
        Some(create_admission),
        move |bindings, _logical_config| async move {
            if request_context_for_plan
                .as_ref()
                .is_some_and(RequestContext::cancel_already_requested)
            {
                return Err(request_cancelled_tool_error());
            }
            let adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
            ));
            let mcp_tools: Arc<dyn AgentToolDispatcher> = adapter.clone();
            let external_tools =
                compose_external_tool_dispatchers(callback_tools_for_plan.clone(), Some(mcp_tools))
                    .map_err(ToolCallError::internal)?;
            let mut build = SessionBuildOptions {
                tool_access_policy: None,
                custom_models: std::collections::BTreeMap::new(),
                image_generation_provider: None,
                auto_compact_threshold_override: None,
                provider: Some(model_resolution.provider),
                override_comms: Default::default(),
                self_hosted_server_id: None,
                output_schema,
                structured_output_retries: input.structured_output_retries,
                hooks_override: input.hooks_override.clone().unwrap_or_default(),
                comms_name: input.comms_name.clone(),
                peer_meta: input.peer_meta.clone(),
                resume_session: Some(session),
                budget_limits: input.budget_limits.clone().map(Into::into),
                provider_params: input.provider_params.clone(),
                call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
                external_tools,
                mcp_servers: Vec::new(),
                recoverable_tool_defs: (!input.tools.is_empty())
                    .then(|| recoverable_callback_tool_defs(&input.tools)),
                llm_client_override: None,
                session_comms_runtime_override: None,
                host_prompt_sections: Default::default(),
                agent_llm_client_decorator: None,
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
                    None,
                )),
                override_builtins: ToolCategoryOverride::from_override(input.enable_builtins),
                override_shell: ToolCategoryOverride::from_override(enable_shell_override),
                override_memory: ToolCategoryOverride::from_override(input.enable_memory),
                override_schedule: ToolCategoryOverride::from_override(input.enable_schedule),
                override_workgraph: ToolCategoryOverride::from_override(input.enable_workgraph),
                override_mob: ToolCategoryOverride::Inherit,
                override_image_generation: ToolCategoryOverride::Inherit,
                override_web_search: ToolCategoryOverride::from_override(input.enable_web_search),
                schedule_tools: None,
                workgraph_tools: None,
                mob_tool_authority_context: None,
                preload_skills,
                realm_id: Some(realm_id),
                instance_id,
                backend,
                config_generation: current_generation,
                auth_binding: model_resolution.auth_binding,
                mob_member_binding: None,
                keep_alive,
                checkpointer: None,
                silent_comms_intents: Vec::new(),
                max_inline_peer_notifications: None,
                app_context: input.app_context.clone(),
                additional_instructions: input.additional_instructions.clone(),
                initial_metadata_entries: std::collections::BTreeMap::new(),
                initial_tool_filter: None,
                shell_env: input.shell_env.clone(),
                resume_override_mask: ResumeOverrideMask {
                    model: input.model.is_some(),
                    provider: input.provider.is_some(),
                    auth_binding: input.auth_binding.is_some(),
                    max_tokens: input.max_tokens.is_some(),
                    structured_output_retries: input.structured_output_retries.is_some(),
                    provider_params: input.provider_params.is_some(),
                    preload_skills: input.preload_skills.is_some(),
                    keep_alive: keep_alive_override.is_some(),
                    comms_name: input.comms_name.is_some(),
                    peer_meta: input.peer_meta.is_some(),
                    override_schedule: input.enable_schedule.is_some(),
                    override_workgraph: input.enable_workgraph.is_some(),
                    override_web_search: input.enable_web_search.is_some(),
                    ..Default::default()
                },
                blob_store_override: None,
                mob_tools: None,
            };
            build.apply_generated_create_only_mob_operator_access(
                ToolCategoryOverride::from_override(input.enable_mob),
            );
            // Dogma K10: initial-turn skill references ride the one typed
            // carrier (`build.initial_turn_metadata`).
            if let Some(refs) = skill_references {
                build
                    .initial_turn_metadata
                    .get_or_insert_with(Default::default)
                    .skill_references
                    .get_or_insert_with(Vec::new)
                    .extend(refs);
            }
            let request = CreateSessionRequest {
                injected_context: Vec::new(),
                model,
                prompt: input.prompt.into(),
                system_prompt: match input.system_prompt {
                    Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                    None => meerkat::SystemPromptOverride::Inherit,
                },
                max_tokens: input.max_tokens,
                event_tx: event_tx_for_plan,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: input.labels,
            };
            Ok(McpActorMaterializationPlan {
                request,
                callback_config: runtime_ingress::McpCallbackConfig::Replace(
                    callback_tools_for_plan,
                ),
                router_to_publish: Some(Arc::clone(&adapter)),
            })
        },
    )
    .await;

    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }
    let outcome = materialization?;
    let should_attach_runtime = outcome.committed;
    let result = outcome.result;
    if result.is_ok() && !should_attach_runtime {
        return Err(ToolCallError::internal(format!(
            "MCP create succeeded without committing its exact actor attachment for {session_id}"
        )));
    }

    // Manage comms drain lifecycle for the new session.
    // keep_alive is a session-level mutation that may commit independently of
    // first-turn success, so once the session exists we align the drain state
    // even when the initial turn fails.
    #[cfg(feature = "comms")]
    if should_attach_runtime {
        ingress
            .configure_peer_ingress_exact(&session_id, keep_alive)
            .await
            .map_err(|error| {
                runtime_ingress::session_error_to_tool_error(
                    error,
                    format!("failed to update peer ingress context for {session_id}"),
                )
            })?;
    }
    match result {
        Ok(run_result) => format_agent_result_tool(Ok(run_result), &session_id),
        Err(err) => {
            if should_attach_runtime {
                Err(post_commit_session_created_error(&session_id, &err))
            } else {
                format_agent_result_tool(Err(err), &session_id)
            }
        }
    }
}

async fn handle_meerkat_resume(
    state: &MeerkatMcpState,
    input: MeerkatResumeInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    validate_public_peer_meta(input.peer_meta.as_ref()).map_err(ToolCallError::invalid_params)?;
    let ingress = state.runtime_ingress_context();
    // A ConfigError is a TRUE fault and must not be laundered into a default config.
    let config = state
        .config_runtime
        .get()
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to read config: {e}")))?
        .config;

    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|err| ToolCallError::invalid_params(invalid_session_id_message(err)))?;

    if let Some(context) = request_context.as_ref() {
        // A generic resume request does not own the session-current run. Keep
        // cancellation request-scoped until an exact input witness exists;
        // never let a late cancel hit replacement attachment B.
        let install = context
            .install_cancel_action_or_cancelled(meerkat::surface::noop_request_action())
            .await;
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            return Err(request_cancelled_tool_error());
        }
    }

    let mut session = state
        .service
        .load_authoritative_session(&session_id)
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to load session: {e}")))?
        .ok_or_else(|| {
            ToolCallError::new(
                -32004,
                format!("Session not found: {}", input.session_id),
                None,
            )
        })?;
    if state
        .service
        .session_archived_by_authority(&session_id, &session)
        .await
        .map_err(|e| {
            ToolCallError::internal(format!("Failed to load session archive state: {e}"))
        })?
    {
        state
            .cleanup_archived_session_runtime(&session_id)
            .await
            .map_err(archive_session_error_to_tool_error)?;
        return Err(ToolCallError::new(
            -32004,
            format!("Session not found: {}", input.session_id),
            None,
        ));
    }

    // Inject tool results into the session before resuming
    if !input.tool_results.is_empty() {
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        session.push(Message::tool_results(results));
    }

    // Resolve settings from stored metadata, failing closed when the durable
    // session identity is absent.
    let stored_metadata = session.session_metadata().ok_or_else(|| {
        ToolCallError::internal(format!(
            "persisted session {session_id} is missing session metadata"
        ))
    })?;

    let enable_builtins_override = input.enable_builtins;
    let enable_shell_override = input
        .builtin_config
        .as_ref()
        .and_then(|cfg| cfg.enable_shell);
    // §10: inherit, disable, and set are different facts.
    // None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    let keep_alive_override =
        resolve_keep_alive(input.keep_alive).map_err(ToolCallError::invalid_params)?;
    let keep_alive = match keep_alive_override {
        Some(val) => val,
        None => stored_metadata.keep_alive,
    };
    let comms_name = input
        .comms_name
        .clone()
        .or_else(|| stored_metadata.comms_name.clone());
    if keep_alive
        && comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires comms_name",
        ));
    }
    let model = input
        .model
        .as_ref()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| stored_metadata.model.clone());
    let max_tokens = input.max_tokens.or(Some(stored_metadata.max_tokens));
    let provider = input.provider.map(ProviderInput::to_provider);
    if keep_alive_override.is_some()
        && keep_alive
        && state.service.comms_runtime(&session_id).await.is_none()
    {
        return Err(ToolCallError::invalid_params(
            "keep_alive requires a session created with comms_name",
        ));
    }

    let replace_callback_tools = !input.tools.is_empty();
    let requested_callback_tools = if replace_callback_tools {
        build_callback_dispatcher(&input.tools)
    } else {
        None
    };
    let live_actor_lease = match state
        .service
        .acquire_live_session_actor_turn_boundary_lease(&session_id)
        .await
    {
        Ok(lease) => Some(lease),
        Err(SessionError::NotFound { .. }) => None,
        Err(error) => {
            return Err(ToolCallError::internal(format!(
                "failed to acquire exact live actor authority for {session_id}: {error}"
            )));
        }
    };
    let live_snapshot = match live_actor_lease.as_ref() {
        Some(actor_lease) => Some(
            ingress
                .logical_config_for_live_actor(&session_id, actor_lease)
                .await
                .map_err(|error| {
                    ToolCallError::internal(format!(
                        "failed to snapshot live MCP configuration for {session_id}: {error}"
                    ))
                })?,
        ),
        None => None,
    };
    let machine_witness = if live_actor_lease.is_some() {
        state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
    } else {
        None
    };
    let exact_live_attachment = live_snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.attachment_witness.is_some() && snapshot.attachment_witness == machine_witness
    });
    let sidecar_existed = live_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.sidecar_exists);
    let callback_config = if replace_callback_tools {
        runtime_ingress::McpCallbackConfig::Replace(requested_callback_tools.clone())
    } else {
        runtime_ingress::McpCallbackConfig::Preserve
    };
    let existing_adapter = live_snapshot.and_then(|snapshot| snapshot.router);

    // Decide from one exact B-owned actor/config/attachment snapshot. Any
    // rebuild re-snapshots logical config after its own prepared transaction
    // acquires B, so no pre-boundary router can enter the replacement actor.
    let needs_rebuild = live_actor_lease.is_none()
        || existing_adapter.is_none()
        || !exact_live_attachment
        || mcp_resume_requires_rebuild(&input);
    drop(live_actor_lease);
    let create_requires_materialization = needs_rebuild;
    let mut resume_admission = if create_requires_materialization {
        Some(
            state
                .service
                .reserve_runtime_turn_admission(&session_id)
                .await
                .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
        )
    } else {
        None
    };
    let mut mcp_adapter = existing_adapter.clone();

    // Use empty prompt when only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        String::new()
    } else {
        input.prompt
    };
    let preload_skills = input.preload_skills.clone();
    let skill_references = match canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    ) {
        Ok(keys) => keys,
        Err(error) => {
            return Err(ToolCallError::invalid_params(error));
        }
    };

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let output_schema = match input.output_schema.clone() {
        Some(schema) => match OutputSchema::from_json_value(schema) {
            Ok(schema) => Some(schema),
            Err(error) => {
                return Err(ToolCallError::invalid_params(format!(
                    "Invalid output_schema: {error}"
                )));
            }
        },
        None => None,
    };
    let llm_binding = meerkat_core::session_recovery::resolve_resume_llm_binding(
        stored_metadata.provider,
        stored_metadata.self_hosted_server_id.clone(),
        input.model.as_ref().map(|m| m.as_str()),
        provider,
    )
    .map_err(|error| ToolCallError::invalid_params(error.to_string()))?;
    let build_session_options = |runtime_bindings, external_tools| {
        let mut build = SessionBuildOptions {
            tool_access_policy: None,
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            provider: llm_binding.provider,
            override_comms: Default::default(),
            self_hosted_server_id: llm_binding.self_hosted_server_id.clone(),
            output_schema: output_schema.clone(),
            structured_output_retries: input.structured_output_retries,
            hooks_override: input.hooks_override.clone().unwrap_or_default(),
            comms_name: input.comms_name.clone(),
            resume_session: Some(session.clone()),
            budget_limits: input.budget_limits.clone().map(Into::into),
            provider_params: input.provider_params.clone(),
            call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
            external_tools,
            mcp_servers: Vec::new(),
            recoverable_tool_defs: (!input.tools.is_empty())
                .then(|| recoverable_callback_tool_defs(&input.tools)),
            llm_client_override: None,
            session_comms_runtime_override: None,
            host_prompt_sections: Default::default(),
            agent_llm_client_decorator: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings),
            initial_turn_metadata: Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
                None,
            )),
            override_builtins: ToolCategoryOverride::from_override(enable_builtins_override),
            override_shell: ToolCategoryOverride::from_override(enable_shell_override),
            override_memory: ToolCategoryOverride::from_override(input.enable_memory),
            override_schedule: ToolCategoryOverride::from_override(input.enable_schedule),
            override_workgraph: ToolCategoryOverride::from_override(input.enable_workgraph),
            override_mob: ToolCategoryOverride::Inherit,
            override_image_generation: ToolCategoryOverride::Inherit,
            override_web_search: ToolCategoryOverride::from_override(input.enable_web_search),
            schedule_tools: None,
            workgraph_tools: None,
            mob_tool_authority_context: None,
            preload_skills: preload_skills.clone(),
            peer_meta: input.peer_meta.clone(),
            realm_id: stored_metadata
                .realm_id
                .clone()
                .or_else(|| Some(state.realm_id.clone())),
            instance_id: stored_metadata
                .instance_id
                .clone()
                .or_else(|| state.instance_id.clone()),
            backend: stored_metadata
                .backend
                .as_deref()
                .and_then(meerkat_core::RecoveryBackendKind::parse)
                .or_else(|| meerkat_core::RecoveryBackendKind::parse(&state.backend)),
            config_generation: current_generation,
            auth_binding: None,
            mob_member_binding: stored_metadata.mob_member_binding.clone(),
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: input.additional_instructions.clone(),
            initial_metadata_entries: std::collections::BTreeMap::new(),
            initial_tool_filter: None,
            shell_env: None,
            resume_override_mask: ResumeOverrideMask {
                model: input.model.is_some(),
                provider: llm_binding.provider_overridden,
                max_tokens: input.max_tokens.is_some(),
                structured_output_retries: input.structured_output_retries.is_some(),
                provider_params: input.provider_params.is_some(),
                preload_skills: input.preload_skills.is_some(),
                keep_alive: keep_alive_override.is_some(),
                comms_name: input.comms_name.is_some(),
                peer_meta: input.peer_meta.is_some(),
                override_schedule: input.enable_schedule.is_some(),
                override_workgraph: input.enable_workgraph.is_some(),
                override_web_search: input.enable_web_search.is_some(),
                ..Default::default()
            },
            blob_store_override: None,
            mob_tools: None,
        };
        build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::from_override(
            input.enable_mob,
        ));
        build
    };

    // Both the initially-missing actor path and the live-turn disappearance
    // race use this same explicit resume transaction. The caller supplies only
    // the recovered admission; prepare, exact config snapshot, actor creation,
    // and attachment commit remain owned by one seam.
    let materialize_resume = |admission: Option<meerkat::RuntimeContextAdmissionGuard>| {
        let build_session_options = &build_session_options;
        let model_for_plan = model.clone();
        let prompt_for_plan = prompt.clone();
        let system_prompt_for_plan = input.system_prompt.clone();
        let event_tx_for_plan = event_tx.clone();
        let skill_references_for_plan = skill_references.clone();
        let requested_callback_tools_for_plan = requested_callback_tools.clone();
        let request_context_for_plan = request_context.clone();
        materialize_mcp_actor_transaction(
            &state.service,
            &state.runtime_adapter,
            &ingress,
            &session_id,
            McpActorMaterializationMode::Resume,
            admission,
            move |resume_bindings, exact_config| async move {
                if request_context_for_plan
                    .as_ref()
                    .is_some_and(RequestContext::cancel_already_requested)
                {
                    return Err(request_cancelled_tool_error());
                }
                let exact_callback_tools = if replace_callback_tools {
                    requested_callback_tools_for_plan
                } else {
                    exact_config.callback_tools.clone()
                };
                let adapter = exact_config.router.clone().unwrap_or_else(|| {
                    Arc::new(meerkat_mcp::McpRouterAdapter::new(
                        McpRouter::new_with_surface_handle(Arc::clone(
                            resume_bindings.external_tool_surface(),
                        )),
                    ))
                });
                let mcp_tools: Arc<dyn AgentToolDispatcher> = adapter.clone();
                let external_tools = compose_external_tool_dispatchers(
                    exact_callback_tools.clone(),
                    Some(mcp_tools),
                )
                .map_err(ToolCallError::internal)?;
                let mut build = build_session_options(resume_bindings, external_tools);
                if let Some(refs) = skill_references_for_plan {
                    build
                        .initial_turn_metadata
                        .get_or_insert_with(Default::default)
                        .skill_references
                        .get_or_insert_with(Vec::new)
                        .extend(refs);
                }
                let request = CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: model_for_plan,
                    prompt: prompt_for_plan.into(),
                    system_prompt: match system_prompt_for_plan {
                        Some(prompt) => meerkat::SystemPromptOverride::Set(prompt),
                        None => meerkat::SystemPromptOverride::Inherit,
                    },
                    max_tokens,
                    event_tx: event_tx_for_plan,
                    initial_turn: InitialTurnPolicy::RunImmediately,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(build),
                    labels: None,
                };
                Ok(McpActorMaterializationPlan {
                    request,
                    callback_config: if replace_callback_tools {
                        runtime_ingress::McpCallbackConfig::Replace(exact_callback_tools)
                    } else {
                        runtime_ingress::McpCallbackConfig::Preserve
                    },
                    router_to_publish: exact_config.router.is_none().then(|| Arc::clone(&adapter)),
                })
            },
        )
    };

    let mut runtime_materialization_committed = false;
    let result = if create_requires_materialization {
        let outcome = materialize_resume(resume_admission.take()).await?;
        runtime_materialization_committed = outcome.committed;
        outcome.result
    } else {
        let mut live_turn_admission = None;
        if keep_alive_override.is_some() {
            let comms_rt = state.service.comms_runtime(&session_id).await;
            if keep_alive && comms_rt.is_none() {
                return Err(ToolCallError::invalid_params(
                    "keep_alive requires a session created with comms_name",
                ));
            }
            live_turn_admission = Some(
                state
                    .service
                    .reserve_runtime_turn_admission(&session_id)
                    .await
                    .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
            );
            // RuntimeTurnMetadata carries the keep_alive mutation; the
            // session service applies it only after generated turn-admission
            // feedback. The surface may refresh machine-owned peer ingress.
            ingress
                .configure_peer_ingress_exact(&session_id, keep_alive)
                .await
                .map_err(|error| {
                    runtime_ingress::session_error_to_tool_error(
                        error,
                        format!("failed to update peer ingress context for {session_id}"),
                    )
                })?;
        }
        // Live MCP resumes still use the runtime/machine service-turn receipt
        // path; the persistent service only owns the live mutation and post-
        // receipt projection, not lifecycle truth.
        // Dogma K13: forward the full typed keep-alive tri-state to the
        // machine. `Some(true)` -> Enable(policy), `Some(false)` -> Disable
        // (explicit operator intent, never dropped into preserve), `None` ->
        // preserve the persisted session intent.
        let turn_seed_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            skill_references: skill_references.clone(),
            turn_tool_overlay: input.turn_tool_overlay.clone().map(Into::into),
            keep_alive: keep_alive_override.map(|keep_alive| {
                if keep_alive {
                    meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Enable(
                        meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
                            ttl: std::time::Duration::from_secs(30),
                            policy: meerkat_core::lifecycle::run_primitive::KeepAliveMode::Pinned,
                        },
                    )
                } else {
                    meerkat_core::lifecycle::run_primitive::KeepAliveDirective::Disable
                }
            }),
            ..Default::default()
        };
        let turn_req = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: prompt.clone().into(),
            system_prompt: None,
            event_tx: event_tx.clone(),
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                input.turn_tool_overlay.clone().map(Into::into),
                Vec::new(),
                Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(Some(
                    turn_seed_metadata,
                ))),
            ),
        };
        let admission = match live_turn_admission.take() {
            Some(admission) => admission,
            None => state
                .service
                .reserve_runtime_turn_admission(&session_id)
                .await
                .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
        };
        let turn_result = state
            .service
            .run_machine_committed_live_turn(
                MachineServiceTurnCommitProtocol::from_machine(state.runtime_adapter.as_ref()),
                &session_id,
                turn_req,
                admission,
            )
            .await;
        match turn_result {
            Ok(run_result) => Ok(run_result),
            Err((SessionError::NotFound { .. }, recovered_admission)) => {
                let admission = match recovered_admission.or_else(|| resume_admission.take()) {
                    Some(admission) => admission,
                    None => state
                        .service
                        .reserve_runtime_turn_admission(&session_id)
                        .await
                        .map_err(|err| ToolCallError::internal(format!("Agent error: {err}")))?,
                };
                let outcome = materialize_resume(Some(admission)).await?;
                runtime_materialization_committed = outcome.committed;
                outcome.result
            }
            Err((other, _admission)) => Err(other),
        }
    };
    let session_exists = match state.service.load_authoritative_session(&session_id).await {
        Ok(Some(session)) => match state
            .service
            .session_archived_by_authority(&session_id, &session)
            .await
        {
            Ok(true) => {
                state
                    .cleanup_archived_session_runtime(&session_id)
                    .await
                    .map_err(archive_session_error_to_tool_error)?;
                false
            }
            Ok(false) => true,
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to load machine archive state during MCP session existence check"
                );
                return Err(ToolCallError::internal(format!(
                    "failed to load machine archive state during MCP session existence check for {session_id}: {err}"
                )));
            }
        },
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to load authoritative session after MCP resume attempt"
            );
            return Err(ToolCallError::internal(format!(
                "failed to load authoritative session after MCP resume attempt for {session_id}: {err}"
            )));
        }
    };
    let live_session_exists = if session_exists {
        state
            .service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .is_ok()
    } else {
        false
    };
    let runtime_was_materialized =
        create_requires_materialization || runtime_materialization_committed;
    if result.is_ok() && (!session_exists || !live_session_exists) {
        return Err(ToolCallError::internal(format!(
            "MCP resume returned success without authoritative live session {session_id}"
        )));
    }

    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }

    let should_configure_runtime_ingress = session_exists
        && live_session_exists
        && (result.is_ok()
            || runtime_materialization_committed
            || (!runtime_was_materialized && sidecar_existed));
    if should_configure_runtime_ingress && !runtime_materialization_committed {
        let router = if existing_adapter.is_some() {
            None
        } else {
            let Some(mcp_adapter) = mcp_adapter.take() else {
                return Err(ToolCallError::internal(format!(
                    "MCP resume completed without a logical router for {session_id}"
                )));
            };
            Some(mcp_adapter)
        };
        let callback_config = if result.is_ok() {
            callback_config
        } else {
            runtime_ingress::McpCallbackConfig::Preserve
        };
        let attachment = match router {
            Some(router) => {
                ingress
                    .configure_session_with_router(&session_id, callback_config, router)
                    .await
            }
            None => {
                ingress
                    .configure_session(&session_id, callback_config)
                    .await
            }
        };
        if let Err(error) = attachment {
            return Err(runtime_ingress::session_error_to_tool_error(
                error,
                format!("failed to attach MCP runtime executor for {session_id}"),
            ));
        }
    }

    // Manage comms drain lifecycle for rebuilt sessions after the session
    // commit boundary. keep_alive may commit independently of turn success.
    #[cfg(feature = "comms")]
    if session_exists && runtime_was_materialized && should_configure_runtime_ingress {
        ingress
            .configure_peer_ingress_exact(&session_id, keep_alive)
            .await
            .map_err(|error| {
                runtime_ingress::session_error_to_tool_error(
                    error,
                    format!("failed to update peer ingress context for {session_id}"),
                )
            })?;
    }

    format_agent_result_tool(result, &session_id)
}

/// Wrap a structured payload in the MCP text-content envelope.
///
/// Serialization is a TRUE fault: a payload that fails to serialize must surface
/// as an error, never as an empty/`null` content envelope masquerading as success.
fn wrap_tool_payload(payload: Value) -> Result<Value, String> {
    let text = serde_json::to_string(&payload)
        .map_err(|err| format!("Failed to serialize tool payload: {err}"))?;
    Ok(json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    }))
}

/// Build a stream-read tool payload from the typed
/// [`meerkat_contracts::StreamReadStatus`] contract.
///
/// A stream read resolves into exactly one terminal shape — a delivered event,
/// an expired timeout, or a closed stream — and that shape is the single typed
/// authority for read status. The transport-level `stream_id` is carried by the
/// enclosing response (one fact, one owner), so surfaces never hand-roll
/// `status: "event" | "timeout" | "closed"` string conventions.
fn stream_read_payload(
    stream_id: &str,
    status: meerkat_contracts::StreamReadStatus,
) -> Result<Value, String> {
    let mut payload = serde_json::to_value(&status)
        .map_err(|err| format!("Failed to serialize stream read status: {err}"))?;
    match &mut payload {
        Value::Object(map) => {
            map.insert(
                "stream_id".to_string(),
                Value::String(stream_id.to_string()),
            );
        }
        _ => {
            return Err("stream read status did not serialize to a JSON object".to_string());
        }
    }
    wrap_tool_payload(payload)
}

/// Build a [`meerkat_contracts::StreamReadStatus::Event`] from a serializable
/// event envelope, riding the body as opaque `Box<RawValue>` (never matched at
/// this layer).
fn stream_read_event_status(
    envelope: &impl serde::Serialize,
) -> Result<meerkat_contracts::StreamReadStatus, String> {
    let body = serde_json::to_string(envelope)
        .map_err(|err| format!("Failed to serialize stream event: {err}"))?;
    let event = serde_json::value::RawValue::from_string(body)
        .map_err(|err| format!("Failed to encode stream event: {err}"))?;
    Ok(meerkat_contracts::StreamReadStatus::Event { event })
}

fn compose_external_tool_dispatchers(
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, String> {
    match (primary, secondary) {
        (None, None) => Ok(None),
        (Some(dispatcher), None) | (None, Some(dispatcher)) => Ok(Some(dispatcher)),
        (Some(a), Some(b)) => {
            // Keep both live dispatchers even when one currently exposes no
            // tools. MCP routers populate asynchronously and must remain in
            // the composition so factory handle binding and later catalog
            // updates reach them. ToolGateway retains its children, resolves
            // live additions, and fails closed on name collisions; filtering
            // from a one-time snapshot would freeze dynamic ownership.
            let gateway = meerkat_core::ToolGatewayBuilder::new()
                .add_dispatcher(a)
                .add_dispatcher(b)
                .build()
                .map_err(|e| format!("failed to compose external tools: {e}"))?;
            Ok(Some(Arc::new(gateway)))
        }
    }
}

// Adapter types needed for the MCP server

use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, Message, ToolDef};

/// MCP tool dispatcher - exposes tools to the LLM and handles callback tools
/// by returning a special error that signals the MCP client needs to handle the tool call
pub struct MpcToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    callback_tools: HashSet<String>,
}

impl MpcToolDispatcher {
    /// Create a new tool dispatcher from MCP tool definitions
    pub fn new(mcp_tools: &[McpToolDef]) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = mcp_tools
            .iter()
            .map(|t| {
                Arc::new(ToolDef {
                    name: t.name.clone().into(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                    provenance: Some(meerkat_core::types::ToolProvenance {
                        kind: meerkat_core::types::ToolSourceKind::Builtin,
                        source_id: "mcp-server".into(),
                    }),
                })
            })
            .collect::<Vec<_>>()
            .into();

        let callback_tools = mcp_tools
            .iter()
            .filter(|t| t.handler_kind() == "callback")
            .map(|t| t.name.clone())
            .collect();

        Self {
            tools,
            callback_tools,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MpcToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        // K1: callback ingress goes through the typed tool-argument contract.
        // Malformed / non-object args fail closed with a typed
        // `InvalidArguments` error — never a `Value::String` wrap that would
        // silently reach the callback consumer.
        let args = meerkat_core::ToolCallArguments::from_raw_json(call.args)
            .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
        // Check if this is a callback tool
        if self.callback_tools.contains(call.name) {
            // Return a special error that signals the agent loop should pause
            Err(ToolError::callback_pending(call.name, args.into_value()))
        } else {
            Err(ToolError::not_found(call.name))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::stream;
    use meerkat::Session;
    use meerkat::surface::{
        CancelActionInstallOutcome, SurfaceRequestExecutor, noop_request_action,
    };
    use meerkat::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Duration, timeout};

    fn mcp_inherited_gemini_binding() -> (Config, meerkat_core::AuthBindingRef) {
        let mut config = Config::default();
        let mut global = meerkat_core::RealmConfigSection::default();
        global.backend.insert(
            "google".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "gemini".to_string(),
                backend_kind: "google_code_assist".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        global.auth.insert(
            "oauth".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".to_string(),
                auth_method: "google_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        global.binding.insert(
            "primary".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "google".to_string(),
                auth_profile: "oauth".to_string(),
                default_model: Some("gemini-3.1-flash-lite-preview".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("global".to_string(), global);
        config.realm.insert(
            "dev".to_string(),
            meerkat_core::RealmConfigSection {
                parent: Some(meerkat_core::RealmId::global()),
                ..Default::default()
            },
        );
        let binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm"),
            binding: meerkat_core::BindingId::parse("primary").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        (config, binding)
    }

    #[test]
    fn mcp_create_session_resolution_preserves_supported_config_pin() {
        let mut config = Config::default();
        config.agent.model = "claude-opus-4-7".to_string();
        config.models.anthropic = "claude-sonnet-4-6".to_string();

        let resolved = resolve_mcp_create_session_model(&config, None, None, None)
            .expect("MCP lowers through shared resolver");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider, Provider::Anthropic);
    }

    #[test]
    fn mcp_create_session_resolution_preserves_inherited_binding_owner() {
        let (config, binding) = mcp_inherited_gemini_binding();
        let resolved = resolve_mcp_create_session_model(&config, None, None, Some(binding))
            .expect("MCP resolves inherited binding");

        assert_eq!(resolved.model, "gemini-3.1-flash-lite-preview");
        assert_eq!(resolved.provider, Provider::Gemini);
        assert_eq!(
            resolved
                .auth_binding
                .expect("owner-stamped binding")
                .realm
                .as_str(),
            "global"
        );
    }

    #[test]
    fn mcp_create_session_resolution_rejects_explicit_model_binding_mismatch() {
        let (config, binding) = mcp_inherited_gemini_binding();
        let err = resolve_mcp_create_session_model(
            &config,
            Some("gpt-5.5".to_string()),
            None,
            Some(binding),
        )
        .expect_err("known model owner must match binding provider");

        assert!(err.to_string().contains("registered for provider 'openai'"));
    }

    #[test]
    fn mcp_create_session_resolution_preserves_config_fault_ownership() {
        let config_error = meerkat::CreateSessionModelResolutionError::Config(
            meerkat_core::ConfigError::Validation("broken model registry".to_string()),
        );
        assert_eq!(
            create_session_model_resolution_error_to_tool_error(config_error).code,
            -32603
        );
        let binding_error =
            meerkat::CreateSessionModelResolutionError::BindingMissingProviderDefault {
                realm: meerkat_core::RealmId::parse("dev").expect("realm"),
                binding: meerkat_core::BindingId::parse("primary").expect("binding"),
                provider: meerkat_core::Provider::SelfHosted,
            };
        assert_eq!(
            create_session_model_resolution_error_to_tool_error(binding_error).code,
            -32603
        );

        assert_eq!(
            create_session_model_resolution_error_to_tool_error(
                meerkat::CreateSessionModelResolutionError::EmptyExplicitModel,
            )
            .code,
            -32602
        );
        assert_eq!(
            create_session_model_resolution_error_to_tool_error(
                meerkat::CreateSessionModelResolutionError::MissingProviderDefault {
                    provider: meerkat_core::Provider::SelfHosted,
                },
            )
            .code,
            -32602
        );
    }

    struct RuntimeTerminationFixtureExecutor(Option<Arc<AtomicBool>>);

    #[async_trait]
    impl meerkat_core::lifecycle::CoreExecutor for RuntimeTerminationFixtureExecutor {
        async fn apply(
            &mut self,
            run_id: meerkat_core::RunId,
            primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<CoreApplyOutput, meerkat_core::lifecycle::CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            if let Some(stopped) = self.0.as_ref() {
                stopped.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct OwnershipCheckingLlmClient {
        old_executor_stopped: Arc<AtomicBool>,
        turn_started: Arc<AtomicBool>,
    }

    #[async_trait]
    impl LlmClient for OwnershipCheckingLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            assert!(
                self.old_executor_stopped.load(Ordering::SeqCst),
                "plain resume must stop the exact foreign executor before starting its LLM turn"
            );
            self.turn_started.store(true, Ordering::SeqCst);
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct MutableToolDispatcher {
        tools: Arc<StdMutex<Vec<Arc<ToolDef>>>>,
    }

    #[async_trait]
    impl AgentToolDispatcher for MutableToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .into()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    #[test]
    fn test_wrap_tool_payload_returns_typed_result_envelope() {
        // wrap_tool_payload must surface serialization as a typed Result, never a
        // silent empty-text/null content envelope. The happy path yields the
        // text-content envelope carrying the serialized payload.
        let wrapped = wrap_tool_payload(json!({ "key": "value" }))
            .expect("valid payload serializes into the content envelope");
        let text = wrapped["content"][0]["text"]
            .as_str()
            .expect("content envelope carries serialized text");
        assert_eq!(serde_json::from_str::<Value>(text).unwrap()["key"], "value");
        // The envelope is never the fail-open empty-text shape.
        assert_ne!(text, "");
    }

    fn unwrap_payload(value: Value) -> Value {
        if value.get("content").is_none() {
            return value;
        }
        let raw = value["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        serde_json::from_str(raw).expect("wrapped payload JSON")
    }

    async fn state_with_persisted_session() -> (MeerkatMcpState, String) {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(&store),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::default())),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().to_string();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        session
            .set_build_state(meerkat_core::SessionBuildState::default())
            .expect("session build state should serialize");
        store.save(&session).await.expect("persisted session");
        (state, session_id)
    }

    async fn attach_test_mcp_router(
        state: &MeerkatMcpState,
        session_id: &meerkat::SessionId,
        router: Arc<meerkat_mcp::McpRouterAdapter>,
    ) {
        let ingress = state.runtime_ingress_context();
        if ingress
            .current_attachment_witness(session_id)
            .await
            .is_some()
        {
            ingress
                .configure_session_with_router(
                    session_id,
                    runtime_ingress::McpCallbackConfig::Preserve,
                    router,
                )
                .await
                .expect("test MCP router must update an exact live attachment");
            return;
        }

        let session = state
            .service
            .load_authoritative_session(session_id)
            .await
            .expect("load test MCP session")
            .expect("test MCP session must be persisted");
        let prepared =
            prepare_mcp_actor_materialization(&state.service, &state.runtime_adapter, session_id)
                .await
                .expect("test MCP attachment must use the shared resume seam");
        let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: session
                .session_metadata()
                .map(|metadata| metadata.model)
                .unwrap_or_else(|| "mock-only-model".to_string()),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(256),
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                provider: Some(Provider::Other),
                resume_session: Some(session),
                external_tools: Some(router_tools),
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                )),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(
                    prepared.bindings_clone(),
                ),
                ..Default::default()
            }),
            labels: None,
        };
        let outcome = create_runtime_backed_session_and_run_initial_turn_call_local(
            &state.service,
            &state.runtime_adapter,
            session_id,
            request,
            None,
            prepared,
        )
        .await
        .expect("test MCP actor reconstruction should report");
        let McpCallLocalActorCreateOutcome {
            result,
            transaction,
        } = outcome;
        result.expect("test MCP actor reconstruction should succeed");
        ingress
            .commit_prepared_session(
                session_id,
                transaction,
                runtime_ingress::McpCallbackConfig::Preserve,
                Some(router),
            )
            .await
            .expect("test MCP router must publish with an exact live attachment");
    }

    fn bounded_resume_input(session_id: String, tools: Vec<McpToolDef>) -> MeerkatResumeInput {
        MeerkatResumeInput {
            session_id,
            prompt: "Resume".to_string(),
            system_prompt: None,
            model: None,
            max_tokens: None,
            provider: None,
            output_schema: None,
            structured_output_retries: None,
            stream: false,
            verbose: false,
            tools,
            tool_results: vec![],
            enable_builtins: None,
            builtin_config: None,
            keep_alive: None,
            comms_name: None,
            peer_meta: None,
            hooks_override: None,
            enable_memory: None,
            enable_schedule: None,
            enable_workgraph: None,
            enable_mob: None,
            enable_web_search: None,
            provider_params: None,
            budget_limits: None,
            preload_skills: None,
            skill_refs: None,
            skill_references: None,
            turn_tool_overlay: None,
            additional_instructions: None,
        }
    }

    #[tokio::test]
    async fn persisted_only_explicit_resume_always_fences_preexisting_attachment_work() {
        let (state, session_id) = state_with_persisted_session().await;
        let session_id = meerkat::SessionId::parse(&session_id).expect("valid session id");
        assert!(
            state
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .is_none(),
            "persisted-only fixture must not have a process-local attachment witness"
        );
        assert!(
            !state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live actor lookup"),
            "persisted-only fixture must not have a live actor"
        );

        let mut prepared =
            prepare_mcp_actor_materialization(&state.service, &state.runtime_adapter, &session_id)
                .await
                .expect("explicit resume prepares a cold materialization");
        assert!(
            prepared.replaces_predecessor(),
            "cold explicit resume must terminalize any durable A-era inputs before B serves"
        );
        prepared
            .rollback_now()
            .await
            .expect("cold replacement claim releases cleanly");
    }

    #[tokio::test]
    async fn resume_success_path_without_tools_does_not_reenter_registration_lock() {
        let (state, session_id) = state_with_persisted_session().await;
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            Box::pin(handle_meerkat_resume(
                &state,
                bounded_resume_input(session_id, vec![]),
                None,
                None,
            )),
        )
        .await
        .expect("successful resume without tools must not deadlock");
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "resume must reach its post-completion boundary: {result:?}"
        );
    }

    #[tokio::test]
    async fn plain_resume_resolves_foreign_attachment_before_running_a_turn() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let old_executor_stopped = Arc::new(AtomicBool::new(false));
        let turn_started = Arc::new(AtomicBool::new(false));
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(OwnershipCheckingLlmClient {
                old_executor_stopped: Arc::clone(&old_executor_stopped),
                turn_started: Arc::clone(&turn_started),
            })),
        )
        .await;
        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare target runtime bindings");
        let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            CreateSessionRequest {
                injected_context: Vec::new(),
                model: "mock-only-model".to_string(),
                prompt: "deferred fixture".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(256),
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    provider: Some(Provider::Other),
                    resume_session: Some(session),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                    )),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            },
            None,
            &meerkat::LiveSessionActorWitnessSlot::default(),
        )
        .await
        .expect("materialize a live actor backed only by the injected mock client");
        attach_test_mcp_router(&state, &session_id, router).await;
        let target_witness = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("target MCP attachment must be committed");

        assert!(
            state
                .runtime_adapter
                .unregister_executor_attachment_if_current(&target_witness)
                .await
                .expect("detach the original MCP executor"),
            "the original exact MCP attachment must still own the machine slot"
        );
        state
            .runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                Box::new(RuntimeTerminationFixtureExecutor(Some(Arc::clone(
                    &old_executor_stopped,
                )))),
            )
            .await
            .expect("install a same-session executor not owned by the MCP sidecar");
        let foreign_witness = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("foreign fixture witness");
        assert_ne!(foreign_witness, target_witness);
        assert_ne!(
            state
                .runtime_ingress_context()
                .current_attachment_witness(&session_id)
                .await,
            Some(foreign_witness.clone()),
            "the replacement executor is deliberately foreign to MCP ownership"
        );

        let result = handle_meerkat_resume(
            &state,
            bounded_resume_input(session_id.to_string(), vec![]),
            None,
            None,
        )
        .await
        .expect("plain resume should replace the foreign executor and run the turn");
        assert!(
            result.get("content").is_some(),
            "successful resume should return the MCP content envelope: {result:?}"
        );
        assert!(
            old_executor_stopped.load(Ordering::SeqCst),
            "plain resume must detach the exact foreign executor"
        );
        assert!(
            turn_started.load(Ordering::SeqCst),
            "the replacement actor must execute the requested turn"
        );
        let replacement_witness = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("plain resume must commit a replacement MCP attachment");
        assert_ne!(replacement_witness, target_witness);
        assert_ne!(replacement_witness, foreign_witness);
        assert_eq!(
            state
                .runtime_ingress_context()
                .current_attachment_witness(&session_id)
                .await,
            Some(replacement_witness),
            "machine and MCP sidecar must converge on the exact replacement witness"
        );
    }

    #[tokio::test]
    async fn resume_callback_config_commits_on_success_and_empty_tools_preserve_it() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let callback = McpToolDef {
            name: "resume_callback".to_string(),
            description: "Resume callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            Box::pin(handle_meerkat_resume(
                &state,
                bounded_resume_input(session_id.clone(), vec![callback]),
                None,
                None,
            )),
        )
        .await
        .expect("successful resume with callback tools must not deadlock");
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "callback resume must reach its post-completion boundary: {result:?}"
        );

        let committed = state
            .runtime_ingress_context()
            .current_callback_tools(&parsed)
            .await
            .expect("successful callback resume must publish callback config");
        assert!(
            committed
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == "resume_callback")
        );

        let empty_resume = tokio::time::timeout(
            Duration::from_secs(3),
            Box::pin(handle_meerkat_resume(
                &state,
                bounded_resume_input(session_id.clone(), vec![]),
                None,
                None,
            )),
        )
        .await
        .expect("empty-tools resume must not deadlock");
        assert!(
            empty_resume.is_ok()
                || empty_resume.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "empty-tools resume must reach its post-completion boundary: {empty_resume:?}"
        );
        let preserved = state
            .runtime_ingress_context()
            .current_callback_tools(&parsed)
            .await
            .expect("empty-tools resume must preserve callback config");
        assert!(
            preserved
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == "resume_callback")
        );

        let replacement = McpToolDef {
            name: "replacement_callback".to_string(),
            description: "Replacement callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let mut rejected_override = bounded_resume_input(session_id, vec![replacement]);
        rejected_override.output_schema = Some(json!("not-an-output-schema"));
        let error = Box::pin(handle_meerkat_resume(&state, rejected_override, None, None))
            .await
            .expect_err("invalid resume override must fail before publication");
        assert!(error.message.contains("Invalid output_schema"));
        let after_rejection = state
            .runtime_ingress_context()
            .current_callback_tools(&parsed)
            .await
            .expect("failed override must retain prior callback config");
        let after_rejection_tools = after_rejection.tools();
        assert!(
            after_rejection_tools
                .iter()
                .any(|tool| tool.name.as_ref() == "resume_callback")
        );
        assert!(
            !after_rejection_tools
                .iter()
                .any(|tool| tool.name.as_ref() == "replacement_callback")
        );
    }

    #[tokio::test]
    async fn resume_missing_session_cleanup_does_not_reenter_registration_lock() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let missing = meerkat::SessionId::new();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            Box::pin(handle_meerkat_resume(
                &state,
                bounded_resume_input(missing.to_string(), vec![]),
                None,
                None,
            )),
        )
        .await
        .expect("missing-session cleanup must not deadlock");
        assert!(result.is_err());
        assert!(!state.runtime_adapter.contains_session(&missing).await);
        assert!(!state.runtime_ingress_context().has_sidecar(&missing).await);
    }

    #[tokio::test]
    async fn archived_resume_cleanup_does_not_reenter_registration_lock() {
        let (state, session_id) = state_with_persisted_session().await;
        let session_id = meerkat::SessionId::parse(&session_id).expect("valid session id");
        attach_test_mcp_router(
            &state,
            &session_id,
            Arc::new(meerkat_mcp::McpRouterAdapter::new(McpRouter::new())),
        )
        .await;

        tokio::time::timeout(
            Duration::from_secs(3),
            state.cleanup_archived_session_runtime(&session_id),
        )
        .await
        .expect("archived resume cleanup must not deadlock")
        .expect("archived resume cleanup should succeed");

        assert!(!state.runtime_adapter.contains_session(&session_id).await);
        assert!(
            !state
                .runtime_ingress_context()
                .has_sidecar(&session_id)
                .await
        );
    }

    fn mock_deferred_materialization_request(
        session: Session,
        bindings: meerkat_core::SessionRuntimeBindings,
        max_inline_peer_notifications: Option<i32>,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "mock-materialization".to_string(),
            prompt: "deferred materialization".to_string().into(),
            system_prompt: meerkat::SystemPromptOverride::Inherit,
            max_tokens: Some(128),
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                provider: Some(Provider::Other),
                resume_session: Some(session),
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                )),
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                max_inline_peer_notifications,
                ..Default::default()
            }),
            labels: None,
        }
    }

    #[cfg(feature = "mob")]
    async fn insert_mcp_archive_partial_destroy_mob(
        state: &MeerkatMcpState,
        owner_session_id: &str,
    ) -> (
        meerkat_mob::MobId,
        Arc<meerkat_mob::store::InMemoryMobEventStore>,
    ) {
        let mob_id = meerkat_mob::MobId::from("mcp-session-archive-partial-destroy");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let owner_session_id =
            meerkat::SessionId::parse(owner_session_id).expect("valid owner bridge session id");
        let events = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
        events.fail_clear_until_allowed();
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_owner_bridge_session_create_authority(owner_session_id, true, false)
            .with_session_service(state.mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create archive-owned mob with failing event clear");
        state
            .mob_state
            .mob_insert_handle(mob_id.clone(), handle)
            .await;
        (mob_id, events)
    }

    #[cfg(feature = "mob")]
    async fn insert_mcp_archive_live_member(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> (meerkat_mob::MobId, meerkat::SessionId) {
        let mob_id = meerkat_mob::MobId::from("mcp-session-archive-live-member");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..meerkat_mob::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let handle = meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create live MCP archive mob");
        let identity = meerkat_mob::AgentIdentity::from("worker-1");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                identity.clone(),
            ))
            .await
            .expect("spawn live MCP mob member");
        let bridge_session_id = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("turn-driven worker should have a bridge session");
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        (mob_id, bridge_session_id)
    }

    fn seed_active_external_surface(
        handle: &dyn meerkat_core::handles::ExternalToolSurfaceHandle,
        surface_id: &str,
    ) {
        handle
            .stage_add(surface_id.to_string(), 0)
            .expect("seed stage add");
        let staged_sequence = handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.staged_intent_sequence)
            .expect("seed staged sequence");
        handle
            .apply_boundary(surface_id.to_string(), 0, staged_sequence, staged_sequence)
            .expect("seed apply boundary");
        let pending_sequence = handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.pending_task_sequence)
            .expect("seed pending sequence");
        handle
            .mark_pending_succeeded(surface_id.to_string(), pending_sequence, staged_sequence)
            .expect("seed active surface");
    }

    async fn cancelled_request_context(key: &str) -> RequestContext {
        use meerkat::surface::CancelOutcome;
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let context = executor.begin_request(key.to_string(), noop_request_action());
        let outcome = executor.cancel_request(key).await;
        assert_eq!(
            outcome,
            CancelOutcome::Cancelled,
            "pre-cancel should transition Pending → Cancelled"
        );
        context
    }

    fn hooks_override_fixture() -> HookRunOverrides {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../test-fixtures/hooks/run_override.json");
        let payload = std::fs::read_to_string(path).expect("hook override fixture must exist");
        serde_json::from_str::<HookRunOverrides>(&payload)
            .expect("hook override fixture must deserialize")
    }

    #[test]
    fn test_config_envelope_value_redacts_paths_by_default() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("sqlite".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_sqlite_path: Some("/tmp/root/sessions.sqlite3".to_string()),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, false).expect("envelope value");
        assert!(value.get("resolved_paths").is_none());
    }

    #[test]
    fn test_config_envelope_value_includes_paths_when_enabled() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("sqlite".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_sqlite_path: Some("/tmp/root/sessions.sqlite3".to_string()),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, true).expect("envelope value");
        assert!(value.get("resolved_paths").is_some());
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_tool_payload_uses_typed_comms_result_contract() {
        let envelope_id = "550e8400-e29b-41d4-a716-446655440000"
            .parse()
            .expect("valid envelope uuid");
        let interaction_id = meerkat_core::interaction::InteractionId(
            "550e8400-e29b-41d4-a716-446655440001"
                .parse()
                .expect("valid interaction uuid"),
        );

        let wrapped = comms_send_tool_payload(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        })
        .expect("comms send payload serializes");
        let payload = unwrap_payload(wrapped);
        let receipt = &payload["receipt"];

        assert_eq!(receipt["kind"], serde_json::json!("peer_request_sent"));
        assert_eq!(
            receipt["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            receipt["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_peers_tool_payload_uses_typed_core_wire_contract() {
        let wrapped = comms_peers_tool_payload(vec![sample_peer_directory_entry()])
            .expect("comms peers payload serializes");
        let payload = unwrap_payload(wrapped);

        assert_peer_directory_wire(&payload);
    }

    #[cfg(feature = "comms")]
    fn sample_peer_directory_entry() -> meerkat_core::comms::PeerDirectoryEntry {
        meerkat_core::comms::PeerDirectoryEntry {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("agent").unwrap(),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "agent",
            ),
            source: meerkat_core::comms::PeerDirectorySource::Inproc,
            sendable_kinds: vec![
                meerkat_core::comms::PeerSendability::PeerMessage,
                meerkat_core::comms::PeerSendability::PeerRequest,
            ],
            capabilities: meerkat_core::comms::PeerCapabilitySet::default()
                .with_extension("vendor.echo", serde_json::json!({ "enabled": true })),
            meta: meerkat_core::PeerMeta::default(),
        }
    }

    #[cfg(feature = "comms")]
    fn assert_peer_directory_wire(result: &Value) {
        let peer = &result["peers"][0];

        assert_eq!(peer["source"], "inproc");
        assert_eq!(
            peer["sendable_kinds"],
            serde_json::json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
    }

    #[test]
    fn test_config_runtime_error_generation_conflict_is_typed() {
        let err = map_config_runtime_error(ConfigRuntimeError::GenerationConflict {
            expected: 2,
            current: 5,
        });
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("generation conflict"));
        let data = err.data.expect("generation conflict should include data");
        assert_eq!(data["type"], "generation_conflict");
        assert_eq!(data["expected_generation"], 2);
        assert_eq!(data["current_generation"], 5);
    }

    #[test]
    fn test_workgraph_tool_error_mapping_preserves_generated_classes() {
        use meerkat::WorkGraphToolErrorCode;
        let cases = [
            (WorkGraphToolErrorCode::InvalidArguments, -32602),
            (WorkGraphToolErrorCode::NotFound, -32601),
            (WorkGraphToolErrorCode::Conflict, -32602),
            (WorkGraphToolErrorCode::InvalidTransition, -32602),
            (
                WorkGraphToolErrorCode::CapabilityUnavailable,
                meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            ),
            (
                WorkGraphToolErrorCode::StoreError,
                meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            ),
            (
                WorkGraphToolErrorCode::InternalError,
                meerkat_contracts::ErrorCode::InternalError.jsonrpc_code(),
            ),
        ];

        for (tool_code, rpc_code) in cases {
            let err = map_workgraph_tool_error(meerkat::WorkGraphToolError {
                code: tool_code,
                message: format!("workgraph {tool_code:?}"),
            });
            assert_eq!(err.code, rpc_code, "tool code {tool_code:?}");
        }
    }

    #[test]
    fn test_validate_config_for_commit_rejects_invalid_skills_identity() {
        let mut config = Config::default();
        let source_uuid =
            meerkat_core::skills::SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("uuid");
        config.skills.repositories = vec![
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "a".into(),
                source_uuid: source_uuid.clone(),
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/a".to_string(),
                },
            },
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "b".into(),
                source_uuid,
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/b".to_string(),
                },
            },
        ];

        let err = validate_config_for_commit(&config).expect_err("duplicate source uuid");
        assert_eq!(err.code, -32602);
        assert!(
            err.message
                .contains("Invalid skills source-identity config")
        );
    }

    #[test]
    fn test_config_set_rejects_payload_that_would_default_missing_fields() {
        let raw = json!({
            "max_tokens": 448
        });
        let parsed: Config = serde_json::from_value(raw.clone())
            .expect("serde would otherwise fabricate config defaults");
        let err = reject_config_payload_defaulting(&raw, &parsed)
            .expect_err("partial config set must be rejected");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("missing field `config.agent`"));
    }

    #[test]
    fn test_config_set_accepts_complete_typed_payload() {
        let config = Config::default();
        let raw = serde_json::to_value(&config).expect("config serializes");
        reject_config_payload_defaulting(&raw, &config).expect("complete config is accepted");
    }

    #[tokio::test]
    async fn test_config_rejects_unknown_action_string_before_dispatch() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({
                "action": "default_if_unknown",
                "config": {"max_tokens": 448}
            }),
        ))
        .await
        .expect_err("unknown config action string must be rejected before dispatch");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("unknown variant") || err.message.contains("expected one of"));
    }

    #[cfg(feature = "mob")]
    fn unwrap_tool_payload_json(value: Value) -> Value {
        let text = value["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        serde_json::from_str(text).expect("json payload")
    }

    #[test]
    fn test_tools_list_schema() {
        let tools = tools_list();
        let schedule_tool_count = meerkat::schedule_tools_list().len();
        let workgraph_tool_count = meerkat::unscoped_workgraph_tools_list().len();
        #[cfg(all(feature = "comms", feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
                + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), feature = "mob"))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + mob_host_tools_list().len()
                + mob_event_stream_tools_list().len()
        );
        #[cfg(all(feature = "comms", not(feature = "mob")))]
        assert_eq!(
            tools.len(),
            base_tools_list().len()
                + schedule_tool_count
                + workgraph_tool_count
                + comms_tools_list().len()
        );
        #[cfg(all(not(feature = "comms"), not(feature = "mob")))]
        assert_eq!(
            tools.len(),
            base_tools_list().len() + schedule_tool_count + workgraph_tool_count
        );

        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!tool_names.contains(&"workgraph_attention_reassign"));
        let find_tool = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .expect("tool should be present")
        };

        let run_tool = find_tool("meerkat_run");
        let help_tool = find_tool("meerkat_help");
        assert!(help_tool["inputSchema"]["properties"]["question"].is_object());
        assert!(help_tool["inputSchema"]["properties"]["prompt"].is_object());
        assert!(help_tool["inputSchema"]["properties"]["execution_mode"].is_object());
        assert!(run_tool["inputSchema"]["properties"]["prompt"].is_object());
        assert!(run_tool["inputSchema"]["properties"]["verbose"].is_object());
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("output_schema")
                .is_some()
        );
        assert_eq!(
            find_tool("meerkat_schedule_create")["name"],
            "meerkat_schedule_create"
        );
        assert_eq!(
            find_tool("meerkat_schedule_update")["name"],
            "meerkat_schedule_update"
        );
        assert_eq!(
            find_tool("meerkat_schedule_occurrences")["name"],
            "meerkat_schedule_occurrences"
        );
        assert_eq!(find_tool("workgraph_create")["name"], "workgraph_create");
        assert_eq!(find_tool("workgraph_ready")["name"], "workgraph_ready");
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("structured_output_retries")
                .is_some()
        );

        let resume_tool = find_tool("meerkat_resume");
        assert!(resume_tool["inputSchema"]["properties"]["session_id"].is_object());
        assert!(resume_tool["inputSchema"]["properties"]["verbose"].is_object());

        let config_tool = find_tool("meerkat_config");
        assert!(config_tool["inputSchema"]["properties"]["action"].is_object());

        let capabilities_tool = find_tool("meerkat_capabilities");
        assert_eq!(capabilities_tool["name"], "meerkat_capabilities");
        let models_catalog_tool = find_tool("meerkat_models_catalog");
        assert_eq!(models_catalog_tool["name"], "meerkat_models_catalog");
        let read_tool = find_tool("meerkat_read");
        assert_eq!(read_tool["name"], "meerkat_read");
        let history_tool = find_tool("meerkat_history");
        assert_eq!(history_tool["name"], "meerkat_history");
        let sessions_tool = find_tool("meerkat_sessions");
        assert_eq!(sessions_tool["name"], "meerkat_sessions");
        let interrupt_tool = find_tool("meerkat_interrupt");
        assert_eq!(interrupt_tool["name"], "meerkat_interrupt");
        let archive_tool = find_tool("meerkat_archive");
        assert_eq!(archive_tool["name"], "meerkat_archive");
        let mcp_add_tool = find_tool("meerkat_mcp_add");
        assert_eq!(mcp_add_tool["name"], "meerkat_mcp_add");
        assert!(
            mcp_add_tool["inputSchema"]["properties"]
                .get("server_config")
                .is_some()
        );
        assert!(
            mcp_add_tool["inputSchema"]["properties"]
                .get("server_name")
                .is_none()
        );
        let mcp_remove_tool = find_tool("meerkat_mcp_remove");
        assert_eq!(mcp_remove_tool["name"], "meerkat_mcp_remove");
        let mcp_reload_tool = find_tool("meerkat_mcp_reload");
        assert_eq!(mcp_reload_tool["name"], "meerkat_mcp_reload");
        let event_stream_open_tool = find_tool("meerkat_event_stream_open");
        assert_eq!(event_stream_open_tool["name"], "meerkat_event_stream_open");
        let event_stream_read_tool = find_tool("meerkat_event_stream_read");
        assert_eq!(event_stream_read_tool["name"], "meerkat_event_stream_read");
        let event_stream_close_tool = find_tool("meerkat_event_stream_close");
        assert_eq!(
            event_stream_close_tool["name"],
            "meerkat_event_stream_close"
        );

        #[cfg(feature = "mob")]
        {
            assert!(!tool_names.contains(&"meerkat_mob_prefabs"));
            assert!(!tool_names.contains(&"mob_create"));
            assert!(!tool_names.contains(&"mob_list"));
            assert!(!tool_names.contains(&"mob_lifecycle"));
            assert!(tool_names.contains(&"meerkat_mob_create"));
            assert!(tool_names.contains(&"meerkat_mob_list"));
            assert!(tool_names.contains(&"meerkat_mob_status"));
            assert!(tool_names.contains(&"meerkat_mob_lifecycle"));
            assert!(tool_names.contains(&"meerkat_mob_spawn"));
            assert!(tool_names.contains(&"meerkat_mob_spawn_many"));
            assert!(tool_names.contains(&"meerkat_mob_member_send"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_open"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_read"));
            assert!(tool_names.contains(&"meerkat_mob_event_stream_close"));
        }
        #[cfg(not(feature = "mob"))]
        {
            assert!(!tool_names.contains(&"mob_create"));
            assert!(!tool_names.contains(&"mob_list"));
            assert!(!tool_names.contains(&"mob_lifecycle"));
            assert!(!tool_names.contains(&"meerkat_mob_create"));
            assert!(!tool_names.contains(&"meerkat_mob_list"));
            assert!(!tool_names.contains(&"meerkat_mob_status"));
            assert!(!tool_names.contains(&"meerkat_mob_lifecycle"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_open"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_read"));
            assert!(!tool_names.contains(&"meerkat_mob_event_stream_close"));
        }

        #[cfg(feature = "comms")]
        {
            assert!(tool_names.contains(&"meerkat_comms_send"));
            assert!(tool_names.contains(&"meerkat_comms_peers"));
        }
    }

    #[test]
    fn base_tool_descriptor_owns_request_lifecycle() {
        // The typed descriptor table is the single owner of base MCP tool
        // lifecycle: the resolver reads the classification straight off the
        // descriptor co-located with name/description/schema.
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_help"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_run"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_resume"),
            Some(RequestLifecycle::LongRunningPublishOnSuccess)
        );
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_sessions"),
            Some(RequestLifecycle::LongRunningObservation)
        );
        // A tool contributed by another surface resolves through that
        // surface's feature-owned declaration, not a default fall-through.
        assert_eq!(
            mcp_tool_request_lifecycle("meerkat_schedule_create"),
            Some(RequestLifecycle::LongRunningObservation)
        );
        // An unadvertised name has no lifecycle: the resolver fails closed
        // with `None` rather than classifying it under a default.
        assert_eq!(mcp_tool_request_lifecycle("definitely_not_a_tool"), None);
    }

    #[test]
    fn every_advertised_tool_has_an_owned_lifecycle() {
        // Completeness gate: every tool `tools_list` advertises — base AND
        // contributed (schedule/workgraph/mob/comms) — must resolve to a
        // feature-owned lifecycle. A `None` here means a contributed surface
        // was composed into the advertisement without declaring its
        // lifecycle.
        for tool in tools_list() {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .expect("advertised tool carries a name");
            assert!(
                mcp_tool_request_lifecycle(name).is_some(),
                "advertised tool '{name}' has no feature-owned request lifecycle"
            );
        }
    }

    #[test]
    fn base_tools_list_projects_every_descriptor() {
        // The advertised JSON is a faithful projection of the typed descriptor
        // table — same count, same names, and each carries its schema.
        let descriptors = base_tool_descriptors();
        let tools = base_tools_list();
        assert_eq!(tools.len(), descriptors.len());
        for (descriptor, tool) in descriptors.iter().zip(tools.iter()) {
            assert_eq!(tool["name"], descriptor.name);
            assert_eq!(tool["description"], descriptor.description);
            assert!(tool["inputSchema"].is_object());
        }
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_tools_list_omits_comms_tools_when_feature_disabled() {
        let tools = tools_list();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&"meerkat_comms_send"));
        assert!(!names.contains(&"meerkat_comms_peers"));
        assert!(names.contains(&"meerkat_event_stream_open"));
        assert!(names.contains(&"meerkat_event_stream_read"));
        assert!(names.contains(&"meerkat_event_stream_close"));
    }

    #[test]
    fn test_meerkat_run_input_parsing() {
        let input_json = json!({
            "prompt": "Hello",
            "model": "claude-sonnet-4"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(
            input.model,
            Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                "claude-sonnet-4"
            ))
        );
        assert_eq!(input.max_tokens, None);
        assert_eq!(input.structured_output_retries, None);
        assert!(input.output_schema.is_none());
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_resume_input_parsing() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue"
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.prompt, "Continue");
        assert!(input.system_prompt.is_none());
        assert!(input.output_schema.is_none());
        assert_eq!(input.structured_output_retries, None);
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_run_input_with_tools() {
        let input_json = json!({
            "prompt": "Hello",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather for a city",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        },
                        "required": ["city"]
                    }
                }
            ]
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.tools.len(), 1);
        assert_eq!(input.tools[0].name, "get_weather");
        assert_eq!(input.tools[0].handler_kind(), "callback"); // default
    }

    #[test]
    fn test_meerkat_resume_input_with_tool_results() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "",
            "tool_results": [
                {
                    "tool_use_id": "tc_123",
                    "content": "Sunny, 72°F"
                }
            ]
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.tool_results.len(), 1);
        assert_eq!(input.tool_results[0].tool_use_id, "tc_123");
        assert_eq!(input.tool_results[0].content, "Sunny, 72\u{b0}F");
        assert!(!input.tool_results[0].is_error);
    }

    #[test]
    fn test_meerkat_run_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "prompt": "Hello",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[0].point,
            meerkat_core::HookPoint::PreToolExecution
        );
    }

    #[test]
    fn test_meerkat_resume_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[1].mode,
            meerkat_core::HookExecutionMode::Background
        );
    }

    #[test]
    fn test_format_agent_result_includes_skill_diagnostics() {
        let session_id = meerkat::SessionId::new();
        let result = meerkat_core::types::RunResult {
            text: "ok".to_string(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Unhealthy,
                    invalid_ratio: 0.9,
                    invalid_count: 9,
                    total_count: 10,
                    failure_streak: 10,
                    handshake_failed: true,
                },
                quarantined: vec![],
                collection_fault: None,
            }),
        };

        let payload = format_agent_result(Ok(result), &session_id).expect("formatted payload");
        let raw = payload["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        let decoded: serde_json::Value = serde_json::from_str(raw).expect("valid wrapped JSON");
        assert_eq!(decoded["content"][0]["text"], "ok");
        assert_eq!(
            decoded["skill_diagnostics"]["source_health"]["state"],
            "unhealthy"
        );
    }

    #[test]
    fn test_mpc_tool_dispatcher_creates_tool_defs() {
        let mcp_tools = vec![
            McpToolDef {
                name: "get_weather".into(),
                description: "Get weather".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
            McpToolDef {
                name: "search".into(),
                description: "Search".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
        ];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let tool_defs = dispatcher.tools();

        assert_eq!(tool_defs.len(), 2);
        assert_eq!(tool_defs[0].name, "get_weather");
        assert_eq!(tool_defs[1].name, "search");
    }

    #[tokio::test]
    async fn test_mpc_tool_dispatcher_returns_callback_error() {
        let mcp_tools = vec![McpToolDef {
            name: "get_weather".into(),
            description: "Get weather".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        }];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let args_raw =
            serde_json::value::RawValue::from_string(json!({"city": "Tokyo"}).to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "get_weather",
            args: &args_raw,
        };
        let result = dispatcher.dispatch(call).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_callback_pending(), "Expected CallbackPending error");
        if let Some((tool_name, args)) = err.as_callback_pending() {
            assert_eq!(tool_name, "get_weather");
            assert_eq!(args["city"], "Tokyo");
        }
    }

    #[test]
    fn test_tools_list_has_tools_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify tools parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["tools"].is_object());
        assert_eq!(
            run_tool["inputSchema"]["properties"]["tools"]["type"],
            "array"
        );
    }

    #[test]
    fn test_tools_list_has_tool_results_parameter() {
        let tools = tools_list();
        let resume_tool = &tools[1];

        // Verify tool_results parameter exists in the schema
        let tool_results = &resume_tool["inputSchema"]["properties"]["tool_results"];
        assert!(tool_results.is_object(), "tool_results should be an object");
        assert_eq!(
            tool_results["type"], "array",
            "tool_results should be an array"
        );
        // Items may use $ref for the ToolResultInput schema
        assert!(
            tool_results["items"].is_object(),
            "tool_results items should be defined"
        );
    }

    #[test]
    fn test_tools_list_has_blob_get_tool() {
        let blob_tool = tools_list()
            .into_iter()
            .find(|tool| tool["name"] == "meerkat_blob_get")
            .expect("meerkat_blob_get tool must exist");
        assert_eq!(blob_tool["name"], "meerkat_blob_get");
        assert_eq!(
            blob_tool["inputSchema"]["properties"]["blob_id"]["type"],
            "string"
        );
    }

    #[test]
    fn test_tools_list_skills_schema_has_source_selector() {
        let tools = tools_list();
        let skills_tool = tools.iter().find(|t| t["name"] == "meerkat_skills");
        let skills_tool = skills_tool.expect("meerkat_skills tool must exist");
        assert!(
            skills_tool["inputSchema"]["properties"]["source"].is_object(),
            "skills inspect should expose optional source selector"
        );
    }

    #[test]
    fn test_tools_list_skills_schema_has_typed_action_enum() {
        let tools = tools_list();
        let skills_tool = tools
            .iter()
            .find(|t| t["name"] == "meerkat_skills")
            .expect("meerkat_skills tool must exist");
        let action_schema = &skills_tool["inputSchema"]["properties"]["action"];
        let action_enum = action_schema.get("enum").or_else(|| {
            let action_ref = action_schema["$ref"].as_str()?;
            let definition = action_ref
                .strip_prefix("#/$defs/")
                .or_else(|| action_ref.strip_prefix("#/definitions/"))?;
            skills_tool["inputSchema"]["$defs"][definition]
                .get("enum")
                .or_else(|| skills_tool["inputSchema"]["definitions"][definition].get("enum"))
        });
        assert_eq!(
            action_enum,
            Some(&serde_json::json!(["list", "inspect"])),
            "skills action should be a closed typed enum, got {action_schema}"
        );

        let input: MeerkatSkillsInput =
            serde_json::from_value(serde_json::json!({ "action": "list" })).unwrap();
        assert!(matches!(input.action, MeerkatSkillsAction::List));
    }

    /// Gate for dogma row #133: MCP skills provenance must PROJECT the real
    /// `SourceIdentityRecord` from the runtime's identity registry, never
    /// fabricate a synthetic `mcp:{display}` / Embedded record from display
    /// data. The wire entry's transport and fingerprint must equal the registry
    /// record exactly.
    #[test]
    fn test_skill_entry_projects_real_registry_provenance_not_synthetic() {
        use meerkat_core::skills::{
            SkillDescriptor, SkillKey, SkillName, SkillScope, SourceIdentityRecord,
            SourceIdentityStatus, SourceTransportKind, SourceUuid,
        };

        let source_uuid = SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").expect("uuid");
        // The authoritative record the registry would surface: a Filesystem
        // source with a real content fingerprint — NOT the synthetic
        // `mcp:{display_name}` / Embedded shape.
        let registry_identity = SourceIdentityRecord {
            source_uuid: source_uuid.clone(),
            display_name: "company-skills".to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: "sha256:abc123".to_string(),
            status: SourceIdentityStatus::Active,
        };

        let key = SkillKey::new(
            source_uuid,
            SkillName::parse("email-extractor").expect("name"),
        );
        let mut descriptor = SkillDescriptor::new(key, "Email Extractor", "Extracts email");
        descriptor.scope = SkillScope::Project;
        descriptor.source_name = "company-skills".to_string();

        let entry = meerkat_core::skills::SkillIntrospectionEntry {
            descriptor,
            source_identity: Some(registry_identity.clone()),
            shadowed_by: None,
            shadowed_by_identity: None,
            shadowed_by_source_uuid: None,
            is_active: true,
        };

        let wire = skill_entry_from_introspection(&entry).expect("real provenance projects");

        // Provenance must equal the registry record verbatim.
        assert_eq!(wire.source.identity, registry_identity);
        assert_eq!(
            wire.source.identity.transport_kind,
            SourceTransportKind::Filesystem
        );
        assert_eq!(wire.source.identity.fingerprint, "sha256:abc123");
        // The synthetic forms must be gone.
        assert_ne!(
            wire.source.identity.transport_kind,
            SourceTransportKind::Embedded
        );
        assert!(
            !wire.source.identity.fingerprint.starts_with("mcp:"),
            "fingerprint must not be the synthetic mcp: form, got {}",
            wire.source.identity.fingerprint
        );

        // Fail closed: an entry without a typed identity must error rather than
        // fabricate one.
        let mut orphan = entry;
        orphan.source_identity = None;
        let err = skill_entry_from_introspection(&orphan)
            .expect_err("missing typed identity must fail closed");
        assert!(
            err.contains("missing typed source identity"),
            "unexpected error: {err}"
        );
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_keep_alive_rejects_when_comms_disabled() {
        let err = resolve_keep_alive(Some(true)).expect_err("keep_alive should be rejected");
        assert!(err.contains("keep_alive requires comms support"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_keep_alive_allows_when_comms_enabled() {
        assert_eq!(resolve_keep_alive(Some(true)).unwrap(), Some(true));
        assert_eq!(resolve_keep_alive(Some(false)).unwrap(), Some(false));
        assert_eq!(resolve_keep_alive(None).unwrap(), None);
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_with_keep_alive() {
        let input_json = json!({
            "prompt": "Hello",
            "keep_alive": true,
            "comms_name": "test-agent"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.keep_alive, Some(true));
        assert_eq!(input.comms_name, Some("test-agent".to_string()));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_keep_alive_defaults_to_none() {
        let input_json = json!({
            "prompt": "Hello"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.keep_alive, None);
        assert!(input.comms_name.is_none());
    }

    #[test]
    fn test_meerkat_resume_input_preserves_optional_override_presence() {
        let omitted: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume"
        }))
        .unwrap();
        assert_eq!(omitted.enable_builtins, None);
        assert_eq!(omitted.structured_output_retries, None);
        assert!(
            omitted
                .builtin_config
                .as_ref()
                .is_none_or(|cfg| cfg.enable_shell.is_none())
        );

        let explicit: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume",
            "enable_builtins": false,
            "structured_output_retries": 4,
            "builtin_config": {
                "enable_shell": false
            }
        }))
        .unwrap();
        assert_eq!(explicit.enable_builtins, Some(false));
        assert_eq!(explicit.structured_output_retries, Some(4));
        assert_eq!(
            explicit
                .builtin_config
                .as_ref()
                .and_then(|cfg| cfg.enable_shell),
            Some(false)
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_includes_structured_details() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::PeerOffline,
        );
        assert_eq!(err.code, -32603);
        assert!(err.message.starts_with("peer_unreachable:"));
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_unreachable")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("reason").and_then(Value::as_str),
            Some("offline_or_no_ack")
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_transport_maps_to_peer_unreachable() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::Transport(
                "Transport error: connection refused".into(),
            ),
        );
        assert_eq!(err.code, -32603);
        assert!(err.message.starts_with("peer_unreachable:"));
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_unreachable")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("reason").and_then(Value::as_str),
            Some("transport_error")
        );
        assert_eq!(
            data.get("details").and_then(Value::as_str),
            Some("Transport error: connection refused")
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_preserves_admission_drop_reason() {
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::AdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::ClassificationRejected,
            },
        );
        assert_eq!(err.code, -32603);
        assert!(err.message.starts_with("peer_admission_dropped:"));
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_admission_dropped")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("reason").and_then(Value::as_str),
            Some("classification_rejected")
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_run_keep_alive_requires_comms_name() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let result = Box::pin(handle_meerkat_run(
            &state,
            MeerkatRunInput {
                prompt: "test".to_string(),
                system_prompt: None,
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-opus-4-8",
                )),
                max_tokens: Some(4096),
                provider: None,
                output_schema: None,
                structured_output_retries: Some(2),
                stream: false,
                verbose: false,
                tools: vec![],
                enable_builtins: Some(false),
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None, // Missing!
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                labels: None,
                additional_instructions: None,
                app_context: None,
                shell_env: None,
                auth_binding: None,
            },
            None,
            None,
        ))
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("comms_name"));
    }

    #[tokio::test]
    async fn test_handle_meerkat_run_returns_request_cancelled_when_pre_cancelled() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let context = cancelled_request_context("req-run-cancelled").await;

        let result = Box::pin(handle_meerkat_run(
            &state,
            MeerkatRunInput {
                prompt: "test".to_string(),
                system_prompt: None,
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "claude-opus-4-8",
                )),
                max_tokens: Some(4096),
                provider: None,
                output_schema: None,
                structured_output_retries: Some(2),
                stream: false,
                verbose: false,
                tools: vec![],
                enable_builtins: Some(false),
                builtin_config: None,
                keep_alive: Some(false),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                labels: None,
                additional_instructions: None,
                app_context: None,
                shell_env: None,
                auth_binding: None,
            },
            None,
            Some(context),
        ))
        .await;

        let err = result.expect_err("pre-cancelled run should be rejected");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[test]
    fn external_tool_composition_retains_an_initially_empty_dynamic_router() {
        let primary = Arc::new(MpcToolDispatcher::new(&[McpToolDef {
            name: "callback_tool".to_string(),
            description: "Callback tool".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        }])) as Arc<dyn AgentToolDispatcher>;
        let live_tools = Arc::new(StdMutex::new(Vec::new()));
        let dynamic = Arc::new(MutableToolDispatcher {
            tools: Arc::clone(&live_tools),
        }) as Arc<dyn AgentToolDispatcher>;

        let composed = compose_external_tool_dispatchers(Some(primary), Some(dynamic))
            .expect("empty dynamic router must remain composable")
            .expect("two dispatchers must produce a composite");
        assert_eq!(
            composed.tools().len(),
            1,
            "only the callback tool is initially visible"
        );

        live_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::new(ToolDef {
                name: "late_mcp_tool".into(),
                description: "Connected after agent construction".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                provenance: Some(meerkat_core::types::ToolProvenance {
                    kind: meerkat_core::types::ToolSourceKind::Mcp,
                    source_id: "late-server".into(),
                }),
            }));

        assert!(
            composed
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == "late_mcp_tool"),
            "the composite must query its retained dynamic router after connection"
        );
    }

    #[tokio::test]
    async fn test_meerkat_run_composition_failure_unregisters_prepared_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_and_max_sessions(store, Some(1)).await;
        let prepared = prepare_surface_session(&state.runtime_adapter)
            .await
            .expect("initial runtime slot should prepare");
        let primary_tool = McpToolDef {
            name: "primary_callback".to_string(),
            description: "Primary callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let duplicate_secondary = McpToolDef {
            name: "duplicate_secondary".to_string(),
            description: "Duplicate secondary callback".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        };
        let primary =
            Arc::new(MpcToolDispatcher::new(&[primary_tool])) as Arc<dyn AgentToolDispatcher>;
        let secondary = Arc::new(MpcToolDispatcher::new(&[
            duplicate_secondary.clone(),
            duplicate_secondary,
        ])) as Arc<dyn AgentToolDispatcher>;

        let result = compose_run_external_tool_dispatchers(
            &state,
            &prepared.session_id,
            Some(primary),
            Some(secondary),
        )
        .await;

        let err = match result {
            Ok(_) => panic!("duplicate composed tools should fail composition"),
            Err(err) => err,
        };
        assert!(
            err.message.contains("failed to compose external tools"),
            "expected composition failure, got {err:?}"
        );

        let next_prepared = prepare_surface_session(&state.runtime_adapter)
            .await
            .expect("composition failure should release the single runtime slot");
        drop(next_prepared);
    }

    #[tokio::test]
    async fn exact_mcp_rebuild_detaches_old_executor_and_discards_old_actor() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let legacy_bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("legacy fixture bindings");
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            mock_deferred_materialization_request(session, legacy_bindings, None),
            None,
            &meerkat::LiveSessionActorWitnessSlot::default(),
        )
        .await
        .expect("fixture actor should materialize");
        let pending = state
            .runtime_adapter
            .ensure_session_with_executor_factory(session_id.clone(), |_witness| {
                Box::new(RuntimeTerminationFixtureExecutor(None))
                    as Box<dyn meerkat_core::lifecycle::CoreExecutor>
            })
            .await
            .expect("fixture runtime executor should prepare");
        let pending = match pending {
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            meerkat_runtime::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fixture unexpectedly found attached executor {witness:?}")
            }
        };
        pending
            .commit()
            .await
            .expect("fixture runtime executor should attach");
        assert!(
            state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live actor lookup")
        );
        assert!(
            state
                .runtime_adapter
                .session_has_executor(&session_id)
                .await
                .expect("executor lookup")
        );

        let mut prepared =
            prepare_mcp_actor_materialization(&state.service, &state.runtime_adapter, &session_id)
                .await
                .expect("rebuild should replace old runtime ownership with an exact claim");
        assert!(
            !state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live actor lookup after rebuild prepare"),
            "old actor must be discarded before replacement creation"
        );
        assert!(state.runtime_adapter.contains_session(&session_id).await);
        assert!(
            !state
                .runtime_adapter
                .session_has_executor(&session_id)
                .await
                .expect("prepared executor lookup"),
            "prepared replacement must not retain the old executor"
        );

        prepared
            .rollback_now()
            .await
            .expect("exact replacement rollback");
        assert!(!state.runtime_adapter.contains_session(&session_id).await);
    }

    #[tokio::test]
    async fn runtime_backed_create_classifies_validation_failure_as_non_preserving() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let prepared = prepare_new_mcp_actor_materialization(
            &state.service,
            &state.runtime_adapter,
            &session_id,
        )
        .await
        .expect("exact materialization should prepare");

        let owned = create_runtime_backed_session_and_run_initial_turn_call_local(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            mock_deferred_materialization_request(session, prepared.bindings_clone(), Some(-2)),
            None,
            prepared,
        )
        .await
        .expect("owned validation attempt should report");
        let McpCallLocalActorCreateOutcome {
            result: error,
            transaction: mut prepared,
        } = owned;
        let error = error.expect_err("invalid inline peer budget must reject actor creation");
        assert!(matches!(
            error,
            McpRuntimeBackedCreateError::Session {
                preserves_runtime_session: false,
                ..
            }
        ));
        prepared
            .rollback_now()
            .await
            .expect("failed actor create must retain exact rollback authority");
        assert!(!state.runtime_adapter.contains_session(&session_id).await);
    }

    #[tokio::test]
    async fn durable_create_remains_discoverable_and_resumable_after_response_loss() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let prepared = prepare_new_mcp_actor_materialization(
            &state.service,
            &state.runtime_adapter,
            &session_id,
        )
        .await
        .expect("exact materialization should prepare");
        let outcome = create_runtime_backed_session_and_run_initial_turn_call_local(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            mock_deferred_materialization_request(session, prepared.bindings_clone(), None),
            None,
            prepared,
        )
        .await
        .expect("call-local create should report");
        let McpCallLocalActorCreateOutcome {
            result,
            transaction,
        } = outcome;
        result.expect("durable deferred create should succeed");

        let mut transaction = Some(transaction);
        release_mcp_actor_materialization(
            &mut transaction,
            &session_id,
            "simulated response loss after durable create",
        )
        .await
        .expect("release must preserve committed durable work");

        let persisted = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative lookup")
            .expect("durable session must remain discoverable");
        assert!(
            !state
                .service
                .session_archived_by_authority(&session_id, &persisted)
                .await
                .expect("archive authority lookup"),
            "response loss must not archive a committed create"
        );

        let mut resumed =
            prepare_mcp_actor_materialization(&state.service, &state.runtime_adapter, &session_id)
                .await
                .expect("the ordinary explicit-resume seam must reclaim the session");
        resumed
            .rollback_now()
            .await
            .expect("test resume claim should release cleanly");
    }
    #[tokio::test]
    async fn test_handle_meerkat_resume_returns_request_cancelled_when_pre_cancelled() {
        let (state, session_id) = state_with_persisted_session().await;
        let context = cancelled_request_context("req-resume-cancelled").await;

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            Some(context),
        ))
        .await;

        let err = result.expect_err("pre-cancelled resume should be rejected");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[tokio::test]
    async fn mcp_request_cancel_after_action_install_rejects_before_service_admission() {
        use meerkat::surface::CancelOutcome;

        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let context = executor.begin_request("req-cancel-before-admission", noop_request_action());
        let install = context
            .install_cancel_action_or_cancelled(noop_request_action())
            .await;
        assert_eq!(install, CancelActionInstallOutcome::Installed);

        let outcome = executor.cancel_request(context.key()).await;
        assert_eq!(outcome, CancelOutcome::Cancelled);

        let cleaned = Arc::new(AtomicBool::new(false));
        let cleaned_for_gate = Arc::clone(&cleaned);
        let err = reject_if_cancelled_before_mcp_service_admission(Some(&context), async move {
            cleaned_for_gate.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect_err("cancel after action install must reject before service admission");

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
        assert!(
            cleaned.load(Ordering::SeqCst),
            "pre-admission cancel gate must run cleanup"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_post_prepare_validation_failure_unregisters_runtime() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: Some(json!("not-a-schema")),
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("invalid output schema should reject resume");
        assert!(
            err.message.contains("Invalid output_schema"),
            "expected output schema validation error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&parsed).await,
            "post-prepare resume validation failure should unregister runtime state"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_post_prepare_validation_failure_preserves_existing_runtime()
    {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        state
            .runtime_adapter
            .ensure_session_with_executor(
                parsed.clone(),
                Box::new(RuntimeTerminationFixtureExecutor(None)),
            )
            .await
            .expect("MCP runtime executor should attach");
        assert!(
            state.runtime_adapter.contains_session(&parsed).await,
            "test requires a pre-existing runtime registration"
        );

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id,
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: Some(json!("not-a-schema")),
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("invalid output schema should reject resume");
        assert!(
            err.message.contains("Invalid output_schema"),
            "expected output schema validation error: {err:?}"
        );
        assert!(
            state.runtime_adapter.contains_session(&parsed).await,
            "post-prepare resume validation failure should preserve existing runtime state"
        );
    }

    /// Seed a session into runtime authority the way a prior host lifetime
    /// would have: a committed runtime-store snapshot (store rows alone are
    /// projection, not authority).
    async fn seed_runtime_authority_session(
        runtime_store: &Arc<dyn meerkat_runtime::RuntimeStore>,
        session: &Session,
    ) {
        use meerkat_runtime::RuntimeStore as _;
        runtime_store
            .commit_session_snapshot(
                &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session.id()),
                meerkat_runtime::store::SessionDelta {
                    session_snapshot: serde_json::to_vec(session)
                        .expect("session snapshot should serialize"),
                },
            )
            .await
            .expect("session snapshot should commit to runtime authority");
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_capacity_failure_unregisters_prepared_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        store.save(&session).await.expect("persisted session");

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "failed persisted-only resume must not leave the prepared runtime registered"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_resume_live_no_rebuild_capacity_failure_does_not_prepare_runtime()
    {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
        let pre_session = Session::new();
        let session_id = pre_session.id().clone();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should prepare");
        let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-opus-4-8".to_string(),
                prompt: "Initial live turn".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(4096),
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    resume_session: Some(pre_session),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                    )),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            },
            None,
            &meerkat::LiveSessionActorWitnessSlot::default(),
        )
        .await
        .expect("live session create should succeed");
        attach_test_mcp_router(&state, &session_id, mcp_adapter).await;
        assert!(
            state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live session lookup"),
            "test requires an existing live session"
        );
        let witness = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("runtime session should have an exact attachment");
        state
            .runtime_adapter
            .unregister_executor_attachment_if_current(&witness)
            .await
            .expect("runtime session should unregister exactly");
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "test starts with no runtime adapter registration"
        );

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume live no rebuild".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: None,
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full live no-rebuild resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "capacity failure in live no-rebuild resume must not prepare runtime bindings"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_capacity_failure_does_not_configure_peer_ingress()
     {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Some(1),
        )
        .await;
        let pre_session = Session::new();
        let session_id = pre_session.id().clone();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime bindings should prepare");
        let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &session_id,
            CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-opus-4-8".to_string(),
                prompt: "Initial live turn".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(4096),
                event_tx: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    resume_session: Some(pre_session),
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                    )),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    comms_name: Some("mcp-agent".to_string()),
                    keep_alive: false,
                    ..Default::default()
                }),
                labels: None,
            },
            None,
            &meerkat::LiveSessionActorWitnessSlot::default(),
        )
        .await
        .expect("live session create should succeed");
        attach_test_mcp_router(&state, &session_id, mcp_adapter).await;
        assert!(
            state.service.comms_runtime(&session_id).await.is_some(),
            "test requires a live session with comms runtime"
        );
        assert!(
            !state
                .runtime_adapter
                .session_has_comms(&session_id)
                .await
                .expect("session_has_comms should resolve"),
            "test starts before peer ingress has been configured"
        );

        let blocker_session = Session::new();
        let blocker_id = blocker_session.id().clone();
        store
            .save(&blocker_session)
            .await
            .expect("persist blocker session");
        seed_runtime_authority_session(&runtime_store, &blocker_session).await;
        let _blocker = state
            .service
            .reserve_runtime_turn_admission(&blocker_id)
            .await
            .expect("blocker should fill active capacity");

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume live keep alive".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("capacity-full keep_alive resume should reject");
        assert!(
            err.message.contains("Max sessions"),
            "expected capacity error: {err:?}"
        );
        assert!(
            !state
                .runtime_adapter
                .session_has_comms(&session_id)
                .await
                .expect("session_has_comms should resolve"),
            "capacity failure must not configure peer ingress before active admission"
        );
        let stored = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative session")
            .expect("session should remain persisted");
        assert!(
            !stored
                .session_metadata()
                .expect("session metadata")
                .keep_alive,
            "capacity failure must not persist keep_alive before active admission"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_live_missing_failure_unregisters_runtime() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-8".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("stale-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        session
            .set_build_state(meerkat_core::SessionBuildState::default())
            .expect("session build state should serialize");
        store.save(&session).await.expect("persisted session");
        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("live-missing keep_alive resume should reject");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("session created with comms_name"),
            "expected live-missing keep_alive rejection: {err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "failed keep_alive resume must not leave the prepared runtime registered"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_resume_keep_alive_live_missing_failure_preserves_existing_runtime()
    {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(Arc::clone(&store)).await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-8".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: false,
                comms_name: Some("existing-mcp-agent".to_string()),
                peer_meta: None,
                realm_id: Some(state.realm_id.clone()),
                instance_id: state.instance_id.clone(),
                backend: Some(state.backend.clone()),
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata should serialize");
        session
            .set_build_state(meerkat_core::SessionBuildState::default())
            .expect("session build state should serialize");
        store.save(&session).await.expect("persisted session");
        state
            .runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                Box::new(RuntimeTerminationFixtureExecutor(None)),
            )
            .await
            .expect("MCP runtime executor should attach");
        assert!(
            state.runtime_adapter.contains_session(&session_id).await,
            "test requires a pre-existing runtime registration"
        );

        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                system_prompt: None,
                model: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                stream: false,
                verbose: false,
                tools: vec![],
                tool_results: vec![],
                enable_builtins: None,
                builtin_config: None,
                keep_alive: Some(true),
                comms_name: None,
                peer_meta: None,
                hooks_override: None,
                enable_memory: None,
                enable_schedule: None,
                enable_workgraph: None,
                enable_mob: None,
                enable_web_search: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
                turn_tool_overlay: None,
                additional_instructions: None,
            },
            None,
            None,
        ))
        .await;

        let err = result.expect_err("live-missing keep_alive resume should reject");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("session created with comms_name"),
            "expected live-missing keep_alive rejection: {err:?}"
        );
        assert!(
            state.runtime_adapter.contains_session(&session_id).await,
            "failed keep_alive resume should preserve existing runtime state"
        );
    }

    #[tokio::test]
    async fn test_handle_meerkat_blob_get_returns_payload() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let blob_ref = state
            .service
            .blob_store()
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");

        let result = Box::pin(handle_tools_call(
            &state,
            "meerkat_blob_get",
            &json!({ "blob_id": blob_ref.blob_id.as_str() }),
        ))
        .await
        .expect("blob get succeeds");

        let text = result["content"][0]["text"].as_str().expect("text payload");
        let payload: Value = serde_json::from_str(text).expect("valid json payload");
        assert_eq!(payload["blob_id"], blob_ref.blob_id.as_str());
        assert_eq!(payload["media_type"], "image/png");
        assert_eq!(payload["data"], "aGVsbG8=");
    }

    #[test]
    fn test_mcp_resume_requires_rebuild_for_tool_results_and_config_changes() {
        let mut input: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue"
        }))
        .unwrap();
        assert!(!mcp_resume_requires_rebuild(&input));

        input.tool_results = vec![ToolResultInput {
            tool_use_id: "tool-1".to_string(),
            content: "done".to_string(),
            is_error: false,
        }];
        assert!(mcp_resume_requires_rebuild(&input));
        input.tool_results.clear();

        input.enable_builtins = Some(false);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_builtins = None;

        input.builtin_config = Some(BuiltinConfigInput {
            enable_shell: Some(false),
            shell_timeout_secs: None,
        });
        assert!(mcp_resume_requires_rebuild(&input));
        input.builtin_config = None;

        input.enable_schedule = Some(true);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_schedule = None;

        input.enable_workgraph = Some(true);
        assert!(mcp_resume_requires_rebuild(&input));
        input.enable_workgraph = None;

        input.comms_name = Some("agent-a".to_string());
        assert!(mcp_resume_requires_rebuild(&input));
    }

    #[test]
    fn test_post_commit_session_created_error_includes_identity() {
        let session_id = meerkat::SessionId::new();
        let session_id_string = session_id.to_string();
        let err = post_commit_session_created_error(
            &session_id,
            &SessionError::Agent(meerkat::AgentError::InternalError("boom".to_string())),
        );
        let data = err.data.as_ref().expect("structured error data");
        assert_eq!(err.code, -32603);
        assert_eq!(
            data.get("session_created").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(data.get("resumable").and_then(Value::as_bool), Some(true));
        assert_eq!(
            data.get("session_id").and_then(Value::as_str),
            Some(session_id_string.as_str())
        );
    }

    #[test]
    fn test_format_agent_result_tool_preserves_cancelled_error_code() {
        let session_id = meerkat::SessionId::new();
        let err = format_agent_result_tool(
            Err(SessionError::Agent(meerkat::AgentError::Cancelled)),
            &session_id,
        )
        .expect_err("cancelled terminal should be a tool error");

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
    }

    #[test]
    fn test_post_commit_session_created_error_preserves_cancelled_error_code() {
        let session_id = meerkat::SessionId::new();
        let err = post_commit_session_created_error(
            &session_id,
            &SessionError::Agent(meerkat::AgentError::Cancelled),
        );

        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::RequestCancelled.jsonrpc_code()
        );
        let data = err.data.as_ref().expect("structured cancellation data");
        assert_eq!(
            data.get("session_created").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_tools_list_has_keep_alive_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify keep_alive parameter exists in the schema (nullable boolean)
        assert!(run_tool["inputSchema"]["properties"]["keep_alive"].is_object());

        // Verify comms_name parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["comms_name"].is_object());
        let comms_name_type = &run_tool["inputSchema"]["properties"]["comms_name"]["type"];
        assert!(
            comms_name_type == "string"
                || comms_name_type
                    .as_array()
                    .is_some_and(|types| types.contains(&json!("string"))),
            "unexpected comms_name type: {comms_name_type}"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool_still_errors() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_prefabz_typo",
            &json!({}),
        ))
        .await
        .expect_err("unknown tool must error");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_dispatches_workgraph_tools() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let created = Box::pin(handle_tools_call(
            &state,
            "workgraph_create",
            &json!({
                "title": "mcp visible work",
                "labels": ["mcp-workgraph"]
            }),
        ))
        .await
        .expect("workgraph create should dispatch");
        let text = created["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        let payload: Value = serde_json::from_str(text).expect("json payload");
        assert_eq!(payload["item"]["title"], "mcp visible work");

        let ready = Box::pin(handle_tools_call(
            &state,
            "workgraph_ready",
            &json!({ "labels": ["mcp-workgraph"] }),
        ))
        .await
        .expect("workgraph ready should dispatch");
        let text = ready["content"][0]["text"]
            .as_str()
            .expect("tool payload text");
        let payload: Value = serde_json::from_str(text).expect("json payload");
        assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn test_handle_tools_call_rejects_attention_scoped_workgraph_tool_without_projection() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let err = Box::pin(handle_tools_call(
            &state,
            "workgraph_attention_reassign",
            &json!({
                "binding_id": "attn_1",
                "expected_revision": 1,
                "target": {
                    "kind": "owner",
                    "owner_key": { "kind": "agent", "id": "agent:mob/demo/agent/member" }
                }
            }),
        ))
        .await
        .expect_err("attention-only WorkGraph tool must not dispatch from public MCP");
        assert_eq!(err.code, -32601);
        assert!(
            err.message
                .contains("unknown WorkGraph tool 'workgraph_attention_reassign'")
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_rejects_raw_mob_dispatcher_tool_names() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(&state, "mob_create", &json!({})))
            .await
            .expect_err("raw internal mob tool name must not be exposed");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Unknown tool"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_exposes_typed_mob_create_and_rejects_internal_fields() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let created = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "mob-1",
                    "profiles": {
                        "worker": { "model": "claude-sonnet-4-6" }
                    }
                }
            }),
        ))
        .await
        .expect("typed mob create should succeed");
        let created = unwrap_tool_payload_json(created);
        assert_eq!(created["mob_id"], "mob-1");

        let listed = Box::pin(handle_tools_call(&state, "meerkat_mob_list", &json!({})))
            .await
            .expect("typed mob list should succeed");
        let listed = unwrap_tool_payload_json(listed);
        assert_eq!(listed["mobs"][0]["mob_id"], "mob-1");

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_create",
            &json!({
                "definition": {
                    "id": "mob-2",
                    "profiles": {
                        "worker": {
                            "model": "claude-sonnet-4-6",
                            "tools": {
                                "rust_bundles": ["internal-only"]
                            }
                        }
                    }
                }
            }),
        ))
        .await
        .expect_err("nested internal tool bundle fields must be rejected");
        assert_eq!(err.code, -32602);
        assert!(
            !err.message.is_empty(),
            "validation errors should return a non-empty message"
        );
    }

    #[tokio::test]
    async fn test_mcp_handlers_sessions_read_list_interrupt_archive() {
        let (state, session_id) = state_with_persisted_session().await;

        let listed = Box::pin(handle_tools_call(&state, "meerkat_sessions", &json!({})))
            .await
            .expect("sessions list call should succeed");
        let listed_payload = unwrap_payload(listed);
        let sessions = listed_payload["sessions"]
            .as_array()
            .expect("sessions should be an array");
        assert!(
            sessions
                .iter()
                .any(|entry| entry["session_id"] == json!(session_id)),
            "persisted session should appear in list"
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_read",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("read call should succeed");
        let read_payload = unwrap_payload(read);
        assert_eq!(
            read_payload["session_id"],
            read_payload["state"]["session_id"]
        );

        let interrupted = Box::pin(handle_tools_call(
            &state,
            "meerkat_interrupt",
            &json!({ "session_id": read_payload["session_id"] }),
        ))
        .await
        .expect("interrupt should no-op for non-running persisted session");
        let interrupted_payload = unwrap_payload(interrupted);
        assert_eq!(interrupted_payload["interrupted"], true);

        // Ask 21: archive is a lifecycle terminal, not a projection
        // promotion. A created-but-never-run session (durable record, no
        // runtime snapshot) is the complete session truth and MUST archive
        // — the old rejection permanently stranded never-prompted mob
        // members in `retiring`.
        let archive_result = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": read_payload["session_id"] }),
        ))
        .await;
        assert!(
            archive_result.is_ok(),
            "archiving a never-run persisted session must succeed: {archive_result:?}"
        );
    }

    // test_mcp_archive_surfaces_incomplete_mob_cleanup_data deleted:
    // its retry premise (retain mob anchor, retry after partial cleanup)
    // is obsolete under runtime authority — the first archive retires
    // the session regardless of mob cleanup outcome.

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mcp_archive_does_not_mask_mob_member_archive_failure_with_child_cleanup() {
        let (mut state, _session_id) = state_with_persisted_session().await;
        let (mob_state, archive_failures) =
            meerkat_mob_mcp::MobMcpState::new_in_memory_with_archive_failure_control();
        state.mob_state = mob_state.clone();
        let (_parent_mob_id, member_session_id) = insert_mcp_archive_live_member(&mob_state).await;
        archive_failures
            .fail_archive(
                member_session_id.clone(),
                "forced MCP mob archive failure after retire event",
            )
            .await;
        let member_session_key = member_session_id.to_string();
        let (child_mob_id, child_events) =
            insert_mcp_archive_partial_destroy_mob(&state, &member_session_key).await;
        child_events.allow_clear();

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": member_session_key }),
        ))
        .await
        .expect_err("failed parent mob member archive must fail meerkat_archive");

        assert_eq!(err.code, -32603);
        assert!(
            err.message
                .contains("forced MCP mob archive failure after retire event"),
            "MCP archive should surface the parent bridge-session archive failure: {err:?}"
        );
        assert!(
            state
                .mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check failed parent bridge session"),
            "failed ArchiveSession must retain the parent bridge session retry anchor"
        );
        assert!(
            state.mob_state.handle_for(&child_mob_id).await.is_ok(),
            "child cleanup must not be run as a success fallback while parent archive failed"
        );

        archive_failures
            .clear_archive_failure(&member_session_id)
            .await;
        let retry_success = Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": member_session_key }),
        ))
        .await
        .expect("retry should complete after parent archive failure clears");
        let retry_payload = unwrap_payload(retry_success);
        assert_eq!(
            retry_payload["archived"], true,
            "MCP archive retry should report success after parent archive and child cleanup complete"
        );
        assert!(
            !state
                .mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check retried parent bridge session"),
            "successful retry must archive the parent bridge session"
        );
        assert!(
            state.mob_state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child cleanup retry anchor"
        );
    }

    #[test]
    fn test_mcp_interrupt_not_ready_noop_is_only_idle_or_attached() {
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Idle
        ));
        assert!(interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Attached
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Destroyed
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Retired
        ));
        assert!(!interrupt_not_ready_is_noop(
            meerkat_runtime::RuntimeState::Stopped
        ));
    }

    #[tokio::test]
    async fn test_mcp_interrupt_ignores_cold_persisted_stopped_projection_when_session_exists() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_and_runtime_store(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
        )
        .await;
        let created = state
            .service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: "seed".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: Some(32),
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    custom_models: std::collections::BTreeMap::new(),
                    image_generation_provider: None,
                    auto_compact_threshold_override: None,
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(TestClient::default()),
                    )),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("runtime-backed MCP service should create deferred session");
        state
            .runtime_adapter
            .register_session(created.session_id.clone())
            .await
            .expect("register session");
        state
            .runtime_adapter
            .stop_runtime_executor(&created.session_id, "seed stopped projection")
            .await
            .expect("runtime state should persist");
        assert_eq!(
            state
                .service
                .persisted_runtime_state(&created.session_id)
                .await
                .expect("runtime-state projection load should succeed"),
            Some(meerkat_runtime::RuntimeState::Stopped)
        );

        // Reconstruct the surface against the same stores to model a process
        // restart. Graceful unregister would finalize this projection to Idle
        // and would not exercise the cold-Stopped compatibility path.
        drop(state);
        let state = MeerkatMcpState::new_with_store_and_runtime_store(store, runtime_store).await;
        assert!(
            !state
                .runtime_adapter
                .contains_session(&created.session_id)
                .await,
            "cold MCP runtime must not inherit process-local registration"
        );
        assert_eq!(
            state
                .service
                .persisted_runtime_state(&created.session_id)
                .await
                .expect("cold runtime-state projection load should succeed"),
            Some(meerkat_runtime::RuntimeState::Stopped)
        );

        let payload = Box::pin(handle_tools_call(
            &state,
            "meerkat_interrupt",
            &json!({ "session_id": created.session_id }),
        ))
        .await
        .expect("persisted stopped projection must not reject cold interrupt no-op");

        let payload = unwrap_payload(payload);
        assert_eq!(payload["interrupted"], true);
    }

    #[tokio::test]
    async fn test_mcp_history_returns_messages_for_live_and_archived_sessions() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
        )
        .await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Hello".to_string()),
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Hi there".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Follow up".to_string()),
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Second answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        store
            .save(&session)
            .await
            .expect("persisted history session");
        seed_runtime_authority_session(&runtime_store, &session).await;

        let history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id, "offset": 1, "limit": 2 }),
        ))
        .await
        .expect("history should succeed");
        let history_payload = unwrap_payload(history);
        assert!(
            history_payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "history should expose the full transcript: {history_payload}"
        );
        assert_eq!(history_payload["offset"], 1);
        assert_eq!(history_payload["limit"], 2);
        assert_eq!(history_payload["has_more"], true);
        assert_eq!(history_payload["messages"].as_array().unwrap().len(), 2);

        Box::pin(handle_tools_call(
            &state,
            "meerkat_archive",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("archive through runtime authority should succeed");

        let archived_history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("archived history should succeed");
        let archived_payload = unwrap_payload(archived_history);
        assert!(
            archived_payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "archived history should preserve the transcript: {archived_payload}"
        );
        assert!(
            archived_payload["messages"].as_array().unwrap().len() >= 4,
            "archived history should return the full transcript"
        );

        let resume_err = Box::pin(handle_tools_call(
            &state,
            "meerkat_resume",
            &json!({
                "session_id": session_id,
                "prompt": "should not resume archived session"
            }),
        ))
        .await
        .expect_err("archived resume should be rejected");
        assert!(
            resume_err.message.contains("Session not found"),
            "archived resume should surface not found: {resume_err:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(session.id()).await,
            "archived MCP resume must not register runtime state"
        );
    }

    #[tokio::test]
    async fn test_mcp_history_serializes_mixed_message_kinds() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
        )
        .await;
        let mut session = meerkat::Session::new();
        let session_id = session.id().to_string();

        session.push(meerkat_core::types::Message::System(
            meerkat_core::types::SystemMessage::new("system rules"),
        ));
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("hello".to_string()),
        ));
        session.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage::new(
                vec![meerkat_core::types::AssistantBlock::ToolUse {
                    id: "tool-2".to_string(),
                    name: "lookup".into(),
                    args: serde_json::value::RawValue::from_string(
                        serde_json::json!({ "item": "transcript" }).to_string(),
                    )
                    .expect("raw tool args"),
                    meta: None,
                }],
                meerkat_core::types::StopReason::ToolUse,
            ),
        ));
        session.push(meerkat_core::types::Message::tool_results(vec![
            meerkat_core::types::ToolResult::new("tool-2".to_string(), "done".to_string(), false),
        ]));
        store.save(&session).await.expect("persisted mixed session");
        seed_runtime_authority_session(&runtime_store, &session).await;

        let history = Box::pin(handle_tools_call(
            &state,
            "meerkat_history",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("mixed history should succeed");
        let payload = unwrap_payload(history);
        let messages = payload["messages"]
            .as_array()
            .expect("history messages should be an array");

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "block_assistant");
        assert_eq!(messages[2]["blocks"][0]["block_type"], "tool_use");
        assert_eq!(
            messages[2]["blocks"][0]["data"]["args"]["item"],
            "transcript"
        );
        assert_eq!(messages[3]["role"], "tool_results");
        assert_eq!(messages[3]["results"][0]["tool_use_id"], "tool-2");
    }

    #[tokio::test]
    async fn test_mcp_handlers_add_remove_reload_require_registered_adapter() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect_err("mcp add should fail without adapter registration");
        assert_eq!(
            err.code,
            meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code()
        );
        assert!(err.message.contains("meerkat_resume"));
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("session_resume_required"))
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_legacy_server_name_mirror_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_name": "demo",
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect_err("legacy server_name mirror must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("unknown field"));
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_malformed_server_config_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {}
            }),
        ))
        .await
        .expect_err("malformed server_config must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(
            err.message.contains("missing field")
                || err.message.contains("data did not match any variant")
        );
    }

    #[tokio::test]
    async fn test_mcp_add_rejects_legacy_config_string_before_dispatch() {
        let (state, session_id) = state_with_persisted_session().await;

        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": "demo"
            }),
        ))
        .await
        .expect_err("legacy config string must be rejected at input boundary");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("invalid type"));
    }

    #[tokio::test]
    async fn test_mcp_handlers_add_remove_reload_after_adapter_registration() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let bindings = state
            .runtime_adapter
            .prepare_bindings(parsed.clone())
            .await
            .expect("session runtime bindings");
        seed_active_external_surface(bindings.external_tool_surface().as_ref(), "demo");
        let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface())),
        ));
        let session = state
            .service
            .load_authoritative_session(&parsed)
            .await
            .expect("load MCP handler fixture")
            .expect("persisted MCP handler fixture");
        create_runtime_backed_session_and_run_initial_turn(
            &state.service,
            &state.runtime_adapter,
            &parsed,
            mock_deferred_materialization_request(session, bindings, None),
            None,
            &meerkat::LiveSessionActorWitnessSlot::default(),
        )
        .await
        .expect("MCP handler fixture should materialize its live actor");
        attach_test_mcp_router(&state, &parsed, router).await;

        let add = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": session_id,
                "server_config": {"name": "demo", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect("mcp add should succeed");
        let add_payload = unwrap_payload(add);
        assert_eq!(add_payload["operation"], "add");
        assert_eq!(add_payload["status"], "staged");

        let remove = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_remove",
            &json!({
                "session_id": add_payload["session_id"],
                "server_name": "demo"
            }),
        ))
        .await
        .expect("mcp remove should succeed");
        let remove_payload = unwrap_payload(remove);
        assert_eq!(remove_payload["operation"], "remove");
        assert_eq!(remove_payload["status"], "staged");

        let reload = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_reload",
            &json!({
                "session_id": remove_payload["session_id"],
                "server_name": "demo"
            }),
        ))
        .await
        .expect("mcp reload should succeed");
        let reload_payload = unwrap_payload(reload);
        assert_eq!(reload_payload["operation"], "reload");
        assert_eq!(reload_payload["status"], "staged");

        let witness = state
            .runtime_adapter
            .current_executor_attachment_witness(&parsed)
            .await
            .expect("runtime attachment should have an exact witness");
        state
            .runtime_adapter
            .unregister_executor_attachment_if_current(&witness)
            .await
            .expect("runtime attachment should unregister exactly");
        assert!(
            state
                .runtime_ingress_context()
                .logical_router(&parsed)
                .await
                .is_some(),
            "logical router should survive an attachment replacement window"
        );
        let error = Box::pin(handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &json!({
                "session_id": parsed.to_string(),
                "server_config": {"name": "stale", "command": "cat", "args": [], "env": {}}
            }),
        ))
        .await
        .expect_err("a logical router without an exact live attachment must be rejected");
        assert!(error.message.contains("Live MCP unavailable"));
    }

    #[tokio::test]
    async fn test_event_stream_open_missing_session_errors() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let missing_id = meerkat::SessionId::new().to_string();
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_open",
            &json!({ "session_id": missing_id }),
        ))
        .await
        .expect_err("open should fail for missing session");
        assert!(err.message.contains("Failed to open session event stream"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_event_stream_open_preserves_scope_denial_before_registration() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut state = MeerkatMcpState::new_with_store(store).await;
        let owner = Arc::clone(&state.mob_state);
        let (mob_id, _session_id) = insert_mcp_archive_live_member(&owner).await;

        let viewer = meerkat_mob_mcp::MobMcpState::new_in_memory_as(
            meerkat_mob::MobControlPrincipal::External(
                meerkat_core::auth::PrincipalId::new("mcp-stream-viewer").expect("principal id"),
            ),
        );
        viewer
            .mob_insert_handle(
                mob_id.clone(),
                owner.handle_for(&mob_id).await.expect("owner handle"),
            )
            .await;
        state.mob_state = viewer;

        let error = handle_tools_call(
            &state,
            "meerkat_mob_event_stream_open",
            &json!({ "mob_id": mob_id.as_str() }),
        )
        .await
        .expect_err("ungranted stream open must deny");

        assert_eq!(
            error.code,
            meerkat_contracts::ErrorCode::ScopeDenied.jsonrpc_code()
        );
        assert_eq!(
            error.data,
            Some(json!({
                "required": "subscribe_events",
                "presented": []
            }))
        );
        assert!(
            state.mob_event_streams.lock().await.is_empty(),
            "denied open must not register stream authority"
        );
    }

    #[tokio::test]
    async fn test_event_stream_read_default_timeout_and_close_behavior() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = McpStreamId::mint();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("read should complete with timeout");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "timeout");

        let closed = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("close should succeed");
        let close_payload = unwrap_payload(closed);
        assert_eq!(close_payload["closed"], true);
    }

    #[tokio::test]
    async fn test_event_stream_read_no_timeout_opt_in_blocks() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = McpStreamId::mint();
        let pending_stream: meerkat_core::EventStream = Box::pin(stream::pending());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(pending_stream),
            }),
        );

        let result = Box::pin(timeout(
            Duration::from_millis(50),
            handle_tools_call(
                &state,
                "meerkat_event_stream_read",
                &json!({ "stream_id": stream_id.to_string(), "no_timeout": true }),
            ),
        ))
        .await;
        assert!(result.is_err(), "no_timeout should allow blocking reads");
    }

    #[tokio::test]
    async fn test_event_stream_read_empty_stream_reports_closed_and_removes_entry() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let stream_id = McpStreamId::mint();
        let empty_stream: meerkat_core::EventStream = Box::pin(stream::empty());
        state.session_event_streams.lock().await.insert(
            stream_id,
            Arc::new(SessionEventStreamHandle {
                stream: Mutex::new(empty_stream),
            }),
        );

        let read = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("read should succeed");
        let read_payload = unwrap_payload(read);
        assert_eq!(read_payload["status"], "closed");

        let close = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_close",
            &json!({ "stream_id": stream_id.to_string() }),
        ))
        .await
        .expect("close should succeed");
        let close_payload = unwrap_payload(close);
        assert_eq!(close_payload["closed"], false);
    }

    #[tokio::test]
    async fn test_event_stream_read_rejects_malformed_stream_id() {
        // Stream identity is a typed fact parsed fail-closed at ingress
        // (`McpStreamId::parse`): a malformed wire stream_id is rejected
        // outright, never treated as a probe into the string-keyed registry.
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = Box::pin(handle_tools_call(
            &state,
            "meerkat_event_stream_read",
            &json!({ "stream_id": "not-a-stream-id" }),
        ))
        .await
        .expect_err("malformed stream_id must fail closed");
        assert!(
            err.message.contains("invalid stream_id"),
            "expected fail-closed parse error, got: {}",
            err.message
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_invalid_handling_mode_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_message",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "body": "hi",
            "handling_mode": "invalid"
        }))
        .expect_err("invalid handling_mode must fail deserialization");
        assert!(
            err.to_string().contains("handling_mode") || err.to_string().contains("invalid"),
            "expected serde error mentioning handling_mode, got: {err}"
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_unknown_intent_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_request",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "intent": "not.generated",
            "params": {}
        }))
        .expect_err("unknown comms intent must fail deserialization");
        assert!(
            err.to_string().contains("not.generated") || err.to_string().contains("variant"),
            "expected serde error mentioning the unknown intent, got: {err}"
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_input_malformed_result_rejected_at_serde_boundary() {
        let err = serde_json::from_value::<MeerkatCommsSendInput>(json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "kind": "peer_response",
            "to": "550e8400-e29b-41d4-a716-446655440000",
            "in_reply_to": "550e8400-e29b-41d4-a716-446655440001",
            "status": "completed",
            "result": {
                "result": "ack",
                "ok": "yes"
            }
        }))
        .expect_err("malformed typed comms result must fail deserialization");
        let message = err.to_string();
        assert!(
            message.contains("ok")
                || message.contains("invalid type")
                || message.contains("did not match any variant"),
            "expected serde error mentioning malformed result, got: {message}"
        );
    }
}
