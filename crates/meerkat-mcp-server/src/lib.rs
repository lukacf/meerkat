//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate exposes Meerkat's session, config, catalog, skill, history,
//! blob, live-MCP, and event-stream operations as MCP tools. Feature-owned
//! schedule, WorkGraph, and mob tool families can also contribute to the
//! published registry.

#![allow(dead_code, unused_imports, clippy::expect_used, clippy::large_futures)]

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod resumed_run_tests;
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
use meerkat_contracts::wire::WireConfigWriteRefusal;
use meerkat_contracts::{RequestLifecycle, SkillsParams};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions, SessionError, SessionService, SessionServiceControlExt,
    SessionServiceHistoryExt, StageToolResultsDisposition, StageToolResultsRequest,
    StartTurnRequest,
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
    if let Some(in_progress) = err.teardown_in_progress_session_error() {
        return in_progress;
    }
    if let Some(refused) = err.reasoning_effort_refused_session_error() {
        return refused;
    }
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
        // Nothing was admitted: the rollback keeps every input the
        // registration recovered from the store, except the inputs bound to a
        // run the registration marked resumable whose stop was asked, which
        // end as stopped.
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

/// Release an uncommitted machine claim without erasing durable session work.
///
/// Exact rollback removes only the provisional machine registration and the
/// actor incarnation published through this transaction's witness slot. The
/// durable session document remains discoverable, and the explicit resume
/// seam can reconstruct a fresh actor/executor incarnation. The rollback
/// keeps every input the registration recovered from the store; once the
/// registration admitted live work (an admission, a deduplicated
/// resubmission, a recycle), the machine stops every input instead, so a
/// turn its caller saw refused never runs later. The inputs bound to a run
/// the registration marked resumable whose stop was asked end as stopped
/// either way, so that run never resumes. A direct initial turn leaves no
/// input to keep or stop.
async fn release_mcp_actor_materialization(
    prepared: &mut Option<McpPreparedActorMaterialization>,
    session_id: &meerkat::SessionId,
    context: &str,
) -> Result<(), ToolCallError> {
    let Some(mut prepared) = prepared.take() else {
        return Ok(());
    };
    let result = prepared.rollback_now().await;
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
    /// Host-regenerated text projected only into provider requests for this
    /// logical turn.
    #[serde(default)]
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
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
    /// The session's provider-neutral reasoning effort level: `none`,
    /// `minimal`, `low`, `medium`, `high`, `xhigh` or `max`. Meerkat checks it
    /// against the model on the route that serves the request and writes the
    /// provider's own field; a level the model or route does not accept is
    /// refused before any provider request. The same setting as
    /// `provider_params.reasoning_effort`; an input that gives both must give
    /// the same level.
    #[serde(default)]
    pub reasoning_effort: Option<meerkat_core::model_profile::capabilities::EffortLevel>,
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

/// The provider params a `meerkat_run` session's build seats: the input's
/// params with its `reasoning_effort` level folded in by Core's one rule. An
/// input that names the level twice with two different levels is refused.
fn run_build_provider_params(
    input: &MeerkatRunInput,
) -> Result<Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>, ToolCallError> {
    meerkat_core::resolve_build_provider_params(
        input.provider_params.as_ref(),
        input
            .reasoning_effort
            .map(meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Set),
    )
    .map_err(|error| ToolCallError::invalid_params(error.to_string()))
}

/// The provider params a `meerkat_resume` rebuild names. A resume that sets
/// or clears `reasoning_effort` also claims the session's level
/// (`ResumeOverrideMask::reasoning_effort`), so the rebuilt session holds
/// that level, or none; a resume that names none keeps the recorded level,
/// also when it replaces the other params. A clear beside params that name
/// a level, and two different levels, are refused.
fn resume_build_provider_params(
    input: &MeerkatResumeInput,
) -> Result<Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>, ToolCallError> {
    use meerkat_contracts::wire::runtime::WireTurnMetadataOverride;
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    let level = input.reasoning_effort.clone().map(|level| match level {
        WireTurnMetadataOverride::Set(level) => TurnMetadataOverride::Set(level),
        WireTurnMetadataOverride::Clear => TurnMetadataOverride::Clear,
    });
    meerkat_core::resolve_build_provider_params(input.provider_params.as_ref(), level)
        .map_err(|error| ToolCallError::invalid_params(error.to_string()))
}

/// Whether a `meerkat_resume` names the provider-neutral reasoning effort
/// level: the `reasoning_effort` member (a level or a clear), or the key of
/// that name inside `provider_params`. A provider's own effort under
/// `provider_tag` is not the level.
fn resume_names_reasoning_effort_level(input: &MeerkatResumeInput) -> bool {
    input.reasoning_effort.is_some()
        || input
            .provider_params
            .as_ref()
            .is_some_and(|params| params.reasoning_effort.is_some())
}

/// Structured-data `kind` of the busy answer to a request that would change
/// a session's reasoning effort level while the session has unfinished
/// work: the value the JSON-RPC surface answers such a turn with.
const REASONING_EFFORT_CHANGE_NEEDS_IDLE_SESSION_KIND: &str =
    "reasoning_effort_change_needs_idle_session";

/// The answer to a `meerkat_resume` that names the reasoning effort level
/// while a turn is running on its session: the session-busy code with the
/// kind as its data. Nothing was retired or stored; the caller waits for
/// the running turn's answer and sends the resume again.
fn reasoning_effort_change_needs_idle_session_tool_error() -> ToolCallError {
    ToolCallError::new(
        meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code(),
        "session is busy: a resume that names the reasoning effort level rebuilds the session \
         and is not taken while a turn is running on it; send it again when the running turn \
         has answered",
        Some(json!({ "kind": REASONING_EFFORT_CHANGE_NEEDS_IDLE_SESSION_KIND })),
    )
}

fn mcp_resume_requires_rebuild(input: &MeerkatResumeInput) -> bool {
    !input.tool_results.is_empty()
        || !input.tools.is_empty()
        || input.model.is_some()
        || input.provider.is_some()
        || input.max_tokens.is_some()
        || input.output_schema.is_some()
        || input.structured_output_retries.is_some()
        || input.provider_params.is_some()
        || input.reasoning_effort.is_some()
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
    /// Default command timeout, in seconds, of the shell tool in the session
    /// this call builds: its `[shell] timeout_secs`. A shell call's own
    /// `timeout_secs` still wins. Omit, `null` or `0` keep the realm's
    /// configured value on a run, and the session's recorded value on a
    /// resume.
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
}

/// The session shell timeout a request asks for; `0` asks for none.
fn requested_shell_timeout(config: Option<&BuiltinConfigInput>) -> Option<std::num::NonZeroU64> {
    config
        .and_then(|config| config.shell_timeout_secs)
        .and_then(std::num::NonZeroU64::new)
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

/// A refused session-owner wake (or a refused standalone attach of a mob
/// member's session), in its typed wire form.
fn session_owner_wake_tool_error(error: SessionError) -> ToolCallError {
    let wire = meerkat_contracts::WireError::from(error);
    ToolCallError::new(wire.code.jsonrpc_code(), wire.message, wire.details)
}

/// A mob member's session that no mob in this process owns: refused, typed.
fn mob_member_session_tool_error(
    session_id: &meerkat::SessionId,
    binding: &meerkat_core::MobMemberBinding,
) -> ToolCallError {
    session_owner_wake_tool_error(SessionError::Unsupported(format!(
        "session {session_id} belongs to member '{}' of mob '{}': deliver to the member through its mob, which wakes it",
        binding.member, binding.mob_id
    )))
}

/// A failed session preparation as a tool error. A session another runtime
/// owner hosts keeps its typed wire form (session busy, with
/// `kind = "session_served_elsewhere"`), as does one whose hosting
/// claim is unavailable (runtime unavailable, with
/// `kind = "session_hosting_unavailable"`), and one whose durable tail is
/// withheld from resume (`SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` and the
/// other durable holds, with the typed `durable_resume_hold` payload; the
/// content is retained); anything else is internal.
fn prepare_session_tool_error(error: SessionError) -> ToolCallError {
    // A hosting refusal, and a session whose durable tail is withheld from
    // resume (its content retained), keep their typed wire form.
    if matches!(
        error,
        SessionError::ServedElsewhere { .. } | SessionError::HostingUnavailable { .. }
    ) || error.durable_resume_hold().is_some()
    {
        let wire = meerkat_contracts::WireError::from(error);
        return ToolCallError::new(wire.code.jsonrpc_code(), wire.message, wire.details);
    }
    ToolCallError::internal(format!("Failed to prepare session: {error}"))
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
        // An ordinary write is not administration of a section the realm's
        // trusted host administers; nothing was written.
        ConfigRuntimeError::Config(refused) if refused.administered_section().is_some() => {
            let refusal = match refused.administered_section() {
                Some(meerkat_core::AdministeredSection::Confinement) => {
                    WireConfigWriteRefusal::ConfinementAdministrationRequired
                }
                _ => WireConfigWriteRefusal::GovernanceAdministrationRequired,
            };
            match serde_json::to_value(refusal) {
                Ok(data) => ToolCallError::new(-32602, refused.to_string(), Some(data)),
                Err(serialize_error) => ToolCallError::new(
                    -32603,
                    format!("failed to serialize config refusal data: {serialize_error}"),
                    None,
                ),
            }
        }
        // The store refused the caller's candidate config (a new or changed
        // realm MCP server with an environment reference, or an invalid
        // written config); document and store failures stay internal.
        ConfigRuntimeError::Config(
            error @ (meerkat_core::ConfigError::RealmMcpServerEnvReference(_)
            | meerkat_core::ConfigError::Validation(_)),
        ) => ToolCallError::invalid_params(format!("Invalid config: {error}")),
        // A held or unusable admission fence: the write did not start and
        // wrote nothing; typed, never an internal error.
        refused @ ConfigRuntimeError::AdmissionInProgress => ToolCallError::new(
            meerkat_contracts::ErrorCode::AdmissionInProgress.jsonrpc_code(),
            refused.to_string(),
            Some(json!({ "type": "admission_in_progress" })),
        ),
        refused @ ConfigRuntimeError::AdmissionFenceUnavailable(_) => ToolCallError::new(
            meerkat_contracts::ErrorCode::AdmissionFenceUnavailable.jsonrpc_code(),
            refused.to_string(),
            Some(json!({ "type": "admission_fence_unavailable" })),
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
/// Mirrors REST: maps the reserved `global` realm to the injected
/// user-global doc and every other realm to its per-realm config. Returned
/// concrete so config-store construction can route head docs through the
/// SAME mapping (`config_doc_path`); surfaces inject this rather than
/// re-deriving the projection.
fn mcp_realm_config_source(
    realms_root: &std::path::Path,
    global_doc: PathBuf,
) -> Arc<meerkat_store::FilesystemRealmConfigSource> {
    Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        realms_root.to_path_buf(),
        global_doc,
        meerkat_models::canonical(),
    ))
}

/// `router` with the host's MCP attempt guards, when given, for the
/// session it serves and that session's mob member, if it is one.
pub(crate) fn guarded_router(
    router: McpRouter,
    guards: Option<&meerkat_mcp::McpAttemptGuards>,
    session_id: &meerkat::SessionId,
    member: Option<meerkat_core::MobMemberBinding>,
) -> McpRouter {
    match guards {
        Some(guards) => router.with_attempt_guards(
            guards.clone(),
            meerkat_mcp::McpConstructionOwner::new(session_id.clone(), member),
        ),
        None => router,
    }
}

/// The one local MCP launch boundary of this server's routers, built once at
/// construction from the host's resolved confinement: its `mcp_stdio`
/// requirement, or the trusted host.
fn host_mcp_stdio_launch_profile(
    confinement: &meerkat::HostConfinement,
) -> meerkat_mcp::McpStdioLaunchProfile {
    let kind = meerkat_core::ConfinementLaunchKind::McpStdio;
    match (
        confinement.requirement(kind),
        confinement.working_directory(kind),
    ) {
        (Some(requirement), Some(working_directory)) => {
            meerkat_mcp::McpStdioLaunchProfile::required(
                requirement.clone(),
                working_directory.to_path_buf(),
            )
        }
        _ => meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    }
}

/// Load the effective config for `realm_id`, failing closed on every
/// document in the realm chain.
///
/// The head realm document is authoritative, so a head that fails to read,
/// parse, or pass its ingress checks (for example an unwired `[agent]
/// provider_params` table) and an effective config that fails `validate` both
/// surface as errors and stop startup, exactly as rkat-rest and the CLI do.
/// Substituting `Config::default()` would boot the server on a configuration
/// the operator never wrote. The parent-chain compose step fails closed for
/// the same reason: an ancestor or the user-global `~/.rkat/config.toml`
/// tail that fails to load carries the fleet's credential binding, model
/// defaults, and limits, and every `create_session` re-composes the same
/// chain, so booting on the head document alone would produce a live server
/// that can create no session.
async fn load_config_async(
    realm_id: &meerkat_core::connection::RealmId,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
    source: &Arc<meerkat_store::FilesystemRealmConfigSource>,
) -> Result<Config, std::io::Error> {
    let store = realm_config_store(
        realm_id,
        realms_root,
        backend_hint,
        origin_hint,
        instance_id,
        source,
    )
    .await
    .map_err(|err| {
        std::io::Error::other(format!(
            "failed to open realm config store for realm '{realm_id}': {err}"
        ))
    })?;
    let head_config = store.get().await.map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to read head realm config for realm '{realm_id}': {err}"),
        )
    })?;
    // Fold the head realm's parent chain (head ⊕ ancestors ⊕ `global` tail) into
    // the effective config so top-level fields inherit before env overrides win.
    // A compose failure (malformed parent edge, or an ancestor / global document
    // that fails its ingress checks) stops startup with the typed error, as on
    // rkat-rpc and rkat-rest.
    let dyn_source: Arc<dyn meerkat_core::RealmConfigSource> = Arc::clone(source) as _;
    let mut config = meerkat_core::EffectiveConfigReader::new(dyn_source)
        .effective_config_over_head(realm_id, head_config)
        .await
        .map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to compose effective config for realm '{realm_id}': {err}"),
            )
        })?;
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    config
        .validate(meerkat_models::canonical())
        .map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid effective config for realm '{realm_id}': {err}"),
            )
        })?;
    Ok(config)
}

/// Resolve an explicit keep_alive override. Returns None when input is None (inherit).
fn resolve_keep_alive(requested: Option<bool>) -> Result<Option<bool>, String> {
    match requested {
        Some(value) => validate_effective_keep_alive(value).map(|()| Some(value)),
        other => Ok(other), // None (inherit) or Some(false) (disable) pass through
    }
}

fn validate_effective_keep_alive(keep_alive: bool) -> Result<(), String> {
    let support = if cfg!(feature = "comms") {
        meerkat::surface::KeepAliveSupport::Available
    } else {
        meerkat::surface::KeepAliveSupport::Unavailable
    };
    meerkat::surface::resolve_keep_alive_for_surface(keep_alive, support).map(|_| ())
}

fn validate_public_peer_meta(peer_meta: Option<&meerkat_core::PeerMeta>) -> Result<(), String> {
    meerkat::surface::validate_public_peer_meta(peer_meta)
}

fn tagged_realm_config_store(
    realms_root: &std::path::Path,
    realm_id: &meerkat_core::connection::RealmId,
    backend: meerkat_store::RealmBackend,
    instance_id: Option<&str>,
    source: &Arc<meerkat_store::FilesystemRealmConfigSource>,
) -> Arc<dyn ConfigStore> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id.as_str());
    // Head doc routed through the shared source mapping: the reserved
    // `global` realm reads/writes the user-global document, so an explicit
    // state root cannot shadow global config with
    // `<state_root>/global/config.toml`. It composes with the same ancestors
    // as at start, so a config write cannot cut the realm from their
    // `[confinement]`.
    let config_doc = source.config_doc_path(realm_id);
    let documents: Arc<dyn meerkat_core::RealmConfigSource> = source.clone();
    let base: Arc<dyn ConfigStore> = Arc::new(
        FileConfigStore::new(config_doc.clone(), meerkat_models::canonical())
            .with_realm_chain(realm_id.clone(), documents),
    );
    let tagged = meerkat_core::TaggedConfigStore::new(
        base,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(realm_id.to_string()),
            instance_id: instance_id.map(ToOwned::to_owned),
            backend: Some(backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: config_doc.display().to_string(),
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
    source: &Arc<meerkat_store::FilesystemRealmConfigSource>,
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
        source,
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
    /// The convention roots this server's agent factory layers hooks from
    /// (its context root, else its project root; and the user root), kept so
    /// the config tool can compare hooks as a session of the realm runs
    /// them.
    hook_context_root: PathBuf,
    hook_user_root: Option<PathBuf>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    /// The host's local MCP launch boundary, owned here once for the life of
    /// the state: every router this server builds (fresh creation, a resume
    /// whose router is missing, schedule seeding) retains it. This server's
    /// agent factory builds no MCP router of its own.
    mcp_stdio_launch_profile: meerkat_mcp::McpStdioLaunchProfile,
    /// The host's MCP attempt guards, given at construction
    /// ([`Self::new_with_bootstrap_options_confinement_and_attempt_guards`]):
    /// every router this server builds (fresh creation, a resume whose
    /// router is missing, schedule seeding) installs them, for the session
    /// it serves.
    mcp_attempt_guards: Option<meerkat_mcp::McpAttemptGuards>,
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
        Self::new_with_bootstrap_options_and_confinement(bootstrap, expose_paths, None).await
    }

    /// [`Self::new_with_bootstrap_and_options`] with the operator's
    /// `--confinement*` selection, which replaces the realm's composed
    /// `[confinement]` for every launch kind of this process (`None`: the
    /// configuration decides).
    pub async fn new_with_bootstrap_options_and_confinement(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        confinement: Option<meerkat_core::LaunchConfinement>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(bootstrap, expose_paths, None, confinement).await
    }

    /// [`Self::new_with_bootstrap_options_and_confinement`] with the host's
    /// MCP attempt guards: every router this server builds (for fresh,
    /// resumed and scheduled sessions alike) installs them, for the session
    /// it serves, so each stdio attempt begins the host's guard first. This
    /// server's agent factory builds no MCP router of its own.
    pub async fn new_with_bootstrap_options_confinement_and_attempt_guards(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        confinement: Option<meerkat_core::LaunchConfinement>,
        attempt_guards: meerkat_mcp::McpAttemptGuards,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_llm_and_guards(
            bootstrap,
            expose_paths,
            None,
            confinement,
            Some(attempt_guards),
        )
        .await
    }

    #[doc(hidden)]
    pub async fn new_with_bootstrap_and_test_client(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_and_llm(
            bootstrap,
            expose_paths,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
    }

    async fn new_with_bootstrap_options_and_llm(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
        confinement: Option<meerkat_core::LaunchConfinement>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_options_llm_and_guards(
            bootstrap,
            expose_paths,
            default_llm_client,
            confinement,
            None,
        )
        .await
    }

    async fn new_with_bootstrap_options_llm_and_guards(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
        confinement: Option<meerkat_core::LaunchConfinement>,
        mcp_attempt_guards: Option<meerkat_mcp::McpAttemptGuards>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Realm-id-first dual-root resolution through the storage layout
        // (the single path authority; the layout's realm-root candidates arm
        // the cross-candidate first-start reservation at open). The
        // project-local candidate participates only with an explicit
        // --context-root (see rkat-rpc).
        let invocation_context = bootstrap
            .context
            .context_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let meerkat_core::ResolvedStorage {
            layout,
            locator,
            root_choice,
        } = meerkat_core::StorageLayout::resolve(
            meerkat_core::StorageLayoutInputs {
                invocation_context,
                explicit_state_root: bootstrap.realm.state_root.clone(),
                user_config_root: bootstrap.context.user_config_root.clone(),
                default_root: Some(meerkat_core::RealmRootDefault::UserGlobal),
                probe_local_candidate: bootstrap.context.context_root.is_some(),
            },
            &bootstrap.realm,
        )?;
        tracing::info!(
            realm = %locator.realm,
            state_root = %locator.state_root.display(),
            root_choice = ?root_choice,
            "resolved realm storage"
        );
        // Locator carries the typed `RealmId` directly; no need to reparse
        // at the mcp-server boundary. Downstream `meerkat_store::*_in` call
        // sites obtain `&str` via `RealmId::as_str`.
        let realm_id = locator.realm;
        let realms_root = locator.state_root;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint);
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        // Open through the layout BEFORE loading config: first
        // materialization must run under the cross-candidate first-start
        // reservation, not the config loader's plain manifest ensure.
        let (manifest, persistence) =
            meerkat::storage_provider::open_realm_persistence_with_layout(
                layout.clone(),
                realm_id.as_str(),
                backend_hint,
                origin_hint,
            )
            .await?;
        // The reserved `global` realm maps to the layout's user-global doc
        // (honoring --user-config-root); when no home-like root resolves, a
        // path under the realms root that will never exist, so `global`
        // yields None.
        let global_doc = layout
            .global_config_path()
            .unwrap_or_else(|| realms_root.join("__no_global__").join("config.toml"));
        let fs_realm_config_source = mcp_realm_config_source(&realms_root, global_doc);
        let mut config = load_config_async(
            &realm_id,
            &realms_root,
            backend_hint,
            origin_hint,
            bootstrap.realm.instance_id.as_deref(),
            &fs_realm_config_source,
        )
        .await?;
        // The operator's flags replace the documents' `[confinement]`.
        config.apply_cli_overrides(meerkat_core::config::CliOverrides {
            confinement,
            ..meerkat_core::config::CliOverrides::default()
        });
        // #1813: a realm that declares multi-process hosting refuses to start
        // when its stores cannot provide it.
        persistence.require_hosting_mode(config.storage.hosting_mode())?;
        let store_path = persistence
            .store_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| realm_store_path(&realms_root, &realm_id, manifest.backend));
        let session_store = persistence.session_store();
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
        // The roots the factory below layers hooks from.
        let hook_context_root = project_root.clone();
        let hook_user_root = bootstrap.context.user_config_root.clone();
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            manifest.backend,
            bootstrap.realm.instance_id.as_deref(),
            &fs_realm_config_source,
        );
        let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> = fs_realm_config_source;
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

        // The host's OS process confinement over --context-root: the
        // workspace preset never grants the realm's own directory. This
        // server's routers keep the one MCP stdio profile it lowers to.
        let host_confinement = meerkat::HostConfinement::resolve(
            &config.confinement,
            conventions_context_root.as_deref(),
        )?;
        // A start whose confined processes could change the realm state, the
        // user config document or the credential store is refused before it
        // serves.
        let mut host_inputs = meerkat::HostConfinement::host_inputs(
            &realms_root,
            layout.global_config_path().as_deref(),
        );
        host_inputs.extend(
            meerkat::TokenStoreBackend::default_auto()
                .ok()
                .as_ref()
                .and_then(meerkat::HostConfinement::credential_store_input),
        );
        host_confinement.admit_host_inputs(&host_inputs)?;
        let convention_sources = meerkat::HostConfinement::convention_sources(
            conventions_context_root.as_deref(),
            bootstrap.context.user_config_root.as_deref(),
        );
        host_confinement.log_startup(&host_inputs, &convention_sources);
        // The factory reads its hooks and skills from the context root, or
        // from its project root when there is none: those are the convention
        // sources its file tools must not write.
        let tool_convention_sources = meerkat::HostConfinement::convention_sources(
            Some(&project_root),
            bootstrap.context.user_config_root.as_deref(),
        );
        let mcp_stdio_launch_profile = host_mcp_stdio_launch_profile(&host_confinement);

        // Create factory with max-permissive flags; per-request overrides
        // in SessionBuildOptions control what tools are actually enabled.
        // It builds no MCP router of its own (its sessions configure no
        // servers and its runtime carries no configured attachment
        // authority), so the host's attempt guards live with the routers
        // this server builds below. The agents' in-process file tools never
        // write the host's inputs, the realm's own documents or the
        // convention sources, in any confinement mode; without a context
        // root the tools are rooted in the realm directory, where the realm
        // documents are kept too.
        let mut factory = AgentFactory::new(store_path)
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .project_root(project_root)
            .builtins(true)
            .shell(true)
            .workgraph(true)
            .schedule(true)
            .with_host_confinement(&host_confinement)
            .with_protected_host_inputs(
                &host_inputs,
                &meerkat::HostConfinement::realm_documents(&realm_paths.root),
                &tool_convention_sources,
            );
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await?;

        let max_sessions = config.max_sessions();
        #[cfg(feature = "mob")]
        let mob_host_config = config.mob_host.clone();
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
        meerkat::surface::set_default_workgraph_namespace_grant(
            &builder,
            Some(workgraph_service.namespace_grant().clone()),
        );
        #[cfg(feature = "mob")]
        let (runtime_delivery_inbox, continuation_bindings) = (
            persistence.runtime_delivery_inbox(),
            persistence.continuation_bindings(),
        );
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
                builder,
                max_sessions,
                persistence,
                realm_paths.root.join("config_state.json"),
            )?;
        #[cfg(feature = "mob")]
        let mob_state = {
            let persistent_mob_root = realm_paths.root.clone();
            let acceptor_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
            let controlling_acceptor =
                meerkat_mob::ControllingAcceptorConfig::from_mob_host_config(
                    &mob_host_config,
                    acceptor_service,
                )
                .map_err(|detail| std::io::Error::new(std::io::ErrorKind::InvalidInput, detail))?;
            let mut state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                service.clone(),
                Some(runtime_adapter.clone()),
                // A16: the local MCP console is the owning operator
                // (phase 5 explicit mint, DEC-P5E-8).
                meerkat_mob::MobControlPrincipal::Owner,
            )?
            .with_persistent_storage_root(Some(persistent_mob_root))
            .with_workgraph_service(Some(workgraph_service.clone()));
            if let Some(acceptor) = controlling_acceptor {
                state = state.with_controlling_acceptor(acceptor);
            }
            let state = state.into_shared();
            // fork_off and council outcomes are submitted durably to the
            // service's delivery owner, which resolves members and confirms
            // jobs through this state.
            if let Err(error) =
                state.bind_continuations(runtime_delivery_inbox, &continuation_bindings)
            {
                tracing::warn!(%error, "fork_off and council run in the turn: continuation owner not bound");
            }
            state.start_workgraph_flow_reconciler();
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
            hook_context_root,
            hook_user_root,
            skill_runtime,
            mcp_stdio_launch_profile,
            mcp_attempt_guards,
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
        Self::new_with_store_options_llm_and_guards(
            store,
            runtime_store,
            max_sessions_override,
            default_llm_client,
            None,
        )
        .await
    }

    /// [`Self::new_with_store`] with the host's MCP attempt guards (tests
    /// only).
    #[cfg(test)]
    pub(crate) async fn new_with_store_and_attempt_guards(
        store: Arc<dyn SessionStore>,
        attempt_guards: meerkat_mcp::McpAttemptGuards,
    ) -> Self {
        Self::new_with_store_options_llm_and_guards(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            None,
            Some(attempt_guards),
        )
        .await
    }

    #[cfg(test)]
    async fn new_with_store_options_llm_and_guards(
        store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
        max_sessions_override: Option<usize>,
        default_llm_client: Option<Arc<dyn meerkat::LlmClient>>,
        mcp_attempt_guards: Option<meerkat_mcp::McpAttemptGuards>,
    ) -> Self {
        let mut bootstrap = RuntimeBootstrap::default();
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
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm_id.as_str());
        let user_root = realm_paths.root.join("user");
        bootstrap.context.user_config_root = Some(user_root.clone());
        let fs_realm_config_source =
            mcp_realm_config_source(&realms_root, user_root.join(".rkat/config.toml"));
        // Test-only constructor over a freshly generated isolated realm: a
        // config-load failure is an environment defect the test must see.
        #[allow(clippy::expect_used)]
        let mut config = load_config_async(
            &realm_id,
            &realms_root,
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Generated),
            bootstrap.realm.instance_id.as_deref(),
            &fs_realm_config_source,
        )
        .await
        .expect("test-only mcp state must load its isolated realm config");
        if let Some(max_sessions) = max_sessions_override {
            config.limits.max_sessions = Some(max_sessions);
        }
        let store_path =
            realm_store_path(&realms_root, &realm_id, meerkat_store::RealmBackend::Sqlite);
        let project_root = realm_paths.root.clone();
        // The roots the factory below layers hooks from.
        let hook_context_root = project_root.clone();
        let hook_user_root = bootstrap.context.user_config_root.clone();
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            meerkat_store::RealmBackend::Sqlite,
            bootstrap.realm.instance_id.as_deref(),
            &fs_realm_config_source,
        );
        let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> = fs_realm_config_source;
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
        meerkat::surface::set_default_workgraph_namespace_grant(
            &builder,
            Some(workgraph_service.namespace_grant().clone()),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(
            meerkat_store::FsBlobStore::new(realm_paths.root.join("blobs")),
        );
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
                builder,
                max_sessions,
                PersistenceBundle::new(store, runtime_store, blob_store)
                    .expect("construct runtime authority"),
                realm_paths.root.join("config_state.json"),
            )
            .expect("compose the runtime-backed service");

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
            hook_context_root,
            hook_user_root,
            skill_runtime: None,
            mcp_stdio_launch_profile: meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
            mcp_attempt_guards,
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
        // Presence only, read body-free: a running session's provisional
        // tail must not refuse a live MCP change.
        if runtime_ingress::committed_source_archived(&self.service, session_id)
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
    /// Host-regenerated text projected only into provider requests for this
    /// newly admitted logical turn.
    #[serde(default)]
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    /// Ordinary System message appended at this admitted turn boundary.
    ///
    /// This is not a recovered build-configuration override.
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
    /// Set, clear, or keep the session's provider-neutral reasoning effort
    /// level from this turn on: a level (`"high"`) sets it,
    /// `{"action": "clear"}` removes it, and omitting the field keeps the
    /// level the session recorded. It is its own setting: `provider_params`
    /// that name no level keep the recorded one.
    #[serde(default)]
    pub reasoning_effort: Option<
        meerkat_contracts::wire::runtime::WireTurnMetadataOverride<
            meerkat_core::model_profile::capabilities::EffortLevel,
        >,
    >,
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
    /// Agent-lifetime ceiling, measured from agent construction.
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    /// Aggregate ceiling for a single turn, re-armed at every run entry.
    #[serde(default)]
    pub max_turn_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
}

impl From<BudgetLimitsInput> for meerkat_core::BudgetLimits {
    fn from(value: BudgetLimitsInput) -> Self {
        meerkat_core::BudgetLimits {
            max_tokens: value.max_tokens,
            max_duration: value.max_duration_secs.map(std::time::Duration::from_secs),
            max_turn_duration: value
                .max_turn_duration_secs
                .map(std::time::Duration::from_secs),
            max_tool_calls: value.max_tool_calls,
        }
    }
}

/// Per-turn tool overlay of `meerkat_resume`: the public overlay the other
/// surfaces take ([`meerkat_core::service::PublicTurnToolOverlay`]), with the
/// MCP inputs' tolerance of keys they do not know. An unknown key is ignored
/// and logged, never forwarded, so a tool call cannot supply host-only
/// dispatch context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct TurnToolOverlayInput {
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
    /// Tool choice for this turn's provider calls, in order: entry `k`
    /// applies to the turn's `k`-th call, and every call after the plan is
    /// exhausted is `auto`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_choice_plan: Vec<meerkat_core::lifecycle::run_primitive::ToolChoice>,
}

impl<'de> Deserialize<'de> for TurnToolOverlayInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Take `key` out of `object`; absent and `null` are the default.
        fn take<T, E>(object: &mut serde_json::Map<String, Value>, key: &str) -> Result<T, E>
        where
            T: serde::de::DeserializeOwned + Default,
            E: serde::de::Error,
        {
            match object.remove(key) {
                None | Some(Value::Null) => Ok(T::default()),
                Some(value) => serde_json::from_value(value)
                    .map_err(|error| E::custom(format!("turn_tool_overlay.{key}: {error}"))),
            }
        }

        let mut object = serde_json::Map::<String, Value>::deserialize(deserializer)?;
        let overlay = Self {
            allowed_tools: take(&mut object, "allowed_tools")?,
            blocked_tools: take(&mut object, "blocked_tools")?,
            tool_choice_plan: take(&mut object, "tool_choice_plan")?,
        };
        if !object.is_empty() {
            tracing::warn!(
                ignored_keys = ?object.keys().collect::<Vec<_>>(),
                "meerkat_resume ignored unknown turn_tool_overlay keys"
            );
        }
        Ok(overlay)
    }
}

impl From<TurnToolOverlayInput> for meerkat_core::service::TurnToolOverlay {
    fn from(value: TurnToolOverlayInput) -> Self {
        meerkat_core::service::PublicTurnToolOverlay {
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
            tool_choice_plan: value.tool_choice_plan,
        }
        .into()
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
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending {
            tool_use_id,
            tool_name,
            args,
            dedupe_key,
        })) => {
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
                tool_use_id,
                tool_name,
                args,
                dedupe_key.map(|key| *key),
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
        Err(SessionError::Agent(meerkat::AgentError::CallbackBatchPending {
            pending_tool_calls,
        })) => {
            let pending = meerkat_contracts::WireCallbackPending::many(
                session_id.clone(),
                None,
                false,
                true,
                pending_tool_calls
                    .into_iter()
                    .map(|call| meerkat_contracts::WirePendingToolCall {
                        tool_use_id: call.tool_use_id,
                        tool_name: call.tool_name,
                        args: call.args,
                        settlement_failures: call.settlement_failures,
                        dedupe_key: call.dedupe_key,
                    })
                    .collect(),
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

/// A refused `reasoning_effort` level as a tool error, in the form JSON-RPC
/// answers it: the session transport's code (invalid params, -32602), the
/// refusal's message, and the typed data `{"kind":
/// "reasoning_effort_refused", "retryable": <bool>,
/// "reasoning_effort_refusal": {...}}`. A host reads the refusal from the
/// data and never from the text. `None` for any other error.
fn reasoning_effort_refused_tool_error(error: &SessionError) -> Option<ToolCallError> {
    let SessionError::Agent(agent_error) = error.primary_error() else {
        return None;
    };
    if !matches!(
        agent_error.primary_error(),
        meerkat::AgentError::ReasoningEffortRefused(_)
    ) {
        return None;
    }
    Some(ToolCallError::new(
        meerkat_contracts::error::session_transport_error_code(error).jsonrpc_code(),
        error.to_string(),
        meerkat_contracts::error::session_error_details(error),
    ))
}

fn format_agent_result_tool(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, ToolCallError> {
    match result {
        Err(SessionError::Agent(meerkat::AgentError::Cancelled)) => {
            Err(request_cancelled_tool_error())
        }
        Err(error) => match reasoning_effort_refused_tool_error(&error) {
            Some(refused) => Err(refused),
            None => format_agent_result(Err(error), session_id).map_err(ToolCallError::internal),
        },
        result => format_agent_result(result, session_id).map_err(ToolCallError::internal),
    }
}

/// The typed tool error of a refused help-session rebuild.
fn help_session_not_resumable_tool_error(
    refusal: &meerkat::help::HelpSessionNotResumable,
) -> ToolCallError {
    ToolCallError::new(
        refusal.error_code().jsonrpc_code(),
        refusal.to_string(),
        Some(refusal.data()),
    )
}

fn build_callback_dispatcher(tools: &[McpToolDef]) -> Option<Arc<dyn AgentToolDispatcher>> {
    if tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(tools)) as Arc<dyn AgentToolDispatcher>)
    }
}

fn recoverable_callback_tool_defs(tools: &[McpToolDef]) -> Vec<ToolDef> {
    tools
        .iter()
        .filter(|tool| tool.handler_kind() == "callback")
        .map(|tool| ToolDef {
            audience: Default::default(),
            name: tool.name.clone().into(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            provenance: Some(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Builtin,
                source_id: "mcp-server".into(),
                artifact: None,
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

    // A refused reasoning effort level keeps its typed form; the data also
    // names the session this call created, as its other failures do.
    if let Some(mut refused) = reasoning_effort_refused_tool_error(err) {
        if let Some(Value::Object(data)) = refused.data.as_mut() {
            data.insert("session_id".to_string(), json!(session_id.to_string()));
            data.insert("session_ref".to_string(), json!(session_id.to_string()));
            data.insert("session_created".to_string(), json!(true));
            data.insert("resumable".to_string(), json!(true));
        }
        return refused;
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
/// `tools/list` JSON (see `base_tools_list`) and the lifecycle lookup (see
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
/// `contributed_tool_lifecycles`. Unknown names resolve to `None` - the
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
            refuse_launching_entries(state, &config).await?;
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
            let preview = apply_patch_preview(&state.config_runtime, &patch).await?;
            refuse_launching_entries(state, &preview).await?;
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

/// The kinds of configuration entry that launch a process on the host, or
/// decide what one runs. This tool does not add one, does not change one and
/// does not bring one into force through the realm's parent chain: like
/// `[confinement]`, they are the host's to administer in the document itself
/// or through its own writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchingConfigEntry {
    /// An MCP server with a stdio transport: its command is started when a
    /// session's MCP servers are applied.
    McpStdioServer,
    /// A hook with a command runtime: its command runs at the hook's point.
    CommandHook,
    /// A skills repository with a stdio transport: its command is started
    /// when the skill source is read.
    StdioSkillsRepository,
    /// A skills repository with a git transport: `git` fetches its URL, with
    /// the SSH command its key path gives.
    GitSkillsRepository,
    /// `shell.program`: the program the shell tool runs its commands with.
    ShellProgram,
    /// `shell.security_mode` and `shell.security_patterns`: the policy the
    /// shell tool checks a command against before it runs it. Only a looser
    /// one is refused.
    ShellCommandPolicy,
}

impl LaunchingConfigEntry {
    /// Every kind.
    #[cfg(test)]
    const ALL: [Self; 6] = [
        Self::McpStdioServer,
        Self::CommandHook,
        Self::StdioSkillsRepository,
        Self::GitSkillsRepository,
        Self::ShellProgram,
        Self::ShellCommandPolicy,
    ];

    /// The kind as a refusal names it.
    const fn describe(self) -> &'static str {
        match self {
            Self::McpStdioServer => "a `tools.mcp_servers` entry with a stdio transport",
            Self::CommandHook => "a `hooks.entries` entry with a command runtime",
            Self::StdioSkillsRepository => "a `skills.repositories` entry with a stdio transport",
            Self::GitSkillsRepository => "a `skills.repositories` entry with a git transport",
            Self::ShellProgram => "a `shell.program`",
            Self::ShellCommandPolicy => {
                "a looser shell command policy (`shell.security_mode`, `shell.security_patterns`)"
            }
        }
    }
}

/// The first launching entry of `candidate` that `committed` does not hold
/// as the same whole entry (the config store's rule for realm servers: a
/// kept name with another command, argument, environment value, working
/// directory, URL, ref or key path is a new entry). An entry that launches
/// no process (an HTTP server, an HTTP or in-process hook, a filesystem or
/// HTTP skills repository) is not looked at, and neither is a removal.
///
/// The refusal runs it twice: on the realm's own document before and after
/// the write, and on what the realm composes along its parent chain before
/// and after it. The shell section is compared only as composed, in
/// [`changed_composed_shell`]: a realm takes it whole, so its own document
/// does not say which program it runs.
///
/// Every section of the document, every field of the three sections that
/// hold a launching entry, and every transport and hook runtime is named
/// here with no rest pattern and no wildcard arm: a new section, field,
/// transport or runtime does not compile until it is placed as launching or
/// not.
fn added_or_changed_launching_entry(
    committed: &Config,
    candidate: &Config,
) -> Result<Option<LaunchingConfigEntry>, serde_json::Error> {
    use meerkat_core::config::{HookAdapterConfig, HooksConfig, ToolsConfig};
    use meerkat_core::mcp_config::McpTransportConfig;
    use meerkat_core::skills_config::{SkillRepoTransport, SkillsConfig};

    let Config {
        tools,
        hooks,
        skills,
        // Compared as the realm composes it, in `changed_composed_shell`.
        shell: _,
        // The config store refuses a write to these on every surface.
        governance: _,
        confinement: _,
        // Credential commands, the one launching entry of this section, are
        // refused by the config store on every surface.
        realm: _,
        // No field of these names a program, an argument, an environment or
        // a working directory of a launch.
        agent: _,
        storage: _,
        budget: _,
        retry: _,
        models: _,
        max_tokens: _,
        store: _,
        comms: _,
        mob_host: _,
        compaction: _,
        limits: _,
        rest: _,
        self_hosted: _,
        provider_tools: _,
        model_fallback: _,
        presentation: _,
    } = candidate;
    let ToolsConfig {
        mcp_servers,
        default_timeout: _,
        tool_timeouts: _,
        max_concurrent: _,
        builtins_enabled: _,
        shell_enabled: _,
        comms_enabled: _,
        mob_enabled: _,
        schedule_enabled: _,
        workgraph_enabled: _,
    } = tools;
    let HooksConfig {
        entries,
        default_timeout_ms: _,
        payload_max_bytes: _,
        background_max_concurrency: _,
    } = hooks;
    let SkillsConfig {
        repositories,
        enabled: _,
        max_injection_bytes: _,
        inventory_threshold: _,
        health_thresholds: _,
        identity: _,
    } = skills;

    for server in mcp_servers {
        let launches = match &server.transport {
            McpTransportConfig::Stdio(_) => true,
            McpTransportConfig::Http(_) => false,
        };
        if launches && !committed.tools.mcp_servers.contains(server) {
            return Ok(Some(LaunchingConfigEntry::McpStdioServer));
        }
    }
    for entry in entries {
        let launches = match &entry.runtime {
            HookAdapterConfig::Command(_) => true,
            HookAdapterConfig::InProcess(_) | HookAdapterConfig::Http(_) => false,
        };
        if launches && !committed.hooks.entries.contains(entry) {
            return Ok(Some(LaunchingConfigEntry::CommandHook));
        }
    }
    // A skills repository has no equality of its own: whole entries are
    // compared as they serialize.
    let committed_repositories = committed
        .skills
        .repositories
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    for repository in repositories {
        let kind = match &repository.transport {
            SkillRepoTransport::Stdio { .. } => Some(LaunchingConfigEntry::StdioSkillsRepository),
            SkillRepoTransport::Git { .. } => Some(LaunchingConfigEntry::GitSkillsRepository),
            SkillRepoTransport::Filesystem { .. } | SkillRepoTransport::Http { .. } => None,
        };
        if let Some(kind) = kind
            && !committed_repositories.contains(&serde_json::to_value(repository)?)
        {
            return Ok(Some(kind));
        }
    }
    Ok(None)
}

/// What a write changes in the shell section a realm composes, when the
/// change decides what the shell tool runs: another program, or a looser
/// command policy. A realm takes the section whole (its parent's, or its
/// own once that differs from the default one), so the realm's own document
/// cannot answer this: a write of another shell key alone would put the
/// default program and the default policy (no restriction) in the place of
/// a parent's.
/// `committed` and `candidate` are the sections the realm composes along its
/// parent chain before and after the write. Every field is named with no
/// rest pattern.
fn changed_composed_shell(
    committed: &meerkat_core::config::ShellDefaults,
    candidate: &meerkat_core::config::ShellDefaults,
) -> Option<LaunchingConfigEntry> {
    let meerkat_core::config::ShellDefaults {
        program,
        timeout_secs: _,
        security_mode,
        security_patterns,
        max_output_chars: _,
    } = candidate;
    if *program != committed.program {
        return Some(LaunchingConfigEntry::ShellProgram);
    }
    loosens_shell_command_policy(committed, *security_mode, security_patterns)
        .then_some(LaunchingConfigEntry::ShellCommandPolicy)
}

/// Whether a candidate command policy of the shell tool is looser than the
/// committed one, as far as the two compare without matching a command.
/// The committed policy decides, and its mode is matched with no wildcard
/// arm:
///
/// - no restriction: nothing can be loosened;
/// - an allow list (only a command that matches a pattern runs): looser
///   unless the candidate is an allow list that holds no pattern the
///   committed list does not hold;
/// - a deny list (a command that matches a pattern does not run): with no
///   pattern it restricts nothing; otherwise looser unless the candidate is
///   a deny list that holds every committed pattern.
///
/// Patterns are compared as exact strings, as sets: a pattern rewritten
/// into an equivalent one counts as another pattern.
fn loosens_shell_command_policy(
    committed: &meerkat_core::config::ShellDefaults,
    mode: meerkat_core::SecurityMode,
    patterns: &[String],
) -> bool {
    use meerkat_core::SecurityMode;
    match committed.security_mode {
        SecurityMode::Unrestricted => false,
        SecurityMode::AllowList => {
            mode != SecurityMode::AllowList
                || patterns
                    .iter()
                    .any(|pattern| !committed.security_patterns.contains(pattern))
        }
        SecurityMode::DenyList => {
            !committed.security_patterns.is_empty()
                && (mode != SecurityMode::DenyList
                    || committed
                        .security_patterns
                        .iter()
                        .any(|pattern| !patterns.contains(pattern)))
        }
    }
}

/// The kinds of entry that decide where a credential the host supplies is
/// read from, and where it is sent: a realm's provider routes and the
/// servers they name. This tool does not add one, does not change one and
/// does not bring one into force through the realm's parent chain: like a
/// launching entry, they are the host's to administer in the document
/// itself or through its own writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialRouteEntry {
    /// `realm.<id>.auth.<name>`: where a credential is read from (an
    /// environment variable of any name, a stored secret, a resolver).
    AuthProfile,
    /// `realm.<id>.backend.<name>`: the address requests, and the
    /// credential with them, are sent to.
    Backend,
    /// `realm.<id>.binding.<name>`: which auth profile's credential goes to
    /// which backend, and for which default model.
    Binding,
    /// `realm.<id>.default_binding`: the binding a run that names none
    /// takes.
    DefaultBinding,
    /// `realm.<id>.copilot`: where the account's routes read their token.
    CopilotAccount,
    /// `realm.<id>.connector_client.<name>`: the issuer a connector login
    /// and its refreshes go to.
    ConnectorClient,
    /// `self_hosted.servers.<id>`: the address of a server a backend names.
    SelfHostedServer,
    /// `self_hosted.models.<alias>`: the server a model's requests go to. A
    /// model added, or pointed at another server, is refused.
    SelfHostedModel,
    /// The server a binding names, through its backend or through its
    /// default model's entry in `self_hosted.models`: a write that keeps the
    /// binding and leaves it naming another server, or none, is refused (the
    /// removal of that model's entry among them).
    BindingServer,
}

impl CredentialRouteEntry {
    /// The kind as a refusal names it.
    const fn describe(self) -> &'static str {
        match self {
            Self::AuthProfile => "an auth profile (`realm.<id>.auth`)",
            Self::Backend => "a backend (`realm.<id>.backend`)",
            Self::Binding => "a binding (`realm.<id>.binding`)",
            Self::DefaultBinding => "a default binding (`realm.<id>.default_binding`)",
            Self::CopilotAccount => "a Copilot account (`realm.<id>.copilot`)",
            Self::ConnectorClient => "a connector client (`realm.<id>.connector_client`)",
            Self::SelfHostedServer => "a self-hosted server (`self_hosted.servers`)",
            Self::SelfHostedModel => {
                "a self-hosted model or the server it names (`self_hosted.models`)"
            }
            Self::BindingServer => {
                "the server a binding names (through its backend, or through the entry of \
                 its default model in `self_hosted.models`)"
            }
        }
    }
}

/// The first provider route entry of `candidate` that `committed` does not
/// hold as the same whole entry, in any realm section either states: an
/// auth profile, a backend, a binding, a set default binding, a Copilot
/// account, a connector client; or a self-hosted server; or a self-hosted
/// model that is new or names another server. A removal is not looked at,
/// with one exception: a binding the write keeps must name the server it
/// named ([`servers_the_bindings_name`]), so the removal of the model a
/// binding names its server through is refused.
/// Whole entries are compared (of a model, the server it names), so a
/// backend is refused whether or not a binding uses it yet.
///
/// The refusal runs it twice, as for launching entries: on the realm's own
/// document and on what the realm composes along its parent chain.
///
/// Every field of a realm section and of the self-hosted section is named
/// with no rest pattern: a new field does not compile until it is placed.
fn added_or_changed_credential_route(
    committed: &Config,
    candidate: &Config,
) -> Option<CredentialRouteEntry> {
    use meerkat_core::config::SelfHostedConfig;
    use meerkat_core::connection::RealmConfigSection;

    for (realm, section) in &candidate.realm {
        let RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding,
            // An edge brings another realm's routes into the chain, or
            // takes them out: the composed comparison sees the entries.
            parent: _,
            connector_client,
            copilot,
        } = section;
        let held = committed.realm.get(realm);
        if auth
            .iter()
            .any(|(name, entry)| held.and_then(|held| held.auth.get(name)) != Some(entry))
        {
            return Some(CredentialRouteEntry::AuthProfile);
        }
        if backend
            .iter()
            .any(|(name, entry)| held.and_then(|held| held.backend.get(name)) != Some(entry))
        {
            return Some(CredentialRouteEntry::Backend);
        }
        if binding
            .iter()
            .any(|(name, entry)| held.and_then(|held| held.binding.get(name)) != Some(entry))
        {
            return Some(CredentialRouteEntry::Binding);
        }
        if default_binding.is_some()
            && held.and_then(|held| held.default_binding.as_ref()) != default_binding.as_ref()
        {
            return Some(CredentialRouteEntry::DefaultBinding);
        }
        if copilot.is_some() && held.and_then(|held| held.copilot.as_ref()) != copilot.as_ref() {
            return Some(CredentialRouteEntry::CopilotAccount);
        }
        if connector_client.iter().any(|(name, entry)| {
            held.and_then(|held| held.connector_client.get(name)) != Some(entry)
        }) {
            return Some(CredentialRouteEntry::ConnectorClient);
        }
    }
    let SelfHostedConfig {
        servers,
        models,
        // Chooses among the models that exist; the binding is then the one
        // for that model's server, so nothing crosses through it.
        default_model: _,
    } = &candidate.self_hosted;
    if servers
        .iter()
        .any(|(id, server)| committed.self_hosted.servers.get(id) != Some(server))
    {
        return Some(CredentialRouteEntry::SelfHostedServer);
    }
    // A request goes to the server its model names, with the credential of
    // the binding for that server, and a binding can name its server
    // through its default model: so a model added, or pointed at another
    // server, moves a credential to another declared server. A model's
    // other fields name no server.
    if models.iter().any(|(alias, model)| {
        committed
            .self_hosted
            .models
            .get(alias)
            .map(|held| &held.server)
            != Some(&model.server)
    }) {
        return Some(CredentialRouteEntry::SelfHostedModel);
    }
    // A binding the write keeps must name the server it named. The removal
    // of the model a binding names its server through leaves it naming
    // none, and a binding that names none can stand for other declared
    // servers.
    let named = servers_the_bindings_name(candidate);
    servers_the_bindings_name(committed)
        .into_iter()
        .any(|(binding, server)| named.get(&binding).is_some_and(|kept| *kept != server))
        .then_some(CredentialRouteEntry::BindingServer)
}

/// The server each binding of `config` names, by realm and binding, as Core
/// answers it ([`meerkat_core::self_hosted_binding_server`]: declared on its
/// backend, derived from its default model's entry in `self_hosted.models`,
/// or none). `None` for the bindings of a realm section whose routes do not
/// load: it then compares unequal to any server a binding named.
fn servers_the_bindings_name(
    config: &Config,
) -> std::collections::BTreeMap<(String, String), Option<meerkat_core::SelfHostedBindingServer>> {
    let mut named = std::collections::BTreeMap::new();
    for (realm, section) in &config.realm {
        let connections = meerkat_core::RealmConnectionSet::from_config(realm, section).ok();
        for name in section.binding.keys() {
            let server = connections.as_ref().and_then(|connections| {
                connections.bindings.get(name).map(|binding| {
                    meerkat_core::self_hosted_binding_server(config, connections, binding)
                })
            });
            named.insert((realm.clone(), name.clone()), server);
        }
    }
    named
}

/// Refuse a write that adds an entry that launches a process on the host,
/// or changes what one runs, or that changes where a credential the host
/// supplies is read from or sent (a realm's provider routes:
/// [`added_or_changed_credential_route`]), with the refusal of a write that
/// would administer `[confinement]`. Nothing is written. Two comparisons,
/// either refuses:
///
/// - the realm's own document holds, after the write, a launching entry or
///   a route entry it does not hold now as the same whole entry (so a copy
///   of a parent's entry is not written into the realm either);
/// - the realm composes, after the write, a launching entry, a shell
///   program, a looser shell command policy or a route entry that it does
///   not compose now: a realm composes its parent chain's entries and realm
///   sections and takes its shell section whole, so a change of its own
///   parent edge, the removal of an own entry that shadowed a parent's, or
///   a write of another shell key alone can bring one into force.
///
/// The compositions are the ones a session of the realm is built from,
/// with the hooks as a session layers them (the first entry of an id, then
/// the convention files of the server's context root and user root): a
/// command hook that an earlier entry of its id hides is not in force, and
/// a write that takes the hiding entry away is refused. A write is refused
/// as well when the realm's chain does not compose before it, or would not
/// after it: what the realm would run cannot then be compared.
async fn refuse_launching_entries(
    state: &MeerkatMcpState,
    candidate: &Config,
) -> Result<(), ToolCallError> {
    let committed = state
        .config_runtime
        .get()
        .await
        .map_err(map_config_runtime_error)?
        .config;
    let serialize_error = |error: serde_json::Error| {
        ToolCallError::new(
            -32603,
            format!("failed to compare the config's launching entries: {error}"),
            None,
        )
    };
    const LAUNCHES: &str = "an entry that launches a process on the host, or decides what one \
                            runs, is the host's to administer";
    const ROUTES: &str = "where a credential the host supplies is read from, and where it is \
                          sent, is the host's to administer";
    let own_launching =
        added_or_changed_launching_entry(&committed, candidate).map_err(serialize_error)?;
    let (refused, reason) = if let Some(entry) = own_launching {
        (
            format!("this tool does not add or change {}", entry.describe()),
            LAUNCHES,
        )
    } else if let Some(entry) = added_or_changed_credential_route(&committed, candidate) {
        (
            format!("this tool does not add or change {}", entry.describe()),
            ROUTES,
        )
    } else {
        let reader =
            meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source));
        let mut composed_committed = reader
            .effective_config_over_head(&state.realm_id, committed)
            .await
            .map_err(|error| {
                ToolCallError::internal(format!(
                    "failed to compose the realm's config chain: {error}"
                ))
            })?;
        let mut composed_candidate = reader
            .effective_config_over_head(&state.realm_id, candidate.clone())
            .await
            .map_err(|error| {
                ToolCallError::invalid_params(format!(
                    "Invalid config: the document does not compose along the realm's parent \
                     chain: {error}; nothing was written"
                ))
            })?;
        // A session runs the hooks as the agent factory layers them, not
        // the composed list as it stands: the first entry of an id (a
        // parent's before the realm's own), then the context root's
        // convention file, then the user root's. A command hook that an
        // earlier entry of its id hides is not in force, so the hooks are
        // compared as layered.
        for composed in [&mut composed_committed, &mut composed_candidate] {
            composed.hooks = meerkat::resolve_layered_hooks_config(
                Some(state.hook_context_root.as_path()),
                state.hook_user_root.as_deref(),
                composed,
            )
            .await;
        }
        let launching =
            match added_or_changed_launching_entry(&composed_committed, &composed_candidate)
                .map_err(serialize_error)?
            {
                Some(entry) => Some(entry),
                None => {
                    changed_composed_shell(&composed_committed.shell, &composed_candidate.shell)
                }
            };
        if let Some(entry) = launching {
            (
                format!(
                    "after this write the realm would compose {} that it does not compose now \
                     (a realm composes its parent chain's entries, and takes its `[shell]` \
                     section whole: its own once that differs from the default one), and this \
                     tool does not bring one into force",
                    entry.describe()
                ),
                LAUNCHES,
            )
        } else if let Some(entry) =
            added_or_changed_credential_route(&composed_committed, &composed_candidate)
        {
            (
                format!(
                    "after this write the realm would compose {} that it does not compose now \
                     (a realm composes the realm sections of its parent chain), and this tool \
                     does not bring one into force",
                    entry.describe()
                ),
                ROUTES,
            )
        } else {
            return Ok(());
        }
    };
    let data = serde_json::to_value(WireConfigWriteRefusal::ConfinementAdministrationRequired)
        .map_err(serialize_error)?;
    Err(ToolCallError::new(
        -32602,
        format!("{refused}: {reason}; nothing was written"),
        Some(data),
    ))
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

async fn apply_patch_preview(
    config_runtime: &meerkat_core::ConfigRuntime,
    patch: &Value,
) -> Result<Config, ToolCallError> {
    // Single owner of patch semantics: meerkat-core. The preview is what the
    // store's patch commits (the delta merges onto the persisted document).
    config_runtime
        .patch_preview(&ConfigDelta(patch.clone()))
        .await
        .map_err(|err| match err {
            ConfigRuntimeError::Config(error @ meerkat_core::config::ConfigError::Json(_)) => {
                ToolCallError::invalid_params(format!("{error}"))
            }
            other => map_config_runtime_error(other),
        })
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
    // An OAuth account selection is a host-file or host-API decision. A host
    // may expose this management tool to an agent, so it never carries one;
    // nothing is staged or written.
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &input.server_config.transport
        && (http.oauth_account.is_some() || http.oauth_account_selection.is_some())
    {
        return Err(ToolCallError::invalid_params(
            "oauth_account and oauth_account_selection are host configuration and cannot be set through meerkat_mcp_add",
        ));
    }
    // Neither is an account declaration: an agent never states the account
    // review shows for a server's operations.
    if input.server_config.account.is_some() {
        return Err(ToolCallError::invalid_params(
            "account is host configuration and cannot be set through meerkat_mcp_add",
        ));
    }
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|error| ToolCallError::invalid_params(invalid_session_id_message(error)))?;
    // This tool carries no authenticated attachment control: a governed
    // runtime never attaches a server through it.
    if state.runtime_adapter.has_native_work_authorization_host() {
        return Err(ToolCallError::new(
            -32600,
            meerkat::surface::McpLiveAddError::GovernedRequiresControl.to_string(),
            None,
        ));
    }
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
    // Comms sent as a dormant mob member: its owner wakes it first.
    if state.service.comms_runtime(&session_id).await.is_none()
        && let Some(Err(error)) =
            meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global()
                .wake(&session_id)
                .await
    {
        return Err(session_owner_wake_tool_error(error));
    }
    if state.service.comms_runtime(&session_id).await.is_none() {
        return Err(ToolCallError::new(
            -32603,
            format!("Session not found or comms not enabled: {session_id}"),
            Some(json!({
                "code": "session_not_found_or_comms_disabled",
                "session_id": session_id.to_string(),
            })),
        ));
    }
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
    let receipt = state
        .service
        .send_comms(&session_id, cmd)
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
        })?
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
        meerkat_core::comms::SendError::DurableAdmissionRejected { envelope_id } => {
            typed_mcp_comms_send_error(&meerkat_contracts::CommsSendErrorData::peer_input_rejected(
                peer_name.unwrap_or("<unknown>"),
                *envelope_id,
            ))
        }
        // A seated member that was not woken: typed, not delivered, never
        // "not found".
        meerkat_core::comms::SendError::PeerWakeRefused {
            peer,
            envelope_id,
            refusal,
        } => typed_mcp_comms_send_error(&meerkat_contracts::CommsSendErrorData::peer_wake_refused(
            peer_name.unwrap_or(peer.as_str()),
            *envelope_id,
            refusal,
        )),
        // Not a failed send: the envelope may already be delivered, and a
        // caller told "failed" retries, which can duplicate the delivery.
        meerkat_core::comms::SendError::AmbiguousDelivery {
            envelope_id,
            detail,
        } => typed_mcp_comms_send_error(
            &meerkat_contracts::CommsSendErrorData::ambiguous_delivery(*envelope_id, detail),
        ),
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

/// A tool error carrying the typed `comms/send` error data RPC and REST share.
#[cfg(feature = "comms")]
fn typed_mcp_comms_send_error(data: &meerkat_contracts::CommsSendErrorData) -> ToolCallError {
    match serde_json::to_value(data) {
        Ok(value) => ToolCallError::new(
            -32603,
            format!("{}: {}", data.code(), data.message()),
            Some(value),
        ),
        Err(error) => ToolCallError::new(
            -32603,
            format!("failed to serialize comms error data: {error}"),
            None,
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
    // Like every help entry point, meerkat_help builds its session from the
    // one shared help session: the embedded skills whole in the system prompt
    // and the read-only reference tools as the session's only tools.
    let help = meerkat::help::help_session();
    let run_input = MeerkatRunInput {
        prompt,
        transient_turn_context: None,
        system_prompt: Some(help.system_prompt),
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
        reasoning_effort: None,
        budget_limits: None,
        preload_skills: None,
        skill_refs: None,
        skill_references: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
        auth_binding: None,
    };

    Box::pin(handle_meerkat_run_with_help_tools(
        state,
        run_input,
        notifier,
        request_context,
        Some((help.tools, help.tool_access_policy)),
    ))
    .await
}

async fn handle_meerkat_run(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    Box::pin(handle_meerkat_run_with_help_tools(
        state,
        input,
        notifier,
        request_context,
        None,
    ))
    .await
}

/// `help_tools` are a help session's only tools, with the launch tool access
/// policy that admits only them: the tools take the place of the client's
/// callback tools and of the session's live MCP router, and the session
/// persists the policy, so a later resume keeps it.
async fn handle_meerkat_run_with_help_tools(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
    request_context: Option<RequestContext>,
    help_tools: Option<(
        Arc<dyn AgentToolDispatcher>,
        meerkat_core::ops::ToolAccessPolicy,
    )>,
) -> Result<Value, ToolCallError> {
    validate_public_peer_meta(input.peer_meta.as_ref()).map_err(ToolCallError::invalid_params)?;
    // The session's top-level labels and application context pass the shared
    // public metadata rule, as on the JSON-RPC and REST session create.
    meerkat::surface::validate_public_surface_metadata(
        input.labels.as_ref(),
        input.app_context.as_ref(),
    )
    .map_err(ToolCallError::invalid_params)?;
    // The level has one home, the build's provider params: both spellings
    // of it must agree before anything is built.
    let build_provider_params = run_build_provider_params(&input)?;
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
    let help_tools_for_plan = help_tools;
    let event_tx_for_plan = event_tx.clone();
    let request_context_for_plan = request_context.clone();
    let stdio_profile_for_plan = state.mcp_stdio_launch_profile.clone();
    let attempt_guards_for_plan = state.mcp_attempt_guards.clone();
    let session_for_plan = session_id.clone();
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
            // A help session's only tools are its help tools: it gets no live
            // MCP router, so no MCP server or catalog control tool joins them.
            // Every other session's router launches local stdio servers inside
            // the host's own launch boundary.
            let adapter = help_tools_for_plan.is_none().then(|| {
                Arc::new(meerkat_mcp::McpRouterAdapter::new(guarded_router(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        stdio_profile_for_plan,
                    ),
                    attempt_guards_for_plan.as_ref(),
                    &session_for_plan,
                    // A fresh `meerkat_run` session is never a mob member.
                    None,
                )))
            });
            let external_tools = match help_tools_for_plan.as_ref() {
                Some((help_tools, _)) => Some(Arc::clone(help_tools)),
                None => compose_external_tool_dispatchers(
                    callback_tools_for_plan.clone(),
                    adapter
                        .clone()
                        .map(|adapter| adapter as Arc<dyn AgentToolDispatcher>),
                )
                .map_err(ToolCallError::internal)?,
            };
            let mut build = SessionBuildOptions {
                model_fallback: None,
                tool_access_policy: help_tools_for_plan
                    .as_ref()
                    .map(|(_, tool_access_policy)| tool_access_policy.clone()),
                declared_tool_restriction: None,
                tool_dispatch_admission: None,
                application_tool_policy: meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
                tool_consequence_policy_registry: None,
                custom_models: std::collections::BTreeMap::new(),
                image_generation_provider: None,
                auto_compact_threshold_override: None,
                compaction_curator_override: None,
                provider: Some(model_resolution.provider),
                override_comms: Default::default(),
                self_hosted_server_id: None,
                output_schema,
                structured_output_retries: input.structured_output_retries,
                hooks_override: input.hooks_override.clone().unwrap_or_default(),
                comms_name: input.comms_name.clone(),
                peer_meta: input.peer_meta.clone(),
                resume_session: Some(session),
                // `session` only claims the new session's id.
                resume_session_intent: meerkat_core::SessionBuildIntent::Mint,
                budget_limits: input.budget_limits.clone().map(Into::into),
                provider_params: build_provider_params.clone(),
                call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
                external_tools,
                // The session service decides hosting when it creates the actor.
                hosting: meerkat_core::session_hosting::SessionHostingIntent::default(),
                mcp_servers: Vec::new(),
                refused_mcp_servers: Vec::new(),
                recoverable_tool_defs: (!input.tools.is_empty())
                    .then(|| recoverable_callback_tool_defs(&input.tools)),
                llm_client_override: None,
                mcp_auth_resolver_override: None,
                session_comms_runtime_override: None,
                prepared_controller: None,
                host_prompt_sections: Default::default(),
                agent_llm_client_decorator: None,
                runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                initial_work_authorization: None,
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
                workgraph_namespace_grant: None,
                host_tools: None,
                mob_tool_authority_context: None,
                preload_skills,
                realm_id: Some(realm_id),
                instance_id,
                backend,
                config_generation: current_generation,
                auth_binding: model_resolution.auth_binding,
                mob_member_binding: None,
                mob_incarnation: None,
                keep_alive,
                checkpointer: None,
                silent_comms_intents: Vec::new(),
                max_inline_peer_notifications: None,
                app_context: input.app_context.clone(),
                fork_source: None,
                prompt_context_fork_source: None,
                additional_instructions: input.additional_instructions.clone(),
                // The help marker makes every rebuild of a help session from
                // storage refuse it.
                initial_metadata_entries: help_tools_for_plan
                    .as_ref()
                    .map(|_| meerkat::help::help_session_metadata_entry())
                    .into_iter()
                    .collect(),
                initial_tool_filter: None,
                initial_tool_visibility_state: None,
                shell_env: input.shell_env.clone(),
                shell_timeout_secs: requested_shell_timeout(input.builtin_config.as_ref()),
                resume_override_mask: ResumeOverrideMask {
                    model: input.model.is_some(),
                    provider: input.provider.is_some(),
                    auth_binding: input.auth_binding.is_some(),
                    max_tokens: input.max_tokens.is_some(),
                    structured_output_retries: input.structured_output_retries.is_some(),
                    provider_params: input.provider_params.is_some(),
                    reasoning_effort: input.reasoning_effort.is_some(),
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
                grant_gated_tools: None,
                grant_gated_tools_where_mounted: None,
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
            if let Some(context) = input.transient_turn_context.clone() {
                build
                    .initial_turn_metadata
                    .get_or_insert_with(Default::default)
                    .transient_turn_context = Some(context);
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
                router_to_publish: adapter,
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
    // The level a rebuild seats, by Core's one rule: a resume that names
    // `reasoning_effort` claims the session's level below.
    let resume_provider_params = resume_build_provider_params(&input)?;
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

    // A dormant mob member's session is woken by its owner (a mob this
    // process hosts) before this surface would attach the session itself.
    if let Some(Err(error)) = meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global()
        .wake(&session_id)
        .await
    {
        return Err(session_owner_wake_tool_error(error));
    }
    // Resume is an attach: resolve a crash-window provisional tail first.
    let claim = state
        .service
        .grant_session_hosting(&session_id)
        .map_err(prepare_session_tool_error)?;
    state
        .service
        .prepare_cold_attach(&session_id, claim)
        .await
        .map_err(prepare_session_tool_error)?;
    // A resume of a running session queues behind its durable run, whose
    // provisional tail stands ahead of the committed snapshot: the archive and
    // help checks and the stored settings read that snapshot.
    let session = state
        .service
        .load_committed_session_tolerating_tail(&session_id)
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to load session: {e}")))?
        .ok_or_else(|| {
            ToolCallError::new(
                -32004,
                format!("Session not found: {}", input.session_id),
                None,
            )
        })?;
    // A mob member's session that no mob in this process owns is its mob's to
    // materialize: a standalone resume would build the member outside its
    // mob, bypassing its wake. Refused typed; deliver through the mob.
    if let Some(binding) = session
        .session_metadata()
        .and_then(|metadata| metadata.mob_member_binding)
        && !meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global().owned(&session_id)
    {
        return Err(mob_member_session_tool_error(&session_id, &binding));
    }
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

    // A help session continues only on the live session its help entry
    // point built, where its help tools are: any rebuild from storage is
    // refused before a turn is admitted or a tool is composed.
    let help_session = meerkat::help::HelpSessionNotResumable::check(&session).err();
    if let Some(refusal) = help_session.as_ref()
        && mcp_resume_requires_rebuild(&input)
    {
        return Err(help_session_not_resumable_tool_error(refusal));
    }

    // A resume that names the provider-neutral reasoning effort level
    // rebuilds the session, and the rebuild replaces the session's runtime
    // attachment: a resume accepted behind a running turn can end with the
    // old attachment. So such a resume is taken only when no turn is running
    // on the session, and is otherwise answered busy at once: it does not
    // wait, and nothing is retired, staged or stored. The level is judged by
    // the rebuild, so a level the model refuses is answered its typed
    // refusal when the resume is sent again once the session is idle.
    if resume_names_reasoning_effort_level(&input)
        && state
            .service
            .session_activity_watch(&session_id)
            .await
            .is_some_and(|activity| activity.is_active())
    {
        return Err(reasoning_effort_change_needs_idle_session_tool_error());
    }

    // Admit callback results through the session service's exact durable
    // callback-batch authority. Direct transcript append bypasses ID-set,
    // idempotency, and replay validation.
    if !input.tool_results.is_empty() {
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        let staged = state
            .service
            .stage_tool_results(&session_id, StageToolResultsRequest { results })
            .await
            .map_err(|error| ToolCallError::invalid_params(error.to_string()))?;
        if callback_resume_is_already_applied(staged.disposition) {
            return wrap_tool_payload(json!({
                "status": "callback_results_already_accepted",
                "session_id": session_id.to_string(),
                "resumable": true,
            }))
            .map_err(ToolCallError::internal);
        }
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
    validate_effective_keep_alive(keep_alive).map_err(ToolCallError::invalid_params)?;
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
    // A live help session has no router by design and takes its turn on the
    // live session; any other help session would need a rebuild.
    let needs_rebuild = live_actor_lease.is_none()
        || (existing_adapter.is_none() && help_session.is_none())
        || !exact_live_attachment
        || mcp_resume_requires_rebuild(&input);
    drop(live_actor_lease);
    if let Some(refusal) = help_session.as_ref()
        && needs_rebuild
    {
        return Err(help_session_not_resumable_tool_error(refusal));
    }
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
            model_fallback: None,
            tool_access_policy: None,
            declared_tool_restriction: None,
            tool_dispatch_admission: None,
            application_tool_policy: meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
            tool_consequence_policy_registry: None,
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            compaction_curator_override: None,
            provider: llm_binding.provider,
            override_comms: Default::default(),
            self_hosted_server_id: llm_binding.self_hosted_server_id.clone(),
            output_schema: output_schema.clone(),
            structured_output_retries: input.structured_output_retries,
            hooks_override: input.hooks_override.clone().unwrap_or_default(),
            comms_name: input.comms_name.clone(),
            resume_session: Some(session.clone()),
            resume_session_intent: meerkat_core::SessionBuildIntent::Resume,
            budget_limits: input.budget_limits.clone().map(Into::into),
            provider_params: resume_provider_params.clone(),
            call_timeout_override: meerkat_core::CallTimeoutOverride::Inherit,
            external_tools,
            // The session service decides hosting when it creates the actor.
            hosting: meerkat_core::session_hosting::SessionHostingIntent::default(),
            mcp_servers: Vec::new(),
            refused_mcp_servers: Vec::new(),
            recoverable_tool_defs: (!input.tools.is_empty())
                .then(|| recoverable_callback_tool_defs(&input.tools)),
            llm_client_override: None,
            mcp_auth_resolver_override: None,
            session_comms_runtime_override: None,
            prepared_controller: None,
            host_prompt_sections: Default::default(),
            agent_llm_client_decorator: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings),
            initial_work_authorization: None,
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
            workgraph_namespace_grant: None,
            host_tools: None,
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
            mob_incarnation: None,
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            fork_source: None,
            prompt_context_fork_source: None,
            additional_instructions: input.additional_instructions.clone(),
            initial_metadata_entries: std::collections::BTreeMap::new(),
            initial_tool_filter: None,
            initial_tool_visibility_state: None,
            shell_env: None,
            // A rebuild keeps the session's recorded timeout unless the
            // resume asks for another.
            shell_timeout_secs: requested_shell_timeout(input.builtin_config.as_ref()).or_else(
                || {
                    session
                        .build_state()
                        .and_then(|state| state.shell_timeout_secs)
                },
            ),
            resume_override_mask: ResumeOverrideMask {
                model: input.model.is_some(),
                provider: llm_binding.provider_overridden,
                max_tokens: input.max_tokens.is_some(),
                structured_output_retries: input.structured_output_retries.is_some(),
                provider_params: input.provider_params.is_some(),
                reasoning_effort: input.reasoning_effort.is_some(),
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
            grant_gated_tools: None,
            grant_gated_tools_where_mounted: None,
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
        let transient_turn_context_for_plan = input.transient_turn_context.clone();
        let turn_tool_overlay_for_plan = input.turn_tool_overlay.clone();
        let event_tx_for_plan = event_tx.clone();
        let skill_references_for_plan = skill_references.clone();
        let requested_callback_tools_for_plan = requested_callback_tools.clone();
        let request_context_for_plan = request_context.clone();
        let stdio_profile_for_plan = state.mcp_stdio_launch_profile.clone();
        let attempt_guards_for_plan = state.mcp_attempt_guards.clone();
        let session_for_plan = session_id.clone();
        // A resumed mob member's guards are scoped to its member, as its
        // factory-built routers' are (the build adopts the same binding).
        let member_for_plan = stored_metadata.mob_member_binding.clone();
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
                // A missing router is rebuilt inside the host's launch
                // boundary, as the fresh path builds it.
                let adapter = exact_config.router.clone().unwrap_or_else(|| {
                    Arc::new(meerkat_mcp::McpRouterAdapter::new(guarded_router(
                        McpRouter::new_with_surface_handle_and_stdio_profile(
                            Arc::clone(resume_bindings.external_tool_surface()),
                            stdio_profile_for_plan,
                        ),
                        attempt_guards_for_plan.as_ref(),
                        &session_for_plan,
                        member_for_plan,
                    )))
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
                if let Some(system_prompt) = system_prompt_for_plan {
                    build
                        .initial_turn_metadata
                        .get_or_insert_with(Default::default)
                        .system_prompts
                        .push(system_prompt);
                }
                if let Some(context) = transient_turn_context_for_plan {
                    build
                        .initial_turn_metadata
                        .get_or_insert_with(Default::default)
                        .transient_turn_context = Some(context);
                }
                // The rebuilt agent's first turn is this resume's turn: it
                // carries the per-turn overlay as the live path does.
                if let Some(overlay) = turn_tool_overlay_for_plan {
                    build
                        .initial_turn_metadata
                        .get_or_insert_with(Default::default)
                        .turn_tool_overlay = Some(overlay.into());
                }
                let request = CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: model_for_plan,
                    prompt: prompt_for_plan.into(),
                    // A resume input's `system_prompt` is an ordinary System
                    // message for this admitted turn. Recovery restores the
                    // durable build configuration unchanged; the initial-turn
                    // metadata above appends the explicit System after claim.
                    system_prompt: meerkat::SystemPromptOverride::Inherit,
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
            #[cfg(feature = "comms")]
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
            transient_turn_context: input.transient_turn_context.clone(),
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
            system_prompt: input.system_prompt.clone(),
            event_tx: event_tx.clone(),
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                input.turn_tool_overlay.clone().map(Into::into),
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
                // The live session ended under the turn: a help session is
                // not rebuilt.
                if let Some(refusal) = help_session.as_ref() {
                    return Err(help_session_not_resumable_tool_error(refusal));
                }
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
    // Presence and archive state, read body-free: a run still going on the
    // session (this resume queued behind it) holds a provisional tail.
    let session_exists = match runtime_ingress::committed_source_archived(
        &state.service,
        &session_id,
    )
    .await
    {
        Ok(Some(true)) => {
            state
                .cleanup_archived_session_runtime(&session_id)
                .await
                .map_err(archive_session_error_to_tool_error)?;
            false
        }
        Ok(Some(false)) => true,
        Ok(None) => false,
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
        let router = if existing_adapter.is_some() || help_session.is_some() {
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

fn callback_resume_is_already_applied(disposition: StageToolResultsDisposition) -> bool {
    matches!(disposition, StageToolResultsDisposition::AlreadyApplied)
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
                    audience: Default::default(),
                    name: t.name.clone().into(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                    provenance: Some(meerkat_core::types::ToolProvenance {
                        kind: meerkat_core::types::ToolSourceKind::Builtin,
                        source_id: "mcp-server".into(),
                        artifact: None,
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

    pub(crate) mod committed_reads_tests;
    use meerkat::surface::{
        CancelActionInstallOutcome, SurfaceRequestExecutor, noop_request_action,
    };
    use meerkat::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Duration, timeout};

    /// `meerkat_resume` of a session whose durable tail is withheld from
    /// resume refuses with the hold's own wire code and its typed
    /// `durable_resume_hold` payload (the content is retained), not with an
    /// internal tool error.
    #[test]
    fn a_withheld_durable_tail_refuses_the_mcp_resume_typed() {
        use meerkat_core::service::DurableResumeHold;
        let id = meerkat_core::SessionId::new();
        for (held, hold) in [
            (
                DurableResumeHold::TailHeldForRecovery,
                "tail_held_for_recovery",
            ),
            (DurableResumeHold::RecoveryRefused, "recovery_refused"),
            (
                DurableResumeHold::EvidenceQuarantined,
                "evidence_quarantined",
            ),
        ] {
            let wire = meerkat_contracts::WireError::from(held.session_error(id.clone()));
            let tool_error = prepare_session_tool_error(held.session_error(id.clone()));
            assert_eq!(tool_error.code, wire.code.jsonrpc_code(), "{hold}");
            assert_ne!(tool_error.code, -32603, "{hold}: not an internal error");
            let data = tool_error.data.expect("the typed hold payload");
            assert_eq!(data["durable_resume_hold"], hold);
            assert_eq!(data["content_retained"], true);
            assert_eq!(data["session_id"], id.to_string());
        }
    }

    /// The head config store for the reserved `global` realm routes to the
    /// injected user-global document: an explicit state root must not shadow
    /// it with `<state_root>/global/config.toml`.
    #[test]
    fn global_realm_head_config_store_routes_to_the_user_global_doc() {
        let realms_root = PathBuf::from("/state/realms");
        let global_doc = PathBuf::from("/home/user/.rkat/config.toml");
        let source = mcp_realm_config_source(&realms_root, global_doc.clone());
        let store = tagged_realm_config_store(
            &realms_root,
            &meerkat_core::connection::RealmId::global(),
            meerkat_store::RealmBackend::Sqlite,
            None,
            &source,
        );
        let resolved = store
            .metadata()
            .and_then(|meta| meta.resolved_paths)
            .expect("tagged store reports resolved paths");
        assert_eq!(resolved.config_path, global_doc.display().to_string());
    }

    /// Non-global realms keep their per-realm head doc under the state root.
    #[test]
    fn non_global_realm_head_config_store_stays_realm_rooted() {
        let realms_root = PathBuf::from("/state/realms");
        let source =
            mcp_realm_config_source(&realms_root, PathBuf::from("/home/user/.rkat/config.toml"));
        let team = meerkat_core::connection::RealmId::parse("team").expect("valid realm id");
        let store = tagged_realm_config_store(
            &realms_root,
            &team,
            meerkat_store::RealmBackend::Sqlite,
            None,
            &source,
        );
        let resolved = store
            .metadata()
            .and_then(|meta| meta.resolved_paths)
            .expect("tagged store reports resolved paths");
        assert_eq!(
            resolved.config_path,
            meerkat_store::realm_paths_in(&realms_root, "team")
                .config_path
                .display()
                .to_string()
        );
    }

    #[tokio::test]
    async fn mcp_realm_bootstrap_preserves_durable_event_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = MeerkatMcpState::new_with_bootstrap_and_test_client(
            RuntimeBootstrap {
                realm: meerkat_core::RealmConfig {
                    selection: RealmSelection::Explicit {
                        realm_id: "mcp-projection".to_string(),
                    },
                    instance_id: None,
                    backend_hint: Some("sqlite".to_string()),
                    state_root: Some(temp.path().to_path_buf()),
                },
                context: meerkat_core::ContextConfig {
                    user_config_root: Some(temp.path().join("user")),
                    ..Default::default()
                },
            },
            false,
        )
        .await
        .expect("MCP realm bootstrap");

        assert!(
            state.service.has_event_projection(),
            "MCP must pass the complete realm persistence bundle to the shared runtime-backed service"
        );
    }

    /// A pre-0.8.37 `[model_fallback] enabled = true` document (no chain)
    /// loads with fallback disabled instead of refusing startup, and loading
    /// never rewrites it.
    #[tokio::test]
    async fn mcp_explicit_global_legacy_fallback_default_loads_disabled_without_rewriting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let user_root = temp.path().join("user");
        let config_dir = user_root.join(".rkat");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        let config_text = "[model_fallback]\nenabled = true\n";
        std::fs::write(&config_path, config_text).unwrap();
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "global".into(),
        };
        bootstrap.realm.state_root = Some(temp.path().join("realms"));
        bootstrap.context.user_config_root = Some(user_root);

        let state = MeerkatMcpState::new_with_bootstrap_and_test_client(bootstrap, false)
            .await
            .expect("legacy fallback default must not refuse startup");
        let head = state.config_runtime.get().await.expect("head config");
        assert!(!head.config.model_fallback.is_enabled());
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), config_text);
    }

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
                server: None,
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
                credential_account: None,
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
            Ok(CoreApplyOutput::with_untyped_snapshot(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
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

    fn provider_for_successful_mcp_test_model(model: &str) -> meerkat_core::Provider {
        if model.starts_with("claude-") {
            meerkat_core::Provider::Anthropic
        } else if model.starts_with("gpt-") || model.starts_with("o1-") {
            meerkat_core::Provider::OpenAI
        } else if model.starts_with("gemini-") {
            meerkat_core::Provider::Gemini
        } else {
            meerkat_core::Provider::Other
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
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        provider_for_successful_mcp_test_model(&request.model),
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
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

    /// Answers `ok` and records the tool names each request offers.
    struct ToolNameRecordingLlmClient {
        tool_names: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    #[async_trait]
    impl LlmClient for ToolNameRecordingLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            self.tool_names
                .lock()
                .expect("recorded tool names lock")
                .push(
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.to_string())
                        .collect(),
                );
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        provider_for_successful_mcp_test_model(&request.model),
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// `meerkat_help` runs with exactly the two read-only reference tools and
    /// no live MCP router; an ordinary `meerkat_run` is unchanged and keeps
    /// its router.
    #[tokio::test]
    async fn test_meerkat_help_runs_with_only_the_help_tools_and_no_live_mcp() {
        let tool_names = Arc::new(std::sync::Mutex::new(Vec::new()));
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            store,
            runtime_store,
            None,
            Some(Arc::new(ToolNameRecordingLlmClient {
                tool_names: Arc::clone(&tool_names),
            })),
        )
        .await;
        let session_id_of = |value: Value| {
            let payload = unwrap_payload(value);
            meerkat::SessionId::parse(payload["session_id"].as_str().expect("session id"))
                .expect("session id parses")
        };

        let help = Box::pin(handle_meerkat_help(
            &state,
            meerkat_contracts::HelpRequest {
                question: "How do I add an MCP server?".to_string(),
                prompt: None,
                execution_mode: meerkat_contracts::HelpExecutionMode::ExplainOnly,
                model: Some("claude-sonnet-4-5".to_string()),
                provider: Some("anthropic".to_string()),
                max_tokens: None,
            },
            None,
            None,
        ))
        .await
        .expect("meerkat_help succeeds");
        let help_session = session_id_of(help);
        let mut tools = tool_names.lock().expect("recorded tool names lock")[0].clone();
        tools.sort();
        assert_eq!(tools, ["skill_list_resources", "skill_read_resource"]);
        let ingress = state.runtime_ingress_context();
        assert!(ingress.logical_router(&help_session).await.is_none());

        let run_input: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "hello",
            "model": "claude-sonnet-4-5",
            "provider": "anthropic",
        }))
        .expect("run input");
        let run = Box::pin(handle_meerkat_run(&state, run_input, None, None))
            .await
            .expect("meerkat_run succeeds");
        let session = session_id_of(run);
        assert!(ingress.logical_router(&session).await.is_some());
    }

    /// Records the tool names each model request offers. An armed tool call
    /// is asked for by the next request only; every other request answers
    /// `ok`.
    #[derive(Default)]
    struct ToolCallScriptLlmClient {
        tool_names: std::sync::Mutex<Vec<Vec<String>>>,
        armed_call: std::sync::Mutex<Option<(String, Value)>>,
    }

    impl ToolCallScriptLlmClient {
        fn arm_call(&self, name: &str, args: Value) {
            *self.armed_call.lock().expect("armed call lock") = Some((name.to_string(), args));
        }

        fn requests(&self) -> Vec<std::collections::BTreeSet<String>> {
            self.tool_names
                .lock()
                .expect("recorded tool names lock")
                .iter()
                .map(|names| names.iter().cloned().collect())
                .collect()
        }
    }

    #[async_trait]
    impl LlmClient for ToolCallScriptLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            self.tool_names
                .lock()
                .expect("recorded tool names lock")
                .push(
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.to_string())
                        .collect(),
                );
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    provider_for_successful_mcp_test_model(&request.model),
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let armed = self.armed_call.lock().expect("armed call lock").take();
            let events = match armed {
                Some((name, args)) => vec![
                    Ok(LlmEvent::ToolCallComplete {
                        id: format!("call_{name}"),
                        name,
                        args,
                        meta: None,
                    }),
                    Ok(usage),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                    }),
                ],
                None => vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    }),
                    Ok(usage),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    }),
                ],
            };
            Box::pin(stream::iter(events))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// The launch policy of a help session: only the two help tools.
    fn help_launch_policy() -> meerkat_core::ops::ToolAccessPolicy {
        meerkat_core::ops::ToolAccessPolicy::AllowList(
            ["skill_list_resources", "skill_read_resource"]
                .into_iter()
                .collect(),
        )
    }

    /// The tool names of the catalog control plane.
    fn catalog_control_plane() -> std::collections::BTreeSet<String> {
        ["tool_catalog_search", "tool_catalog_load"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// Asserts that the call `tool_use_id` was refused as `access_denied`.
    fn assert_refused_as_access_denied(session: &Session, tool_use_id: &str) {
        let result = session
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                meerkat_core::types::Message::ToolResults { results, .. } => results
                    .iter()
                    .find(|result| result.tool_use_id == tool_use_id)
                    .cloned(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no tool result for {tool_use_id}"));
        assert!(result.is_error, "{tool_use_id} must fail: {result:?}");
        let payload: Value =
            serde_json::from_str(&result.text_content()).expect("typed tool error payload");
        assert_eq!(
            payload["error"], "access_denied",
            "{tool_use_id} must be refused by the session's policy: {payload}"
        );
    }

    fn session_id_of(value: Value) -> meerkat::SessionId {
        let payload = unwrap_payload(value);
        meerkat::SessionId::parse(payload["session_id"].as_str().expect("session id"))
            .expect("session id parses")
    }

    async fn load_saved_session(
        state: &MeerkatMcpState,
        session_id: &meerkat::SessionId,
    ) -> Session {
        state
            .service
            .load_authoritative_session(session_id)
            .await
            .expect("load the saved session")
            .expect("the session is saved")
    }

    fn help_tool_names() -> std::collections::BTreeSet<String> {
        ["skill_list_resources", "skill_read_resource"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    /// A server over `store` and `runtime_store` whose model is `llm`.
    async fn mcp_server_over(
        store: &Arc<dyn SessionStore>,
        runtime_store: &Arc<dyn meerkat_runtime::RuntimeStore>,
        llm: &Arc<ToolCallScriptLlmClient>,
    ) -> MeerkatMcpState {
        MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(store),
            Arc::clone(runtime_store),
            None,
            Some(Arc::clone(llm) as Arc<dyn meerkat::LlmClient>),
        )
        .await
    }

    /// A help session asked through `meerkat_help` and an ordinary session
    /// run through `meerkat_run` on `state`.
    async fn help_and_ordinary_sessions(
        state: &MeerkatMcpState,
    ) -> (meerkat::SessionId, meerkat::SessionId) {
        let help_session = session_id_of(
            Box::pin(handle_meerkat_help(
                state,
                meerkat_contracts::HelpRequest {
                    question: "How do I add an MCP server?".to_string(),
                    prompt: None,
                    execution_mode: meerkat_contracts::HelpExecutionMode::ExplainOnly,
                    model: Some("claude-sonnet-4-5".to_string()),
                    provider: Some("anthropic".to_string()),
                    max_tokens: None,
                },
                None,
                None,
            ))
            .await
            .expect("meerkat_help succeeds"),
        );
        // A restarted test server opens a fresh realm, and a WorkGraph
        // namespace grant is bound to its realm, so the ordinary session
        // leaves WorkGraph off to stay resumable there.
        let run_input: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "hello",
            "model": "claude-sonnet-4-5",
            "provider": "anthropic",
            "enable_workgraph": false,
        }))
        .expect("run input");
        let ordinary = session_id_of(
            Box::pin(handle_meerkat_run(state, run_input, None, None))
                .await
                .expect("meerkat_run succeeds"),
        );
        (help_session, ordinary)
    }

    async fn resume_session(
        state: &MeerkatMcpState,
        session_id: &meerkat::SessionId,
        tools: Vec<McpToolDef>,
    ) -> Result<Value, ToolCallError> {
        Box::pin(handle_meerkat_resume(
            state,
            bounded_resume_input(session_id.to_string(), tools),
            None,
            None,
        ))
        .await
    }

    /// Resume `session_id` with the client's callback `tools` and no other
    /// override. Like the other cold-resume tests, accept the one post-turn
    /// failure this harness can raise, the peer-ingress refresh.
    async fn resume_with_client_tools(
        state: &MeerkatMcpState,
        session_id: &meerkat::SessionId,
        tools: Vec<McpToolDef>,
    ) -> Option<Value> {
        match resume_session(state, session_id, tools).await {
            Ok(payload) => Some(unwrap_payload(payload)),
            Err(error)
                if error
                    .message
                    .contains("failed to update peer ingress context") =>
            {
                None
            }
            Err(error) => panic!("meerkat_resume of {session_id} failed: {error:?}"),
        }
    }

    fn callback_tool(name: &str) -> McpToolDef {
        McpToolDef {
            name: name.to_string(),
            description: "A tool the MCP client serves".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        }
    }

    /// Asserts that `meerkat_resume` refused to rebuild `help_session` from
    /// storage with the typed help-session refusal, before any model call and
    /// without publishing an MCP router for it.
    async fn assert_help_rebuild_refused(
        state: &MeerkatMcpState,
        help_session: &meerkat::SessionId,
        llm: &ToolCallScriptLlmClient,
        result: Result<Value, ToolCallError>,
    ) {
        let refusal = result.expect_err("a help session is not rebuilt from storage");
        assert_eq!(
            refusal.code,
            meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code(),
            "{refusal:?}"
        );
        assert_eq!(
            refusal.data.as_ref().map(|data| data["kind"].clone()),
            Some(json!("help_session_not_resumable")),
            "{refusal:?}"
        );
        assert!(llm.requests().is_empty(), "no model call");
        assert!(
            state
                .runtime_ingress_context()
                .logical_router(help_session)
                .await
                .is_none(),
            "no MCP router was published for the help session"
        );
    }

    /// A help session continues only on the live session `meerkat_help`
    /// built: while that runs, `meerkat_resume` takes the turn there with the
    /// help tools. On a restarted server, `meerkat_resume` with no override
    /// refuses to rebuild it from storage with a typed error before any model
    /// call, and publishes no MCP router.
    #[tokio::test]
    async fn test_a_saved_help_session_runs_warm_and_is_refused_cold() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let warm_llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &warm_llm).await;
        let (help_session, _ordinary) = help_and_ordinary_sessions(&state).await;
        let saved = load_saved_session(&state, &help_session).await;

        resume_with_client_tools(&state, &help_session, vec![]).await;
        assert_eq!(
            warm_llm.requests().last(),
            Some(&help_tool_names()),
            "a live help session keeps its help tools"
        );
        assert_eq!(
            saved.metadata().get("meerkat.help_session"),
            Some(&json!(true)),
            "meerkat_help marks its session as a help session"
        );

        // Reconstruct the server against the same stores to model a restart.
        drop(state);
        let llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &llm).await;
        let result = resume_session(&state, &help_session, vec![]).await;
        assert_help_rebuild_refused(&state, &help_session, &llm, result).await;
    }

    /// A client resuming a saved help session on a restarted server with its
    /// callback tools, one of them named like a help tool, is refused before
    /// any model call: no call is handed back to the client and no MCP router
    /// is published. An ordinary session resumed the same way offers the
    /// callback tools.
    #[tokio::test]
    async fn test_a_saved_help_session_resumed_cold_refuses_client_callback_tools() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let first_llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &first_llm).await;
        let (help_session, ordinary) = help_and_ordinary_sessions(&state).await;
        drop(state);
        let llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &llm).await;
        let client_tools = || {
            vec![
                callback_tool("client_callback"),
                callback_tool("skill_read_resource"),
            ]
        };

        let result = resume_session(&state, &help_session, client_tools()).await;
        assert_help_rebuild_refused(&state, &help_session, &llm, result).await;

        // Control: an ordinary session resumed the same way offers them.
        resume_with_client_tools(&state, &ordinary, client_tools()).await;
        let offered = llm.requests().last().cloned().unwrap_or_default();
        assert!(
            offered.contains("client_callback") && offered.contains("skill_read_resource"),
            "an ordinary resumed session offers the client's callback tools: {offered:?}"
        );
    }

    /// The help launch policy is the second line: a saved help session that
    /// carries no help marker, as one saved before the marker existed does,
    /// is rebuilt by `meerkat_resume` under the policy `meerkat_help`
    /// persisted. The model is offered only the catalog control plane of the
    /// rebuilt router, and a call to it is refused and loads nothing.
    #[tokio::test]
    async fn test_a_saved_help_session_without_its_marker_keeps_its_launch_policy() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let first_llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &first_llm).await;
        let (help_session, _ordinary) = help_and_ordinary_sessions(&state).await;
        let mut unmarked = load_saved_session(&state, &help_session).await;
        let first_turn_tooling = unmarked
            .session_metadata()
            .expect("help session metadata")
            .tooling;
        unmarked.remove_metadata("meerkat.help_session");
        drop(state);
        seed_runtime_authority_session(&runtime_store, &unmarked).await;
        let llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &llm).await;

        llm.arm_call(
            "tool_catalog_load",
            json!({"names": ["skill_read_resource"]}),
        );
        resume_with_client_tools(&state, &help_session, vec![]).await;
        let requests = llm.requests();
        let [.., offered, after_load] = requests.as_slice() else {
            panic!("the resumed turn asks the model twice: {requests:?}");
        };
        assert_eq!(
            offered,
            &catalog_control_plane(),
            "the restored policy offers no composed tool but the control plane"
        );
        assert_eq!(
            first_turn_tooling.spawn_tool_access_policy,
            Some(meerkat_core::ops::SpawnToolAccessPolicy::Restricted {
                policy: help_launch_policy()
            }),
            "meerkat_help persists the launch policy that admits only the help tools"
        );
        let resumed = load_saved_session(&state, &help_session).await;
        assert_eq!(
            resumed
                .session_metadata()
                .expect("help session metadata")
                .tooling
                .tool_access_policy,
            Some(help_launch_policy()),
            "the rebuild restores the help session's launch policy"
        );
        assert_refused_as_access_denied(&resumed, "call_tool_catalog_load");
        assert_eq!(after_load, offered, "the refused load attaches no tool");
    }

    /// `builtin_config.shell_timeout_secs` sets the shell's default command
    /// timeout for the session it builds: a command that outlives a short
    /// override is stopped by it instead of running to its end under the 30 s
    /// default.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_meerkat_run_honours_a_short_shell_timeout_override() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = mcp_server_over(&store, &runtime_store, &llm).await;
        llm.arm_call("shell", json!({"command": "sleep 5"}));
        let run_input: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "run the command",
            "model": "claude-sonnet-4-5",
            "provider": "anthropic",
            "enable_builtins": true,
            "enable_workgraph": false,
            "builtin_config": {"enable_shell": true, "shell_timeout_secs": 1},
        }))
        .expect("run input");
        let session = session_id_of(
            Box::pin(handle_meerkat_run(&state, run_input, None, None))
                .await
                .expect("meerkat_run succeeds"),
        );

        let saved = load_saved_session(&state, &session).await;
        let result = saved
            .messages()
            .iter()
            .find_map(|message| match message {
                meerkat_core::types::Message::ToolResults { results, .. } => results
                    .iter()
                    .find(|result| result.tool_use_id == "call_shell")
                    .cloned(),
                _ => None,
            })
            .expect("the shell call has a result");
        let text = result.text_content();
        assert!(
            text.contains("timed out after"),
            "the 1 s override stops the 5 s command: {text}"
        );
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
            request: &'a LlmRequest,
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
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Other,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
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
        state_with_persisted_session_and_llm(Arc::new(TestClient::for_provider(
            Provider::Anthropic,
        )))
        .await
    }

    async fn state_with_persisted_session_and_llm(
        llm_client: Arc<dyn LlmClient>,
    ) -> (MeerkatMcpState, String) {
        state_with_persisted_session_for(llm_client, None).await
    }

    /// A persisted session, a mob member's when `member` is given.
    async fn state_with_persisted_session_for(
        llm_client: Arc<dyn LlmClient>,
        member: Option<meerkat_core::MobMemberBinding>,
    ) -> (MeerkatMcpState, String) {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
            Some(llm_client),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().to_string();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                model_fallback: None,
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Anthropic,
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
                mob_member_binding: member,
            })
            .expect("session metadata should serialize");
        session
            .set_build_state(meerkat_core::SessionBuildState::default())
            .expect("session build state should serialize");
        store.save(&session).await.expect("persisted session");
        seed_runtime_authority_session(&runtime_store, &session).await;
        (state, session_id)
    }

    /// A launch boundary over `root` that no platform backend enforces
    /// (`require_descendant_termination` is unsupported on every backend),
    /// so every stdio launch under it is refused before its program runs.
    #[cfg(unix)]
    fn unenforceable_stdio_profile(root: &std::path::Path) -> meerkat_mcp::McpStdioLaunchProfile {
        use meerkat_core::confinement::{
            ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess, PlatformBaseline,
        };
        let requirement = ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_path_buf())]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.to_path_buf())]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: true,
        }
        .try_into()
        .expect("structurally valid requirement");
        meerkat_mcp::McpStdioLaunchProfile::required(requirement, root.to_path_buf())
    }

    /// Stage a stdio server on `router` and assert the launch boundary
    /// refused it, typed, without starting its process.
    #[cfg(unix)]
    async fn assert_router_refuses_stdio_launch(
        router: &meerkat_mcp::McpRouterAdapter,
        root: &std::path::Path,
    ) {
        let marker = root.join("launch-marker");
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "confined-server",
                "/bin/sh",
                vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
                HashMap::from([("MARKER".into(), marker.display().to_string())]),
            ))
            .await
            .expect("stage the server");
        router.apply_staged().await.expect("apply the staged add");
        let deltas = router
            .wait_until_ready(std::time::Duration::from_secs(15))
            .await
            .expect("the add settles");
        assert!(
            deltas.iter().any(|delta| {
                delta.target == "confined-server"
                    && delta.phase == meerkat_core::ExternalToolDeltaPhase::Failed
                    && delta.confinement_refusal
                        == Some(
                            meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
                        )
            }),
            "the launch is refused by the state's boundary: {deltas:?}"
        );
        assert!(!marker.exists(), "a refused server never starts");
    }

    /// A fresh `meerkat_run` builds its session's router inside the state's
    /// MCP stdio launch profile.
    #[cfg(unix)]
    #[tokio::test]
    async fn fresh_run_router_launches_inside_the_state_stdio_launch_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
        )
        .await;
        state.mcp_stdio_launch_profile = unenforceable_stdio_profile(&root);
        let input: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "Hello",
            "model": "claude-opus-4-8",
            "provider": "anthropic"
        }))
        .expect("run input");
        let result = Box::pin(handle_meerkat_run(&state, input, None, None))
            .await
            .expect("the fresh run completes");
        let session_id = meerkat::SessionId::parse(
            unwrap_payload(result)["session_id"]
                .as_str()
                .expect("run result names its session"),
        )
        .expect("valid session id");
        let router = state
            .runtime_ingress_context()
            .logical_router(&session_id)
            .await
            .expect("the fresh session publishes its router");
        assert_router_refuses_stdio_launch(&router, &root).await;
    }

    /// A resume whose session has no live router rebuilds it inside the
    /// state's MCP stdio launch profile, as the fresh path builds it.
    #[cfg(unix)]
    #[tokio::test]
    async fn resume_without_a_router_rebuilds_it_inside_the_state_stdio_launch_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (mut state, session_id) = state_with_persisted_session().await;
        state.mcp_stdio_launch_profile = unenforceable_stdio_profile(&root);
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        assert!(
            state
                .runtime_ingress_context()
                .logical_router(&parsed)
                .await
                .is_none(),
            "the cold session has no router before its resume"
        );
        let result = Box::pin(handle_meerkat_resume(
            &state,
            bounded_resume_input(session_id, vec![]),
            None,
            None,
        ))
        .await;
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "the resume reaches its post-completion boundary: {result:?}"
        );
        let router = state
            .runtime_ingress_context()
            .logical_router(&parsed)
            .await
            .expect("the resume publishes the rebuilt router");
        assert_router_refuses_stdio_launch(&router, &root).await;
    }

    /// Schedule seeding builds its router inside the state's MCP stdio
    /// launch profile, so a realm's configured server is refused too.
    #[cfg(unix)]
    #[tokio::test]
    async fn schedule_seeding_launches_inside_the_state_stdio_launch_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let marker = root.join("seeded-launch-marker");
        let mut state =
            MeerkatMcpState::new_with_store(Arc::new(meerkat::MemoryStore::new())).await;
        state.mcp_stdio_launch_profile = unenforceable_stdio_profile(&root);
        let server = meerkat_core::McpServerConfig::stdio(
            "confined-seeded",
            "/bin/sh",
            vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
            HashMap::from([("MARKER".into(), marker.display().to_string())]),
        );
        state
            .config_runtime
            .patch(
                meerkat_core::ConfigDelta(json!({"tools": {"mcp_servers": [server]}})),
                None,
            )
            .await
            .expect("the realm configures its server");
        let adapter = schedule_host::seed_realm_mcp_adapter_for_test(
            &state,
            Arc::new(meerkat_runtime::handles::RuntimeExternalToolSurfaceHandle::ephemeral()),
        )
        .await
        .expect("schedule seeding stages the realm's server");
        let deltas = adapter
            .wait_until_ready(std::time::Duration::from_secs(15))
            .await
            .expect("the seeded add settles");
        assert!(
            deltas.iter().any(|delta| {
                delta.target == "confined-seeded"
                    && delta.confinement_refusal
                        == Some(
                            meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement,
                        )
            }),
            "the seeded launch is refused by the state's boundary: {deltas:?}"
        );
        assert!(!marker.exists(), "a refused seeded server never starts");
    }

    /// A host guard factory that refuses every attempt and keeps what it
    /// was asked.
    #[derive(Default)]
    struct RefusingGuards(StdMutex<Vec<meerkat_mcp::McpAttemptGuardRequest>>);

    #[async_trait::async_trait]
    impl meerkat_mcp::McpAttemptGuardFactory for RefusingGuards {
        async fn begin(
            &self,
            request: meerkat_mcp::McpAttemptGuardRequest,
            _cancel: meerkat_mcp::McpAttemptCancellation,
        ) -> Result<
            Option<Box<dyn meerkat_mcp::McpAttemptGuard>>,
            meerkat_core::mcp_attachment::McpAttachmentRefusal,
        > {
            self.0.lock().unwrap().push(request);
            Err(meerkat_core::mcp_attachment::McpAttachmentRefusal::Denied)
        }
    }

    fn refusing_guards() -> (Arc<RefusingGuards>, meerkat_mcp::McpAttemptGuards) {
        let refusing = Arc::new(RefusingGuards::default());
        let guards = meerkat_mcp::McpAttemptGuards::new(
            Arc::clone(&refusing) as Arc<dyn meerkat_mcp::McpAttemptGuardFactory>,
            meerkat_mcp::McpAttemptGuardBounds::DEFAULT,
        );
        (refusing, guards)
    }

    /// Stage a stdio server on `router` and assert the host's guard was asked
    /// once, for this router, and refused it before its process started;
    /// returns what the guard was asked.
    #[cfg(unix)]
    async fn assert_guard_refuses_stdio_launch(
        router: &meerkat_mcp::McpRouterAdapter,
        refusing: &RefusingGuards,
        root: &std::path::Path,
    ) -> meerkat_mcp::McpAttemptGuardRequest {
        let marker = root.join("guarded-launch-marker");
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "guarded-server",
                "/bin/sh",
                vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
                HashMap::from([("MARKER".into(), marker.display().to_string())]),
            ))
            .await
            .expect("stage the server");
        router.apply_staged().await.expect("apply the staged add");
        let deltas = router
            .wait_until_ready(std::time::Duration::from_secs(15))
            .await
            .expect("the add settles");
        assert!(
            deltas.iter().any(|delta| {
                delta.target == "guarded-server"
                    && delta.phase == meerkat_core::ExternalToolDeltaPhase::Failed
            }),
            "the host's guard refuses the launch: {deltas:?}"
        );
        assert!(!marker.exists(), "a refused server never starts");
        let requests = refusing.0.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "the guard began once: {requests:?}");
        assert_eq!(requests[0].key.server, "guarded-server");
        assert_eq!(requests[0].key.construction, router.construction_id());
        requests[0].clone()
    }

    /// A fresh `meerkat_run` builds its session's router with the host's
    /// attempt guards, for that session (never a mob member's).
    #[cfg(unix)]
    #[tokio::test]
    async fn fresh_run_router_begins_the_installed_attempt_guards() {
        let temp = tempfile::tempdir().unwrap();
        let (refusing, guards) = refusing_guards();
        let mut state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::new(meerkat::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
        )
        .await;
        state.mcp_attempt_guards = Some(guards);
        let input: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "Hello",
            "model": "claude-opus-4-8",
            "provider": "anthropic"
        }))
        .expect("run input");
        let result = Box::pin(handle_meerkat_run(&state, input, None, None))
            .await
            .expect("the fresh run completes");
        let session_id = meerkat::SessionId::parse(
            unwrap_payload(result)["session_id"]
                .as_str()
                .expect("run result names its session"),
        )
        .expect("valid session id");
        let router = state
            .runtime_ingress_context()
            .logical_router(&session_id)
            .await
            .expect("the fresh session publishes its router");
        let request = assert_guard_refuses_stdio_launch(&router, &refusing, temp.path()).await;
        assert_eq!(request.owner.session_id, session_id);
        assert_eq!(request.owner.member, None);
    }

    /// The owner of a mob member's session in this process, as a mob that
    /// holds the member registers it: its wake answers that the member is
    /// ready, and the surface's operation proceeds. Scripted: it
    /// materializes nothing, so the resume builds the session itself.
    #[cfg(unix)]
    struct HoldingSessionOwner;

    #[cfg(unix)]
    impl meerkat_core::session_owner_wake::SessionOwnerWaker for HoldingSessionOwner {
        fn wake_session_owner(
            &self,
            _session_id: &meerkat::SessionId,
        ) -> meerkat_core::session_owner_wake::SessionOwnerWakeFuture {
            Box::pin(async { Ok(meerkat_core::session_owner_wake::SessionOwnerWakeOutcome::Ready) })
        }
    }

    /// A resume whose session has no live router rebuilds it with the host's
    /// attempt guards, scoped to the resumed session's mob member (as its
    /// factory-built routers are). The member's session is owned in this
    /// process (a mob holds the member): a standalone resume of a member's
    /// session that no mob here owns is refused instead (the row below).
    #[cfg(unix)]
    #[tokio::test]
    async fn resume_rebuilds_its_router_with_the_guards_and_the_member_scope() {
        let temp = tempfile::tempdir().unwrap();
        let (refusing, guards) = refusing_guards();
        let member = meerkat_core::MobMemberBinding {
            mob_id: "mob".into(),
            role: "worker".into(),
            member: "worker-1".into(),
        };
        let (mut state, session_id) = state_with_persisted_session_for(
            Arc::new(TestClient::for_provider(Provider::Anthropic)),
            Some(member.clone()),
        )
        .await;
        state.mcp_attempt_guards = Some(guards);
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let owner: Arc<dyn meerkat_core::session_owner_wake::SessionOwnerWaker> =
            Arc::new(HoldingSessionOwner);
        let owners = meerkat_core::session_owner_wake::SessionOwnerWakeRegistry::global();
        owners.publish(parsed.clone(), Arc::downgrade(&owner));
        let result = Box::pin(handle_meerkat_resume(
            &state,
            bounded_resume_input(session_id, vec![]),
            None,
            None,
        ))
        .await;
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "the resume reaches its post-completion boundary: {result:?}"
        );
        let router = state
            .runtime_ingress_context()
            .logical_router(&parsed)
            .await
            .expect("the resume publishes the rebuilt router");
        let request = assert_guard_refuses_stdio_launch(&router, &refusing, temp.path()).await;
        assert_eq!(request.owner.session_id, parsed);
        assert_eq!(
            request.owner.member,
            Some(member),
            "a resumed mob member's guard is scoped to its member"
        );
        assert!(owners.retract(&parsed, &Arc::downgrade(&owner)));
    }

    /// A standalone resume of a mob member's session that no mob in this
    /// process owns is refused, typed: the member is its mob's to
    /// materialize, through its wake. Nothing of it is built here.
    #[tokio::test]
    async fn resume_of_a_member_session_no_mob_here_owns_is_refused_typed() {
        let member = meerkat_core::MobMemberBinding {
            mob_id: "mob".into(),
            role: "worker".into(),
            member: "worker-1".into(),
        };
        let (state, session_id) = state_with_persisted_session_for(
            Arc::new(TestClient::for_provider(Provider::Anthropic)),
            Some(member),
        )
        .await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let refused = Box::pin(handle_meerkat_resume(
            &state,
            bounded_resume_input(session_id, vec![]),
            None,
            None,
        ))
        .await
        .expect_err("a member's session no mob here owns is refused");
        assert!(
            refused
                .message
                .contains("belongs to member 'worker-1' of mob 'mob'")
                && refused
                    .message
                    .contains("deliver to the member through its mob"),
            "{refused:?}"
        );
        assert!(
            state
                .runtime_ingress_context()
                .logical_router(&parsed)
                .await
                .is_none(),
            "nothing of the member is built outside its mob"
        );
    }

    /// Schedule seeding installs the host's MCP attempt guards: a seeded
    /// stdio server begins the host's guard before anything of it runs, and
    /// a refusing host's server never starts.
    #[cfg(unix)]
    #[tokio::test]
    async fn schedule_seeding_begins_the_installed_attempt_guards() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("guarded-seeded-marker");
        let (refusing, guards) = refusing_guards();
        let state = MeerkatMcpState::new_with_store_and_attempt_guards(
            Arc::new(meerkat::MemoryStore::new()),
            guards.clone(),
        )
        .await;
        let server = meerkat_core::McpServerConfig::stdio(
            "guarded-seeded",
            "/bin/sh",
            vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
            HashMap::from([("MARKER".into(), marker.display().to_string())]),
        );
        state
            .config_runtime
            .patch(
                meerkat_core::ConfigDelta(json!({"tools": {"mcp_servers": [server]}})),
                None,
            )
            .await
            .expect("the realm configures its server");
        let adapter = schedule_host::seed_realm_mcp_adapter_for_test(
            &state,
            Arc::new(meerkat_runtime::handles::RuntimeExternalToolSurfaceHandle::ephemeral()),
        )
        .await
        .expect("schedule seeding stages the realm's server");
        let deltas = adapter
            .wait_until_ready(std::time::Duration::from_secs(15))
            .await
            .expect("the seeded add settles");
        assert!(
            deltas.iter().any(|delta| {
                delta.target == "guarded-seeded"
                    && delta.phase == meerkat_core::ExternalToolDeltaPhase::Failed
            }),
            "the host's guard refuses the seeded server: {deltas:?}"
        );
        assert!(!marker.exists(), "a refused seeded server never starts");
        let requests = refusing.0.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "the guard began once: {requests:?}");
        assert_eq!(requests[0].key.server, "guarded-seeded");
        assert_eq!(requests[0].key.construction, adapter.construction_id());
        adapter.shutdown().await;
        let _ = guards
            .close_and_join(tokio::time::Instant::now() + std::time::Duration::from_secs(10))
            .await;
    }

    async fn materialize_test_mcp_fixture<F>(
        state: &MeerkatMcpState,
        session_id: &meerkat::SessionId,
        mode: McpActorMaterializationMode,
        build_plan: F,
    ) -> meerkat::RunResult
    where
        F: FnOnce(
            meerkat_core::SessionRuntimeBindings,
        ) -> (CreateSessionRequest, Arc<meerkat_mcp::McpRouterAdapter>),
    {
        let ingress = state.runtime_ingress_context();
        let outcome = materialize_mcp_actor_transaction(
            &state.service,
            &state.runtime_adapter,
            &ingress,
            session_id,
            mode,
            None,
            move |bindings, _logical_config| {
                let (request, router) = build_plan(bindings);
                std::future::ready(Ok::<McpActorMaterializationPlan, ToolCallError>(
                    McpActorMaterializationPlan {
                        request,
                        callback_config: runtime_ingress::McpCallbackConfig::Preserve,
                        router_to_publish: Some(router),
                    },
                ))
            },
        )
        .await
        .expect("test MCP materialization transaction should report");
        assert!(
            outcome.committed,
            "successful test MCP materialization must commit its exact attachment"
        );
        let result = outcome
            .result
            .expect("test MCP materialization transaction should succeed");
        let sidecar_witness = ingress
            .current_attachment_witness(session_id)
            .await
            .expect("committed test MCP sidecar must retain an attachment witness");
        let machine_witness = state
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .expect("committed test MCP session must retain a machine attachment witness");
        assert_eq!(
            sidecar_witness, machine_witness,
            "test MCP sidecar must publish the machine's exact attachment witness"
        );
        result
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
        materialize_test_mcp_fixture(
            state,
            session_id,
            McpActorMaterializationMode::Resume,
            move |bindings| {
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
                        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                        ..Default::default()
                    }),
                    labels: None,
                };
                (request, router)
            },
        )
        .await;
    }

    /// The bound on a resume that must not re-enter the registration lock.
    ///
    /// These tests assert a structural property: the property either holds and
    /// the resume completes in milliseconds, or it does not and the resume
    /// never completes at all. The timeout exists only so a re-entrant lock
    /// fails the test instead of hanging the run forever, so it wants to be
    /// generous rather than tight. A tight bound measures how loaded the box
    /// is, which is a different question - at three seconds these failed under
    /// a full-workspace run on a machine that was also compiling, and passed
    /// in 0.13s standalone, which says nothing about locking either way.
    const NO_DEADLOCK_BOUND: Duration = Duration::from_mins(1);

    fn bounded_resume_input(session_id: String, tools: Vec<McpToolDef>) -> MeerkatResumeInput {
        MeerkatResumeInput {
            session_id,
            prompt: "Resume".to_string(),
            transient_turn_context: None,
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
            reasoning_effort: None,
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
            NO_DEADLOCK_BOUND,
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
    async fn cold_resume_appends_turn_system_once_without_rewriting_build_config() {
        let (state, session_id) = state_with_persisted_session().await;
        let parsed = meerkat::SessionId::parse(&session_id).expect("valid session id");
        let mut input = bounded_resume_input(session_id, vec![]);
        input.system_prompt = Some("cold turn system".to_string());

        let result = tokio::time::timeout(
            NO_DEADLOCK_BOUND,
            Box::pin(handle_meerkat_resume(&state, input, None, None)),
        )
        .await
        .expect("cold resume with an explicit System must not deadlock");
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "cold resume must reach its post-completion boundary: {result:?}"
        );

        let stored = state
            .service
            .load_authoritative_session(&parsed)
            .await
            .expect("load cold-resumed session")
            .expect("cold-resumed session remains durable");
        let system_indices = stored
            .messages()
            .iter()
            .enumerate()
            .filter_map(|(index, message)| match message {
                meerkat_core::types::Message::System(message)
                    if message.content == "cold turn system" =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            system_indices.len(),
            1,
            "cold materialization must append the turn System exactly once"
        );
        let user_index = stored
            .messages()
            .iter()
            .position(|message| matches!(message, meerkat_core::types::Message::User(_)))
            .expect("cold resume must append its user input");
        assert!(
            system_indices[0] < user_index,
            "the admitted System must precede the same turn's user input"
        );
    }

    #[tokio::test]
    async fn live_resume_appends_turn_system_without_forcing_actor_rebuild() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::default())),
        )
        .await;
        let session = Session::new();
        let session_id = session.id().clone();
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                request
                    .build
                    .get_or_insert_with(Default::default)
                    .external_tools = Some(router_tools);
                (request, router)
            },
        )
        .await;
        let attachment_before = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("live fixture has an exact attachment");
        let mut input = bounded_resume_input(session_id.to_string(), vec![]);
        input.system_prompt = Some("live turn system".to_string());

        handle_meerkat_resume(&state, input, None, None)
            .await
            .expect("live resume with an explicit System should run");

        assert_eq!(
            state
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            Some(attachment_before),
            "an ordinary turn System must not force actor rematerialization"
        );
        let stored = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load live-resumed session")
            .expect("live-resumed session remains durable");
        assert_eq!(
            stored
                .messages()
                .iter()
                .filter(|message| {
                    matches!(
                        message,
                        meerkat_core::types::Message::System(message)
                            if message.content == "live turn system"
                    )
                })
                .count(),
            1,
            "live admission must append the explicit System exactly once"
        );
    }

    /// Records the tool choice of every provider call.
    struct ToolChoiceRecordingLlmClient {
        provider: meerkat_core::Provider,
        tool_choices: Arc<std::sync::Mutex<Vec<meerkat_core::ToolChoice>>>,
    }

    #[async_trait]
    impl LlmClient for ToolChoiceRecordingLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            self.tool_choices
                .lock()
                .expect("tool choice log lock poisoned")
                .push(request.tool_choice.clone());
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        provider_for_successful_mcp_test_model(&request.model),
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.provider
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A state with one live session whose agent runs on `llm_client`.
    async fn state_with_live_session(
        llm_client: Arc<dyn LlmClient>,
    ) -> (MeerkatMcpState, meerkat::SessionId) {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::default())),
        )
        .await;
        let session = Session::new();
        let session_id = session.id().clone();
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                let build = request.build.get_or_insert_with(Default::default);
                build.external_tools = Some(router_tools);
                build.llm_client_override =
                    Some(meerkat::encode_llm_client_override_for_service(llm_client));
                (request, router)
            },
        )
        .await;
        (state, session_id)
    }

    /// `meerkat_resume` takes the same per-turn overlay as the other
    /// surfaces: its `tool_choice_plan` reaches the turn's provider call on a
    /// live session instead of being dropped.
    #[tokio::test]
    async fn live_resume_forwards_the_turn_tool_choice_plan() {
        let tool_choices = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (state, session_id) = state_with_live_session(Arc::new(ToolChoiceRecordingLlmClient {
            provider: Provider::Other,
            tool_choices: Arc::clone(&tool_choices),
        }))
        .await;
        let attachment_before = state
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("live fixture has an exact attachment");

        handle_tools_call(
            &state,
            "meerkat_resume",
            &json!({
                "session_id": session_id.to_string(),
                "prompt": "Answer without calling a tool",
                "turn_tool_overlay": { "tool_choice_plan": [{ "mode": "none" }] }
            }),
        )
        .await
        .expect("live resume with a tool choice plan should run");

        assert_eq!(
            state
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await,
            Some(attachment_before),
            "the overlay rides the live turn without an actor rebuild"
        );
        assert_eq!(
            tool_choices
                .lock()
                .expect("tool choice log lock poisoned")
                .first(),
            Some(&meerkat_core::ToolChoice::None),
            "the plan's first entry is the first provider call's tool choice"
        );
    }

    /// A resume that rebuilds the session's agent (here a cold session) keeps
    /// the per-turn overlay: its `tool_choice_plan` reaches the turn's first
    /// provider call.
    #[tokio::test]
    async fn cold_resume_forwards_the_turn_tool_overlay() {
        let tool_choices = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (state, session_id) =
            state_with_persisted_session_and_llm(Arc::new(ToolChoiceRecordingLlmClient {
                provider: Provider::Anthropic,
                tool_choices: Arc::clone(&tool_choices),
            }))
            .await;

        let result = timeout(
            NO_DEADLOCK_BOUND,
            Box::pin(handle_tools_call(
                &state,
                "meerkat_resume",
                &json!({
                    "session_id": session_id,
                    "prompt": "Answer without calling a tool",
                    "turn_tool_overlay": { "tool_choice_plan": [{ "mode": "none" }] }
                }),
            )),
        )
        .await
        .expect("cold resume with an overlay must not deadlock");
        assert!(
            result.is_ok()
                || result.as_ref().is_err_and(|error| {
                    error
                        .message
                        .contains("failed to update peer ingress context")
                }),
            "cold resume must reach its post-completion boundary: {result:?}"
        );
        assert_eq!(
            tool_choices
                .lock()
                .expect("tool choice log lock poisoned")
                .first(),
            Some(&meerkat_core::ToolChoice::None),
            "the rebuilt turn keeps the overlay's tool choice plan"
        );
    }

    /// Unknown overlay keys stay tolerated, as on every other MCP input that
    /// 0.9.0 clients may send: they are ignored (and logged), never
    /// forwarded, and the known keys still apply.
    #[test]
    fn turn_tool_overlay_input_ignores_unknown_keys_and_keeps_known_ones() {
        let input: MeerkatResumeInput = serde_json::from_value(json!({
            "session_id": "unused",
            "prompt": "x",
            "turn_tool_overlay": {
                "allowed_tools": ["shell"],
                "tool_choice_plan": [{ "mode": "required" }],
                "dispatch_context": { "trusted": true },
                "allowed_tool": ["typo"]
            }
        }))
        .expect("unknown overlay keys are tolerated");
        let overlay = meerkat_core::service::TurnToolOverlay::from(
            input.turn_tool_overlay.expect("overlay parsed"),
        );
        assert_eq!(
            overlay.allowed_tools,
            Some(vec![meerkat_core::ToolName::from("shell")])
        );
        assert_eq!(
            overlay.tool_choice_plan,
            vec![meerkat_core::ToolChoice::Required]
        );
        assert!(
            overlay.dispatch_context.is_empty(),
            "host-only dispatch context is never taken from a tool call"
        );
    }

    /// Holds every provider call until the test releases it.
    struct GatedLlmClient {
        calls: Arc<tokio::sync::watch::Sender<usize>>,
        release: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait]
    impl LlmClient for GatedLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            self.calls.send_modify(|calls| *calls += 1);
            let release = Arc::clone(&self.release);
            let usage = meerkat_core::TurnUsage::host_declared(
                provider_for_successful_mcp_test_model(&request.model),
                &request.model,
                meerkat_core::Usage::default(),
            );
            Box::pin(stream::unfold(0u8, move |step| {
                let release = Arc::clone(&release);
                let usage = usage.clone();
                async move {
                    match step {
                        0 => {
                            release
                                .acquire()
                                .await
                                .expect("release semaphore stays open")
                                .forget();
                            Some((
                                Ok(LlmEvent::TextDelta {
                                    delta: "ok".to_string(),
                                    meta: None,
                                }),
                                1,
                            ))
                        }
                        1 => Some((Ok(LlmEvent::UsageUpdate { usage }), 2)),
                        2 => Some((
                            Ok(LlmEvent::Done {
                                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                            }),
                            3,
                        )),
                        _ => None,
                    }
                }
            }))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A `meerkat_resume` that arrives while another turn on the same live
    /// session is running waits behind it and then runs; it is not refused
    /// as busy.
    #[tokio::test]
    async fn resume_while_a_turn_runs_waits_instead_of_busy() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        let calls = Arc::new(tokio::sync::watch::channel(0usize).0);
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let (state, session_id) = state_with_live_session(Arc::new(GatedLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
        }))
        .await;
        let state = Arc::new(state);
        let resume = |prompt: &'static str| {
            let state = Arc::clone(&state);
            let session_id = session_id.to_string();
            tokio::spawn(async move {
                Box::pin(handle_tools_call(
                    &state,
                    "meerkat_resume",
                    &json!({ "session_id": session_id, "prompt": prompt }),
                ))
                .await
            })
        };
        let mut seen = calls.subscribe();

        let first = resume("first running turn");
        timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 1))
            .await
            .expect("the first resume reaches the provider")
            .expect("call counter stays open");
        let mut second = resume("second resume");
        assert!(
            timeout(Duration::from_millis(300), &mut second)
                .await
                .is_err(),
            "a resume on a running session waits behind the running turn \
             instead of answering at once"
        );

        release.add_permits(1);
        let first = timeout(FAILURE_BOUND, first)
            .await
            .expect("the first resume finishes once released")
            .expect("first resume task");
        assert!(first.is_ok(), "first resume: {:?}", first.err());
        timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 2))
            .await
            .expect("the waiting resume runs its turn after the first one")
            .expect("call counter stays open");
        release.add_permits(1);
        let second = timeout(FAILURE_BOUND, second)
            .await
            .expect("the waiting resume finishes once released")
            .expect("second resume task");
        assert!(
            second.is_ok(),
            "the waiting resume returns its own result: {:?}",
            second.err()
        );
    }

    /// A `meerkat_resume` that names the provider-neutral reasoning effort
    /// level while a turn is running on its session is answered busy at
    /// once, with the kind JSON-RPC answers such a turn with as the tool
    /// error's data: it does not wait, and nothing is retired or stored.
    /// The running turn and a plain resume sent after the refused one both
    /// run (a level resume that waited ended the resume queued behind it).
    /// Sent again once the session is idle, the resume reaches the rebuild,
    /// which judges the level: the test client's route states no levels, so
    /// it answers the typed refusal, and the next plain resume runs. Both
    /// forms of the level: the member and the key inside `provider_params`.
    #[tokio::test]
    async fn resume_that_names_a_level_while_a_turn_runs_is_answered_busy_at_once() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        for (form, level) in [
            ("direct", json!({"reasoning_effort": "high"})),
            (
                "nested",
                json!({"provider_params": {"reasoning_effort": "high"}}),
            ),
            ("clear", json!({"reasoning_effort": {"action": "clear"}})),
        ] {
            let calls = Arc::new(tokio::sync::watch::channel(0usize).0);
            let release = Arc::new(tokio::sync::Semaphore::new(0));
            let (state, session_id) = state_with_live_session(Arc::new(GatedLlmClient {
                calls: Arc::clone(&calls),
                release: Arc::clone(&release),
            }))
            .await;
            let state = Arc::new(state);
            let resume = |input: Value| {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    Box::pin(handle_tools_call(&state, "meerkat_resume", &input)).await
                })
            };
            let mut level_input =
                json!({ "session_id": session_id.to_string(), "prompt": "level turn" });
            for (member, value) in level.as_object().expect("a level form is an object") {
                level_input[member.as_str()] = value.clone();
            }
            let attachment_before = state
                .runtime_adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .expect("the live fixture has an exact attachment");
            let mut seen = calls.subscribe();

            let running = resume(
                json!({ "session_id": session_id.to_string(), "prompt": "first running turn" }),
            );
            timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 1))
                .await
                .expect("the first resume reaches the provider")
                .expect("call counter stays open");

            // Answered while the first turn is still held at the provider.
            let refused = timeout(FAILURE_BOUND, resume(level_input.clone()))
                .await
                .unwrap_or_else(|_| {
                    panic!("{form}: a level resume on a busy session answers at once")
                })
                .expect("level resume task")
                .expect_err("a level resume while a turn runs is refused");
            assert!(!running.is_finished(), "{form}: the first turn still runs");
            assert_eq!(refused.code, -32002, "{form}: {}", refused.message);
            assert_eq!(
                refused.data,
                Some(json!({"kind": "reasoning_effort_change_needs_idle_session"})),
                "{form}"
            );
            assert_eq!(
                state
                    .runtime_adapter
                    .current_executor_attachment_witness(&session_id)
                    .await,
                Some(attachment_before),
                "{form}: the refused resume retired nothing"
            );

            // A plain resume sent after the refused one waits and then runs.
            let mut plain =
                resume(json!({ "session_id": session_id.to_string(), "prompt": "plain behind" }));
            assert!(
                timeout(Duration::from_millis(300), &mut plain)
                    .await
                    .is_err(),
                "{form}: a plain resume waits behind the running turn"
            );
            release.add_permits(1);
            let first = timeout(FAILURE_BOUND, running)
                .await
                .expect("the first resume finishes once released")
                .expect("first resume task");
            assert!(first.is_ok(), "{form}: first resume: {:?}", first.err());
            timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 2))
                .await
                .unwrap_or_else(|_| panic!("{form}: the plain resume runs its turn"))
                .expect("call counter stays open");
            release.add_permits(1);
            let plain = timeout(FAILURE_BOUND, plain)
                .await
                .expect("the plain resume finishes once released")
                .expect("plain resume task");
            assert!(plain.is_ok(), "{form}: the plain resume: {:?}", plain.err());

            // The same resume on the idle session reaches the rebuild. A
            // level is judged there and refused typed; a clear names no
            // level to judge and runs.
            release.add_permits(4);
            let retried = timeout(FAILURE_BOUND, resume(level_input))
                .await
                .unwrap_or_else(|_| panic!("{form}: the resume sent again answers"))
                .expect("retried resume task");
            if form == "clear" {
                assert!(
                    retried.is_ok(),
                    "{form}: the retried clear: {:?}",
                    retried.err()
                );
            } else {
                let retried = retried.expect_err("the rebuild judges the level");
                assert_eq!(retried.code, -32602, "{form}: {}", retried.message);
                let data = retried.data.expect("the typed refusal data");
                assert_eq!(data["kind"], "reasoning_effort_refused", "{form}: {data}");
                assert_eq!(
                    data["reasoning_effort_refusal"]["requested"], "high",
                    "{form}: {data}"
                );
            }
            let next = timeout(
                FAILURE_BOUND,
                resume(json!({ "session_id": session_id.to_string(), "prompt": "next plain" })),
            )
            .await
            .unwrap_or_else(|_| panic!("{form}: the next plain resume answers"))
            .expect("next resume task");
            assert!(
                next.is_ok(),
                "{form}: the next plain resume: {:?}",
                next.err()
            );
        }
    }

    /// A `meerkat_resume` that rebuilds the session for another build field
    /// (here `max_tokens`) is unchanged: sent while a turn runs, it is not
    /// answered busy. It waits behind that turn, rebuilds the session and
    /// runs. The rebuild replaces the session's runtime attachment, so a
    /// plain resume queued beside it either runs or ends with the typed
    /// `session_resume_required` answer, depending on which of the two
    /// reaches the session first: both outcomes are pinned, and nothing
    /// else. The session is live afterwards and takes its next resume.
    #[tokio::test]
    async fn resume_that_rebuilds_for_another_field_while_a_turn_runs_waits_and_runs() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        for plain_first in [true, false] {
            let calls = Arc::new(tokio::sync::watch::channel(0usize).0);
            let release = Arc::new(tokio::sync::Semaphore::new(0));
            let (state, session_id) = state_with_live_session(Arc::new(GatedLlmClient {
                calls: Arc::clone(&calls),
                release: Arc::clone(&release),
            }))
            .await;
            let state = Arc::new(state);
            let resume = |input: Value| {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    Box::pin(handle_tools_call(&state, "meerkat_resume", &input)).await
                })
            };
            let rebuild_input = json!({
                "session_id": session_id.to_string(),
                "prompt": "rebuild turn",
                "max_tokens": 2048
            });
            let plain_input =
                json!({ "session_id": session_id.to_string(), "prompt": "plain turn" });
            let mut seen = calls.subscribe();

            let running = resume(
                json!({ "session_id": session_id.to_string(), "prompt": "first running turn" }),
            );
            timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 1))
                .await
                .expect("the first resume reaches the provider")
                .expect("call counter stays open");
            let (mut rebuild, mut plain) = if plain_first {
                let plain = resume(plain_input);
                tokio::time::sleep(Duration::from_millis(200)).await;
                (resume(rebuild_input), plain)
            } else {
                let rebuild = resume(rebuild_input);
                tokio::time::sleep(Duration::from_millis(200)).await;
                (rebuild, resume(plain_input))
            };
            assert!(
                timeout(Duration::from_millis(300), &mut rebuild)
                    .await
                    .is_err(),
                "plain_first={plain_first}: the rebuilding resume waits behind the running turn"
            );
            assert!(
                timeout(Duration::from_millis(300), &mut plain)
                    .await
                    .is_err(),
                "plain_first={plain_first}: the plain resume waits behind the running turn"
            );

            release.add_permits(16);
            for (name, turn) in [("first", running), ("rebuilding", rebuild)] {
                let answer = timeout(FAILURE_BOUND, turn)
                    .await
                    .unwrap_or_else(|_| {
                        panic!("plain_first={plain_first}: the {name} resume answers")
                    })
                    .expect("resume task");
                assert!(
                    answer.is_ok(),
                    "plain_first={plain_first}: the {name} resume: {:?}",
                    answer.err()
                );
            }
            match timeout(FAILURE_BOUND, plain)
                .await
                .unwrap_or_else(|_| panic!("plain_first={plain_first}: the plain resume answers"))
                .expect("resume task")
            {
                Ok(_) => {}
                Err(ended) => {
                    assert_eq!(
                        (
                            ended.code,
                            ended.data.as_ref().map(|data| data["kind"].clone()),
                        ),
                        (-32003, Some(json!("session_resume_required"))),
                        "plain_first={plain_first}: the plain resume: {}",
                        ended.message
                    );
                }
            }
            assert_eq!(
                (
                    state.service.has_live_session(&session_id).await.ok(),
                    state.runtime_adapter.contains_session(&session_id).await,
                ),
                (Some(true), true),
                "plain_first={plain_first}: the session is live and registered"
            );
            let next = timeout(
                FAILURE_BOUND,
                resume(json!({ "session_id": session_id.to_string(), "prompt": "next plain" })),
            )
            .await
            .unwrap_or_else(|_| panic!("plain_first={plain_first}: the next resume answers"))
            .expect("next resume task");
            assert!(
                next.is_ok(),
                "plain_first={plain_first}: the next plain resume: {:?}",
                next.err()
            );
        }
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
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                request
                    .build
                    .get_or_insert_with(Default::default)
                    .external_tools = Some(router_tools);
                (request, router)
            },
        )
        .await;
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
            NO_DEADLOCK_BOUND,
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
            NO_DEADLOCK_BOUND,
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
            NO_DEADLOCK_BOUND,
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
    async fn mcp_add_tool_refuses_oauth_account_selection_without_staging() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::default())),
        )
        .await;
        let session = Session::new();
        let session_id = session.id().clone();
        // A router bound to this session's real external-tool surface, so
        // staging is observable in its snapshot.
        let adapter_slot = Arc::new(std::sync::Mutex::new(None));
        let slot = Arc::clone(&adapter_slot);
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                *slot.lock().expect("slot") = Some(Arc::clone(&router));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                request
                    .build
                    .get_or_insert_with(Default::default)
                    .external_tools = Some(router_tools);
                (request, router)
            },
        )
        .await;
        let adapter = adapter_slot
            .lock()
            .expect("slot")
            .clone()
            .expect("the fixture built the router");
        let snapshot = || {
            AgentToolDispatcher::external_tool_surface_snapshot(adapter.as_ref())
                .expect("a surface-bound router exposes its snapshot")
        };
        let before = snapshot();
        let session_id = session_id.to_string();
        for selection in [
            serde_json::json!({"oauth_account_selection": "unverified"}),
            serde_json::json!({"oauth_account_selection": "discover"}),
            serde_json::json!({"oauth_account": "provider-subject-7"}),
            serde_json::json!({"account": "none"}),
        ] {
            let mut server_config = serde_json::json!({
                "name": "selected",
                "url": "https://mcp.example.invalid/mcp",
            });
            for (key, value) in selection.as_object().expect("object") {
                server_config[key] = value.clone();
            }
            let error = handle_tools_call(
                &state,
                "meerkat_mcp_add",
                &serde_json::json!({"session_id": session_id, "server_config": server_config}),
            )
            .await
            .expect_err("an agent-reachable tool cannot select an account mode");
            assert_eq!(error.code, -32602, "{}", error.message);
            assert!(
                error.message.contains("host configuration"),
                "{}",
                error.message
            );
            // Nothing was staged or written: the whole surface is unchanged.
            assert_eq!(snapshot(), before, "a refusal must not change the surface");
        }
        // Paired control: the same server without a selection is staged.
        handle_tools_call(
            &state,
            "meerkat_mcp_add",
            &serde_json::json!({
                "session_id": session_id,
                "server_config": {"name": "plain", "url": "https://mcp.example.invalid/mcp"},
            }),
        )
        .await
        .expect("a plain server passes the guard and is staged");
        let after = snapshot();
        let staged: Vec<_> = after
            .entries
            .iter()
            .filter(|entry| entry.staged_op == meerkat_core::ExternalToolSurfaceStagedOp::Add)
            .map(|entry| entry.surface_id.as_str())
            .collect();
        assert_eq!(staged, ["plain"]);
        assert!(
            after
                .entries
                .iter()
                .all(|entry| entry.surface_id != "selected"),
            "the refused server never reached the surface"
        );
    }

    #[tokio::test]
    async fn archived_resume_cleanup_does_not_reenter_registration_lock() {
        let (state, session_id) = state_with_persisted_session().await;
        let session_id = meerkat::SessionId::parse(&session_id).expect("valid session id");
        attach_test_mcp_router(
            &state,
            &session_id,
            Arc::new(meerkat_mcp::McpRouterAdapter::new(
                McpRouter::new_with_stdio_profile(
                    meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                ),
            )),
        )
        .await;

        tokio::time::timeout(
            NO_DEADLOCK_BOUND,
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
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                auth_binding: None,
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
                durable: false,
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
    /// `mob_id` must be unique per calling test: the mob supervisor binds
    /// `<mob_id>/__mob_supervisor__` in the process-global inproc registry,
    /// and two tests creating the same mob in parallel fail with
    /// `ParticipantNameOccupied` (2026-09-22 native unit lane).
    async fn insert_mcp_archive_live_member(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
        mob_id: &str,
    ) -> (meerkat_mob::MobId, meerkat::SessionId) {
        let mob_id = meerkat_mob::MobId::from(mob_id);
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                auth_binding: None,
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
                durable: false,
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
        serde_json::from_str::<HookRunOverrides>(
            meerkat_core::config::HOOK_RUN_OVERRIDES_FIXTURE_JSON,
        )
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
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
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
        assert_eq!(receipt["delivery"], serde_json::json!("queued"));
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

    /// Realm MCP servers are literal, judged entry by entry by the store, so
    /// the commit prevalidator does not veto a document that keeps a legacy
    /// entry. The store's refusal of the caller's own new or changed entry,
    /// and its validation refusal, are invalid params; document and store
    /// failures stay internal.
    #[test]
    fn test_realm_mcp_env_references_are_refused_by_the_store_as_invalid_params() {
        let mut config = Config::default();
        config.tools.mcp_servers = vec![meerkat_core::McpServerConfig::streamable_http(
            "legacy",
            "https://mcp.example.com/${HOST_SECRET}",
            std::collections::HashMap::new(),
        )];
        validate_config_for_commit(&config)
            .expect("the prevalidator leaves literal-server checks to the store");

        let refused = map_config_runtime_error(ConfigRuntimeError::Config(
            meerkat_core::ConfigError::RealmMcpServerEnvReference(
                meerkat_core::mcp_config::McpRealmServerEnvReference {
                    server: "exfil".to_string(),
                    field: "url",
                },
            ),
        ));
        assert_eq!(refused.code, -32602);
        assert!(
            refused.message.contains("'exfil'")
                && refused
                    .message
                    .contains("never expanded from the environment"),
            "{}",
            refused.message
        );
        let invalid = map_config_runtime_error(ConfigRuntimeError::Config(
            meerkat_core::ConfigError::Validation("max_tokens".to_string()),
        ));
        assert_eq!(invalid.code, -32602);
        let io = map_config_runtime_error(ConfigRuntimeError::Io(std::io::Error::other("disk")));
        assert_eq!(io.code, -32603);
    }

    /// A config write an admission fence refused keeps its typed code on the
    /// MCP config tool (-32038 held, -32039 unusable), never an internal
    /// error.
    #[test]
    fn test_config_admission_refusals_keep_their_codes() {
        let held = map_config_runtime_error(ConfigRuntimeError::AdmissionInProgress);
        assert_eq!(held.code, -32038, "{}", held.message);
        assert_eq!(held.data, Some(json!({ "type": "admission_in_progress" })));
        let unusable = map_config_runtime_error(ConfigRuntimeError::AdmissionFenceUnavailable(
            "unopenable".to_string(),
        ));
        assert_eq!(unusable.code, -32039, "{}", unusable.message);
        assert_eq!(
            unusable.data,
            Some(json!({ "type": "admission_fence_unavailable" }))
        );
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

    /// The committed config generation, read through the `meerkat_config` tool.
    async fn config_generation(state: &MeerkatMcpState) -> u64 {
        let payload = Box::pin(handle_tools_call(
            state,
            "meerkat_config",
            &json!({"action": "get"}),
        ))
        .await
        .expect("config get");
        let text = payload["content"][0]["text"]
            .as_str()
            .expect("config payload text");
        serde_json::from_str::<Value>(text).expect("config payload json")["generation"]
            .as_u64()
            .expect("config generation")
    }

    /// A `meerkat_config` write can administer neither the host's
    /// `[confinement]` nor its governance: invalid params carrying each
    /// section's own typed refusal, and nothing is written.
    #[tokio::test]
    async fn test_config_writes_cannot_administer_host_sections() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let generation = config_generation(&state).await;
        for (patch, discriminator) in [
            (
                json!({"confinement": {"all": {"mode": "workspace"}}}),
                "confinement_administration_required",
            ),
            (
                json!({"governance": {}}),
                "governance_administration_required",
            ),
        ] {
            let refused = Box::pin(handle_tools_call(
                &state,
                "meerkat_config",
                &json!({"action": "patch", "patch": patch}),
            ))
            .await
            .expect_err("a config write cannot administer a host section");
            assert_eq!(refused.code, -32602, "{patch}");
            assert_eq!(
                refused.data,
                Some(json!({"type": discriminator})),
                "{patch}"
            );
        }
        assert_eq!(
            config_generation(&state).await,
            generation,
            "a refused write commits nothing"
        );
    }

    /// The realm's committed configuration document, as the config runtime
    /// answers it.
    async fn committed_config_document(state: &MeerkatMcpState) -> Value {
        let snapshot = state
            .config_runtime
            .get()
            .await
            .expect("the committed configuration");
        serde_json::to_value(snapshot.config).expect("config value")
    }

    const FIRST_SOURCE: &str = "11111111-1111-4111-8111-111111111111";
    const SECOND_SOURCE: &str = "22222222-2222-4222-8222-222222222222";
    const THIRD_SOURCE: &str = "33333333-3333-4333-8333-333333333333";

    fn stdio_server_entry(name: &str, command: &str, args: &[&str]) -> Value {
        json!({"name": name, "command": command, "args": args})
    }

    fn http_server_entry(name: &str) -> Value {
        json!({"name": name, "url": "http://synthetic.invalid/mcp"})
    }

    fn hook_entry(id: &str, runtime: Value) -> Value {
        json!({
            "id": id,
            "enabled": true,
            "point": "run_started",
            "mode": "foreground",
            "capability": "observe",
            "priority": 0,
            "runtime": runtime,
        })
    }

    fn command_hook_entry(id: &str, command: &str) -> Value {
        hook_entry(
            id,
            json!({"type": "command", "command": command, "args": []}),
        )
    }

    fn stdio_skills_entry(name: &str, source: &str, command: &str) -> Value {
        json!({"name": name, "source_uuid": source, "type": "stdio", "command": command})
    }

    fn git_skills_entry(name: &str, source: &str, url: &str, git_ref: &str) -> Value {
        json!({"name": name, "source_uuid": source, "type": "git", "url": url, "git_ref": git_ref})
    }

    /// The entries the state below is seeded with.
    fn seeded_server() -> Value {
        stdio_server_entry("existing-server", "/synthetic/server", &["serve"])
    }

    fn seeded_hook() -> Value {
        command_hook_entry("existing-hook", "/synthetic/hook")
    }

    fn seeded_stdio_skills() -> Value {
        stdio_skills_entry("existing-stdio-skills", FIRST_SOURCE, "/synthetic/skills")
    }

    fn seeded_git_skills() -> Value {
        git_skills_entry(
            "existing-git-skills",
            SECOND_SOURCE,
            "https://synthetic.invalid/skills.git",
            "main",
        )
    }

    /// A state whose realm document already holds one entry of each kind
    /// that launches a process, and a shell program, all written by the
    /// host's own writer.
    async fn state_with_launching_entries() -> MeerkatMcpState {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        state
            .config_runtime
            .patch(
                ConfigDelta(json!({
                    "tools": {"mcp_servers": [seeded_server()]},
                    "hooks": {"entries": [seeded_hook()]},
                    "skills": {"repositories": [seeded_stdio_skills(), seeded_git_skills()]},
                    "shell": {"program": "/synthetic/shell"},
                })),
                None,
            )
            .await
            .expect("the host's own writer adds the entries");
        state
    }

    /// A merge patch that sets the value at `pointer` (a `/section/key`
    /// path) and nothing else.
    fn patch_at(pointer: &str, value: Value) -> Value {
        pointer
            .rsplit('/')
            .filter(|key| !key.is_empty())
            .fold(value, |inner, key| json!({ key: inner }))
    }

    /// Each case of `cases` on a state of its own: the config tool must
    /// refuse the write as host administration and leave the committed
    /// document and its generation as they were. Every case that is not so
    /// is reported, not only the first.
    async fn assert_launching_writes_refused(cases: Vec<(String, Value)>) {
        assert_writes_refused_after(None, cases).await;
    }

    /// A state like the one above whose document the host's own writer has
    /// then patched with `host_patch`.
    async fn state_after_host_patch(host_patch: Option<&Value>) -> MeerkatMcpState {
        let state = state_with_launching_entries().await;
        if let Some(patch) = host_patch {
            state
                .config_runtime
                .patch(ConfigDelta(patch.clone()), None)
                .await
                .expect("the host's own writer patches the document");
        }
        state
    }

    /// As `assert_launching_writes_refused`, each case on a state the host's
    /// own writer has patched with `host_patch` first.
    async fn assert_writes_refused_after(host_patch: Option<&Value>, cases: Vec<(String, Value)>) {
        let mut not_refused = Vec::new();
        for (what, arguments) in cases {
            let state = state_after_host_patch(host_patch).await;
            let generation = config_generation(&state).await;
            let document = committed_config_document(&state).await;
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => not_refused.push(format!("{what}: the write was accepted")),
                Err(refused) => {
                    if refused.code != -32602
                        || refused.data
                            != Some(json!({"type": "confinement_administration_required"}))
                    {
                        not_refused.push(format!(
                            "{what}: refused otherwise: {} {} {:?}",
                            refused.code, refused.message, refused.data
                        ));
                    }
                    assert_eq!(config_generation(&state).await, generation, "{what}");
                    assert_eq!(committed_config_document(&state).await, document, "{what}");
                }
            }
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// The writes that add `added` to the list at `pointer`: a patch that
    /// restates the list with it, and a set of the whole document that
    /// patch would write (the complete typed document a set requires).
    async fn writes_that_add(what: &str, pointer: &str, added: Value) -> Vec<(String, Value)> {
        let state = state_with_launching_entries().await;
        let document = committed_config_document(&state).await;
        let mut entries = document
            .pointer(pointer)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("the document's {pointer}"))
            .clone();
        entries.push(added);
        let patch = patch_at(pointer, json!(entries));
        let replaced = state
            .config_runtime
            .patch_preview(&ConfigDelta(patch.clone()))
            .await
            .expect("the document the patch would write");
        vec![
            (
                format!("a patch adds {what}"),
                json!({"action": "patch", "patch": patch}),
            ),
            (
                format!("a set adds {what}"),
                json!({"action": "set", "config": replaced}),
            ),
        ]
    }

    /// A patch that sets the value at `pointer` to `value`.
    fn patch_case(what: &str, pointer: &str, value: Value) -> (String, Value) {
        (
            format!("a patch changes {what}"),
            json!({"action": "patch", "patch": patch_at(pointer, value)}),
        )
    }

    /// The config tool adds no MCP server with a stdio transport and does
    /// not change what one runs: a kept name with another command, argument
    /// or environment value is a new entry. Refused as host administration,
    /// nothing written.
    #[tokio::test]
    async fn test_config_writes_cannot_add_or_change_a_stdio_mcp_server() {
        let mut cases = writes_that_add(
            "a stdio MCP server",
            "/tools/mcp_servers",
            stdio_server_entry("added-server", "/synthetic/added", &[]),
        )
        .await;
        let mut with_env = seeded_server();
        with_env["env"] = json!({"SYNTHETIC": "1"});
        cases.extend([
            patch_case(
                "a server's command",
                "/tools/mcp_servers",
                json!([stdio_server_entry(
                    "existing-server",
                    "/synthetic/other",
                    &["serve"]
                )]),
            ),
            patch_case(
                "a server's arguments",
                "/tools/mcp_servers",
                json!([stdio_server_entry(
                    "existing-server",
                    "/synthetic/server",
                    &["serve", "--more"]
                )]),
            ),
            patch_case(
                "a server's environment",
                "/tools/mcp_servers",
                json!([with_env]),
            ),
            (
                "a set changes a server's command".to_string(),
                json!({"action": "set", "config": set_document_with(
                    "/tools/mcp_servers/0/command",
                    json!("/synthetic/other"),
                ).await}),
            ),
        ]);
        assert_launching_writes_refused(cases).await;

        // A kept name whose transport goes from HTTP to stdio is a new entry.
        let with_http_server = json!({"tools": {"mcp_servers": [
            seeded_server(),
            http_server_entry("existing-http-server"),
        ]}});
        assert_writes_refused_after(
            Some(&with_http_server),
            vec![patch_case(
                "an HTTP server into a stdio server of the same name",
                "/tools/mcp_servers",
                json!([
                    seeded_server(),
                    stdio_server_entry("existing-http-server", "/synthetic/added", &[]),
                ]),
            )],
        )
        .await;
    }

    /// The committed document of the seeded state with the value at
    /// `pointer` (which must exist) replaced: the complete typed document a
    /// set requires, changed in one place.
    async fn set_document_with(pointer: &str, value: Value) -> Value {
        let mut document = committed_config_document(&state_with_launching_entries().await).await;
        *document
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("the document's {pointer}")) = value;
        document
    }

    /// The config tool adds no hook with a command runtime and does not
    /// change the command of one.
    #[tokio::test]
    async fn test_config_writes_cannot_add_or_change_a_command_hook() {
        let mut cases = writes_that_add(
            "a command hook",
            "/hooks/entries",
            command_hook_entry("added-hook", "/synthetic/added"),
        )
        .await;
        cases.push(patch_case(
            "a hook's command",
            "/hooks/entries",
            json!([command_hook_entry("existing-hook", "/synthetic/other")]),
        ));
        cases.push((
            "a set changes a hook's command".to_string(),
            json!({"action": "set", "config": set_document_with(
                "/hooks/entries/0/runtime/command",
                json!("/synthetic/other"),
            ).await}),
        ));
        assert_launching_writes_refused(cases).await;
    }

    /// The config tool adds no skills repository with a stdio transport and
    /// does not change the command or the working directory of one.
    #[tokio::test]
    async fn test_config_writes_cannot_add_or_change_a_stdio_skills_repository() {
        let mut cases = writes_that_add(
            "a stdio skills repository",
            "/skills/repositories",
            stdio_skills_entry("added-stdio-skills", THIRD_SOURCE, "/synthetic/added"),
        )
        .await;
        let mut with_cwd = seeded_stdio_skills();
        with_cwd["cwd"] = json!("/synthetic/elsewhere");
        cases.extend([
            patch_case(
                "a stdio skills repository's command",
                "/skills/repositories",
                json!([
                    stdio_skills_entry("existing-stdio-skills", FIRST_SOURCE, "/synthetic/other"),
                    seeded_git_skills(),
                ]),
            ),
            patch_case(
                "a stdio skills repository's working directory",
                "/skills/repositories",
                json!([with_cwd, seeded_git_skills()]),
            ),
        ]);
        assert_launching_writes_refused(cases).await;
    }

    /// The config tool adds no skills repository with a git transport and
    /// does not change the URL, the ref or the key path of one.
    #[tokio::test]
    async fn test_config_writes_cannot_add_or_change_a_git_skills_repository() {
        let mut cases = writes_that_add(
            "a git skills repository",
            "/skills/repositories",
            git_skills_entry(
                "added-git-skills",
                THIRD_SOURCE,
                "https://synthetic.invalid/added.git",
                "main",
            ),
        )
        .await;
        let mut with_key = seeded_git_skills();
        with_key["auth"] = json!({"scheme": "ssh-key", "path": "/synthetic/key"});
        cases.extend([
            patch_case(
                "a git skills repository's URL",
                "/skills/repositories",
                json!([
                    seeded_stdio_skills(),
                    git_skills_entry(
                        "existing-git-skills",
                        SECOND_SOURCE,
                        "https://synthetic.invalid/other.git",
                        "main",
                    ),
                ]),
            ),
            patch_case(
                "a git skills repository's ref",
                "/skills/repositories",
                json!([
                    seeded_stdio_skills(),
                    git_skills_entry(
                        "existing-git-skills",
                        SECOND_SOURCE,
                        "https://synthetic.invalid/skills.git",
                        "other",
                    ),
                ]),
            ),
            patch_case(
                "a git skills repository's key path",
                "/skills/repositories",
                json!([seeded_stdio_skills(), with_key]),
            ),
            (
                "a set changes a git skills repository's ref".to_string(),
                json!({"action": "set", "config": set_document_with(
                    "/skills/repositories/1/git_ref",
                    json!("other"),
                ).await}),
            ),
        ]);
        assert_launching_writes_refused(cases).await;
    }

    /// A write that adds a launching entry and is invalid besides is refused
    /// as host administration, not as an invalid document: the refusal comes
    /// before the document is validated.
    #[tokio::test]
    async fn test_config_writes_refuse_a_launching_entry_before_they_validate_the_document() {
        // The writes that add the server (the set's document is the
        // complete typed one), each with a value the validation refuses.
        let mut cases = writes_that_add(
            "a stdio MCP server and an invalid value",
            "/tools/mcp_servers",
            stdio_server_entry("added-server", "/synthetic/added", &[]),
        )
        .await;
        for (_, arguments) in &mut cases {
            let document = if arguments["action"] == "patch" {
                "patch"
            } else {
                "config"
            };
            arguments[document]["max_tokens"] = json!(0);
        }
        assert_launching_writes_refused(cases).await;
    }

    /// A state that serves the realm `team`, whose parent realm `org` holds
    /// `org_sections` in its own document. The directory lives as long as
    /// the state is used.
    async fn state_for_child_realm(org_sections: &str) -> (tempfile::TempDir, MeerkatMcpState) {
        state_for_team_realm("", &[("org", org_sections)]).await
    }

    /// A state that serves the realm `team`, a child of `org`, whose own
    /// document holds `team_sections`; `realms` are the other realms of the
    /// state root (`org` among them), each with the sections of its own
    /// document. The directory lives as long as the state is used.
    async fn state_for_team_realm(
        team_sections: &str,
        realms: &[(&str, &str)],
    ) -> (tempfile::TempDir, MeerkatMcpState) {
        state_for_team_realm_in_context(team_sections, realms, None).await
    }

    /// As `state_for_team_realm`; with `context_file`, the server is given
    /// a context root (`<directory>/context`) whose convention file holds
    /// that text.
    async fn state_for_team_realm_in_context(
        team_sections: &str,
        realms: &[(&str, &str)],
        context_file: Option<&str>,
    ) -> (tempfile::TempDir, MeerkatMcpState) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("realms");
        let mut documents = vec![(
            "team".to_string(),
            format!("[realm.team]\nparent = \"org\"\n\n{team_sections}"),
        )];
        for (realm, sections) in realms {
            documents.push((
                (*realm).to_string(),
                format!("[realm.{realm}]\n\n{sections}"),
            ));
        }
        for (realm, text) in &documents {
            let path = meerkat_store::realm_paths_in(&root, realm).config_path;
            std::fs::create_dir_all(path.parent().expect("realm dir")).expect("realm dir");
            std::fs::write(&path, text).expect("realm document");
        }
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "team".into(),
        };
        bootstrap.realm.state_root = Some(root);
        bootstrap.context.user_config_root = Some(temp.path().join("user"));
        if let Some(text) = context_file {
            let context = temp.path().join("context");
            std::fs::create_dir_all(context.join(".rkat")).expect("context dir");
            std::fs::write(context.join(".rkat").join("config.toml"), text)
                .expect("context convention file");
            bootstrap.context.context_root = Some(context);
        }
        let state = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
        .expect("the team realm serves");
        (temp, state)
    }

    /// The sections of a realm document that holds one stdio MCP server.
    fn stdio_server_sections(name: &str, command: &str) -> String {
        format!("[[tools.mcp_servers]]\nname = \"{name}\"\ncommand = \"{command}\"\n")
    }

    /// The stdio MCP servers the served realm composes along its parent
    /// chain, as `name command`.
    async fn composed_stdio_servers(state: &MeerkatMcpState) -> Vec<String> {
        let head = state
            .config_runtime
            .get()
            .await
            .expect("the realm's own document")
            .config;
        meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
            .effective_config_over_head(&state.realm_id, head)
            .await
            .expect("the realm's chain composes")
            .tools
            .mcp_servers
            .iter()
            .filter_map(|server| match &server.transport {
                meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) => {
                    Some(format!("{} {}", server.name, stdio.command))
                }
                meerkat_core::mcp_config::McpTransportConfig::Http(_) => None,
            })
            .collect()
    }

    /// One write through the config tool on a state of its own, which must
    /// be refused as host administration with nothing written; `composed`
    /// are the stdio servers the realm must compose before and after.
    /// Answers what went otherwise, if anything.
    async fn refused_in_team_realm(
        what: &str,
        team_sections: &str,
        realms: &[(&str, &str)],
        arguments: Value,
        composed: &[&str],
    ) -> Option<String> {
        let (_realms, state) = state_for_team_realm(team_sections, realms).await;
        assert_eq!(composed_stdio_servers(&state).await, composed, "{what}");
        let generation = config_generation(&state).await;
        let document = committed_config_document(&state).await;
        match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
            Ok(_) => Some(format!(
                "{what}: the write was accepted and the realm composes the stdio servers {:?}",
                composed_stdio_servers(&state).await
            )),
            Err(refused) => {
                assert_eq!(config_generation(&state).await, generation, "{what}");
                assert_eq!(committed_config_document(&state).await, document, "{what}");
                assert_eq!(composed_stdio_servers(&state).await, composed, "{what}");
                (refused.code != -32602
                    || refused.data != Some(json!({"type": "confinement_administration_required"})))
                .then(|| {
                    format!(
                        "{what}: refused otherwise: {} {} {:?}",
                        refused.code, refused.message, refused.data
                    )
                })
            }
        }
    }

    /// The config tool does not bring into force, in the realm it serves, a
    /// launching entry the realm does not compose now. A realm composes its
    /// parent chain's entries, so its own document is not the whole answer:
    /// a patch of the realm's parent edge to a realm that holds a stdio
    /// server, and the removal of an own server that shadowed a parent's of
    /// the same name, are refused as host administration, nothing written.
    /// And a copy of a parent's entry is not written into the realm's own
    /// document. Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_bring_a_launching_entry_into_force_through_the_chain() {
        let other = stdio_server_sections("other-server", "/synthetic/other-server");
        let parent = stdio_server_sections("shared", "/synthetic/parent-server");
        let own = stdio_server_sections("shared", "/synthetic/own-server");
        let mut not_refused = Vec::new();
        not_refused.extend(
            refused_in_team_realm(
                "a patch of the parent edge to a realm that holds a stdio server",
                "",
                &[("org", ""), ("other", other.as_str())],
                json!({"action": "patch", "patch": {"realm": {"team": {"parent": "other"}}}}),
                &[],
            )
            .await,
        );
        not_refused.extend(
            refused_in_team_realm(
                "a patch that removes an own server that shadowed a parent's",
                &own,
                &[("org", parent.as_str())],
                json!({"action": "patch", "patch": {"tools": {"mcp_servers": []}}}),
                &["shared /synthetic/own-server"],
            )
            .await,
        );
        not_refused.extend(
            refused_in_team_realm(
                "a patch that copies a parent's server into the realm's own document",
                "",
                &[("org", parent.as_str())],
                json!({"action": "patch", "patch": {"tools": {"mcp_servers": [
                    stdio_server_entry("shared", "/synthetic/parent-server", &[]),
                ]}}}),
                &["shared /synthetic/parent-server"],
            )
            .await,
        );
        // A skills repository composes by its source id: an own entry with
        // a parent's source id takes the parent's place.
        let parent_skills = stdio_skills_sections("/synthetic/parent-skills");
        for (what, entry) in [
            (
                "a patch that gives a stdio skills repository a parent's source id",
                stdio_skills_entry("own-skills", FIRST_SOURCE, "/synthetic/own-skills"),
            ),
            (
                "a patch that gives a git skills repository a parent's source id",
                git_skills_entry(
                    "own-skills",
                    FIRST_SOURCE,
                    "https://synthetic.invalid/own.git",
                    "main",
                ),
            ),
        ] {
            not_refused.extend(
                refused_in_team_realm(
                    what,
                    "",
                    &[("org", parent_skills.as_str())],
                    json!({"action": "patch", "patch": {"skills": {"repositories": [entry]}}}),
                    &[],
                )
                .await,
            );
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// The sections of a realm document that holds one stdio skills
    /// repository, with the first source id.
    fn stdio_skills_sections(command: &str) -> String {
        format!(
            "[[skills.repositories]]\nname = \"parent-skills\"\nsource_uuid = \"{FIRST_SOURCE}\"\n\
             type = \"stdio\"\ncommand = \"{command}\"\n"
        )
    }

    /// An own skills repository that launches nothing and carries a
    /// parent's source id takes the place of the parent's stdio repository:
    /// a removal by effect, which commits; the realm then composes no
    /// skills repository that launches a process.
    #[tokio::test]
    async fn test_config_writes_can_put_a_non_launching_skills_repository_in_a_parents_place() {
        let parent_skills = stdio_skills_sections("/synthetic/parent-skills");
        let (_realms, state) = state_for_team_realm("", &[("org", parent_skills.as_str())]).await;
        let launching = |repositories: &[meerkat_core::skills_config::SkillRepositoryConfig]| {
            repositories
                .iter()
                .filter(|repository| {
                    matches!(
                        repository.transport,
                        meerkat_core::skills_config::SkillRepoTransport::Stdio { .. }
                            | meerkat_core::skills_config::SkillRepoTransport::Git { .. }
                    )
                })
                .count()
        };
        assert_eq!(
            launching(&composed_config(&state).await.skills.repositories),
            1
        );
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"skills": {"repositories": [{
                "name": "own-skills",
                "source_uuid": FIRST_SOURCE,
                "type": "filesystem",
                "path": "/synthetic/skills",
            }]}}}),
        ))
        .await
        .expect("an entry that launches nothing commits in a parent's place");
        assert!(config_generation(&state).await > generation);
        assert_eq!(
            launching(&composed_config(&state).await.skills.repositories),
            0
        );
    }

    /// What the served realm composes along its parent chain: what a
    /// session of the realm is built from.
    async fn composed_config(state: &MeerkatMcpState) -> Config {
        let head = state
            .config_runtime
            .get()
            .await
            .expect("the realm's own document")
            .config;
        meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
            .effective_config_over_head(&state.realm_id, head)
            .await
            .expect("the realm's chain composes")
    }

    /// A patch of the realm's parent edge that brings no launching entry
    /// into force commits: to a realm whose document holds none.
    #[tokio::test]
    async fn test_config_writes_can_change_the_parent_edge_to_a_realm_that_launches_nothing() {
        let (_realms, state) = state_for_team_realm("", &[("org", ""), ("plain", "")]).await;
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"realm": {"team": {"parent": "plain"}}}}),
        ))
        .await
        .expect("a parent edge to a realm that launches nothing commits");
        assert!(config_generation(&state).await > generation);
        assert_eq!(
            committed_config_document(&state).await["realm"]["team"]["parent"],
            json!("plain")
        );
        assert!(composed_stdio_servers(&state).await.is_empty());
    }

    /// A write after which the realm's chain would not compose (here its
    /// own parent edge names the realm itself) is refused and nothing is
    /// written: what the realm would run cannot be compared.
    #[tokio::test]
    async fn test_config_writes_whose_chain_does_not_compose_are_refused() {
        let (_realms, state) = state_for_team_realm("", &[("org", "")]).await;
        let generation = config_generation(&state).await;
        let document = committed_config_document(&state).await;
        let refused = Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"realm": {"team": {"parent": "team"}}}}),
        ))
        .await
        .expect_err("a document whose chain does not compose is refused");
        assert_eq!(refused.code, -32602, "{}", refused.message);
        assert_eq!(config_generation(&state).await, generation);
        assert_eq!(committed_config_document(&state).await, document);
    }

    /// A write is refused while the realm's committed chain does not
    /// compose (here the parent's document no longer parses): what the
    /// realm runs cannot be compared, so nothing is written, also for a key
    /// that launches nothing.
    #[tokio::test]
    async fn test_config_writes_are_refused_while_the_realm_chain_does_not_compose() {
        let (realms, state) = state_for_team_realm("", &[("org", "")]).await;
        let generation = config_generation(&state).await;
        let document = committed_config_document(&state).await;
        let org = meerkat_store::realm_paths_in(&realms.path().join("realms"), "org").config_path;
        std::fs::write(&org, "this is [not a realm document").expect("the parent's document");
        let refused = Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"max_tokens": 2048}}),
        ))
        .await
        .expect_err("a write is refused while the chain does not compose");
        assert_eq!(refused.code, -32603, "{}", refused.message);
        assert_eq!(config_generation(&state).await, generation);
        assert_eq!(committed_config_document(&state).await, document);
    }

    /// The sections of a realm document, or of a convention file, that hold
    /// one hook of `id` with `runtime` (an inline table).
    fn hook_sections(id: &str, runtime: &str) -> String {
        format!(
            "[[hooks.entries]]\nid = \"{id}\"\nenabled = true\npoint = \"run_started\"\n\
             mode = \"foreground\"\ncapability = \"observe\"\npriority = 0\nruntime = {runtime}\n"
        )
    }

    const HTTP_HOOK_RUNTIME: &str = "{ type = \"http\", url = \"http://synthetic.invalid/hook\" }";

    fn command_hook_runtime(command: &str) -> String {
        format!("{{ type = \"command\", command = \"{command}\" }}")
    }

    /// The commands of the command hooks a session of the served realm
    /// runs: the realm's composed hooks as the agent factory layers them
    /// (the first entry of an id, then the context root's convention file,
    /// then the user root's).
    async fn session_hook_commands(
        realms: &tempfile::TempDir,
        state: &MeerkatMcpState,
    ) -> Vec<String> {
        let context = realms.path().join("context");
        let user = realms.path().join("user");
        let composed = composed_config(state).await;
        meerkat::resolve_layered_hooks_config(
            context.is_dir().then_some(context.as_path()),
            Some(user.as_path()),
            &composed,
        )
        .await
        .entries
        .iter()
        .filter_map(|entry| match &entry.runtime {
            meerkat_core::config::HookAdapterConfig::Command(command) => {
                Some(command.command.clone())
            }
            meerkat_core::config::HookAdapterConfig::InProcess(_)
            | meerkat_core::config::HookAdapterConfig::Http(_) => None,
        })
        .collect()
    }

    /// A session runs the first hook of an id: the realm's composed entries
    /// in order (a parent's before its own), then the context root's
    /// convention file, then the user root's. So a command hook can be
    /// held by the realm, or by a file, and hidden by an earlier entry of
    /// its id; the config tool does not bring it into force: a parent edge
    /// away from the parent whose entry hid it, and the removal of the own
    /// entry that hid a file's, are refused as host administration, nothing
    /// written, and a session runs no command hook as before. Each case on
    /// a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_bring_a_hidden_command_hook_into_force() {
        let hiding = hook_sections("shared-hook", HTTP_HOOK_RUNTIME);
        let hidden = hook_sections(
            "shared-hook",
            &command_hook_runtime("/synthetic/hidden-hook"),
        );
        let mut not_refused = Vec::new();
        for (what, team_sections, realms, context_file, patch) in [
            (
                "a patch of the parent edge away from a parent whose hook hid an own command hook",
                hidden.as_str(),
                vec![("org", hiding.as_str()), ("plain", "")],
                None,
                json!({"realm": {"team": {"parent": "plain"}}}),
            ),
            (
                "a patch that removes an own hook that hid a command hook of the context file",
                hiding.as_str(),
                vec![("org", "")],
                Some(hidden.as_str()),
                json!({"hooks": {"entries": []}}),
            ),
        ] {
            let (directory, state) =
                state_for_team_realm_in_context(team_sections, &realms, context_file).await;
            assert!(
                session_hook_commands(&directory, &state).await.is_empty(),
                "{what}: the command hook is hidden"
            );
            let generation = config_generation(&state).await;
            let document = committed_config_document(&state).await;
            let arguments = json!({"action": "patch", "patch": patch});
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => not_refused.push(format!(
                    "{what}: the write was accepted and a session runs the command hooks {:?}",
                    session_hook_commands(&directory, &state).await
                )),
                Err(refused) => {
                    if refused.code != -32602
                        || refused.data
                            != Some(json!({"type": "confinement_administration_required"}))
                    {
                        not_refused.push(format!(
                            "{what}: refused otherwise: {} {} {:?}",
                            refused.code, refused.message, refused.data
                        ));
                    }
                    assert_eq!(config_generation(&state).await, generation, "{what}");
                    assert_eq!(committed_config_document(&state).await, document, "{what}");
                    assert!(
                        session_hook_commands(&directory, &state).await.is_empty(),
                        "{what}"
                    );
                }
            }
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// In one document that holds two hooks of one id a session runs the
    /// first: the config tool does not remove the first, nor put the second
    /// before it, when the second is a command hook.
    #[tokio::test]
    async fn test_config_writes_cannot_unhide_the_second_of_two_hooks_of_one_id() {
        let hiding = hook_entry(
            "twice",
            json!({"type": "http", "url": "http://synthetic.invalid/hook"}),
        );
        let hidden = command_hook_entry("twice", "/synthetic/second");
        let host_patch = json!({"hooks": {"entries": [
            seeded_hook(),
            hiding.clone(),
            hidden.clone(),
        ]}});
        assert_writes_refused_after(
            Some(&host_patch),
            vec![
                patch_case(
                    "the hooks, with the first of two of an id removed",
                    "/hooks/entries",
                    json!([seeded_hook(), hidden.clone()]),
                ),
                patch_case(
                    "the hooks, with the two of an id in the other order",
                    "/hooks/entries",
                    json!([seeded_hook(), hidden, hiding]),
                ),
            ],
        )
        .await;
    }

    /// The removal of a hook that hides nothing commits, also where the
    /// context root's convention file holds a command hook of another id:
    /// a session runs that one before and after.
    #[tokio::test]
    async fn test_config_writes_can_remove_a_hook_that_hides_nothing() {
        let own = hook_sections("own-hook", HTTP_HOOK_RUNTIME);
        let file = hook_sections("file-hook", &command_hook_runtime("/synthetic/file-hook"));
        let (directory, state) =
            state_for_team_realm_in_context(&own, &[("org", "")], Some(&file)).await;
        assert_eq!(
            session_hook_commands(&directory, &state).await,
            ["/synthetic/file-hook"]
        );
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"hooks": {"entries": []}}}),
        ))
        .await
        .expect("the removal of a hook that hides nothing commits");
        assert!(config_generation(&state).await > generation);
        assert_eq!(
            session_hook_commands(&directory, &state).await,
            ["/synthetic/file-hook"]
        );
    }

    /// The sections of the document of `realm` that state one provider
    /// route: a backend with an address, an auth profile that reads a
    /// variable (no row resolves it: no row here starts a run), and a
    /// binding of the two.
    fn route_sections(realm: &str) -> String {
        format!(
            "[realm.{realm}.backend.main]\nprovider = \"openai\"\nbackend_kind = \"openai_api\"\n\
             base_url = \"http://synthetic.invalid/v1\"\n\n\
             [realm.{realm}.auth.main]\nprovider = \"openai\"\nauth_method = \"api_key\"\n\
             source = {{ kind = \"env\", env = \"MEERKAT_CONFIG_TOOL_ROW_VALUE\" }}\n\n\
             [realm.{realm}.binding.main]\nbackend_profile = \"main\"\nauth_profile = \"main\"\n"
        )
    }

    /// A patch through the tool that sets `value` at `pointer` under the
    /// served realm's own section.
    fn team_section_patch(pointer: &str, value: Value) -> Value {
        json!({"action": "patch", "patch": {"realm": {"team": patch_at(pointer, value)}}})
    }

    /// Where a credential the host supplies is read from, and where it is
    /// sent, is the host's to administer. In a realm that states a provider
    /// route the config tool does not change the address of the backend a
    /// binding uses, the source of its auth profile, the binding, or the
    /// realm's default binding, does not add a route beside it, a Copilot
    /// account, a connector client or a self-hosted server, and does not
    /// take a parent edge to a realm that states a route. Refused as host
    /// administration, nothing written. Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_change_where_a_host_credential_goes() {
        let own = route_sections("team");
        let other = route_sections("other");
        let realms = [("org", ""), ("other", other.as_str())];
        let set_arguments = {
            let (_directory, state) = state_for_team_realm(&own, &realms).await;
            let mut document = committed_config_document(&state).await;
            document["realm"]["team"]["backend"]["main"]["base_url"] =
                json!("http://other.synthetic.invalid/v1");
            json!({"action": "set", "config": document})
        };
        let mut not_refused = Vec::new();
        for (what, arguments) in [
            (
                "a patch of the address of a backend a binding uses",
                team_section_patch(
                    "/backend/main/base_url",
                    json!("http://other.synthetic.invalid/v1"),
                ),
            ),
            (
                "a set of the address of a backend a binding uses",
                set_arguments,
            ),
            (
                "a patch of the source of an auth profile",
                team_section_patch(
                    "/auth/main/source",
                    json!({"kind": "env", "env": "MEERKAT_CONFIG_TOOL_OTHER_VALUE"}),
                ),
            ),
            (
                "a patch that points the binding at another backend",
                json!({"action": "patch", "patch": {"realm": {"team": {
                    "backend": {"second": {"provider": "openai", "backend_kind": "openai_api"}},
                    "binding": {"main": {"backend_profile": "second"}},
                }}}}),
            ),
            (
                "a patch that adds a binding",
                team_section_patch(
                    "/binding/second",
                    json!({"backend_profile": "main", "auth_profile": "main"}),
                ),
            ),
            (
                "a patch that adds an auth profile",
                team_section_patch(
                    "/auth/second",
                    json!({
                        "provider": "openai",
                        "auth_method": "api_key",
                        "source": {"kind": "env", "env": "MEERKAT_CONFIG_TOOL_OTHER_VALUE"},
                    }),
                ),
            ),
            (
                "a patch that adds a backend",
                team_section_patch(
                    "/backend/second",
                    json!({
                        "provider": "openai",
                        "backend_kind": "openai_api",
                        "base_url": "http://other.synthetic.invalid/v1",
                    }),
                ),
            ),
            (
                "a patch that sets the realm's default binding",
                team_section_patch("/default_binding", json!("main")),
            ),
            (
                "a patch that sets a Copilot account",
                team_section_patch(
                    "/copilot",
                    json!({"source": {"kind": "env", "env": "MEERKAT_CONFIG_TOOL_OTHER_VALUE"}}),
                ),
            ),
            (
                "a patch that adds a connector client",
                team_section_patch(
                    "/connector_client/second",
                    json!({"issuer": "https://synthetic.invalid", "client": "synthetic-client"}),
                ),
            ),
            (
                "a patch that adds a self-hosted server",
                json!({"action": "patch", "patch": {"self_hosted": {"servers": {"box": {
                    "base_url": "http://other.synthetic.invalid/v1",
                }}}}}),
            ),
            (
                "a patch of the parent edge to a realm that states a route",
                team_section_patch("/parent", json!("other")),
            ),
        ] {
            not_refused.extend(refused_in_team_realm(what, &own, &realms, arguments, &[]).await);
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// What the config tool still takes in a realm that states a provider
    /// route: a patch of another key, a patch that restates the route, the
    /// removal of the realm's default binding, and the removal of the
    /// binding, its auth profile and its backend. Each case on a state of
    /// its own.
    #[tokio::test]
    async fn test_config_writes_that_keep_or_remove_a_provider_route_still_commit() {
        let own = format!("default_binding = \"main\"\n\n{}", route_sections("team"));
        let mut not_committed = Vec::new();
        for (what, arguments) in [
            (
                "a patch of another key",
                json!({"action": "patch", "patch": {"max_tokens": 2048}}),
            ),
            (
                "a patch that restates the route",
                json!({"action": "patch", "patch": {"realm": {"team": {
                    "backend": {"main": {
                        "provider": "openai",
                        "backend_kind": "openai_api",
                        "base_url": "http://synthetic.invalid/v1",
                    }},
                    "auth": {"main": {
                        "provider": "openai",
                        "auth_method": "api_key",
                        "source": {"kind": "env", "env": "MEERKAT_CONFIG_TOOL_ROW_VALUE"},
                    }},
                    "binding": {"main": {"backend_profile": "main", "auth_profile": "main"}},
                    "default_binding": "main",
                }}}}),
            ),
            (
                "a patch that removes the default binding",
                team_section_patch("/default_binding", Value::Null),
            ),
            (
                "a patch that removes the route",
                json!({"action": "patch", "patch": {"realm": {"team": {
                    "default_binding": null,
                    "binding": {"main": null},
                    "auth": {"main": null},
                    "backend": {"main": null},
                }}}}),
            ),
        ] {
            let (_directory, state) = state_for_team_realm(&own, &[("org", "")]).await;
            let generation = config_generation(&state).await;
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => {
                    if config_generation(&state).await <= generation {
                        not_committed.push(format!("{what}: nothing was committed"));
                    }
                }
                Err(refused) => not_committed.push(format!(
                    "{what}: refused: {} {:?}",
                    refused.message, refused.data
                )),
            }
        }
        assert!(
            not_committed.is_empty(),
            "the config tool did not take: {not_committed:#?}"
        );
    }

    /// The sections of a realm document that state two self-hosted servers
    /// and one model on the first.
    const SELF_HOSTED_SECTIONS: &str = "[self_hosted.servers.box]\n\
        base_url = \"http://box.synthetic.invalid/v1\"\n\n\
        [self_hosted.servers.gateway]\n\
        base_url = \"http://gateway.synthetic.invalid/v1\"\n\n\
        [self_hosted.models.local]\n\
        server = \"box\"\nremote_model = \"local-model\"\ndisplay_name = \"Local\"\n\
        family = \"synthetic\"\ntier = \"supported\"\n";

    /// The sections above with a route of the served realm whose binding
    /// names its server through its default model (the form of documents
    /// written before a backend could state its server).
    fn self_hosted_route_sections() -> String {
        format!(
            "{SELF_HOSTED_SECTIONS}\n\
             [realm.team.backend.box]\nprovider = \"self_hosted\"\n\
             backend_kind = \"self_hosted\"\n\n\
             [realm.team.auth.box]\nprovider = \"self_hosted\"\nauth_method = \"static_bearer\"\n\
             source = {{ kind = \"env\", env = \"MEERKAT_CONFIG_TOOL_ROW_VALUE\" }}\n\n\
             [realm.team.binding.box]\nbackend_profile = \"box\"\nauth_profile = \"box\"\n\
             default_model = \"local\"\n"
        )
    }

    /// A self-hosted request goes to the server its model names, with the
    /// credential of the binding for that server, and a binding can name
    /// its server through its default model. So the server of a model is
    /// part of where a credential goes: the config tool does not point an
    /// existing model at another server and does not add a model. And the
    /// removal of an own server entry that stood in front of a parent's of
    /// the same id brings the parent's address into force: refused as well.
    /// Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_change_the_server_a_self_hosted_model_names() {
        let own = self_hosted_route_sections();
        let mut not_refused = Vec::new();
        for (what, patch) in [
            (
                "a patch that points a model at another server",
                json!({"self_hosted": {"models": {"local": {"server": "gateway"}}}}),
            ),
            (
                "a patch that adds a model on another server",
                json!({"self_hosted": {"models": {"second": {
                    "server": "gateway",
                    "remote_model": "second-model",
                    "display_name": "Second",
                    "family": "synthetic",
                    "tier": "supported",
                }}, "default_model": "local"}}),
            ),
        ] {
            not_refused.extend(
                refused_in_team_realm(
                    what,
                    &own,
                    &[("org", "")],
                    json!({"action": "patch", "patch": patch}),
                    &[],
                )
                .await,
            );
        }
        // An own server entry in front of a parent's of the same id.
        let parent =
            "[self_hosted.servers.box]\nbase_url = \"http://parent.synthetic.invalid/v1\"\n";
        not_refused.extend(
            refused_in_team_realm(
                "a patch that removes an own server entry in front of a parent's",
                "[self_hosted.servers.box]\nbase_url = \"http://box.synthetic.invalid/v1\"\n",
                &[("org", parent)],
                json!({"action": "patch", "patch": {"self_hosted": {"servers": {"box": null}}}}),
                &[],
            )
            .await,
        );
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// What the config tool still takes of a self-hosted model: a field
    /// that names no server (here its display name).
    #[tokio::test]
    async fn test_config_writes_that_keep_a_self_hosted_models_server_still_commit() {
        let own = self_hosted_route_sections();
        let (_directory, state) = state_for_team_realm(&own, &[("org", "")]).await;
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"self_hosted": {"models": {"local": {
                "display_name": "Local, renamed",
            }}}}}),
        ))
        .await
        .expect("a patch of a model's display name commits");
        assert!(config_generation(&state).await > generation);
        let document = committed_config_document(&state).await;
        assert_eq!(
            document["self_hosted"]["models"]["local"]["server"],
            json!("box")
        );
        assert_eq!(
            document["self_hosted"]["models"]["local"]["display_name"],
            json!("Local, renamed")
        );
    }

    /// The server the served realm's binding `box` names in what the realm
    /// composes, as Core answers it: declared on its backend, derived from
    /// its default model, or none.
    async fn server_the_binding_names(state: &MeerkatMcpState) -> String {
        let composed = composed_config(state).await;
        let section = composed
            .realm
            .get("team")
            .expect("the served realm's section");
        let connections = meerkat_core::RealmConnectionSet::from_config("team", section)
            .expect("the served realm's routes");
        let binding = connections.bindings.get("box").expect("the binding");
        format!(
            "{:?}",
            meerkat_core::self_hosted_binding_server(&composed, &connections, binding)
        )
    }

    /// A binding can name its server through its default model's entry.
    /// The removal of that entry leaves the binding naming no server, and a
    /// binding that names none can stand for other declared servers: so the
    /// config tool does not remove it. Refused as host administration,
    /// nothing written; the binding names its server as before.
    #[tokio::test]
    async fn test_config_writes_cannot_change_the_server_a_binding_names() {
        let own = self_hosted_route_sections();
        let (_directory, state) = state_for_team_realm(&own, &[("org", "")]).await;
        let named = server_the_binding_names(&state).await;
        assert_eq!(named, "DerivedFromDefaultModel { server: \"box\" }");
        let generation = config_generation(&state).await;
        let document = committed_config_document(&state).await;
        match Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"self_hosted": {"models": {"local": null}}}}),
        ))
        .await
        {
            Ok(_) => panic!(
                "the removal of the model was accepted and the binding names {}",
                server_the_binding_names(&state).await
            ),
            Err(refused) => {
                assert_eq!(refused.code, -32602, "{}", refused.message);
                assert_eq!(
                    refused.data,
                    Some(json!({"type": "confinement_administration_required"})),
                    "{}",
                    refused.message
                );
            }
        }
        assert_eq!(config_generation(&state).await, generation);
        assert_eq!(committed_config_document(&state).await, document);
        assert_eq!(server_the_binding_names(&state).await, named);
    }

    /// The removal of a model that no binding names its server through
    /// commits, and the binding names its server as before.
    #[tokio::test]
    async fn test_config_writes_can_remove_a_model_no_binding_names_its_server_through() {
        let own = format!(
            "[self_hosted]\ndefault_model = \"local\"\n\n{}\n\
             [self_hosted.models.spare]\nserver = \"box\"\nremote_model = \"spare-model\"\n\
             display_name = \"Spare\"\nfamily = \"synthetic\"\ntier = \"supported\"\n",
            self_hosted_route_sections()
        );
        let (_directory, state) = state_for_team_realm(&own, &[("org", "")]).await;
        let named = server_the_binding_names(&state).await;
        assert_eq!(named, "DerivedFromDefaultModel { server: \"box\" }");
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"self_hosted": {"models": {"spare": null}}}}),
        ))
        .await
        .expect("the removal of a model no binding names its server through commits");
        assert!(config_generation(&state).await > generation);
        assert!(
            committed_config_document(&state).await["self_hosted"]["models"]
                .get("spare")
                .is_none()
        );
        assert_eq!(server_the_binding_names(&state).await, named);
    }

    /// The shell section the served realm composes along its parent chain:
    /// what a session of the realm runs its shell tool with.
    async fn composed_shell(state: &MeerkatMcpState) -> meerkat_core::config::ShellDefaults {
        let head = state
            .config_runtime
            .get()
            .await
            .expect("the realm's own document")
            .config;
        meerkat_core::EffectiveConfigReader::new(Arc::clone(&state.realm_config_source))
            .effective_config_over_head(&state.realm_id, head)
            .await
            .expect("the realm's chain composes")
            .shell
    }

    const PARENT_SHELL: &str = "/synthetic/parent-shell";

    /// A realm composes its shell section whole: its parent's, until its own
    /// differs from the default one, then its own. So in a realm whose
    /// parent names the shell program, a write of another shell key alone
    /// would put the default program in the parent's place: the config tool
    /// refuses it as host administration and the realm composes the
    /// parent's program as before. Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_change_the_shell_program_a_realm_composes() {
        let org_sections = format!("[shell]\nprogram = \"{PARENT_SHELL}\"\n");
        let mut not_refused = Vec::new();
        for what in [
            "a patch of the shell tool's timeout",
            "a patch of the shell tool's output cap",
            "a set of the shell tool's timeout",
        ] {
            let (_realms, state) = state_for_child_realm(&org_sections).await;
            assert_eq!(composed_shell(&state).await.program, PARENT_SHELL, "{what}");
            let generation = config_generation(&state).await;
            let arguments = match what {
                "a patch of the shell tool's timeout" => {
                    json!({"action": "patch", "patch": {"shell": {"timeout_secs": 60}}})
                }
                "a patch of the shell tool's output cap" => {
                    json!({"action": "patch", "patch": {"shell": {"max_output_chars": 1000}}})
                }
                _ => {
                    let mut document = committed_config_document(&state).await;
                    document["shell"]["timeout_secs"] = json!(60);
                    json!({"action": "set", "config": document})
                }
            };
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => not_refused.push(format!(
                    "{what}: the write was accepted and the realm composes the program {:?}",
                    composed_shell(&state).await.program
                )),
                Err(refused) => {
                    if refused.code != -32602
                        || refused.data
                            != Some(json!({"type": "confinement_administration_required"}))
                    {
                        not_refused.push(format!(
                            "{what}: refused otherwise: {} {} {:?}",
                            refused.code, refused.message, refused.data
                        ));
                    }
                    assert_eq!(config_generation(&state).await, generation, "{what}");
                    assert_eq!(composed_shell(&state).await.program, PARENT_SHELL, "{what}");
                }
            }
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// What the config tool still takes in a realm whose parent names the
    /// shell program: a patch of another section, a patch that states the
    /// default program (the realm's own section stays the default one, so
    /// the parent's still composes), and a patch of another shell key that
    /// states the parent's program with it. The realm composes the parent's
    /// program after each. Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_that_keep_the_composed_shell_program_still_commit() {
        let org_sections = format!("[shell]\nprogram = \"{PARENT_SHELL}\"\n");
        let default_program = meerkat_core::config::ShellDefaults::default().program;
        let mut not_committed = Vec::new();
        for (what, patch, timeout_secs) in [
            (
                "a patch of another section",
                json!({"max_tokens": 2048}),
                None,
            ),
            (
                "a patch that states the default program",
                json!({"shell": {"program": default_program}}),
                None,
            ),
            (
                "a patch of the timeout that states the parent's program",
                json!({"shell": {"program": PARENT_SHELL, "timeout_secs": 60}}),
                Some(60),
            ),
        ] {
            let (_realms, state) = state_for_child_realm(&org_sections).await;
            let before = composed_shell(&state).await;
            let generation = config_generation(&state).await;
            let arguments = json!({"action": "patch", "patch": patch});
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => {
                    let after = composed_shell(&state).await;
                    if config_generation(&state).await <= generation
                        || after.program != PARENT_SHELL
                        || after.timeout_secs != timeout_secs.unwrap_or(before.timeout_secs)
                    {
                        not_committed.push(format!(
                            "{what}: committed as program {:?}, timeout {}",
                            after.program, after.timeout_secs
                        ));
                    }
                }
                Err(refused) => not_committed.push(format!(
                    "{what}: refused: {} {:?}",
                    refused.message, refused.data
                )),
            }
        }
        assert!(
            not_committed.is_empty(),
            "the config tool did not take: {not_committed:#?}"
        );
    }

    /// A host patch that commits a command policy for the shell tool.
    fn shell_command_policy(mode: &str, patterns: &[&str]) -> Value {
        json!({"shell": {"security_mode": mode, "security_patterns": patterns}})
    }

    /// The document a state holds after `host_patch`, with `/shell/<key>`
    /// set to `value`: the complete typed document a set requires.
    async fn document_with_shell_key(host_patch: &Value, key: &str, value: Value) -> Value {
        let mut document =
            committed_config_document(&state_after_host_patch(Some(host_patch)).await).await;
        document["shell"][key] = value;
        document
    }

    /// The config tool does not loosen the command policy the shell tool
    /// checks its commands against. Under an allow list it takes no other
    /// mode (named, or the default in place of the committed one) and no
    /// pattern the committed list does not hold, by a patch or by a set;
    /// refused as host administration, nothing written.
    #[tokio::test]
    async fn test_config_writes_cannot_loosen_an_allow_list_of_shell_commands() {
        let allow = shell_command_policy("allow_list", &["git *", "ls *"]);
        let allow_set =
            document_with_shell_key(&allow, "security_mode", json!("unrestricted")).await;
        assert_writes_refused_after(
            Some(&allow),
            vec![
                patch_case(
                    "an allow list into no restriction",
                    "/shell/security_mode",
                    json!("unrestricted"),
                ),
                patch_case(
                    "an allow list into a deny list",
                    "/shell/security_mode",
                    json!("deny_list"),
                ),
                patch_case(
                    "an allow list's mode, back to the default",
                    "/shell/security_mode",
                    Value::Null,
                ),
                patch_case(
                    "an allow list, with a pattern added",
                    "/shell/security_patterns",
                    json!(["git *", "ls *", "rm *"]),
                ),
                patch_case(
                    "an allow list, with a pattern replaced",
                    "/shell/security_patterns",
                    json!(["git *", "cat *"]),
                ),
                (
                    "a set changes an allow list into no restriction".to_string(),
                    json!({"action": "set", "config": allow_set}),
                ),
            ],
        )
        .await;
    }

    /// Under a deny list with patterns the config tool takes no other mode
    /// and leaves no committed pattern out, by a patch or by a set; refused
    /// as host administration, nothing written.
    #[tokio::test]
    async fn test_config_writes_cannot_loosen_a_deny_list_of_shell_commands() {
        let deny = shell_command_policy("deny_list", &["rm *", "curl *"]);
        let deny_set = document_with_shell_key(&deny, "security_patterns", json!(["rm *"])).await;
        assert_writes_refused_after(
            Some(&deny),
            vec![
                patch_case(
                    "a deny list, with a pattern left out",
                    "/shell/security_patterns",
                    json!(["rm *"]),
                ),
                patch_case(
                    "a deny list, with every pattern left out",
                    "/shell/security_patterns",
                    json!([]),
                ),
                patch_case(
                    "a deny list into no restriction",
                    "/shell/security_mode",
                    json!("unrestricted"),
                ),
                patch_case(
                    "a deny list into an allow list",
                    "/shell/security_mode",
                    json!("allow_list"),
                ),
                (
                    "a set leaves a deny list's pattern out".to_string(),
                    json!({"action": "set", "config": deny_set}),
                ),
            ],
        )
        .await;
    }

    /// What the config tool still takes of the shell tool's command policy:
    /// a restated policy, an allow list with fewer patterns, a deny list
    /// with more (or the same in another order), and any policy where the
    /// committed one restricts nothing (no restriction, or a deny list with
    /// no pattern). Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_that_tighten_the_shell_command_policy_still_commit() {
        fn policy_patch(mode: &str, patterns: &[&str]) -> Value {
            json!({"action": "patch", "patch": shell_command_policy(mode, patterns)})
        }
        let allow = shell_command_policy("allow_list", &["git *", "ls *"]);
        let deny = shell_command_policy("deny_list", &["rm *", "curl *"]);
        let empty_deny = shell_command_policy("deny_list", &[]);
        let allow_set = document_with_shell_key(&allow, "security_patterns", json!(["ls *"])).await;
        let mut not_committed = Vec::new();
        for (what, host_patch, arguments) in [
            (
                "a patch restates an allow list",
                Some(&allow),
                policy_patch("allow_list", &["git *", "ls *"]),
            ),
            (
                "a patch leaves an allow list's pattern out",
                Some(&allow),
                policy_patch("allow_list", &["git *"]),
            ),
            (
                "a patch leaves every pattern of an allow list out",
                Some(&allow),
                policy_patch("allow_list", &[]),
            ),
            (
                "a set leaves an allow list's pattern out",
                Some(&allow),
                json!({"action": "set", "config": allow_set}),
            ),
            (
                "a patch restates a deny list",
                Some(&deny),
                policy_patch("deny_list", &["rm *", "curl *"]),
            ),
            (
                "a patch restates a deny list in another order",
                Some(&deny),
                policy_patch("deny_list", &["curl *", "rm *"]),
            ),
            (
                "a patch adds a pattern to a deny list",
                Some(&deny),
                policy_patch("deny_list", &["rm *", "curl *", "wget *"]),
            ),
            (
                "a patch changes no restriction into a deny list",
                None,
                policy_patch("deny_list", &["rm *"]),
            ),
            (
                "a patch changes no restriction into an allow list",
                None,
                policy_patch("allow_list", &["git *"]),
            ),
            (
                "a patch changes a deny list with no pattern into no restriction",
                Some(&empty_deny),
                policy_patch("unrestricted", &[]),
            ),
            (
                "a patch changes a deny list with no pattern into an allow list",
                Some(&empty_deny),
                policy_patch("allow_list", &["git *"]),
            ),
        ] {
            let state = state_after_host_patch(host_patch).await;
            let generation = config_generation(&state).await;
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => {
                    if config_generation(&state).await <= generation {
                        not_committed.push(format!("{what}: nothing was committed"));
                    }
                }
                Err(refused) => not_committed.push(format!(
                    "{what}: refused: {} {:?}",
                    refused.message, refused.data
                )),
            }
        }
        assert!(
            not_committed.is_empty(),
            "the config tool did not take: {not_committed:#?}"
        );
    }

    const PARENT_POLICY: &str =
        "[shell]\nsecurity_mode = \"allow_list\"\nsecurity_patterns = [\"git *\"]\n";

    /// The command policy the served realm composes, as `mode patterns`.
    async fn composed_command_policy(state: &MeerkatMcpState) -> String {
        let shell = composed_shell(state).await;
        format!("{:?} {:?}", shell.security_mode, shell.security_patterns)
    }

    /// A realm takes its shell section whole, so in a realm whose parent
    /// states an allow list a write of another shell key alone would put
    /// the default policy (no restriction) in the parent's place, and so
    /// would a parent edge to a realm that states none: the config tool
    /// refuses both as host administration and the realm composes the
    /// parent's policy as before. Each case on a state of its own.
    #[tokio::test]
    async fn test_config_writes_cannot_loosen_the_shell_command_policy_a_realm_composes() {
        let mut not_refused = Vec::new();
        for (what, patch) in [
            (
                "a patch of the shell tool's timeout",
                json!({"shell": {"timeout_secs": 60}}),
            ),
            (
                "a patch of the timeout that states the parent's mode with another pattern",
                json!({"shell": {
                    "timeout_secs": 60,
                    "security_mode": "allow_list",
                    "security_patterns": ["git *", "rm *"],
                }}),
            ),
            (
                "a patch of the parent edge to a realm that states no policy",
                json!({"realm": {"team": {"parent": "open"}}}),
            ),
        ] {
            let (_realms, state) =
                state_for_team_realm("", &[("org", PARENT_POLICY), ("open", "")]).await;
            let policy = composed_command_policy(&state).await;
            assert_eq!(policy, "AllowList [\"git *\"]", "{what}");
            let generation = config_generation(&state).await;
            let arguments = json!({"action": "patch", "patch": patch});
            match Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await {
                Ok(_) => not_refused.push(format!(
                    "{what}: the write was accepted and the realm composes the policy {}",
                    composed_command_policy(&state).await
                )),
                Err(refused) => {
                    if refused.code != -32602
                        || refused.data
                            != Some(json!({"type": "confinement_administration_required"}))
                    {
                        not_refused.push(format!(
                            "{what}: refused otherwise: {} {} {:?}",
                            refused.code, refused.message, refused.data
                        ));
                    }
                    assert_eq!(config_generation(&state).await, generation, "{what}");
                    assert_eq!(composed_command_policy(&state).await, policy, "{what}");
                }
            }
        }
        assert!(
            not_refused.is_empty(),
            "the config tool did not refuse: {not_refused:#?}"
        );
    }

    /// In a realm whose parent states an allow list, a patch of another
    /// shell key that states the parent's policy with it commits, and the
    /// realm composes the parent's policy after it.
    #[tokio::test]
    async fn test_config_writes_that_keep_the_composed_shell_command_policy_still_commit() {
        let (_realms, state) = state_for_child_realm(PARENT_POLICY).await;
        let policy = composed_command_policy(&state).await;
        let generation = config_generation(&state).await;
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"shell": {
                "timeout_secs": 60,
                "security_mode": "allow_list",
                "security_patterns": ["git *"],
            }}}),
        ))
        .await
        .expect("a patch that states the parent's policy with the timeout commits");
        assert!(config_generation(&state).await > generation);
        assert_eq!(composed_command_policy(&state).await, policy);
        assert_eq!(composed_shell(&state).await.timeout_secs, 60);
    }

    /// The config tool does not change the program the shell tool runs its
    /// commands with: another program, or the default in place of the
    /// committed one, by a patch or by a set.
    #[tokio::test]
    async fn test_config_writes_cannot_change_the_shell_program() {
        let mut other_program =
            committed_config_document(&state_with_launching_entries().await).await;
        other_program["shell"]["program"] = json!("/synthetic/other-shell");
        assert_launching_writes_refused(vec![
            patch_case(
                "the shell tool's program",
                "/shell/program",
                json!("/synthetic/other-shell"),
            ),
            patch_case(
                "the shell tool's program, back to the default",
                "/shell/program",
                Value::Null,
            ),
            (
                "a set changes the shell tool's program".to_string(),
                json!({"action": "set", "config": other_program}),
            ),
        ])
        .await;
    }

    /// What the config tool still takes: a read, a patch of another key,
    /// a patch and a set that restate the existing entries unchanged, an
    /// entry that launches no process (an HTTP server, an HTTP hook, a
    /// filesystem skills repository), another shell setting, and removals.
    #[tokio::test]
    async fn test_config_writes_that_launch_nothing_still_commit() {
        let state = state_with_launching_entries().await;
        let mut generation = config_generation(&state).await;
        let mut restated = committed_config_document(&state).await;
        restated["max_tokens"] = json!(4096);
        let repositories = json!([seeded_stdio_skills(), seeded_git_skills()]);
        for (what, arguments) in [
            (
                "a patch of another key",
                json!({"action": "patch", "patch": {"max_tokens": 2048}}),
            ),
            (
                "a patch that restates the existing entries",
                json!({"action": "patch", "patch": {
                    "tools": {"mcp_servers": [seeded_server()]},
                    "hooks": {"entries": [seeded_hook()]},
                    "skills": {"repositories": repositories},
                    "shell": {"program": "/synthetic/shell"},
                }}),
            ),
            (
                "a set that restates the existing entries",
                json!({"action": "set", "config": restated}),
            ),
            (
                "a patch that adds an HTTP server",
                json!({"action": "patch", "patch": {"tools": {"mcp_servers": [
                    seeded_server(),
                    http_server_entry("added-http-server"),
                ]}}}),
            ),
            (
                "a patch that adds an HTTP hook",
                json!({"action": "patch", "patch": {"hooks": {"entries": [
                    seeded_hook(),
                    hook_entry(
                        "added-http-hook",
                        json!({"type": "http", "url": "http://synthetic.invalid/hook"}),
                    ),
                ]}}}),
            ),
            (
                "a patch that adds a filesystem skills repository",
                json!({"action": "patch", "patch": {"skills": {"repositories": [
                    seeded_stdio_skills(),
                    seeded_git_skills(),
                    {
                        "name": "added-filesystem-skills",
                        "source_uuid": THIRD_SOURCE,
                        "type": "filesystem",
                        "path": "/synthetic/skills",
                    },
                ]}}}),
            ),
            (
                "a patch of the shell tool's timeout",
                json!({"action": "patch", "patch": {"shell": {"timeout_secs": 60}}}),
            ),
            (
                "a patch that removes the servers",
                json!({"action": "patch", "patch": {"tools": {"mcp_servers": []}}}),
            ),
            (
                "a patch that removes the hooks",
                json!({"action": "patch", "patch": {"hooks": {"entries": []}}}),
            ),
            (
                "a patch that removes the skills repositories",
                json!({"action": "patch", "patch": {"skills": {"repositories": []}}}),
            ),
        ] {
            if let Err(refused) =
                Box::pin(handle_tools_call(&state, "meerkat_config", &arguments)).await
            {
                panic!("{what}: {} {:?}", refused.message, refused.data);
            }
            let committed = config_generation(&state).await;
            assert!(committed > generation, "{what}: the write commits");
            generation = committed;
        }
        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "get"}),
        ))
        .await
        .expect("a read");
        let document = committed_config_document(&state).await;
        for pointer in [
            "/tools/mcp_servers",
            "/hooks/entries",
            "/skills/repositories",
        ] {
            assert!(
                document
                    .pointer(pointer)
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty),
                "{pointer} is empty after its removal: {document}"
            );
        }
        assert_eq!(document["shell"]["program"], json!("/synthetic/shell"));
        assert_eq!(document["shell"]["timeout_secs"], json!(60));
    }

    /// Which configuration entries feed a kind of process the host launches.
    enum LaunchFeed {
        /// Entries the config tool refuses to add or change.
        ConfigTool(&'static [LaunchingConfigEntry]),
        /// Entries the config store refuses on every surface.
        ConfigStore,
        /// No entry of the configuration document: a fixed program the host
        /// starts for its own operator.
        NotTheDocument,
    }

    /// The confined launch kinds. The match has no wildcard arm: a new kind
    /// does not compile until it is placed.
    fn confined_launch_feed(kind: meerkat_core::ConfinementLaunchKind) -> LaunchFeed {
        match kind {
            meerkat_core::ConfinementLaunchKind::Shell => LaunchFeed::ConfigTool(&[
                LaunchingConfigEntry::ShellProgram,
                LaunchingConfigEntry::ShellCommandPolicy,
            ]),
            meerkat_core::ConfinementLaunchKind::CommandHooks => {
                LaunchFeed::ConfigTool(&[LaunchingConfigEntry::CommandHook])
            }
            meerkat_core::ConfinementLaunchKind::McpStdio => LaunchFeed::ConfigTool(&[
                LaunchingConfigEntry::McpStdioServer,
                LaunchingConfigEntry::StdioSkillsRepository,
            ]),
        }
    }

    /// The host launch exemptions (a non-exhaustive type: one this table
    /// does not know answers `None`, which the row below refuses).
    fn exempt_launch_feed(exemption: meerkat_core::HostLaunchExemption) -> Option<LaunchFeed> {
        match exemption {
            meerkat_core::HostLaunchExemption::GitSkillSource => Some(LaunchFeed::ConfigTool(&[
                LaunchingConfigEntry::GitSkillsRepository,
            ])),
            // An auth profile's credential command.
            meerkat_core::HostLaunchExemption::CredentialHelper => Some(LaunchFeed::ConfigStore),
            // The platform's browser opener at a login the operator starts,
            // the terminal's own `stty`, and `wasm-pack` under `rkat doctor`.
            meerkat_core::HostLaunchExemption::BrowserOpener
            | meerkat_core::HostLaunchExemption::TerminalControl
            | meerkat_core::HostLaunchExemption::DeveloperBuildTooling => {
                Some(LaunchFeed::NotTheDocument)
            }
            _ => None,
        }
    }

    /// Every kind of process the host launches is placed against the config
    /// tool's refusal: its configuration entries are refused by the tool,
    /// refused by the config store, or it is fed by no entry of the
    /// document. A launch in shipped code carries one of these kinds (the
    /// workspace's launch lint and its launch site inventory), so a new kind
    /// of launch reaches this row; and every launching entry the tool
    /// refuses is the feed of exactly one kind.
    #[test]
    fn test_every_launch_kind_is_placed_against_the_config_tool_refusal() {
        let mut refused = Vec::new();
        let feeds = meerkat_core::ConfinementLaunchKind::ALL
            .into_iter()
            .map(confined_launch_feed)
            .chain(
                meerkat_core::HostLaunchExemption::ALL
                    .into_iter()
                    .map(|exemption| {
                        exempt_launch_feed(exemption).unwrap_or_else(|| {
                            panic!(
                                "{exemption:?} launches a process and is not placed against \
                                 the config tool's refusal"
                            )
                        })
                    }),
            );
        for feed in feeds {
            match feed {
                LaunchFeed::ConfigTool(entries) => refused.extend_from_slice(entries),
                LaunchFeed::ConfigStore | LaunchFeed::NotTheDocument => {}
            }
        }
        for entry in LaunchingConfigEntry::ALL {
            assert_eq!(
                refused.iter().filter(|fed| **fed == entry).count(),
                1,
                "{entry:?} is the feed of exactly one launch kind"
            );
        }
        assert_eq!(refused.len(), LaunchingConfigEntry::ALL.len());
    }

    /// The fields of the sections the refusal passes over, each named with
    /// no rest pattern: a new field of any of them does not compile until
    /// someone has read whether it names a program, an argument, an
    /// environment or a working directory of a launch. (The three sections
    /// that hold a launching entry, and the shell section, are destructured
    /// in the refusal itself;
    /// `[governance]`, `[confinement]` and the realm sections' credential
    /// commands are the config store's refusals.)
    #[allow(clippy::too_many_lines)]
    fn census_of_the_sections_that_launch_nothing(config: Config) {
        use meerkat_core::config::{
            AgentConfig, BudgetConfig, CommsRuntimeConfig, CompactionRuntimeConfig,
            HtmlPresentationConfig, HtmlTemplateConfig, LimitsConfig, MobHostConfig, ModelDefaults,
            ModelFallbackConfig, PresentationConfig, ProviderToolsConfig, RestServerConfig,
            RetryConfig, SelfHostedConfig, SelfHostedServerConfig, StorageConfig, StoreConfig,
        };
        // Prompts, a prompt file read as text, a model and its parameters.
        let AgentConfig {
            system_prompt: _,
            system_prompt_file: _,
            tool_instructions: _,
            model: _,
            max_tokens_per_turn: _,
            temperature: _,
            budget_warning_threshold: _,
            max_turns: _,
            provider_params: _,
            provider_native_tools: _,
            output_schema: _,
            structured_output_retries: _,
            extraction_prompt: _,
        } = config.agent;
        // Where state is kept. `directory` is deprecated and read by no
        // surface; it is named here so the census stays whole until the
        // field is removed.
        #[allow(deprecated)]
        let StorageConfig {
            hosting: _,
            directory: _,
        } = config.storage;
        let StoreConfig {
            sessions_path: _,
            tasks_path: _,
            database_dir: _,
        } = config.store;
        // Numbers and durations.
        let BudgetConfig {
            max_tokens: _,
            max_duration: _,
            max_tool_calls: _,
        } = config.budget;
        let RetryConfig {
            max_retries: _,
            initial_delay: _,
            max_delay: _,
            multiplier: _,
            call_timeout_override: _,
            stream_inactivity_timeout_override: _,
        } = config.retry;
        let LimitsConfig {
            budget: _,
            max_sessions: _,
            max_duration: _,
            max_turn_duration: _,
            max_tool_calls: _,
        } = config.limits;
        let CompactionRuntimeConfig {
            auto_compact_threshold: _,
            auto_compact_threshold_explicit: _,
            max_request_bytes: _,
            recent_turn_budget: _,
            max_summary_tokens: _,
            min_turns_between_compactions: _,
            history_retained_rewrites: _,
        } = config.compaction;
        // Model names and capabilities.
        let ModelDefaults {
            anthropic: _,
            openai: _,
            gemini: _,
            custom: _,
        } = config.models;
        let ModelFallbackConfig {
            enabled: _,
            chain: _,
            policy: _,
        } = config.model_fallback;
        let ProviderToolsConfig {
            anthropic: _,
            openai: _,
            gemini: _,
        } = config.provider_tools;
        // Addresses the host calls: no program.
        let SelfHostedConfig {
            servers,
            models: _,
            default_model: _,
        } = config.self_hosted;
        for (_, server) in servers {
            let SelfHostedServerConfig {
                transport: _,
                base_url: _,
                api_style: _,
            } = server;
        }
        // Listeners, addresses and identity paths.
        let CommsRuntimeConfig {
            mode: _,
            address: _,
            advertise_address: _,
            auth: _,
            require_peer_auth: _,
            event_address: _,
            pairing_password: _,
        } = config.comms;
        let RestServerConfig { host: _, port: _ } = config.rest;
        let MobHostConfig {
            realm: _,
            listen_tcp: _,
            advertise_tcp: _,
            live_ws: _,
            live_ws_advertise: _,
            identity_dir: _,
            descriptor_out: _,
            max_connections: _,
            read_deadline_ms: _,
            pairing_rate: _,
            live_buffer_events: _,
            pairing_password: _,
        } = config.mob_host;
        // Template text, or a template path read as text.
        let PresentationConfig { html } = config.presentation;
        let HtmlPresentationConfig {
            default_template: _,
            templates,
        } = html;
        for (_, template) in templates {
            let HtmlTemplateConfig { path: _, body: _ } = template;
        }
    }

    /// The census above is a compile-time check; this keeps it built.
    #[test]
    fn test_the_sections_the_config_tool_passes_over_are_read_field_by_field() {
        census_of_the_sections_that_launch_nothing(Config::default());
    }

    /// The entries the refusal judges to launch nothing (an HTTP server, an
    /// in-process or HTTP hook, a filesystem or HTTP skills repository),
    /// each with its fields beside the transport or the runtime and its
    /// payload named with no rest pattern: a new field of one of them, or a
    /// new way for an HTTP skills repository to authenticate, does not
    /// compile until someone has read whether it names a program, an
    /// argument, an environment or a working directory of a launch. (A
    /// launching entry is compared whole, so its own payload needs no
    /// census; the fields beside it are the same ones named here.)
    fn census_of_the_entries_that_launch_nothing(
        server: meerkat_core::mcp_config::McpServerConfig,
        hook: meerkat_core::config::HookEntryConfig,
        repository: meerkat_core::skills_config::SkillRepositoryConfig,
    ) -> [&'static str; 3] {
        use meerkat_core::config::{
            HookAdapterConfig, HookEntryConfig, HookInProcessRuntimeConfig, HttpRuntimeConfig,
        };
        use meerkat_core::mcp_config::{McpHttpConfig, McpServerConfig, McpTransportConfig};
        use meerkat_core::skills_config::{
            HttpSkillRepoAuth, SkillRepoTransport, SkillRepositoryConfig,
        };
        // A name, a connect timeout, the names its tools are exposed under,
        // the host's access ceiling, a connector credential binding and an
        // account declaration.
        let McpServerConfig {
            name: _,
            transport,
            connect_timeout_secs: _,
            tool_names: _,
            access: _,
            connector: _,
            account: _,
        } = server;
        let server = match transport {
            // An address, request headers, the wire transport and the
            // account an OAuth login is held to.
            McpTransportConfig::Http(McpHttpConfig {
                url: _,
                headers: _,
                transport: _,
                oauth_account: _,
                oauth_account_selection: _,
            }) => "launches nothing",
            McpTransportConfig::Stdio(_) => "compared whole by the refusal",
        };
        // An id, a switch, the point and the mode it runs at, what it may
        // decide, an order, a timeout and the model operations it governs.
        let HookEntryConfig {
            id: _,
            enabled: _,
            point: _,
            mode: _,
            capability: _,
            priority: _,
            timeout_ms: _,
            model_operations: _,
            runtime,
        } = hook;
        let hook = match runtime {
            // The id of a handler registered in the host's own code.
            HookAdapterConfig::InProcess(HookInProcessRuntimeConfig { handler: _ }) => {
                "launches nothing: a registered handler"
            }
            // An address, a method and request headers.
            HookAdapterConfig::Http(HttpRuntimeConfig {
                url: _,
                method: _,
                headers: _,
            }) => "launches nothing: a request",
            HookAdapterConfig::Command(_) => "compared whole by the refusal",
        };
        // A display name and the source's stable id.
        let SkillRepositoryConfig {
            name: _,
            source_uuid: _,
            transport,
        } = repository;
        let repository = match transport {
            // A directory read as skill documents.
            SkillRepoTransport::Filesystem { path: _ } => "launches nothing: a directory",
            // An address, a credential sent with the request, two durations.
            SkillRepoTransport::Http {
                url: _,
                auth,
                refresh_seconds: _,
                timeout_seconds: _,
            } => match auth {
                None => "launches nothing: a request",
                Some(HttpSkillRepoAuth::Bearer { token: _ }) => {
                    "launches nothing: a request with a bearer token"
                }
                Some(HttpSkillRepoAuth::Header { name: _, value: _ }) => {
                    "launches nothing: a request with a header credential"
                }
            },
            SkillRepoTransport::Stdio { .. } | SkillRepoTransport::Git { .. } => {
                "compared whole by the refusal"
            }
        };
        [server, hook, repository]
    }

    /// The census above is a compile-time check; this keeps it built, on one
    /// entry of each kind that launches nothing.
    #[test]
    fn test_the_entries_the_config_tool_passes_over_are_read_field_by_field() {
        let placed = census_of_the_entries_that_launch_nothing(
            serde_json::from_value(http_server_entry("http-server")).expect("an HTTP server"),
            serde_json::from_value(hook_entry(
                "http-hook",
                json!({"type": "http", "url": "http://synthetic.invalid/hook"}),
            ))
            .expect("an HTTP hook"),
            serde_json::from_value(json!({
                "name": "filesystem-skills",
                "source_uuid": FIRST_SOURCE,
                "type": "filesystem",
                "path": "/synthetic/skills",
            }))
            .expect("a filesystem skills repository"),
        );
        assert!(
            placed
                .iter()
                .all(|placed| placed.starts_with("launches nothing")),
            "{placed:?}"
        );
    }

    /// A requirement no platform backend enforces (descendant termination),
    /// so a server that installs it refuses every confined launch on any OS.
    const UNENFORCEABLE_CONFINEMENT: &str = r#"[confinement.all]
mode = "custom"
working_directory = "/"
requirement = { baseline = "command_runtime_v1", read = { kind = "paths", paths = [] }, write = { kind = "paths", paths = [] }, deny_read = [], deny_write = [], network = { kind = "denied" }, unix_connect = [], require_descendant_termination = true }
"#;

    /// A `meerkat_config` write cannot cut the served realm from the parent
    /// whose `[confinement]` it composes: a patch that removes the realm's
    /// parent edge, or a full replace without it, is refused with the
    /// confinement discriminator and writes nothing. A write that keeps the
    /// edge commits.
    #[tokio::test]
    async fn test_config_writes_cannot_drop_inherited_confinement() {
        let temp = tempfile::tempdir().expect("tempdir");
        let realms = temp.path().join("realms");
        let team_text = "[realm.team]\nparent = \"org\"\n";
        let org_text = format!("[realm.org]\n\n{UNENFORCEABLE_CONFINEMENT}");
        for (realm, text) in [("org", org_text.as_str()), ("team", team_text)] {
            let path = meerkat_store::realm_paths_in(&realms, realm).config_path;
            std::fs::create_dir_all(path.parent().expect("realm dir")).expect("realm dir");
            std::fs::write(&path, text).expect("realm document");
        }
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "team".into(),
        };
        bootstrap.realm.state_root = Some(realms.clone());
        bootstrap.context.user_config_root = Some(temp.path().join("user"));
        let state = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
        .expect("the team realm serves");
        let team_doc = meerkat_store::realm_paths_in(&realms, "team").config_path;
        let generation = config_generation(&state).await;

        let detached = Config {
            max_tokens: Some(1024),
            ..Config::default()
        };
        for arguments in [
            json!({"action": "patch", "patch": {"realm": {"team": {"parent": null}}}}),
            json!({"action": "patch", "patch": {"realm": {"team": null}}}),
            json!({"action": "set", "config": detached}),
        ] {
            let refused = Box::pin(handle_tools_call(&state, "meerkat_config", &arguments))
                .await
                .expect_err("a write cannot drop the inherited confinement");
            assert_eq!(refused.code, -32602, "{arguments}");
            assert_eq!(
                refused.data,
                Some(json!({"type": "confinement_administration_required"})),
                "{arguments}: {}",
                refused.message
            );
        }
        assert_eq!(std::fs::read_to_string(&team_doc).expect("team"), team_text);
        assert_eq!(config_generation(&state).await, generation);

        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"max_tokens": 2048}}),
        ))
        .await
        .expect("a patch that keeps the edge commits");
        let committed = std::fs::read_to_string(&team_doc).expect("team");
        assert!(committed.contains("parent = \"org\""), "{committed}");
    }

    /// A `meerkat_config` write cannot cut the served realm from the parent
    /// whose `[governance]` it composes: a patch that removes the realm's
    /// parent edge, or a full replace without it, is refused with the
    /// governance discriminator and writes nothing. A write that keeps the
    /// edge commits.
    #[tokio::test]
    async fn test_config_writes_cannot_drop_inherited_governance() {
        let temp = tempfile::tempdir().expect("tempdir");
        let realms = temp.path().join("realms");
        let team_text = "[realm.team]\nparent = \"org\"\n";
        let org_text = "[realm.org]\n\n[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\" }\ntier = \"r2\"\n";
        for (realm, text) in [("org", org_text), ("team", team_text)] {
            let path = meerkat_store::realm_paths_in(&realms, realm).config_path;
            std::fs::create_dir_all(path.parent().expect("realm dir")).expect("realm dir");
            std::fs::write(&path, text).expect("realm document");
        }
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "team".into(),
        };
        bootstrap.realm.state_root = Some(realms.clone());
        bootstrap.context.user_config_root = Some(temp.path().join("user"));
        let state = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
        .expect("the team realm serves");
        let team_doc = meerkat_store::realm_paths_in(&realms, "team").config_path;
        let generation = config_generation(&state).await;

        let detached = Config {
            max_tokens: Some(1024),
            ..Config::default()
        };
        for arguments in [
            json!({"action": "patch", "patch": {"realm": {"team": {"parent": null}}}}),
            json!({"action": "patch", "patch": {"realm": {"team": null}}}),
            json!({"action": "set", "config": detached}),
        ] {
            let refused = Box::pin(handle_tools_call(&state, "meerkat_config", &arguments))
                .await
                .expect_err("a write cannot drop the inherited governance");
            assert_eq!(refused.code, -32602, "{arguments}");
            assert_eq!(
                refused.data,
                Some(json!({"type": "governance_administration_required"})),
                "{arguments}: {}",
                refused.message
            );
        }
        assert_eq!(std::fs::read_to_string(&team_doc).expect("team"), team_text);
        assert_eq!(config_generation(&state).await, generation);

        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"max_tokens": 2048}}),
        ))
        .await
        .expect("a patch that keeps the edge commits");
        let committed = std::fs::read_to_string(&team_doc).expect("team");
        assert!(committed.contains("parent = \"org\""), "{committed}");
    }

    /// Served as the global realm, the MCP config tool writes the user
    /// document, whose `[realm.global]` section puts the user document's
    /// `[governance]` on the chain of every realm that states no parent of
    /// its own. A patch that removes that section is refused with the
    /// governance discriminator, naming the section, and writes nothing; an
    /// unrelated edit commits.
    #[tokio::test]
    async fn test_user_document_writes_cannot_drop_the_governance_other_realms_compose() {
        let temp = tempfile::tempdir().expect("tempdir");
        let realms = temp.path().join("realms");
        let user_root = temp.path().join("user");
        std::fs::create_dir_all(user_root.join(".rkat")).expect("user root");
        let user_doc = user_root.join(".rkat").join("config.toml");
        let user_text = "[realm.global]\n\n[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\" }\ntier = \"r2\"\n";
        std::fs::write(&user_doc, user_text).expect("user document");
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "global".into(),
        };
        bootstrap.realm.state_root = Some(realms);
        bootstrap.context.user_config_root = Some(user_root);
        let state = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
        .expect("the global realm serves");

        let refused = Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"realm": {"global": null}}}),
        ))
        .await
        .expect_err("the patch would drop the governance other realms compose");
        assert_eq!(refused.code, -32602);
        assert_eq!(
            refused.data,
            Some(json!({"type": "governance_administration_required"})),
            "{}",
            refused.message
        );
        assert!(
            refused.message.contains("[realm.global]"),
            "{}",
            refused.message
        );
        assert_eq!(std::fs::read_to_string(&user_doc).expect("user"), user_text);

        Box::pin(handle_tools_call(
            &state,
            "meerkat_config",
            &json!({"action": "patch", "patch": {"max_tokens": 2048}}),
        ))
        .await
        .expect("an unrelated edit commits");
    }

    /// A start whose confined processes could change the realm state is
    /// refused before the server serves, naming the input and its remedy.
    #[tokio::test]
    async fn test_startup_refuses_confinement_that_could_change_the_realm_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().canonicalize().expect("canonical temp");
        let state = base.join("realms");
        std::fs::create_dir_all(&state).expect("state root");
        let user_root = base.join("user");
        std::fs::create_dir_all(user_root.join(".rkat")).expect("user root");
        std::fs::write(user_root.join(".rkat").join("config.toml"), format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{state}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{state}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{state}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n",
            state = state.display()
        ))
        .expect("user document");
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "state-writer".into(),
        };
        bootstrap.realm.state_root = Some(state);
        bootstrap.context.context_root = Some(base.clone());
        bootstrap.context.user_config_root = Some(user_root);
        let refused = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
            None,
        )
        .await
        .err()
        .expect("a confined process could change the realm state");
        let message = refused.to_string();
        assert!(
            message.contains("the realm state") && message.contains("--state-root"),
            "{message}"
        );
    }

    /// The server's one MCP stdio launch profile comes from the composed
    /// `[confinement]`: a requirement in the user document applies without a
    /// `[realm.global]` section, the operator's flag replaces the documents,
    /// and the workspace preset without --context-root refuses startup
    /// instead of confining nothing.
    #[tokio::test]
    async fn test_startup_takes_the_stdio_profile_from_documents_and_flags() {
        let temp = tempfile::tempdir().expect("tempdir");
        let user_root = temp.path().join("user");
        std::fs::create_dir_all(user_root.join(".rkat")).expect("user root");
        std::fs::write(
            user_root.join(".rkat").join("config.toml"),
            UNENFORCEABLE_CONFINEMENT,
        )
        .expect("user document");
        let bootstrap = |realm_id: &str, context_root: Option<PathBuf>| {
            let mut bootstrap = RuntimeBootstrap::default();
            bootstrap.realm.selection = RealmSelection::Explicit {
                realm_id: realm_id.into(),
            };
            bootstrap.realm.state_root = Some(temp.path().join("realms"));
            bootstrap.context.context_root = context_root;
            bootstrap.context.user_config_root = Some(user_root.clone());
            bootstrap
        };
        let start = |bootstrap, confinement| {
            MeerkatMcpState::new_with_bootstrap_options_and_llm(
                bootstrap,
                false,
                Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
                confinement,
            )
        };

        let configured = start(bootstrap("confined", Some(temp.path().to_path_buf())), None)
            .await
            .expect("configured server");
        assert_eq!(
            configured.mcp_stdio_launch_profile.capabilities().err(),
            Some(meerkat_core::confinement::ConfinementRefusal::UnsupportedRequirement),
            "the user document's requirement reaches the server's launch boundary"
        );

        let flagged = start(
            bootstrap("flagged", Some(temp.path().to_path_buf())),
            Some(meerkat_core::LaunchConfinement::TrustedHost),
        )
        .await
        .expect("flagged server");
        assert!(
            matches!(flagged.mcp_stdio_launch_profile.capabilities(), Ok(None)),
            "the operator's flag replaces the documents"
        );

        let refused = start(
            bootstrap("unrooted", None),
            Some(meerkat_core::LaunchConfinement::Workspace(
                meerkat_core::WorkspaceConfinement::default(),
            )),
        )
        .await
        .err()
        .expect("workspace confinement without --context-root refuses startup");
        assert!(refused.to_string().contains("--context-root"), "{refused}");
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

    /// `meerkat_run` takes the provider-neutral `reasoning_effort` level and
    /// `meerkat_resume` its tri-state. A resume that names it rebuilds the
    /// session, as a resume that names `provider_params` does.
    #[test]
    fn meerkat_run_and_resume_inputs_read_the_reasoning_effort() {
        let run: MeerkatRunInput = serde_json::from_value(json!({
            "prompt": "Hello",
            "reasoning_effort": "high"
        }))
        .expect("meerkat_run reads a level");
        assert!(
            format!("{run:?}").contains("reasoning_effort: Some(High)"),
            "the level is read, not ignored: {run:?}"
        );
        let run: MeerkatRunInput =
            serde_json::from_value(json!({"prompt": "Hello"})).expect("meerkat_run");
        assert!(format!("{run:?}").contains("reasoning_effort: None"));

        let resume = |reasoning_effort: Option<serde_json::Value>| {
            let mut input = json!({
                "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                "prompt": "Continue"
            });
            if let Some(value) = reasoning_effort {
                input["reasoning_effort"] = value;
            }
            serde_json::from_value::<MeerkatResumeInput>(input)
        };
        for (value, expected) in [
            (json!("low"), "reasoning_effort: Some(Set(Low))"),
            (
                json!({"action": "set", "value": "xhigh"}),
                "reasoning_effort: Some(Set(Xhigh))",
            ),
            (json!({"action": "clear"}), "reasoning_effort: Some(Clear)"),
        ] {
            let input = resume(Some(value.clone()))
                .unwrap_or_else(|error| panic!("meerkat_resume reads {value}: {error}"));
            assert!(
                format!("{input:?}").contains(expected),
                "{value} reads as {expected}: {input:?}"
            );
            assert!(
                mcp_resume_requires_rebuild(&input),
                "a resume that names the level rebuilds the session: {value}"
            );
        }
        let keep = resume(None).expect("meerkat_resume without a level");
        assert!(format!("{keep:?}").contains("reasoning_effort: None"));
        assert!(!mcp_resume_requires_rebuild(&keep));

        for not_a_level in ["x_high", "extreme"] {
            let error = serde_json::from_value::<MeerkatRunInput>(json!({
                "prompt": "Hello",
                "reasoning_effort": not_a_level
            }))
            .expect_err("meerkat_run refuses a word outside the level vocabulary")
            .to_string();
            assert!(error.contains("`xhigh`"), "{not_a_level}: {error}");
            let error = resume(Some(json!(not_a_level)))
                .expect_err("meerkat_resume refuses a word outside the level vocabulary")
                .to_string();
            assert!(error.contains("`xhigh`"), "{not_a_level}: {error}");
        }
    }

    /// The level of `meerkat_run` reaches the build inside its provider
    /// params, and `meerkat_resume` folds its tri-state the same way: a
    /// level beside params that name none is added, a clear leaves the
    /// params without one, and two different outcomes are refused.
    #[test]
    fn mcp_reasoning_effort_build_params_and_resume_claim() {
        use meerkat_core::model_profile::capabilities::EffortLevel;

        let run = |input: serde_json::Value| {
            let input: MeerkatRunInput = serde_json::from_value(input).expect("run input");
            run_build_provider_params(&input)
        };
        let Ok(folded) = run(json!({
            "prompt": "Hello",
            "reasoning_effort": "high",
            "provider_params": {"temperature": 0.2}
        })) else {
            panic!("a level beside other params is folded");
        };
        let folded = folded.expect("params");
        assert_eq!(folded.reasoning_effort, Some(EffortLevel::High));
        assert_eq!(folded.temperature, Some(0.2));
        let Ok(nested) = run(json!({
            "prompt": "Hello",
            "reasoning_effort": "low",
            "provider_params": {"reasoning_effort": "low"}
        })) else {
            panic!("the same level in both is accepted");
        };
        assert_eq!(
            nested.and_then(|params| params.reasoning_effort),
            Some(EffortLevel::Low)
        );
        let Ok(none) = run(json!({"prompt": "Hello"})) else {
            panic!("an input without a level builds");
        };
        assert_eq!(none, None);
        assert!(
            run(json!({
                "prompt": "Hello",
                "reasoning_effort": "high",
                "provider_params": {"reasoning_effort": "low"}
            }))
            .is_err(),
            "two different levels are refused"
        );

        let resume = |reasoning_effort: Option<serde_json::Value>,
                      provider_params: Option<serde_json::Value>| {
            let mut input = json!({
                "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                "prompt": "Continue"
            });
            if let Some(value) = reasoning_effort {
                input["reasoning_effort"] = value;
            }
            if let Some(value) = provider_params {
                input["provider_params"] = value;
            }
            let input: MeerkatResumeInput = serde_json::from_value(input).expect("resume input");
            resume_build_provider_params(&input)
        };
        let level_of = |result: Result<
            Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
            ToolCallError,
        >| match result {
            Ok(params) => params.and_then(|params| params.reasoning_effort),
            Err(error) => panic!("the resume input asks for one outcome: {error:?}"),
        };
        assert_eq!(
            level_of(resume(Some(json!("medium")), None)),
            Some(EffortLevel::Medium)
        );
        assert_eq!(
            level_of(resume(Some(json!({"action": "clear"})), None)),
            None
        );
        assert_eq!(level_of(resume(None, None)), None);
        assert_eq!(
            level_of(resume(
                Some(json!("medium")),
                Some(json!({"temperature": 0.5}))
            )),
            Some(EffortLevel::Medium)
        );
        // Params that name no level keep the recorded one: nothing is named.
        assert_eq!(
            level_of(resume(None, Some(json!({"temperature": 0.5})))),
            None
        );
        assert!(
            resume(
                Some(json!({"action": "clear"})),
                Some(json!({"reasoning_effort": "low"}))
            )
            .is_err(),
            "a clear beside params that name a level is refused"
        );
        assert!(
            resume(
                Some(json!("high")),
                Some(json!({"reasoning_effort": "low"}))
            )
            .is_err(),
            "two different levels are refused"
        );
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
            reasoning_effort: None,
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
            run_usage: None,
            request_usage: Vec::new(),
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
    fn test_format_agent_result_preserves_callback_batch_as_resumable_full_set() {
        let session_id = meerkat::SessionId::new();
        let pending_tool_calls = vec![
            meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "callback-a".to_string(),
                tool_name: "ask_a".to_string(),
                args: json!({"question": "a"}),
                settlement_failures: vec![meerkat_core::ToolDispatchSettlementFailure {
                    admission_source: meerkat_core::ToolDispatchAdmissionSource::ConfiguredGate,
                    effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
                    physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Unknown,
                    failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::Unavailable,
                }],
                dedupe_key: None,
            },
            meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "callback-b".to_string(),
                tool_name: "ask_b".to_string(),
                args: json!({"question": "b"}),
                settlement_failures: Vec::new(),
                dedupe_key: None,
            },
        ];
        let payload = format_agent_result(
            Err(SessionError::Agent(
                meerkat::AgentError::CallbackBatchPending { pending_tool_calls },
            )),
            &session_id,
        )
        .expect("callback batch should remain a resumable MCP result");
        let raw = payload["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        let decoded: Value = serde_json::from_str(raw).expect("valid wrapped JSON");
        assert_eq!(decoded["status"], "pending_tool_call");
        assert_eq!(decoded["resumable"], true);
        assert_eq!(
            decoded["pending_tool_calls"][0]["settlement_failures"][0]["physical_outcome"],
            "unknown"
        );
        assert_eq!(decoded["pending_tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(
            decoded["pending_tool_calls"][0]["tool_use_id"],
            "callback-a"
        );
        assert_eq!(
            decoded["pending_tool_calls"][1]["tool_use_id"],
            "callback-b"
        );
        assert!(
            decoded["pending_tool_calls"][0]["args"]
                .get("tool_use_id")
                .is_none(),
            "tool_use_id must not be smuggled into callback args"
        );
    }

    /// The `pending_tool_calls` of a single pending callback call carry its
    /// dedupe key unchanged, and a call without one omits the field. (Unrun
    /// until the candidate's validation.)
    #[test]
    fn test_format_agent_result_carries_the_callback_dedupe_key() {
        let session_id = meerkat::SessionId::new();
        let arguments = serde_json::value::RawValue::from_string(r#"{"question":"a"}"#.into())
            .expect("valid arguments");
        let key = meerkat_core::ToolCallDedupeKey::new(
            meerkat_core::RunId::new(),
            0,
            "callback-a",
            "ask_a",
            &arguments,
        )
        .expect("valid dedupe key");
        for dedupe_key in [Some(key), None] {
            let payload = format_agent_result(
                Err(SessionError::Agent(meerkat::AgentError::CallbackPending {
                    tool_use_id: "callback-a".to_string(),
                    tool_name: "ask_a".to_string(),
                    args: json!({"question": "a"}),
                    dedupe_key: dedupe_key.clone().map(Box::new),
                })),
                &session_id,
            )
            .expect("a pending callback call stays a resumable MCP result");
            let raw = payload["content"][0]["text"]
                .as_str()
                .expect("wrapped MCP payload text");
            let decoded: Value = serde_json::from_str(raw).expect("valid wrapped JSON");
            assert_eq!(decoded["status"], "pending_tool_call");
            let call = &decoded["pending_tool_calls"][0];
            match &dedupe_key {
                Some(key) => assert_eq!(
                    call["dedupe_key"],
                    serde_json::to_value(key).expect("key serializes")
                ),
                None => assert!(call.get("dedupe_key").is_none(), "{call}"),
            }
        }
    }

    #[test]
    fn callback_resume_retries_after_stage_before_run_crash() {
        assert!(
            !callback_resume_is_already_applied(StageToolResultsDisposition::AlreadyStaged),
            "durably staged replay must continue through actor ensure and run_pending"
        );
        assert!(
            callback_resume_is_already_applied(StageToolResultsDisposition::AlreadyApplied),
            "only an applied receipt may short-circuit callback resume"
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

    #[cfg(not(feature = "comms"))]
    #[tokio::test]
    async fn test_meerkat_resume_rejects_inherited_persisted_keep_alive_without_comms() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            None,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                model_fallback: None,
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling::default(),
                keep_alive: true,
                comms_name: Some("persisted-mcp-agent".to_string()),
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
        seed_runtime_authority_session(&runtime_store, &session).await;

        let error = Box::pin(handle_meerkat_resume(
            &state,
            bounded_resume_input(session_id.to_string(), vec![]),
            None,
            None,
        ))
        .await
        .expect_err("inherited keep_alive should reject without surface comms");

        assert!(error.message.contains("keep_alive requires comms support"));
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
    fn test_normalize_mcp_comms_send_error_response_to_a_non_requester_is_send_failed() {
        let addressee = meerkat_core::comms::PeerId::new();
        let in_reply_to = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::ResponseAddresseeNotRequester {
                in_reply_to,
                addressee,
            },
        );
        assert_eq!(err.code, -32603);
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("send_failed")
        );
        let detail = data
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for name in [addressee.to_string(), in_reply_to.to_string()] {
            assert!(err.message.contains(&name), "{}", err.message);
            assert!(detail.contains(&name), "{detail}");
        }
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

    // An ambiguous delivery may already have landed: it must reach the caller
    // as `send_ambiguous` with its envelope id and the same reconcile fields
    // RPC and REST carry, never as `send_failed`, which invites a retry.
    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_names_ambiguous_delivery() {
        let envelope_id = uuid::Uuid::new_v4();
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::AmbiguousDelivery {
                envelope_id,
                detail: "ack timed out".to_string(),
            },
        );
        assert_eq!(err.code, -32603);
        assert!(
            err.message.starts_with("send_ambiguous:"),
            "{}",
            err.message
        );
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("send_ambiguous")
        );
        assert_eq!(
            data.get("envelope_id").and_then(Value::as_str),
            Some(envelope_id.to_string().as_str())
        );
        assert_eq!(data.get("retry_safe").and_then(Value::as_bool), Some(false));
        assert_eq!(
            data.get("required_action").and_then(Value::as_str),
            Some("reconcile")
        );
    }

    // A durably rejected envelope is a terminal, unambiguous rejection: the
    // caller gets `peer_input_rejected` with the envelope id, as on RPC and
    // REST, not a plain `send_failed`.
    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_mcp_comms_send_error_names_durable_rejection() {
        let envelope_id = uuid::Uuid::new_v4();
        let err = normalize_mcp_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::DurableAdmissionRejected { envelope_id },
        );
        assert_eq!(err.code, -32603);
        assert!(
            err.message.starts_with("peer_input_rejected:"),
            "{}",
            err.message
        );
        let data = err.data.expect("structured error data");
        assert_eq!(
            data.get("code").and_then(Value::as_str),
            Some("peer_input_rejected")
        );
        assert_eq!(data.get("peer").and_then(Value::as_str), Some("peer-a"));
        assert_eq!(
            data.get("envelope_id").and_then(Value::as_str),
            Some(envelope_id.to_string().as_str())
        );
    }

    /// A `meerkat_run` whose reasoning effort level the route does not
    /// accept is refused by the session's build, before a session exists:
    /// the typed tool error names no created session, and no session is
    /// listed or stored afterwards. The test client reports no route, so
    /// every level is refused as unknown.
    #[tokio::test]
    async fn test_meerkat_run_refused_for_its_level_leaves_no_session() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(&store),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
        )
        .await;

        // The level in both forms a run names it in: the direct member and
        // the nested `provider_params` key.
        for (form, level) in [
            ("direct", json!({"reasoning_effort": "high"})),
            (
                "nested",
                json!({"provider_params": {"reasoning_effort": "high"}}),
            ),
        ] {
            let mut input = json!({
                "prompt": "Reply briefly.",
                "model": "claude-opus-4-8",
                "enable_builtins": false
            });
            for (member, value) in level.as_object().expect("a level form is an object") {
                input[member.as_str()] = value.clone();
            }
            let refused = Box::pin(handle_tools_call(&state, "meerkat_run", &input))
                .await
                .expect_err("a level the route does not accept is refused");
            assert_eq!(refused.code, -32602, "{form}: {}", refused.message);
            let data = refused.data.expect("the typed refusal data");
            assert_eq!(data["kind"], "reasoning_effort_refused", "{form}: {data}");
            assert_eq!(
                data["reasoning_effort_refusal"]["reason"], "supported_levels_unknown",
                "{form}: {data}"
            );
            assert_eq!(
                (data.get("session_id"), data.get("session_created")),
                (None, None),
                "{form}: the build refused the level before a session was created: {data}"
            );
        }

        let listed = Box::pin(handle_tools_call(&state, "meerkat_sessions", &json!({})))
            .await
            .expect("sessions list call should succeed");
        assert_eq!(
            unwrap_payload(listed)["sessions"],
            json!([]),
            "a run refused for its level leaves no session listed"
        );
        let stored = store
            .list(meerkat::SessionFilter::default())
            .await
            .expect("list the stored sessions");
        assert!(
            stored.is_empty(),
            "a run refused for its level stores no session: {} stored",
            stored.len()
        );
    }

    // Caller labels and application context on `meerkat_run`. A session's
    // top-level `labels` and `app_context` pass the shared metadata rule of
    // `meerkat_core::surface_metadata`, as on the JSON-RPC and REST session
    // create: a key Meerkat reserves for its own runtime facts is invalid
    // params with the rule's message, and no session is created.

    async fn run_metadata_state() -> (MeerkatMcpState, Arc<dyn SessionStore>) {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store_options_and_llm(
            Arc::clone(&store),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            None,
            Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
        )
        .await;
        (state, store)
    }

    /// The labels `meerkat_read` shows for the session a run result names.
    async fn run_session_labels(state: &MeerkatMcpState, run_payload: &Value) -> Value {
        let session_id = run_payload["session_id"]
            .as_str()
            .expect("run result names its session");
        let read = Box::pin(handle_tools_call(
            state,
            "meerkat_read",
            &json!({ "session_id": session_id }),
        ))
        .await
        .expect("read the run's session");
        unwrap_payload(read)["state"]["labels"].clone()
    }

    fn run_metadata_with_key(key: &str) -> Value {
        let mut metadata = serde_json::Map::new();
        metadata.insert("org".to_string(), json!("acme"));
        metadata.insert(key.to_string(), json!("caller-value"));
        Value::Object(metadata)
    }

    /// Each `(member, key, message)` case is a `meerkat_run` whose `member`
    /// (`labels` or `app_context`) carries `key`: every one is refused as
    /// invalid params with exactly `message`, and afterwards no session is
    /// listed or stored.
    async fn assert_run_metadata_refused(cases: Vec<(&str, &str, String)>) {
        let (state, store) = run_metadata_state().await;
        let mut wrong = Vec::new();
        for (member, key, message) in cases {
            let mut input = json!({
                "prompt": "Reply briefly.",
                "model": "claude-opus-4-8",
                "provider": "anthropic",
                "enable_builtins": false
            });
            input[member] = run_metadata_with_key(key);
            match Box::pin(handle_tools_call(&state, "meerkat_run", &input)).await {
                Err(err) if err.code == -32602 && err.message == message => {}
                Err(err) => wrong.push(format!(
                    "{member} key '{key}': answered {} '{}', wanted -32602 '{message}'",
                    err.code, err.message
                )),
                Ok(result) => {
                    let labels = run_session_labels(&state, &unwrap_payload(result)).await;
                    wrong.push(format!(
                        "{member} key '{key}': the run was accepted (wanted -32602 '{message}'); its session's labels read {labels}"
                    ));
                }
            }
        }
        let listed = Box::pin(handle_tools_call(&state, "meerkat_sessions", &json!({})))
            .await
            .expect("sessions list call should succeed");
        let listed = unwrap_payload(listed);
        assert!(
            wrong.is_empty(),
            "runs with a reserved key were not refused as the rule answers:\n{}",
            wrong.join("\n")
        );
        assert_eq!(
            listed["sessions"],
            json!([]),
            "a refused run leaves no session listed"
        );
        let stored = store
            .list(meerkat::SessionFilter::default())
            .await
            .expect("list the stored sessions");
        assert!(
            stored.is_empty(),
            "a refused run stores no session: {} stored",
            stored.len()
        );
    }

    #[tokio::test]
    async fn test_meerkat_run_refuses_reserved_label_keys_and_creates_no_session() {
        // The four mob discovery names, the Meerkat namespace, and the forms
        // the rule covers beyond them: another letter case, the bare token.
        let cases = [
            "mob_id",
            "role",
            "meerkat_id",
            "agent_identity",
            "meerkat.runtime_id",
            "Role",
            "MOB_ID",
            "meerkat",
        ]
        .into_iter()
        .map(|key| {
            (
                "labels",
                key,
                format!("metadata label key '{key}' is reserved for Meerkat-owned runtime facts"),
            )
        })
        .collect();
        assert_run_metadata_refused(cases).await;
    }

    #[tokio::test]
    async fn test_meerkat_run_refuses_reserved_app_context_keys_and_creates_no_session() {
        let cases = ["meerkat.runtime_id", "MEERKAT.runtime_id", "meerkat"]
            .into_iter()
            .map(|key| {
                (
                    "app_context",
                    key,
                    format!("app_context key '{key}' is reserved for Meerkat-owned runtime facts"),
                )
            })
            .collect();
        assert_run_metadata_refused(cases).await;
    }

    /// Ordinary labels and application context are taken as before: the run
    /// completes and its session is listed under the caller's labels. The
    /// mob discovery names are reserved as label keys only, so an
    /// application context may use them as top-level keys.
    #[tokio::test]
    async fn test_meerkat_run_keeps_ordinary_labels_and_app_context() {
        let (state, _store) = run_metadata_state().await;
        let result = Box::pin(handle_tools_call(
            &state,
            "meerkat_run",
            &json!({
                "prompt": "Reply briefly.",
                "model": "claude-opus-4-8",
                "provider": "anthropic",
                "enable_builtins": false,
                "labels": {"org": "acme", "tier": "gold"},
                "app_context": {"client_ref": {"view": "compact"}, "role": "reviewer"}
            }),
        ))
        .await
        .unwrap_or_else(|err| panic!("a run with ordinary labels completes: {}", err.message));
        assert_eq!(
            run_session_labels(&state, &unwrap_payload(result)).await,
            json!({"org": "acme", "tier": "gold"}),
            "the session carries the caller's labels"
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
                audience: Default::default(),
                name: "late_mcp_tool".into(),
                description: "Connected after agent construction".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                provenance: Some(meerkat_core::types::ToolProvenance {
                    kind: meerkat_core::types::ToolSourceKind::Mcp,
                    source_id: "late-server".into(),
                    artifact: None,
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

    /// An explicit resume refused by its agent build rolls the registration
    /// its transaction prepared back with the keep: a prompt an earlier
    /// process left queued in the session's durable runtime state, which the
    /// registration recovered, stays queued for the next start, not ended
    /// and with no requeue mark. RED before: the rollback stopped it.
    #[tokio::test]
    async fn refused_explicit_resume_keeps_a_recovered_prompt() {
        let (state, session_id) = state_with_persisted_session().await;
        let session_id = meerkat::SessionId::parse(&session_id).expect("session id");
        let store = state.service.runtime_store();
        // An earlier process registered the session and accepted a prompt no
        // executor ran, so the store holds the prompt queued.
        let earlier = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(Arc::clone(&store))
                .expect("the earlier process's runtime"),
        );
        earlier
            .prepare_bindings(session_id.clone())
            .await
            .expect("the earlier process registers the session");
        let (accepted, _) = earlier
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                    "queued when the process stopped",
                    None,
                )),
            )
            .await
            .expect("the registration holds the prompt");
        let meerkat_runtime::AcceptOutcome::Accepted {
            input_id: queued, ..
        } = accepted
        else {
            panic!("the registration accepts the prompt, got {accepted:?}");
        };
        let session = state
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the persisted session")
            .expect("the session is persisted");
        let ingress = state.runtime_ingress_context();

        let outcome = materialize_mcp_actor_transaction(
            &state.service,
            &state.runtime_adapter,
            &ingress,
            &session_id,
            McpActorMaterializationMode::Resume,
            None,
            move |bindings, _config| {
                std::future::ready(Ok::<_, ToolCallError>(McpActorMaterializationPlan {
                    request: mock_deferred_materialization_request(session, bindings, Some(-2)),
                    callback_config: runtime_ingress::McpCallbackConfig::Preserve,
                    router_to_publish: None,
                }))
            },
        )
        .await
        .expect("the refused resume reports");
        assert!(!outcome.committed, "the refused resume commits nothing");
        let refusal = outcome.result.as_ref().err();
        assert!(
            refusal.is_some_and(|error| error
                .to_string()
                .contains("max_inline_peer_notifications=-2")),
            "the agent build refuses the resume: {refusal:?}"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "the refused resume rolls its registration back"
        );
        let row = store
            .load_input_state(
                &meerkat_runtime::LogicalRuntimeId::for_session(&session_id),
                &queued,
            )
            .await
            .expect("load the prompt's row")
            .expect("the prompt's row stays durable");
        assert_eq!(
            (row.seed.phase, &row.seed.terminal_outcome),
            (meerkat_runtime::InputLifecycleState::Queued, &None),
            "the recovered prompt stays queued for the next start: {:?}",
            row.state.history
        );
        assert!(
            row.state.stop_requeue.is_none(),
            "the prompt is kept as the store holds it, with no requeue mark"
        );
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
        assert!(
            !state
                .service
                .has_live_session(&session_id)
                .await
                .expect("live actor lookup"),
            "exact release must remove only the uncommitted actor incarnation"
        );
        assert!(
            !state.runtime_adapter.contains_session(&session_id).await,
            "exact release must remove the provisional runtime registration"
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(session)
                        .expect("session snapshot should serialize")
                        .into(),
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
                model_fallback: None,
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
        seed_runtime_authority_session(&runtime_store, &session).await;

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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let request = CreateSessionRequest {
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
                        compaction_curator_override: None,
                        resume_session: Some(pre_session),
                        external_tools: Some(router_tools),
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                        )),
                        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                        ..Default::default()
                    }),
                    labels: None,
                };
                (request, router)
            },
        )
        .await;
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
        materialize_test_mcp_fixture(
            &state,
            &session_id,
            McpActorMaterializationMode::Fresh,
            move |bindings| {
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let request = CreateSessionRequest {
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
                        compaction_curator_override: None,
                        resume_session: Some(pre_session),
                        external_tools: Some(router_tools),
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(MockLlmClient) as Arc<dyn LlmClient>,
                        )),
                        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                        comms_name: Some("mcp-agent".to_string()),
                        keep_alive: false,
                        ..Default::default()
                    }),
                    labels: None,
                };
                (request, router)
            },
        )
        .await;
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_and_runtime_store(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                model_fallback: None,
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
        seed_runtime_authority_session(&runtime_store, &session).await;
        let result = Box::pin(handle_meerkat_resume(
            &state,
            MeerkatResumeInput {
                session_id: session_id.to_string(),
                prompt: "Resume".to_string(),
                transient_turn_context: None,
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
                reasoning_effort: None,
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
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let state = MeerkatMcpState::new_with_store_and_runtime_store(
            Arc::clone(&store),
            Arc::clone(&runtime_store),
        )
        .await;
        let mut session = Session::new();
        let session_id = session.id().clone();
        session
            .set_session_metadata(meerkat::SessionMetadata {
                model_fallback: None,
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
        seed_runtime_authority_session(&runtime_store, &session).await;
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
                transient_turn_context: None,
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
                reasoning_effort: None,
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

        input.system_prompt = Some("ordinary turn System".to_string());
        assert!(
            !mcp_resume_requires_rebuild(&input),
            "an explicit turn System is live input, not an actor build override"
        );
        input.system_prompt = None;

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

    /// A refused reasoning effort level is a typed tool error, as JSON-RPC
    /// answers it: invalid params with the typed refusal as the error's
    /// data, never an internal error whose text a host would have to read.
    /// After the call created its session the data also names the session.
    #[test]
    fn test_a_refused_reasoning_effort_is_a_typed_tool_error() {
        let session_id = meerkat::SessionId::new();
        for (reason, retryable) in [
            ("unsupported_level", false),
            ("supported_levels_unavailable", true),
        ] {
            let document = json!({
                "requested": "max", "provider": "anthropic", "model": "claude-opus-4-5",
                "route": {"backend": "anthropic_api", "api": "messages"},
                "reason": reason, "supported": null, "facts": null,
            });
            let refusal: meerkat_core::ReasoningEffortRefusal =
                serde_json::from_value(document.clone()).expect("a refusal document reads");
            let refused = || {
                SessionError::Agent(meerkat::AgentError::ReasoningEffortRefused(Box::new(
                    refusal.clone(),
                )))
            };
            let typed = json!({
                "kind": "reasoning_effort_refused",
                "retryable": retryable,
                "reasoning_effort_refusal": document,
            });

            let err = format_agent_result_tool(Err(refused()), &session_id)
                .expect_err("a refused level is a tool error");
            assert_eq!(err.code, -32602, "{reason}");
            assert_eq!(err.message, format!("agent error: {refusal}"), "{reason}");
            assert_eq!(err.data, Some(typed.clone()), "{reason}");

            let err = post_commit_session_created_error(&session_id, &refused());
            assert_eq!(err.code, -32602, "{reason}");
            let data = err.data.expect("the typed data");
            for (key, value) in typed.as_object().expect("an object") {
                assert_eq!(&data[key], value, "{reason}: {key}");
            }
            assert_eq!(data["session_id"], session_id.to_string(), "{reason}");
            assert_eq!(data["session_created"], true, "{reason}");
            assert_eq!(data["resumable"], true, "{reason}");

            // A reconfiguration's refusal reaches the same answer.
            let from_driver = runtime_driver_error_to_session_error(
                meerkat_runtime::RuntimeDriverError::ReasoningEffortRefused(Box::new(
                    refusal.clone(),
                )),
            );
            let err = format_agent_result_tool(Err(from_driver), &session_id)
                .expect_err("a refused level is a tool error");
            assert_eq!((err.code, err.data), (-32602, Some(typed)), "{reason}");
        }
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

    /// The realm profile tools take the public profile input and
    /// `auth_binding`: any other profile field is an argument error naming
    /// the field, and nothing is stored or changed.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_profile_tools_take_the_public_profile_input() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let with = |key: &str, value: Value| {
            let mut profile = json!({ "model": "claude-sonnet-4-6", "tools": { "comms": true } });
            if key == "auth_binding" {
                profile[key] = value;
            } else {
                profile["tools"][key] = value;
            }
            profile
        };
        let outside = [
            ("mcp_servers", json!([])),
            ("rust_bundles", json!(["internal-only"])),
        ];

        let bound = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_profile_create",
            &json!({
                "name": "bound",
                "profile": with("auth_binding", json!({ "realm": "global", "binding": "some-binding" })),
            }),
        ))
        .await
        .expect("a profile naming a binding is stored");
        assert_eq!(unwrap_tool_payload_json(bound)["revision"], 1);

        for (field, value) in &outside {
            let err = Box::pin(handle_tools_call(
                &state,
                "meerkat_mob_profile_create",
                &json!({ "name": "worker", "profile": with(field, value.clone()) }),
            ))
            .await
            .expect_err("a profile field outside the public input is refused");
            assert_eq!(err.code, -32602, "{field}: {}", err.message);
            assert!(err.message.contains(field), "{field}: {}", err.message);
        }
        let absent = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_profile_get",
            &json!({ "name": "worker" }),
        ))
        .await
        .expect("profile get should succeed");
        assert_eq!(unwrap_tool_payload_json(absent)["not_found"], true);

        let created = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_profile_create",
            &json!({ "name": "worker", "profile": with("comms", json!(true)) }),
        ))
        .await
        .expect("a public profile is stored");
        assert_eq!(unwrap_tool_payload_json(created)["revision"], 1);

        for (field, value) in &outside {
            let err = Box::pin(handle_tools_call(
                &state,
                "meerkat_mob_profile_update",
                &json!({
                    "name": "worker",
                    "profile": with(field, value.clone()),
                    "expected_revision": 1,
                }),
            ))
            .await
            .expect_err("a profile field outside the public input is refused");
            assert_eq!(err.code, -32602, "{field}: {}", err.message);
            assert!(err.message.contains(field), "{field}: {}", err.message);
        }
        let kept = Box::pin(handle_tools_call(
            &state,
            "meerkat_mob_profile_get",
            &json!({ "name": "worker" }),
        ))
        .await
        .expect("profile get should succeed");
        assert_eq!(unwrap_tool_payload_json(kept)["revision"], 1);
    }

    /// The unified server's `tools/call` answers a reserved label or context
    /// key on the two public mob spawn tools as invalid params with the
    /// shared metadata rule's message, and the mob seats nobody; ordinary
    /// labels and context are taken and stored.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_public_mcp_server_spawn_tools_refuse_reserved_metadata_keys() {
        async fn create_mob(state: &MeerkatMcpState, mob_id: &str) {
            Box::pin(handle_tools_call(
                state,
                "meerkat_mob_create",
                &json!({
                    "definition": {
                        "id": mob_id,
                        "profiles": {
                            "worker": { "model": "claude-sonnet-4-6", "tools": {"comms": true} }
                        }
                    }
                }),
            ))
            .await
            .expect("typed mob create should succeed");
        }
        fn with_key(key: &str) -> Value {
            let mut metadata = serde_json::Map::new();
            metadata.insert("org".to_string(), json!("acme"));
            metadata.insert(key.to_string(), json!("caller-value"));
            Value::Object(metadata)
        }
        // `meerkat_mob_spawn_many` gets an entry with no labels and no
        // context first, then the entry that carries them.
        fn arguments(tool: &str, mob_id: &str, labels: Value, context: Value) -> Value {
            if tool == "meerkat_mob_spawn" {
                json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "w-labelled",
                    "runtime_mode": "turn_driven",
                    "labels": labels,
                    "context": context,
                })
            } else {
                json!({
                    "mob_id": mob_id,
                    "specs": [
                        {
                            "profile": "worker",
                            "agent_identity": "w-plain",
                            "runtime_mode": "turn_driven",
                        },
                        {
                            "profile": "worker",
                            "agent_identity": "w-labelled",
                            "runtime_mode": "turn_driven",
                            "labels": labels,
                            "context": context,
                        },
                    ],
                })
            }
        }

        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;

        let mut cases = Vec::new();
        for tool in ["meerkat_mob_spawn", "meerkat_mob_spawn_many"] {
            for key in [
                "mob_id",
                "role",
                "meerkat_id",
                "agent_identity",
                "meerkat.runtime_id",
                "Role",
            ] {
                cases.push((
                    tool,
                    with_key(key),
                    Value::Null,
                    format!(
                        "metadata label key '{key}' is reserved for Meerkat-owned runtime facts"
                    ),
                ));
            }
            cases.push((
                tool,
                Value::Null,
                with_key("meerkat.runtime_id"),
                "app_context key 'meerkat.runtime_id' is reserved for Meerkat-owned runtime facts"
                    .to_string(),
            ));
        }
        let mut wrong = Vec::new();
        for (case, (tool, labels, context, message)) in cases.into_iter().enumerate() {
            let mob = format!("spawn-metadata-{case}");
            create_mob(&state, &mob).await;
            let outcome = Box::pin(handle_tools_call(
                &state,
                tool,
                &arguments(tool, &mob, labels, context),
            ))
            .await;
            let seated: Vec<String> = state
                .mob_state
                .mob_list_members(&meerkat_mob::MobId::from(mob.as_str()))
                .await
                .expect("list the mob's members")
                .into_iter()
                .map(|member| {
                    format!(
                        "'{}' with roster labels {:?}",
                        member.agent_identity, member.labels
                    )
                })
                .collect();
            match outcome {
                Err(err) if err.code == -32602 && err.message == message && seated.is_empty() => {}
                Err(err) => wrong.push(format!(
                    "{tool} answered {} '{}' (wanted -32602 '{message}'); seated: [{}]",
                    err.code,
                    err.message,
                    seated.join(", ")
                )),
                Ok(_) => wrong.push(format!(
                    "{tool} accepted the call (wanted -32602 '{message}'); seated: [{}]",
                    seated.join(", ")
                )),
            }
        }
        assert!(
            wrong.is_empty(),
            "calls with a reserved key were not refused as the rule answers:\n{}",
            wrong.join("\n")
        );

        for tool in ["meerkat_mob_spawn", "meerkat_mob_spawn_many"] {
            let mob = format!(
                "spawn-metadata-plain-{}",
                if tool == "meerkat_mob_spawn" {
                    "one"
                } else {
                    "many"
                }
            );
            create_mob(&state, &mob).await;
            Box::pin(handle_tools_call(
                &state,
                tool,
                &arguments(
                    tool,
                    &mob,
                    json!({"org": "acme", "tier": "gold"}),
                    json!({"client_ref": {"view": "compact"}, "role": "reviewer"}),
                ),
            ))
            .await
            .unwrap_or_else(|err| panic!("{tool} takes ordinary labels: {}", err.message));
            let members = state
                .mob_state
                .mob_list_members(&meerkat_mob::MobId::from(mob.as_str()))
                .await
                .expect("list the mob's members");
            let labelled = members
                .iter()
                .find(|member| member.agent_identity.as_str() == "w-labelled")
                .unwrap_or_else(|| panic!("{tool} seats the labelled member"));
            assert_eq!(
                labelled.labels,
                BTreeMap::from([
                    ("org".to_string(), "acme".to_string()),
                    ("tier".to_string(), "gold".to_string()),
                ]),
                "{tool}: roster labels"
            );
        }
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
        let (_parent_mob_id, member_session_id) =
            insert_mcp_archive_live_member(&mob_state, "mcp-session-archive-live-member").await;
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
                    compaction_curator_override: None,
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
                stop_reason: Some(meerkat_core::types::StopReason::EndTurn),
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
                assistant_message_id: None,
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
                stop_reason: Some(meerkat_core::types::StopReason::EndTurn),
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
                assistant_message_id: None,
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
        let archived_runtime_state = meerkat_runtime::store::load_runtime_state(
            runtime_store.as_ref(),
            &meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session.id()),
        )
        .await
        .expect("archive runtime state should remain readable");
        assert!(
            matches!(
                archived_runtime_state,
                Some(
                    meerkat_runtime::RuntimeState::Retired
                        | meerkat_runtime::RuntimeState::Destroyed
                )
            ),
            "archive must leave an absorbing runtime terminal, got {archived_runtime_state:?}"
        );

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
        let session = state
            .service
            .load_authoritative_session(&parsed)
            .await
            .expect("load MCP handler fixture")
            .expect("persisted MCP handler fixture");
        materialize_test_mcp_fixture(
            &state,
            &parsed,
            McpActorMaterializationMode::Resume,
            move |bindings| {
                seed_active_external_surface(bindings.external_tool_surface().as_ref(), "demo");
                let router = Arc::new(meerkat_mcp::McpRouterAdapter::new(
                    McpRouter::new_with_surface_handle_and_stdio_profile(
                        Arc::clone(bindings.external_tool_surface()),
                        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                    ),
                ));
                let router_tools: Arc<dyn AgentToolDispatcher> = router.clone();
                let mut request = mock_deferred_materialization_request(session, bindings, None);
                request
                    .build
                    .get_or_insert_with(Default::default)
                    .external_tools = Some(router_tools);
                (request, router)
            },
        )
        .await;

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
        assert_eq!(
            error.code,
            meerkat_contracts::ErrorCode::SessionNotRunning.jsonrpc_code()
        );
        assert!(error.message.contains("meerkat_resume"));
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|data| data.get("session_resume_required"))
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
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
        let (mob_id, _session_id) =
            insert_mcp_archive_live_member(&owner, "mcp-event-stream-scope-denial-member").await;

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

    /// rkat-mcp never hosts a stock-governed realm's schedules: it opens its
    /// realm only through the ordinary open, so its startup refuses a governed
    /// realm with `PersistenceError::RealmGoverned`. The control starts an
    /// ungoverned realm through the same constructor.
    #[tokio::test]
    async fn mcp_server_startup_refuses_a_governed_realm() {
        let temp = tempfile::tempdir().expect("tempdir");
        let realms_root = temp.path().join("realms");
        let bootstrap = |realm_id: &str| {
            let mut bootstrap = RuntimeBootstrap::default();
            bootstrap.realm.selection = RealmSelection::Explicit {
                realm_id: realm_id.into(),
            };
            bootstrap.realm.backend_hint = Some("sqlite".into());
            bootstrap.realm.state_root = Some(realms_root.clone());
            bootstrap.context.user_config_root = Some(temp.path().join("user"));
            bootstrap
        };
        let start = |bootstrap| {
            MeerkatMcpState::new_with_bootstrap_options_and_llm(
                bootstrap,
                false,
                Some(Arc::new(TestClient::for_provider(Provider::Anthropic))),
                None,
            )
        };
        meerkat_store::ensure_realm_manifest_in(
            &realms_root,
            "governed-mcp",
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
        )
        .await
        .expect("materialize the realm");
        meerkat_store::govern_realm_in(&realms_root, "governed-mcp")
            .await
            .expect("govern the realm");

        let control = start(bootstrap("ordinary-mcp")).await;
        assert!(
            control.is_ok(),
            "an ungoverned realm starts: {:?}",
            control.err().map(|error| error.to_string())
        );

        let refused = start(bootstrap("governed-mcp"))
            .await
            .err()
            .expect("a governed realm is refused at startup");
        assert!(
            matches!(
                refused.downcast_ref::<meerkat::PersistenceError>(),
                Some(meerkat::PersistenceError::RealmGoverned { .. })
            ),
            "{refused}"
        );
    }

    /// The server's own construction declares its inputs to its agents'
    /// in-process file tools. Started without a context root, the tools are
    /// rooted in the realm directory: the realm's documents are refused
    /// there, and so is the skills directory the factory reads from that
    /// root (`.rkat/skills`), also where it is a link to a directory beside
    /// it. An ordinary file is still written in the realm directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_file_tools_never_write_the_servers_own_documents() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().canonicalize().expect("canonical tempdir");
        let realms = base.join("realms");
        let realm_paths = meerkat_store::realm_paths_in(&realms, "file-tools");
        let realm_dir = realm_paths.root.clone();
        let document_text = "[realm.file-tools]\n";
        std::fs::create_dir_all(realm_dir.join(".rkat")).expect("realm dir");
        std::fs::create_dir_all(realm_dir.join("shared-skills")).expect("skills dir");
        std::os::unix::fs::symlink("../shared-skills", realm_dir.join(".rkat/skills"))
            .expect("link the skills directory");
        std::fs::write(&realm_paths.config_path, document_text).expect("realm document");
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.selection = RealmSelection::Explicit {
            realm_id: "file-tools".into(),
        };
        bootstrap.realm.state_root = Some(realms.clone());
        bootstrap.context.user_config_root = Some(base.join("user"));
        let llm = Arc::new(ToolCallScriptLlmClient::default());
        let state = MeerkatMcpState::new_with_bootstrap_options_and_llm(
            bootstrap,
            false,
            Some(Arc::clone(&llm) as Arc<dyn meerkat::LlmClient>),
            None,
        )
        .await
        .expect("the server starts");

        let add = |path: &str| {
            format!("*** Begin Patch\n*** Add File: {path}\n+written by the model\n*** End Patch")
        };
        let patches = [
            (
                "the realm config document",
                "config.toml",
                "*** Begin Patch\n*** Update File: config.toml\n@@\n+# appended by the model\n*** End Patch"
                    .to_string(),
                false,
            ),
            (
                "the grant owner store",
                "authorization/grant-owner.jsonl",
                add("authorization/grant-owner.jsonl"),
                false,
            ),
            (
                "the hooks the factory reads from the tool root",
                ".rkat/config.toml",
                add(".rkat/config.toml"),
                false,
            ),
            (
                "the skills directory behind the link",
                "shared-skills/planted/SKILL.md",
                add("shared-skills/planted/SKILL.md"),
                false,
            ),
            ("an ordinary file", "out.txt", add("out.txt"), true),
        ];
        for (case, path, patch, written) in patches {
            llm.arm_call("apply_patch", json!({ "patch": patch }));
            let run_input: MeerkatRunInput = serde_json::from_value(json!({
                "prompt": "Apply the patch.",
                "model": "claude-sonnet-4-5",
                "provider": "anthropic",
                "enable_workgraph": false,
            }))
            .expect("run input");
            Box::pin(handle_meerkat_run(&state, run_input, None, None))
                .await
                .expect("meerkat_run succeeds");
            assert!(
                llm.armed_call.lock().expect("armed call lock").is_none(),
                "{case}: the model's apply_patch call was made"
            );
            let target = realm_dir.join(path);
            if written {
                assert_eq!(
                    std::fs::read_to_string(&target).expect("the ordinary file"),
                    "written by the model\n",
                    "{case}"
                );
            } else if path == "config.toml" {
                assert_eq!(
                    std::fs::read_to_string(&target).expect("realm document"),
                    document_text,
                    "{case} was changed by the file tool"
                );
            } else {
                assert!(!target.exists(), "{case}: {path} was written");
            }
        }
    }
}
