use super::disposal::{
    BulkBestEffort, DisposalContext, DisposalReport, DisposalStep, ErrorPolicy, WarnAndContinue,
};
use super::flow_frame_engine::FlowFrameLoopStorePlan;
use super::mob_member_lifecycle_projection::{
    CanonicalMemberSnapshotMaterial, MobMemberLifecycleInput, MobMemberLifecycleProjection,
};
use super::mob_runtime_bridge_authority::{MobRuntimeBridgeAuthority, MobRuntimeBridgeEffect};
use super::provision_guard::PendingProvision;
use super::terminalization::{TerminalizationOutcome, TerminalizationTarget};
use super::transaction::LifecycleRollback;
use super::*;
use crate::generated::protocol_mob_destroying_session_ingress::MobDestroyingSessionIngressObligation;
use crate::ids::{AgentIdentity, Generation};
use crate::machines::mob_machine as mob_dsl;
use crate::run::{MobMachineFlowAuthorityToken, MobMachineFlowRunCommand, MobRunStatus, flow_run};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::comms::{PeerLifecycleKind, PeerRoute, TrustedPeerDescriptor};
use meerkat_core::time_compat::SystemTime;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Lightweight handle for a spawned autonomous initial turn.
///
/// The `JoinHandle` is for abort on stop/dispose.
pub(super) struct InitialTurnHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl InitialTurnHandle {
    fn abort(self) {
        self.handle.abort();
    }
}

// Sized for real mob-scale startup/shutdown fan-out (50+ members).
#[cfg(not(target_arch = "wasm32"))]
const MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS: usize = 64;
const MAX_LIFECYCLE_NOTIFICATION_TASKS: usize = 16;

#[derive(Clone)]
enum WiringEndpoint {
    Local {
        entry: Box<RosterEntry>,
        comms: Arc<dyn CoreCommsRuntime>,
        spec: TrustedPeerDescriptor,
        comms_name: String,
    },
    PeerOnly {
        spec: TrustedPeerDescriptor,
        binding: crate::RuntimeBinding,
    },
}

struct SupervisorPrivateTrustInstall {
    peer_id: String,
    epoch: u64,
    removal_key: String,
}

struct PreparedDslInput {
    authority: mob_dsl::MobMachineAuthority,
    effects: Vec<mob_dsl::MobMachineEffect>,
    phase_changed: bool,
}

/// Resolve the runtime binding for a spawn request.
///
/// `RuntimeBinding` takes precedence over the legacy `backend` tag. When neither
/// is provided, resolves from profile/definition defaults. `External` without
/// a concrete `RuntimeBinding` is an error — you cannot spawn an external
/// member without declaring the real process identity.
fn resolve_binding(
    binding: Option<crate::RuntimeBinding>,
    backend: Option<crate::MobBackendKind>,
    profile_backend: Option<crate::MobBackendKind>,
    definition_default: crate::MobBackendKind,
    agent_identity: &MeerkatId,
) -> Result<crate::RuntimeBinding, MobError> {
    if let Some(b) = binding {
        return Ok(b);
    }
    let kind = backend.or(profile_backend).unwrap_or(definition_default);
    match kind {
        crate::MobBackendKind::Session => Ok(crate::RuntimeBinding::Session),
        crate::MobBackendKind::External => Err(MobError::WiringError(format!(
            "external backend requires explicit RuntimeBinding for '{agent_identity}'"
        ))),
    }
}

fn normalize_runtime_mode_for_binding(
    runtime_mode: crate::MobRuntimeMode,
    binding: &crate::RuntimeBinding,
) -> crate::MobRuntimeMode {
    match binding {
        crate::RuntimeBinding::External { .. } => crate::MobRuntimeMode::TurnDriven,
        crate::RuntimeBinding::Session => runtime_mode,
    }
}

fn admit_bridge_session_for_spawn(
    req: &mut meerkat_core::service::CreateSessionRequest,
) -> SessionId {
    if req.build.is_none() {
        req.build = Some(meerkat_core::service::SessionBuildOptions::default());
    }
    let build = req
        .build
        .as_mut()
        .expect("build options were initialized above");
    if let Some(session) = build.resume_session.as_ref() {
        return session.id().clone();
    }
    let session_id = SessionId::new();
    build.resume_session = Some(meerkat_core::session::Session::with_id(session_id.clone()));
    session_id
}

/// Project a DSL `MobPhase` into the shell `MobState` enum. Used by
/// `MobActor::state()` and by the `MobCommand::QueryPhase` reply so that
/// external `MobHandle::status()` callers observe the same DSL-authority
/// value the actor uses internally (dogma #1, #13, #17).
fn project_dsl_phase(phase: mob_dsl::MobPhase) -> MobState {
    match phase {
        mob_dsl::MobPhase::Running => MobState::Running,
        mob_dsl::MobPhase::Stopped => MobState::Stopped,
        mob_dsl::MobPhase::Completed => MobState::Completed,
        mob_dsl::MobPhase::Destroyed => MobState::Destroyed,
    }
}

/// Render forked conversation messages as a text context block for the new member.
fn render_fork_context(
    source_member_id: &MeerkatId,
    messages: &[meerkat_core::types::Message],
) -> String {
    use meerkat_core::types::Message;

    let mut lines = Vec::new();
    lines.push(format!(
        "[Forked conversation context from member '{source_member_id}']"
    ));
    for msg in messages {
        match msg {
            Message::System(s) => {
                lines.push(format!("[system]: {}", s.content));
            }
            Message::SystemNotice(notice) => {
                lines.push(format!("[system_notice]: {}", notice.rendered_text()));
            }
            Message::User(u) => {
                lines.push(format!("[user]: {}", u.text_content()));
            }
            Message::Assistant(a) => {
                if !a.content.is_empty() {
                    lines.push(format!("[assistant]: {}", a.content));
                }
            }
            Message::BlockAssistant(ba) => {
                let text: String = ba
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        meerkat_core::types::AssistantBlock::Text { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    lines.push(format!("[assistant]: {text}"));
                }
            }
            Message::ToolResults { results, .. } => {
                for tr in results {
                    let text: String = tr
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            meerkat_core::types::ContentBlock::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        let preview = if text.len() > 200 {
                            // Find a valid UTF-8 char boundary at or before byte 200
                            let end = text
                                .char_indices()
                                .map(|(i, _)| i)
                                .take_while(|&i| i <= 200)
                                .last()
                                .unwrap_or(0);
                            format!("{}...", &text[..end])
                        } else {
                            text
                        };
                        lines.push(format!("[tool_result({})]: {preview}", tr.tool_use_id));
                    }
                }
            }
        }
    }
    lines.push("[End of forked context]".to_string());
    lines.join("\n")
}

/// Unified MCP server entry: process handle + running status behind a single lock.
pub(super) struct McpServerEntry {
    #[cfg(not(target_arch = "wasm32"))]
    pub process: Option<Child>,
    pub running: bool,
}

pub(super) struct PendingSpawn {
    pub(super) profile_name: ProfileName,
    pub(super) agent_identity: MeerkatId,
    pub(super) admitted_bridge_session_id: SessionId,
    pub(super) prompt: ContentInput,
    pub(super) runtime_mode: crate::MobRuntimeMode,
    pub(super) labels: std::collections::BTreeMap<String, String>,
    pub(super) owner_bridge_session_id: Option<SessionId>,
    pub(super) auto_wire_parent: bool,
    /// Peer wiring to restore after respawn completes.
    pub(super) restore_wiring: Option<RestoreWiringPlan>,
    /// Effective profile override from `SpawnTooling::Profile` resolution.
    /// Persisted in the roster so respawn/restore can use it.
    pub(super) effective_profile_override: Option<crate::profile::Profile>,
    pub(super) progress: Arc<std::sync::Mutex<PendingSpawnProgress>>,
    pub(super) reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
}

#[derive(Debug, Default)]
pub(super) struct PendingSpawnProgress {
    pub(super) bridge_session_id: Option<meerkat_core::types::SessionId>,
    pub(super) operation_id: Option<meerkat_core::ops::OperationId>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RestoreWiringPlan {
    local_peers: Vec<MeerkatId>,
    external_peers: Vec<TrustedPeerDescriptor>,
}

struct RespawnSnapshot {
    profile_name: ProfileName,
    runtime_mode: crate::MobRuntimeMode,
    labels: std::collections::BTreeMap<String, String>,
    old_runtime_id: crate::ids::AgentRuntimeId,
    old_fence_token: crate::ids::FenceToken,
    /// Generation of the member being respawned.
    generation: crate::ids::Generation,
    restore_wiring: RestoreWiringPlan,
    /// Runtime binding extracted from the old roster entry's member_ref.
    /// Preserves real external identity across respawns.
    binding: crate::RuntimeBinding,
    /// Effective profile override persisted in the roster.
    /// Used on respawn to avoid re-resolving from the definition.
    effective_profile_override: Option<crate::profile::Profile>,
}

struct FinalizeSpawnOutcome {
    receipt: super::handle::MemberSpawnReceipt,
    failed_restore_peer_ids: Vec<MeerkatId>,
}

#[cfg(not(target_arch = "wasm32"))]
struct RemoteDestroyOutcome {
    identity: AgentIdentity,
    force_destroyed: bool,
    orphaned: bool,
    errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// MobActor
// ---------------------------------------------------------------------------

/// The actor that processes mob commands sequentially.
///
/// Owns all mutable state. Runs in a dedicated tokio task.
/// All mutations go through here; reads bypass via shared `Arc` state.
pub(super) struct MobActor {
    pub(super) definition: Arc<MobDefinition>,
    pub(super) roster: Arc<RwLock<RosterAuthority>>,
    pub(super) task_board: Arc<RwLock<TaskBoard>>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) run_store: Arc<dyn MobRunStore>,
    pub(super) provisioner: Arc<dyn MobProvisioner>,
    pub(super) flow_engine: FlowEngine,
    /// Whether this mob's definition declares an orchestrator.
    /// Gates orchestrator-specific transitions and notification fan-out.
    pub(super) has_orchestrator: bool,
    pub(super) run_tasks: BTreeMap<RunId, tokio::task::JoinHandle<()>>,
    pub(super) run_cancel_tokens: BTreeMap<RunId, (tokio_util::sync::CancellationToken, FlowId)>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, McpServerEntry>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
    pub(super) retired_event_index: Arc<RwLock<HashSet<String>>>,
    pub(super) autonomous_initial_turns:
        Arc<tokio::sync::Mutex<BTreeMap<MeerkatId, InitialTurnHandle>>>,
    pub(super) next_spawn_ticket: u64,
    /// Monotonically increasing fence token counter.
    /// Each spawn/respawn/reset issues a strictly newer token.
    /// Uses `AtomicU64` so `&self` methods (batch finalization) can issue tokens.
    pub(super) next_fence_token: std::sync::atomic::AtomicU64,
    pub(super) pending_spawns: PendingSpawnLineage,
    pub(super) edge_locks: Arc<super::edge_locks::EdgeLockRegistry>,
    pub(super) lifecycle_tasks: tokio::task::JoinSet<()>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    pub(super) runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    pub(super) restore_diagnostics:
        Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
    pub(super) runtime_metadata: Arc<dyn crate::store::MobRuntimeMetadataStore>,
    pub(super) supervisor_bridge: Arc<super::MobSupervisorBridge>,
    pub(super) task_board_service: crate::tasks::MobTaskBoardService,
    pub(super) spawn_policy: Arc<super::spawn_policy::SpawnPolicyService>,
    pub(super) dsl_authority: mob_dsl::MobMachineAuthority,
    /// Terminal-phase projection for external observers. Written by the
    /// actor after every DSL phase transition and once more right before
    /// the actor task exits. `MobHandle::status()` falls back to this
    /// `watch` receiver when the command channel has closed (actor has
    /// exited post-Shutdown/Destroy). The watch is an explicit dogma-#13
    /// projection: the actor owns the sole writer, external handles hold
    /// read-only receivers, and the source of truth remains the DSL
    /// authority inside the actor.
    pub(super) phase_watch_tx: tokio::sync::watch::Sender<MobState>,
    pub(super) default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    pub(super) realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    /// Typed composition binding for the `meerkat_mob_seam` composition
    /// (wave-c C-6p). Every routed effect emitted by the mob DSL travels
    /// through `CompositionDispatcher::dispatch` via this binding rather
    /// than through direct peer / provisioner calls. Construction uses
    /// `CompositionBinding::Standalone` by default (test / ephemeral
    /// path); production surface assembly swaps in
    /// `CompositionBinding::Wired(Arc<dyn CompositionDispatcher<...>>)`.
    pub(super) composition_binding: super::composition::MobCompositionBinding,
    /// Routed seam-effects queued by sync `apply_dsl_input` /
    /// `apply_dsl_signal` calls. Drained by `flush_routed_effects` at
    /// every command-loop boundary and after each command handler; the
    /// first dispatch failure surfaces as a typed `MobError`, no silent
    /// drops.
    pub(super) pending_routed_effects: Vec<super::composition::MobSeamEffect>,
}

impl MobActor {
    fn peer_only_member_control_error(
        runtime_mode: crate::MobRuntimeMode,
        action: &str,
    ) -> MobError {
        MobError::UnsupportedForMode {
            mode: runtime_mode,
            reason: format!("{action} is not supported for peer-only members in phase 1"),
        }
    }

    fn peer_only_spec_from_parts(
        peer_id: &str,
        address: &str,
        context: &'static str,
        pubkey: Option<[u8; 32]>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let peer_name = address
            .strip_prefix("inproc://")
            .map(|value| value.split('?').next().unwrap_or(value).to_string())
            .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}"));
        let pubkey = pubkey.or_else(|| {
            let (registry_pubkey, _) =
                meerkat_comms::InprocRegistry::global().get_by_name(&peer_name)?;
            (registry_pubkey.to_peer_id().as_str() == peer_id).then(|| *registry_pubkey.as_bytes())
        });
        let result = match pubkey {
            Some(pubkey) => TrustedPeerDescriptor::unsigned_with_pubkey(
                peer_name,
                peer_id.to_string(),
                pubkey,
                address.to_string(),
            ),
            None => TrustedPeerDescriptor::test_only_unsigned(
                peer_name,
                peer_id.to_string(),
                address.to_string(),
            ),
        };
        result.map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid peer-only runtime spec: {error}"
            ))
        })
    }

    fn peer_only_spec_for_binding(
        binding: &crate::RuntimeBinding,
        context: &'static str,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        match binding {
            crate::RuntimeBinding::External {
                peer_id,
                address,
                pubkey,
                ..
            } => Self::peer_only_spec_from_parts(peer_id, address, context, *pubkey),
            crate::RuntimeBinding::Session => Err(MobError::Internal(format!(
                "{context}: peer-only runtime spec requested for session binding"
            ))),
        }
    }

    fn trusted_peer_removal_key(peer: &TrustedPeerDescriptor) -> String {
        peer.peer_id.to_string()
    }

    async fn remove_trusted_peer_by_descriptor(
        comms: &dyn CoreCommsRuntime,
        peer: &TrustedPeerDescriptor,
    ) -> Result<bool, meerkat_core::comms::SendError> {
        let peer_id = peer.peer_id.to_string();
        comms.remove_trusted_peer(&peer_id).await
    }

    fn supervisor_spec_for_authority(
        mob_id: &crate::MobId,
        authority: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let participant_name = format!("{mob_id}/__mob_supervisor__");
        let public_key = authority.keypair().public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            participant_name.clone(),
            authority.public_peer_id.clone(),
            *public_key.as_bytes(),
            format!("inproc://{participant_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid supervisor spec: {error}")))
    }

    async fn install_supervisor_private_trust_for_session(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        previous_private_trust_removal_key: Option<&str>,
    ) -> Result<SupervisorPrivateTrustInstall, MobError> {
        let spec = self.supervisor_bridge.supervisor_spec().await?;
        let authority = self.supervisor_bridge.authority().await;
        #[cfg(feature = "runtime-adapter")]
        let Some(adapter) = self.runtime_adapter.as_ref() else {
            return Err(MobError::Internal(format!(
                "cannot publish supervisor private trust for session '{session_id}': runtime adapter unavailable"
            )));
        };
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = session_id;
        #[cfg(not(feature = "runtime-adapter"))]
        {
            return Err(MobError::Internal(
                "cannot publish supervisor private trust without runtime adapter".to_string(),
            ));
        }

        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::protocol_supervisor_trust_publish;

            let next_name = spec.name.as_str().to_owned();
            let next_peer_id = spec.peer_id.as_str().to_owned();
            let next_address = spec.address.to_string();
            let next_epoch = authority.epoch;
            let previous = adapter.supervisor_binding(session_id).await;
            let already_bound = matches!(
                &previous,
                meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                    name,
                    peer_id,
                    address,
                    epoch,
                } if name == &next_name
                    && peer_id == &next_peer_id
                    && address == &next_address
                    && *epoch == next_epoch
            );

            let publish_obligation = if already_bound {
                None
            } else {
                let stage_effects = match &previous {
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Unbound => {
                        adapter
                            .stage_supervisor_bind(
                                session_id,
                                next_name.clone(),
                                next_peer_id.clone(),
                                next_address.clone(),
                                next_epoch,
                            )
                            .await
                            .map_err(|error| {
                                MobError::WiringError(format!(
                                    "supervisor private trust bind rejected for session '{session_id}': {error}"
                                ))
                            })?
                    }
                    meerkat_runtime::meerkat_machine::SupervisorBinding::Bound { .. } => {
                        adapter
                            .stage_supervisor_authorize(
                                session_id,
                                next_name.clone(),
                                next_peer_id.clone(),
                                next_address.clone(),
                                next_epoch,
                            )
                            .await
                            .map_err(|error| {
                                MobError::WiringError(format!(
                                    "supervisor private trust rotation rejected for session '{session_id}': {error}"
                                ))
                            })?
                    }
                    _ => {
                        return Err(MobError::WiringError(format!(
                            "supervisor private trust publication for session '{session_id}' saw an unknown supervisor binding variant"
                        )));
                    }
                };
                let obligations =
                    protocol_supervisor_trust_publish::extract_obligations(&stage_effects);
                let obligation = match obligations.as_slice() {
                    [obligation] => obligation.clone(),
                    [] => {
                        return Err(MobError::WiringError(format!(
                            "supervisor private trust publication for session '{session_id}' produced no generated publish obligation"
                        )));
                    }
                    _ => {
                        return Err(MobError::WiringError(format!(
                            "supervisor private trust publication for session '{session_id}' produced multiple generated publish obligations"
                        )));
                    }
                };
                if obligation.name != next_name
                    || obligation.peer_id != next_peer_id
                    || obligation.address != next_address
                    || obligation.epoch != next_epoch
                {
                    return Err(MobError::WiringError(format!(
                        "supervisor private trust publication for session '{session_id}' generated obligation did not match the staged supervisor binding"
                    )));
                }
                Some(obligation)
            };
            let publish_peer_id = publish_obligation
                .as_ref()
                .map(|obligation| obligation.peer_id.clone())
                .unwrap_or_else(|| next_peer_id.clone());
            let publish_epoch = publish_obligation
                .as_ref()
                .map(|obligation| obligation.epoch)
                .unwrap_or(next_epoch);

            if let Err(error) = comms.add_private_trusted_peer(spec.clone()).await {
                let _ = adapter
                    .stage_supervisor_trust_publish_failed(
                        session_id,
                        publish_peer_id.clone(),
                        publish_epoch,
                        error.to_string(),
                    )
                    .await;
                let rollback = self
                    .rollback_supervisor_private_trust_binding(
                        adapter,
                        session_id,
                        &previous,
                        &publish_peer_id,
                        publish_epoch,
                    )
                    .await;
                let mut reason = format!(
                    "supervisor private trust publication failed for session '{session_id}': {error}"
                );
                if let Err(rollback_error) = rollback {
                    reason.push_str(&format!("; rollback failed: {rollback_error}"));
                }
                return Err(MobError::WiringError(reason));
            }

            adapter
                .stage_supervisor_trust_published(
                    session_id,
                    publish_peer_id.clone(),
                    publish_epoch,
                )
                .await
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor private trust publication ack rejected for session '{session_id}': {error}"
                    ))
                })?;

            if let meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                peer_id: previous_peer_id,
                ..
            } = &previous
                && previous_peer_id != &next_peer_id
            {
                let previous_removal_key = previous_private_trust_removal_key
                    .map(str::to_string)
                    .unwrap_or_else(|| previous_peer_id.clone());
                if let Err(error) = comms
                    .remove_private_trusted_peer(&previous_removal_key)
                    .await
                {
                    let rollback = self
                        .rollback_supervisor_private_trust_binding(
                            adapter,
                            session_id,
                            &previous,
                            &next_peer_id,
                            next_epoch,
                        )
                        .await;
                    let next_removal_key = Self::trusted_peer_removal_key(&spec);
                    let cleanup = comms.remove_private_trusted_peer(&next_removal_key).await;
                    let mut reason = format!(
                        "previous supervisor private trust removal failed for session '{session_id}': {error}"
                    );
                    if let Err(rollback_error) = rollback {
                        reason.push_str(&format!("; rollback failed: {rollback_error}"));
                    }
                    if let Err(cleanup_error) = cleanup {
                        reason.push_str(&format!(
                            "; cleanup failed while removing new supervisor trust: {cleanup_error}"
                        ));
                    }
                    return Err(MobError::WiringError(reason));
                }
            }

            Ok(SupervisorPrivateTrustInstall {
                peer_id: next_peer_id,
                epoch: next_epoch,
                removal_key: Self::trusted_peer_removal_key(&spec),
            })
        }
    }

    async fn cleanup_supervisor_private_trust_for_session(
        &self,
        session_id: &SessionId,
        comms: &Arc<dyn CoreCommsRuntime>,
        install: &SupervisorPrivateTrustInstall,
    ) {
        if let Err(error) = comms
            .remove_private_trusted_peer(&install.removal_key)
            .await
        {
            tracing::warn!(
                %session_id,
                peer_id = %install.peer_id,
                epoch = install.epoch,
                %error,
                "failed to clean up supervisor private trust"
            );
        }
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = self.runtime_adapter.as_ref() {
            let _ = adapter
                .stage_supervisor_trust_revoked(session_id, install.peer_id.clone(), install.epoch)
                .await;
            let _ = adapter
                .stage_supervisor_revoke(session_id, install.peer_id.clone(), install.epoch)
                .await;
        }
    }

    #[cfg(feature = "runtime-adapter")]
    async fn rollback_supervisor_private_trust_binding(
        &self,
        adapter: &Arc<meerkat_runtime::MeerkatMachine>,
        session_id: &SessionId,
        previous: &meerkat_runtime::meerkat_machine::SupervisorBinding,
        current_peer_id: &str,
        current_epoch: u64,
    ) -> Result<(), MobError> {
        match previous {
            meerkat_runtime::meerkat_machine::SupervisorBinding::Unbound => adapter
                .stage_supervisor_revoke(session_id, current_peer_id.to_string(), current_epoch)
                .await
                .map_err(|error| MobError::WiringError(error.to_string())),
            meerkat_runtime::meerkat_machine::SupervisorBinding::Bound {
                name,
                peer_id,
                address,
                epoch,
            } => adapter
                .stage_supervisor_authorize(
                    session_id,
                    name.clone(),
                    peer_id.clone(),
                    address.clone(),
                    *epoch,
                )
                .await
                .map(|_| ())
                .map_err(|error| MobError::WiringError(error.to_string())),
            _ => Err(MobError::WiringError(
                "unknown supervisor binding variant during rollback".to_string(),
            )),
        }
    }

    async fn bridge_supervisor_payload(
        &self,
    ) -> Result<super::bridge_protocol::BridgeSupervisorPayload, MobError> {
        let authority = self.supervisor_bridge.authority().await;
        let spec = self.supervisor_bridge.supervisor_spec().await?;
        Ok(super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: spec.into(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
        })
    }

    fn bridge_bootstrap_token_from_binding(
        binding: &crate::RuntimeBinding,
    ) -> Result<super::bridge_protocol::BridgeBootstrapToken, MobError> {
        match binding {
            crate::RuntimeBinding::External {
                address,
                bootstrap_token,
                ..
            } => bootstrap_token
                .as_ref()
                .filter(|token| !token.is_empty())
                .cloned()
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "external runtime binding for '{address}' is missing typed bootstrap_token field"
                    ))
                }),
            crate::RuntimeBinding::Session => Err(MobError::Internal(
                "bridge bootstrap token requested for session binding".to_string(),
            )),
        }
    }

    async fn bind_peer_only_member_for_binding(
        &self,
        peer: &TrustedPeerDescriptor,
        binding: &crate::RuntimeBinding,
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        self.bind_peer_only_member_for_binding_with_payload(peer, binding, &payload)
            .await
    }

    async fn bind_peer_only_member_for_binding_with_payload(
        &self,
        peer: &TrustedPeerDescriptor,
        binding: &crate::RuntimeBinding,
        payload: &super::bridge_protocol::BridgeSupervisorPayload,
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
        let crate::RuntimeBinding::External {
            peer_id,
            address,
            bootstrap_token: _,
            pubkey: _,
        } = binding
        else {
            return Err(MobError::Internal(
                "bind requested for non-external runtime binding".to_string(),
            ));
        };
        let bootstrap_token = Self::bridge_bootstrap_token_from_binding(binding)?;
        let command = super::bridge_protocol::BridgeCommand::BindMember(
            super::bridge_protocol::BridgeBindPayload {
                supervisor: payload.supervisor.clone(),
                epoch: payload.epoch,
                protocol_version: payload.protocol_version,
                expected_peer_id: peer_id.clone(),
                expected_address: address.clone(),
                bootstrap_token,
            },
        );
        self.send_bridge_command_typed(peer, &command, std::time::Duration::from_secs(30))
            .await
    }

    fn bridge_rejection_reply(
        protocol_version: u32,
        value: &serde_json::Value,
    ) -> Option<super::bridge_protocol::BridgeRejectionReply> {
        super::bridge_protocol::decode_bridge_rejection_reply(protocol_version, value)
    }

    async fn persist_rebound_binding(
        &self,
        prior_binding: &crate::RuntimeBinding,
        bind_response: &super::bridge_protocol::BridgeBindResponse,
    ) -> Result<(), MobError> {
        let crate::RuntimeBinding::External {
            peer_id: prior_peer_id,
            ..
        } = prior_binding
        else {
            return Ok(());
        };
        let bootstrap_token = Some(Self::bridge_bootstrap_token_from_binding(prior_binding)?);
        let canonical_address =
            super::bridge_protocol::canonicalize_bridge_address(&bind_response.address);
        let updated_entries = self
            .roster
            .write()
            .await
            .replace_backend_peer_binding_by_peer_id(
                prior_peer_id,
                &bind_response.peer_id,
                &canonical_address,
                bootstrap_token.clone(),
            );
        for (identity, generation) in updated_entries {
            self.runtime_metadata
                .upsert_external_binding_overlay(
                    &self.definition.id,
                    &crate::store::ExternalBindingOverlayRecord {
                        agent_identity: identity,
                        generation,
                        normalized_member_ref: Some(MemberRef::BackendPeer {
                            peer_id: bind_response.peer_id.clone(),
                            address: canonical_address.clone(),
                            bootstrap_token: None,
                            session_id: None,
                        }),
                        bootstrap_token: bootstrap_token.clone(),
                        status: crate::store::ExternalBindingOverlayStatus::Normalized,
                        updated_at: chrono::Utc::now(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn ensure_supervisor_authorized(
        &self,
        peer: &TrustedPeerDescriptor,
        binding: Option<&crate::RuntimeBinding>,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        let protocol_version = payload.protocol_version;
        let command = super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(payload);
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, &command, std::time::Duration::from_secs(30))
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(protocol_version, &value) {
            if let Some(cause) = rejection.typed_cause()
                && super::bridge_fallback::should_fall_back_to_bind(cause)
                && let Some(binding) = binding
            {
                let bind = self
                    .bind_peer_only_member_for_binding(peer, binding)
                    .await?;
                let effective_bootstrap_token = Self::bridge_bootstrap_token_from_binding(binding)?;
                let crate::RuntimeBinding::External { pubkey, .. } = binding else {
                    return Err(MobError::Internal(
                        "bind fallback returned for non-external runtime binding".to_string(),
                    ));
                };
                self.persist_rebound_binding(binding, &bind).await?;
                return Self::peer_only_spec_for_binding(
                    &crate::RuntimeBinding::External {
                        peer_id: bind.peer_id,
                        address: super::bridge_protocol::canonicalize_bridge_address(&bind.address),
                        bootstrap_token: Some(effective_bootstrap_token),
                        pubkey: *pubkey,
                    },
                    "ensure_supervisor_authorized rebound peer",
                );
            }
            return Err(MobError::WiringError(rejection.reason().to_string()));
        }
        let _ack: super::bridge_protocol::BridgeAck =
            serde_json::from_value(value).map_err(|error| {
                MobError::Internal(format!(
                    "failed to decode authorize supervisor response: {error}"
                ))
            })?;
        Ok(peer.clone())
    }

    async fn send_bridge_command_typed<R: DeserializeOwned>(
        &self,
        peer: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: std::time::Duration,
    ) -> Result<R, MobError> {
        self.supervisor_bridge.trust_recipient(peer).await?;
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, command, timeout)
            .await?;
        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
            return Err(MobError::WiringError(rejection.reason().to_string()));
        }
        serde_json::from_value(value).map_err(|error| {
            MobError::Internal(format!("failed to decode bridge command response: {error}"))
        })
    }

    async fn observe_peer_only_binding(
        &self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<super::bridge_protocol::BridgeObservationResponse, MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "observe_peer_only_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::ObserveMember(payload);
        self.send_bridge_command_typed(&peer, &command, timeout)
            .await
    }

    async fn destroy_peer_only_binding(
        &self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<super::bridge_protocol::BridgeDestroyResponse, MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "destroy_peer_only_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::DestroyMember(payload);
        self.send_bridge_command_typed(&peer, &command, timeout)
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn revoke_supervisor_for_binding(
        &self,
        binding: &crate::RuntimeBinding,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let peer = Self::peer_only_spec_for_binding(binding, "revoke_supervisor_for_binding")?;
        let peer = self
            .ensure_supervisor_authorized(&peer, Some(binding))
            .await?;
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::RevokeSupervisor(payload);
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&peer, &command, timeout)
            .await?;
        Ok(())
    }

    async fn wire_peer_only_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerDescriptor,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let recipient = self
            .ensure_supervisor_authorized(recipient, recipient_binding)
            .await?;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        let command = super::bridge_protocol::BridgeCommand::WireMember(
            super::bridge_protocol::BridgePeerWiringPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                peer_spec: peer_spec.clone().into(),
            },
        );
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&recipient, &command, timeout)
            .await?;
        Ok(())
    }

    async fn unwire_peer_only_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerDescriptor,
        timeout: std::time::Duration,
    ) -> Result<(), MobError> {
        let recipient = self
            .ensure_supervisor_authorized(recipient, recipient_binding)
            .await?;
        let authority = self.supervisor_bridge.authority().await;
        let sup_spec = self.supervisor_bridge.supervisor_spec().await?;
        let command = super::bridge_protocol::BridgeCommand::UnwireMember(
            super::bridge_protocol::BridgePeerWiringPayload {
                supervisor: sup_spec.into(),
                epoch: authority.epoch,
                protocol_version: authority.protocol_version,
                peer_spec: peer_spec.clone().into(),
            },
        );
        let _ack: super::bridge_protocol::BridgeAck = self
            .send_bridge_command_typed(&recipient, &command, timeout)
            .await?;
        Ok(())
    }

    fn observation_is_terminal(
        observation: &super::bridge_protocol::BridgeObservationResponse,
    ) -> bool {
        super::bridge::observation_is_terminal(observation)
    }

    /// Issue a strictly increasing fence token.
    fn issue_fence_token(&self) -> crate::ids::FenceToken {
        let val = self
            .next_fence_token
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::ids::FenceToken::new(val)
    }

    fn next_fence_token_preview(&self) -> crate::ids::FenceToken {
        crate::ids::FenceToken::new(
            self.next_fence_token
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn invalid_transition_to(&self, target: MobState) -> MobError {
        MobError::InvalidTransition {
            from: self.state(),
            to: target,
        }
    }

    async fn restore_failure_for(
        &self,
        agent_identity: &MeerkatId,
    ) -> Option<super::handle::RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(agent_identity)
            .cloned()
    }

    async fn ensure_member_not_broken(&self, agent_identity: &MeerkatId) -> Result<(), MobError> {
        if let Some(diag) = self.restore_failure_for(agent_identity).await {
            return Err(MobError::MemberRestoreFailed {
                member_id: agent_identity.clone(),
                session_id: diag.bridge_session_id,
                reason: diag.reason,
            });
        }
        Ok(())
    }

    /// Project the DSL authority's `lifecycle_phase` into the shell
    /// `MobState` enum. The DSL authority is the single source of truth
    /// for the mob phase; external observers read the same value via
    /// `MobCommand::QueryPhase` (dogma #1, #13, #17).
    fn state(&self) -> MobState {
        project_dsl_phase(self.dsl_authority.state.lifecycle_phase)
    }

    fn apply_dsl_input(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<(), MobError> {
        self.apply_dsl_input_collect_effects(input, context)
            .map(|_| ())
    }

    fn apply_dsl_input_collect_effects(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<Vec<mob_dsl::MobMachineEffect>, MobError> {
        let input_debug = format!("{input:?}");
        let transition = mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input)
            .map_err(|e| {
                MobError::Internal(format!(
                    "DSL authority ({context}) rejected {input_debug}: {e}"
                ))
            })?;
        let effects = transition.effects.clone();
        self.queue_routed_effects_from(&transition.effects);
        if transition.from_phase != transition.to_phase {
            self.dsl_authority.state.lifecycle_phase = transition.to_phase;
            // Publish the projected phase for external observers. This is
            // the sole write seam for the dogma-#13 projection watch.
            let _ = self.phase_watch_tx.send(self.state());
        }
        Ok(effects)
    }

    fn prepare_dsl_input(
        &self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<PreparedDslInput, MobError> {
        self.prepare_dsl_inputs(std::slice::from_ref(&input), context)
    }

    fn prepare_dsl_inputs(
        &self,
        inputs: &[mob_dsl::MobMachineInput],
        context: &str,
    ) -> Result<PreparedDslInput, MobError> {
        let mut authority =
            mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
        let mut effects = Vec::new();
        let mut phase_changed = false;
        for input in inputs {
            let input_debug = format!("{input:?}");
            let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input.clone())
                .map_err(|e| {
                    MobError::Internal(format!(
                        "DSL authority prepare ({context}) rejected {input_debug}: {e}"
                    ))
                })?;
            if transition.from_phase != transition.to_phase {
                authority.state.lifecycle_phase = transition.to_phase;
                phase_changed = true;
            }
            effects.extend(transition.effects);
        }
        Ok(PreparedDslInput {
            authority,
            effects,
            phase_changed,
        })
    }

    fn commit_prepared_dsl_input(&mut self, prepared: PreparedDslInput) {
        let effects = prepared.effects;
        let phase_changed = prepared.phase_changed;
        self.dsl_authority = prepared.authority;
        self.queue_routed_effects_from(&effects);
        if phase_changed {
            let _ = self.phase_watch_tx.send(self.state());
        }
    }

    fn preview_dsl_input(
        &self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<mob_dsl::MobMachineState, MobError> {
        let input_debug = format!("{input:?}");
        let mut authority =
            mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input).map_err(|e| {
            MobError::Internal(format!(
                "DSL authority preview ({context}) rejected {input_debug}: {e}"
            ))
        })?;
        if transition.from_phase != transition.to_phase {
            authority.state.lifecycle_phase = transition.to_phase;
        }
        Ok(authority.state)
    }

    fn apply_dsl_signal(
        &mut self,
        signal: mob_dsl::MobMachineSignal,
        context: &str,
    ) -> Result<(), MobError> {
        let signal_debug = format!("{signal:?}");
        let transition = self
            .dsl_authority
            .apply_signal(signal)
            .map_err(|e| {
                MobError::Internal(format!(
                    "DSL authority ({context}): {e}; signal={signal_debug}; live_runtime_ids={:?}; runtime_fence_tokens={:?}",
                    self.dsl_authority.state.live_runtime_ids,
                    self.dsl_authority.state.runtime_fence_tokens,
                ))
            })?;
        self.queue_routed_effects_from(&transition.effects);
        if transition.from_phase != transition.to_phase {
            self.dsl_authority.state.lifecycle_phase = transition.to_phase;
            let _ = self.phase_watch_tx.send(self.state());
        }
        Ok(())
    }

    /// Wave-c C-6p — harvest routed seam effects from a DSL transition's
    /// effect list into the actor's pending-dispatch queue.
    ///
    /// Non-routed variants (persist-kickoff, emit-lifecycle-notice,
    /// topology-signal, etc.) stay on the in-process effect-drain path
    /// reached from the individual command handlers and never enter the
    /// composition dispatcher. Routed variants flow through
    /// `flush_routed_effects` at the next async boundary.
    fn queue_routed_effects_from(&mut self, effects: &[mob_dsl::MobMachineEffect]) {
        for effect in effects {
            if Self::is_placeholder_session_routed_effect(effect) {
                continue;
            }
            if let Some(seam_effect) = super::composition::lift_routed_effect(effect) {
                self.pending_routed_effects.push(seam_effect);
            }
        }
    }

    fn is_placeholder_session_routed_effect(effect: &mob_dsl::MobMachineEffect) -> bool {
        match effect {
            mob_dsl::MobMachineEffect::RequestRuntimeBinding { session_id, .. }
            | mob_dsl::MobMachineEffect::RequestRuntimeRetire { session_id }
            | mob_dsl::MobMachineEffect::RequestRuntimeDestroy { session_id } => {
                session_id.0.is_empty()
            }
            mob_dsl::MobMachineEffect::RequestRuntimeIngress { .. } => false,
            _ => false,
        }
    }

    /// Drain the pending-routed-effect queue, dispatching each payload
    /// through the typed [`CompositionBinding`]. Returns the first typed
    /// [`MobError`] a dispatch yields; subsequent effects remain queued
    /// so the next flush sees them (FIFO).
    ///
    /// In [`CompositionBinding::Standalone`] mode (single-machine / test
    /// construction) this drains the queue without attempting any
    /// dispatch — the producer has no consumer-side surface to target by
    /// construction. In [`CompositionBinding::Wired`] mode,
    /// [`DispatchRefusal::UnwiredConsumer`] surfaces as
    /// [`MobError::WiringError`] per the wave-c spine's intermediate-state
    /// contract (C-6p landed, C-6c pending).
    pub(super) async fn flush_routed_effects(&mut self) -> Result<(), MobError> {
        use super::composition::dispatch_routed_effect;
        while let Some(effect) = self.pending_routed_effects.first().cloned() {
            match dispatch_routed_effect(&self.composition_binding, effect).await {
                Ok(_outcome) => {
                    // Drop the head now that dispatch succeeded (or was a
                    // standalone no-op); keep subsequent queue entries
                    // intact so a later failure preserves FIFO ordering.
                    self.pending_routed_effects.remove(0);
                }
                Err(error) => {
                    // Preserve the head + tail on failure so the operator
                    // can inspect them; caller decides to retry or abort.
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn discard_pending_routed_effects_for_session(&mut self, session_id: &SessionId) {
        let dsl_session_id = mob_dsl::SessionId::from_domain(session_id);
        self.pending_routed_effects.retain(|effect| {
            !matches!(
                effect,
                super::composition::MobSeamEffect::Mob(
                    mob_dsl::MobMachineEffect::RequestRuntimeBinding {
                        session_id,
                        ..
                    }
                    | mob_dsl::MobMachineEffect::RequestRuntimeRetire { session_id }
                    | mob_dsl::MobMachineEffect::RequestRuntimeDestroy { session_id },
                ) if session_id == &dsl_session_id
            )
        });
    }

    /// Snapshot the DSL's current `member_state_markers` as a set of
    /// runtime-id keys (in their stringified DSL form) currently marked
    /// `Retiring`. The stringified form matches
    /// `AgentRuntimeId::Display` (`"identity:generation"`), which is what
    /// `mob_dsl::AgentRuntimeId::from_domain` produces.
    fn retiring_runtime_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state
            .member_state_markers
            .iter()
            .filter_map(|(runtime_id, member_state)| match member_state {
                mob_dsl::MobMemberState::Retiring => Some(runtime_id.0.clone()),
                mob_dsl::MobMemberState::Active => None,
            })
            .collect()
    }

    fn pending_kickoff_member_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state
            .member_kickoff_pending
            .iter()
            .chain(self.dsl_authority.state.member_kickoff_starting.iter())
            .chain(
                self.dsl_authority
                    .state
                    .member_kickoff_callback_pending
                    .iter(),
            )
            .cloned()
            .collect()
    }

    fn ready_runtime_ids_from_dsl(&self) -> std::collections::BTreeSet<String> {
        self.dsl_authority
            .state
            .member_startup_runtime_ready
            .iter()
            .chain(self.dsl_authority.state.member_startup_ready.iter())
            .map(|runtime_id| runtime_id.0.clone())
            .collect()
    }

    fn machine_projection_for_identity(
        &self,
        agent_identity: &crate::ids::AgentIdentity,
    ) -> super::state::MobMemberMachineProjection {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let dsl = &self.dsl_authority.state;
        let runtime_id = dsl.identity_to_runtime.get(&dsl_identity).cloned();
        let state_marker = runtime_id
            .as_ref()
            .and_then(|runtime_id| dsl.member_state_markers.get(runtime_id).copied());
        let live_runtime = runtime_id
            .as_ref()
            .is_some_and(|runtime_id| dsl.live_runtime_ids.contains(runtime_id));
        let bound_session_id = dsl.member_session_bindings.get(&dsl_identity).cloned();
        super::state::MobMemberMachineProjection {
            runtime_id,
            state_marker,
            live_runtime,
            bound_session_id,
        }
    }

    async fn machine_member_material(
        &self,
        agent_identity: &MeerkatId,
        include_session_details: bool,
    ) -> CanonicalMemberSnapshotMaterial {
        let (roster_entry, current_bridge_session_id) = {
            let roster = self.roster.read().await;
            match roster.get(agent_identity) {
                Some(entry) => (
                    Some(entry.clone()),
                    entry.member_ref.bridge_session_id().cloned(),
                ),
                None => (None, None),
            }
        };
        let domain_identity = crate::ids::AgentIdentity::from(agent_identity.as_str());
        let machine_projection = self.machine_projection_for_identity(&domain_identity);
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let machine_bridge_session_id = machine_projection
            .bound_session_id
            .as_ref()
            .and_then(|dsl_session_id| SessionId::parse(&dsl_session_id.0).ok());
        let current_bridge_session_id = current_bridge_session_id.or(machine_bridge_session_id);
        let member_present = roster_entry.is_some();
        let machine_lifecycle = self
            .dsl_authority
            .state
            .member_lifecycle_for_identity(&dsl_identity, member_present);

        let restore_failure = {
            self.restore_diagnostics
                .read()
                .await
                .get(agent_identity)
                .cloned()
        };
        if let Some(diag) = restore_failure {
            return MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
                member_present,
                machine_lifecycle,
                restore_failure: Some(diag.reason),
                output_preview: None,
                tokens_used: 0,
                agent_runtime_id: roster_entry
                    .as_ref()
                    .map(|entry| entry.agent_runtime_id.clone())
                    .unwrap_or_else(|| {
                        crate::ids::AgentRuntimeId::initial(crate::ids::AgentIdentity::from(
                            agent_identity.as_str(),
                        ))
                    }),
                fence_token: roster_entry
                    .as_ref()
                    .map(|entry| entry.fence_token)
                    .unwrap_or(crate::ids::FenceToken::new(0)),
                current_bridge_session_id: diag.bridge_session_id,
                peer_connectivity: None,
                kickoff: roster_entry
                    .as_ref()
                    .and_then(|entry| entry.kickoff.clone()),
            });
        }

        let (output_preview, tokens_used) = match current_bridge_session_id.as_ref() {
            None => (None, 0),
            Some(bridge_session_id) if include_session_details => {
                match self.session_service.read(bridge_session_id).await {
                    Ok(view) => (
                        view.state.last_assistant_text.clone(),
                        view.billing.total_tokens,
                    ),
                    Err(meerkat_core::service::SessionError::NotFound { .. }) => (None, 0),
                    Err(_) => (None, 0),
                }
            }
            Some(_) => (None, 0),
        };

        MobMemberLifecycleProjection::materialize(MobMemberLifecycleInput {
            member_present,
            machine_lifecycle,
            restore_failure: None,
            output_preview,
            tokens_used,
            agent_runtime_id: roster_entry
                .as_ref()
                .map(|entry| entry.agent_runtime_id.clone())
                .unwrap_or_else(|| {
                    crate::ids::AgentRuntimeId::initial(crate::ids::AgentIdentity::from(
                        agent_identity.as_str(),
                    ))
                }),
            fence_token: roster_entry
                .as_ref()
                .map(|entry| entry.fence_token)
                .unwrap_or(crate::ids::FenceToken::new(0)),
            current_bridge_session_id,
            peer_connectivity: None,
            kickoff: roster_entry.and_then(|entry| entry.kickoff),
        })
    }

    async fn project_member_list_from_machine(
        &self,
        include_retiring: bool,
    ) -> Vec<MobMemberListEntry> {
        let entries: Vec<_> = {
            let roster = self.roster.read().await;
            if include_retiring {
                roster.list_all().cloned().collect()
            } else {
                roster.list().cloned().collect()
            }
        };
        let mut projected = Vec::with_capacity(entries.len());
        for entry in entries {
            let snapshot = self
                .machine_member_material(&entry.agent_identity, false)
                .await
                .to_snapshot();
            let current_bridge_session_id = snapshot.current_bridge_session_id().cloned();
            projected.push(
                MobMemberListEntry {
                    agent_identity: entry.agent_identity,
                    agent_runtime_id: entry.agent_runtime_id,
                    fence_token: entry.fence_token,
                    role: entry.role,
                    runtime_mode: entry.runtime_mode,
                    peer_id: entry.peer_id,
                    transport_public_key: entry.transport_public_key,
                    state: entry.state,
                    wired_to: entry.wired_to,
                    external_peer_specs: entry.external_peer_specs,
                    labels: entry.labels,
                    status: snapshot.status,
                    error: snapshot.error,
                    is_final: snapshot.is_final,
                    current_session_id: None,
                    current_bridge_session_id: None,
                    kickoff: snapshot.kickoff,
                }
                .with_current_bridge_session_id(current_bridge_session_id),
            );
        }
        projected
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            definition: self.definition.clone(),
            events: self.events.clone(),
            flow_streams: self.flow_streams.clone(),
            session_service: self.session_service.clone(),
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: self.runtime_adapter.clone(),
            restore_diagnostics: self.restore_diagnostics.clone(),
            phase_watch_rx: self.phase_watch_tx.subscribe(),
            // W2-E: the actor's internal handle-for-tools does not carry the
            // realtime factory — that seam lives on the caller-facing
            // `MobHandle` returned from `MobBuilder`. Tools built from the
            // actor do not dial realtime endpoints.
            realtime_session_factory: None,
        }
    }

    async fn persist_kickoff_state(
        &self,
        agent_identity: &MeerkatId,
        phase: crate::roster::MobMemberKickoffPhase,
        error: Option<String>,
    ) -> Result<(), MobError> {
        let kickoff = crate::roster::MobMemberKickoffSnapshot {
            phase,
            error,
            updated_at: SystemTime::now(),
        };
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberKickoffUpdated {
                    member: AgentIdentity::from(agent_identity.as_str()),
                    kickoff: kickoff.clone(),
                },
            })
            .await
            .map_err(MobError::from)?;
        self.roster
            .write()
            .await
            .set_kickoff(agent_identity, Some(kickoff));
        Ok(())
    }

    async fn kickoff_phase_for(
        &self,
        agent_identity: &MeerkatId,
    ) -> Option<crate::roster::MobMemberKickoffPhase> {
        self.roster
            .read()
            .await
            .get(agent_identity)
            .and_then(|entry| entry.kickoff.as_ref().map(|snapshot| snapshot.phase))
    }

    /// Project the shell-side [`TaskStatus`] onto the DSL bridging enum
    /// consumed by `MobMachineInput::TaskUpdate` guards. Shell's `Open` is
    /// the "not-started, accepting-claims" state; the DSL names that state
    /// `Pending`.
    fn task_status_to_dsl(status: crate::tasks::TaskStatus) -> mob_dsl::TaskStatus {
        match status {
            crate::tasks::TaskStatus::Open => mob_dsl::TaskStatus::Pending,
            crate::tasks::TaskStatus::InProgress => mob_dsl::TaskStatus::InProgress,
            crate::tasks::TaskStatus::Completed => mob_dsl::TaskStatus::Completed,
            crate::tasks::TaskStatus::Cancelled => mob_dsl::TaskStatus::Cancelled,
        }
    }

    /// Build the DSL's opaque [`MobTask`] payload from the shell-side
    /// `subject` / `description` / `blocked_by` on a newly-created task.
    /// The DSL only observes `tasks.contains_key(id)` in guards, so the
    /// richer shell fields (`created_at`, `updated_at`, `id`) do not need
    /// to survive projection.
    fn task_payload_to_dsl(
        subject: &str,
        description: &str,
        blocked_by: &[crate::ids::TaskId],
    ) -> mob_dsl::MobTask {
        mob_dsl::MobTask {
            subject: subject.to_string(),
            description: description.to_string(),
            status: mob_dsl::TaskStatus::Pending,
            owner: None,
            blocked_by: blocked_by
                .iter()
                .map(|id| mob_dsl::TaskId::from(id.as_str()))
                .collect(),
        }
    }

    fn kickoff_phase_to_dsl(phase: crate::roster::MobMemberKickoffPhase) -> mob_dsl::KickoffPhase {
        match phase {
            crate::roster::MobMemberKickoffPhase::Pending => mob_dsl::KickoffPhase::Pending,
            crate::roster::MobMemberKickoffPhase::Starting => mob_dsl::KickoffPhase::Starting,
            crate::roster::MobMemberKickoffPhase::Started => mob_dsl::KickoffPhase::Started,
            crate::roster::MobMemberKickoffPhase::CallbackPending => {
                mob_dsl::KickoffPhase::CallbackPending
            }
            crate::roster::MobMemberKickoffPhase::Failed => mob_dsl::KickoffPhase::Failed,
            crate::roster::MobMemberKickoffPhase::Cancelled => mob_dsl::KickoffPhase::Cancelled,
        }
    }

    fn kickoff_phase_from_dsl(
        phase: mob_dsl::KickoffPhase,
    ) -> crate::roster::MobMemberKickoffPhase {
        match phase {
            mob_dsl::KickoffPhase::Pending => crate::roster::MobMemberKickoffPhase::Pending,
            mob_dsl::KickoffPhase::Starting => crate::roster::MobMemberKickoffPhase::Starting,
            mob_dsl::KickoffPhase::Started => crate::roster::MobMemberKickoffPhase::Started,
            mob_dsl::KickoffPhase::CallbackPending => {
                crate::roster::MobMemberKickoffPhase::CallbackPending
            }
            mob_dsl::KickoffPhase::Failed => crate::roster::MobMemberKickoffPhase::Failed,
            mob_dsl::KickoffPhase::Cancelled => crate::roster::MobMemberKickoffPhase::Cancelled,
        }
    }

    fn kickoff_notice_intent(
        intent: crate::machines::mob_machine::KickoffIntent,
    ) -> Option<&'static str> {
        use crate::machines::mob_machine::KickoffIntent;
        match intent {
            KickoffIntent::Failed => Some("mob.kickoff_failed"),
            KickoffIntent::Cancelled => Some("mob.kickoff_cancelled"),
            KickoffIntent::Pending
            | KickoffIntent::Starting
            | KickoffIntent::Started
            | KickoffIntent::CallbackPending => None,
        }
    }

    async fn clear_kickoff_state(&mut self, agent_identity: &MeerkatId) {
        match self
            .apply_kickoff_input(
                agent_identity,
                mob_dsl::MobMachineInput::KickoffClear {
                    member_id: agent_identity.to_string(),
                },
            )
            .await
        {
            Ok(true) => {
                self.roster.write().await.set_kickoff(agent_identity, None);
            }
            Ok(false) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    "kickoff clear rejected by MobMachine; roster projection left unchanged"
                );
            }
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    %error,
                    "kickoff clear failed"
                );
            }
        }
    }

    async fn fail_startup_to_stopped(&mut self, failure_label: &'static str) {
        if let Err(stop_error) = self.stop_all_autonomous_members().await {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %stop_error,
                "failed cleaning up autonomous host loops after startup error"
            );
        }
        if let Err(stop_error) = self.stop_mcp_servers().await {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %stop_error,
                "failed cleaning up mcp servers after startup error"
            );
        }
        if let Err(error) =
            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "stop_after_startup_failure")
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                error = %error,
                failure_label,
                "authority rejected Stop after startup failure"
            );
        }
    }

    async fn apply_kickoff_input(
        &mut self,
        agent_identity: &MeerkatId,
        input: mob_dsl::MobMachineInput,
    ) -> Result<bool, MobError> {
        let transition = match mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input) {
            Ok(transition) => transition,
            Err(_) => return Ok(false),
        };

        // Wave-c C-6p — harvest routed seam effects from this transition
        // into the pending-dispatch queue. Non-routed kickoff-specific
        // variants continue to be handled in the per-effect match below;
        // routed variants (`Request*`) are NOT silently dropped by the
        // prior `_ => {}` arm — they're lifted through
        // `lift_routed_effect` and await `flush_routed_effects`.
        self.queue_routed_effects_from(&transition.effects);

        for effect in transition.effects {
            match effect {
                mob_dsl::MobMachineEffect::PersistKickoffUpdate {
                    member_id: _,
                    phase,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(agent_identity, phase, None)
                        .await?;
                }
                mob_dsl::MobMachineEffect::PersistKickoffFailureUpdate {
                    member_id: _,
                    phase,
                    error,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(agent_identity, phase, Some(error))
                        .await?;
                }
                mob_dsl::MobMachineEffect::EmitKickoffLifecycleNotice {
                    member_id: _,
                    intent,
                } => {
                    if let Some(notice_intent) = Self::kickoff_notice_intent(intent)
                        && let Err(error) = self
                            .notify_kickoff_event(agent_identity, notice_intent)
                            .await
                    {
                        tracing::warn!(
                            agent_identity = %agent_identity,
                            error = %error,
                            intent = %intent,
                            "failed to emit kickoff lifecycle notice"
                        );
                    }
                }
                // Routed seam effects (`Request*`) were harvested above
                // into `pending_routed_effects`; drain happens at the
                // next async boundary via `flush_routed_effects`.
                mob_dsl::MobMachineEffect::RequestRuntimeBinding { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeIngress { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeRetire { .. }
                | mob_dsl::MobMachineEffect::RequestRuntimeDestroy { .. } => {}
                _ => {}
            }
        }

        Ok(true)
    }

    /// Count of in-flight flow runs tracked by the mob actor's shell,
    /// reported by test-only snapshots (`MobOrchestratorSnapshot`,
    /// `MobLifecycleSnapshot`). Wave-c WAR-1: this is shell-state
    /// introspection, not an authority read seam — `run_cancel_tokens`
    /// is the actor's own `BTreeMap<RunId, CancellationToken>` and
    /// carries no DSL semantics. Production callers on the
    /// `apply_in_phase` / `can_accept_in_phase` path (historical, see
    /// docs/architecture/machine-simplification-proposal.md) were
    /// removed earlier, leaving only the cfg-test snapshot sites; gate
    /// the method the same way its callers are gated so the
    /// `NoDeadAuthorityWiring` rule can drop the silencing allow
    /// without widening production reachability.
    #[cfg(test)]
    fn machine_active_run_count(&self) -> u32 {
        self.run_cancel_tokens.len() as u32
    }

    /// Project an observable orchestrator snapshot from DSL state, if this mob
    /// has an orchestrator. Returns `None` for plain mobs.
    #[cfg(test)]
    fn machine_orchestrator_snapshot(&self, phase: MobState) -> Option<MobOrchestratorSnapshot> {
        if !self.has_orchestrator {
            return None;
        }
        let coordinator_bound = self.dsl_authority.state.coordinator_bound;
        Some(MobOrchestratorSnapshot {
            phase,
            coordinator_bound,
            pending_spawn_count: self.dsl_authority.state.pending_spawn_count as u32,
            active_flow_count: self.machine_active_run_count(),
            // topology_revision and supervisor_active are shell diagnostics not
            // tracked by the DSL; project supervisor_active from coordinator_bound
            // to preserve existing test expectations.
            topology_revision: 0,
            supervisor_active: coordinator_bound,
        })
    }

    /// Probe a DSL input against a clone of current state — returns
    /// `InvalidTransition` if the DSL rejects the input, without
    /// mutating the live authority. Used as the pre-admission gate for
    /// simple lifecycle commands whose side effects can only run after the
    /// target transition is known to be accepted.
    ///
    /// `target` is used purely for the `InvalidTransition` error hint
    /// (the phase the caller is trying to reach). The DSL rejects on the
    /// guard level, not a phase comparison.
    fn probe_mob_machine_input(
        &self,
        input: mob_dsl::MobMachineInput,
        target: MobState,
    ) -> Result<(), MobError> {
        let mut probe = mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
        if mob_dsl::MobMachineMutator::apply(&mut probe, input).is_err() {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: target,
            });
        }
        Ok(())
    }

    /// Guard that the mob is in one of the `allowed` phases.
    ///
    /// Used by command handlers that operate *within* the current state
    /// (retire, wire, external turn, etc.). The first allowed state is used
    /// as the `to` hint in the error.
    fn require_state(&self, allowed: &[MobState]) -> Result<(), MobError> {
        if allowed.contains(&self.state()) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: allowed[0],
            })
        }
    }

    async fn notify_orchestrator_lifecycle(&mut self, message: String) {
        // Drain completed lifecycle tasks (non-blocking).
        while let Some(result) = self.lifecycle_tasks.try_join_next() {
            if let Err(error) = result {
                tracing::debug!(error = %error, "lifecycle notification task failed");
            }
        }

        let Some(orchestrator) = &self.definition.orchestrator else {
            return;
        };
        let Some(orchestrator_entry) = self
            .roster
            .read()
            .await
            .by_profile(&orchestrator.profile)
            .next()
            .cloned()
        else {
            return;
        };

        // Backpressure: drop notification if at capacity.
        if self.lifecycle_tasks.len() >= MAX_LIFECYCLE_NOTIFICATION_TASKS {
            tracing::warn!(
                mob_id = %self.definition.id,
                pending = self.lifecycle_tasks.len(),
                "lifecycle notification dropped: task limit reached"
            );
            return;
        }

        let provisioner = self.provisioner.clone();
        let member_ref = orchestrator_entry.member_ref;
        let runtime_mode = orchestrator_entry.runtime_mode;
        let agent_identity = orchestrator_entry.agent_identity;
        self.lifecycle_tasks.spawn(async move {
            let result = match runtime_mode {
                crate::MobRuntimeMode::AutonomousHost => {
                    let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                        return;
                    };
                    let Some(injector) = provisioner
                        .interaction_event_injector(bridge_session_id)
                        .await
                    else {
                        return;
                    };
                    injector
                        .inject(
                            message.into(),
                            meerkat_core::PlainEventSource::Rpc,
                            meerkat_core::types::HandlingMode::Queue,
                            None,
                        )
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "orchestrator lifecycle inject failed for '{agent_identity}': {error}"
                            ))
                        })
                }
                crate::MobRuntimeMode::TurnDriven => {
                    provisioner
                        .start_turn(
                            &member_ref,
                            meerkat_core::service::StartTurnRequest {
                                prompt: message.into(),
                                system_prompt: None,
                                render_metadata: None,
                                handling_mode: meerkat_core::types::HandlingMode::Queue,
                                event_tx: None,

                                skill_references: None,
                                flow_tool_overlay: None,
                                turn_metadata: None,

                            },
                        )
                        .await
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    orchestrator_member_ref = ?member_ref,
                    error = %error,
                    "failed to notify orchestrator lifecycle turn"
                );
            }
        });
    }

    fn retire_event_key(agent_identity: &MeerkatId, member_ref: &MemberRef) -> String {
        let member =
            serde_json::to_string(member_ref).unwrap_or_else(|_| format!("{member_ref:?}"));
        format!("{agent_identity}|{member}")
    }

    fn pending_spawn_maps_aligned(&self) -> bool {
        self.pending_spawn_alignment_violation().is_none()
    }

    fn pending_spawn_alignment_violation(&self) -> Option<String> {
        let expected = if self.has_orchestrator {
            Some(self.dsl_authority.state.pending_spawn_count as usize)
        } else {
            None
        };
        if let Some(message) = self.pending_spawns.alignment_violation(expected) {
            return Some(message);
        }
        if self.has_orchestrator {
            let dsl_pending = self
                .dsl_authority
                .state
                .pending_spawn_sessions
                .iter()
                .map(|(identity, session_id)| (identity.0.clone(), session_id.0.clone()))
                .collect::<BTreeMap<_, _>>();
            let local_pending = self.pending_spawns.member_session_pairs();
            if dsl_pending != local_pending {
                return Some(format!(
                    "pending admission mismatch: dsl={dsl_pending:?}, local={local_pending:?}"
                ));
            }
        }
        None
    }

    fn ensure_pending_spawn_alignment(&self, context: &str) -> Result<(), MobError> {
        if let Some(message) = self.pending_spawn_alignment_violation() {
            return Err(MobError::Internal(format!(
                "{context}: pending spawn alignment violation: {message}"
            )));
        }
        Ok(())
    }

    fn debug_assert_pending_spawn_alignment(&self) {
        debug_assert!(
            self.pending_spawn_maps_aligned(),
            "pending spawn alignment must hold across pending maps and orchestrator count"
        );
    }

    fn insert_pending_spawn(
        &mut self,
        spawn_ticket: u64,
        pending: PendingSpawn,
        task: tokio::task::JoinHandle<()>,
    ) {
        let impact = self.pending_spawns.insert(spawn_ticket, pending, task);
        if let PendingSpawnInsertImpact::Collided { replaced_identity } = impact {
            // StageSpawn has already been accepted for the new slot in enqueue paths.
            // If we replaced a prior slot at the same ticket, close that prior
            // staged snapshot now so authority counters cannot drift silently.
            if let Some(replaced_identity) = replaced_identity.as_ref() {
                self.complete_orchestrator_spawn(
                    Some(spawn_ticket),
                    replaced_identity,
                    "pending spawn slot collision replaced existing entry",
                );
            }
            tracing::warn!(
                spawn_ticket,
                "pending spawn slot collision replaced existing entry"
            );
        }
        self.debug_assert_pending_spawn_alignment();
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                spawn_ticket,
                message = %message,
                "pending spawn alignment violated after insert"
            );
        }
    }

    fn take_pending_spawn_slot(
        &mut self,
        spawn_ticket: u64,
    ) -> (Option<PendingSpawn>, Option<tokio::task::JoinHandle<()>>) {
        self.pending_spawns
            .take_slot(spawn_ticket)
            .map_or((None, None), |slot| (Some(slot.spawn), slot.task))
    }

    fn complete_pending_spawn_slot(
        &mut self,
        spawn_ticket: u64,
        context: &'static str,
    ) -> (Option<PendingSpawn>, Option<tokio::task::JoinHandle<()>>) {
        let (pending, task) = self.take_pending_spawn_slot(spawn_ticket);
        if pending.is_some() || task.is_some() {
            if let Some(pending) = pending.as_ref() {
                self.complete_orchestrator_spawn(
                    Some(spawn_ticket),
                    &pending.agent_identity,
                    context,
                );
            }
        }
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                spawn_ticket,
                context,
                message = %message,
                "pending spawn alignment violated after completion"
            );
        }
        (pending, task)
    }

    fn stage_orchestrator_spawn(
        &mut self,
        agent_identity: &MeerkatId,
        session_id: &SessionId,
    ) -> Result<(), MobError> {
        if self.has_orchestrator {
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::StageSpawn {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(
                        agent_identity.as_str(),
                    )),
                    session_id: mob_dsl::SessionId::from_domain(session_id),
                },
                "stage_spawn",
            )?;
        }
        Ok(())
    }

    fn preview_spawn_admission(
        &self,
        agent_identity: &MeerkatId,
        external_addressable: bool,
        bridge_session_id: &SessionId,
    ) -> Result<(), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let replacing = self
            .dsl_authority
            .state
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();

        self.preview_dsl_input(
            mob_dsl::MobMachineInput::Spawn {
                agent_identity: dsl_identity,
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                    &crate::ids::AgentRuntimeId::initial(domain_identity),
                ),
                fence_token: mob_dsl::FenceToken::from_domain(self.next_fence_token_preview()),
                generation: mob_dsl::Generation::from_domain(crate::ids::Generation::INITIAL),
                external_addressable,
                bridge_session_id: mob_dsl::SessionId::from_domain(bridge_session_id),
                replacing,
            },
            "spawn_command_admission",
        )
        .map(|_| ())
        .map_err(|_| self.invalid_transition_to(MobState::Running))
    }

    fn submit_work_rejection_for_machine_state(
        &self,
        machine_state: &mob_dsl::MobMachineState,
        dsl_runtime_id: &mob_dsl::AgentRuntimeId,
        origin: WorkOrigin,
        agent_identity: &MeerkatId,
    ) -> MobError {
        if project_dsl_phase(machine_state.lifecycle_phase) != MobState::Running {
            return MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Running,
            };
        }
        if !machine_state.live_runtime_ids.contains(dsl_runtime_id) {
            return MobError::MemberNotFound(agent_identity.clone());
        }
        if matches!(origin, WorkOrigin::External)
            && !machine_state
                .externally_addressable_runtime_ids
                .contains(dsl_runtime_id)
        {
            return MobError::NotExternallyAddressable(agent_identity.clone());
        }
        match origin {
            WorkOrigin::External => MobError::NotExternallyAddressable(agent_identity.clone()),
            WorkOrigin::Internal => MobError::MemberNotFound(agent_identity.clone()),
        }
    }

    fn preview_policy_spawn_submit_work_admission(
        &self,
        agent_identity: &MeerkatId,
        external_addressable: bool,
        work_ref: &WorkRef,
        origin: WorkOrigin,
    ) -> Result<(), MobError> {
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let domain_runtime_id =
            crate::ids::AgentRuntimeId::new(domain_identity, crate::ids::Generation::INITIAL);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&domain_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(self.next_fence_token_preview());
        let replacing = self
            .dsl_authority
            .state
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        let mut authority =
            mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
        let spawn = mob_dsl::MobMachineInput::Spawn {
            agent_identity: dsl_identity,
            agent_runtime_id: dsl_runtime_id.clone(),
            fence_token: dsl_fence_token,
            generation: mob_dsl::Generation::from_domain(crate::ids::Generation::INITIAL),
            external_addressable,
            bridge_session_id: mob_dsl::SessionId::default(),
            replacing,
        };
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, spawn)
            .map_err(|_| self.invalid_transition_to(MobState::Running))?;
        if transition.from_phase != transition.to_phase {
            authority.state.lifecycle_phase = transition.to_phase;
        }

        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: mob_dsl::WorkId::from_work_ref(work_ref),
                origin: mob_dsl::WorkOrigin::from(origin),
            },
        )
        .map(|_| ())
        .map_err(|_| {
            self.submit_work_rejection_for_machine_state(
                &authority.state,
                &dsl_runtime_id,
                origin,
                agent_identity,
            )
        })
    }

    fn complete_orchestrator_spawn(
        &mut self,
        spawn_ticket: Option<u64>,
        agent_identity: &MeerkatId,
        context: &'static str,
    ) {
        if self.has_orchestrator
            && let Err(error) = self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::CompleteSpawn {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(
                        agent_identity.as_str(),
                    )),
                },
                "complete_spawn",
            )
        {
            if let Some(spawn_ticket) = spawn_ticket {
                tracing::warn!(
                    spawn_ticket,
                    agent_identity = %agent_identity,
                    error = %error,
                    context,
                    "failed to reconcile orchestrator pending-spawn snapshot"
                );
            } else {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    error = %error,
                    context,
                    "failed to reconcile orchestrator pending-spawn snapshot"
                );
            }
        }
    }

    async fn flow_tracker_alignment_violation(&self) -> Option<String> {
        let snapshot = self.flow_tracker_snapshot().await;
        let run_task_ids = snapshot.run_task_ids;
        let run_token_ids = snapshot.cancel_token_ids;
        if run_task_ids != run_token_ids {
            return Some(format!(
                "run task/token tracker mismatch: tasks={run_task_ids:?}, tokens={run_token_ids:?}"
            ));
        }

        let stream_ids = snapshot.stream_ids;
        let unknown_streams = stream_ids
            .iter()
            .filter(|run_id| !run_task_ids.contains(*run_id))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown_streams.is_empty() {
            return Some(format!(
                "flow stream tracker contains unknown runs: {unknown_streams:?}"
            ));
        }

        None
    }

    async fn flow_tracker_snapshot(&self) -> super::MobFlowTrackerSnapshot {
        super::MobFlowTrackerSnapshot {
            run_task_ids: self
                .run_tasks
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            cancel_token_ids: self
                .run_cancel_tokens
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            stream_ids: self
                .flow_streams
                .lock()
                .await
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            tracked_flows: self
                .run_cancel_tokens
                .iter()
                .map(|(run_id, (_, flow_id))| (run_id.clone(), flow_id.clone()))
                .collect(),
        }
    }

    async fn ensure_flow_tracker_alignment(&self, context: &str) -> Result<(), MobError> {
        if let Some(message) = self.flow_tracker_alignment_violation().await {
            return Err(MobError::Internal(format!(
                "{context}: flow tracker alignment violation: {message}"
            )));
        }
        Ok(())
    }

    async fn stop_mcp_servers(&self) -> Result<(), MobError> {
        let mut servers = self.mcp_servers.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        let mut first_error: Option<MobError> = None;
        for (_name, entry) in servers.iter_mut() {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(child) = entry.process.as_mut() {
                if let Err(error) = child.kill().await {
                    let mob_error =
                        MobError::Internal(format!("failed to stop mcp server '{_name}': {error}"));
                    tracing::warn!(error = %mob_error, "mcp server kill failed");
                    if first_error.is_none() {
                        first_error = Some(mob_error);
                    }
                }
                if let Err(error) = child.wait().await {
                    let mob_error = MobError::Internal(format!(
                        "failed waiting for mcp server '{_name}' to exit: {error}"
                    ));
                    tracing::warn!(error = %mob_error, "mcp server wait failed");
                    if first_error.is_none() {
                        first_error = Some(mob_error);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                entry.process = None;
            }
            entry.running = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn start_mcp_servers(&self) -> Result<(), MobError> {
        let mut servers = self.mcp_servers.lock().await;
        for (name, cfg) in &self.definition.mcp_servers {
            if cfg.command.is_empty() {
                continue;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if servers
                    .get(name)
                    .is_some_and(|entry| entry.process.is_some())
                {
                    continue;
                }
                let mut cmd = Command::new(&cfg.command[0]);
                for arg in cfg.command.iter().skip(1) {
                    cmd.arg(arg);
                }
                for (k, v) in &cfg.env {
                    cmd.env(k, v);
                }
                let child = cmd.spawn().map_err(|error| {
                    MobError::Internal(format!(
                        "failed to start mcp server '{name}' command '{}': {error}",
                        cfg.command.join(" ")
                    ))
                })?;
                servers.insert(
                    name.clone(),
                    McpServerEntry {
                        process: Some(child),
                        running: true,
                    },
                );
            }
            #[cfg(target_arch = "wasm32")]
            servers.insert(name.clone(), McpServerEntry { running: true });
        }
        // Mark any servers that were already in the map but had no command
        // (i.e. URL-only servers) as running.
        for (name, entry) in servers.iter_mut() {
            if self.definition.mcp_servers.contains_key(name) {
                entry.running = true;
            }
        }
        Ok(())
    }

    async fn cleanup_namespace(&self) -> Result<(), MobError> {
        self.mcp_servers.lock().await.clear();
        Ok(())
    }

    fn fallback_spawn_prompt(
        &self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
    ) -> String {
        format!(
            "You have been spawned as '{}' (role: {}) in mob '{}'.",
            agent_identity, profile_name, self.definition.id
        )
    }

    /// Start the autonomous runtime for a member and deliver its initial prompt.
    ///
    /// Sets up the keep-alive infrastructure (comms drain, dispatch capability)
    /// then delivers the prompt as a normal turn. The session was created with
    /// `InitialTurnPolicy::Defer` so the prompt hasn't been executed yet.
    ///
    /// Two paths:
    /// - **Runtime-backed (adapter present):** Builds `Input::Prompt` and calls
    ///   `accept_input_with_completion` for a true admission ack. Spawns a
    ///   background task for completion wait + barrier signal.
    /// - **No adapter (test/ephemeral):** Falls back to `provisioner.start_turn()`
    ///   in a spawned task with yield-check for immediate failure detection.
    #[cfg(feature = "runtime-adapter")]
    async fn start_autonomous_member(
        &mut self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
        prompt: meerkat_core::types::ContentInput,
    ) -> Result<(), MobError> {
        self.ensure_autonomous_runtime_ready(agent_identity, member_ref)
            .await?;

        let startup_marker = {
            let roster = self.roster.read().await;
            roster
                .get_by_identity(&AgentIdentity::from(agent_identity.as_str()))
                .map(|entry| {
                    (
                        mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                        mob_dsl::FenceToken::from_domain(entry.fence_token),
                    )
                })
        }
        .ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' missing roster entry for startup readiness"
            ))
        })?;

        if !self
            .dsl_authority
            .state
            .member_startup_ready
            .contains(&startup_marker.0)
        {
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::StartupMarkReady {
                    agent_runtime_id: startup_marker.0,
                    fence_token: startup_marker.1,
                },
                "start_autonomous_member/startup_mark_ready",
            )?;
        }

        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' must be session-backed"
            ))
        })?;

        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' requires admission-capable substrate (runtime adapter)"
            ))
        })?;

        {
            // Runtime-backed path: true admission ack via accept_input_with_completion.
            use meerkat_runtime::{Input, InputHeader, PromptInput};

            let input = Input::Prompt(PromptInput {
                header: InputHeader {
                    id: meerkat_core::lifecycle::InputId::new(),
                    timestamp: chrono::Utc::now(),
                    source: meerkat_runtime::InputOrigin::Operator,
                    durability: meerkat_runtime::InputDurability::Durable,
                    visibility: meerkat_runtime::InputVisibility::default(),
                    idempotency_key: None,
                    supersession_key: None,
                    correlation_id: None,
                },
                text: prompt.text_content(),
                blocks: if prompt.has_images() {
                    Some(prompt.into_blocks())
                } else {
                    None
                },
                turn_metadata: None,
            });

            let (_outcome, completion_handle) = adapter
                .accept_input_with_completion(bridge_session_id, input)
                .await
                .map_err(|e| {
                    MobError::Internal(format!(
                        "autonomous prompt admission failed for '{agent_identity}': {e}"
                    ))
                })?;

            // Spawn background task for completion wait.
            let log_id = agent_identity.clone();
            let completion_command_tx = self.command_tx.clone();
            let handle = tokio::spawn(async move {
                if let Some(h) = completion_handle {
                    let outcome = h.wait().await;
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if completion_command_tx
                        .send(MobCommand::KickoffOutcomeResolved {
                            agent_identity: log_id.clone(),
                            outcome,
                            ack_tx,
                        })
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            agent_identity = %log_id,
                            "mob actor dropped before kickoff outcome could be recorded"
                        );
                    } else {
                        let _ = ack_rx.await;
                    }
                }
            });

            self.autonomous_initial_turns
                .lock()
                .await
                .insert(agent_identity.clone(), InitialTurnHandle { handle });
        }

        tracing::debug!(agent_identity = %agent_identity, "autonomous member started");
        Ok(())
    }

    async fn ensure_autonomous_runtime_ready(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Session registration + RuntimeLoop attachment is owned by the
        // provisioner's lazy `runtime_session_state()` init (called during
        // provision_member). stop_autonomous_member preserves registration
        // (only aborts the drain), so resume just needs to re-spawn the drain.
        self.ensure_mob_comms_drain(agent_identity, member_ref)
            .await?;

        self.ensure_autonomous_dispatch_capability(agent_identity, member_ref)
            .await
    }

    async fn ensure_mob_comms_drain(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
        {
            let Some(bridge_session_id) = member_ref.bridge_session_id() else {
                return Ok(());
            };

            if let Some(adapter) = self.runtime_adapter.clone()
                && let Some(comms_runtime) = self.provisioner.comms_runtime(member_ref).await
            {
                let mob_id =
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(self.definition.id.as_ref());
                let spawned = adapter
                    .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                    .await;
                if spawned {
                    tracing::debug!(
                        agent_identity = %agent_identity,
                        session_id = %bridge_session_id,
                        "updated peer ingress for mob member"
                    );
                }
            }
        }

        #[cfg(any(target_arch = "wasm32", not(feature = "runtime-adapter")))]
        {
            let _ = (agent_identity, member_ref);
        }

        Ok(())
    }

    async fn teardown_autonomous_runtime(&self, member_ref: &MemberRef) {
        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(bridge_session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            adapter.unregister_session(bridge_session_id).await;
        }
    }

    async fn teardown_session_runtime_bindings_from_roster(&self) {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = &self.runtime_adapter {
            let session_ids = {
                let roster = self.roster.read().await;
                roster
                    .list()
                    .filter_map(|entry| entry.member_ref.bridge_session_id().cloned())
                    .collect::<Vec<_>>()
            };
            for session_id in session_ids {
                adapter.abort_comms_drain(&session_id).await;
                adapter.unregister_session(&session_id).await;
            }
        }
    }

    async fn ensure_autonomous_dispatch_capability_for_provisioner(
        provisioner: &Arc<dyn MobProvisioner>,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' must be session-backed for injector dispatch"
            ))
        })?;
        if provisioner
            .interaction_event_injector(bridge_session_id)
            .await
            .is_none()
        {
            return Err(MobError::Internal(format!(
                "autonomous member '{agent_identity}' is missing event injector capability"
            )));
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    async fn resolve_kickoff_outcome(
        &mut self,
        agent_identity: &MeerkatId,
        outcome: meerkat_runtime::completion::CompletionOutcome,
    ) -> Result<(), MobError> {
        if let meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } =
            &outcome
        {
            tracing::debug!(
                agent_identity = %agent_identity,
                tool_name = %tool_name,
                args = ?args,
                "autonomous kickoff reached callback-pending boundary"
            );
        }

        let _ = self
            .apply_kickoff_input(
                agent_identity,
                match outcome {
                    meerkat_runtime::completion::CompletionOutcome::Completed(_)
                    | meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => {
                        mob_dsl::MobMachineInput::KickoffResolveStarted {
                            member_id: agent_identity.to_string(),
                        }
                    }
                    meerkat_runtime::completion::CompletionOutcome::CallbackPending { .. } => {
                        mob_dsl::MobMachineInput::KickoffResolveCallbackPending {
                            member_id: agent_identity.to_string(),
                        }
                    }
                    meerkat_runtime::completion::CompletionOutcome::Abandoned(error)
                    | meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(error) => {
                        mob_dsl::MobMachineInput::KickoffResolveFailed {
                            member_id: agent_identity.to_string(),
                            error,
                        }
                    }
                },
            )
            .await?;
        Ok(())
    }

    async fn maybe_mark_kickoff_cancelled(&mut self, agent_identity: &MeerkatId) {
        if let Err(error) = self
            .apply_kickoff_input(
                agent_identity,
                mob_dsl::MobMachineInput::KickoffCancelRequested {
                    member_id: agent_identity.to_string(),
                },
            )
            .await
        {
            tracing::warn!(
                agent_identity = %agent_identity,
                error = %error,
                "failed to apply kickoff cancellation transition"
            );
        }
    }

    async fn ensure_autonomous_dispatch_capability(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            agent_identity,
            member_ref,
        )
        .await
    }

    /// Stop an autonomous member: abort initial turn, interrupt, abort comms drain.
    ///
    /// Does NOT unregister the session — that happens only on dispose (retire/destroy).
    /// This allows resume to re-spawn the comms drain without re-registering.
    async fn stop_autonomous_member(
        &mut self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Abort any in-flight initial turn.
        let had_kickoff_handle = self
            .autonomous_initial_turns
            .lock()
            .await
            .contains_key(agent_identity);
        if had_kickoff_handle {
            self.maybe_mark_kickoff_cancelled(agent_identity).await;
        }
        if let Some(handle) = self
            .autonomous_initial_turns
            .lock()
            .await
            .remove(agent_identity)
        {
            handle.abort();
        }
        if let Err(error) = self.provisioner.interrupt_member(member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(error);
        }
        // Abort the comms drain but keep the session registered.
        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            adapter.abort_comms_drain(session_id).await;
        }
        // Ensure stop semantics are strong: do not report completion while the
        // session still appears active, otherwise immediate resume can race into
        // SessionError::Busy.
        let mut still_active = false;
        for _ in 0..40 {
            match self.provisioner.is_member_active(member_ref).await? {
                Some(true) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
                _ => {
                    still_active = false;
                    break;
                }
            }
            still_active = true;
        }
        if still_active {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "autonomous member stop polling exhausted before member became idle"
            );
        }
        Ok(())
    }

    async fn stop_all_autonomous_members(&mut self) -> Result<(), MobError> {
        let entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost)
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            return Ok(());
        }
        let mut first_error: Option<MobError> = None;
        for entry in entries {
            let result = self.stop_autonomous_member_entry(entry).await;
            if let Err((agent_identity, error)) = result {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    error = %error,
                    "failed stopping autonomous member"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn stop_autonomous_member_entry(
        &mut self,
        entry: RosterEntry,
    ) -> Result<(), (MeerkatId, MobError)> {
        self.stop_autonomous_member(&entry.agent_identity, &entry.member_ref)
            .await
            .map_err(|error| (entry.agent_identity, error))
    }

    /// Ensure all autonomous roster members have their runtime ready.
    ///
    /// Called on mob startup and resume. Does NOT fire synthetic kickoff turns —
    /// the keep-alive runtime infrastructure is sufficient. On resume, the
    /// member's session already has its conversation history.
    async fn ensure_autonomous_runtimes_from_roster(&self) -> Result<(), MobError> {
        let broken_members = self
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| {
                    entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                        && !broken_members.contains(&entry.agent_identity)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            // Turn-driven resumed members still need their mob-owned comms
            // drain rebound even though they do not need autonomous dispatch.
        }

        let mut first_error: Option<MobError> = None;
        let all_entries = {
            let roster = self.roster.read().await;
            roster
                .list()
                .filter(|entry| {
                    entry.runtime_mode != crate::MobRuntimeMode::AutonomousHost
                        && !broken_members.contains(&entry.agent_identity)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for entry in &all_entries {
            let ensure_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                self.provisioner
                    .ensure_runtime_session_state(&entry.member_ref)
                    .await?;
                self.ensure_mob_comms_drain(&entry.agent_identity, &entry.member_ref)
                    .await
            })
            .await;
            let result = match ensure_result {
                Ok(result) => result,
                Err(_elapsed) => {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        "timed out ensuring mob comms drain ready"
                    );
                    continue;
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    error = %error,
                    "failed ensuring mob comms drain ready"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        for entry in &entries {
            let ensure_result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.ensure_autonomous_runtime_ready(&entry.agent_identity, &entry.member_ref),
            )
            .await;
            let result = match ensure_result {
                Ok(result) => result,
                Err(_elapsed) => {
                    tracing::warn!(
                        agent_identity = %entry.agent_identity,
                        "timed out ensuring autonomous runtime ready"
                    );
                    continue;
                }
            };
            if let Err(error) = result {
                tracing::warn!(
                    agent_identity = %entry.agent_identity,
                    error = %error,
                    "failed ensuring autonomous runtime ready"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    /// Main actor loop: process commands sequentially until Shutdown.
    pub(super) async fn run(mut self, mut command_rx: mpsc::Receiver<MobCommand>) {
        if matches!(self.state(), MobState::Running) {
            if let Err(error) = self.start_mcp_servers().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start mcp servers during actor startup; entering Stopped"
                );
                self.fail_startup_to_stopped("mcp startup failure").await;
            } else if let Err(error) = self.ensure_autonomous_runtimes_from_roster().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start autonomous host loops during actor startup; entering Stopped"
                );
                self.fail_startup_to_stopped("autonomous runtime startup failure")
                    .await;
            }
        }
        let mut deferred_commands = VecDeque::new();
        loop {
            let cmd = if let Some(cmd) = deferred_commands.pop_front() {
                cmd
            } else if let Some(cmd) = command_rx.recv().await {
                cmd
            } else {
                break;
            };
            match cmd {
                MobCommand::Spawn {
                    spec,
                    owner_bridge_session_id,
                    ops_registry,
                    reply_tx,
                } => {
                    Box::pin(self.enqueue_spawn(
                        *spec,
                        owner_bridge_session_id,
                        ops_registry,
                        reply_tx,
                    ))
                    .await;
                }
                MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result,
                } => {
                    let mut completions = vec![(spawn_ticket, result)];
                    loop {
                        match command_rx.try_recv() {
                            Ok(MobCommand::SpawnProvisioned {
                                spawn_ticket,
                                result,
                            }) => completions.push((spawn_ticket, result)),
                            Ok(other) => {
                                deferred_commands.push_back(other);
                                break;
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                        }
                    }
                    // Box::pin: the `handle_spawn_provisioned_batch`
                    // future tips past clippy's `large_futures` 16 KiB
                    // threshold after Track-B R5 added
                    // `peer_projection_*` state to `MeerkatMachine` —
                    // the enum size cascades through the actor's
                    // transitive borrows. Heap-allocating the future
                    // keeps the polling stack frame small without
                    // reshaping the handler.
                    Box::pin(self.handle_spawn_provisioned_batch(completions)).await;
                }
                MobCommand::Retire {
                    agent_identity,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Stopped]) {
                        Ok(()) => {
                            let canceled = self.cancel_pending_spawns_for_member(
                                &agent_identity,
                                "retire command received",
                            );
                            if canceled > 0 {
                                tracing::info!(
                                    agent_identity = %agent_identity,
                                    canceled,
                                    "retire canceled pending spawn lineage before roster retirement"
                                );
                            }
                            self.handle_retire(agent_identity).await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Respawn {
                    agent_identity,
                    initial_message,
                    reply_tx,
                } => {
                    let result =
                        Box::pin(self.handle_respawn(agent_identity, initial_message)).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::RetireAll { reply_tx } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Stopped]) {
                        Ok(()) => self.retire_all_members("retire_all").await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::SubmitWork { payload, reply_tx } => {
                    let result = Box::pin(self.handle_submit_work(payload)).await;
                    let _ = reply_tx.send(result);
                }
                #[cfg(feature = "runtime-adapter")]
                MobCommand::KickoffOutcomeResolved {
                    agent_identity,
                    outcome,
                    ack_tx,
                } => {
                    if let Err(error) = self.resolve_kickoff_outcome(&agent_identity, outcome).await
                    {
                        tracing::warn!(
                            agent_identity = %agent_identity,
                            error = %error,
                            "failed to persist kickoff outcome"
                        );
                    }
                    let _ = ack_tx.send(());
                }
                MobCommand::RunFlow {
                    flow_id,
                    activation_params,
                    scoped_event_tx,
                    reply_tx,
                } => {
                    let result = self
                        .handle_run_flow(flow_id, activation_params, scoped_event_tx)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelFlow { run_id, reply_tx } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_cancel_flow(run_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::FlowStatus { run_id, reply_tx } => {
                    let result = self
                        .run_store
                        .get_run(&run_id)
                        .await
                        .map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowRunCommand {
                    run_id,
                    command,
                    context,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_run_command_in_actor(&run_id, *command, context)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowTerminalization {
                    run_id,
                    flow_id,
                    target,
                    command,
                    context,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_terminalization_in_actor(
                            run_id, flow_id, target, *command, context,
                        )
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CommitFlowFrameStorePlan {
                    run_id,
                    plan,
                    reply_tx,
                } => {
                    let result = self
                        .commit_flow_frame_store_plan_in_actor(&run_id, *plan)
                        .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::ProjectMachineInput { input, reply_tx } => {
                    let result = self
                        .apply_dsl_input(*input, "project_machine_input")
                        .map(|()| self.dsl_authority.state.clone());
                    let _ = reply_tx.send(result);
                }
                MobCommand::PreviewMachineInput { input, reply_tx } => {
                    let result = self.preview_dsl_input(*input, "preview_machine_input");
                    let _ = reply_tx.send(result);
                }
                MobCommand::QueryMachineState { reply_tx } => {
                    let _ = reply_tx.send(self.dsl_authority.state.clone());
                }
                MobCommand::ProjectMachineSignal { signal } => {
                    if let Err(error) = self.apply_dsl_signal(signal, "project_machine_signal") {
                        tracing::error!(
                            error = %error,
                            "typed composition signal projection failed"
                        );
                    }
                }
                MobCommand::FlowFinished { run_id } => {
                    if let Err(error) = self
                        .handle_flow_cleanup(run_id, "flow finished cleanup")
                        .await
                    {
                        tracing::error!(error = %error, "flow finished cleanup failed");
                    }
                }
                MobCommand::FlowCanceledCleanup { run_id } => {
                    if let Err(error) = self
                        .handle_flow_cleanup(run_id, "flow canceled cleanup")
                        .await
                    {
                        tracing::error!(error = %error, "flow canceled cleanup failed");
                    }
                }
                #[cfg(test)]
                MobCommand::FlowTrackerCounts { reply_tx } => {
                    let snapshot = self.flow_tracker_snapshot().await;
                    let tasks = snapshot.run_task_ids.len();
                    let tokens = snapshot.cancel_token_ids.len();
                    let _ = reply_tx.send((tasks, tokens));
                }
                #[cfg(test)]
                MobCommand::OrchestratorSnapshot { reply_tx } => {
                    let phase = self.state();
                    let _ = reply_tx.send(
                        self.machine_orchestrator_snapshot(phase)
                            .unwrap_or_default(),
                    );
                }
                #[cfg(test)]
                MobCommand::LifecycleSnapshot { reply_tx } => {
                    let _ = reply_tx.send(super::state::MobLifecycleSnapshot {
                        phase: self.state(),
                        active_run_count: self.machine_active_run_count(),
                        cleanup_pending: false,
                    });
                }
                #[cfg(test)]
                MobCommand::DslT2Snapshot { reply_tx } => {
                    let dsl = &self.dsl_authority.state;
                    let _ = reply_tx.send(super::state::MobDslT2Snapshot {
                        member_state_markers: dsl.member_state_markers.clone(),
                        wiring_edges: dsl.wiring_edges.clone(),
                        external_peer_edges: dsl.external_peer_edges.clone(),
                        identity_to_runtime: dsl.identity_to_runtime.clone(),
                        tasks: dsl.tasks.clone(),
                        in_progress_task_ids: dsl.in_progress_task_ids.clone(),
                        completed_task_ids: dsl.completed_task_ids.clone(),
                        member_session_bindings: dsl.member_session_bindings.clone(),
                        pending_spawn_sessions: dsl.pending_spawn_sessions.clone(),
                        pending_session_ingress_detach_runtime_ids: dsl
                            .pending_session_ingress_detach_runtime_ids
                            .clone(),
                        topology_epoch: dsl.topology_epoch,
                    });
                }
                MobCommand::StartupKickoffSnapshot { reply_tx } => {
                    let _ = reply_tx.send(super::state::MobStartupKickoffSnapshot {
                        pending_kickoff_member_ids: self.pending_kickoff_member_ids_from_dsl(),
                        ready_runtime_ids: self.ready_runtime_ids_from_dsl(),
                    });
                }
                MobCommand::ProjectMemberList {
                    include_retiring,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(
                        self.project_member_list_from_machine(include_retiring)
                            .await,
                    );
                }
                MobCommand::ProjectMemberStatus {
                    agent_identity,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(
                        self.machine_member_material(&MeerkatId::from(&agent_identity), true)
                            .await
                            .to_snapshot(),
                    );
                }
                MobCommand::MemberMachineProjection {
                    agent_identity,
                    reply_tx,
                } => {
                    let _ = reply_tx.send(self.machine_projection_for_identity(&agent_identity));
                }
                MobCommand::CurrentRealtimeBinding {
                    agent_identity,
                    reply_tx,
                } => {
                    let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
                    let binding = self
                        .dsl_authority
                        .state
                        .member_session_bindings
                        .get(&dsl_identity)
                        .and_then(|dsl_session_id| {
                            meerkat_core::types::SessionId::parse(&dsl_session_id.0).ok()
                        });
                    let _ = reply_tx.send(binding);
                }
                MobCommand::Stop { reply_tx } => {
                    let result = match self
                        .probe_mob_machine_input(mob_dsl::MobMachineInput::Stop, MobState::Stopped)
                    {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is stopping").await;
                            self.notify_orchestrator_lifecycle(format!(
                                "Mob '{}' is stopping.",
                                self.definition.id
                            ))
                            .await;
                            // Cancel checkpointer gates before stopping host loops so
                            // in-flight saves that complete after the loop stops don't
                            // race with subsequent external cleanup (e.g. DML deletes).
                            self.provisioner.cancel_all_checkpointers().await;
                            let mut stop_result: Result<(), MobError> = Ok(());
                            let loop_result = self.stop_all_autonomous_members().await;
                            let mcp_result = self.stop_mcp_servers().await;
                            if let Err(error) = loop_result {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    error = %error,
                                    "stop encountered autonomous loop cleanup error"
                                );
                                if stop_result.is_ok() {
                                    stop_result = Err(error);
                                }
                            }
                            if let Err(error) = mcp_result {
                                tracing::warn!(
                                    mob_id = %self.definition.id,
                                    error = %error,
                                    "stop encountered mcp cleanup error"
                                );
                                if stop_result.is_ok() {
                                    stop_result = Err(error);
                                }
                            }
                            if stop_result.is_ok() {
                                if self.has_orchestrator
                                    && let Err(error) = self.apply_dsl_signal(
                                        mob_dsl::MobMachineSignal::StopOrchestrator,
                                        "stop_orchestrator",
                                    )
                                {
                                    stop_result = Err(MobError::Internal(format!(
                                        "orchestrator StopOrchestrator transition failed during stop: {error}"
                                    )));
                                }
                                if stop_result.is_ok()
                                    && let Err(error) = self.apply_dsl_input(
                                        mob_dsl::MobMachineInput::Stop,
                                        "stop_input",
                                    )
                                {
                                    stop_result = Err(MobError::Internal(format!(
                                        "lifecycle Stop transition failed during stop: {error}"
                                    )));
                                }
                            }
                            if stop_result.is_err() {
                                // Restore checkpointer state — mob stays Running.
                                self.provisioner.rearm_all_checkpointers().await;
                            }
                            stop_result
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ResumeLifecycle { reply_tx } => {
                    let result = match self.probe_mob_machine_input(
                        mob_dsl::MobMachineInput::Resume,
                        MobState::Running,
                    ) {
                        Ok(()) => {
                            // Re-enable checkpointers cancelled during stop.
                            self.provisioner.rearm_all_checkpointers().await;
                            if let Err(error) = self.start_mcp_servers().await {
                                if let Err(stop_error) = self.stop_mcp_servers().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping mcp servers"
                                    );
                                }
                                Err(error)
                            } else if let Err(error) =
                                self.ensure_autonomous_runtimes_from_roster().await
                            {
                                if let Err(stop_error) = self.stop_all_autonomous_members().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping autonomous loops"
                                    );
                                }
                                if let Err(stop_error) = self.stop_mcp_servers().await {
                                    tracing::warn!(
                                        mob_id = %self.definition.id,
                                        error = %stop_error,
                                        "resume cleanup failed while stopping mcp servers"
                                    );
                                }
                                Err(error)
                            } else {
                                let mut resume_result: Result<(), MobError> = Ok(());
                                if self.has_orchestrator
                                    && let Err(error) = self.apply_dsl_signal(
                                        mob_dsl::MobMachineSignal::ResumeOrchestrator,
                                        "resume_orchestrator",
                                    )
                                {
                                    resume_result = Err(MobError::Internal(format!(
                                        "orchestrator ResumeOrchestrator transition failed during resume: {error}"
                                    )));
                                }
                                if resume_result.is_ok()
                                    && let Err(error) = self.apply_dsl_input(
                                        mob_dsl::MobMachineInput::Resume,
                                        "resume_input",
                                    )
                                {
                                    resume_result = Err(MobError::Internal(format!(
                                        "lifecycle Resume transition failed during resume: {error}"
                                    )));
                                }
                                if let Err(error) = resume_result {
                                    if let Err(stop_error) =
                                        self.stop_all_autonomous_members().await
                                    {
                                        tracing::warn!(
                                            mob_id = %self.definition.id,
                                            error = %stop_error,
                                            "resume transition rollback failed while stopping autonomous loops"
                                        );
                                    }
                                    if let Err(stop_error) = self.stop_mcp_servers().await {
                                        tracing::warn!(
                                            mob_id = %self.definition.id,
                                            error = %stop_error,
                                            "resume transition rollback failed while stopping mcp servers"
                                        );
                                    }
                                    self.provisioner.cancel_all_checkpointers().await;
                                    Err(error)
                                } else {
                                    Ok(())
                                }
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Complete { reply_tx } => {
                    let result = match self.probe_mob_machine_input(
                        mob_dsl::MobMachineInput::Complete,
                        MobState::Completed,
                    ) {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is completing").await;
                            self.handle_complete().await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    let result = self.handle_destroy().await;
                    let destroy_succeeded = result.is_ok();
                    let _ = reply_tx.send(result);
                    if destroy_succeeded {
                        // Destroy is terminal for the current ownership model:
                        // once a mob is destroyed, the actor task exits and no
                        // further commands are accepted.
                        self.lifecycle_tasks.abort_all();
                        while self.lifecycle_tasks.join_next().await.is_some() {}
                        break;
                    }
                }
                MobCommand::Reset { reply_tx } => {
                    if let Err(error) = self
                        .probe_mob_machine_input(mob_dsl::MobMachineInput::Reset, MobState::Running)
                    {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    self.fail_all_pending_spawns("mob is resetting").await;
                    let result = self.handle_reset().await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskCreate {
                    subject,
                    description,
                    blocked_by,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => {
                            self.handle_task_create(subject, description, blocked_by)
                                .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskUpdate {
                    task_id,
                    status,
                    owner,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_task_update(task_id, status, owner).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskList { reply_tx } => {
                    let tasks = self.task_board.read().await.list().cloned().collect();
                    let _ = reply_tx.send(tasks);
                }
                MobCommand::TaskGet { task_id, reply_tx } => {
                    let task = self.task_board.read().await.get(&task_id).cloned();
                    let _ = reply_tx.send(task);
                }
                MobCommand::McpServerStates { reply_tx } => {
                    let states = self
                        .mcp_servers
                        .lock()
                        .await
                        .iter()
                        .map(|(name, entry)| (name.clone(), entry.running))
                        .collect();
                    let _ = reply_tx.send(states);
                }
                MobCommand::SubscribeAgentEvents {
                    agent_identity,
                    reply_tx,
                } => {
                    let result = async {
                        let entry = self
                            .roster
                            .read()
                            .await
                            .entry(&agent_identity)
                            .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
                        let session_id = match entry.member_ref.bridge_session_id().cloned() {
                            Some(session_id) => session_id,
                            None => {
                                return Err(MobError::UnsupportedForMode {
                                    mode: entry.runtime_mode,
                                    reason: "agent event subscriptions are not supported for peer-only members in phase 1".to_string(),
                                });
                            }
                        };
                        crate::runtime::session_service::MobSessionService::subscribe_session_events(
                            self.session_service.as_ref(),
                            &session_id,
                        )
                        .await
                        .map_err(|e| {
                            MobError::Internal(format!(
                                "failed to subscribe to agent events for '{agent_identity}': {e}"
                            ))
                        })
                    }
                    .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::SubscribeAllAgentEvents { reply_tx } => {
                    let result = async {
                        let entries = self.roster.read().await.list().cloned().collect::<Vec<_>>();
                        let mut streams = Vec::with_capacity(entries.len());
                        let mut unsupported_mode: Option<crate::MobRuntimeMode> = None;
                        for entry in entries {
                            let Some(session_id) = entry.member_ref.bridge_session_id().cloned()
                            else {
                                // Peer-only members cannot supply an agent event stream;
                                // skip silently and record the mode so callers who ended
                                // up with zero streams learn why.
                                unsupported_mode.get_or_insert(entry.runtime_mode);
                                continue;
                            };
                            let stream = crate::runtime::session_service::MobSessionService::subscribe_session_events(
                                self.session_service.as_ref(),
                                &session_id,
                            )
                            .await
                            .map_err(|e| {
                                MobError::Internal(format!(
                                    "failed to subscribe to agent events for '{}': {e}",
                                    entry.agent_identity
                                ))
                            })?;
                            streams.push((entry.agent_identity.clone(), stream));
                        }
                        if streams.is_empty()
                            && let Some(mode) = unsupported_mode
                        {
                            return Err(MobError::UnsupportedForMode {
                                mode,
                                reason: "agent event subscriptions are not supported for peer-only members in phase 1".to_string(),
                            });
                        }
                        Ok(streams)
                    }
                    .await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::RotateSupervisor { reply_tx } => {
                    let result = self.handle_rotate_supervisor().await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::PollEvents {
                    after_cursor,
                    limit,
                    reply_tx,
                } => {
                    let result = self
                        .events
                        .poll(after_cursor, limit)
                        .await
                        .map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::ReplayAllEvents { reply_tx } => {
                    let result = self.events.replay_all().await.map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::RecordOperatorActionProvenance {
                    tool_name,
                    authority_context,
                    reply_tx,
                } => {
                    let result = self
                        .events
                        .append(NewMobEvent {
                            mob_id: self.definition.id.clone(),
                            timestamp: None,
                            kind: MobEventKind::OperatorActionRecorded {
                                tool_name,
                                principal_token: authority_context.principal_token().clone(),
                                caller_provenance: authority_context.caller_provenance().cloned(),
                                audit_invocation_id: authority_context
                                    .audit_invocation_id()
                                    .map(ToOwned::to_owned),
                            },
                        })
                        .await
                        .map(|_| ())
                        .map_err(MobError::from);
                    let _ = reply_tx.send(result);
                }
                MobCommand::ForceCancel {
                    agent_identity,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_force_cancel(agent_identity).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = self.handle_wire(local, target).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = self.handle_unwire(local, target).await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::CancelAllWork {
                    runtime_id,
                    fence_token,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_cancel_all_work(runtime_id, fence_token).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                    let result = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::SetSpawnPolicy,
                        "set_spawn_policy",
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "SetSpawnPolicy rejected by MobMachine guards before shell policy write: {error}"
                        ))
                    });
                    if result.is_ok() {
                        self.spawn_policy.set(policy).await;
                    }
                    let _ = reply_tx.send(result);
                }
                MobCommand::QueryPhase { reply_tx } => {
                    let _ = reply_tx.send(self.state());
                }
                MobCommand::Shutdown { reply_tx } => {
                    if let Err(error) = self.probe_mob_machine_input(
                        mob_dsl::MobMachineInput::Shutdown,
                        MobState::Stopped,
                    ) {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    self.fail_all_pending_spawns("mob runtime is shutting down")
                        .await;
                    let mut result: Result<(), MobError> = Ok(());
                    if let Err(error) = self.cancel_all_flow_tasks().await {
                        tracing::warn!(error = %error, "shutdown flow cancellation encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    if let Err(error) = self.stop_all_autonomous_members().await {
                        tracing::warn!(error = %error, "shutdown loop stop encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    self.teardown_session_runtime_bindings_from_roster().await;
                    if let Err(error) = self.stop_mcp_servers().await {
                        tracing::warn!(error = %error, "shutdown mcp stop encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    // Cancel remaining lifecycle notification tasks.
                    // abort_all is non-blocking; join_next drains the abort results.
                    self.lifecycle_tasks.abort_all();
                    while self.lifecycle_tasks.join_next().await.is_some() {}
                    // Wave-c WAR-2: submit `Stop` unconditionally and let the
                    // authority be the single decider of whether the
                    // transition is valid for the current phase. A pre-state
                    // check here would be a shell-side second source of truth
                    // about when `Stop` is accepted, which the `NoGuardedApply`
                    // rule forbids. During `Shutdown`, an authority rejection
                    // of `Stop` is the benign "already past Running" shape and
                    // is logged at debug level without contaminating the
                    // shutdown result.
                    if let Err(error) =
                        self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "shutdown_stop")
                    {
                        tracing::debug!(
                            error = %error,
                            "authority rejected Stop during shutdown (expected when mob is \
                             already past Running); continuing shutdown",
                        );
                    }
                    let _ = reply_tx.send(result);
                    break;
                }
            }

            // Wave-c C-6p — drain routed-effect queue at every loop
            // boundary. `apply_dsl_input` / `apply_dsl_signal` populated
            // the queue synchronously inside the command handler; this
            // is the async site where `CompositionDispatcher::dispatch`
            // runs. A typed dispatch failure — most notably
            // `DispatchRefusal::UnwiredConsumer` while C-6c (consumer
            // surface on `MeerkatMachine`) is outstanding — is logged
            // loud and the actor task terminates rather than silently
            // dropping the effect. Once the spine lands C-6c, the
            // dispatcher resolves cleanly and this path becomes
            // everyday.
            if let Err(error) = self.flush_routed_effects().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    queued_remaining = self.pending_routed_effects.len(),
                    "composition dispatch failed; terminating mob actor task"
                );
                return;
            }
        }
    }

    async fn fail_all_pending_spawns(&mut self, reason: &str) {
        if self.pending_spawns.is_empty() {
            if let Some(message) = self.pending_spawn_alignment_violation() {
                tracing::error!(
                    reason,
                    message = %message,
                    "pending spawn alignment violated with no local pending slots to drain"
                );
            }
            return;
        }

        for slot in self.pending_spawns.drain_all() {
            let spawn_ticket = slot.ticket;
            let agent_identity = slot.spawn.agent_identity.clone();
            self.complete_orchestrator_spawn(
                Some(spawn_ticket),
                &agent_identity,
                "lifecycle transition cleared pending spawn",
            );
            self.abort_pending_spawn_slot(&slot, reason).await;
            slot.fail(&format!("spawn canceled for '{agent_identity}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                agent_identity = %agent_identity,
                "failed pending spawn due to lifecycle transition"
            );
        }
        debug_assert!(
            self.pending_spawns.is_empty(),
            "all pending spawn slots should be drained during lifecycle transition"
        );
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                message = %message,
                "pending spawn alignment still violated after lifecycle drain"
            );
        }
    }

    async fn abort_pending_spawn_slot(
        &self,
        slot: &super::pending_spawn_lineage::PendingSpawnSlot,
        reason: &str,
    ) {
        let snapshot = {
            let progress = slot
                .spawn
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            progress
                .bridge_session_id
                .clone()
                .zip(progress.operation_id.clone())
        };
        if let Some((session_id, operation_id)) = snapshot
            && let Err(error) = self
                .provisioner
                .abort_member_provision(
                    &MemberRef::from_bridge_session_id(session_id),
                    &operation_id,
                    reason,
                )
                .await
        {
            tracing::warn!(
                spawn_ticket = slot.ticket,
                agent_identity = %slot.spawn.agent_identity,
                operation_id = %operation_id,
                error = %error,
                "failed to abort pending member provision during lifecycle drain"
            );
        }
    }

    fn cancel_pending_spawns_for_member(
        &mut self,
        agent_identity: &MeerkatId,
        reason: &str,
    ) -> usize {
        let slots = self.pending_spawns.take_for_member(agent_identity);
        if slots.is_empty() {
            if let Some(message) = self.pending_spawn_alignment_violation() {
                tracing::error!(
                    agent_identity = %agent_identity,
                    reason,
                    message = %message,
                    "pending spawn alignment violated while canceling member-specific pending spawns"
                );
            }
            return 0;
        }
        let canceled = slots.len();

        for slot in &slots {
            self.complete_orchestrator_spawn(
                Some(slot.ticket),
                &slot.spawn.agent_identity,
                "member lifecycle command canceled pending spawn",
            );
        }

        let pending_abortions = slots
            .iter()
            .map(|slot| {
                (
                    slot.ticket,
                    slot.spawn.agent_identity.clone(),
                    slot.spawn.progress.clone(),
                )
            })
            .collect::<Vec<_>>();
        let provisioner = self.provisioner.clone();
        let reason_owned = reason.to_string();
        tokio::spawn(async move {
            for (spawn_ticket, agent_identity, progress) in pending_abortions {
                let snapshot = {
                    let progress = progress
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    progress
                        .bridge_session_id
                        .clone()
                        .zip(progress.operation_id.clone())
                };
                if let Some((session_id, operation_id)) = snapshot
                    && let Err(error) = provisioner
                        .abort_member_provision(
                            &MemberRef::from_bridge_session_id(session_id),
                            &operation_id,
                            &reason_owned,
                        )
                        .await
                {
                    tracing::warn!(
                        spawn_ticket,
                        agent_identity = %agent_identity,
                        operation_id = %operation_id,
                        error = %error,
                        "failed to abort pending member provision during member-specific cancellation"
                    );
                }
            }
        });

        for slot in slots {
            let spawn_ticket = slot.ticket;
            slot.fail(&format!("spawn canceled for '{agent_identity}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                agent_identity = %agent_identity,
                "canceled pending spawn for member lifecycle command"
            );
        }

        self.debug_assert_pending_spawn_alignment();
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                agent_identity = %agent_identity,
                message = %message,
                "pending spawn alignment violated after member-specific cancellation"
            );
        }
        canceled
    }

    /// P1-T04: spawn() creates a real session.
    ///
    /// Provisioning runs in parallel tasks; final actor commit stays serialized.
    async fn enqueue_spawn(
        &mut self,
        spec: super::handle::SpawnMemberSpec,
        owner_bridge_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
        reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
    ) {
        let super::handle::SpawnMemberSpec {
            role_name: profile_name,
            identity,
            initial_message,
            runtime_mode,
            backend,
            binding,
            context,
            labels,
            launch_mode,
            tool_access_policy: _tool_access_policy,
            budget_split_policy: _budget_split_policy,
            auto_wire_parent,
            additional_instructions,
            shell_env,
            inherited_tool_filter,
            override_profile,
            connection_ref,
        } = spec;
        let agent_identity = MeerkatId::from(identity.as_str());
        // Normalize launch-mode resume/fork details for the provisioning path.
        let resume_bridge_session_id = launch_mode.resume_bridge_session_id().cloned();
        let fork_spec = match launch_mode {
            crate::launch::MemberLaunchMode::Fork {
                source_member_id,
                fork_context,
            } => Some((source_member_id, fork_context)),
            _ => None,
        };
        let prepare_result = async {
            if agent_identity
                .as_str()
                .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{agent_identity}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
                )));
            }
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                profile = %profile_name,
                "MobActor::enqueue_spawn start"
            );

            if self.pending_spawns.contains_member(&agent_identity) {
                return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
            }

            {
                let roster = self.roster.read().await;
                if roster.get(&agent_identity).is_some() {
                    return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
                }
                if roster
                    .list()
                    .any(|entry| entry.external_peer_specs.contains_key(&agent_identity))
                {
                    return Err(MobError::WiringError(format!(
                        "meerkat id '{agent_identity}' collides with an existing external peer name"
                    )));
                }
            }

            // Always validate role_name exists in definition for roster consistency,
            // even when an override profile is provided.
            if !self.definition.profiles.contains_key(&profile_name) {
                return Err(MobError::ProfileNotFound(profile_name.clone()));
            }

            // Capture the override for roster persistence before consuming it.
            let effective_profile_override = override_profile.clone();

            // Use override_profile if provided (from SpawnTooling::Profile resolution),
            // otherwise resolve from the mob definition.
            let profile = if let Some(p) = override_profile {
                p
            } else {
                self.definition
                    .resolve_profile(&profile_name, self.realm_profile_store.as_ref())
                    .await?
            };

            let selected_runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);
            let profile_external_addressable = profile.external_addressable;

            // ---------- Resume bridge-session fast-path ----------
            // When resume_bridge_session_id is set, skip provisioning and go
            // straight to finalization. The bridge session must already exist
            // and be usable.
            if let Some(resume_id) = resume_bridge_session_id {
                let member_ref = MemberRef::from_bridge_session_id(resume_id.clone());

                // Validate the session exists and is active.
                let is_active = self
                    .provisioner
                    .is_member_active(&member_ref)
                    .await
                    .map_err(|e| {
                        MobError::Internal(format!(
                            "resume bridge session check failed for '{agent_identity}': {e}"
                        ))
                    })?;
                if is_active.unwrap_or(false) {
                    // Validate event injector for autonomous mode.
                    if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                        && self.provisioner.interaction_event_injector(&resume_id).await.is_none()
                    {
                        return Err(MobError::Internal(format!(
                            "resumed session '{resume_id}' has no event injector for autonomous '{agent_identity}'"
                        )));
                    }

                    // Validate comms if wiring rules exist.
                    let has_wiring = self.definition.wiring.auto_wire_orchestrator
                        || !self.definition.wiring.role_wiring.is_empty();
                    if has_wiring
                        && self
                            .provisioner
                            .comms_runtime(&member_ref)
                            .await
                            .is_none()
                    {
                        return Err(MobError::Internal(format!(
                            "resumed session '{resume_id}' has no comms runtime for '{agent_identity}'"
                        )));
                    }

                    let prompt = initial_message.clone().unwrap_or_else(|| {
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
                    });
                    let resolved_labels = labels.unwrap_or_default();

                    return Ok((
                        profile_name,
                        agent_identity,
                        prompt,
                        selected_runtime_mode,
                        profile_external_addressable,
                        resolved_labels,
                        Some(member_ref),
                        None,
                        owner_bridge_session_id.clone(),
                        auto_wire_parent,
                        effective_profile_override.clone(),
                    ));
                }

                if self.session_service.supports_persistent_sessions() {
                    let stored_session = self
                        .session_service
                        .load_persisted_session(&resume_id)
                        .await
                        .map_err(MobError::from)?
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "missing durable session snapshot for '{resume_id}'"
                            ))
                        })?;

                    let external_tools = self.external_tools_for_profile(&profile)?;
                    let mut config = build::build_resumed_agent_config(
                        build::BuildResumedAgentConfigParams {
                            base: build::BuildAgentConfigParams {
                                mob_id: &self.definition.id,
                                profile_name: &profile_name,
                                agent_identity: &agent_identity,
                                profile: &profile,
                                definition: &self.definition,
                                external_tools,
                                context,
                                labels: labels.clone(),
                                additional_instructions,
                                shell_env,
                                mob_tool_access_context: crate::build::MobToolAccessContext::None,
                                inherited_tool_filter: inherited_tool_filter.clone(),
                            },
                            expected_session_id: &resume_id,
                            resumed_session: stored_session,
                        },
                    )
                    .await?;
                    config.keep_alive =
                        selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost;
                    if let Some(ref client) = self.default_llm_client {
                        config.llm_client_override = Some(client.clone());
                    }

                    let prompt = initial_message.clone().unwrap_or_else(|| {
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
                    });
                    let req = build::to_create_session_request(&config, prompt.clone());
                    let selected_binding = resolve_binding(
                        binding.clone(),
                        backend,
                        profile.backend,
                        self.definition.backend.default,
                        &agent_identity,
                    )?;
                    let selected_runtime_mode =
                        normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
                    let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
                    let provision_request = ProvisionMemberRequest {
                        create_session: req,
                        binding: selected_binding,
                        peer_name,
                        owner_bridge_session_id: owner_bridge_session_id.clone(),
                        ops_registry: ops_registry.clone(),
                    };
                    let resolved_labels = labels.unwrap_or_default();
                    return Ok((
                        profile_name,
                        agent_identity,
                        prompt,
                        selected_runtime_mode,
                        profile_external_addressable,
                        resolved_labels,
                        None::<MemberRef>,
                        Some(provision_request),
                        owner_bridge_session_id.clone(),
                        auto_wire_parent,
                        effective_profile_override.clone(),
                    ));
                }

                return Err(MobError::Internal(format!(
                    "resumed session '{resume_id}' not found or inactive for '{agent_identity}'"
                )));
            }

            // ---------- Fork path ----------
            // When fork_spec is set, read source member's session and render
            // conversation history as context in the initial prompt.
            let fork_context_text = if let Some((source_member_id, fork_context)) = fork_spec {
                let source_session_id = {
                    let roster = self.roster.read().await;
                    let source_entry = roster.get(&source_member_id).ok_or_else(|| {
                        MobError::MemberNotFound(source_member_id.clone())
                    })?;
                    source_entry
                        .member_ref
                        .bridge_session_id()
                        .cloned()
                        .ok_or_else(|| {
                            MobError::Internal(format!(
                                "fork source '{source_member_id}' has no session"
                            ))
                        })?
                };

                // Read full history for fork context rendering
                let query = match fork_context {
                    crate::launch::ForkContext::FullHistory => {
                        meerkat_core::service::SessionHistoryQuery::default()
                    }
                    crate::launch::ForkContext::LastMessages { count } => {
                        // We need last N messages; read_history uses offset/limit.
                        // First read the session to get message count.
                        let view = self
                            .session_service
                            .read(&source_session_id)
                            .await
                            .map_err(|e| {
                                MobError::Internal(format!(
                                    "failed to read source session metadata for fork from '{source_member_id}': {e}"
                                ))
                            })?;
                        let total = view.state.message_count;
                        let offset = total.saturating_sub(count as usize);
                        meerkat_core::service::SessionHistoryQuery {
                            offset,
                            limit: Some(count as usize),
                        }
                    }
                };

                let history = meerkat_core::service::SessionServiceHistoryExt::read_history(
                    self.session_service.as_ref(),
                    &source_session_id,
                    query,
                )
                .await
                .map_err(|e| {
                    MobError::Internal(format!(
                        "failed to read source session history for fork from '{source_member_id}': {e}"
                    ))
                })?;

                Some(render_fork_context(&source_member_id, &history.messages))
            } else {
                None
            };

            let external_tools = self.external_tools_for_profile(&profile)?;
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &self.definition.id,
                profile_name: &profile_name,
                agent_identity: &agent_identity,
                profile: &profile,
                definition: &self.definition,
                external_tools,
                context,
                labels: labels.clone(),
                additional_instructions,
                shell_env,
                mob_tool_access_context: crate::build::MobToolAccessContext::None,
                inherited_tool_filter,
            })
            .await?;
            config.keep_alive =
                selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost;
            if let Some(ref client) = self.default_llm_client {
                config.llm_client_override = Some(client.clone());
            }
            // Deferral §1: per-member auth binding.
            if let Some(ref cref) = connection_ref {
                config.connection_ref = Some(cref.clone());
            }

            let base_prompt = initial_message.clone().unwrap_or_else(|| {
                ContentInput::from(self.fallback_spawn_prompt(&profile_name, &agent_identity))
            });
            let prompt = if let Some(fork_text) = fork_context_text {
                let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                    text: format!("{fork_text}\n\n"),
                }];
                blocks.extend(base_prompt.into_blocks());
                ContentInput::Blocks(blocks)
            } else {
                base_prompt
            };
            let req = build::to_create_session_request(&config, prompt.clone());
            let selected_binding = resolve_binding(
                binding,
                backend,
                profile.backend,
                self.definition.backend.default,
                &agent_identity,
            )?;
            let selected_runtime_mode =
                normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
            let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
            let provision_request = ProvisionMemberRequest {
                create_session: req,
                binding: selected_binding,
                peer_name,
                owner_bridge_session_id: owner_bridge_session_id.clone(),
                ops_registry: ops_registry.clone(),
            };
            let resolved_labels = labels.unwrap_or_default();
            Ok((
                profile_name,
                agent_identity,
                prompt,
                selected_runtime_mode,
                profile_external_addressable,
                resolved_labels,
                None::<MemberRef>,
                Some(provision_request),
                owner_bridge_session_id.clone(),
                auto_wire_parent,
                effective_profile_override,
            ))
        }
        .await;

        let (
            profile_name,
            agent_identity,
            prompt,
            selected_runtime_mode,
            external_addressable,
            resolved_labels,
            resume_member_ref,
            maybe_provision_request,
            spawn_owner_bridge_session_id,
            auto_wire_parent,
            effective_profile_override,
        ) = match prepare_result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = reply_tx.send(Err(error));
                return;
            }
        };

        // ---------- Resume fast-path: skip async provisioning ----------
        if let Some(member_ref) = resume_member_ref {
            let Some(bridge_session_id) = member_ref.bridge_session_id().cloned() else {
                let _ = reply_tx.send(Err(MobError::Internal(format!(
                    "resumed member '{agent_identity}' has no bridge session id"
                ))));
                return;
            };
            if let Err(error) = self.preview_spawn_admission(
                &agent_identity,
                external_addressable,
                &bridge_session_id,
            ) {
                let _ = reply_tx.send(Err(error));
                return;
            }
            if let (Some(owner_bridge_session_id), Some(ops_registry)) =
                (owner_bridge_session_id.clone(), ops_registry.clone())
                && let Err(error) = self
                    .provisioner
                    .bind_member_owner_context(&member_ref, owner_bridge_session_id, ops_registry)
                    .await
            {
                let _ = reply_tx.send(Err(error));
                return;
            }
            let operation_id = self
                .provisioner
                .active_operation_id_for_member(&member_ref)
                .await
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "resumed member '{agent_identity}' has no tracked mob child operation"
                    ))
                });
            let operation_id = match operation_id {
                Ok(operation_id) => operation_id,
                Err(error) => {
                    let _ = reply_tx.send(Err(error));
                    return;
                }
            };
            let provision =
                PendingProvision::new(member_ref, agent_identity.clone(), self.provisioner.clone());
            // Go straight to finalization — no async provisioning task needed.
            let fence = self.issue_fence_token();
            let result = self
                .finalize_spawn_from_pending(
                    &profile_name,
                    &agent_identity,
                    crate::ids::Generation::INITIAL,
                    fence,
                    selected_runtime_mode,
                    prompt,
                    resolved_labels,
                    provision,
                    operation_id,
                    spawn_owner_bridge_session_id,
                    auto_wire_parent,
                    None,
                    effective_profile_override,
                )
                .await
                .map(|outcome| outcome.receipt);
            let _ = reply_tx.send(result);
            return;
        }

        // Normal provisioning path — resume path already returned above.
        let Some(mut provision_request) = maybe_provision_request else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "provision_request missing for normal spawn path".into(),
            )));
            return;
        };
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);

        if let Err(error) = self.preview_spawn_admission(
            &agent_identity,
            external_addressable,
            &admitted_bridge_session_id,
        ) {
            let _ = reply_tx.send(Err(error));
            return;
        }

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let spawn_meerkat_id = agent_identity.clone();
        let spawn_meerkat_id_for_log = spawn_meerkat_id.clone();
        let spawn_runtime_mode = selected_runtime_mode;
        let pending_progress = Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default()));

        if let Err(error) =
            self.stage_orchestrator_spawn(&agent_identity, &admitted_bridge_session_id)
        {
            let _ = reply_tx.send(Err(error));
            return;
        }

        let pending = PendingSpawn {
            profile_name,
            agent_identity,
            admitted_bridge_session_id,
            prompt,
            runtime_mode: selected_runtime_mode,
            labels: resolved_labels,
            owner_bridge_session_id: spawn_owner_bridge_session_id,
            auto_wire_parent,
            restore_wiring: None,
            effective_profile_override,
            progress: pending_progress.clone(),
            reply_tx,
        };
        // Treat pending spawn lifecycle as a single keyed table: pending intent
        // and async task handle must be inserted/removed together.
        let provisioner = self.provisioner.clone();
        let command_tx = self.command_tx.clone();
        let task = tokio::spawn(async move {
            let panic_meerkat_id = spawn_meerkat_id.clone();
            let provision_result = std::panic::AssertUnwindSafe(async {
                let spawn_receipt = provisioner.provision_member(provision_request).await?;
                if let Some(bridge_session_id) =
                    spawn_receipt.member_ref.bridge_session_id().cloned()
                {
                    let mut progress = pending_progress
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    progress.bridge_session_id = Some(bridge_session_id);
                    progress.operation_id = Some(spawn_receipt.operation_id.clone());
                }
                if spawn_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                    && let Err(capability_error) =
                        Self::ensure_autonomous_dispatch_capability_for_provisioner(
                            &provisioner,
                            &spawn_meerkat_id,
                            &spawn_receipt.member_ref,
                        )
                        .await
                {
                    if let Err(retire_error) =
                        provisioner.retire_member(&spawn_receipt.member_ref).await
                    {
                        return Err(MobError::Internal(format!(
                            "autonomous capability check failed for '{spawn_meerkat_id}': {capability_error}; cleanup retire failed for member '{:?}': {retire_error}",
                            spawn_receipt.member_ref
                        )));
                    }
                    return Err(capability_error);
                }
                Ok(spawn_receipt)
            })
            .catch_unwind()
            .await;
            let provision_result = match provision_result {
                Ok(result) => result,
                Err(_) => Err(MobError::Internal(format!(
                    "spawn provisioning task panicked for '{panic_meerkat_id}'"
                ))),
            };

            if let Err(send_error) = command_tx
                .send(MobCommand::SpawnProvisioned {
                    spawn_ticket,
                    result: provision_result,
                })
                .await
                && let MobCommand::SpawnProvisioned {
                    result: Ok(spawn_receipt),
                    ..
                } = send_error.0
                && let Err(cleanup_error) =
                    provisioner.retire_member(&spawn_receipt.member_ref).await
            {
                tracing::warn!(
                    spawn_ticket,
                    member_ref = ?spawn_receipt.member_ref,
                    error = %cleanup_error,
                    "spawn completion dropped; failed cleanup retire for provisioned member"
                );
            }
        });
        self.insert_pending_spawn(spawn_ticket, pending, task);
        if let Err(error) = self.ensure_pending_spawn_alignment("enqueue_spawn post-insert") {
            tracing::error!(
                spawn_ticket,
                error = %error,
                "pending spawn alignment check failed after enqueue; canceling all pending spawns"
            );
            self.fail_all_pending_spawns("pending spawn alignment violated after enqueue")
                .await;
            return;
        }

        tracing::debug!(
            spawn_ticket,
            agent_identity = %spawn_meerkat_id_for_log,
            runtime_mode = ?spawn_runtime_mode,
            "MobActor::enqueue_spawn queued provisioning task"
        );
    }

    async fn handle_spawn_provisioned_batch(
        &mut self,
        completions: Vec<(u64, Result<super::handle::MemberSpawnReceipt, MobError>)>,
    ) {
        if let Err(error) = self.ensure_pending_spawn_alignment("spawn batch preflight") {
            tracing::error!(
                error = %error,
                "pending spawn alignment check failed before spawn completion batch"
            );
            self.fail_all_pending_spawns("pending spawn alignment violated before spawn batch")
                .await;
            return;
        }

        let mut pending_items = Vec::with_capacity(completions.len());
        for (spawn_ticket, result) in completions {
            let (pending, task_handle) =
                self.complete_pending_spawn_slot(spawn_ticket, "spawn provisioned batch");
            let Some(pending) = pending else {
                tracing::warn!(spawn_ticket, "received spawn completion for unknown ticket");
                if let Some(handle) = task_handle {
                    handle.abort();
                    tracing::warn!(
                        spawn_ticket,
                        "received spawn completion for unknown pending metadata but found task handle"
                    );
                }
                if let Ok(spawn_receipt) = result {
                    let orphan = PendingProvision::new(
                        spawn_receipt.member_ref,
                        MeerkatId::from("__unknown_ticket__"),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = orphan.rollback().await {
                        tracing::warn!(
                            spawn_ticket,
                            error = %error,
                            "unknown spawn completion cleanup failed"
                        );
                    }
                }
                continue;
            };
            pending_items.push((pending, result));
        }

        for (pending, result) in pending_items {
            let PendingSpawn {
                profile_name,
                agent_identity,
                admitted_bridge_session_id: _,
                prompt,
                runtime_mode,
                labels,
                owner_bridge_session_id,
                auto_wire_parent,
                restore_wiring,
                effective_profile_override,
                progress: _,
                reply_tx,
            } = pending;
            let reply = match result {
                Ok(spawn_receipt) => {
                    let provision = PendingProvision::new(
                        spawn_receipt.member_ref.clone(),
                        agent_identity.clone(),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = self.require_state(&[MobState::Running]) {
                        if let Err(retire_error) = provision.rollback().await {
                            Err(MobError::Internal(format!(
                                "spawn completed while mob state changed for '{agent_identity}': {error}; cleanup retire failed: {retire_error}"
                            )))
                        } else {
                            Err(error)
                        }
                    } else {
                        let fence = self.issue_fence_token();
                        self.finalize_spawn_from_pending(
                            &profile_name,
                            &agent_identity,
                            crate::ids::Generation::INITIAL,
                            fence,
                            runtime_mode,
                            prompt,
                            labels,
                            provision,
                            spawn_receipt.operation_id,
                            owner_bridge_session_id,
                            auto_wire_parent,
                            restore_wiring,
                            effective_profile_override,
                        )
                        .await
                        .map(|outcome| outcome.receipt)
                    }
                }
                Err(error) => Err(error),
            };
            let _ = reply_tx.send(reply);
        }

        if let Err(error) = self.ensure_pending_spawn_alignment("spawn batch completion") {
            tracing::error!(
                error = %error,
                "pending spawn alignment check failed after spawn completion batch"
            );
            self.fail_all_pending_spawns("pending spawn alignment violated after spawn batch")
                .await;
        }
    }

    async fn spawn_from_policy_inline(
        &mut self,
        agent_identity: &MeerkatId,
        spawn_spec: super::spawn_policy::SpawnSpec,
    ) -> Result<super::handle::MemberSpawnReceipt, MobError> {
        self.ensure_pending_spawn_alignment("spawn_from_policy_inline preflight")?;

        if agent_identity
            .as_str()
            .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
        {
            return Err(MobError::WiringError(format!(
                "meerkat id '{agent_identity}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
            )));
        }
        if self.pending_spawns.contains_member(agent_identity) {
            return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
        }
        {
            let roster = self.roster.read().await;
            if roster.get(agent_identity).is_some() {
                return Err(MobError::MemberAlreadyExists(agent_identity.clone()));
            }
            if roster
                .list()
                .any(|entry| entry.external_peer_specs.contains_key(agent_identity))
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{agent_identity}' collides with an existing external peer name"
                )));
            }
        }

        let profile_name = spawn_spec.profile;
        let profile = self
            .definition
            .resolve_profile(&profile_name, self.realm_profile_store.as_ref())
            .await?;
        let runtime_mode = spawn_spec.runtime_mode.unwrap_or(profile.runtime_mode);
        let external_tools = self.external_tools_for_profile(&profile)?;
        let labels = std::collections::BTreeMap::new();
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &profile_name,
            agent_identity,
            profile: &profile,
            definition: &self.definition,
            external_tools,
            context: None,
            labels: Some(labels.clone()),
            additional_instructions: None,
            shell_env: None,
            mob_tool_access_context: crate::build::MobToolAccessContext::None,
            inherited_tool_filter: None,
        })
        .await?;
        config.keep_alive = runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }

        let prompt = ContentInput::from(self.fallback_spawn_prompt(&profile_name, agent_identity));
        let req = build::to_create_session_request(&config, prompt.clone());
        let selected_binding = resolve_binding(
            None,
            None,
            profile.backend,
            self.definition.backend.default,
            agent_identity,
        )?;
        let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, agent_identity);
        let mut provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: selected_binding,
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
        };
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        self.stage_orchestrator_spawn(agent_identity, &admitted_bridge_session_id)?;
        let (pending_reply_tx, _pending_reply_rx) = oneshot::channel();
        let pending = PendingSpawn {
            profile_name: profile_name.clone(),
            agent_identity: agent_identity.clone(),
            admitted_bridge_session_id,
            prompt: prompt.clone(),
            runtime_mode,
            labels: labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: None,
            effective_profile_override: None,
            progress: Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default())),
            reply_tx: pending_reply_tx,
        };
        let pending_task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        self.insert_pending_spawn(spawn_ticket, pending, pending_task);
        if let Err(error) =
            self.ensure_pending_spawn_alignment("spawn_from_policy_inline staged pending")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated while staging inline policy spawn"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated while staging inline policy spawn",
            )
            .await;
            return Err(error);
        }

        let spawn_result = Box::pin(async {
            let spawn_receipt = self.provisioner.provision_member(provision_request).await?;
            if runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        agent_identity,
                        &spawn_receipt.member_ref,
                    )
                    .await
            {
                if let Err(retire_error) = self
                    .provisioner
                    .retire_member(&spawn_receipt.member_ref)
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "autonomous capability check failed for '{agent_identity}': {capability_error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(capability_error);
            }
            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                agent_identity.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "policy spawn completed while mob state changed for '{agent_identity}': {error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(error);
            }
            let fence = self.issue_fence_token();
            self.finalize_spawn_from_pending(
                &profile_name,
                agent_identity,
                crate::ids::Generation::INITIAL,
                fence,
                runtime_mode,
                prompt,
                labels,
                provision,
                spawn_receipt.operation_id,
                None,
                false,
                None,
                None, // policy spawns use definition profiles, no override
            )
            .await
            .map(|outcome| outcome.receipt)
        })
        .await;

        let (_pending, task_handle) =
            self.complete_pending_spawn_slot(spawn_ticket, "policy inline spawn completion");
        if let Some(handle) = task_handle {
            handle.abort();
        }
        if let Err(error) =
            self.ensure_pending_spawn_alignment("spawn_from_policy_inline completion")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated after inline policy spawn completion"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated after inline policy spawn completion",
            )
            .await;
            return Err(error);
        }

        spawn_result
    }

    #[allow(clippy::too_many_arguments)]
    async fn finalize_spawn_from_pending(
        &mut self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
        generation: crate::ids::Generation,
        fence_token: crate::ids::FenceToken,
        runtime_mode: crate::MobRuntimeMode,
        prompt: ContentInput,
        labels: std::collections::BTreeMap<String, String>,
        provision: PendingProvision,
        operation_id: meerkat_core::ops::OperationId,
        owner_bridge_session_id: Option<SessionId>,
        auto_wire_parent: bool,
        restore_wiring: Option<RestoreWiringPlan>,
        effective_profile_override: Option<crate::profile::Profile>,
    ) -> Result<FinalizeSpawnOutcome, MobError> {
        let identity = crate::ids::AgentIdentity::from(agent_identity.as_str());
        let agent_runtime_id = crate::ids::AgentRuntimeId::new(identity.clone(), generation);
        let overlay_record =
            self.external_binding_overlay_record(&identity, generation, provision.member_ref());

        // Resolve `external_addressable` from the effective profile so we
        // can inform the DSL (see the `MobMachineInput::Spawn` dispatch
        // below). Honours any override previously applied via
        // `SpawnTooling::Profile` resolution.
        let external_addressable = if let Some(profile) = effective_profile_override.as_ref() {
            profile.external_addressable
        } else {
            self.definition
                .resolve_profile(profile_name, self.realm_profile_store.as_ref())
                .await
                .map(|profile| profile.external_addressable)
                .unwrap_or(false)
        };

        // Feed `Spawn` into the MobMachine DSL so it populates
        // `live_runtime_ids` + `externally_addressable_runtime_ids` and
        // downstream guards (Retire, SubmitWork, …) operate on authoritative
        // membership state.
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let bridge_session_id = match provision.member_ref().bridge_session_id() {
            Some(sid) => mob_dsl::SessionId::from_domain(sid),
            None => mob_dsl::SessionId::default(),
        };
        let replacing = self
            .dsl_authority
            .state
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::Spawn {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                generation: mob_dsl::Generation::from_domain(generation),
                external_addressable,
                bridge_session_id,
                replacing,
            },
            "finalize_spawn_from_pending_dsl_spawn",
        )?;

        let supervisor_private_trust_install = if let (Some(session_id), Some(comms)) = (
            provision.member_ref().bridge_session_id().cloned(),
            self.provisioner_comms(provision.member_ref()).await,
        ) {
            match self
                .install_supervisor_private_trust_for_session(&session_id, &comms, None)
                .await
            {
                Ok(install) => Some((session_id, comms, install)),
                Err(error) => {
                    let _ = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::Retire {
                            mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                                &agent_runtime_id,
                            ),
                            agent_identity: dsl_identity.clone(),
                            releasing: self
                                .dsl_authority
                                .state
                                .member_session_bindings
                                .get(&dsl_identity)
                                .cloned(),
                            session_id: self
                                .dsl_authority
                                .state
                                .member_session_bindings
                                .get(&dsl_identity)
                                .cloned()
                                .unwrap_or_else(mob_dsl::SessionId::default),
                        },
                        "finalize_spawn_supervisor_trust_failed_retire_dsl",
                    );
                    if let Some(session_id) = provision.member_ref().bridge_session_id() {
                        self.discard_pending_routed_effects_for_session(session_id);
                    }
                    if let Err(rollback_error) = provision.rollback().await {
                        return Err(MobError::Internal(format!(
                            "spawn supervisor private trust failed for '{agent_identity}': {error}; archive compensation failed: {rollback_error}"
                        )));
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };

        if let Some(overlay_record) = overlay_record.as_ref() {
            if let Err(error) = self
                .runtime_metadata
                .upsert_external_binding_overlay(&self.definition.id, overlay_record)
                .await
            {
                if let Some((session_id, comms, install)) =
                    supervisor_private_trust_install.as_ref()
                {
                    self.cleanup_supervisor_private_trust_for_session(session_id, comms, install)
                        .await;
                }
                let _ = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::Retire {
                        mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                        agent_identity: dsl_identity.clone(),
                        releasing: self
                            .dsl_authority
                            .state
                            .member_session_bindings
                            .get(&dsl_identity)
                            .cloned(),
                        session_id: self
                            .dsl_authority
                            .state
                            .member_session_bindings
                            .get(&dsl_identity)
                            .cloned()
                            .unwrap_or_else(mob_dsl::SessionId::default),
                    },
                    "finalize_spawn_overlay_failed_retire_dsl",
                );
                if let Some(session_id) = provision.member_ref().bridge_session_id() {
                    self.discard_pending_routed_effects_for_session(session_id);
                }
                if let Err(rollback_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "spawn overlay upsert failed for '{agent_identity}': {error}; archive compensation failed: {rollback_error}"
                    )));
                }
                return Err(error.into());
            }
        }
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberSpawned({
                    let mut event = crate::event::MemberSpawnedEvent::new(
                        identity.clone(),
                        generation,
                        fence_token,
                        agent_runtime_id.clone(),
                        profile_name.clone(),
                    )
                    .with_bridge_member_ref(Some(Self::sanitized_member_ref(
                        provision.member_ref(),
                    )));
                    event.runtime_mode = runtime_mode;
                    event.labels = labels.clone();
                    event
                }),
            })
            .await
        {
            if overlay_record.is_some() {
                let _ = self
                    .delete_external_binding_overlay_for_member(&identity, generation)
                    .await;
            }
            if let Some((session_id, comms, install)) = supervisor_private_trust_install.as_ref() {
                self.cleanup_supervisor_private_trust_for_session(session_id, comms, install)
                    .await;
            }
            let _ = self.apply_dsl_input(
                mob_dsl::MobMachineInput::Retire {
                    mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&agent_runtime_id),
                    agent_identity: dsl_identity.clone(),
                    releasing: self
                        .dsl_authority
                        .state
                        .member_session_bindings
                        .get(&dsl_identity)
                        .cloned(),
                    session_id: self
                        .dsl_authority
                        .state
                        .member_session_bindings
                        .get(&dsl_identity)
                        .cloned()
                        .unwrap_or_else(mob_dsl::SessionId::default),
                },
                "finalize_spawn_append_failed_retire_dsl",
            );
            if let Some(session_id) = provision.member_ref().bridge_session_id() {
                self.discard_pending_routed_effects_for_session(session_id);
            }
            if let Err(rollback_error) = provision.rollback().await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{agent_identity}': {append_error}; archive compensation failed: {rollback_error}"
                )));
            }
            return Err(MobError::from(append_error));
        }

        // Commit the provision: the member is now owned by the roster.
        // From this point, rollback_failed_spawn handles cleanup via the
        // disposal pipeline.
        let member_ref = provision.commit()?;
        self.restore_diagnostics
            .write()
            .await
            .remove(agent_identity);

        // Populate the Roster projection AFTER DSL `Spawn` authoritatively
        // applies. The pre-DSL roster insert was deleted in Wave-A commit
        // `e77ce8797` (running before `MobMachineInput::Spawn` committed, so
        // rejected admissions could leave shell state stale); the
        // correctly-ordered replacement was never wired until now. Without
        // this insert, `start_autonomous_member` below reads an empty roster
        // and fails with `"autonomous member '{id}' missing roster entry for
        // startup readiness"` (#30 D-spawn-readiness-lookup).
        //
        // `peer_id` is the canonical comms routing UUID. Keep the Ed25519
        // public key in its own transport/auth slot so roster projections do
        // not bind peer-directory lookups to key material.
        let (peer_id, transport_public_key) =
            if let Some(session_id) = member_ref.bridge_session_id() {
                match self.session_service.comms_runtime(session_id).await {
                    Some(runtime) => (runtime.peer_id(), runtime.public_key()),
                    None => (None, None),
                }
            } else {
                (None, None)
            };
        {
            let mut roster = self.roster.write().await;
            roster.add_member(crate::roster::RosterAddEntry {
                agent_identity: identity.clone(),
                generation,
                fence_token,
                agent_runtime_id: agent_runtime_id.clone(),
                role: profile_name.clone(),
                runtime_mode,
                member_ref: Self::sanitized_member_ref(&member_ref),
                peer_id,
                transport_public_key,
                labels: labels.clone(),
                effective_profile_override: effective_profile_override.clone(),
            });
        }

        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkPending {
                        member_id: agent_identity.to_string(),
                    },
                )
                .await?;
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending roster updated"
        );

        // Wave-A damage restored: `spawn_wiring_targets` computes the
        // auto-wire + role-wiring fan-out targets for this spawn, and the
        // imperative wire-call loop deleted alongside the pre-DSL
        // `roster.add_member` insert (commit `e77ce8797`) needs to run here
        // so role-wired profiles actually get wired at spawn time. The
        // compensating rollback in `rollback_failed_spawn` expects both
        // `wired_spawn_targets` and `planned_wiring_targets` populated so
        // partial-wire failures can unwind.
        let mut planned_wiring_targets = self
            .spawn_wiring_targets(profile_name, agent_identity)
            .await;
        if auto_wire_parent
            && let Some(parent_target) = self
                .resolve_auto_wire_parent_target(owner_bridge_session_id.as_ref(), agent_identity)
                .await
            && !planned_wiring_targets.contains(&parent_target)
        {
            planned_wiring_targets.push(parent_target);
        }
        let mut wired_spawn_targets: Vec<MeerkatId> = Vec::new();
        for target in &planned_wiring_targets {
            let target_identity = crate::ids::AgentIdentity::from(target.as_str());
            let local_meerkat = agent_identity.clone();
            match self
                .handle_wire(
                    local_meerkat,
                    super::handle::PeerTarget::Local(target_identity),
                )
                .await
            {
                Ok(()) => wired_spawn_targets.push(target.clone()),
                Err(wire_error) => {
                    let surfaced_wire_error = match wire_error {
                        MobError::WiringError(_) => wire_error,
                        other => MobError::WiringError(other.to_string()),
                    };
                    // Rollback the spawn: the member is in the DSL + roster
                    // but the role-wiring contract was violated. Surface the
                    // failure to the caller so they can decide how to
                    // compensate (tests assert this path at e.g.
                    // `test_role_wiring_failure_is_returned_to_spawn_caller`).
                    self.clear_kickoff_state(agent_identity).await;
                    if let Err(rollback_error) = self
                        .rollback_failed_spawn(
                            agent_identity,
                            profile_name,
                            &member_ref,
                            &wired_spawn_targets,
                            &planned_wiring_targets,
                        )
                        .await
                    {
                        return Err(MobError::Internal(format!(
                            "spawn wire fan-out failed for '{agent_identity}': {surfaced_wire_error}; rollback failed: {rollback_error}"
                        )));
                    }
                    return Err(surfaced_wire_error);
                }
            }
        }

        #[cfg(feature = "runtime-adapter")]
        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkStarting {
                        member_id: agent_identity.to_string(),
                    },
                )
                .await?;
            // Spawn emits RequestRuntimeBinding. Drain it before startup can
            // publish RuntimeBound, otherwise the session may emit a fallback
            // runtime id that MobMachine correctly rejects as not live.
            if let Err(binding_error) = self.flush_routed_effects().await {
                self.clear_kickoff_state(agent_identity).await;
                if let Err(rollback_error) = self
                    .rollback_failed_spawn(
                        agent_identity,
                        profile_name,
                        &member_ref,
                        &wired_spawn_targets,
                        &planned_wiring_targets,
                    )
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "spawn runtime binding failed for '{agent_identity}': {binding_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(binding_error);
            }
            if let Err(start_error) = self
                .start_autonomous_member(agent_identity, &member_ref, prompt)
                .await
            {
                self.clear_kickoff_state(agent_identity).await;
                if let Err(rollback_error) = self
                    .rollback_failed_spawn(
                        agent_identity,
                        profile_name,
                        &member_ref,
                        &wired_spawn_targets,
                        &planned_wiring_targets,
                    )
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "spawn host-loop start failed for '{agent_identity}': {start_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(start_error);
            }
        } else {
            // Turn-driven mob members still need a persistent comms drain:
            // async peer requests/responses arrive between user turns (think
            // realtime audio operators calling `send_request` and waiting for
            // `send_response`). Without a drain running, the
            // `peer_response_terminal` notice never reaches the session's
            // runtime queue, so the wake path is dead. The drain-spawn seam
            // is independent of `config.keep_alive` (which the mock session
            // services overload as "block on start_turn"), so we drive it
            // explicitly here for turn-driven members that have a bridge
            // session and a comms runtime.
            #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
            if let (Some(adapter), Some(bridge_session_id)) =
                (self.runtime_adapter.clone(), member_ref.bridge_session_id())
            {
                let comms_runtime = self.provisioner.comms_runtime(&member_ref).await;
                if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some()
                    && let Some(runtime) = comms_runtime.as_ref()
                {
                    tracing::info!(
                        agent_identity = %agent_identity,
                        session_id = %bridge_session_id,
                        comms_ptr = ?Arc::as_ptr(runtime),
                        "mob turn-driven spawn binding comms drain"
                    );
                }
                // W2-G: route through the mob-owned spawn seam so peer-ingress
                // ownership transitions to `MobOwned { comms_runtime_id, mob_id }`.
                if let Some(comms_runtime) = comms_runtime {
                    let mob_id = meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        self.definition.id.as_ref(),
                    );
                    let _ = adapter
                        .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                        .await;
                }
            }
        }

        // Respawn restore: re-fire topology captured from MobMachine, not
        // from roster projection fields. `handle_wire` repairs live comms and
        // read-model projection after MobMachine admits or already owns the
        // edge, so respawn never writes roster wiring directly.
        let mut failed_restore_peer_ids: Vec<MeerkatId> = Vec::new();
        if let Some(plan) = restore_wiring {
            for peer_identity in plan.local_peers {
                if peer_identity == *agent_identity {
                    continue;
                }
                let peer_agent_identity = crate::ids::AgentIdentity::from(peer_identity.as_str());
                if let Err(error) = self
                    .handle_wire(
                        agent_identity.clone(),
                        super::handle::PeerTarget::Local(peer_agent_identity),
                    )
                    .await
                {
                    tracing::warn!(
                        agent_identity = %agent_identity,
                        peer = %peer_identity,
                        %error,
                        "respawn: failed to restore machine-owned local peer edge"
                    );
                    failed_restore_peer_ids.push(peer_identity);
                }
            }
            for peer_spec in plan.external_peers {
                let peer_name_id = MeerkatId::from(peer_spec.name.as_str());
                if let Err(error) = self
                    .handle_wire(
                        agent_identity.clone(),
                        super::handle::PeerTarget::External(peer_spec.clone()),
                    )
                    .await
                {
                    tracing::warn!(
                        agent_identity = %agent_identity,
                        peer = %peer_spec.name,
                        %error,
                        "respawn: failed to restore machine-owned external peer edge"
                    );
                    failed_restore_peer_ids.push(peer_name_id);
                }
            }
        }

        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor::finalize_spawn_from_pending done"
        );
        Ok(FinalizeSpawnOutcome {
            receipt: super::handle::MemberSpawnReceipt {
                member_ref,
                operation_id,
            },
            failed_restore_peer_ids,
        })
    }

    async fn spawn_wiring_targets(
        &self,
        profile_name: &ProfileName,
        agent_identity: &MeerkatId,
    ) -> Vec<MeerkatId> {
        let mut targets = Vec::new();
        let broken_members = self
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();

        if self.definition.wiring.auto_wire_orchestrator
            && let Some(orchestrator) = &self.definition.orchestrator
            && profile_name != &orchestrator.profile
        {
            let orchestrator_ids = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orchestrator.profile)
                    .filter(|entry| {
                        entry.state == crate::roster::MemberState::Active
                            && !broken_members.contains(&entry.agent_identity)
                    })
                    .map(|entry| entry.agent_identity.clone())
                    .collect::<Vec<_>>()
            };
            for orchestrator_id in orchestrator_ids {
                if orchestrator_id != *agent_identity && !targets.contains(&orchestrator_id) {
                    targets.push(orchestrator_id);
                }
            }
        }

        for rule in &self.definition.wiring.role_wiring {
            let target_profile = if &rule.a == profile_name {
                Some(&rule.b)
            } else if &rule.b == profile_name {
                Some(&rule.a)
            } else {
                None
            };
            if let Some(target_profile) = target_profile {
                let target_ids = {
                    let roster = self.roster.read().await;
                    roster
                        .by_profile(target_profile)
                        .filter(|entry| {
                            entry.state == crate::roster::MemberState::Active
                                && !broken_members.contains(&entry.agent_identity)
                                && entry.agent_identity != *agent_identity
                        })
                        .map(|entry| entry.agent_identity.clone())
                        .collect::<Vec<_>>()
                };
                for target_id in target_ids {
                    if !targets.contains(&target_id) {
                        targets.push(target_id);
                    }
                }
            }
        }

        targets
    }

    async fn resolve_auto_wire_parent_target(
        &self,
        owner_bridge_session_id: Option<&SessionId>,
        spawned_meerkat_id: &MeerkatId,
    ) -> Option<MeerkatId> {
        let owner_bridge_session_id = owner_bridge_session_id?;
        let broken_members = self
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let roster = self.roster.read().await;
        roster
            .list()
            .find(|entry| {
                entry.state == crate::roster::MemberState::Active
                    && entry.agent_identity != *spawned_meerkat_id
                    && !broken_members.contains(&entry.agent_identity)
                    && entry.member_ref.bridge_session_id() == Some(owner_bridge_session_id)
            })
            .map(|entry| entry.agent_identity.clone())
    }

    /// P1-T05: retire() removes a meerkat.
    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Does NOT retire the member — the member remains in the roster and can
    /// receive new turns. Use [`handle_retire`] to fully remove a member.
    async fn handle_force_cancel(&mut self, agent_identity: MeerkatId) -> Result<(), MobError> {
        let roster = self.roster.read().await;
        let entry = roster
            .get(&agent_identity)
            .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
        let member_ref = entry.member_ref.clone();
        drop(roster);

        self.preview_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
        self.provisioner.interrupt_member(&member_ref).await?;
        self.apply_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
        Ok(())
    }

    /// D-wire-handler (#26) + #31 D-trust-reconciliation (Wave D): forward a
    /// wire command to the MobMachine DSL and install bidirectional comms
    /// trust + peer notifications.
    ///
    /// Shell-mechanical steps on a successful local-local wire:
    ///
    /// 1. Normalize `(local, target)` into a canonical `WiringEdge`.
    /// 2. Resolve both endpoints' comms runtimes + trusted-peer specs. Any
    ///    missing comms runtime / public key fails fast as
    ///    [`MobError::WiringError`] with **zero side effects**.
    /// 3. Submit `MobMachineInput::WireMembers { edge }` to the DSL
    ///    authority. `edge_not_already_wired` makes re-wiring the same
    ///    edge a no-op success (idempotent) when the roster is already
    ///    in sync. Otherwise we proceed to repair live comms state.
    /// 4. On DSL acceptance, bidirectionally install trust on both
    ///    runtimes (A trusts B, B trusts A) and emit `mob.peer_added`
    ///    notifications from both sides. Any failure mid-step rolls back
    ///    the trust installs and reverts the DSL wire so failure leaves
    ///    no observable side effect. This replaces the pre-DSL
    ///    trust-install loop that Wave-A commit `0ad584cde` deleted.
    /// 5. Append `MembersWired` event and project it through the roster.
    ///    Append failure also rolls back trust + DSL wire.
    ///
    /// External peer targets ([`PeerTarget::External`]) are routed to
    /// [`Self::handle_wire_external`], whose descriptor-bearing trust edge is
    /// admitted by `MobMachineInput::WireExternalPeer` instead of being
    /// coerced into a member `WiringEdge`.
    async fn handle_wire(
        &mut self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        let peer_identity = match target {
            super::handle::PeerTarget::Local(id) => id,
            super::handle::PeerTarget::External(descriptor) => {
                return self.handle_wire_external(local, descriptor).await;
            }
        };

        let local_identity = AgentIdentity::from(local.as_str());
        if local_identity == peer_identity {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{local}')"
            )));
        }
        self.ensure_member_not_broken(&local).await?;

        let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
        self.ensure_member_not_broken(&peer_meerkat_id).await?;

        // Project domain identities into the DSL bridging type.
        let dsl_a = mob_dsl::AgentIdentity::from_domain(&local_identity);
        let dsl_b = mob_dsl::AgentIdentity::from_domain(&peer_identity);
        let edge = mob_dsl::WiringEdge::new(dsl_a, dsl_b);

        // Pre-flight: roster lookups. Missing members fail fast before
        // any authority mutation.
        let (local_entry, peer_entry) = {
            let roster = self.roster.read().await;
            (
                roster
                    .get(&local)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(local.clone()))?,
                roster
                    .get(&peer_meerkat_id)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(peer_meerkat_id.clone()))?,
            )
        };

        // Idempotent short-circuit when DSL AND roster both already
        // reflect the edge — no trust/notification fan-out needed.
        let dsl_has_edge = self
            .dsl_authority
            .state
            .wiring_edges
            .iter()
            .any(|existing| existing == &edge);
        let roster_has_edge = local_entry.wired_to.contains(&peer_entry.agent_identity)
            && peer_entry.wired_to.contains(&local_entry.agent_identity);

        // Resolve both endpoints' comms runtimes + specs BEFORE mutating
        // any authority state. Missing comms / missing public key yields
        // WiringError with zero side effects.
        let local_endpoint = self.resolve_wiring_endpoint(&local_entry, "wire").await?;
        let peer_endpoint = self.resolve_wiring_endpoint(&peer_entry, "wire").await?;
        if dsl_has_edge && roster_has_edge {
            match (&local_endpoint, &peer_endpoint) {
                (
                    WiringEndpoint::Local {
                        comms: local_comms,
                        spec: local_spec,
                        ..
                    },
                    WiringEndpoint::Local {
                        comms: peer_comms,
                        spec: peer_spec,
                        ..
                    },
                ) => {
                    local_comms.add_trusted_peer(peer_spec.clone()).await?;
                    peer_comms.add_trusted_peer(local_spec.clone()).await?;
                }
                (
                    WiringEndpoint::PeerOnly {
                        spec: local_spec,
                        binding: local_binding,
                    },
                    WiringEndpoint::PeerOnly {
                        spec: peer_spec,
                        binding: peer_binding,
                    },
                ) => {
                    self.wire_peer_only_recipient(
                        local_spec,
                        Some(local_binding),
                        peer_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await?;
                    self.wire_peer_only_recipient(
                        peer_spec,
                        Some(peer_binding),
                        local_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await?;
                }
                (
                    WiringEndpoint::Local {
                        comms: local_comms,
                        spec: local_spec,
                        ..
                    },
                    WiringEndpoint::PeerOnly {
                        spec: peer_spec,
                        binding: peer_binding,
                    },
                ) => {
                    local_comms.add_trusted_peer(peer_spec.clone()).await?;
                    self.wire_peer_only_recipient(
                        peer_spec,
                        Some(peer_binding),
                        local_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await?;
                }
                (
                    WiringEndpoint::PeerOnly {
                        spec: local_spec,
                        binding: local_binding,
                    },
                    WiringEndpoint::Local {
                        comms: peer_comms,
                        spec: peer_spec,
                        ..
                    },
                ) => {
                    peer_comms.add_trusted_peer(local_spec.clone()).await?;
                    self.wire_peer_only_recipient(
                        local_spec,
                        Some(local_binding),
                        peer_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await?;
                }
            }
            return Ok(());
        }
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let dsl_added = self.apply_wire_members_idempotent(&edge)?;
            if let Err(error) = self
                .wire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_wire(&edge, dsl_added, &[], local_spec, peer_spec)
                    .await;
                return Err(error);
            }
            if let Err(error) = self
                .wire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                self.rollback_peer_only_wire(&edge, dsl_added, &["local"], local_spec, peer_spec)
                    .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["local", "peer"],
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        if let (
            WiringEndpoint::Local {
                comms: local_comms,
                spec: local_spec,
                ..
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let dsl_added = self.apply_wire_members_idempotent(&edge)?;
            if let Err(error) = local_comms.add_trusted_peer(peer_spec.clone()).await {
                self.rollback_peer_only_wire(&edge, dsl_added, &[], local_spec, peer_spec)
                    .await;
                return Err(MobError::from(error));
            }
            if let Err(error) = self
                .wire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                let _ = local_comms
                    .remove_trusted_peer(&Self::trusted_peer_removal_key(peer_spec))
                    .await;
                self.rollback_peer_only_wire(&edge, dsl_added, &[], local_spec, peer_spec)
                    .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    let _ = local_comms
                        .remove_trusted_peer(&Self::trusted_peer_removal_key(peer_spec))
                        .await;
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["peer"],
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::Local {
                comms: peer_comms,
                spec: peer_spec,
                ..
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            let dsl_added = self.apply_wire_members_idempotent(&edge)?;
            if let Err(error) = peer_comms.add_trusted_peer(local_spec.clone()).await {
                self.rollback_peer_only_wire(&edge, dsl_added, &[], local_spec, peer_spec)
                    .await;
                return Err(MobError::from(error));
            }
            if let Err(error) = self
                .wire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                let _ = peer_comms
                    .remove_trusted_peer(&Self::trusted_peer_removal_key(local_spec))
                    .await;
                self.rollback_peer_only_wire(&edge, dsl_added, &[], local_spec, peer_spec)
                    .await;
                return Err(error);
            }
            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersWired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    let _ = peer_comms
                        .remove_trusted_peer(&Self::trusted_peer_removal_key(local_spec))
                        .await;
                    self.rollback_peer_only_wire(
                        &edge,
                        dsl_added,
                        &["local"],
                        local_spec,
                        peer_spec,
                    )
                    .await;
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        let (local_comms, local_spec) = match local_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "wire requires local session comms runtime for '{local}'"
                )));
            }
        };
        let (peer_comms, peer_spec) = match peer_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "wire requires local session comms runtime for '{peer_identity}'"
                )));
            }
        };

        // Submit the DSL input. `edge_not_already_wired` may reject as
        // idempotent-success. We still fall through to repair trust/
        // notifications if the roster projection is out of sync.
        let dsl_added = match self.apply_dsl_input(
            mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
            "wire_members",
        ) {
            Ok(()) => true,
            Err(MobError::Internal(message)) if message.contains("wire_members") => {
                if self
                    .dsl_authority
                    .state
                    .wiring_edges
                    .iter()
                    .any(|existing| existing == &edge)
                {
                    false
                } else {
                    return Err(MobError::WiringError(format!(
                        "wire rejected by MobMachine DSL: {message}"
                    )));
                }
            }
            Err(other) => return Err(other),
        };

        let local_peer_id = Self::trusted_peer_removal_key(&local_spec);
        let peer_peer_id = Self::trusted_peer_removal_key(&peer_spec);

        // A-side trust install.
        if let Err(err) = local_comms.add_trusted_peer(peer_spec.clone()).await {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                false,
                false,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
            )
            .await;
            return Err(MobError::from(err));
        }

        // B-side trust install.
        if let Err(err) = peer_comms.add_trusted_peer(local_spec.clone()).await {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                true,
                false,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
            )
            .await;
            return Err(MobError::from(err));
        }

        // Notify A that B is now wired.
        if let Err(err) = self
            .notify_peer_added(&peer_comms, &local_spec, &peer_meerkat_id, &peer_entry)
            .await
        {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                true,
                true,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
            )
            .await;
            return Err(err);
        }

        // Notify B that A is now wired.
        if let Err(err) = self
            .notify_peer_added(&local_comms, &peer_spec, &local, &local_entry)
            .await
        {
            self.rollback_wire_side_effects(
                &edge,
                dsl_added,
                true,
                true,
                &local_comms,
                &peer_comms,
                &local_peer_id,
                &peer_peer_id,
            )
            .await;
            return Err(err);
        }

        // Append MembersWired — rollback on failure.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersWired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(err) => {
                self.rollback_wire_side_effects(
                    &edge,
                    dsl_added,
                    true,
                    true,
                    &local_comms,
                    &peer_comms,
                    &local_peer_id,
                    &peer_peer_id,
                )
                .await;
                return Err(MobError::from(err));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    fn apply_wire_members_idempotent(
        &mut self,
        edge: &mob_dsl::WiringEdge,
    ) -> Result<bool, MobError> {
        match self.apply_dsl_input(
            mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
            "wire_members",
        ) {
            Ok(()) => Ok(true),
            Err(MobError::Internal(message)) if message.contains("wire_members") => {
                if self
                    .dsl_authority
                    .state
                    .wiring_edges
                    .iter()
                    .any(|existing| existing == edge)
                {
                    Ok(false)
                } else {
                    Err(MobError::WiringError(format!(
                        "wire rejected by MobMachine DSL: {message}"
                    )))
                }
            }
            Err(other) => Err(other),
        }
    }

    async fn rollback_peer_only_wire(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_added: bool,
        installed_sides: &[&str],
        local_spec: &TrustedPeerDescriptor,
        peer_spec: &TrustedPeerDescriptor,
    ) {
        if installed_sides.contains(&"local")
            && let Err(error) = self
                .unwire_peer_only_recipient(
                    local_spec,
                    None,
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to unwire local recipient"
            );
        }
        if installed_sides.contains(&"peer")
            && let Err(error) = self
                .unwire_peer_only_recipient(
                    peer_spec,
                    None,
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to unwire peer recipient"
            );
        }
        if dsl_added
            && let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
                "peer_only_wire_rollback",
            )
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "peer-only wire rollback: failed to revert DSL wire"
            );
        }
    }

    /// Unwind side effects from a failed local-local wire. Best-effort:
    /// logs on compensation failure since we've already decided to surface
    /// the original error to the caller.
    #[allow(clippy::too_many_arguments)]
    async fn rollback_wire_side_effects(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_added: bool,
        installed_local_trust: bool,
        installed_peer_trust: bool,
        local_comms: &Arc<dyn CoreCommsRuntime>,
        peer_comms: &Arc<dyn CoreCommsRuntime>,
        local_peer_id: &str,
        peer_peer_id: &str,
    ) {
        if installed_local_trust {
            if let Err(err) = local_comms.remove_trusted_peer(peer_peer_id).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to remove local trust"
                );
            }
        }
        if installed_peer_trust {
            if let Err(err) = peer_comms.remove_trusted_peer(local_peer_id).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to remove peer trust"
                );
            }
        }
        if dsl_added {
            if let Err(err) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
                "wire_members_rollback",
            ) {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "wire rollback: failed to revert DSL wire"
                );
            }
        }
    }

    /// D-wire-handler (#26): forward an unwire command to the MobMachine DSL.
    ///
    /// Mirror of [`handle_wire`]: submits
    /// `MobMachineInput::UnwireMembers { edge }` and records
    /// `MobEventKind::MembersUnwired { a, b }` on acceptance. DSL
    /// rejection on the `edge_currently_wired` guard is treated as
    /// idempotent success (the edge is already absent from the authority).
    async fn handle_unwire(
        &mut self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        let peer_identity = match target {
            super::handle::PeerTarget::Local(id) => {
                // If the peer is not in the roster but is tracked as an
                // external peer spec on the local member, route through
                // the external unwire path. Callers commonly unwire an
                // external peer by name (projected as an `AgentIdentity`
                // Local target) rather than passing the full descriptor.
                let external_peer = {
                    let roster = self.roster.read().await;
                    match roster.get(&local) {
                        Some(local_entry)
                            if roster.get(&MeerkatId::from(id.as_str())).is_none() =>
                        {
                            local_entry.external_peer_specs.contains_key(&id)
                        }
                        _ => false,
                    }
                };
                if external_peer {
                    let peer_name = meerkat_core::comms::PeerName::new(id.as_str().to_string())
                        .map_err(|error| {
                            MobError::WiringError(format!(
                                "unwire external peer has invalid peer name: {error}",
                            ))
                        })?;
                    return self.handle_unwire_external(local, peer_name, None).await;
                }
                id
            }
            super::handle::PeerTarget::External(descriptor) => {
                let peer_name = meerkat_core::comms::PeerName::new(descriptor.name.clone())
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "unwire external peer has invalid peer name: {error}",
                        ))
                    })?;
                return self
                    .handle_unwire_external(local, peer_name, Some(descriptor))
                    .await;
            }
        };

        let local_identity = AgentIdentity::from(local.as_str());
        if local_identity == peer_identity {
            return Err(MobError::WiringError(format!(
                "unwire requires distinct peers (got '{local}')"
            )));
        }

        let peer_meerkat_id = MeerkatId::from(peer_identity.as_str());
        let dsl_a = mob_dsl::AgentIdentity::from_domain(&local_identity);
        let dsl_b = mob_dsl::AgentIdentity::from_domain(&peer_identity);
        let edge = mob_dsl::WiringEdge::new(dsl_a, dsl_b);

        let (local_entry, peer_entry) = {
            let roster = self.roster.read().await;
            (
                roster
                    .get(&local)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(local.clone()))?,
                roster
                    .get(&peer_meerkat_id)
                    .cloned()
                    .ok_or_else(|| MobError::MemberNotFound(peer_meerkat_id.clone()))?,
            )
        };

        let dsl_has_edge = self
            .dsl_authority
            .state
            .wiring_edges
            .iter()
            .any(|existing| existing == &edge);
        let roster_has_edge = local_entry.wired_to.contains(&peer_entry.agent_identity)
            || peer_entry.wired_to.contains(&local_entry.agent_identity);

        // Resolve endpoints. Failing here leaves the DSL + roster
        // untouched — matches the zero-side-effect expectations.
        let local_endpoint = self.resolve_wiring_endpoint(&local_entry, "unwire").await?;
        let peer_endpoint = self.resolve_wiring_endpoint(&peer_entry, "unwire").await?;
        if let (
            WiringEndpoint::PeerOnly {
                spec: local_spec,
                binding: local_binding,
            },
            WiringEndpoint::PeerOnly {
                spec: peer_spec,
                binding: peer_binding,
            },
        ) = (&local_endpoint, &peer_endpoint)
        {
            if !dsl_has_edge && !roster_has_edge {
                let _ = self
                    .unwire_peer_only_recipient(
                        local_spec,
                        Some(local_binding),
                        peer_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await;
                let _ = self
                    .unwire_peer_only_recipient(
                        peer_spec,
                        Some(peer_binding),
                        local_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await;
                return Ok(());
            }

            let dsl_removed = self.apply_unwire_members_idempotent(&edge)?;
            if let Err(error) = self
                .unwire_peer_only_recipient(
                    local_spec,
                    Some(local_binding),
                    peer_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                if dsl_removed {
                    let _ = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
                        "peer_only_unwire_rollback",
                    );
                }
                return Err(error);
            }
            if let Err(error) = self
                .unwire_peer_only_recipient(
                    peer_spec,
                    Some(peer_binding),
                    local_spec,
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                let _ = self
                    .wire_peer_only_recipient(
                        local_spec,
                        Some(local_binding),
                        peer_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await;
                if dsl_removed {
                    let _ = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
                        "peer_only_unwire_rollback",
                    );
                }
                return Err(error);
            }

            let event = NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MembersUnwired {
                    a: AgentIdentity::from(edge.a.0.as_str()),
                    b: AgentIdentity::from(edge.b.0.as_str()),
                },
            };
            let stored = match self.events.append(event).await {
                Ok(stored) => stored,
                Err(error) => {
                    let _ = self
                        .wire_peer_only_recipient(
                            local_spec,
                            Some(local_binding),
                            peer_spec,
                            std::time::Duration::from_secs(10),
                        )
                        .await;
                    let _ = self
                        .wire_peer_only_recipient(
                            peer_spec,
                            Some(peer_binding),
                            local_spec,
                            std::time::Duration::from_secs(10),
                        )
                        .await;
                    if dsl_removed {
                        let _ = self.apply_dsl_input(
                            mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
                            "peer_only_unwire_rollback",
                        );
                    }
                    return Err(MobError::from(error));
                }
            };
            self.roster.write().await.apply_event(&stored);
            return Ok(());
        }
        let (local_comms, local_spec) = match local_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "unwire requires local session comms runtime for '{local}'"
                )));
            }
        };
        let (peer_comms, peer_spec) = match peer_endpoint {
            WiringEndpoint::Local { comms, spec, .. } => (comms, spec),
            WiringEndpoint::PeerOnly { .. } => {
                return Err(MobError::WiringError(format!(
                    "unwire requires local session comms runtime for '{peer_identity}'"
                )));
            }
        };

        let local_peer_id = Self::trusted_peer_removal_key(&local_spec);
        let peer_peer_id = Self::trusted_peer_removal_key(&peer_spec);

        // Idempotent stale-trust cleanup: if the DSL + roster both show
        // the edge as already absent, but the comms runtimes still carry
        // trust (e.g. re-added externally after the first unwire), prune
        // it without emitting a new event.
        if !dsl_has_edge && !roster_has_edge {
            let _ = local_comms.remove_trusted_peer(&peer_peer_id).await;
            let _ = peer_comms.remove_trusted_peer(&local_peer_id).await;
            return Ok(());
        }

        // Submit DSL input first. Idempotent rejection (edge absent) is
        // handled above; here we expect acceptance (or a non-idempotent
        // rejection that must surface).
        let dsl_removed = match self.apply_dsl_input(
            mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
            "unwire_members",
        ) {
            Ok(()) => true,
            Err(MobError::Internal(message)) if message.contains("unwire_members") => {
                if !self
                    .dsl_authority
                    .state
                    .wiring_edges
                    .iter()
                    .any(|existing| existing == &edge)
                {
                    // DSL already absent — proceed with trust removal +
                    // notification so live comms state is repaired.
                    false
                } else {
                    return Err(MobError::WiringError(format!(
                        "unwire rejected by MobMachine DSL: {message}"
                    )));
                }
            }
            Err(other) => return Err(other),
        };

        let mut removed_local_trust = false;
        let mut removed_peer_trust = false;
        let mut sent_unwired_from_local = false;
        let mut sent_unwired_from_peer = false;

        // Notify peer_unwired on both sides BEFORE trust removal so the
        // notifications can still be delivered (the send path resolves
        // the recipient by name in the comms' trusted-peers table). If a
        // notification fails, compensate the prior side's notification
        // by re-sending peer_added so the observable intent stream stays
        // balanced.
        if let Err(err) = self
            .notify_peer_event(
                "mob.peer_unwired",
                &peer_spec,
                &peer_meerkat_id,
                &peer_entry,
                &local_comms,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
            )
            .await;
            return Err(err);
        }
        sent_unwired_from_local = true;

        if let Err(err) = self
            .notify_peer_event(
                "mob.peer_unwired",
                &local_spec,
                &local,
                &local_entry,
                &peer_comms,
            )
            .await
        {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
            )
            .await;
            return Err(err);
        }
        sent_unwired_from_peer = true;

        // A-side trust removal (after notifications succeeded).
        if let Err(err) = local_comms.remove_trusted_peer(&peer_peer_id).await {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
            )
            .await;
            return Err(MobError::from(err));
        }
        removed_local_trust = true;

        // B-side trust removal.
        if let Err(err) = peer_comms.remove_trusted_peer(&local_peer_id).await {
            self.rollback_unwire_side_effects(
                &edge,
                dsl_removed,
                removed_local_trust,
                removed_peer_trust,
                sent_unwired_from_local,
                sent_unwired_from_peer,
                &local_comms,
                &peer_comms,
                &local_spec,
                &peer_spec,
                &local,
                &peer_meerkat_id,
                &local_entry,
                &peer_entry,
            )
            .await;
            return Err(MobError::from(err));
        }
        removed_peer_trust = true;

        // Append MembersUnwired — rollback on failure.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MembersUnwired {
                a: AgentIdentity::from(edge.a.0.as_str()),
                b: AgentIdentity::from(edge.b.0.as_str()),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(err) => {
                self.rollback_unwire_side_effects(
                    &edge,
                    dsl_removed,
                    removed_local_trust,
                    removed_peer_trust,
                    sent_unwired_from_local,
                    sent_unwired_from_peer,
                    &local_comms,
                    &peer_comms,
                    &local_spec,
                    &peer_spec,
                    &local,
                    &peer_meerkat_id,
                    &local_entry,
                    &peer_entry,
                )
                .await;
                return Err(MobError::from(err));
            }
        };
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    fn apply_unwire_members_idempotent(
        &mut self,
        edge: &mob_dsl::WiringEdge,
    ) -> Result<bool, MobError> {
        match self.apply_dsl_input(
            mob_dsl::MobMachineInput::UnwireMembers { edge: edge.clone() },
            "unwire_members",
        ) {
            Ok(()) => Ok(true),
            Err(MobError::Internal(message)) if message.contains("unwire_members") => {
                if !self
                    .dsl_authority
                    .state
                    .wiring_edges
                    .iter()
                    .any(|existing| existing == edge)
                {
                    Ok(false)
                } else {
                    Err(MobError::WiringError(format!(
                        "unwire rejected by MobMachine DSL: {message}"
                    )))
                }
            }
            Err(other) => Err(other),
        }
    }

    fn external_peer_edge(
        local_identity: &AgentIdentity,
        spec: &TrustedPeerDescriptor,
    ) -> mob_dsl::ExternalPeerEdge {
        mob_dsl::ExternalPeerEdge::new(
            mob_dsl::AgentIdentity::from_domain(local_identity),
            mob_dsl::ExternalPeerEndpoint::from(spec),
        )
    }

    fn external_peer_edge_for_name(
        &self,
        local_identity: &AgentIdentity,
        peer_name: &meerkat_core::comms::PeerName,
    ) -> Option<mob_dsl::ExternalPeerEdge> {
        let local = mob_dsl::AgentIdentity::from_domain(local_identity);
        self.dsl_authority
            .state
            .external_peer_edges
            .iter()
            .find(|edge| edge.local == local && edge.endpoint.name.0 == peer_name.as_str())
            .cloned()
    }

    fn apply_wire_external_peer_idempotent(
        &mut self,
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> Result<bool, MobError> {
        match self.apply_dsl_input(
            mob_dsl::MobMachineInput::WireExternalPeer { edge: edge.clone() },
            "wire_external_peer",
        ) {
            Ok(()) => Ok(true),
            Err(MobError::Internal(message)) if message.contains("wire_external_peer") => {
                if self
                    .dsl_authority
                    .state
                    .external_peer_edges
                    .iter()
                    .any(|existing| existing == edge)
                {
                    Ok(false)
                } else {
                    Err(MobError::WiringError(format!(
                        "wire external peer rejected by MobMachine DSL: {message}"
                    )))
                }
            }
            Err(other) => Err(other),
        }
    }

    fn apply_unwire_external_peer_idempotent(
        &mut self,
        edge: &mob_dsl::ExternalPeerEdge,
    ) -> Result<bool, MobError> {
        match self.apply_dsl_input(
            mob_dsl::MobMachineInput::UnwireExternalPeer { edge: edge.clone() },
            "unwire_external_peer",
        ) {
            Ok(()) => Ok(true),
            Err(MobError::Internal(message)) if message.contains("unwire_external_peer") => {
                if !self
                    .dsl_authority
                    .state
                    .external_peer_edges
                    .iter()
                    .any(|existing| existing == edge)
                {
                    Ok(false)
                } else {
                    Err(MobError::WiringError(format!(
                        "unwire external peer rejected by MobMachine DSL: {message}"
                    )))
                }
            }
            Err(other) => Err(other),
        }
    }

    /// Unwind side effects from a failed local-local unwire. Best-effort.
    ///
    /// Re-installs trust on sides where trust was removed, and emits
    /// compensating `mob.peer_added` notifications on sides that already
    /// sent `mob.peer_unwired` so the observable intent stream stays
    /// balanced. The DSL wire is re-submitted if it was removed.
    #[allow(clippy::too_many_arguments)]
    async fn rollback_unwire_side_effects(
        &mut self,
        edge: &mob_dsl::WiringEdge,
        dsl_removed: bool,
        removed_local_trust: bool,
        removed_peer_trust: bool,
        sent_unwired_from_local: bool,
        sent_unwired_from_peer: bool,
        local_comms: &Arc<dyn CoreCommsRuntime>,
        peer_comms: &Arc<dyn CoreCommsRuntime>,
        local_spec: &TrustedPeerDescriptor,
        peer_spec: &TrustedPeerDescriptor,
        local_meerkat_id: &MeerkatId,
        peer_meerkat_id: &MeerkatId,
        local_entry: &RosterEntry,
        peer_entry: &RosterEntry,
    ) {
        if removed_local_trust {
            if let Err(err) = local_comms.add_trusted_peer(peer_spec.clone()).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to re-install local trust"
                );
            }
        }
        if removed_peer_trust {
            if let Err(err) = peer_comms.add_trusted_peer(local_spec.clone()).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to re-install peer trust"
                );
            }
        }
        // Compensate peer_unwired notifications by re-sending peer_added.
        if sent_unwired_from_local {
            if let Err(err) = self
                .notify_peer_added(local_comms, peer_spec, peer_meerkat_id, peer_entry)
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to send compensating peer_added from local side"
                );
            }
        }
        if sent_unwired_from_peer {
            if let Err(err) = self
                .notify_peer_added(peer_comms, local_spec, local_meerkat_id, local_entry)
                .await
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to send compensating peer_added from peer side"
                );
            }
        }
        if dsl_removed {
            if let Err(err) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::WireMembers { edge: edge.clone() },
                "unwire_members_rollback",
            ) {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %err,
                    "unwire rollback: failed to re-submit DSL wire"
                );
            }
        }
    }

    /// Wire a local member to an external trusted peer.
    ///
    /// `MobMachineInput::WireExternalPeer` owns the descriptor-bearing trust
    /// edge; the shell mechanically installs the admitted descriptor into the
    /// local comms runtime and projects the event only after the machine
    /// admits the edge.
    ///
    /// Idempotent: a second wire of the same (local, external_name) edge
    /// with the same descriptor is treated as a no-op success.
    async fn handle_wire_external(
        &mut self,
        local: MeerkatId,
        spec: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let local_identity = AgentIdentity::from(local.as_str());
        let external_identity = AgentIdentity::from(spec.name.as_str());
        if local_identity == external_identity {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{local}')"
            )));
        }
        let edge = Self::external_peer_edge(&local_identity, &spec);

        // Look up the local member's roster entry and session binding.
        let (member_ref, already_wired_with_same_spec) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&local)
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            let already = entry
                .external_peer_specs
                .get(&external_identity)
                .map(|existing| existing == &spec)
                .unwrap_or(false);
            (entry.member_ref.clone(), already)
        };

        // Resolve the local session's comms runtime for trust install.
        let comms = self.provisioner_comms(&member_ref).await.ok_or_else(|| {
            MobError::WiringError(format!(
                "wire requires comms runtime for '{local}' (external peer wire)"
            ))
        })?;

        let dsl_added = self.apply_wire_external_peer_idempotent(&edge)?;

        if already_wired_with_same_spec {
            comms.add_trusted_peer(spec.clone()).await?;
            return Ok(());
        }

        // Install trust on the local's session comms runtime.
        if let Err(error) = comms.add_trusted_peer(spec.clone()).await {
            self.rollback_external_wire_dsl(&edge, dsl_added).await;
            return Err(MobError::from(error));
        }

        // Append ExternalPeerWired — if the append fails, compensate by
        // rolling back the trust install so failure leaves no side effect.
        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerWired {
                local: local_identity,
                spec: spec.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_err) => {
                if let Err(rollback_err) = comms
                    .remove_trusted_peer(&Self::trusted_peer_removal_key(&spec))
                    .await
                {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        local = %local,
                        error = %rollback_err,
                        "failed to rollback external trust install after event append failure"
                    );
                }
                self.rollback_external_wire_dsl(&edge, dsl_added).await;
                return Err(MobError::from(append_err));
            }
        };

        // Mirror the event through the roster projection — same pattern
        // as MembersWired in `handle_wire`.
        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    async fn rollback_external_wire_dsl(
        &mut self,
        edge: &mob_dsl::ExternalPeerEdge,
        dsl_added: bool,
    ) {
        if dsl_added
            && let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::UnwireExternalPeer { edge: edge.clone() },
                "external_wire_rollback",
            )
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                %error,
                "external wire rollback: failed to revert DSL wire"
            );
        }
    }

    /// D-external-peer (#31): unwire a local member from an external trusted peer.
    ///
    /// Mirror of [`Self::handle_wire_external`]: removes trust from the
    /// local session comms runtime, appends `ExternalPeerUnwired`, and
    /// projects through the roster.
    ///
    /// Idempotent: unwiring an already-absent external peer is a no-op
    /// success.
    async fn handle_unwire_external(
        &mut self,
        local: MeerkatId,
        peer_name: meerkat_core::comms::PeerName,
        stale_cleanup_spec: Option<TrustedPeerDescriptor>,
    ) -> Result<(), MobError> {
        let local_identity = AgentIdentity::from(local.as_str());
        let external_identity = AgentIdentity::from(peer_name.as_str());

        // Look up the prior descriptor so we can compensate on append
        // failure. Idempotent: absent projection stays success, but a
        // descriptor-bearing External target still prunes any stale comms
        // trust that was re-injected after the first unwire.
        let (member_ref, prior_spec) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&local)
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            let prior = entry.external_peer_specs.get(&external_identity).cloned();
            (entry.member_ref.clone(), prior)
        };

        let authority_edge = prior_spec
            .as_ref()
            .map(|spec| Self::external_peer_edge(&local_identity, spec))
            .or_else(|| {
                stale_cleanup_spec
                    .as_ref()
                    .map(|spec| Self::external_peer_edge(&local_identity, spec))
            })
            .or_else(|| self.external_peer_edge_for_name(&local_identity, &peer_name));

        let Some(prior_spec) = prior_spec else {
            let dsl_removed = match authority_edge.as_ref() {
                Some(edge) => self.apply_unwire_external_peer_idempotent(edge)?,
                None => false,
            };
            if let Some(spec) = stale_cleanup_spec
                && let Some(comms) = self.provisioner_comms(&member_ref).await
            {
                if let Err(error) = comms
                    .remove_trusted_peer(&Self::trusted_peer_removal_key(&spec))
                    .await
                {
                    if dsl_removed
                        && let Some(edge) = authority_edge.as_ref()
                        && let Err(rollback_err) = self.apply_dsl_input(
                            mob_dsl::MobMachineInput::WireExternalPeer { edge: edge.clone() },
                            "external_unwire_stale_cleanup_rollback",
                        )
                    {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            local = %local,
                            error = %rollback_err,
                            "failed to rollback external DSL unwire after stale cleanup failure"
                        );
                    }
                    return Err(MobError::from(error));
                }
            }
            return Ok(());
        };

        let edge = authority_edge
            .unwrap_or_else(|| Self::external_peer_edge(&local_identity, &prior_spec));
        let comms = self.provisioner_comms(&member_ref).await.ok_or_else(|| {
            MobError::WiringError(format!(
                "unwire requires comms runtime for '{local}' (external peer unwire)"
            ))
        })?;

        let dsl_removed = self.apply_unwire_external_peer_idempotent(&edge)?;

        // Remove trust on the local session runtime.
        if let Err(error) = comms
            .remove_trusted_peer(&Self::trusted_peer_removal_key(&prior_spec))
            .await
        {
            if dsl_removed
                && let Err(rollback_err) = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::WireExternalPeer { edge: edge.clone() },
                    "external_unwire_trust_remove_rollback",
                )
            {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    local = %local,
                    error = %rollback_err,
                    "failed to rollback external DSL unwire after trust removal failure"
                );
            }
            return Err(MobError::from(error));
        }

        let event = NewMobEvent {
            mob_id: self.definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::ExternalPeerUnwired {
                local: local_identity,
                peer_name: peer_name.clone(),
            },
        };
        let stored = match self.events.append(event).await {
            Ok(stored) => stored,
            Err(append_err) => {
                // Restore trust on append failure so the unwire is a full
                // no-op from the caller's perspective.
                if let Err(rollback_err) = comms.add_trusted_peer(prior_spec.clone()).await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        local = %local,
                        error = %rollback_err,
                        "failed to rollback external trust removal after event append failure"
                    );
                }
                if dsl_removed
                    && let Err(rollback_err) = self.apply_dsl_input(
                        mob_dsl::MobMachineInput::WireExternalPeer { edge: edge.clone() },
                        "external_unwire_rollback",
                    )
                {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        local = %local,
                        error = %rollback_err,
                        "failed to rollback external DSL unwire after event append failure"
                    );
                }
                return Err(MobError::from(append_err));
            }
        };

        self.roster.write().await.apply_event(&stored);
        Ok(())
    }

    ///
    /// Mark-then-best-effort-cleanup: event first, mark Retiring, disposal
    /// pipeline (policy-driven), then unconditional roster removal.
    async fn handle_retire(&mut self, agent_identity: MeerkatId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_retire preflight")?;
        self.handle_retire_inner(&agent_identity, false, false)
            .await?;
        self.ensure_pending_spawn_alignment("handle_retire completion")
    }

    async fn detach_session_ingress_for_mob_destroy(
        &mut self,
        session_id: &SessionId,
        obligation: MobDestroyingSessionIngressObligation,
    ) -> Result<(), MobError> {
        use crate::generated::protocol_mob_destroying_session_ingress::{
            submit_session_ingress_detach_failed_for_mob_destroy,
            submit_session_ingress_detached_for_mob_destroy,
        };

        let detach_result = self.detach_runtime_session_ingress(session_id).await;
        match detach_result {
            Ok(()) => {
                submit_session_ingress_detached_for_mob_destroy(&mut self.dsl_authority, obligation)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "MobMachine SessionIngressDetachedForMobDestroy transition rejected: {error}"
                        ))
                    })?;
                Ok(())
            }
            Err(error) => {
                let reason = error.to_string();
                let _ = submit_session_ingress_detach_failed_for_mob_destroy(
                    &mut self.dsl_authority,
                    obligation,
                    reason.clone(),
                );
                Err(MobError::Internal(format!(
                    "mob_destroying_session_ingress detach failed for {session_id}: {reason}"
                )))
            }
        }
    }

    async fn detach_runtime_session_ingress(&self, session_id: &SessionId) -> Result<(), MobError> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = &self.runtime_adapter {
            adapter
                .update_peer_ingress_context(session_id, false, None)
                .await;
            let owner = adapter.peer_ingress_owner(session_id).await;
            if !matches!(owner, meerkat_runtime::PeerIngressOwner::Unattached) {
                return Err(MobError::Internal(format!(
                    "peer ingress owner remained attached after detach: {owner:?}"
                )));
            }
        }
        let _ = session_id;
        Ok(())
    }

    async fn admit_member_retire_for_destroy(
        &mut self,
        entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let agent_identity = &entry.agent_identity;
        let retire_event_already_present = self
            .retire_event_exists(agent_identity, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(agent_identity, &entry.role, &entry.member_ref)
                .await?;
        }

        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let releasing = self
            .dsl_authority
            .state
            .member_session_bindings
            .get(&dsl_identity)
            .cloned();
        let session_id_for_route = releasing
            .clone()
            .unwrap_or_else(mob_dsl::SessionId::default);
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::Retire {
                mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                agent_identity: dsl_identity,
                releasing: releasing.clone(),
                session_id: session_id_for_route,
            },
            "destroy_mark_member_retiring",
        )?;

        let mut detach_obligations =
            crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                &effects,
            );
        if let Some(obligation) = detach_obligations.pop() {
            let detach_session_id = entry
                .member_ref
                .bridge_session_id()
                .cloned()
                .or_else(|| releasing.and_then(|session_id| SessionId::parse(&session_id.0).ok()))
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "destroy retire for member '{agent_identity}' opened a session ingress detach obligation without a bridge session id"
                    ))
                })?;
            self.detach_session_ingress_for_mob_destroy(&detach_session_id, obligation)
                .await?;
        }

        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring_by_identity(agent_identity);
        }

        self.flush_routed_effects().await
    }

    async fn handle_retire_inner(
        &mut self,
        agent_identity: &MeerkatId,
        bulk: bool,
        preserve_realtime_binding: bool,
    ) -> Result<(), MobError> {
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(agent_identity).cloned() else {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %agent_identity,
                    "retire requested for unknown meerkat id"
                );
                return Ok(());
            };
            entry
        };

        // Mark as Retiring in the DSL (blocks re-spawn with same ID).
        // Shell roster does not carry authoritative state; `member_state_markers`
        // in the DSL is the source of truth and overlays the read-only projection
        // on snapshot construction.
        //
        // The DSL guards reject Retire when the runtime_id is absent from
        // `live_runtime_ids` or the phase forbids it.
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        let detach_session_id = if preserve_realtime_binding {
            None
        } else {
            entry.member_ref.bridge_session_id().cloned()
        };
        let releasing = if preserve_realtime_binding {
            None
        } else {
            self.dsl_authority
                .state
                .member_session_bindings
                .get(&dsl_identity)
                .cloned()
        };
        let session_id_for_route = releasing
            .clone()
            .or_else(|| {
                self.dsl_authority
                    .state
                    .member_session_bindings
                    .get(&dsl_identity)
                    .cloned()
            })
            .unwrap_or_else(mob_dsl::SessionId::default);
        let retire_input = mob_dsl::MobMachineInput::Retire {
            mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
            agent_identity: dsl_identity,
            releasing,
            session_id: session_id_for_route,
        };

        // MobMachine admits the command before the event projection records
        // retirement. The actual transition is still applied after the durable
        // event append so crash recovery keeps its event-first ordering.
        self.preview_dsl_input(retire_input.clone(), "handle_retire_inner_admission")?;

        // Append retire event (event-first for crash recovery).
        let retire_event_already_present = self
            .retire_event_exists(agent_identity, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(agent_identity, &entry.role, &entry.member_ref)
                .await?;
        }

        let effects = self
            .apply_dsl_input_collect_effects(retire_input, "handle_retire_inner_mark_retiring")?;
        let mut detach_obligations =
            crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                &effects,
            );
        if let (Some(obligation), Some(session_id)) = (detach_obligations.pop(), detach_session_id)
        {
            self.detach_session_ingress_for_mob_destroy(&session_id, obligation)
                .await?;
        }

        // #31 Wave D (D-trust-reconciliation subsystem 4): flip the roster
        // entry's state to `Retiring` so live readers of
        // `list_members()` / `member_status()` observe the retiring state
        // during the disposal window (notify peers, archive session).
        // The canonical removal still runs in the finally block of
        // `dispose_member`. Without this explicit flip the
        // `RosterEntry.state` stayed `Active` right up to removal because
        // the `MemberRetired` event projection is "remove-by-identity",
        // not "mark-retiring" — there was a lost observability seam for
        // the in-flight-retire window.
        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring_by_identity(&entry.agent_identity);
        }

        // Flush routed effects *before* the disposal pipeline tears down
        // the runtime session. The `Retire` DSL input above emits a
        // `RequestRuntimeRetire` routed effect that the `MeerkatConsumer
        // Surface` delivers as a `Retire` input on the meerkat DSL
        // authority; that authority lives behind the session registered
        // with the shared `MeerkatMachine`. `dispose_member` →
        // `dispose_stop_host_loop` → `teardown_autonomous_runtime` calls
        // `adapter.unregister_session`, so if the routed effect hasn't
        // been drained yet, the consumer surface fails with
        // `ConsumerRefused { reason: "session is not registered" }` and
        // the actor task crashes. Drain the pending queue at the natural
        // async boundary here — before the disposal pipeline runs — so
        // the routed-effect delivery observes the session in its
        // pre-teardown state.
        if let Err(error) = self.flush_routed_effects().await {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                %error,
                "pre-disposal routed-effect flush failed; proceeding with disposal"
            );
        }

        // Snapshot context and run disposal pipeline.
        let ctx = self
            .disposal_context_from_entry(agent_identity, &entry)
            .await;
        let mut policy: Box<dyn ErrorPolicy> = if bulk {
            Box::new(BulkBestEffort)
        } else {
            Box::new(WarnAndContinue)
        };
        let report = self.dispose_member(&ctx, policy.as_mut()).await;

        // ArchiveSession is critical: a skipped archive means an orphan session
        // the caller believes was cleaned up. Surface the error.
        // Comms steps (NotifyPeers, RemoveTrustEdges) remain best-effort.
        if let Some((_, error)) = report
            .skipped
            .iter()
            .find(|(step, _)| *step == DisposalStep::ArchiveSession)
        {
            return Err(MobError::Internal(format!(
                "disposal completed but ArchiveSession failed: {error}"
            )));
        }
        if let Some((step, error)) = &report.aborted_at
            && *step == DisposalStep::ArchiveSession
        {
            return Err(MobError::Internal(format!(
                "disposal aborted at ArchiveSession: {error}"
            )));
        }

        self.delete_external_binding_overlay_for_member(&entry.agent_identity, entry.generation)
            .await?;

        Ok(())
    }

    /// Respawn a member: retire the old session and spawn a fresh one with the
    /// same identity, profile, wiring, and labels. The old session is archived;
    /// the new session gets a fresh session ID.
    ///
    /// This is helper composition over primitive mob behavior. No rollback is
    /// attempted after retire. Returns a receipt on full success, or a
    /// structured error on failure.
    async fn handle_respawn(
        &mut self,
        agent_identity: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<super::handle::MemberRespawnReceipt, super::handle::MobRespawnError> {
        use super::handle::{MemberRespawnReceipt, MobRespawnError};

        self.ensure_pending_spawn_alignment("handle_respawn preflight")
            .map_err(MobRespawnError::from)?;

        let canceled = self.cancel_pending_spawns_for_member(
            &agent_identity,
            "respawn command superseded pending spawn",
        );
        if canceled > 0 {
            tracing::info!(
                agent_identity = %agent_identity,
                canceled,
                "respawn canceled pending spawn lineage before replacement workflow"
            );
        }

        // 1. Snapshot all replacement inputs before retiring. Topology comes
        // from the MobMachine authority; the roster is only the read model
        // that supplies profile/runtime binding details for the old member and
        // filters stale local edges whose peers are no longer live.
        let snapshot = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&agent_identity)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?;
            let live_local_identities = roster
                .list()
                .map(|entry| MeerkatId::from(entry.agent_identity.as_str()))
                .collect::<HashSet<_>>();
            let restore_wiring = self
                .machine_restore_wiring_plan(&agent_identity, &live_local_identities)
                .map_err(MobRespawnError::from)?;
            let binding = match &entry.member_ref {
                crate::event::MemberRef::BackendPeer {
                    peer_id,
                    address,
                    bootstrap_token,
                    ..
                } => crate::RuntimeBinding::External {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    bootstrap_token: bootstrap_token.clone(),
                    pubkey: None,
                },
                crate::event::MemberRef::Session { .. } => crate::RuntimeBinding::Session,
            };
            RespawnSnapshot {
                profile_name: entry.role.clone(),
                runtime_mode: entry.runtime_mode,
                labels: entry.labels.clone(),
                old_runtime_id: entry.agent_runtime_id.clone(),
                old_fence_token: entry.fence_token,
                generation: entry.generation,
                restore_wiring,
                binding,
                effective_profile_override: entry.effective_profile_override,
            }
        };

        self.preview_dsl_input(
            mob_dsl::MobMachineInput::Respawn {
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&snapshot.old_runtime_id),
            },
            "handle_respawn_admission",
        )
        .map_err(|_| MobRespawnError::from(self.invalid_transition_to(MobState::Running)))?;

        let replacement_generation = snapshot.generation.next();

        // 2. Retire the existing member (archives the session, removes from roster).
        if let Err(error) = self.handle_retire_inner(&agent_identity, false, true).await {
            let roster_still_contains_member = {
                let roster = self.roster.read().await;
                roster.get(&agent_identity).is_some()
            };
            if roster_still_contains_member {
                return Err(MobRespawnError::from(error));
            }
            let mut cleanup_report = super::handle::PreviousMemberCleanupReport {
                identity: AgentIdentity::from(agent_identity.as_str()),
                agent_runtime_id: snapshot.old_runtime_id.clone(),
                fence_token: snapshot.old_fence_token,
                retire_attempted: true,
                retire_error: Some(error.to_string()),
                confirmatory_observation_attempted: false,
                confirmatory_observation: None,
                destroy_attempted: false,
                destroy_error: None,
            };

            match &snapshot.binding {
                crate::RuntimeBinding::External { .. } => {
                    cleanup_report.confirmatory_observation_attempted = true;
                    match self
                        .observe_peer_only_binding(
                            &snapshot.binding,
                            std::time::Duration::from_millis(750),
                        )
                        .await
                    {
                        Ok(observation) => {
                            cleanup_report.confirmatory_observation =
                                Some(format!("state={}", observation.state));
                            if !Self::observation_is_terminal(&observation) {
                                cleanup_report.destroy_attempted = true;
                                if let Err(destroy_error) = self
                                    .destroy_peer_only_binding(
                                        &snapshot.binding,
                                        std::time::Duration::from_secs(5),
                                    )
                                    .await
                                {
                                    cleanup_report.destroy_error = Some(destroy_error.to_string());
                                    return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                                        report: cleanup_report,
                                    });
                                }
                            }
                        }
                        Err(observe_error) => {
                            cleanup_report.confirmatory_observation =
                                Some(observe_error.to_string());
                            cleanup_report.destroy_attempted = true;
                            if let Err(destroy_error) = self
                                .destroy_peer_only_binding(
                                    &snapshot.binding,
                                    std::time::Duration::from_secs(5),
                                )
                                .await
                            {
                                cleanup_report.destroy_error = Some(destroy_error.to_string());
                                return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                                    report: cleanup_report,
                                });
                            }
                        }
                    }
                }
                crate::RuntimeBinding::Session => {
                    return Err(MobRespawnError::PreviousMemberCleanupAmbiguous {
                        report: cleanup_report,
                    });
                }
            }
        }

        // 3. Rebuild the replacement spawn preserving identity, profile, labels, mode, and peer intent.
        let prompt = initial_message.unwrap_or_else(|| {
            ContentInput::from(self.fallback_spawn_prompt(&snapshot.profile_name, &agent_identity))
        });
        // Prefer roster's effective_profile_override on respawn for lifecycle safety.
        let profile = if let Some(p) = snapshot.effective_profile_override.clone() {
            p
        } else {
            self.definition
                .resolve_profile(&snapshot.profile_name, self.realm_profile_store.as_ref())
                .await?
        };
        let external_tools = self.external_tools_for_profile(&profile)?;
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &snapshot.profile_name,
            agent_identity: &agent_identity,
            profile: &profile,
            definition: &self.definition,
            external_tools,
            context: None,
            labels: Some(snapshot.labels.clone()),
            additional_instructions: None,
            shell_env: None,
            mob_tool_access_context: crate::build::MobToolAccessContext::None,
            inherited_tool_filter: None,
        })
        .await?;
        config.keep_alive = snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }
        let req = build::to_create_session_request(&config, prompt.clone());
        let peer_name = format!(
            "{}/{}/{}",
            self.definition.id, snapshot.profile_name, agent_identity
        );
        let mut provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: snapshot.binding.clone(),
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
        };
        let admitted_bridge_session_id =
            admit_bridge_session_for_spawn(&mut provision_request.create_session);

        let respawn_spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        self.stage_orchestrator_spawn(&agent_identity, &admitted_bridge_session_id)
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: format!("failed to stage respawn replacement spawn: {error}"),
            })?;
        let (respawn_inline_reply_tx, _respawn_inline_reply_rx) = oneshot::channel();
        let respawn_pending = PendingSpawn {
            profile_name: snapshot.profile_name.clone(),
            agent_identity: agent_identity.clone(),
            admitted_bridge_session_id,
            prompt: prompt.clone(),
            runtime_mode: snapshot.runtime_mode,
            labels: snapshot.labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: (!snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty())
            .then_some(snapshot.restore_wiring.clone()),
            effective_profile_override: snapshot.effective_profile_override.clone(),
            progress: Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default())),
            reply_tx: respawn_inline_reply_tx,
        };
        let respawn_inline_task = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        self.insert_pending_spawn(respawn_spawn_ticket, respawn_pending, respawn_inline_task);
        if let Err(error) = self.ensure_pending_spawn_alignment("handle_respawn staged replacement")
        {
            tracing::error!(
                agent_identity = %agent_identity,
                error = %error,
                "pending spawn alignment violated while staging respawn replacement"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated while staging respawn replacement",
            )
            .await;
            return Err(MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            });
        }

        // 4. Provision and finalize the replacement member inline so the receipt reflects
        //    the committed canonical member/session state before we return.
        let replacement_result: Result<super::handle::MemberSpawnReceipt, MobRespawnError> = Box::pin(async {
            let spawn_receipt = self
                .provisioner
                .provision_member(provision_request)
                .await
                .map_err(|error| MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: error.to_string(),
                })?;
            if snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        &agent_identity,
                        &spawn_receipt.member_ref,
                    )
                    .await
            {
                if let Err(retire_error) = self
                    .provisioner
                    .retire_member(&spawn_receipt.member_ref)
                    .await
                {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        identity: AgentIdentity::from(agent_identity.as_str()),
                        reason: format!(
                            "autonomous capability check failed: {capability_error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: capability_error.to_string(),
                });
            }

            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                agent_identity.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        identity: AgentIdentity::from(agent_identity.as_str()),
                        reason: format!(
                            "mob state changed before respawn finalization: {error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(agent_identity.as_str()),
                    reason: error.to_string(),
                });
            }

            if !snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty()
            {
                tracing::info!(
                    agent_identity = %agent_identity,
                    local_peers = ?snapshot.restore_wiring.local_peers,
                    external_peers = ?snapshot.restore_wiring.external_peers,
                    "respawn: restoring peer wiring during replacement finalization"
                );
            }

            let respawn_fence = self.issue_fence_token();
            let finalized = self
                .finalize_spawn_from_pending(
                &snapshot.profile_name,
                &agent_identity,
                replacement_generation,
                respawn_fence,
                snapshot.runtime_mode,
                prompt,
                snapshot.labels.clone(),
                provision,
                spawn_receipt.operation_id,
                None,
                false,
                (!snapshot.restore_wiring.local_peers.is_empty()
                    || !snapshot.restore_wiring.external_peers.is_empty())
                .then_some(snapshot.restore_wiring.clone()),
                snapshot.effective_profile_override.clone(),
            )
            .await
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            })?;

            if finalized.failed_restore_peer_ids.is_empty() {
                Ok(finalized.receipt)
            } else {
                Err(MobRespawnError::TopologyRestoreFailed {
                    receipt: super::handle::MemberRespawnReceipt::new(
                        AgentIdentity::from(agent_identity.as_str()),
                        crate::ids::AgentRuntimeId::new(
                            AgentIdentity::from(agent_identity.as_str()),
                            replacement_generation,
                        ),
                        snapshot.old_fence_token,
                        respawn_fence,
                    ),
                    failed_peer_ids: finalized.failed_restore_peer_ids.into_iter().map(|mid| AgentIdentity::from(mid.as_str())).collect(),
                })
            }
        })
        .await;

        let (_respawn_pending, respawn_task) =
            self.complete_pending_spawn_slot(respawn_spawn_ticket, "respawn replacement spawn");
        if let Some(handle) = respawn_task {
            handle.abort();
        }
        self.ensure_pending_spawn_alignment("handle_respawn completion")
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(agent_identity.as_str()),
                reason: error.to_string(),
            })?;
        let _replacement = replacement_result?;

        // 5. Build the receipt from the committed replacement member reference.
        Ok(MemberRespawnReceipt::new(
            AgentIdentity::from(agent_identity.as_str()),
            crate::ids::AgentRuntimeId::new(
                AgentIdentity::from(agent_identity.as_str()),
                replacement_generation,
            ),
            snapshot.old_fence_token,
            self.roster
                .read()
                .await
                .get(&agent_identity)
                .map(|entry| entry.fence_token)
                .unwrap_or(snapshot.old_fence_token),
        ))
    }

    // -----------------------------------------------------------------------
    // Disposal pipeline
    // -----------------------------------------------------------------------

    /// Snapshot member state for disposal from a roster entry.
    async fn disposal_context_from_entry(
        &self,
        agent_identity: &MeerkatId,
        entry: &RosterEntry,
    ) -> DisposalContext {
        let retiring_key = self
            .provisioner_comms(&entry.member_ref)
            .await
            .and_then(|comms| comms.public_key());
        DisposalContext {
            agent_identity: agent_identity.clone(),
            entry: entry.clone(),
            retiring_key,
        }
    }

    /// Execute the disposal pipeline for a member.
    ///
    /// Runs policy-driven steps in order, then unconditionally removes the
    /// member from the roster and prunes wire edge locks. The finally block
    /// runs regardless of whether the policy aborted.
    async fn dispose_member(
        &mut self,
        ctx: &DisposalContext,
        policy: &mut dyn ErrorPolicy,
    ) -> DisposalReport {
        let mut report = DisposalReport::new();

        for &step in &DisposalStep::ORDERED {
            match self.execute_step(step, ctx).await {
                Ok(()) => report.completed.push(step),
                Err(error) => {
                    if policy.on_step_error(step, &error, ctx) {
                        report.skipped.push((step, error));
                    } else {
                        report.aborted_at = Some((step, error));
                        break;
                    }
                }
            }
        }

        // Finally: unconditional, outside policy control.
        self.dispose_prune_edge_locks(ctx).await;
        self.dispose_remove_from_roster(ctx).await;
        report.roster_removed = true;
        report
    }

    /// Dispatch a disposal step. Exhaustive match ensures compiler forces new
    /// arms when `DisposalStep` variants are added.
    async fn execute_step(
        &mut self,
        step: DisposalStep,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        match step {
            DisposalStep::StopHostLoop => self.dispose_stop_host_loop(ctx).await,
            DisposalStep::NotifyPeers => self.dispose_notify_peers(ctx).await,
            DisposalStep::ArchiveSession => self.dispose_archive_session(ctx).await,
        }
    }

    /// Stop the autonomous member and unregister session (disposal only).
    async fn dispose_stop_host_loop(&mut self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_member(&ctx.agent_identity, &ctx.entry.member_ref)
                .await?;
            // Full teardown: unregister from MeerkatMachine.
            // stop_autonomous_member only aborts the drain; disposal also
            // needs to release the session registration.
            self.teardown_autonomous_runtime(&ctx.entry.member_ref)
                .await;
        }
        Ok(())
    }

    /// Notify all wired peers that this member is retiring.
    ///
    /// Iterates the full `wired_to` set internally; skips absent peers.
    /// Returns the first error encountered, if any.
    async fn dispose_notify_peers(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        let Some(retiring_comms) = self.sender_runtime_for_entry(&ctx.entry).await else {
            return Ok(());
        };
        let retiring_spec = match self
            .resolve_wiring_endpoint(&ctx.entry, "dispose_notify_peers retiring member")
            .await?
        {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        let mut first_error: Option<MobError> = None;
        for peer_identity in &ctx.entry.wired_to {
            // Resolve identity to bridge MeerkatId; skip absent peers (already retired).
            let peer_entry = {
                let roster = self.roster.read().await;
                roster.get_by_identity(peer_identity).cloned()
            };
            let Some(peer_entry) = peer_entry else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    agent_identity = %ctx.agent_identity,
                    peer_id = %peer_identity,
                    "dispose_notify_peers: skipping absent peer"
                );
                continue;
            };
            let (recipient_spec, recipient_comms, recipient_binding) = match self
                .resolve_wiring_endpoint(&peer_entry, "dispose_notify_peers")
                .await?
            {
                WiringEndpoint::Local { comms, spec, .. } => (spec, Some(comms), None),
                WiringEndpoint::PeerOnly { spec, binding } => (spec, None, Some(binding)),
            };

            if let Err(error) = self
                .notify_peer_retired(
                    &recipient_spec,
                    &ctx.agent_identity,
                    &ctx.entry,
                    &retiring_comms,
                )
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            if let Some(recipient_comms) = recipient_comms {
                if let Err(error) = recipient_comms
                    .remove_trusted_peer(&Self::trusted_peer_removal_key(&retiring_spec))
                    .await
                    && first_error.is_none()
                {
                    first_error = Some(MobError::from(error));
                }
            } else if let Some(recipient_binding) = recipient_binding
                && let Err(error) = self
                    .unwire_peer_only_recipient(
                        &recipient_spec,
                        Some(&recipient_binding),
                        &retiring_spec,
                        std::time::Duration::from_secs(10),
                    )
                    .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Archive the member's session. Treats NotFound as success.
    pub(super) async fn dispose_archive_session(
        &self,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        if let Err(error) = self.provisioner.retire_member(&ctx.entry.member_ref).await {
            if matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            ) {
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    /// Prune edge locks for the member. Infallible.
    async fn dispose_prune_edge_locks(&self, ctx: &DisposalContext) {
        self.edge_locks.prune(ctx.agent_identity.as_str()).await;
    }

    /// Remove the member from the roster. Infallible.
    pub(super) async fn dispose_remove_from_roster(&self, ctx: &DisposalContext) {
        let mut roster = self.roster.write().await;
        roster.remove_member(&ctx.agent_identity);
        drop(roster);
        self.restore_diagnostics
            .write()
            .await
            .remove(&ctx.agent_identity);
    }

    async fn handle_complete(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_complete preflight")?;
        self.ensure_flow_tracker_alignment("handle_complete preflight")
            .await?;
        self.cancel_all_flow_tasks().await?;

        self.notify_orchestrator_lifecycle(format!("Mob '{}' is completing.", self.definition.id))
            .await;
        self.retire_all_members("complete").await?;
        self.stop_mcp_servers().await?;

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await?;
        // Apply Complete input to set active_run_count = 0 and transition to Completed phase
        self.apply_dsl_input(mob_dsl::MobMachineInput::Complete, "complete_input")
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle Complete transition failed during complete: {error}"
                ))
            })?;
        self.ensure_pending_spawn_alignment("handle_complete completion")?;
        self.ensure_flow_tracker_alignment("handle_complete completion")
            .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn remote_destroy_cleanup_deadline(remote_member_count: usize) -> std::time::Duration {
        let batches = remote_member_count
            .saturating_add(MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS.saturating_sub(1))
            / MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS.max(1);
        let deadline_secs = std::cmp::min(90, 5 + (batches as u64) * 17);
        std::time::Duration::from_secs(deadline_secs)
    }

    fn push_unique_identity(target: &mut Vec<AgentIdentity>, identity: AgentIdentity) {
        if !target.iter().any(|existing| existing == &identity) {
            target.push(identity);
        }
    }

    fn runtime_binding_for_entry(entry: &RosterEntry) -> Option<crate::RuntimeBinding> {
        match &entry.member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: None,
            } => Some(crate::RuntimeBinding::External {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                bootstrap_token: bootstrap_token.clone(),
                pubkey: None,
            }),
            _ => None,
        }
    }

    fn sanitized_member_ref(member_ref: &MemberRef) -> MemberRef {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id,
                ..
            } => MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                bootstrap_token: bootstrap_token.clone(),
                session_id: session_id.clone(),
            },
            MemberRef::Session { session_id } => MemberRef::Session {
                session_id: session_id.clone(),
            },
        }
    }

    fn external_binding_overlay_record(
        &self,
        agent_identity: &AgentIdentity,
        generation: crate::ids::Generation,
        member_ref: &MemberRef,
    ) -> Option<crate::store::ExternalBindingOverlayRecord> {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                bootstrap_token,
                session_id: None,
            } => Some(crate::store::ExternalBindingOverlayRecord {
                agent_identity: agent_identity.clone(),
                generation,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: super::bridge_protocol::canonicalize_bridge_address(address),
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: bootstrap_token.clone(),
                status: crate::store::ExternalBindingOverlayStatus::Normalized,
                updated_at: chrono::Utc::now(),
            }),
            _ => None,
        }
    }

    async fn delete_external_binding_overlay_for_member(
        &self,
        agent_identity: &AgentIdentity,
        generation: crate::ids::Generation,
    ) -> Result<(), MobError> {
        self.runtime_metadata
            .delete_external_binding_overlay(&self.definition.id, agent_identity, generation)
            .await
            .map_err(MobError::from)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn destroy_remote_member_for_destroy(
        &mut self,
        entry: RosterEntry,
    ) -> RemoteDestroyOutcome {
        let identity = entry.agent_identity.clone();
        let agent_identity = entry.agent_identity.clone();
        let mut outcome = RemoteDestroyOutcome {
            identity: identity.clone(),
            force_destroyed: false,
            orphaned: false,
            errors: Vec::new(),
        };
        let Some(binding) = Self::runtime_binding_for_entry(&entry) else {
            outcome
                .errors
                .push("remote destroy requested for non peer-only member".to_string());
            outcome.orphaned = true;
            return outcome;
        };

        let ctx = self
            .disposal_context_from_entry(&agent_identity, &entry)
            .await;
        let mut policy = BulkBestEffort;
        let disposal = self.dispose_member(&ctx, &mut policy).await;

        let archive_error = disposal
            .skipped
            .iter()
            .find(|(step, _)| *step == DisposalStep::ArchiveSession)
            .map(|(_, error)| error.to_string())
            .or_else(|| {
                disposal.aborted_at.as_ref().and_then(|(step, error)| {
                    (*step == DisposalStep::ArchiveSession).then(|| error.to_string())
                })
            });

        let mut remote_cleanup_complete = archive_error.is_none();
        if let Some(error) = archive_error {
            outcome
                .errors
                .push(format!("graceful retire failed: {error}"));

            match self
                .observe_peer_only_binding(&binding, std::time::Duration::from_millis(750))
                .await
            {
                Ok(observation) => {
                    if Self::observation_is_terminal(&observation) {
                        remote_cleanup_complete = true;
                    } else {
                        outcome.errors.push(format!(
                            "confirmatory observation reported non-terminal state {}",
                            observation.state
                        ));
                    }
                }
                Err(error) => outcome
                    .errors
                    .push(format!("confirmatory observation failed: {error}")),
            }

            if !remote_cleanup_complete {
                match self
                    .destroy_peer_only_binding(&binding, std::time::Duration::from_secs(5))
                    .await
                {
                    Ok(_) => {
                        remote_cleanup_complete = true;
                        outcome.force_destroyed = true;
                    }
                    Err(error) => outcome
                        .errors
                        .push(format!("force destroy failed: {error}")),
                }
            }
        }

        let mut revoke_failed = false;
        if let Err(error) = self
            .revoke_supervisor_for_binding(&binding, std::time::Duration::from_secs(5))
            .await
        {
            let error_text = error.to_string().to_ascii_lowercase();
            let expected_after_destroy = remote_cleanup_complete
                && (error_text.contains("peer not found")
                    || error_text.contains("peer offline")
                    || error_text.contains("no authorized supervisor registered"));
            if expected_after_destroy {
                let peer_id = match &binding {
                    crate::RuntimeBinding::External { peer_id, .. } => peer_id.as_str(),
                    crate::RuntimeBinding::Session => "session",
                };
                tracing::debug!(
                    peer_id = %peer_id,
                    error = %error,
                    "destroy cleanup: supervisor revoke failed after terminal remote cleanup"
                );
            } else {
                revoke_failed = true;
                outcome
                    .errors
                    .push(format!("supervisor revoke failed: {error}"));
            }
        }

        outcome.orphaned = !remote_cleanup_complete || revoke_failed;
        outcome
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn destroy_remote_members_for_destroy(
        &mut self,
        remote_entries: Vec<RosterEntry>,
        report: &mut super::handle::MobDestroyReport,
    ) {
        if remote_entries.is_empty() {
            return;
        }

        // Phase 2 sequential expedient: dispose_member currently takes
        // `&mut self` (via `dispose_stop_host_loop` / `stop_autonomous_member`).
        // Phase 4 will re-introduce FuturesUnordered parallelism once the
        // disposal steps are converted to `&self` so a shared actor reference
        // can be held across concurrent disposal futures.
        let deadline = Self::remote_destroy_cleanup_deadline(remote_entries.len());
        let deadline_at = tokio::time::Instant::now() + deadline;
        let mut remaining = VecDeque::from(remote_entries);

        while let Some(entry) = remaining.pop_front() {
            let identity = entry.agent_identity.clone();
            let next =
                tokio::time::timeout_at(deadline_at, self.destroy_remote_member_for_destroy(entry))
                    .await;
            let outcome = match next {
                Ok(outcome) => outcome,
                Err(_) => {
                    report.remote_cleanup_deadline_exceeded = true;
                    Self::push_unique_identity(&mut report.orphaned_remote_members, identity);
                    for entry in remaining {
                        Self::push_unique_identity(
                            &mut report.orphaned_remote_members,
                            entry.agent_identity.clone(),
                        );
                    }
                    return;
                }
            };

            if outcome.force_destroyed {
                Self::push_unique_identity(
                    &mut report.force_destroyed_members,
                    outcome.identity.clone(),
                );
            }
            if outcome.orphaned {
                Self::push_unique_identity(
                    &mut report.orphaned_remote_members,
                    outcome.identity.clone(),
                );
            }
            for error in outcome.errors {
                report.push_error(format!("{}: {error}", outcome.identity));
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn destroy_remote_members_for_destroy(
        &self,
        remote_entries: Vec<RosterEntry>,
        report: &mut super::handle::MobDestroyReport,
    ) {
        for entry in remote_entries {
            Self::push_unique_identity(
                &mut report.orphaned_remote_members,
                entry.agent_identity.clone(),
            );
        }
    }

    async fn dispose_local_member_after_destroy_admission(
        &mut self,
        entry: RosterEntry,
        report: &mut super::handle::MobDestroyReport,
    ) -> Result<(), super::handle::MobDestroyError> {
        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring_by_identity(&entry.agent_identity);
        }
        let ctx = self
            .disposal_context_from_entry(&entry.agent_identity, &entry)
            .await;
        let mut policy = BulkBestEffort;
        let disposal_report = self.dispose_member(&ctx, &mut policy).await;
        if let Some((_, error)) = disposal_report
            .skipped
            .iter()
            .find(|(step, _)| *step == DisposalStep::ArchiveSession)
        {
            report.push_error(format!(
                "{}: disposal completed but ArchiveSession failed: {error}",
                entry.agent_identity
            ));
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        if let Some((step, error)) = &disposal_report.aborted_at
            && *step == DisposalStep::ArchiveSession
        {
            report.push_error(format!(
                "{}: disposal aborted at ArchiveSession: {error}",
                entry.agent_identity
            ));
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        if let Err(error) = self
            .delete_external_binding_overlay_for_member(&entry.agent_identity, entry.generation)
            .await
        {
            report.push_error(error.to_string());
            return Err(super::handle::MobDestroyError::Incomplete {
                report: report.clone(),
            });
        }
        Ok(())
    }

    async fn handle_destroy(
        &mut self,
    ) -> Result<super::handle::MobDestroyReport, super::handle::MobDestroyError> {
        use super::handle::{MobDestroyError, MobDestroyReport};

        let mut report = MobDestroyReport::default();

        self.ensure_pending_spawn_alignment("handle_destroy preflight")
            .map_err(MobDestroyError::from)?;
        self.ensure_flow_tracker_alignment("handle_destroy preflight")
            .await
            .map_err(MobDestroyError::from)?;
        self.fail_all_pending_spawns("mob is destroying").await;
        let entries = {
            let roster = self.roster.read().await;
            roster.list_all().cloned().collect::<Vec<_>>()
        };
        if self.has_orchestrator {
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::StopOrchestrator,
                "stop_orchestrator_destroy",
            )
            .map_err(MobDestroyError::from)?;
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::DestroyOrchestrator,
                "destroy_orchestrator",
            )
            .map_err(MobDestroyError::from)?;
        }
        for entry in &entries {
            if let Err(error) = self.admit_member_retire_for_destroy(entry).await {
                report.push_error(format!(
                    "{}: destroy retire admission failed: {error}",
                    entry.agent_identity
                ));
                return Err(MobDestroyError::Incomplete { report });
            }
        }
        self.cancel_all_flow_tasks()
            .await
            .map_err(MobDestroyError::from)?;
        self.apply_dsl_input(mob_dsl::MobMachineInput::Destroy, "destroy_input")
            .map_err(MobDestroyError::from)?;
        self.flush_routed_effects()
            .await
            .map_err(MobDestroyError::from)?;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is destroying.", self.definition.id))
            .await;
        let (remote_entries, local_entries): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|entry| Self::runtime_binding_for_entry(entry).is_some());

        for entry in local_entries {
            self.dispose_local_member_after_destroy_admission(entry, &mut report)
                .await?;
        }
        self.destroy_remote_members_for_destroy(remote_entries, &mut report)
            .await;
        if let Err(error) = self.stop_mcp_servers().await {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        if let Err(error) = self
            .runtime_metadata
            .delete_external_binding_overlays(&self.definition.id)
            .await
        {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        if let Err(error) = self
            .runtime_metadata
            .delete_supervisor_authority(&self.definition.id)
            .await
        {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        report.metadata_scrubbed = true;
        if let Err(error) = self.events.clear().await {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        report.events_cleared = true;
        if let Err(error) = self.cleanup_namespace().await {
            report.push_error(error.to_string());
            return Err(MobDestroyError::Incomplete { report });
        }
        report.namespace_cleaned = true;
        self.edge_locks.clear().await;
        self.ensure_pending_spawn_alignment("handle_destroy completion")
            .map_err(|error| {
                let mut report = report.clone();
                report.push_error(error.to_string());
                MobDestroyError::Incomplete { report }
            })?;
        self.ensure_flow_tracker_alignment("handle_destroy completion")
            .await
            .map_err(|error| {
                let mut report = report.clone();
                report.push_error(error.to_string());
                MobDestroyError::Incomplete { report }
            })?;
        if report.remote_cleanup_deadline_exceeded
            || !report.orphaned_remote_members.is_empty()
            || !report.errors.is_empty()
        {
            return Err(MobDestroyError::Incomplete { report });
        }
        Ok(report)
    }

    async fn handle_rotate_supervisor(
        &self,
    ) -> Result<super::handle::SupervisorRotationReport, MobError> {
        let current = self
            .runtime_metadata
            .load_supervisor_authority(&self.definition.id)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "cannot rotate supervisor for mob '{}': missing supervisor runtime metadata",
                    self.definition.id
                ))
            })?;
        let mut next = crate::store::SupervisorAuthorityRecord::generate(current.protocol_version);
        next.epoch = current.epoch + 1;
        let remote_bindings = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .filter_map(Self::runtime_binding_for_entry)
                .collect::<Vec<_>>()
        };
        if !remote_bindings.is_empty() {
            let next_public_key = next.keypair().public_key();
            let next_sup_spec: super::bridge_protocol::BridgePeerSpec =
                meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
                    format!("{}/__mob_supervisor__", self.definition.id),
                    next.public_peer_id.clone(),
                    *next_public_key.as_bytes(),
                    format!("inproc://{}/__mob_supervisor__", self.definition.id),
                )
                .map_err(|error| {
                    MobError::WiringError(format!("invalid rotated supervisor spec: {error}"))
                })?
                .into();
            let next_payload = super::bridge_protocol::BridgeSupervisorPayload {
                supervisor: next_sup_spec,
                epoch: next.epoch,
                protocol_version: next.protocol_version,
            };
            let next_command =
                super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(next_payload.clone());
            let mut rotated_peers: Vec<(TrustedPeerDescriptor, crate::RuntimeBinding)> = Vec::new();
            for binding in remote_bindings {
                let peer = Self::peer_only_spec_for_binding(&binding, "handle_rotate_supervisor")?;
                let mut effective_peer = peer.clone();
                let mut effective_binding = binding.clone();
                self.supervisor_bridge.trust_recipient(&peer).await?;
                let authorize_result = self
                    .supervisor_bridge
                    .send_bridge_command(&peer, &next_command, std::time::Duration::from_secs(5))
                    .await;
                let authorize_error = match authorize_result {
                    Ok(value) => {
                        if let Some(rejection) =
                            Self::bridge_rejection_reply(next_payload.protocol_version, &value)
                        {
                            let rejection_reason = rejection.reason().to_string();
                            if let Some(cause) = rejection.typed_cause()
                                && super::bridge_fallback::should_fall_back_to_bind(cause)
                            {
                                let bind = self
                                    .bind_peer_only_member_for_binding_with_payload(
                                        &peer,
                                        &binding,
                                        &next_payload,
                                    )
                                    .await;
                                match bind {
                                    Ok(bind_response) => {
                                        let effective_bootstrap_token =
                                            Self::bridge_bootstrap_token_from_binding(&binding)?;
                                        self.persist_rebound_binding(&binding, &bind_response)
                                            .await?;
                                        effective_binding = crate::RuntimeBinding::External {
                                            peer_id: bind_response.peer_id.clone(),
                                            address:
                                                super::bridge_protocol::canonicalize_bridge_address(
                                                    &bind_response.address,
                                                ),
                                            bootstrap_token: Some(effective_bootstrap_token),
                                            pubkey: match &binding {
                                                crate::RuntimeBinding::External {
                                                    pubkey, ..
                                                } => *pubkey,
                                                crate::RuntimeBinding::Session => None,
                                            },
                                        };
                                        effective_peer = Self::peer_only_spec_for_binding(
                                            &effective_binding,
                                            "handle_rotate_supervisor rebound peer",
                                        )?;
                                        None
                                    }
                                    Err(bind_error) => Some(MobError::WiringError(format!(
                                        "{rejection_reason}; bind fallback failed: {bind_error}"
                                    ))),
                                }
                            } else {
                                Some(MobError::WiringError(rejection_reason))
                            }
                        } else if let Err(error) =
                            serde_json::from_value::<super::bridge_protocol::BridgeAck>(value)
                        {
                            Some(MobError::Internal(format!(
                                "failed to decode rotate supervisor response: {error}"
                            )))
                        } else {
                            None
                        }
                    }
                    Err(error) => Some(error),
                };
                if let Some(error) = authorize_error {
                    if !rotated_peers.is_empty() {
                        match self
                            .rollback_rotated_supervisor_peers(&current, &rotated_peers)
                            .await
                        {
                            Ok(()) => {
                                return Err(MobError::WiringError(format!(
                                    "failed to rotate supervisor authority: {error}; rolled back {} remote peer(s)",
                                    rotated_peers.len()
                                )));
                            }
                            Err(rollback_error) => {
                                self.activate_supervisor_authority(&current, &next).await?;
                                return Err(MobError::WiringError(format!(
                                    "failed to rotate supervisor authority after partially applying next authority: {error}; rollback failed: {rollback_error}; local supervisor authority advanced to epoch {}",
                                    next.epoch
                                )));
                            }
                        }
                    }
                    return Err(MobError::WiringError(format!(
                        "failed to rotate supervisor authority: {error}"
                    )));
                }
                rotated_peers.push((effective_peer, effective_binding));
            }
        }
        let public_peer_id = next.public_peer_id.clone();
        self.activate_supervisor_authority(&current, &next).await?;
        Ok(super::handle::SupervisorRotationReport {
            previous_epoch: current.epoch,
            current_epoch: next.epoch,
            public_peer_id,
        })
    }

    async fn rollback_rotated_supervisor_peers(
        &self,
        current: &crate::store::SupervisorAuthorityRecord,
        rotated_peers: &[(TrustedPeerDescriptor, crate::RuntimeBinding)],
    ) -> Result<(), MobError> {
        let current_sup_spec: super::bridge_protocol::BridgePeerSpec =
            Self::supervisor_spec_for_authority(&self.definition.id, current)?.into();
        let current_payload = super::bridge_protocol::BridgeSupervisorPayload {
            supervisor: current_sup_spec,
            epoch: current.epoch,
            protocol_version: current.protocol_version,
        };
        for (peer, binding) in rotated_peers {
            let bind = self
                .bind_peer_only_member_for_binding_with_payload(peer, binding, &current_payload)
                .await
                .map_err(|bind_error| {
                    MobError::WiringError(format!(
                        "failed to roll peer back to prior supervisor authority: {bind_error}"
                    ))
                })?;
            self.persist_rebound_binding(binding, &bind).await?;
        }
        Ok(())
    }

    async fn activate_supervisor_authority(
        &self,
        current: &crate::store::SupervisorAuthorityRecord,
        next: &crate::store::SupervisorAuthorityRecord,
    ) -> Result<(), MobError> {
        self.runtime_metadata
            .put_supervisor_authority(&self.definition.id, next)
            .await?;
        self.supervisor_bridge.rotate(next.clone()).await?;
        let previous_private_trust_removal_key = current.public_peer_id.clone();
        let session_member_refs = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .filter_map(|entry| match &entry.member_ref {
                    MemberRef::Session { .. } => Some(entry.member_ref.clone()),
                    MemberRef::BackendPeer { .. } => None,
                })
                .collect::<Vec<_>>()
        };
        for member_ref in session_member_refs {
            if let (Some(session_id), Some(comms)) = (
                member_ref.bridge_session_id().cloned(),
                self.provisioner_comms(&member_ref).await,
            ) {
                self.install_supervisor_private_trust_for_session(
                    &session_id,
                    &comms,
                    Some(&previous_private_trust_removal_key),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Cancel checkpointers and transition to Stopped. Used by `handle_reset`
    /// error paths after destructive steps have already been taken.
    async fn fail_reset_to_stopped(&mut self) {
        self.provisioner.cancel_all_checkpointers().await;
        if let Err(e) =
            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "fail_reset_to_stopped")
        {
            tracing::warn!(error = %e, "authority rejected Stop in fail_reset_to_stopped");
        }
    }

    async fn handle_reset(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_reset preflight")?;
        self.ensure_flow_tracker_alignment("handle_reset preflight")
            .await?;
        let prior_state = self.state();
        let was_stopped = prior_state == MobState::Stopped;
        self.cancel_all_flow_tasks().await?;

        // Rearm checkpointers temporarily so retire can checkpoint if needed.
        if was_stopped {
            self.provisioner.rearm_all_checkpointers().await;
        }

        // --- Destructive phase: retire members and stop MCP servers. ---
        // After this point the mob is effectively stopped regardless of what
        // the prior state field says.
        if let Err(error) = self.retire_all_members("reset").await {
            if was_stopped {
                self.provisioner.cancel_all_checkpointers().await;
            }
            return Err(error);
        }
        if let Err(error) = self.stop_mcp_servers().await {
            // Members already retired -- fail-closed to Stopped.
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        // --- Event rewrite phase: append new epoch markers. ---
        // Append-only epoch model: MobCreated (for resume) + MobReset (epoch
        // marker). Projections (roster, task board) clear on MobReset; resume
        // uses the last MobCreated. No clear() needed -- crash-safe.
        // Batch append ensures both events land atomically.
        let mob_id = self.definition.id.clone();
        if let Err(error) = self
            .events
            .append_batch(vec![
                NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MobCreated {
                        definition: Box::new(self.definition.as_ref().clone()),
                    },
                },
                NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind: MobEventKind::MobReset,
                },
            ])
            .await
        {
            self.fail_reset_to_stopped().await;
            return Err(MobError::from(error));
        }

        // Clear in-memory projections. Don't call cleanup_namespace() — it
        // wipes mcp_servers keys which start_mcp_servers needs to track state.
        // stop_mcp_servers already cleared processes and set running=false.
        self.edge_locks.clear().await;
        self.retired_event_index.write().await.clear();
        self.task_board_service.clear().await;

        // --- Restart phase: bring MCP servers back up. ---
        if let Err(error) = self.start_mcp_servers().await {
            if let Err(stop_error) = self.stop_mcp_servers().await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    error = %stop_error,
                    "reset cleanup failed while stopping mcp servers"
                );
            }
            self.fail_reset_to_stopped().await;
            return Err(error);
        }

        // The command handler already probes Reset before the destructive phase.
        // Once side effects and epoch events have succeeded, realize that same
        // typed transition as the single authority-owned lifecycle landing. The
        // ResetToRunning transition owns active/pending run counters and
        // coordinator binding, so reset does not need shell Stop/Resume or
        // orchestrator stop/resume choreography.
        self.apply_dsl_input(mob_dsl::MobMachineInput::Reset, "reset_to_running")
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle Reset transition failed during reset: {error}"
                ))
            })?;
        self.ensure_pending_spawn_alignment("handle_reset completion")?;
        self.ensure_flow_tracker_alignment("handle_reset completion")
            .await?;
        Ok(())
    }

    /// Retire all roster members in parallel (sliding window of
    /// `MAX_PARALLEL_REMOTE_MEMBER_TEARDOWNS`). handle_retire only returns Err on
    /// event-append failures (pre-cleanup); cleanup errors are best-effort.
    /// If any member fails to retire the operation is aborted — the caller
    /// can retry since already-retired members are idempotent.
    async fn retire_all_members(&mut self, context: &str) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("retire_all_members preflight")?;
        let pending_reason = format!("{context}: draining pending spawns before bulk retirement");
        self.fail_all_pending_spawns(&pending_reason).await;
        self.ensure_pending_spawn_alignment("retire_all_members after pending drain")?;

        let ids = {
            let roster = self.roster.read().await;
            roster
                .list_all()
                .map(|entry| entry.agent_identity.clone())
                .collect::<Vec<_>>()
        };
        if ids.is_empty() {
            return Ok(());
        }

        let mut retire_failures: Vec<String> = Vec::new();
        for id in ids {
            let result = self.retire_one(id).await;
            if let Err((id, error)) = result {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %id,
                    error = %error,
                    "{context}: retire failed for member"
                );
                retire_failures.push(format!("{id}: {error}"));
            }
        }

        if !retire_failures.is_empty() {
            return Err(MobError::Internal(format!(
                "{context} aborted: {} member(s) could not be retired: {}",
                retire_failures.len(),
                retire_failures.join("; ")
            )));
        }
        self.ensure_pending_spawn_alignment("retire_all_members completion")?;
        Ok(())
    }

    async fn retire_one(&mut self, id: MeerkatId) -> Result<(), (MeerkatId, MobError)> {
        self.handle_retire_inner(&id, true, false)
            .await
            .map_err(|error| (id, error))
    }

    async fn handle_task_create(
        &mut self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        if subject.trim().is_empty() {
            return Err(MobError::Internal(
                "task subject cannot be empty".to_string(),
            ));
        }
        let task_id = TaskId::from(uuid::Uuid::new_v4().to_string());
        let dsl_input = mob_dsl::MobMachineInput::TaskCreate {
            task_id: mob_dsl::TaskId::from(task_id.as_str()),
            task_payload: Self::task_payload_to_dsl(&subject, &description, &blocked_by),
        };
        // MobMachine is the admission authority. Allocate the id in the
        // actor, fail closed through the DSL, then append/project the
        // shell task-board event using the same id. This prevents the
        // prior event-sourced-board/DSL split-brain where DSL rejection
        // was only logged after the shell write.
        self.apply_dsl_input(dsl_input, "handle_task_create")
            .map_err(|error| {
                MobError::Internal(format!(
                    "task create rejected by MobMachine guards before task-board write: {error}"
                ))
            })?;
        self.task_board_service
            .create_task_with_id(task_id.clone(), subject, description, blocked_by)
            .await?;
        Ok(task_id)
    }

    async fn handle_task_update(
        &mut self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        // TLA+ TaskBindingInvariant: owner must be a known identity (roster member).
        if let Some(ref owner_id) = owner {
            let roster = self.roster.read().await;
            let agent_identity = MeerkatId::from(owner_id.as_str());
            if roster.get(&agent_identity).is_none() {
                return Err(MobError::Internal(format!(
                    "TaskBindingInvariant violated: task owner '{owner_id}' is not in the roster",
                )));
            }
        }
        // Gate the status transition through the DSL guard set before
        // applying the shell-side event-sourced update. Rolling back from
        // a terminal status (Completed / Cancelled) is rejected here
        // rather than by the event projection.
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::TaskUpdate {
                task_id: mob_dsl::TaskId::from(task_id.as_str()),
                new_status: Self::task_status_to_dsl(status),
            },
            "handle_task_update",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "task update rejected by MobMachine guards: {error}"
            ))
        })?;
        self.task_board_service
            .update_task(task_id, status, owner)
            .await
    }

    /// Unified work-lane entry.
    ///
    /// The `MobMachine` DSL owns work-origin legality: whether this runtime is
    /// live, which origin is admissible (External vs Internal), and whether
    /// external callers may address this runtime. The shell no longer
    /// re-decides any of those facts — it forwards the caller-declared
    /// [`WorkOrigin`] to the DSL and lets the guards accept or reject.
    ///
    /// Shell-owned pre-work (shell is the only place that can do these):
    ///   * Fence-token freshness (concurrency invariant, not legality).
    ///   * Auto-spawn via the roster's [`SpawnPolicy`] when the target member
    ///     is absent but a policy resolves a spec. Only meaningful for
    ///     externally-originated work — internal origins never auto-spawn.
    ///   * `ensure_member_not_broken` / `MemberState::Active` filtering so
    ///     broken or retiring members return typed [`MobError::MemberNotFound`].
    ///   * Post-authorization dispatch — reading the machine's
    ///     `RequestRuntimeIngress` effect and materializing it as an actual
    ///     runtime ingress (event injector or `StartTurnRequest`). This is
    ///     the shell's realization of the DSL's routed-to-MeerkatMachine
    ///     effect.
    async fn handle_submit_work(
        &mut self,
        payload: Box<super::state::SubmitWorkPayload>,
    ) -> Result<(), MobError> {
        let super::state::SubmitWorkPayload {
            runtime_id,
            fence_token,
            work_ref,
            content,
            origin,
            handling_mode,
            render_metadata,
        } = *payload;
        self.ensure_pending_spawn_alignment("handle_submit_work preflight")?;

        let agent_identity = MeerkatId::from(&runtime_id.identity);

        // SubmitWork admission belongs to MobMachine even when the shell may
        // need to auto-spawn an absent external target. Probe the declared
        // command before policy resolution so stopped/completed mobs reject
        // without staging spawn side effects.
        if let Err(error) = self.probe_mob_machine_input(
            mob_dsl::MobMachineInput::SubmitWork {
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&runtime_id),
                fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                work_id: mob_dsl::WorkId::from_work_ref(&work_ref),
                origin: mob_dsl::WorkOrigin::from(origin),
            },
            MobState::Running,
        ) {
            if self.state() != MobState::Running {
                return Err(error);
            }
        }

        // Resolve entry + validate fence freshness in a single roster read.
        // Fence-token freshness is a shell-owned concurrency invariant (a
        // stale token means the caller is talking to a superseded
        // incarnation); auto-spawn is an external-only policy seam that
        // runs when the target member is absent and a `SpawnPolicy` is set.
        let initial_entry = {
            let roster = self.roster.read().await;
            roster.get(&agent_identity).cloned()
        };
        if let Some(ref entry) = initial_entry
            && entry.fence_token != fence_token
        {
            return Err(MobError::StaleFenceToken {
                runtime_id,
                expected: entry.fence_token,
                actual: fence_token,
            });
        }
        let entry = match initial_entry {
            Some(e) => {
                if e.state != crate::roster::MemberState::Active {
                    return Err(MobError::MemberNotFound(agent_identity));
                }
                self.ensure_member_not_broken(&e.agent_identity).await?;
                e
            }
            None => {
                if matches!(origin, WorkOrigin::Internal) {
                    return Err(MobError::MemberNotFound(agent_identity));
                }
                let identity = AgentIdentity::from(agent_identity.as_str());
                if let Some(spec) = self.spawn_policy.resolve(&identity).await {
                    let profile = self
                        .definition
                        .resolve_profile(&spec.profile, self.realm_profile_store.as_ref())
                        .await?;
                    self.preview_policy_spawn_submit_work_admission(
                        &identity,
                        profile.external_addressable,
                        &work_ref,
                        origin,
                    )?;
                    Box::pin(self.spawn_from_policy_inline(&identity, spec))
                        .await
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "auto-spawn failed for '{identity}': {error}"
                            ))
                        })?;
                    let spawned_entry = {
                        let roster = self.roster.read().await;
                        roster.get(&identity).cloned()
                    }
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "auto-spawned member '{identity}' missing from roster after completion"
                        ))
                    })?;
                    if spawned_entry.state != crate::roster::MemberState::Active {
                        return Err(MobError::Internal(format!(
                            "auto-spawned member '{identity}' is not active"
                        )));
                    }
                    spawned_entry
                } else {
                    return Err(MobError::MemberNotFound(agent_identity));
                }
            }
        };

        // Project the caller's identifiers into DSL bridging types.
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(entry.fence_token);
        let dsl_work_id = mob_dsl::WorkId::from_work_ref(&work_ref);
        let dsl_origin = mob_dsl::WorkOrigin::from(origin);

        // Apply the DSL SubmitWork input. The MobMachine owns work-origin
        // legality: `SubmitWorkRunningExternal` / `SubmitWorkRunningInternal`
        // encode the origin + addressability + live-runtime + phase guards.
        // A rejection is a state-legality violation, not a freshness issue.
        let transition = match mob_dsl::MobMachineMutator::apply(
            &mut self.dsl_authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: dsl_work_id,
                origin: dsl_origin,
            },
        ) {
            Ok(transition) => transition,
            Err(_) => {
                return Err(self.submit_work_rejection_for_machine_state(
                    &self.dsl_authority.state,
                    &dsl_runtime_id,
                    origin,
                    &agent_identity,
                ));
            }
        };
        if transition.from_phase != transition.to_phase {
            self.dsl_authority.state.lifecycle_phase = transition.to_phase;
            let _ = self.phase_watch_tx.send(self.state());
        }

        // The MobMachine emits `RequestRuntimeIngress` whenever SubmitWork
        // is admitted. The shell realizes that routed-to-MeerkatMachine
        // effect by actually dispatching the turn. Every admitted path
        // must emit at least one ingress effect — if none is present the
        // machine schema has drifted. Drop the transition before the await
        // so the large `MobMachineTransition` struct doesn't balloon the
        // command-loop future size across yield points.
        let ingress_admitted = transition.effects.iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::RequestRuntimeIngress { .. }
            )
        });
        drop(transition);
        if !ingress_admitted {
            return Err(MobError::Internal(
                "MobMachine accepted SubmitWork but emitted no RequestRuntimeIngress effect".into(),
            ));
        }

        self.dispatch_member_turn(&entry, content, handling_mode, render_metadata)
            .await
    }

    /// Unified work-lane cancel entry.
    ///
    /// The MobMachine DSL `CancelAllWork` transition owns live-runtime
    /// membership + phase legality; fence-token freshness is a shell-owned
    /// concurrency invariant (matches the submit-work pattern). Once the
    /// machine accepts, the shell dispatches `interrupt_member` on the
    /// current bridge session.
    async fn handle_cancel_all_work(
        &mut self,
        runtime_id: AgentRuntimeId,
        fence_token: FenceToken,
    ) -> Result<(), MobError> {
        let agent_identity = MeerkatId::from(&runtime_id.identity);

        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&agent_identity)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(agent_identity.clone()))?
        };
        if entry.fence_token != fence_token {
            return Err(MobError::StaleFenceToken {
                runtime_id,
                expected: entry.fence_token,
                actual: fence_token,
            });
        }

        // Feed the DSL CancelAllWork input. Guards enforce live-runtime
        // membership + phase == Running. A rejection here is a state
        // legality violation.
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(entry.fence_token);
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::CancelAllWork {
                agent_runtime_id: dsl_runtime_id,
                fence_token: dsl_fence_token,
            },
            "handle_cancel_all_work",
        )?;

        // Dispatch the interrupt now that the machine has authorized.
        self.provisioner.interrupt_member(&entry.member_ref).await
    }

    async fn dispatch_member_turn(
        &self,
        entry: &RosterEntry,
        content: ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("dispatch_member_turn preflight")?;
        match entry.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                let bridge_session_id = entry.member_ref.bridge_session_id().ok_or_else(|| {
                    Self::peer_only_member_control_error(entry.runtime_mode, "direct turn delivery")
                })?;

                self.ensure_autonomous_runtime_ready(&entry.agent_identity, &entry.member_ref)
                    .await?;

                let injector = self
                    .provisioner
                    .interaction_event_injector(bridge_session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "missing event injector for autonomous member '{}'",
                            entry.agent_identity
                        ))
                    })?;
                injector
                    .inject(
                        content,
                        meerkat_core::PlainEventSource::Rpc,
                        handling_mode,
                        render_metadata,
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "autonomous dispatch inject failed for '{}': {}",
                            entry.agent_identity, error
                        ))
                    })?;
                Ok(())
            }
            crate::MobRuntimeMode::TurnDriven => {
                let req = meerkat_core::service::StartTurnRequest {
                    prompt: content,
                    system_prompt: None,
                    render_metadata,
                    handling_mode,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    turn_metadata: None,
                };
                self.provisioner.start_turn(&entry.member_ref, req).await?;
                Ok(())
            }
        }
    }

    async fn handle_run_flow(
        &mut self,
        flow_id: FlowId,
        activation_params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<meerkat_core::ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        self.ensure_pending_spawn_alignment("handle_run_flow preflight")?;
        self.ensure_flow_tracker_alignment("handle_run_flow preflight")
            .await?;
        let config = FlowRunConfig::from_definition(flow_id, &self.definition)?;
        let run_id = RunId::new();
        let run_flow = MobRun::run_flow_input(&run_id, &config)?;
        debug_assert!(matches!(run_flow, mob_dsl::MobMachineInput::RunFlow { .. }));
        let prepared_run_flow = self
            .prepare_dsl_input(run_flow, "run_flow")
            .map_err(|_| self.invalid_transition_to(MobState::Running))?;
        self.create_pending_run(
            run_id.clone(),
            &config,
            &prepared_run_flow.authority.state,
            activation_params.clone(),
        )
        .await
        .inspect_err(|error| {
            tracing::warn!(
                run_id = %run_id,
                flow_id = %config.flow_id,
                error = %error,
                "flow admission run-store create failed before committing MobMachine RunFlow"
            );
        })?;
        self.commit_prepared_dsl_input(prepared_run_flow);
        if let Err(error) = self.apply_dsl_signal(mob_dsl::MobMachineSignal::StartRun, "start_run")
        {
            let mut details = Vec::new();
            if let Err(rollback_error) = self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::CompleteFlow,
                "complete_flow_rollback",
            ) {
                details.push(format!(
                    "RunFlow CompleteFlow rollback failed: {rollback_error}"
                ));
            }
            let terminalize_reason =
                format!("lifecycle StartRun transition failed during flow admission: {error}");
            if let Err(terminalize_error) = self
                .terminalize_failed_in_actor(
                    run_id.clone(),
                    config.flow_id.clone(),
                    terminalize_reason,
                    "run_flow_start_signal_terminalize_failed",
                )
                .await
            {
                details.push(format!(
                    "terminalizing pending run failed: {terminalize_error}"
                ));
            }
            let detail_suffix = if details.is_empty() {
                String::new()
            } else {
                format!("; {}", details.join("; "))
            };
            return Err(MobError::Internal(format!(
                "lifecycle StartRun transition failed during flow admission: {error}{detail_suffix}"
            )));
        }

        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.run_cancel_tokens.insert(
            run_id.clone(),
            (cancel_token.clone(), config.flow_id.clone()),
        );
        if let Some(scoped_event_tx) = scoped_event_tx {
            self.flow_streams
                .lock()
                .await
                .insert(run_id.clone(), scoped_event_tx);
        }

        let engine = self.flow_engine.clone();
        let cleanup_tx = self.command_tx.clone();
        let flow_engine = self.flow_engine.clone();
        let flow_run_id = run_id.clone();
        let flow_id_for_task = config.flow_id.clone();
        let cleanup_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            let run_id_for_execute = flow_run_id.clone();
            let execution_cancel = cancel_token.clone();
            if let Err(error) = engine
                .execute_flow(
                    run_id_for_execute,
                    config,
                    activation_params,
                    execution_cancel,
                )
                .await
            {
                tracing::error!(
                    run_id = %flow_run_id,
                    flow_id = %flow_id_for_task,
                    error = %error,
                    "flow task execution failed; delegating terminalization to flow-run kernel"
                );
                if cancel_token.is_cancelled() {
                    if let Err(finalize_error) = flow_engine
                        .terminalize_canceled(flow_run_id.clone(), flow_id_for_task)
                        .await
                    {
                        tracing::error!(
                            run_id = %flow_run_id,
                            error = %finalize_error,
                            "failed to finalize canceled run after flow task cancellation"
                        );
                    }
                } else {
                    match error {
                        MobError::RunCanceled(_) => {
                            if let Err(finalize_error) = flow_engine
                                .terminalize_canceled(flow_run_id.clone(), flow_id_for_task)
                                .await
                            {
                                tracing::error!(
                                    run_id = %flow_run_id,
                                    error = %finalize_error,
                                    "failed to finalize canceled run after flow task cancellation"
                                );
                            }
                        }
                        other => {
                            if let Err(finalize_error) = flow_engine
                                .terminalize_failed(
                                    flow_run_id.clone(),
                                    flow_id_for_task,
                                    other.to_string(),
                                )
                                .await
                            {
                                tracing::error!(
                                    run_id = %flow_run_id,
                                    error = %finalize_error,
                                    "failed to finalize run after flow task error"
                                );
                            }
                        }
                    }
                }
            }
            if cleanup_tx
                .send(MobCommand::FlowFinished {
                    run_id: cleanup_run_id,
                })
                .await
                .is_err()
            {
                tracing::warn!(
                    run_id = %flow_run_id,
                    "failed to send FlowFinished cleanup command"
                );
            }
        });
        self.run_tasks.insert(run_id.clone(), handle);
        self.ensure_flow_tracker_alignment("handle_run_flow completion")
            .await?;

        Ok(run_id)
    }

    async fn create_pending_run(
        &self,
        run_id: RunId,
        config: &FlowRunConfig,
        machine_state: &mob_dsl::MobMachineState,
        activation_params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        let flow_state =
            MobRun::flow_state_for_config_with_authority(&run_id, config, machine_state)?;
        let run = MobRun::pending_with_run_id(
            run_id.clone(),
            self.definition.id.clone(),
            config.flow_id.clone(),
            flow_state,
            activation_params,
        );
        self.run_store.create_run(run).await?;
        Ok(run_id)
    }

    async fn handle_flow_cleanup(
        &mut self,
        run_id: RunId,
        context: &'static str,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_flow_cleanup preflight")?;
        self.ensure_flow_tracker_alignment("handle_flow_cleanup preflight")
            .await?;
        let has_task = self.run_tasks.contains_key(&run_id);
        let has_token = self.run_cancel_tokens.contains_key(&run_id);
        let has_stream = self.flow_streams.lock().await.contains_key(&run_id);

        if !has_task && !has_token && !has_stream {
            let run_terminal = self
                .run_store
                .get_run(&run_id)
                .await?
                .as_ref()
                .is_some_and(|run| run.status.is_terminal());
            if !run_terminal {
                return Err(MobError::Internal(format!(
                    "{context}: received cleanup for run {run_id} with no local trackers before the persisted run reached a terminal status"
                )));
            }
            tracing::debug!(
                run_id = %run_id,
                context = context,
                "flow cleanup command had no local run-tracker entries"
            );
            return Ok(());
        }

        self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "flow_cleanup")?;
        self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "flow_cleanup")?;

        let _ = self.run_tasks.remove(&run_id);
        let _ = self.run_cancel_tokens.remove(&run_id);
        let _ = self.flow_streams.lock().await.remove(&run_id);
        self.ensure_flow_tracker_alignment("handle_flow_cleanup completion")
            .await?;
        Ok(())
    }

    async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_cancel_flow preflight")?;
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::CancelFlow {
                run_id: mob_dsl::RunId::from(run_id.to_string()),
            },
            "cancel_flow",
        )?;
        let Some((cancel_token, flow_id)) = self
            .run_cancel_tokens
            .get(&run_id)
            .map(|(token, flow_id)| (token.clone(), flow_id.clone()))
        else {
            if self.run_tasks.contains_key(&run_id)
                || self.flow_streams.lock().await.contains_key(&run_id)
            {
                return Err(MobError::Internal(format!(
                    "handle_cancel_flow: run {run_id} missing cancel token despite live task/stream trackers"
                )));
            }
            self.ensure_flow_tracker_alignment("handle_cancel_flow no-op completion")
                .await?;
            return Ok(());
        };
        self.flow_streams.lock().await.remove(&run_id);
        cancel_token.cancel();

        let Some(mut handle) = self.run_tasks.remove(&run_id) else {
            self.flow_engine.cancel_unfinished_steps(&run_id).await?;
            self.terminalize_canceled_in_actor(
                run_id.clone(),
                flow_id,
                "cancel_flow_no_handle_terminalize_canceled",
            )
            .await?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_flow_no_handle")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "flow canceled cleanup (no task handle): lifecycle CompleteFlow transition failed for run {run_id}: {error}"
                    ))
                })?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "cancel_flow_no_handle")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "flow canceled cleanup (no task handle): lifecycle FinishRun transition failed for run {run_id}: {error}"
                    ))
                })?;
            let _ = self.run_cancel_tokens.remove(&run_id);
            self.ensure_flow_tracker_alignment("handle_cancel_flow no-task cleanup")
                .await?;
            return Ok(());
        };

        let flow_engine = self.flow_engine.clone();
        let cleanup_tx = self.command_tx.clone();
        let cancel_grace_timeout = self
            .definition
            .limits
            .as_ref()
            .and_then(|limits| limits.cancel_grace_timeout_ms)
            .map_or_else(
                || std::time::Duration::from_secs(5),
                std::time::Duration::from_millis,
            );
        let cleanup_run_tracker = run_id.clone();
        let cleanup_run_id_for_error = run_id.clone();
        let cleanup_handle = tokio::spawn(async move {
            let cleanup_run_id = run_id.clone();
            let completed = tokio::select! {
                _ = &mut handle => true,
                () = tokio::time::sleep(cancel_grace_timeout) => false,
            };
            if completed {
                if let Err(error) = flow_engine.cancel_unfinished_steps(&run_id).await {
                    tracing::error!(
                        error = %error,
                        "failed to settle dispatched steps after flow task completion during cancellation"
                    );
                }
                if let Err(error) = flow_engine
                    .terminalize_canceled(run_id.clone(), flow_id)
                    .await
                {
                    tracing::error!(
                        error = %error,
                        "failed to apply canceled terminalization after flow task completion"
                    );
                }
                if cleanup_tx
                    .send(MobCommand::FlowCanceledCleanup {
                        run_id: cleanup_run_id,
                    })
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "failed to send FlowCanceledCleanup command after task completion"
                    );
                }
                return;
            }

            handle.abort();
            if let Err(error) = flow_engine.cancel_unfinished_steps(&run_id).await {
                tracing::error!(
                    error = %error,
                    "failed to settle dispatched steps before flow cancellation terminalization"
                );
            }
            if let Err(error) = flow_engine.terminalize_canceled(run_id, flow_id).await {
                tracing::error!(
                    error = %error,
                    "failed flow-run kernel cancellation terminalization"
                );
            }
            if cleanup_tx
                .send(MobCommand::FlowCanceledCleanup {
                    run_id: cleanup_run_id,
                })
                .await
                .is_err()
            {
                tracing::warn!("failed to send FlowCanceledCleanup command");
            }
        });
        if let Some(replaced) = self.run_tasks.insert(cleanup_run_tracker, cleanup_handle) {
            replaced.abort();
            return Err(MobError::Internal(format!(
                "handle_cancel_flow: duplicate flow cleanup task registration for run {cleanup_run_id_for_error}"
            )));
        }
        self.ensure_flow_tracker_alignment("handle_cancel_flow completion")
            .await?;

        Ok(())
    }

    async fn apply_flow_run_command_in_actor(
        &mut self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        let authority_input = command.authority_input(run_id);
        let prepared = self.prepare_dsl_input(authority_input.clone(), context)?;
        let machine_state = prepared.authority.state.clone();
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let effects = if matches!(command, MobMachineFlowRunCommand::StartRun(_)) {
            self.flow_engine
                .start_run_state_with_machine_state(run_id, machine_state, authority, context)
                .await?
        } else {
            Some(
                self.flow_engine
                    .apply_command_with_machine_state(
                        run_id,
                        command,
                        machine_state,
                        authority,
                        context,
                    )
                    .await?,
            )
        };
        if effects.is_some() {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(effects)
    }

    async fn commit_flow_run_command_in_actor(
        &mut self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.apply_flow_run_command_in_actor(run_id, command, context)
            .await
    }

    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        if !self
            .flow_frame_store_plan_expected_matches(run_id, &plan)
            .await?
        {
            return Ok(false);
        }
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::InsertFrame {
                frame_id,
                initial_frame,
                ..
            } => self
                .run_store
                .cas_frame_state(run_id, frame_id, None, initial_frame.clone())
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::FrameState {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(expected_frame), next_frame.clone())
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                ..
            } => self
                .run_store
                .cas_complete_step_and_record_output(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    step_output_key.clone(),
                    step_output.clone(),
                    loop_context
                        .as_ref()
                        .map(|(loop_id, iteration)| (loop_id, *iteration)),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_grant_node_slot(
                    run_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
                ..
            } => self
                .run_store
                .cas_start_loop(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    initial_loop.clone(),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_grant_body_frame_start(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    initial_frame.clone(),
                    ledger_entry.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(expected_frame), next_frame.clone())
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_loop_request_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from)?,
            FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_loop(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from)?,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }

    async fn flow_frame_store_plan_expected_matches(
        &self,
        run_id: &RunId,
        plan: &FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        Ok(match plan {
            FlowFrameLoopStorePlan::InsertFrame { frame_id, .. } => {
                !run.frames.contains_key(frame_id)
            }
            FlowFrameLoopStorePlan::FrameState {
                frame_id,
                expected_frame,
                ..
            }
            | FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                ..
            }
            | FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                ..
            } => run.frames.get(frame_id) == Some(expected_frame),
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                frame_id,
                expected_frame,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.frames.get(frame_id) == Some(expected_frame)
            }
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                frame_id,
                expected_frame,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.frames.get(frame_id) == Some(expected_frame)
                    && !run.loops.contains_key(loop_instance_id)
            }
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
                    && !run.frames.contains_key(frame_id)
            }
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state, ..
            } => &run.flow_state == expected_run_state,
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_frame,
                expected_run_state,
                ..
            }
            | FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                frame_id,
                expected_frame,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
                    && run.frames.get(frame_id) == Some(expected_frame)
            }
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                expected_run_state,
                ..
            } => {
                &run.flow_state == expected_run_state
                    && run.loops.get(loop_instance_id) == Some(expected_loop)
            }
        })
    }

    async fn commit_flow_terminalization_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        command: MobMachineFlowRunCommand,
        context: &'static str,
    ) -> Result<TerminalizationOutcome, MobError> {
        let run = self
            .run_store
            .get_run(&run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if run.status.is_terminal()
            && !matches!(
                (&target, &run.status),
                (TerminalizationTarget::Canceled, MobRunStatus::Failed)
            )
        {
            return Ok(TerminalizationOutcome::Noop);
        }

        let authority_input = command.authority_input(&run_id);
        let prepared = self.prepare_dsl_input(authority_input.clone(), context)?;
        let machine_state = prepared.authority.state.clone();
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        let outcome = self
            .flow_engine
            .terminalize_with_machine_state(
                run_id.clone(),
                flow_id,
                target.clone(),
                command,
                machine_state,
                authority,
            )
            .await;
        match outcome {
            Ok(TerminalizationOutcome::Transitioned) => {
                self.commit_prepared_dsl_input(prepared);
                Ok(TerminalizationOutcome::Transitioned)
            }
            Ok(TerminalizationOutcome::Noop) => Ok(TerminalizationOutcome::Noop),
            Err(error) => {
                if self
                    .persisted_terminal_status_matches_target(&run_id, &target)
                    .await?
                {
                    self.commit_prepared_dsl_input(prepared);
                }
                Err(error)
            }
        }
    }

    async fn persisted_terminal_status_matches_target(
        &self,
        run_id: &RunId,
        target: &TerminalizationTarget,
    ) -> Result<bool, MobError> {
        Ok(self
            .run_store
            .get_run(run_id)
            .await?
            .is_some_and(|run| run.status == target.status()))
    }

    async fn cancel_unfinished_steps_in_actor(&mut self, run_id: &RunId) -> Result<(), MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        for step_id in run.ordered_steps()? {
            if run
                .flow_state
                .step_status
                .get(&step_id)
                .and_then(|status| *status)
                .is_some_and(|status| !matches!(status, flow_run::StepRunStatus::Dispatched))
            {
                continue;
            }
            self.apply_flow_run_command_in_actor(
                run_id,
                MobMachineFlowRunCommand::CancelStep(flow_run::inputs::CancelStep { step_id }),
                "actor_cancel_unfinished_step",
            )
            .await?;
        }
        Ok(())
    }

    async fn terminalize_canceled_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        context: &'static str,
    ) -> Result<(), MobError> {
        let _ = self
            .commit_flow_terminalization_in_actor(
                run_id,
                flow_id,
                TerminalizationTarget::Canceled,
                MobMachineFlowRunCommand::TerminalizeCanceled(
                    flow_run::inputs::TerminalizeCanceled {},
                ),
                context,
            )
            .await?;
        Ok(())
    }

    async fn terminalize_failed_in_actor(
        &mut self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
        context: &'static str,
    ) -> Result<(), MobError> {
        let _ = self
            .commit_flow_terminalization_in_actor(
                run_id,
                flow_id,
                TerminalizationTarget::Failed { reason },
                MobMachineFlowRunCommand::TerminalizeFailed(flow_run::inputs::TerminalizeFailed {}),
                context,
            )
            .await?;
        Ok(())
    }

    async fn cancel_all_flow_tasks(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("cancel_all_flow_tasks preflight")?;
        let tracked_run_ids = self.run_cancel_tokens.keys().cloned().collect::<Vec<_>>();
        for run_id in tracked_run_ids {
            let Some((token, flow_id)) = self
                .run_cancel_tokens
                .get(&run_id)
                .map(|(token, flow_id)| (token.clone(), flow_id.clone()))
            else {
                continue;
            };

            token.cancel();
            if let Some(handle) = self.run_tasks.remove(&run_id) {
                handle.abort();
            }
            self.flow_streams.lock().await.remove(&run_id);

            self.cancel_unfinished_steps_in_actor(&run_id).await?;
            self.terminalize_canceled_in_actor(
                run_id.clone(),
                flow_id.clone(),
                "cancel_all_flow_terminalize_canceled",
            )
            .await?;

            // CompleteFlow / FinishRun accept `active_run_count == 0` as
            // a legitimate terminal convergence (see
            // `CompleteFlowRunningZero` and `FinishRunRunningZero` in
            // the mob_machine DSL). The natural `FlowFinished` cleanup
            // races with this destroy-driven cancel; both paths drive
            // the authority toward the same terminal state.
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_all_flow")?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishRun, "cancel_all_flow")?;

            let _ = self.run_cancel_tokens.remove(&run_id);
        }
        self.ensure_flow_tracker_alignment("cancel_all_flow_tasks completion")
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Compensate a failed spawn wiring path to avoid partial state.
    async fn rollback_failed_spawn(
        &mut self,
        agent_identity: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
        successful_wiring_targets: &[MeerkatId],
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let retire_event_already_present =
            self.retire_event_exists(agent_identity, member_ref).await?;
        if !retire_event_already_present {
            self.append_retire_event(agent_identity, profile_name, member_ref)
                .await?;
        }
        let spawned_entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        };
        if let Some(entry) = spawned_entry.as_ref() {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
            let releasing = member_ref
                .bridge_session_id()
                .map(mob_dsl::SessionId::from_domain);
            let session_id_for_route = releasing.clone().unwrap_or_default();
            if let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::Retire {
                    mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                    agent_identity: dsl_identity,
                    releasing,
                    session_id: session_id_for_route,
                },
                "rollback_failed_spawn_mark_retiring",
            ) {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    %error,
                    "spawn rollback could not mark runtime retired in DSL"
                );
            }
            if let Some(session_id) = member_ref.bridge_session_id() {
                self.discard_pending_routed_effects_for_session(session_id);
            }
        }

        let mut wired_peers = successful_wiring_targets.to_vec();
        wired_peers.sort();
        wired_peers.dedup();

        let mut cleanup_peers = wired_peers.clone();
        for peer_id in planned_wiring_targets {
            if peer_id != agent_identity && !cleanup_peers.contains(peer_id) {
                cleanup_peers.push(peer_id.clone());
            }
        }
        let spawned_comms = self.provisioner_comms(member_ref).await;
        let mut rollback = LifecycleRollback::new("spawn rollback");

        if !wired_peers.is_empty() {
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{agent_identity}'"
                ))
            })?;
            let spawned_sender = self
                .sender_runtime_for_entry(spawned_entry)
                .await
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "spawn rollback requires sender runtime for '{agent_identity}'"
                    ))
                })?;
            for peer_meerkat_id in &wired_peers {
                let peer_spec = {
                    let roster = self.roster.read().await;
                    let peer_entry = roster.get(peer_meerkat_id).cloned().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "spawn rollback requires roster entry for wired peer '{peer_meerkat_id}'"
                        ))
                    })?;
                    drop(roster);
                    match self
                        .resolve_wiring_endpoint(&peer_entry, "spawn rollback")
                        .await?
                    {
                        WiringEndpoint::Local { spec, .. }
                        | WiringEndpoint::PeerOnly { spec, .. } => spec,
                    }
                };
                self.notify_peer_retired(
                    &peer_spec,
                    agent_identity,
                    spawned_entry,
                    &spawned_sender,
                )
                .await?;
                rollback.defer(
                    format!(
                        "compensating mob.peer_added '{agent_identity}' -> '{peer_meerkat_id}'"
                    ),
                    {
                        let spawned_sender = spawned_sender.clone();
                        let peer_spec = peer_spec.clone();
                        let spawned_entry = spawned_entry.clone();
                        let agent_identity = agent_identity.clone();
                        let actor = &*self;
                        move || async move {
                            actor
                                .notify_peer_added(
                                    &spawned_sender,
                                    &peer_spec,
                                    &agent_identity,
                                    &spawned_entry,
                                )
                                .await
                        }
                    },
                );
            }
        }

        if let Some(spawned_entry) = spawned_entry.as_ref()
            && let Ok(spawned_endpoint) = self
                .resolve_wiring_endpoint(spawned_entry, "spawn rollback trust cleanup spawned")
                .await
        {
            let (spawned_spec, spawned_comms, spawned_binding) = match spawned_endpoint {
                WiringEndpoint::Local { comms, spec, .. } => (spec, Some(comms), None),
                WiringEndpoint::PeerOnly { spec, binding } => (spec, None, Some(binding)),
            };
            for peer_meerkat_id in &cleanup_peers {
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    continue;
                };
                let Ok(peer_endpoint) = self
                    .resolve_wiring_endpoint(&peer_entry, "spawn rollback trust cleanup peer")
                    .await
                else {
                    continue;
                };
                let (peer_spec, peer_comms, peer_binding) = match peer_endpoint {
                    WiringEndpoint::Local { comms, spec, .. } => (spec, Some(comms), None),
                    WiringEndpoint::PeerOnly { spec, binding } => (spec, None, Some(binding)),
                };
                if let Some(spawned_comms) = spawned_comms.as_ref() {
                    let _ =
                        Self::remove_trusted_peer_by_descriptor(&**spawned_comms, &peer_spec).await;
                } else if let Some(spawned_binding) = spawned_binding.as_ref() {
                    let _ = self
                        .unwire_peer_only_recipient(
                            &spawned_spec,
                            Some(spawned_binding),
                            &peer_spec,
                            std::time::Duration::from_secs(2),
                        )
                        .await;
                }
                if let Some(peer_comms) = peer_comms {
                    let _ =
                        Self::remove_trusted_peer_by_descriptor(&*peer_comms, &spawned_spec).await;
                } else if let Some(peer_binding) = peer_binding {
                    let _ = self
                        .unwire_peer_only_recipient(
                            &peer_spec,
                            Some(&peer_binding),
                            &spawned_spec,
                            std::time::Duration::from_secs(2),
                        )
                        .await;
                }
            }
        }

        // Reuse disposal pipeline methods for session archive + roster removal.
        let rollback_ctx = DisposalContext {
            agent_identity: agent_identity.clone(),
            entry: spawned_entry.clone().unwrap_or_else(|| {
                let identity = AgentIdentity::from(agent_identity.as_str());
                RosterEntry {
                    agent_identity: identity.clone(),
                    generation: crate::ids::Generation::INITIAL,
                    fence_token: crate::ids::FenceToken::new(0),
                    agent_runtime_id: crate::ids::AgentRuntimeId::initial(identity),
                    role: profile_name.clone(),
                    member_ref: member_ref.clone(),
                    runtime_mode: crate::MobRuntimeMode::TurnDriven,
                    peer_id: spawned_comms.as_ref().and_then(|c| c.peer_id()),
                    transport_public_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
                    state: crate::roster::MemberState::Active,
                    wired_to: std::collections::BTreeSet::new(),
                    external_peer_specs: std::collections::BTreeMap::new(),
                    labels: std::collections::BTreeMap::new(),
                    kickoff: None,
                    effective_profile_override: None,
                }
            }),
            retiring_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
        };
        if let Err(error) = self.dispose_archive_session(&rollback_ctx).await {
            return Err(rollback.fail(error).await);
        }

        self.dispose_remove_from_roster(&rollback_ctx).await;
        self.delete_external_binding_overlay_for_member(
            &rollback_ctx.entry.agent_identity,
            rollback_ctx.entry.generation,
        )
        .await?;

        Ok(())
    }

    /// Resolve profile-declared rust tool bundles to a dispatcher.
    fn external_tools_for_profile(
        &self,
        profile: &crate::profile::Profile,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        let default_tools = self
            .default_external_tools_provider
            .as_ref()
            .and_then(|p| p());
        compose_external_tools_for_profile(
            profile,
            &self.tool_bundles,
            self.mob_handle_for_tools(),
            default_tools,
            crate::build::MobToolAccessContext::None,
        )
    }

    async fn retire_event_exists(
        &self,
        agent_identity: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<bool, MobError> {
        let key = Self::retire_event_key(agent_identity, member_ref);
        let index = self.retired_event_index.read().await;
        Ok(index.contains(&key))
    }

    async fn append_retire_event(
        &self,
        agent_identity: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Look up identity-native fields from the roster for the 0.6 event model.
        let (resolved_identity, generation) = {
            let roster = self.roster.read().await;
            match roster.get(agent_identity) {
                Some(entry) => (entry.agent_identity.clone(), entry.generation),
                None => (
                    AgentIdentity::from(agent_identity.as_str()),
                    Generation::INITIAL,
                ),
            }
        };
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberRetired {
                    agent_identity: resolved_identity,
                    generation,
                    role: profile_name.clone(),
                },
            })
            .await?;
        let key = Self::retire_event_key(agent_identity, member_ref);
        self.retired_event_index.write().await.insert(key);
        Ok(())
    }

    /// Get the comms runtime for a session, if available.
    async fn provisioner_comms(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.provisioner.comms_runtime(member_ref).await
    }

    fn trusted_peer_descriptor_from_machine_endpoint(
        endpoint: &mob_dsl::ExternalPeerEndpoint,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.name.0.clone(),
            &endpoint.peer_id.0,
            endpoint.signing_key.0,
            &endpoint.address.0,
        )
        .map_err(|error| {
            MobError::WiringError(format!(
                "MobMachine external peer edge has invalid descriptor '{}': {error}",
                endpoint.name.0
            ))
        })
    }

    fn machine_restore_wiring_plan(
        &self,
        agent_identity: &MeerkatId,
        live_local_identities: &HashSet<MeerkatId>,
    ) -> Result<RestoreWiringPlan, MobError> {
        let local =
            mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
        let mut local_peers = Vec::new();
        for edge in &self.dsl_authority.state.wiring_edges {
            let peer = if edge.a == local {
                Some(&edge.b)
            } else if edge.b == local {
                Some(&edge.a)
            } else {
                None
            };
            if let Some(peer) = peer {
                let peer_identity = MeerkatId::from(peer.0.as_str());
                if live_local_identities.contains(&peer_identity) {
                    local_peers.push(peer_identity);
                }
            }
        }
        local_peers.sort();
        local_peers.dedup();

        let mut external_peers = Vec::new();
        for edge in &self.dsl_authority.state.external_peer_edges {
            if edge.local == local {
                external_peers.push(Self::trusted_peer_descriptor_from_machine_endpoint(
                    &edge.endpoint,
                )?);
            }
        }
        external_peers.sort_by(|a, b| {
            a.name
                .as_str()
                .cmp(b.name.as_str())
                .then_with(|| a.peer_id.to_string().cmp(&b.peer_id.to_string()))
        });
        external_peers.dedup_by(|a, b| a.name == b.name && a.peer_id == b.peer_id);

        Ok(RestoreWiringPlan {
            local_peers,
            external_peers,
        })
    }

    async fn sender_runtime_for_entry(
        &self,
        entry: &RosterEntry,
    ) -> Option<Arc<dyn CoreCommsRuntime>> {
        if let Some(comms) = self.provisioner_comms(&entry.member_ref).await {
            return Some(comms);
        }
        if matches!(
            entry.member_ref,
            MemberRef::BackendPeer {
                session_id: None,
                ..
            }
        ) {
            return Some(self.supervisor_bridge.runtime_core().await);
        }
        None
    }

    /// Generate the comms name for a roster entry.
    fn comms_name_for(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.role, entry.agent_identity
        )
    }

    async fn resolve_wiring_endpoint(
        &self,
        entry: &RosterEntry,
        context: &'static str,
    ) -> Result<WiringEndpoint, MobError> {
        let comms_name = self.comms_name_for(entry);
        if let Some(comms) = self.provisioner_comms(&entry.member_ref).await {
            let public_key = comms.public_key().ok_or_else(|| {
                MobError::WiringError(format!(
                    "{context} requires public key for '{}'",
                    entry.agent_identity
                ))
            })?;
            let spec = self
                .provisioner
                .trusted_peer_spec(&entry.member_ref, &comms_name, &public_key)
                .await?;
            return Ok(WiringEndpoint::Local {
                entry: Box::new(entry.clone()),
                comms,
                spec,
                comms_name,
            });
        }

        match &entry.member_ref {
            MemberRef::BackendPeer { peer_id, .. } => {
                let binding = Self::runtime_binding_for_entry(entry).ok_or_else(|| {
                    MobError::WiringError(format!(
                        "{context} requires external runtime binding for '{}'",
                        entry.agent_identity
                    ))
                })?;
                let spec = Self::peer_only_spec_for_binding(&binding, context)?;
                Ok(WiringEndpoint::PeerOnly { spec, binding })
            }
            MemberRef::Session { .. } => Err(MobError::WiringError(format!(
                "{context} requires comms runtime for '{}'",
                entry.agent_identity
            ))),
        }
    }

    /// Notify a peer that a new peer was added.
    ///
    /// Sends a `PeerRequest` with intent `mob.peer_added` FROM `sender_comms`
    /// TO the peer identified by `recipient_comms_name`. The params contain
    /// the new peer's identity and role.
    ///
    /// REQ-MOB-010/011: Notification is required for successful wiring.
    async fn notify_peer_added(
        &self,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
        recipient_spec: &TrustedPeerDescriptor,
        new_peer_id: &MeerkatId,
        new_peer_entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let peer_description = self
            .definition
            .resolve_profile(&new_peer_entry.role, self.realm_profile_store.as_ref())
            .await
            .map(|p| p.peer_description)
            .unwrap_or_default();

        let new_peer_spec = match self
            .resolve_wiring_endpoint(new_peer_entry, "notify_peer_added")
            .await?
        {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        let peer_route =
            PeerRoute::with_display_name(recipient_spec.peer_id, recipient_spec.name.clone());

        let cmd = CommsCommand::PeerLifecycle {
            to: peer_route,
            kind: PeerLifecycleKind::PeerAdded,
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.role.as_str(),
                "description": peer_description,
                "peer_name": new_peer_spec.name,
                "peer_id": new_peer_spec.peer_id,
                "address": new_peer_spec.address,
                "peer_spec": new_peer_spec,
            }),
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_peer_event(
        &self,
        intent: &'static str,
        recipient_spec: &TrustedPeerDescriptor,
        other_peer_id: &MeerkatId,
        other_peer_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let other_peer_spec = match self
            .resolve_wiring_endpoint(other_peer_entry, "notify_peer_event")
            .await?
        {
            WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
        };
        let peer_route =
            PeerRoute::with_display_name(recipient_spec.peer_id, recipient_spec.name.clone());

        let params = serde_json::json!({
            "peer": other_peer_id.as_str(),
            "role": other_peer_entry.role.as_str(),
            "peer_name": other_peer_spec.name,
            "peer_id": other_peer_spec.peer_id,
            "address": other_peer_spec.address,
            "peer_spec": other_peer_spec,
        });

        let cmd = match intent {
            "mob.peer_retired" => CommsCommand::PeerLifecycle {
                to: peer_route,
                kind: PeerLifecycleKind::PeerRetired,
                params,
            },
            "mob.peer_unwired" => CommsCommand::PeerLifecycle {
                to: peer_route,
                kind: PeerLifecycleKind::PeerUnwired,
                params,
            },
            _ => CommsCommand::PeerRequest {
                to: peer_route,
                intent: intent.to_string(),
                params,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                stream: meerkat_core::comms::InputStreamMode::None,
            },
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_kickoff_event(
        &self,
        agent_identity: &MeerkatId,
        intent: &'static str,
    ) -> Result<(), MobError> {
        let (entry, wired_peers) = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(agent_identity).cloned() else {
                return Ok(());
            };
            let wired_peers: Vec<MeerkatId> = entry
                .wired_to
                .iter()
                .filter_map(|id| roster.get_by_identity(id).map(|e| e.agent_identity.clone()))
                .collect();
            (entry, wired_peers)
        };
        let sender_comms = self.provisioner_comms(&entry.member_ref).await;
        let effects = MobRuntimeBridgeAuthority::plan_lifecycle_notice(
            sender_comms.is_some()
                || matches!(
                    entry.member_ref,
                    MemberRef::BackendPeer {
                        session_id: None,
                        ..
                    }
                ),
            &wired_peers,
            intent,
        );

        let Some(sender_comms) = self.sender_runtime_for_entry(&entry).await else {
            return Ok(());
        };

        for effect in effects {
            let MobRuntimeBridgeEffect::DeliverLifecycleNotice { peer_id, intent } = effect;
            let recipient_entry = {
                let roster = self.roster.read().await;
                roster.get(&peer_id).cloned()
            };
            let Some(recipient_entry) = recipient_entry else {
                continue;
            };
            let recipient_spec = match self
                .resolve_wiring_endpoint(&recipient_entry, "notify_kickoff_event")
                .await?
            {
                WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
            };
            self.notify_peer_event(
                intent,
                &recipient_spec,
                agent_identity,
                &entry,
                &sender_comms,
            )
            .await?;
        }
        Ok(())
    }

    /// Notify a peer that another peer was retired from the mob.
    async fn notify_peer_retired(
        &self,
        recipient_spec: &TrustedPeerDescriptor,
        retired_id: &MeerkatId,
        retired_entry: &RosterEntry,
        retiring_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_retired",
            recipient_spec,
            retired_id,
            retired_entry,
            retiring_comms,
        )
        .await
    }

    /// Notify a peer that another peer was unwired (trust link removed).
    async fn notify_peer_unwired(
        &self,
        recipient_spec: &TrustedPeerDescriptor,
        unwired_id: &MeerkatId,
        unwired_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_unwired",
            recipient_spec,
            unwired_id,
            unwired_entry,
            sender_comms,
        )
        .await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod bridge_rejection_tests {
    use super::MobActor;
    use crate::runtime::bridge_protocol::{
        BridgeRejectionCause, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    };
    use serde_json::json;

    #[test]
    fn actor_decodes_typed_protocol_v2_bridge_rejection() {
        let value = json!({
            "result": "rejected",
            "cause": "stale_supervisor",
            "reason": "epoch too low",
        });

        let rejection =
            MobActor::bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value)
                .expect("typed rejection should decode");

        assert_eq!(
            rejection.typed_cause(),
            Some(BridgeRejectionCause::StaleSupervisor)
        );
        assert_eq!(rejection.reason(), "epoch too low");
    }

    #[test]
    fn actor_does_not_promote_raw_string_as_protocol_v2_rejection() {
        let value = json!("legacy rejection");

        assert!(
            MobActor::bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value).is_none()
        );
        let legacy = MobActor::bridge_rejection_reply(1, &value)
            .expect("legacy raw string should decode only on protocol v1");
        assert_eq!(legacy.typed_cause(), None);
        assert!(legacy.is_legacy_v1_raw_string());
    }
}
