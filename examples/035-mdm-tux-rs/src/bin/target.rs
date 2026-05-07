//! # 035 — MDM TUX: Target Agent (Runtime-Backed Surface)
//!
//! Runs on managed machines. Serves a JSON-RPC TCP server for TUX to
//! connect to (direct mode) or registers with a kennel for brokered
//! adoption. Comms is retained for inter-agent traffic (delegate
//! helpers, mob members).
//!
//! This target is a **runtime-backed surface** (same tier as CLI/RPC/REST):
//! - Agent construction via `AgentFactory::build_agent()`
//! - Session lifecycle via `PersistentSessionService`
//! - Input routing via `MeerkatMachine` with `HandlingMode::Steer`
//! - JSON-RPC over TCP for all command/control traffic
//!
//! Sessions are persisted to disk. On restart the most recent session
//! is automatically resumed.
//!
//! ```text
//! mdm-target <HOST:PORT> [--name NAME] [--rpc-port PORT] [--model MODEL]
//! mdm-target --kennel HOST:PORT [--advertise IP] [--rpc-port PORT] [--name NAME]
//! ```
//!
//! Set one of: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use meerkat::PersistentSessionService;
use meerkat::surface::{
    NoopScheduleMobHost, ScheduledPromptDispatch, SharedScheduleTargetAdapter,
    SurfaceScheduleSessionHost, schedule_attempt_idempotency_key, schedule_host_supported,
    spawn_schedule_host,
};
use meerkat::{
    AgentFactory, FactoryAgentBuilder, PersistenceBundle, ScheduleService, ScheduleToolDispatcher,
    SqliteScheduleStore,
};
use meerkat_comms::{CommsRuntime, PeerMeta, ResolvedCommsConfig, TrustedPeer};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunApplyBoundary, RunPrimitive,
};
use meerkat_core::mcp_config::McpConfig;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest,
};
use meerkat_core::types::{ContentInput, HandlingMode, SessionId};
use meerkat_core::{AgentToolDispatcher as _, Config, Session};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
use meerkat_mob::MobSessionService;
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
use meerkat_runtime::input::{InputDurability, InputHeader, InputVisibility};
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputOrigin, MeerkatMachine, PromptInput,
};
use meerkat_store::{JsonlStore, MemoryBlobStore, SessionFilter, SessionStore};

use mdm_tux::{
    DirectControlPayload, KennelPayload, ProviderKind, auto_detect, build_signed_envelope,
    direct_control_request, parse_direct_control_message, read_envelope, verify_envelope,
    write_envelope,
};
use tokio::io::BufReader;
use tokio::net::TcpStream;

const SYSTEM_PROMPT: &str = "\
You are a managed system agent named '{name}' controlled by a human operator via TUX.
Execute user requests using your available tools. Respond conversationally.
Your current session_id is '{session_id}'. If you schedule follow-up work for this same running agent session, use that exact session_id.
Your responses stream directly to the controller — do not use the 'send_message' comms tool to reply.";

struct TargetRuntimeSurface {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    jsonl_store: Arc<JsonlStore>,
    mob_state: Arc<MobMcpState>,
    _factory: Arc<AgentFactory>,
    _config: Config,
    _schedule_host: Option<meerkat::surface::ScheduleHostHandle>,
}

#[derive(Clone)]
struct TargetScheduleSessionHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<MeerkatMachine>,
    mob_state: Arc<MobMcpState>,
}

impl TargetScheduleSessionHost {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        runtime_adapter: Arc<MeerkatMachine>,
        mob_state: Arc<MobMcpState>,
    ) -> Self {
        Self {
            service,
            runtime_adapter,
            mob_state,
        }
    }

    fn executor(&self, session_id: SessionId) -> TargetCoreExecutor {
        TargetCoreExecutor::new(
            Arc::clone(&self.service),
            Arc::clone(&self.mob_state),
            session_id,
        )
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), meerkat::ScheduleDomainError> {
        let session_exists = self.service.read(session_id).await.is_ok()
            || self
                .service
                .load_persisted_session(session_id)
                .await
                .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?
                .is_some();
        if !session_exists {
            return Err(meerkat::ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        let executor = Box::new(self.executor(session_id.clone()));
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        Ok(())
    }

    async fn update_peer_ingress_context(&self, session_id: &SessionId) {
        let keep_alive = self
            .service
            .load_persisted_session(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .map(|metadata| metadata.keep_alive)
            })
            .unwrap_or(false);
        let comms_rt = self.service.comms_runtime(session_id).await;
        self.runtime_adapter
            .update_peer_ingress_context(session_id, keep_alive, comms_rt)
            .await;
    }
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn runtime_delivery_dispatch(
    occurrence: &meerkat::Occurrence,
    outcome: meerkat_runtime::accept::AcceptOutcome,
    handle: Option<meerkat_runtime::completion::CompletionHandle>,
    materialized_session_id: Option<SessionId>,
) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
    match outcome {
        meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => {
            Ok(meerkat::surface::build_dispatch_from_accepted(
                occurrence,
                meerkat::surface::AcceptedScheduledInput {
                    correlation_id: Some(input_id.to_string()),
                    handle,
                },
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Deduplicated { existing_id, .. } => {
            Ok(meerkat::surface::build_dispatch_from_accepted(
                occurrence,
                meerkat::surface::AcceptedScheduledInput {
                    correlation_id: Some(existing_id.to_string()),
                    handle: None,
                },
                materialized_session_id,
            ))
        }
        meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
            Ok(meerkat::surface::immediate_delivery_failure(
                occurrence,
                reason.to_string(),
                meerkat::OccurrenceFailureClass::RuntimeRejected,
                None,
                materialized_session_id,
            ))
        }
        _ => Ok(meerkat::surface::immediate_delivery_failure(
            occurrence,
            "runtime returned an unknown admission outcome".to_string(),
            meerkat::OccurrenceFailureClass::RuntimeRejected,
            None,
            materialized_session_id,
        )),
    }
}

#[async_trait::async_trait]
impl SurfaceScheduleSessionHost for TargetScheduleSessionHost {
    async fn probe_session_target(
        &self,
        binding: &meerkat::SessionTargetBinding,
    ) -> Result<meerkat::TargetProbeOutcome, meerkat::ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(meerkat::TargetProbeOutcome::Ready);
        };

        if let Ok(view) = self.service.read(session_id).await {
            return Ok(if view.state.is_active {
                meerkat::TargetProbeOutcome::Busy {
                    detail: Some(format!("session still running: {session_id}")),
                }
            } else {
                meerkat::TargetProbeOutcome::Ready
            });
        }

        let persisted = self
            .service
            .load_persisted_session(session_id)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;
        match persisted {
            Some(session) if !session_metadata_marks_archived(&session) => {
                Ok(meerkat::TargetProbeOutcome::Ready)
            }
            _ => Ok(meerkat::TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_session(
        &self,
        create: &meerkat::SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, meerkat::ScheduleDomainError> {
        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        let build = SessionBuildOptions {
            provider: create.provider,
            output_schema: create.output_schema.clone(),
            structured_output_retries: create.structured_output_retries,
            comms_name: create.comms_name.clone(),
            peer_meta: create.peer_meta.clone(),
            resume_session: Some(session),
            provider_params: create.provider_params.clone(),
            preload_skills: (!create.preload_skills.is_empty()).then(|| {
                create
                    .preload_skills
                    .iter()
                    .cloned()
                    .map(Into::into)
                    .collect()
            }),
            additional_instructions: (!create.additional_instructions.is_empty())
                .then(|| create.additional_instructions.clone()),
            realm_id: create.realm_id.clone(),
            instance_id: create.instance_id.clone(),
            backend: create.backend.clone(),
            keep_alive: create.keep_alive,
            app_context: create.app_context.clone(),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..SessionBuildOptions::default()
        };

        let result = self
            .service
            .create_session(CreateSessionRequest {
                model: create.model.clone(),
                prompt: ContentInput::Text(String::new()),
                render_metadata: None,
                system_prompt: prompt_system_prompt
                    .map(str::to_owned)
                    .or_else(|| create.system_prompt.clone()),
                max_tokens: create.max_tokens,
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: Some(create.labels.clone()),
            })
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        self.runtime_adapter
            .ensure_session_with_executor(
                result.session_id.clone(),
                Box::new(self.executor(result.session_id.clone())),
            )
            .await;

        Ok(result.session_id)
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &meerkat::Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;
        self.update_peer_ingress_context(session_id).await;

        let turn_metadata = Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: None,
                keep_alive: None,
                skill_references: (!dispatch.skill_refs.is_empty()).then(|| {
                    dispatch
                        .skill_refs
                        .iter()
                        .map(|skill_ref| skill_ref.key().clone())
                        .collect()
                }),
                flow_tool_overlay: None,
                additional_instructions: (!dispatch.additional_instructions.is_empty())
                    .then(|| {
                        dispatch
                            .additional_instructions
                            .iter()
                            .map(|body| {
                                meerkat_core::lifecycle::run_primitive::TurnInstruction {
                                    kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::Host,
                                    body: body.clone(),
                                }
                            })
                            .collect()
                    }),
                model: None,
                provider: None,
                provider_params: None,
                render_metadata: dispatch.render_metadata.clone(),
                execution_kind: None,
                peer_response_terminal_apply_intent: None,
                auth_binding: None,
            },
        );
        let mut prompt_input = PromptInput::from_content_input(dispatch.prompt, turn_metadata);
        prompt_input.header.source = InputOrigin::System;
        prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
            schedule_attempt_idempotency_key(occurrence),
        ));
        prompt_input.header.correlation_id =
            Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        runtime_delivery_dispatch(
            occurrence,
            outcome,
            handle,
            dispatch.materialized_session_id,
        )
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &meerkat::Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<meerkat::DeliveryDispatch, meerkat::ScheduleDomainError> {
        self.ensure_runtime_session_registered(session_id).await?;
        self.update_peer_ingress_context(session_id).await;

        let input = Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::External {
                    source_name: format!("schedule:{}", occurrence.schedule_id),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: Some(IdempotencyKey::new(schedule_attempt_idempotency_key(
                    occurrence,
                ))),
                supersession_key: None,
                correlation_id: Some(CorrelationId::from_uuid(occurrence.occurrence_id.0)),
            },
            event_type,
            payload,
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata,
        });

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|error| meerkat::ScheduleDomainError::Internal(error.to_string()))?;

        runtime_delivery_dispatch(occurrence, outcome, handle, materialized_session_id)
    }
}

async fn build_target_runtime_surface(
    session_dir: &Path,
    comms_runtime: Arc<CommsRuntime>,
) -> anyhow::Result<TargetRuntimeSurface> {
    let factory = AgentFactory::new(session_dir)
        .shell(true)
        .builtins(true)
        .comms(true)
        .schedule(true)
        .mob(true)
        .with_comms_runtime(comms_runtime);
    let schedule_store = Arc::new(SqliteScheduleStore::open(
        session_dir.join("schedule.sqlite"),
    )?) as Arc<dyn meerkat::ScheduleStore>;
    let schedule_service = ScheduleService::new(Arc::clone(&schedule_store));
    let home = dirs::home_dir();
    let config = Config::load_from(session_dir, home.as_deref())
        .await
        .unwrap_or_default();
    let shared_factory = Arc::new(factory.clone());
    let shared_config = config.clone();
    let builder = FactoryAgentBuilder::new(factory, config);
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    *builder
        .default_schedule_tools
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        ScheduleToolDispatcher::new(schedule_service.clone()),
    ));

    let jsonl_store = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
    jsonl_store.init().await?;

    let persistence = PersistenceBundle::new_with_schedule_store(
        Arc::clone(&jsonl_store) as Arc<dyn SessionStore>,
        None,
        Arc::new(MemoryBlobStore::new()),
        schedule_store,
    );
    let runtime_adapter = persistence.runtime_adapter();
    let (session_store, runtime_store, blob_store) = persistence.into_parts();

    let session_service =
        PersistentSessionService::new(builder, 10, session_store, runtime_store, blob_store);
    let service = Arc::new(session_service);
    let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
        service.clone(),
        Some(runtime_adapter.clone()),
    ));
    *mob_tools_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));
    let schedule_host = if schedule_host_supported(schedule_service.store().kind()) {
        let session_host: Arc<dyn SurfaceScheduleSessionHost> =
            Arc::new(TargetScheduleSessionHost::new(
                service.clone(),
                runtime_adapter.clone(),
                mob_state.clone(),
            ));
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            schedule_service.clone(),
            session_host,
            Arc::new(NoopScheduleMobHost::new(
                "scheduled mob targets are not enabled in mdm-target",
            )),
        ));
        Some(spawn_schedule_host(
            schedule_service,
            shared_adapter,
            "mdm-target",
        ))
    } else {
        None
    };

    Ok(TargetRuntimeSurface {
        service,
        runtime_adapter,
        jsonl_store,
        mob_state,
        _factory: shared_factory,
        _config: shared_config,
        _schedule_host: schedule_host,
    })
}

async fn discard_live_session_with_mob_cleanup(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    mob_state: &Arc<MobMcpState>,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    let discard_result = service.discard_live_session(session_id).await;
    if let Err(error) = mob_state
        .destroy_bridge_session_mobs(&session_id.to_string())
        .await
    {
        eprintln!("[target] warning: cleanup mobs for session {session_id}: {error}");
    }
    discard_result
}

/// Build a `ResolvedCommsConfig` for the target's stable identity.
fn target_comms_config(name: &str, data_dir: &Path) -> ResolvedCommsConfig {
    ResolvedCommsConfig {
        enabled: true,
        name: name.to_string(),
        inproc_namespace: None,
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: data_dir.join("identity"),
        trusted_peers_path: data_dir.join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: true,
        allow_external_unauthenticated: false,
    }
}

/// Create a `CommsRuntime` with the target's stable identity and a blob store.
async fn create_target_comms_runtime(
    name: &str,
    data_dir: &Path,
) -> anyhow::Result<Arc<CommsRuntime>> {
    let mut runtime = CommsRuntime::new_with_silent_intents(
        target_comms_config(name, data_dir),
        Arc::new(std::collections::HashSet::new()),
    )
    .await
    .map_err(|e| anyhow::anyhow!("comms runtime: {e}"))?;
    runtime.set_blob_store(Arc::new(MemoryBlobStore::new()));
    Ok(Arc::new(runtime))
}

/// Bind a TCP listener on a random port and spawn the accept loop using
/// the CommsRuntime's router primitives. Returns the bound port.
async fn spawn_comms_listener(comms_runtime: &Arc<CommsRuntime>) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let local_addr = listener.local_addr()?;
    let keypair = comms_runtime.router_arc().keypair_arc();
    let trusted = comms_runtime.trusted_peers_shared();
    let inbox_sender = comms_runtime.router_arc().inbox_sender().clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (kp, tp, sender) = (keypair.clone(), trusted.clone(), inbox_sender.clone());
            tokio::spawn(async move {
                let _ = meerkat_comms::handle_connection(stream, true, &kp, &tp, &sender).await;
            });
        }
    });
    Ok(local_addr.port())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    // ── Parse CLI args ────────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!(
            "Usage: mdm-target <HOST:PORT> [--name NAME] [--model MODEL --provider PROVIDER]"
        );
        eprintln!(
            "   or: mdm-target --kennel HOST:PORT [--advertise IP] [--rpc-port PORT] [--name NAME] [--model MODEL --provider PROVIDER]"
        );
        eprintln!("Set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY");
        std::process::exit(1);
    }
    if find_flag(&args, "--kennel").is_some() {
        return run_kennel_mode(&args).await;
    }
    let name = find_flag(&args, "--name")
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let provider_override = parse_provider_override(&args)?;
    let (_model, provider) = match find_flag(&args, "--model") {
        Some(m) => {
            let p = provider_override
                .context("--model requires --provider in override mode")?
                .to_string();
            (m, p)
        }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p.to_string()),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };

    let data_dir = find_flag(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!(".rkat/mdm/targets/{name}"))
        });

    // ── 1. Create CommsRuntime with target's stable identity ───────────────
    let comms_runtime = create_target_comms_runtime(&name, &data_dir).await?;

    // ── 2. Bind TCP listener (0.0.0.0:0 for port discovery) ─────────────────
    let comms_port = spawn_comms_listener(&comms_runtime).await?;

    // ── 3. Build runtime-backed session service ───────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let _surface = build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime)).await?;

    // ── 4. RPC TCP server (replaces old comms-based command protocol) ────────
    let rpc_port: u16 = find_flag(&args, "--rpc-port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(4800);
    let rpc_factory = AgentFactory::new(&session_dir)
        .shell(true)
        .builtins(true)
        .comms(true)
        .schedule(true)
        .mob(true)
        .with_comms_runtime(Arc::clone(&comms_runtime));
    let home = dirs::home_dir();
    let rpc_config = Config::load_from(&session_dir, home.as_deref())
        .await
        .unwrap_or_default();
    let rpc_schedule_store = Arc::new(SqliteScheduleStore::open(
        session_dir.join("rpc_schedule.sqlite"),
    )?) as Arc<dyn meerkat::ScheduleStore>;
    let rpc_jsonl = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
    rpc_jsonl.init().await?;
    let rpc_persistence = PersistenceBundle::new_with_schedule_store(
        rpc_jsonl as Arc<dyn SessionStore>,
        None,
        Arc::new(MemoryBlobStore::new()),
        rpc_schedule_store,
    );
    let rpc_config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(meerkat_core::MemoryConfigStore::new(rpc_config.clone()));
    let rpc_runtime = Arc::new(meerkat_rpc::session_runtime::SessionRuntime::new(
        rpc_factory,
        rpc_config,
        10,
        rpc_persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    ));

    println!("=== MDM Target: {name} ===");
    println!("rpc       : tcp://0.0.0.0:{rpc_port}");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("provider  : {provider}");

    let addr = format!("0.0.0.0:{rpc_port}");
    eprintln!("[target] ready — serving RPC on {addr}\n");
    meerkat_rpc::serve_tcp(&addr, rpc_runtime, rpc_config_store, None).await?;
    Ok(())
}

use mdm_tux::machines::target_attachment::{Effect as TaEffect, State as TaState};
use mdm_tux::machines::target_kennel_control::{
    self, Effect as TkcEffect, Event as TkcEvent, State as TkcState,
};
use mdm_tux::machines::target_kennel_session::{self, Event as TksEvent, State as TksState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAttachmentHint {
    tux_id: String,
    lease_id: String,
}

async fn load_attachment_hint(path: &Path) -> anyhow::Result<Option<PersistedAttachmentHint>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("parse attachment hint")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("read attachment hint"),
    }
}

async fn save_attachment_hint(path: &Path, hint: &PersistedAttachmentHint) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(hint).context("serialize attachment hint")?;
    tokio::fs::write(path, bytes)
        .await
        .context("write attachment hint")
}

async fn clear_attachment_hint(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("remove attachment hint"),
    }
}

/// Spawn a background heartbeat task that pings the peer every 10 seconds
/// via comms Request. Used by the attachment protocol to maintain the direct
/// TUX link. On 3 consecutive failures, triggers the disconnect signal.
fn spawn_heartbeat(
    router: Arc<meerkat_comms::Router>,
    peer: meerkat_core::comms::PeerId,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            let send_fut = router.send(
                peer,
                meerkat_comms::MessageKind::Request {
                    intent: "heartbeat".into(),
                    params: serde_json::json!(null),
                    handling_mode: None,
                },
            );
            let failed =
                match tokio::time::timeout(std::time::Duration::from_secs(5), send_fut).await {
                    Ok(Ok(_)) => false,
                    Ok(Err(_)) => true,
                    Err(_) => true,
                };
            if failed {
                consecutive_failures += 1;
                eprintln!("[target] heartbeat failed ({consecutive_failures}/3)");
                if consecutive_failures >= 3 {
                    let _ = disconnect_tx.send(true);
                    break;
                }
            } else {
                consecutive_failures = 0;
            }
        }
    })
}

#[allow(dead_code)] // Will be used when all effect processing is centralized
async fn apply_target_attachment_effects(
    comms_runtime: &Arc<CommsRuntime>,
    effects: &[TaEffect],
    heartbeat: &mut Option<tokio::task::JoinHandle<()>>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<bool> {
    let router = comms_runtime.router_arc();
    for effect in effects {
        match effect {
            TaEffect::EnsureTuxPeer {
                tux_pubkey,
                tux_direct_addr,
            } => {
                let tux_pk = meerkat_comms::identity::PubKey::from_pubkey_string(tux_pubkey)
                    .context("parse adopted tux pubkey")?;
                let trusted = comms_runtime.trusted_peers_shared();
                let stale_keys: Vec<_> = trusted
                    .read()
                    .peers
                    .iter()
                    .filter(|p| p.name == "tux")
                    .map(|p| p.pubkey)
                    .collect();
                for pk in stale_keys {
                    router.remove_trusted_peer(&pk.to_peer_id());
                }
                comms_runtime
                    .register_trusted_peer(TrustedPeer {
                        name: "tux".into(),
                        pubkey: tux_pk,
                        addr: tux_direct_addr.clone(),
                        meta: PeerMeta::default(),
                    })
                    .await?;
            }
            TaEffect::PersistAttachmentHint { tux_id, lease_id } => {
                let next = PersistedAttachmentHint {
                    tux_id: tux_id.clone(),
                    lease_id: lease_id.clone(),
                };
                save_attachment_hint(attachment_hint_path, &next).await?;
                *attachment_hint = Some(next);
            }
            TaEffect::ClearAttachmentHint => {
                clear_attachment_hint(attachment_hint_path).await?;
                *attachment_hint = None;
            }
            TaEffect::SendAttachRequest {
                target_id,
                lease_id,
            } => {
                let Ok(kind) = direct_control_request(&DirectControlPayload::AttachRequest {
                    lease_id: lease_id.clone(),
                    target_id: target_id.clone(),
                }) else {
                    return Ok(false);
                };
                let Some(tux_peer) = attachment_hint
                    .as_ref()
                    .and_then(|hint| meerkat_core::comms::PeerId::parse(&hint.tux_id).ok())
                else {
                    return Ok(false);
                };
                let attach_fut = router.send(tux_peer, kind);
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(10), attach_fut).await,
                    Ok(Ok(_))
                ) {
                    return Ok(false);
                }
            }
            TaEffect::StartDirectHeartbeat => {
                if heartbeat.is_none() {
                    let Some(tux_peer) = attachment_hint
                        .as_ref()
                        .and_then(|hint| meerkat_core::comms::PeerId::parse(&hint.tux_id).ok())
                    else {
                        return Ok(false);
                    };
                    *heartbeat = Some(spawn_heartbeat(
                        router.clone(),
                        tux_peer,
                        disconnect_tx.clone(),
                    ));
                }
            }
            TaEffect::StopDirectHeartbeat => {
                if let Some(hb) = heartbeat.take() {
                    hb.abort();
                }
            }
            TaEffect::ReregisterWithHint { .. } | TaEffect::ReregisterClean => {}
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn apply_target_control_effects(
    comms_runtime: &Arc<CommsRuntime>,
    effects: &[TkcEffect],
    heartbeat: &mut Option<tokio::task::JoinHandle<()>>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<(bool, bool)> {
    let mut attachment_effects = Vec::new();
    let mut should_return_to_register = false;
    for effect in effects {
        match effect {
            TkcEffect::Attachment(effect) => attachment_effects.push(effect.clone()),
            TkcEffect::ReturnToRegisterLoop => should_return_to_register = true,
        }
    }
    let applied = apply_target_attachment_effects(
        comms_runtime,
        &attachment_effects,
        heartbeat,
        disconnect_tx,
        attachment_hint,
        attachment_hint_path,
    )
    .await?;
    Ok((applied, should_return_to_register))
}

// ── Session lifecycle ────────────────────────────────────────────────────────

/// Create a new session or resume an existing one. Registers the session
/// with the MeerkatMachine and subscribes to its events.
async fn create_or_resume_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<MeerkatMachine>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    comms_runtime: Option<Arc<meerkat_comms::CommsRuntime>>,
) -> anyhow::Result<SessionId> {
    // Try to auto-resume the most recent session. On failure, warn and start fresh.
    if let Ok(mut sessions) = jsonl_store.list(SessionFilter::default()).await {
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if let Some(latest) = sessions.first() {
            eprintln!(
                "[target] auto-resuming session {} ({} messages)",
                latest.id, latest.message_count
            );
            match setup_session(
                service,
                runtime_adapter,
                Some(latest.id.clone()),
                model,
                system_prompt,
                mob_state,
                provider,
                external_tools.clone(),
                comms_runtime.clone(),
            )
            .await
            {
                Ok(sid) => return Ok(sid),
                Err(e) => {
                    eprintln!("[target] auto-resume failed: {e} — starting fresh session");
                }
            }
        }
    }

    eprintln!("[target] starting fresh session");
    setup_session(
        service,
        runtime_adapter,
        None,
        model,
        system_prompt,
        mob_state,
        provider,
        external_tools,
        comms_runtime,
    )
    .await
}

/// Create or resume a session and register it with the runtime adapter.
async fn setup_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<MeerkatMachine>,
    resume_id: Option<SessionId>,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    comms_runtime: Option<Arc<meerkat_comms::CommsRuntime>>,
) -> anyhow::Result<SessionId> {
    let resume_session = match &resume_id {
        Some(id) => {
            let loaded = service
                .load_persisted_session(id)
                .await
                .map_err(|e| anyhow::anyhow!("load session {id}: {e}"))?;
            Some(loaded.ok_or_else(|| anyhow::anyhow!("session {id} not found on disk"))?)
        }
        None => Some(Session::new()),
    };
    let prepared_session = resume_session
        .clone()
        .ok_or_else(|| anyhow::anyhow!("target setup requires a prepared session"))?;
    let prepared_session_id = prepared_session.id().clone();
    let system_prompt = system_prompt.replace("{session_id}", &prepared_session_id.to_string());

    // Prepare canonical runtime bindings (registers + allocates epoch in one shot).
    let bindings = runtime_adapter
        .prepare_bindings(prepared_session_id.clone())
        .await
        .map_err(|e| anyhow::anyhow!("runtime bindings: {e}"))?;

    let build_opts = SessionBuildOptions {
        provider: Some(meerkat_core::Provider::from_name(provider)),
        override_builtins: meerkat_core::ToolCategoryOverride::Enable,
        override_shell: meerkat_core::ToolCategoryOverride::Enable,
        override_mob: meerkat_core::ToolCategoryOverride::Enable,
        resume_session: Some(prepared_session),
        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
        external_tools,
        ..Default::default()
    };

    let req = CreateSessionRequest {
        model: model.to_string(),
        prompt: ContentInput::Text(String::new()),
        render_metadata: None,
        system_prompt: Some(system_prompt.to_string()),
        max_tokens: None,
        event_tx: None,
        skill_references: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build_opts),
        labels: None,
    };

    let result = match service.create_session(req).await {
        Ok(result) => result,
        Err(error) => {
            runtime_adapter
                .unregister_session(&prepared_session_id)
                .await;
            return Err(anyhow::anyhow!("create session: {error}"));
        }
    };

    let session_id = result.session_id;
    eprintln!("[target] session ready: {session_id}");

    // Create executor and register with runtime adapter for Steer support
    let executor = Box::new(TargetCoreExecutor::new(
        service.clone(),
        mob_state.clone(),
        session_id.clone(),
    ));
    runtime_adapter
        .ensure_session_with_executor(session_id.clone(), executor)
        .await;

    // Enable peer ingress for this session so the hive can reach us via comms.
    // This is the one session the mob manages for this target — TUX resets go
    // through mob/respawn, not session/create, so this wiring is stable.
    if let Some(comms) = comms_runtime {
        runtime_adapter
            .update_peer_ingress_context(
                &session_id,
                true,
                Some(comms as Arc<dyn meerkat_core::agent::CommsRuntime>),
            )
            .await;
    }

    Ok(session_id)
}

// ── CoreExecutor for runtime loop ────────────────────────────────────────────

/// Bridges the MeerkatMachine's runtime loop to the PersistentSessionService.
/// When the runtime loop dequeues an input (via DefaultPolicyTable routing),
/// it calls `apply()` which translates the RunPrimitive into a `start_turn()`.
struct TargetCoreExecutor {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    mob_state: Arc<MobMcpState>,
    session_id: SessionId,
}

impl TargetCoreExecutor {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        mob_state: Arc<MobMcpState>,
        session_id: SessionId,
    ) -> Self {
        Self {
            service,
            mob_state,
            session_id,
        }
    }
}

struct TargetCoreBoundaryHandle {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for TargetCoreBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary(&self.session_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

struct TargetCoreInterruptHandle {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for TargetCoreInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .interrupt(&self.session_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

fn render_runtime_context_append_text(content: &CoreRenderable) -> String {
    match content {
        CoreRenderable::Text { text } => text.clone(),
        CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
        CoreRenderable::Json { value } => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        CoreRenderable::Reference { uri, label } => match label {
            Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
            _ => format!("[Reference] {uri}"),
        },
        _ => String::new(),
    }
}

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            text: render_runtime_context_append_text(&append.content),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
        })
        .collect()
}

fn start_turn_request_from_primitive(
    primitive: &RunPrimitive,
) -> Result<StartTurnRequest, CoreExecutorError> {
    if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
        return Err(CoreExecutorError::apply_failed_primitive_rejected(
            reason.to_string(),
        ));
    }
    let metadata = primitive.turn_metadata();
    let pre_turn_context_appends = match primitive {
        RunPrimitive::StagedInput(staged) if primitive.is_peer_response_terminal_context_and_run() => {
            pending_system_context_appends(&staged.context_appends)
        }
        _ => Vec::new(),
    };
    Ok(StartTurnRequest {
        prompt: primitive.extract_content_input(),
        system_prompt: None,
        render_metadata: metadata.and_then(|meta| meta.render_metadata.clone()),
        handling_mode: metadata
            .and_then(|meta| meta.handling_mode)
            .unwrap_or(HandlingMode::Queue),
        event_tx: None,
        skill_references: metadata.and_then(|meta| meta.skill_references.clone()),
        flow_tool_overlay: metadata.and_then(|meta| meta.flow_tool_overlay.clone()),
        pre_turn_context_appends,
        turn_metadata: metadata.cloned(),
    })
}

#[async_trait::async_trait]
impl CoreExecutor for TargetCoreExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(TargetCoreBoundaryHandle {
            service: Arc::clone(&self.service),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(TargetCoreInterruptHandle {
            service: Arc::clone(&self.service),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let req = start_turn_request_from_primitive(&primitive)?;

        let boundary = match &primitive {
            RunPrimitive::StagedInput(staged) => staged.boundary,
            _ => RunApplyBoundary::Immediate,
        };
        let input_ids = primitive.contributing_input_ids().to_vec();

        self.service
            .apply_runtime_turn(&self.session_id, run_id, req, boundary, input_ids)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary(&self.session_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(|e| CoreExecutorError::control_failed_runtime(e.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        let discard_result = discard_live_session_with_mob_cleanup(
            &self.service,
            &self.mob_state,
            &self.session_id,
        )
        .await;
        runtime_adapter_unregister_noop();
        match discard_result {
            Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
            Err(err) => Err(CoreExecutorError::control_failed_runtime(err.to_string())),
        }
    }
}

/// Placeholder for StopRuntimeExecutor — the target owns the adapter lifetime
/// so explicit unregistration isn't needed during shutdown.
fn runtime_adapter_unregister_noop() {}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ActiveAdoption {
    lease_id: String,
    target_id: String,
    tux_id: String,
    tux_pubkey: String,
    tux_direct_addr: String,
}

/// Load MCP tools from `~/.rkat/mcp.toml` (user scope) and optionally
/// `<data_dir>/.rkat/mcp.toml` (per-target project scope).
/// Returns `None` when no servers are configured.
async fn load_mcp_tools(data_dir: Option<&Path>) -> anyhow::Result<Option<McpRouterAdapter>> {
    // data_dir as project scope, home dir as user scope
    let home = dirs::home_dir();
    let servers = McpConfig::load_with_scopes_from_roots(data_dir, home.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("MCP config: {e}"))?;
    if servers.is_empty() {
        return Ok(None);
    }
    eprintln!("[target] staging {} MCP server(s)", servers.len());
    let mut router = McpRouter::new();
    for s in &servers {
        eprintln!("[target]   - {}", s.server.name);
        router
            .stage_add(s.server.clone())
            .map_err(|e| anyhow::anyhow!("MCP stage: {e}"))?;
    }
    router
        .apply_staged()
        .await
        .map_err(|e| anyhow::anyhow!("MCP apply: {e}"))?;
    let adapter = McpRouterAdapter::new(router);
    // Wait up to 30s for servers to connect
    let _ = adapter
        .wait_until_ready(std::time::Duration::from_secs(30))
        .await;
    adapter
        .refresh_tools()
        .await
        .map_err(|e| anyhow::anyhow!("MCP refresh: {e}"))?;
    Ok(Some(adapter))
}

async fn run_kennel_mode(args: &[String]) -> anyhow::Result<()> {
    let kennel_addr = find_flag(args, "--kennel").context("--kennel HOST:PORT is required")?;
    let name = find_flag(args, "--name")
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let provider_override = parse_provider_override(args)?;
    let (model, provider) = match find_flag(args, "--model") {
        Some(m) => {
            let p = provider_override
                .context("--model requires --provider in override mode")?
                .to_string();
            (m, p)
        }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p.to_string()),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };
    let data_dir = find_flag(args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!(".rkat/mdm/targets/{name}"))
        });

    // Load MCP servers from ~/.rkat/mcp.toml (user) + <data_dir>/.rkat/mcp.toml (per-target)
    let mcp_external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>> =
        match load_mcp_tools(Some(&data_dir)).await {
            Ok(Some(adapter)) => {
                let adapter = Arc::new(adapter);
                eprintln!("[target] MCP tools loaded: {}", adapter.tools().len());
                Some(adapter)
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("[target] MCP load failed (continuing without): {e}");
                None
            }
        };

    let comms_runtime = create_target_comms_runtime(&name, &data_dir).await?;
    let comms_port = spawn_comms_listener(&comms_runtime).await?;
    let target_id = comms_runtime.public_key().to_peer_id().to_string();
    let target_pubkey = comms_runtime.public_key().to_pubkey_string();
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let surface = build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime)).await?;
    let _schedule_host_guard = surface._schedule_host.as_ref();
    let service = Arc::clone(&surface.service);
    let runtime_adapter = Arc::clone(&surface.runtime_adapter);
    let jsonl_store = Arc::clone(&surface.jsonl_store);
    let mob_state = Arc::clone(&surface.mob_state);
    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);
    let current_session_id = create_or_resume_session(
        &service,
        &runtime_adapter,
        &jsonl_store,
        &model,
        &system_prompt,
        &mob_state,
        &provider,
        mcp_external_tools.as_ref().cloned(),
        None,
    )
    .await?;

    let explicit_ip = find_flag(args, "--advertise");
    let attachment_hint_path = data_dir.join("attachment_hint.json");

    // ── RPC TCP server ────────────────────────────────────────────────────
    let rpc_port: u16 = find_flag(args, "--rpc-port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(4800);
    let rpc_session_runtime = {
        let rpc_factory = AgentFactory::new(&session_dir)
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(Arc::clone(&comms_runtime));
        let home = dirs::home_dir();
        let rpc_config = Config::load_from(&session_dir, home.as_deref())
            .await
            .unwrap_or_default();
        let rpc_schedule_store = Arc::new(SqliteScheduleStore::open(
            session_dir.join("rpc_schedule.sqlite"),
        )?) as Arc<dyn meerkat::ScheduleStore>;
        let rpc_jsonl = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
        rpc_jsonl.init().await?;
        let rpc_persistence = PersistenceBundle::new_with_schedule_store(
            rpc_jsonl as Arc<dyn SessionStore>,
            None,
            Arc::new(MemoryBlobStore::new()),
            rpc_schedule_store,
        );
        let rpc_config_store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(meerkat_core::MemoryConfigStore::new(rpc_config.clone()));
        let mut rpc_runtime = meerkat_rpc::session_runtime::SessionRuntime::new(
            rpc_factory,
            rpc_config,
            1024,
            rpc_persistence,
            meerkat_rpc::router::NotificationSink::noop(),
        );
        // Wire mob tools so delegate/mob_create/mob_profile_* are available
        let rpc_mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
            rpc_runtime.session_service(),
            Some(rpc_runtime.runtime_adapter()),
        ));
        rpc_runtime.set_mob_tools(Arc::new(AgentMobToolSurfaceFactory::new(Arc::clone(
            &rpc_mob_state,
        ))));
        rpc_runtime.set_mob_state(rpc_mob_state);
        // Wire config runtime so hot-swap (turn/start with model override) can
        // resolve self-hosted models from the loaded config.
        rpc_runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&rpc_config_store),
            session_dir.join("rpc_config_state.json"),
        )));
        let rpc_runtime = Arc::new(rpc_runtime);
        let rpc_config_store_clone = Arc::clone(&rpc_config_store);
        let rpc_runtime_clone = Arc::clone(&rpc_runtime);
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{rpc_port}");
            if let Err(e) =
                meerkat_rpc::serve_tcp(&addr, rpc_runtime_clone, rpc_config_store_clone, None).await
            {
                eprintln!("[target] RPC server error: {e}");
            }
        });
        rpc_runtime
    };
    rpc_session_runtime.ensure_schedule_host_started().await?;
    rpc_session_runtime
        .enable_autonomous_comms_drain(
            &current_session_id,
            Arc::clone(&comms_runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>,
        )
        .await
        .map_err(|error| anyhow::anyhow!("enable RPC comms drain: {}", error.message))?;
    let _ = rpc_session_runtime; // keep alive

    println!("=== MDM Target: {name} ===");
    println!("rpc       : tcp://0.0.0.0:{rpc_port}");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("kennel    : {kennel_addr}");
    println!("provider  : {provider} ({model})");

    let mut attached_tux_hint = load_attachment_hint(&attachment_hint_path).await?;
    let mut kennel_session_state = TksState::Disconnected;

    loop {
        let (kennel_host, kennel_port) = kennel_addr
            .rsplit_once(':')
            .context("invalid --kennel HOST:PORT")?;
        let kennel_port: u16 = kennel_port.parse().context("invalid kennel port")?;
        let local_ip = match &explicit_ip {
            Some(ip) => ip.clone(),
            None => match discover_local_ip(kennel_host, kennel_port) {
                Ok(ip) => ip,
                Err(e) => {
                    eprintln!(
                        "[target] kennel address probe failed: {e} — retrying in 5s \
                        (use --advertise <IP> to skip)"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            },
        };
        let advertised_addr = format!("tcp://{local_ip}:{comms_port}");

        let stream =
            match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&kennel_addr))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    eprintln!("[target] kennel connect failed: {e} — retrying in 2s");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(_) => {
                    eprintln!("[target] kennel connect timed out — retrying in 2s");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let register = build_signed_envelope(
            comms_runtime.router_arc().keypair_arc().as_ref(),
            &target_id,
            KennelPayload::TargetRegister {
                target_id: target_id.clone(),
                name: name.clone(),
                pubkey: target_pubkey.clone(),
                direct_addr: advertised_addr.clone(),
                rpc_addr: Some(format!("tcp://{local_ip}:{rpc_port}")),
                labels: Default::default(),
                capabilities: BTreeMap::from([
                    ("shell".to_string(), true),
                    ("runtime".to_string(), true),
                ]),
                attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
            },
        )?;
        if let Err(e) = write_envelope(&mut write_half, &register).await {
            eprintln!("[target] kennel register failed: {e} — retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        kennel_session_state =
            target_kennel_session::transition(kennel_session_state, TksEvent::RegisterSent)
                .map_err(|e| anyhow::anyhow!("target kennel session register sent: {e}"))?;
        let Some(env) = read_envelope(&mut reader).await? else {
            eprintln!("[target] kennel closed during register");
            kennel_session_state =
                target_kennel_session::transition(kennel_session_state, TksEvent::ControlLost)
                    .map_err(|e| anyhow::anyhow!("target kennel session register EOF: {e}"))?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let _ = verify_envelope(&env)?;
        match &env.payload {
            KennelPayload::TargetRegistered {
                hive_pubkey,
                hive_comms_addr,
            } => {
                // Add the hive as a trusted peer so it can send us comms messages.
                if let (Some(pk_str), Some(addr)) = (hive_pubkey, hive_comms_addr) {
                    if let Ok(pk) =
                        meerkat_comms::identity::PubKey::from_pubkey_string(pk_str.as_str())
                    {
                        comms_runtime
                            .register_trusted_peer(meerkat_comms::TrustedPeer {
                                name: "hive".into(),
                                pubkey: pk,
                                addr: addr.clone(),
                                meta: meerkat_comms::PeerMeta::default(),
                            })
                            .await?;
                        eprintln!("[target] added hive as trusted peer at {addr}");
                    }
                }
                kennel_session_state = target_kennel_session::transition(
                    kennel_session_state,
                    TksEvent::RegistrationAcked,
                )
                .map_err(|e| anyhow::anyhow!("target kennel session registration ack: {e}"))?;
            }
            KennelPayload::TargetRegistrationRejected { reason, message } => {
                let _ = target_kennel_session::transition(
                    kennel_session_state.clone(),
                    TksEvent::RegistrationRejected,
                )
                .map_err(|e| anyhow::anyhow!("target kennel session registration rejected: {e}"))?;
                bail!("kennel rejected target registration ({reason:?}): {message}");
            }
            other => {
                bail!("unexpected kennel registration reply: {other:?}");
            }
        }

        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let hb = build_signed_envelope(
                        comms_runtime.router_arc().keypair_arc().as_ref(),
                        &target_id,
                        KennelPayload::TargetHeartbeat,
                    )?;
                    if write_envelope(&mut write_half, &hb).await.is_err() {
                        break;
                    }
                }
                maybe = read_envelope(&mut reader) => {
                    let Some(env) = maybe? else { break; };
                    let _ = verify_envelope(&env)?;
                    match env.payload {
                        KennelPayload::Adopted {
                            lease_id,
                            target_id: adopted_target_id,
                            tux_id,
                            tux_pubkey,
                            tux_direct_addr,
                            ..
                        } => {
                            if !kennel_session_state.allows_control_payloads() {
                                bail!("received adopted before kennel registration completed");
                            }
                            if adopted_target_id != target_id {
                                continue;
                            }
                            let adoption = ActiveAdoption {
                                lease_id,
                                target_id: adopted_target_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                            };
                            let _adopted_exit = run_adopted_loop(
                                &comms_runtime,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                true,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                comms_runtime.router_arc().keypair_arc().as_ref(),
                                &target_id,
                                KennelPayload::TargetRegister {
                                    target_id: target_id.clone(),
                                    name: name.clone(),
                                    pubkey: target_pubkey.clone(),
                                    direct_addr: advertised_addr.clone(),
                                    rpc_addr: Some(format!("tcp://{local_ip}:{rpc_port}")),
                                    labels: Default::default(),
                                    capabilities: BTreeMap::from([
                                        ("shell".to_string(), true),
                                        ("runtime".to_string(), true),
                                    ]),
                                    attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
                                },
                            )?;
                            if write_envelope(&mut write_half, &re_register).await.is_ok() {
                                kennel_session_state = target_kennel_session::transition(
                                    kennel_session_state,
                                    TksEvent::RegisterSent,
                                )
                                .unwrap_or(TksState::Disconnected);
                            }
                        }
                        KennelPayload::LeaseRebound {
                            lease_id,
                            target_id: rebound_target_id,
                            tux_id,
                            tux_pubkey,
                            tux_direct_addr,
                            ..
                        } => {
                            if !kennel_session_state.allows_control_payloads() {
                                bail!("received lease rebound before kennel registration completed");
                            }
                            if rebound_target_id != target_id {
                                continue;
                            }
                            // Use the fresh TUX endpoint from the kennel, not
                            // stale cached peer data. TUX may have reconnected
                            // with a different --listen/--advertise address.
                            let adoption = ActiveAdoption {
                                lease_id,
                                target_id: rebound_target_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                            };
                            let _adopted_exit = run_adopted_loop(
                                &comms_runtime,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                false,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                comms_runtime.router_arc().keypair_arc().as_ref(),
                                &target_id,
                                KennelPayload::TargetRegister {
                                    target_id: target_id.clone(),
                                    name: name.clone(),
                                    pubkey: target_pubkey.clone(),
                                    direct_addr: advertised_addr.clone(),
                                    rpc_addr: Some(format!("tcp://{local_ip}:{rpc_port}")),
                                    labels: Default::default(),
                                    capabilities: BTreeMap::from([
                                        ("shell".to_string(), true),
                                        ("runtime".to_string(), true),
                                    ]),
                                    attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
                                },
                            )?;
                            if write_envelope(&mut write_half, &re_register).await.is_ok() {
                                kennel_session_state = target_kennel_session::transition(
                                    kennel_session_state,
                                    TksEvent::RegisterSent,
                                )
                                .unwrap_or(TksState::Disconnected);
                            }
                        }
                        KennelPayload::TargetRegistered { hive_pubkey, hive_comms_addr } => {
                            if let (Some(pk_str), Some(addr)) = (hive_pubkey, hive_comms_addr) {
                                if let Ok(pk) = meerkat_comms::identity::PubKey::from_pubkey_string(pk_str.as_str()) {
                                    comms_runtime
                                        .register_trusted_peer(meerkat_comms::TrustedPeer {
                                        name: "hive".into(),
                                        pubkey: pk,
                                        addr: addr.clone(),
                                        meta: meerkat_comms::PeerMeta::default(),
                                        })
                                        .await?;
                                }
                            }
                            kennel_session_state = target_kennel_session::transition(
                                kennel_session_state,
                                TksEvent::RegistrationAcked,
                            )
                            .map_err(|e| anyhow::anyhow!("target kennel session re-register ack: {e}"))?;
                        }
                        KennelPayload::TargetRegistrationRejected { reason, message } => {
                            let _ = target_kennel_session::transition(
                                kennel_session_state.clone(),
                                TksEvent::RegistrationRejected,
                            )
                            .map_err(|e| anyhow::anyhow!(
                                "target kennel session re-register rejected: {e}"
                            ))?;
                            bail!("kennel rejected target re-registration ({reason:?}): {message}");
                        }
                        KennelPayload::PeerWire { peer_name, peer_id, peer_addr } => {
                            if let Ok(pk) = meerkat_comms::identity::PubKey::from_pubkey_string(&peer_id) {
                                comms_runtime
                                    .register_trusted_peer(meerkat_comms::TrustedPeer {
                                    name: peer_name.clone(),
                                    pubkey: pk,
                                    addr: peer_addr.clone(),
                                    meta: meerkat_comms::PeerMeta::default(),
                                    })
                                    .await?;
                                eprintln!("[target] peer wired: {peer_name} at {peer_addr}");
                            }
                        }
                        KennelPayload::PeerUnwire { peer_id } => {
                            if let Ok(pk) = meerkat_comms::identity::PubKey::from_pubkey_string(&peer_id) {
                                comms_runtime.router_arc().remove_trusted_peer(&pk.to_peer_id());
                                eprintln!("[target] peer unwired: {peer_id}");
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        eprintln!("[target] kennel disconnected — reconnecting...");
        kennel_session_state =
            target_kennel_session::transition(kennel_session_state, TksEvent::ControlLost)
                .unwrap_or(TksState::Disconnected);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_adopted_loop(
    comms_runtime: &Arc<CommsRuntime>,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    adoption: ActiveAdoption,
    attach_required: bool,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<TkcState> {
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::watch::channel::<bool>(false);

    run_adopted_loop_inner(
        comms_runtime,
        reader,
        write_half,
        adoption,
        attach_required,
        attachment_hint,
        attachment_hint_path,
        &disconnect_tx,
        &mut disconnect_rx,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_adopted_loop_inner(
    comms_runtime: &Arc<CommsRuntime>,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    adoption: ActiveAdoption,
    attach_required: bool,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    disconnect_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<TkcState> {
    let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;
    let initial_event = if attach_required {
        TkcEvent::Adopted {
            target_id: adoption.target_id.clone(),
            lease_id: adoption.lease_id.clone(),
            tux_id: adoption.tux_id.clone(),
            tux_pubkey: adoption.tux_pubkey.clone(),
            tux_direct_addr: adoption.tux_direct_addr.clone(),
        }
    } else {
        TkcEvent::LeaseRebound {
            target_id: adoption.target_id.clone(),
            lease_id: adoption.lease_id.clone(),
            tux_id: adoption.tux_id.clone(),
            tux_pubkey: adoption.tux_pubkey.clone(),
            tux_direct_addr: adoption.tux_direct_addr.clone(),
        }
    };
    let (mut machine_state, effects) =
        target_kennel_control::transition(TkcState::idle(), initial_event)
            .map_err(|e| anyhow::anyhow!("machine transition: {e}"))?;
    let (applied, should_return) = apply_target_control_effects(
        comms_runtime,
        &effects,
        &mut heartbeat,
        disconnect_tx,
        attachment_hint,
        attachment_hint_path,
    )
    .await?;
    if !applied {
        let (state, effects) =
            target_kennel_control::transition(machine_state, TkcEvent::AttachSendFailed)
                .map_err(|e| anyhow::anyhow!("attach-send-failed transition: {e}"))?;
        let _ = apply_target_control_effects(
            comms_runtime,
            &effects,
            &mut heartbeat,
            disconnect_tx,
            attachment_hint,
            attachment_hint_path,
        )
        .await?;
        return Ok(state);
    }
    if should_return {
        return Ok(machine_state);
    }

    let mut kennel_heartbeat = tokio::time::interval(Duration::from_secs(10));

    loop {
        let poll_kennel = matches!(
            &machine_state.attachment,
            TaState::Attaching { .. } | TaState::Attached { .. }
        );

        tokio::select! {
            msg = comms_runtime.recv_message(), if matches!(machine_state.attachment, TaState::Attaching { .. }) => {
                let Some(msg) = msg else {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    return Ok(s);
                };
                // Handle attach-ack from TUX (kennel protocol).
                if let Some(payload) = parse_direct_control_message(&msg)?
                    && let DirectControlPayload::AttachAck { lease_id } = payload
                    && matches!(machine_state.attachment, TaState::Attaching { .. })
                {
                    let (s, effects) = target_kennel_control::transition(
                        machine_state.clone(),
                        TkcEvent::DirectAttachAck { lease_id },
                    )
                    .map_err(|e| anyhow::anyhow!("direct-attach-ack transition: {e}"))?;
                    machine_state = s;
                    let _ = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    continue;
                }
                // Once attached, ordinary peer traffic is left entirely to
                // the runtime adapter's peer ingress drain. This branch only
                // exists to receive the direct attach ACK during the short
                // Attaching phase.
                tracing::debug!(from = %msg.from_peer, "ignoring non-control comms message while attaching");
            }
            _ = kennel_heartbeat.tick(), if poll_kennel => {
                let kennel_hb = build_signed_envelope(
                    comms_runtime.router_arc().keypair_arc().as_ref(), &adoption.target_id,
                    KennelPayload::TargetHeartbeat,
                )?;
                if write_envelope(write_half, &kennel_hb).await.is_err() {
                    eprintln!("[target] kennel control lost");
                    let (s, effects) = target_kennel_control::transition(
                        machine_state.clone(),
                        TkcEvent::KennelHeartbeatFailed,
                    )
                    .map_err(|e| anyhow::anyhow!("kennel-heartbeat-failed transition: {e}"))?;
                    machine_state = s;
                    let (_, should_return) = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    if should_return {
                        if let Some(h) = heartbeat.take() { h.abort(); }
                        return Ok(machine_state);
                    }
                    if machine_state.attachment.is_terminal() {
                        if let Some(h) = heartbeat.take() { h.abort(); }
                        return Ok(machine_state);
                    }
                }
            }
            maybe = read_envelope(reader), if poll_kennel => {
                match maybe {
                    Ok(Some(env)) => {
                        if verify_envelope(&env).is_err() {
                            eprintln!("[target] invalid signed kennel control frame");
                            let (s, effects) = target_kennel_control::transition(
                                machine_state.clone(),
                                TkcEvent::KennelDisconnected,
                            )
                            .map_err(|e| anyhow::anyhow!("kennel-disconnected transition: {e}"))?;
                            machine_state = s;
                            let (_, should_return) = apply_target_control_effects(
                                comms_runtime,
                                &effects,
                                &mut heartbeat,
                                disconnect_tx,
                                attachment_hint,
                                attachment_hint_path,
                            )
                            .await?;
                            if should_return {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                return Ok(machine_state);
                            }
                            if machine_state.attachment.is_terminal() {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                return Ok(machine_state);
                            }
                            continue;
                        }
                        match env.payload {
                            KennelPayload::Released { lease_ref, .. } => {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                let (s, effects) = target_kennel_control::transition(
                                    machine_state,
                                    TkcEvent::KennelReleased { lease_ref },
                                )
                                .map_err(|e| anyhow::anyhow!("kennel-released transition: {e}"))?;
                                let _ = apply_target_control_effects(
                                    comms_runtime,
                                    &effects,
                                    &mut heartbeat,
                                    disconnect_tx,
                                    attachment_hint,
                                    attachment_hint_path,
                                )
                                .await?;
                                return Ok(s);
                            }
                            KennelPayload::LeaseRebound {
                                target_id: rebound_target_id,
                                lease_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                                ..
                            } => {
                                let (s, effects) = target_kennel_control::transition(
                                    machine_state,
                                    TkcEvent::LeaseRebound {
                                        target_id: rebound_target_id,
                                        lease_id,
                                        tux_id,
                                        tux_pubkey,
                                        tux_direct_addr,
                                    },
                                )
                                .map_err(|e| anyhow::anyhow!("lease-rebound transition: {e}"))?;
                                machine_state = s;
                                let _ = apply_target_control_effects(
                                    comms_runtime,
                                    &effects,
                                    &mut heartbeat,
                                    disconnect_tx,
                                    attachment_hint,
                                    attachment_hint_path,
                                )
                                .await?;
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        eprintln!("[target] kennel control lost");
                        let (s, effects) = target_kennel_control::transition(
                            machine_state.clone(),
                            TkcEvent::KennelDisconnected,
                        )
                        .map_err(|e| anyhow::anyhow!("kennel-control-lost transition: {e}"))?;
                        machine_state = s;
                        let (_, should_return) = apply_target_control_effects(
                            comms_runtime,
                            &effects,
                            &mut heartbeat,
                            disconnect_tx,
                            attachment_hint,
                            attachment_hint_path,
                        )
                        .await?;
                        if should_return {
                            if let Some(h) = heartbeat.take() { h.abort(); }
                            return Ok(machine_state);
                        }
                        if machine_state.attachment.is_terminal() {
                            if let Some(h) = heartbeat.take() { h.abort(); }
                            return Ok(machine_state);
                        }
                    }
                }
            }
            _ = disconnect_rx.changed() => {
                if *disconnect_rx.borrow() {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("disconnect-direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    return Ok(s);
                }
            }
        }
    }
}

/// Probe our outbound IP toward a host via a non-sending UDP "connect".
fn discover_local_ip(host: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
    sock.connect(format!("{host}:{port}"))
        .with_context(|| format!("UDP probe to {host}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_provider_override(args: &[String]) -> anyhow::Result<Option<ProviderKind>> {
    let model = find_flag(args, "--model");
    let provider = find_flag(args, "--provider");
    match (model, provider) {
        (Some(_), Some(provider)) => Ok(Some(provider.parse()?)),
        (Some(_), None) => anyhow::bail!("--model requires --provider"),
        (None, Some(_)) => anyhow::bail!("--provider requires --model"),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    mod test_support {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/test_support.rs"));
    }

    use super::{
        TargetRuntimeSurface, TargetScheduleSessionHost, build_target_runtime_surface,
        create_target_comms_runtime, discard_live_session_with_mob_cleanup,
        parse_provider_override, setup_session,
    };
    use anyhow::Context as _;
    use chrono::{Duration as ChronoDuration, Utc};
    use mdm_tux::ProviderKind;
    use meerkat::surface::{
        NoopScheduleMobHost, SharedScheduleTargetAdapter, SurfaceScheduleSessionHost,
        schedule_host_supported, spawn_schedule_host,
    };
    use meerkat::{
        AgentFactory, CreateScheduleRequest, FactoryAgentBuilder, LlmClient, MisfirePolicy,
        MissingTargetPolicy, OverlapPolicy, PersistenceBundle, PersistentSessionService,
        ScheduleService, ScheduleToolDispatcher, ScheduledSessionAction, SessionAgentBuilder,
        SessionService, SessionTargetBinding, SqliteScheduleStore, TargetBinding, TriggerSpec,
        encode_llm_client_override_for_service,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, TestClient};
    use meerkat_comms::{CommsRuntime, ResolvedCommsConfig};
    use meerkat_core::Config;
    use meerkat_core::ops_lifecycle::OperationKind;
    use meerkat_core::service::{
        CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, StartTurnRequest,
    };
    use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive};
    use meerkat_core::types::{ContentInput, HandlingMode};
    use meerkat_mob::MobSessionService;
    use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
    use meerkat_store::{JsonlStore, MemoryBlobStore, SessionStore};
    use std::collections::{HashSet, VecDeque};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use test_support::CaptureClient;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    struct ScriptedClient {
        responses: Mutex<VecDeque<Vec<LlmEvent>>>,
    }

    impl ScriptedClient {
        fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    async fn wait_for_captured_user_message(
        capture: &CaptureClient,
        expected: &str,
    ) -> anyhow::Result<()> {
        timeout(Duration::from_secs(5), async {
            loop {
                let matches = capture
                    .user_messages()
                    .into_iter()
                    .filter(|message| message == expected)
                    .count();
                if matches == 1 {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for scheduled prompt '{expected}'"))?
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let _ = request;
            let events = self
                .responses
                .lock()
                .expect("scripted client lock")
                .pop_front()
                .unwrap_or_else(|| {
                    vec![LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    }]
                });
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> &'static str {
            "test"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn build_target_runtime_surface_with_client(
        session_dir: &Path,
        comms_runtime: Arc<CommsRuntime>,
        llm_client: Arc<dyn LlmClient>,
    ) -> anyhow::Result<TargetRuntimeSurface> {
        let factory = AgentFactory::new(session_dir)
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(comms_runtime);
        let schedule_store = Arc::new(SqliteScheduleStore::open(
            session_dir.join("schedule.sqlite"),
        )?) as Arc<dyn meerkat::ScheduleStore>;
        let schedule_service = ScheduleService::new(Arc::clone(&schedule_store));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(llm_client);
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        *builder
            .default_schedule_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            ScheduleToolDispatcher::new(schedule_service.clone()),
        ));

        let jsonl_store = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
        jsonl_store.init().await?;

        let bundle_store = JsonlStore::new(session_dir.to_path_buf());
        bundle_store.init().await?;

        let persistence = PersistenceBundle::new_with_schedule_store(
            Arc::new(bundle_store) as Arc<dyn SessionStore>,
            None,
            Arc::new(MemoryBlobStore::new()),
            schedule_store,
        );
        let runtime_adapter = persistence.runtime_adapter();
        let (session_store, runtime_store, blob_store) = persistence.into_parts();

        let session_service =
            PersistentSessionService::new(builder, 10, session_store, runtime_store, blob_store);
        let service = Arc::new(session_service);
        let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
            service.clone(),
            Some(runtime_adapter.clone()),
        ));
        *mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
        ));
        let schedule_host = if schedule_host_supported(schedule_service.store().kind()) {
            let session_host: Arc<dyn SurfaceScheduleSessionHost> =
                Arc::new(TargetScheduleSessionHost::new(
                    service.clone(),
                    runtime_adapter.clone(),
                    mob_state.clone(),
                ));
            let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
                schedule_service.clone(),
                session_host,
                Arc::new(NoopScheduleMobHost::new(
                    "scheduled mob targets are not enabled in mdm-target",
                )),
            ));
            Some(spawn_schedule_host(
                schedule_service,
                shared_adapter,
                "mdm-target-test",
            ))
        } else {
            None
        };

        Ok(TargetRuntimeSurface {
            service,
            runtime_adapter,
            jsonl_store,
            mob_state,
            _factory: Arc::new(AgentFactory::new(session_dir)),
            _config: Config::default(),
            _schedule_host: schedule_host,
        })
    }

    #[test]
    fn provider_override_requires_model_and_provider_together() {
        let err = parse_provider_override(&["--model".into(), "gpt-5.5".into()]).unwrap_err();
        assert!(err.to_string().contains("--model requires --provider"));

        let err = parse_provider_override(&["--provider".into(), "openai".into()]).unwrap_err();
        assert!(err.to_string().contains("--provider requires --model"));
    }

    #[test]
    fn provider_override_parses_explicit_provider() {
        let provider = parse_provider_override(&[
            "--model".into(),
            "gpt-5.5".into(),
            "--provider".into(),
            "openai".into(),
        ])
        .unwrap();
        assert_eq!(provider, Some(ProviderKind::Openai));
    }

    #[test]
    fn target_executor_carries_terminal_peer_response_context_into_turn_request() {
        let primitive = RunPrimitive::StagedInput(
            meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                appends: Vec::new(),
                context_appends: vec![
                    meerkat_core::lifecycle::run_primitive::ConversationContextAppend {
                        key: "peer_response_terminal:analyst:req-1".to_string(),
                        content: CoreRenderable::Text {
                            text: "terminal answer from analyst".to_string(),
                        },
                    },
                ],
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        execution_kind: Some(
                            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                        ),
                        peer_response_terminal_apply_intent: Some(
                            meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent::AppendContextAndRun,
                        ),
                        ..Default::default()
                    },
                ),
            },
        );

        let request =
            super::start_turn_request_from_primitive(&primitive).expect("primitive should convert");

        assert_eq!(request.prompt.text_content(), "");
        assert_eq!(request.pre_turn_context_appends.len(), 1);
        assert_eq!(
            request.pre_turn_context_appends[0].idempotency_key.as_deref(),
            Some("peer_response_terminal:analyst:req-1")
        );
        assert_eq!(
            request.pre_turn_context_appends[0].text,
            "terminal answer from analyst"
        );
    }

    #[tokio::test]
    async fn target_builder_exposes_shell_comms_and_mob_tools() {
        let temp = tempfile::tempdir().unwrap();
        // Create a CommsRuntime — the factory will compose comms tools from it.
        let comms_config = ResolvedCommsConfig {
            enabled: true,
            name: "test-target".to_string(),
            inproc_namespace: None,
            listen_tcp: None,
            listen_uds: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: temp.path().join("identity"),
            trusted_peers_path: temp.path().join("trusted_peers.json"),
            comms_config: Default::default(),
            auth: Default::default(),
            require_peer_auth: false,
            allow_external_unauthenticated: false,
        };
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                comms_config,
                Arc::new(std::collections::HashSet::new()),
            )
            .await
            .unwrap(),
        );
        // Add a dummy peer so comms tools pass the availability gate.
        comms_runtime
            .register_trusted_peer(meerkat_comms::TrustedPeer {
                name: "tux".into(),
                pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
                addr: "tcp://127.0.0.1:9999".into(),
                meta: meerkat_comms::PeerMeta::default(),
            })
            .await
            .unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(comms_runtime);
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let mob_state = Arc::new(MobMcpState::new_in_memory());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
        ));
        *builder
            .default_schedule_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(SqliteScheduleStore::open(temp.path().join("schedule.sqlite")).unwrap()),
            ))));

        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());

        let req = CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: ContentInput::Text("inspect target tools".to_string()),
            render_metadata: None,
            system_prompt: Some("test target".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture.clone() as Arc<dyn LlmClient>
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("inspect".to_string()))
            .await
            .unwrap();

        let tool_names = capture.tool_names();
        assert!(tool_names.iter().any(|name| name == "datetime"));
        assert!(!tool_names.iter().any(|name| name == "wait"));
        assert!(tool_names.iter().any(|name| name == "shell"));
        assert!(tool_names.iter().any(|name| name == "send_message"));
        assert!(tool_names.iter().any(|name| name == "peers"));
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_create")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name == "meerkat_schedule_list")
        );
        assert!(tool_names.iter().any(|name| name == "delegate"));
        assert!(tool_names.iter().any(|name| name == "mob_list"));
        assert!(tool_names.iter().any(|name| name == "mob_check_member"));
    }

    #[tokio::test]
    async fn target_schedule_host_delivers_future_prompt_once() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = create_target_comms_runtime("test-scheduled-target", temp.path())
            .await
            .unwrap();
        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());

        let surface = build_target_runtime_surface_with_client(
            temp.path(),
            Arc::clone(&comms_runtime),
            capture.clone() as Arc<dyn LlmClient>,
        )
        .await
        .unwrap();
        let session_id = setup_session(
            &surface.service,
            &surface.runtime_adapter,
            None,
            "gpt-5.5",
            "test",
            &surface.mob_state,
            "openai",
            None,
            None,
        )
        .await
        .unwrap();

        let schedule_service = ScheduleService::new(Arc::new(
            SqliteScheduleStore::open(temp.path().join("schedule.sqlite")).unwrap(),
        ));
        schedule_service
            .create(CreateScheduleRequest {
                name: Some("scheduled-ping".into()),
                description: Some("deliver one scheduled prompt".into()),
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() + ChronoDuration::seconds(1),
                },
                target: TargetBinding::session(SessionTargetBinding::ExactSession {
                    session_id,
                    action: ScheduledSessionAction::Prompt {
                        prompt: "scheduled ping".into(),
                        system_prompt: None,
                        render_metadata: None,
                        skill_refs: Vec::new(),
                        additional_instructions: Vec::new(),
                    },
                }),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: Default::default(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .unwrap();

        wait_for_captured_user_message(capture.as_ref(), "scheduled ping")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn target_discard_cleanup_removes_owned_mobs() {
        let temp = tempfile::tempdir().unwrap();
        let comms_config = ResolvedCommsConfig {
            enabled: true,
            name: "test-cleanup".to_string(),
            inproc_namespace: None,
            listen_tcp: None,
            listen_uds: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: temp.path().join("identity"),
            trusted_peers_path: temp.path().join("trusted_peers.json"),
            comms_config: Default::default(),
            auth: Default::default(),
            require_peer_auth: false,
            allow_external_unauthenticated: false,
        };
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                comms_config,
                Arc::new(std::collections::HashSet::new()),
            )
            .await
            .unwrap(),
        );
        let surface = build_target_runtime_surface(temp.path(), comms_runtime)
            .await
            .unwrap();
        let req = CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: Some("cleanup".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(Arc::new(
                    TestClient::default(),
                ))),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };
        let session_id = surface
            .service
            .create_session(req)
            .await
            .unwrap()
            .session_id;

        surface
            .mob_state
            .get_or_create_implicit_mob_for_bridge_session(&session_id.to_string(), "gpt-5.5")
            .await
            .unwrap();
        assert!(
            surface
                .mob_state
                .find_implicit_mob_for_bridge_session(&session_id.to_string())
                .await
                .is_some()
        );

        discard_live_session_with_mob_cleanup(&surface.service, &surface.mob_state, &session_id)
            .await
            .unwrap();
        assert!(
            surface
                .mob_state
                .find_implicit_mob_for_bridge_session(&session_id.to_string())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn target_setup_session_routes_background_shell_completions_into_runtime_feed() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                ResolvedCommsConfig {
                    enabled: true,
                    name: "test-background-feed".to_string(),
                    inproc_namespace: None,
                    listen_tcp: None,
                    listen_uds: None,
                    event_listen_tcp: None,
                    #[cfg(unix)]
                    event_listen_uds: None,
                    identity_dir: temp.path().join("identity"),
                    trusted_peers_path: temp.path().join("trusted_peers.json"),
                    comms_config: Default::default(),
                    auth: Default::default(),
                    require_peer_auth: false,
                    allow_external_unauthenticated: false,
                },
                Arc::new(HashSet::new()),
            )
            .await
            .unwrap(),
        );
        let llm_client: Arc<dyn LlmClient> = Arc::new(ScriptedClient::new(vec![
            vec![
                LlmEvent::ToolCallComplete {
                    id: "tc_shell_background".to_string(),
                    name: "shell".to_string(),
                    args: serde_json::json!({
                        "command": "echo target-background-feed",
                        "background": true,
                    }),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    delta: "background started".to_string(),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ],
        ]));
        let surface =
            build_target_runtime_surface_with_client(temp.path(), comms_runtime, llm_client)
                .await
                .unwrap();

        let session_id = setup_session(
            &surface.service,
            &surface.runtime_adapter,
            None,
            "gpt-5.5",
            "test background shell",
            &surface.mob_state,
            "openai",
            None,
            None,
        )
        .await
        .unwrap();

        let runtime_registry = surface
            .runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("runtime registry must exist after setup");
        let feed = runtime_registry.completion_feed_handle();
        assert_eq!(feed.watermark(), 0);

        surface
            .service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("run shell".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    pre_turn_context_appends: Vec::new(),
                    turn_metadata: Some(
                        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                            execution_kind: Some(
                                meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                            ),
                            ..Default::default()
                        },
                    ),
                },
            )
            .await
            .unwrap();

        timeout(Duration::from_secs(5), async {
            loop {
                if feed.watermark() > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .context("background shell completion should advance the runtime feed")
        .unwrap();

        let batch = feed.list_since(0);
        assert!(
            batch
                .entries
                .iter()
                .any(|entry| entry.kind == OperationKind::BackgroundToolOp),
            "runtime feed should contain a background shell completion"
        );
    }

    /// Regression: ensure setup_session through PersistentSessionService
    /// produces an agent with builtins, shell, comms, and mob tools —
    /// not just comms tools.
    #[tokio::test]
    async fn setup_session_via_service_exposes_all_tool_categories() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = create_target_comms_runtime("test-full", temp.path())
            .await
            .unwrap();
        // Add a peer so comms tools pass the availability gate.
        comms_runtime
            .register_trusted_peer(meerkat_comms::TrustedPeer {
                name: "tux".into(),
                pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
                addr: "tcp://127.0.0.1:9999".into(),
                meta: meerkat_comms::PeerMeta::default(),
            })
            .await
            .unwrap();
        let _surface = build_target_runtime_surface(temp.path(), Arc::clone(&comms_runtime))
            .await
            .unwrap();

        // Build an agent through the full pipeline and check tool visibility.
        let factory = AgentFactory::new(temp.path().join("sessions2"))
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(Arc::clone(&comms_runtime));
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(MobMcpState::new_in_memory()),
        ));
        *builder
            .default_schedule_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            ScheduleToolDispatcher::new(ScheduleService::new(Arc::new(
                SqliteScheduleStore::open(temp.path().join("sessions2-schedule.sqlite")).unwrap(),
            ))),
        ));

        let capture2: Arc<CaptureClient> = Arc::new(CaptureClient::default());
        let req2 = CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: ContentInput::Text("list tools".into()),
            render_metadata: None,
            system_prompt: Some("test".into()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture2.clone() as Arc<dyn LlmClient>
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req2, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("list tools".into()))
            .await
            .unwrap();

        let tool_names = capture2.tool_names();
        // Builtins
        assert!(
            tool_names.iter().any(|n| n == "datetime"),
            "missing builtin 'datetime', got: {tool_names:?}"
        );
        assert!(
            !tool_names.iter().any(|n| n == "wait"),
            "unexpected removed builtin 'wait', got: {tool_names:?}"
        );
        // Shell
        assert!(
            tool_names.iter().any(|n| n == "shell"),
            "missing 'shell', got: {tool_names:?}"
        );
        // Comms
        assert!(
            tool_names.iter().any(|n| n == "send_message"),
            "missing comms 'send_message', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "peers"),
            "missing comms 'peers', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "meerkat_schedule_create"),
            "missing scheduler 'meerkat_schedule_create', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "meerkat_schedule_list"),
            "missing scheduler 'meerkat_schedule_list', got: {tool_names:?}"
        );
        // Mob
        assert!(
            tool_names.iter().any(|n| n == "delegate"),
            "missing mob 'delegate', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "mob_list"),
            "missing 'mob_list', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "mob_profile_create"),
            "missing 'mob_profile_create', got: {tool_names:?}"
        );
    }

    /// Regression: auto-resumed session from disk must retain all tool categories.
    /// Old persisted metadata with Inherit/Disable must not suppress builtins/shell.
    #[tokio::test]
    async fn resumed_session_retains_all_tool_categories() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = create_target_comms_runtime("test-resume", temp.path())
            .await
            .unwrap();
        comms_runtime
            .register_trusted_peer(meerkat_comms::TrustedPeer {
                name: "tux".into(),
                pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
                addr: "tcp://127.0.0.1:9999".into(),
                meta: meerkat_comms::PeerMeta::default(),
            })
            .await
            .unwrap();

        let session_dir = temp.path().join("sessions");

        // 1. Create a fresh session via the full pipeline, persist it.
        let surface = build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime))
            .await
            .unwrap();
        let fresh_id = setup_session(
            &surface.service,
            &surface.runtime_adapter,
            None,
            "gpt-5.5",
            "test",
            &surface.mob_state,
            "openai",
            None,
            None,
        )
        .await
        .unwrap();

        // 2. Resume that session through the builder with a capture client.
        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());
        let loaded = surface
            .service
            .load_persisted_session(&fresh_id)
            .await
            .unwrap()
            .unwrap();

        let factory = AgentFactory::new(temp.path().join("sessions2"))
            .shell(true)
            .builtins(true)
            .comms(true)
            .schedule(true)
            .mob(true)
            .with_comms_runtime(Arc::clone(&comms_runtime));
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(MobMcpState::new_in_memory()),
        ));
        *builder
            .default_schedule_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            ScheduleToolDispatcher::new(ScheduleService::new(Arc::new(
                SqliteScheduleStore::open(temp.path().join("sessions2-schedule.sqlite")).unwrap(),
            ))),
        ));

        let req = CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: ContentInput::Text("list tools".into()),
            render_metadata: None,
            system_prompt: Some("test".into()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture.clone() as Arc<dyn LlmClient>
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                resume_session: Some(loaded),
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("list tools".into()))
            .await
            .unwrap();

        let tool_names = capture.tool_names();
        assert!(
            tool_names.iter().any(|n| n == "datetime"),
            "resumed session missing builtin 'datetime', got: {tool_names:?}"
        );
        assert!(
            !tool_names.iter().any(|n| n == "wait"),
            "resumed session unexpectedly includes removed builtin 'wait', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "shell"),
            "resumed session missing 'shell', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "send_message"),
            "resumed session missing comms 'send_message', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "meerkat_schedule_create"),
            "resumed session missing scheduler 'meerkat_schedule_create', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "delegate"),
            "resumed session missing mob 'delegate', got: {tool_names:?}"
        );
    }
}
