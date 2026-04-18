use super::disposal::{
    BulkBestEffort, DisposalContext, DisposalReport, DisposalStep, ErrorPolicy, WarnAndContinue,
};
use super::mob_runtime_bridge_authority::{MobRuntimeBridgeAuthority, MobRuntimeBridgeEffect};
use super::mob_wiring_authority::{
    ExternalUnwirePlan, ExternalWireInput, ExternalWirePlan, LocalUnwirePlan, LocalWirePlan,
    MobWiringAuthority,
};
use super::provision_guard::PendingProvision;
use super::transaction::LifecycleRollback;
use super::*;
use crate::ids::{AgentIdentity, Generation};
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::time_compat::SystemTime;
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet, VecDeque};

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
        spec: TrustedPeerSpec,
        comms_name: String,
    },
    PeerOnly {
        spec: TrustedPeerSpec,
        binding: crate::RuntimeBinding,
    },
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
    meerkat_id: &MeerkatId,
) -> Result<crate::RuntimeBinding, MobError> {
    if let Some(b) = binding {
        return Ok(b);
    }
    let kind = backend.or(profile_backend).unwrap_or(definition_default);
    match kind {
        crate::MobBackendKind::Session => Ok(crate::RuntimeBinding::Session),
        crate::MobBackendKind::External => Err(MobError::WiringError(format!(
            "external backend requires explicit RuntimeBinding for '{meerkat_id}'"
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
            Message::ToolResults { results } => {
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
    pub(super) meerkat_id: MeerkatId,
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
    pub(super) voice_intent_present: bool,
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
    external_peers: Vec<TrustedPeerSpec>,
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
    voice_intent_present: bool,
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
    pub(super) state: Arc<AtomicU8>,
    pub(super) events: Arc<dyn MobEventStore>,
    pub(super) run_store: Arc<dyn MobRunStore>,
    pub(super) provisioner: Arc<dyn MobProvisioner>,
    pub(super) flow_engine: FlowEngine,
    pub(super) flow_kernel: FlowRunKernel,
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
    pub(super) default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    pub(super) realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
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
    ) -> Result<TrustedPeerSpec, MobError> {
        TrustedPeerSpec::new(
            address
                .strip_prefix("inproc://")
                .map(|value| value.split('?').next().unwrap_or(value).to_string())
                .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}")),
            peer_id.to_string(),
            address.to_string(),
        )
        .map_err(|error| {
            MobError::WiringError(format!(
                "{context}: invalid peer-only runtime spec: {error}"
            ))
        })
    }

    fn peer_only_spec_for_binding(
        binding: &crate::RuntimeBinding,
        context: &'static str,
    ) -> Result<TrustedPeerSpec, MobError> {
        match binding {
            crate::RuntimeBinding::External {
                peer_id, address, ..
            } => Self::peer_only_spec_from_parts(peer_id, address, context),
            crate::RuntimeBinding::Session => Err(MobError::Internal(format!(
                "{context}: peer-only runtime spec requested for session binding"
            ))),
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
            } => {
                if let Some(token) = bootstrap_token.as_ref().filter(|token| !token.is_empty()) {
                    return Ok(token.clone());
                }
                let query = address.split_once('?').map(|(_, query)| query).ok_or_else(|| {
                    MobError::WiringError(format!(
                        "external runtime binding for '{address}' is missing bridge bootstrap token"
                    ))
                })?;
                for pair in query.split('&') {
                    let Some((key, value)) = pair.split_once('=') else {
                        continue;
                    };
                    if key == super::bridge_protocol::SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM
                        && !value.is_empty()
                    {
                        return Ok(super::bridge_protocol::BridgeBootstrapToken::new(value));
                    }
                }
                Err(MobError::WiringError(format!(
                    "external runtime binding for '{address}' is missing bootstrap token ('{}' query param or explicit field)",
                    super::bridge_protocol::SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM
                )))
            }
            crate::RuntimeBinding::Session => Err(MobError::Internal(
                "bridge bootstrap token requested for session binding".to_string(),
            )),
        }
    }

    async fn bind_peer_only_member_for_binding(
        &self,
        peer: &TrustedPeerSpec,
        binding: &crate::RuntimeBinding,
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        self.bind_peer_only_member_for_binding_with_payload(peer, binding, &payload)
            .await
    }

    async fn bind_peer_only_member_for_binding_with_payload(
        &self,
        peer: &TrustedPeerSpec,
        binding: &crate::RuntimeBinding,
        payload: &super::bridge_protocol::BridgeSupervisorPayload,
    ) -> Result<super::bridge_protocol::BridgeBindResponse, MobError> {
        let crate::RuntimeBinding::External {
            peer_id,
            address,
            bootstrap_token: _,
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

    fn bridge_rejection_reason(
        value: &serde_json::Value,
    ) -> Option<(super::bridge_protocol::BridgeRejectionCause, String)> {
        if let Some(reason) = value.as_str() {
            return Some((
                super::bridge_protocol::BridgeRejectionCause::Internal,
                reason.to_string(),
            ));
        }
        match serde_json::from_value::<super::bridge_protocol::BridgeReply>(value.clone()).ok()? {
            super::bridge_protocol::BridgeReply::Rejected { cause, reason } => {
                Some((cause, reason))
            }
            _ => None,
        }
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
        peer: &TrustedPeerSpec,
        binding: Option<&crate::RuntimeBinding>,
    ) -> Result<TrustedPeerSpec, MobError> {
        let payload = self.bridge_supervisor_payload().await?;
        let command = super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(payload);
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, &command, std::time::Duration::from_secs(30))
            .await?;
        if let Some((cause, reason)) = Self::bridge_rejection_reason(&value) {
            if super::bridge_fallback::should_fall_back_to_bind(cause)
                && let Some(binding) = binding
            {
                let bind = self
                    .bind_peer_only_member_for_binding(peer, binding)
                    .await?;
                let effective_bootstrap_token = Self::bridge_bootstrap_token_from_binding(binding)?;
                self.persist_rebound_binding(binding, &bind).await?;
                return Self::peer_only_spec_for_binding(
                    &crate::RuntimeBinding::External {
                        peer_id: bind.peer_id,
                        address: super::bridge_protocol::canonicalize_bridge_address(&bind.address),
                        bootstrap_token: Some(effective_bootstrap_token),
                    },
                    "ensure_supervisor_authorized rebound peer",
                );
            }
            return Err(MobError::WiringError(reason));
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
        peer: &TrustedPeerSpec,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: std::time::Duration,
    ) -> Result<R, MobError> {
        let value = self
            .supervisor_bridge
            .send_bridge_command(peer, command, timeout)
            .await?;
        if let Some((_cause, reason)) = Self::bridge_rejection_reason(&value) {
            return Err(MobError::WiringError(reason));
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
        recipient: &TrustedPeerSpec,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerSpec,
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
        recipient: &TrustedPeerSpec,
        recipient_binding: Option<&crate::RuntimeBinding>,
        peer_spec: &TrustedPeerSpec,
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

    async fn restore_failure_for(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Option<super::handle::RestoreFailureDiagnostic> {
        self.restore_diagnostics
            .read()
            .await
            .get(meerkat_id)
            .cloned()
    }

    async fn ensure_member_not_broken(&self, meerkat_id: &MeerkatId) -> Result<(), MobError> {
        if let Some(diag) = self.restore_failure_for(meerkat_id).await {
            return Err(MobError::MemberRestoreFailed {
                member_id: meerkat_id.clone(),
                session_id: diag.bridge_session_id,
                reason: diag.reason,
            });
        }
        Ok(())
    }

    fn state(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn apply_dsl_input(
        &mut self,
        input: mob_dsl::MobMachineInput,
        context: &str,
    ) -> Result<(), MobError> {
        let transition = mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input)
            .map_err(|e| MobError::Internal(format!("DSL authority ({context}): {e}")))?;
        if transition.from_phase != transition.to_phase {
            let mob_phase = match transition.to_phase {
                mob_dsl::MobPhase::Running => MobState::Running,
                mob_dsl::MobPhase::Stopped => MobState::Stopped,
                mob_dsl::MobPhase::Completed => MobState::Completed,
                mob_dsl::MobPhase::Destroyed => MobState::Destroyed,
            };
            self.state
                .store(mob_phase as u8, std::sync::atomic::Ordering::Release);
            // Explicitly update DSL state's lifecycle_phase
            self.dsl_authority.state.lifecycle_phase = transition.to_phase;
        }
        Ok(())
    }

    fn apply_dsl_signal(
        &mut self,
        signal: mob_dsl::MobMachineSignal,
        context: &str,
    ) -> Result<(), MobError> {
        let transition = self
            .dsl_authority
            .apply_signal(signal)
            .map_err(|e| MobError::Internal(format!("DSL authority ({context}): {e}")))?;
        if transition.from_phase != transition.to_phase {
            let mob_phase = match transition.to_phase {
                mob_dsl::MobPhase::Running => MobState::Running,
                mob_dsl::MobPhase::Stopped => MobState::Stopped,
                mob_dsl::MobPhase::Completed => MobState::Completed,
                mob_dsl::MobPhase::Destroyed => MobState::Destroyed,
            };
            self.state
                .store(mob_phase as u8, std::sync::atomic::Ordering::Release);
            // Explicitly update DSL state's lifecycle_phase
            self.dsl_authority.state.lifecycle_phase = transition.to_phase;
        }
        Ok(())
    }

    async fn sync_dsl_roster_state(&mut self) {
        let roster = self.roster.read().await;
        let mut live_ids = std::collections::BTreeSet::new();
        let mut fence_map = std::collections::BTreeMap::new();
        for entry in roster.list() {
            let rid = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
            live_ids.insert(rid.clone());
            fence_map.insert(rid, mob_dsl::FenceToken::from_domain(entry.fence_token));
        }
        drop(roster);
        self.dsl_authority.state.live_runtime_ids = live_ids;
        self.dsl_authority.state.runtime_fence_tokens = fence_map;
    }

    #[allow(dead_code)]
    fn sync_dsl_run_state(&mut self) {
        let active_run_count = self.machine_active_run_count() as u64;
        self.dsl_authority.state.active_run_count = active_run_count;
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            definition: self.definition.clone(),
            state: self.state.clone(),
            events: self.events.clone(),
            flow_streams: self.flow_streams.clone(),
            session_service: self.session_service.clone(),
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: self.runtime_adapter.clone(),
            restore_diagnostics: self.restore_diagnostics.clone(),
        }
    }

    async fn persist_kickoff_state(
        &self,
        meerkat_id: &MeerkatId,
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
                    member: AgentIdentity::from(meerkat_id.as_str()),
                    kickoff: kickoff.clone(),
                },
            })
            .await
            .map_err(MobError::from)?;
        self.roster
            .write()
            .await
            .set_kickoff(meerkat_id, Some(kickoff));
        Ok(())
    }

    async fn kickoff_phase_for(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Option<crate::roster::MobMemberKickoffPhase> {
        self.roster
            .read()
            .await
            .get(meerkat_id)
            .and_then(|entry| entry.kickoff.as_ref().map(|snapshot| snapshot.phase))
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

    fn kickoff_outcome_string(outcome: &meerkat_runtime::completion::CompletionOutcome) -> String {
        match outcome {
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
            | meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => {
                "Started".to_string()
            }
            meerkat_runtime::completion::CompletionOutcome::CallbackPending { .. } => {
                "CallbackPending".to_string()
            }
            meerkat_runtime::completion::CompletionOutcome::Abandoned(reason)
            | meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
                reason.clone()
            }
        }
    }

    fn kickoff_notice_intent(intent: &str) -> Option<&'static str> {
        match intent {
            "Failed" => Some("mob.kickoff_failed"),
            "Cancelled" => Some("mob.kickoff_cancelled"),
            _ => None,
        }
    }

    async fn clear_kickoff_state(&mut self, meerkat_id: &MeerkatId) {
        self.dsl_authority
            .state
            .member_kickoff_pending
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_starting
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_callback_pending
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_started
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_failed
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_cancelled
            .remove(&meerkat_id.to_string());
        self.dsl_authority
            .state
            .member_kickoff_error
            .remove(&meerkat_id.to_string());
        self.roster.write().await.set_kickoff(meerkat_id, None);
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
        meerkat_id: &MeerkatId,
        input: mob_dsl::MobMachineInput,
    ) -> Result<bool, MobError> {
        if !self
            .dsl_authority
            .state
            .member_kickoff_pending
            .contains(&meerkat_id.to_string())
            && !self
                .dsl_authority
                .state
                .member_kickoff_starting
                .contains(&meerkat_id.to_string())
            && !self
                .dsl_authority
                .state
                .member_kickoff_callback_pending
                .contains(&meerkat_id.to_string())
            && !self
                .dsl_authority
                .state
                .member_kickoff_started
                .contains(&meerkat_id.to_string())
            && !self
                .dsl_authority
                .state
                .member_kickoff_failed
                .contains(&meerkat_id.to_string())
            && !self
                .dsl_authority
                .state
                .member_kickoff_cancelled
                .contains(&meerkat_id.to_string())
            && let Some(current_phase) = self.kickoff_phase_for(meerkat_id).await
        {
            match Self::kickoff_phase_to_dsl(current_phase) {
                mob_dsl::KickoffPhase::Pending => {
                    self.dsl_authority
                        .state
                        .member_kickoff_pending
                        .insert(meerkat_id.to_string());
                }
                mob_dsl::KickoffPhase::Starting => {
                    self.dsl_authority
                        .state
                        .member_kickoff_starting
                        .insert(meerkat_id.to_string());
                }
                mob_dsl::KickoffPhase::CallbackPending => {
                    self.dsl_authority
                        .state
                        .member_kickoff_callback_pending
                        .insert(meerkat_id.to_string());
                }
                mob_dsl::KickoffPhase::Started => {
                    self.dsl_authority
                        .state
                        .member_kickoff_started
                        .insert(meerkat_id.to_string());
                }
                mob_dsl::KickoffPhase::Failed => {
                    self.dsl_authority
                        .state
                        .member_kickoff_failed
                        .insert(meerkat_id.to_string());
                }
                mob_dsl::KickoffPhase::Cancelled => {
                    self.dsl_authority
                        .state
                        .member_kickoff_cancelled
                        .insert(meerkat_id.to_string());
                }
            }
            if let Some(error) = self.roster.read().await.get(meerkat_id).and_then(|entry| {
                entry
                    .kickoff
                    .as_ref()
                    .and_then(|snapshot| snapshot.error.clone())
            }) {
                self.dsl_authority
                    .state
                    .member_kickoff_error
                    .insert(meerkat_id.to_string(), error);
            }
        }

        let transition = match mob_dsl::MobMachineMutator::apply(&mut self.dsl_authority, input) {
            Ok(transition) => transition,
            Err(_) => return Ok(false),
        };

        for effect in transition.effects {
            match effect {
                mob_dsl::MobMachineEffect::PersistKickoffUpdate {
                    member_id: _,
                    phase,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(meerkat_id, phase, None).await?;
                }
                mob_dsl::MobMachineEffect::PersistKickoffFailureUpdate {
                    member_id: _,
                    phase,
                    error,
                } => {
                    let phase = Self::kickoff_phase_from_dsl(phase);
                    self.persist_kickoff_state(meerkat_id, phase, Some(error))
                        .await?;
                }
                mob_dsl::MobMachineEffect::EmitKickoffLifecycleNotice {
                    member_id: _,
                    intent,
                } => {
                    if let Some(notice_intent) = Self::kickoff_notice_intent(&intent)
                        && let Err(error) =
                            self.notify_kickoff_event(meerkat_id, notice_intent).await
                    {
                        tracing::warn!(
                            meerkat_id = %meerkat_id,
                            error = %error,
                            intent = %intent,
                            "failed to emit kickoff lifecycle notice"
                        );
                    }
                }
                _ => {}
            }
        }

        Ok(true)
    }

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

    fn machine_coordinator_bound(&self) -> bool {
        // Plain mobs (no orchestrator) are always considered coordinator-bound
        // for the purposes of spawn/run-flow admission checks.
        !self.has_orchestrator || self.dsl_authority.state.coordinator_bound
    }

    fn require_mob_machine_stop(&self) -> Result<(), MobError> {
        if self.state() != MobState::Running {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Stopped,
            });
        }
        if self.machine_active_run_count() != 0 {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Stopped,
            });
        }
        Ok(())
    }

    fn require_mob_machine_resume(&self) -> Result<(), MobError> {
        if self.state() == MobState::Stopped {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Running,
            })
        }
    }

    fn require_mob_machine_complete(&self) -> Result<(), MobError> {
        if self.state() == MobState::Running {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Completed,
            })
        }
    }

    fn require_mob_machine_destroy(&self) -> Result<(), MobError> {
        if matches!(
            self.state(),
            MobState::Running | MobState::Stopped | MobState::Completed
        ) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Destroyed,
            })
        }
    }

    fn require_mob_machine_reset(&self) -> Result<(), MobError> {
        if matches!(
            self.state(),
            MobState::Running | MobState::Stopped | MobState::Completed
        ) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Running,
            })
        }
    }

    fn require_mob_machine_shutdown(&self) -> Result<(), MobError> {
        if matches!(
            self.state(),
            MobState::Running | MobState::Stopped | MobState::Completed
        ) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Stopped,
            })
        }
    }

    fn require_mob_machine_spawn(&self) -> Result<(), MobError> {
        if self.state() != MobState::Running || !self.machine_coordinator_bound() {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Running,
            });
        }
        Ok(())
    }

    fn require_mob_machine_run_flow(&self) -> Result<(), MobError> {
        if self.state() != MobState::Running || !self.machine_coordinator_bound() {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Running,
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
        let meerkat_id = orchestrator_entry.meerkat_id;
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
                                "orchestrator lifecycle inject failed for '{meerkat_id}': {error}"
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
                                additional_instructions: None,
                                execution_kind: None,
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

    fn retire_event_key(meerkat_id: &MeerkatId, member_ref: &MemberRef) -> String {
        let member =
            serde_json::to_string(member_ref).unwrap_or_else(|_| format!("{member_ref:?}"));
        format!("{meerkat_id}|{member}")
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
        self.pending_spawns.alignment_violation(expected)
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
        if matches!(impact, PendingSpawnInsertImpact::Collided) {
            // StageSpawn has already been accepted for the new slot in enqueue paths.
            // If we replaced a prior slot at the same ticket, close that prior
            // staged snapshot now so authority counters cannot drift silently.
            self.complete_orchestrator_spawn(
                Some(spawn_ticket),
                "pending spawn slot collision replaced existing entry",
            );
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
            self.complete_orchestrator_spawn(Some(spawn_ticket), context);
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

    fn stage_orchestrator_spawn(&mut self) -> Result<(), MobError> {
        let mut topology_advanced = false;
        if self.has_orchestrator {
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::StageSpawn, "stage_spawn")?;
            topology_advanced = true;
        }
        if topology_advanced {
            self.flow_engine.note_topology_spawn_boundary();
        }
        Ok(())
    }

    fn complete_orchestrator_spawn(&mut self, spawn_ticket: Option<u64>, context: &'static str) {
        let mut topology_advanced = false;
        if self.has_orchestrator {
            match self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteSpawn, "complete_spawn")
            {
                Ok(()) => {
                    topology_advanced = true;
                }
                Err(error) => {
                    if let Some(spawn_ticket) = spawn_ticket {
                        tracing::warn!(
                            spawn_ticket,
                            error = %error,
                            context,
                            "failed to reconcile orchestrator pending-spawn snapshot"
                        );
                    } else {
                        tracing::warn!(
                            error = %error,
                            context,
                            "failed to reconcile orchestrator pending-spawn snapshot"
                        );
                    }
                }
            }
        }
        if topology_advanced {
            self.flow_engine.note_topology_spawn_boundary();
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

    fn fallback_spawn_prompt(&self, profile_name: &ProfileName, meerkat_id: &MeerkatId) -> String {
        format!(
            "You have been spawned as '{}' (role: {}) in mob '{}'.",
            meerkat_id, profile_name, self.definition.id
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
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
        prompt: meerkat_core::types::ContentInput,
    ) -> Result<(), MobError> {
        self.ensure_autonomous_runtime_ready(meerkat_id, member_ref)
            .await?;

        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{meerkat_id}' must be session-backed"
            ))
        })?;

        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{meerkat_id}' requires admission-capable substrate (runtime adapter)"
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
                        "autonomous prompt admission failed for '{meerkat_id}': {e}"
                    ))
                })?;

            // Spawn background task for completion wait.
            let log_id = meerkat_id.clone();
            let completion_command_tx = self.command_tx.clone();
            let handle = tokio::spawn(async move {
                if let Some(h) = completion_handle {
                    let outcome = h.wait().await;
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if completion_command_tx
                        .send(MobCommand::KickoffOutcomeResolved {
                            meerkat_id: log_id.clone(),
                            outcome,
                            ack_tx,
                        })
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            meerkat_id = %log_id,
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
                .insert(meerkat_id.clone(), InitialTurnHandle { handle });
        }

        tracing::debug!(meerkat_id = %meerkat_id, "autonomous member started");
        Ok(())
    }

    async fn ensure_autonomous_runtime_ready(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Session registration + RuntimeLoop attachment is owned by the
        // provisioner's lazy `runtime_session_state()` init (called during
        // provision_member). stop_autonomous_member preserves registration
        // (only aborts the drain), so resume just needs to re-spawn the drain.
        #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
        {
            let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
                MobError::Internal(format!(
                    "autonomous member '{meerkat_id}' must be session-backed for runtime readiness"
                ))
            })?;

            if let Some(adapter) = self.runtime_adapter.clone() {
                let comms_runtime = self.provisioner.comms_runtime(member_ref).await;
                let spawned = adapter
                    .maybe_spawn_comms_drain(bridge_session_id, true, comms_runtime)
                    .await;
                if spawned {
                    tracing::debug!(
                        meerkat_id = %meerkat_id,
                        bridge_session_id = %bridge_session_id,
                        "updated peer ingress for autonomous member"
                    );
                }
            }
        }

        self.ensure_autonomous_dispatch_capability(meerkat_id, member_ref)
            .await
    }

    async fn teardown_autonomous_runtime(&self, member_ref: &MemberRef) {
        #[cfg(feature = "runtime-adapter")]
        if let (Some(adapter), Some(bridge_session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        {
            adapter.unregister_session(bridge_session_id).await;
        }
    }

    async fn ensure_autonomous_dispatch_capability_for_provisioner(
        provisioner: &Arc<dyn MobProvisioner>,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let bridge_session_id = member_ref.bridge_session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{meerkat_id}' must be session-backed for injector dispatch"
            ))
        })?;
        if provisioner
            .interaction_event_injector(bridge_session_id)
            .await
            .is_none()
        {
            return Err(MobError::Internal(format!(
                "autonomous member '{meerkat_id}' is missing event injector capability"
            )));
        }
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    async fn resolve_kickoff_outcome(
        &mut self,
        meerkat_id: &MeerkatId,
        outcome: meerkat_runtime::completion::CompletionOutcome,
    ) -> Result<(), MobError> {
        if let meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } =
            &outcome
        {
            tracing::debug!(
                meerkat_id = %meerkat_id,
                tool_name = %tool_name,
                args = ?args,
                "autonomous kickoff reached callback-pending boundary"
            );
        }

        let _ = self
            .apply_kickoff_input(
                meerkat_id,
                mob_dsl::MobMachineInput::KickoffResolveOutcome {
                    member_id: meerkat_id.to_string(),
                    outcome: Self::kickoff_outcome_string(&outcome),
                },
            )
            .await?;
        Ok(())
    }

    async fn maybe_mark_kickoff_cancelled(&mut self, meerkat_id: &MeerkatId) {
        if let Err(error) = self
            .apply_kickoff_input(
                meerkat_id,
                mob_dsl::MobMachineInput::KickoffCancelRequested {
                    member_id: meerkat_id.to_string(),
                },
            )
            .await
        {
            tracing::warn!(
                meerkat_id = %meerkat_id,
                error = %error,
                "failed to apply kickoff cancellation transition"
            );
        }
    }

    async fn ensure_autonomous_dispatch_capability(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        Self::ensure_autonomous_dispatch_capability_for_provisioner(
            &self.provisioner,
            meerkat_id,
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
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Abort any in-flight initial turn.
        let had_kickoff_handle = self
            .autonomous_initial_turns
            .lock()
            .await
            .contains_key(meerkat_id);
        if had_kickoff_handle {
            self.maybe_mark_kickoff_cancelled(meerkat_id).await;
        }
        if let Some(handle) = self
            .autonomous_initial_turns
            .lock()
            .await
            .remove(meerkat_id)
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
                meerkat_id = %meerkat_id,
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
            if let Err((meerkat_id, error)) = result {
                tracing::warn!(
                    meerkat_id = %meerkat_id,
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
        self.stop_autonomous_member(&entry.meerkat_id, &entry.member_ref)
            .await
            .map_err(|error| (entry.meerkat_id, error))
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
                        && !broken_members.contains(&entry.meerkat_id)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        if entries.is_empty() {
            return Ok(());
        }

        let mut first_error: Option<MobError> = None;
        for entry in &entries {
            if let Err(error) = self
                .ensure_autonomous_runtime_ready(&entry.meerkat_id, &entry.member_ref)
                .await
            {
                tracing::warn!(
                    meerkat_id = %entry.meerkat_id,
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
                    if let Err(error) = self.require_mob_machine_spawn() {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
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
                    self.handle_spawn_provisioned_batch(completions).await;
                }
                MobCommand::Retire {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Stopped]) {
                        Ok(()) => {
                            let canceled = self.cancel_pending_spawns_for_member(
                                &meerkat_id,
                                "retire command received",
                            );
                            if canceled > 0 {
                                tracing::info!(
                                    meerkat_id = %meerkat_id,
                                    canceled,
                                    "retire canceled pending spawn lineage before roster retirement"
                                );
                            }
                            self.handle_retire(meerkat_id).await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Respawn {
                    meerkat_id,
                    initial_message,
                    reply_tx,
                } => {
                    let result = match self.require_mob_machine_spawn() {
                        Ok(()) => Box::pin(self.handle_respawn(meerkat_id, initial_message)).await,
                        Err(error) => Err(super::handle::MobRespawnError::from(error)),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RetireAll { reply_tx } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Stopped]) {
                        Ok(()) => self.retire_all_members("retire_all").await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_wire(local, target).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire {
                    local,
                    target,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_unwire(local, target).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ExternalTurn {
                    meerkat_id,
                    content,
                    handling_mode,
                    render_metadata,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => {
                            Box::pin(self.handle_external_turn(
                                meerkat_id,
                                content,
                                handling_mode,
                                render_metadata,
                            ))
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::InternalTurn {
                    meerkat_id,
                    content,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_internal_turn(meerkat_id, content).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                #[cfg(feature = "runtime-adapter")]
                MobCommand::KickoffOutcomeResolved {
                    meerkat_id,
                    outcome,
                    ack_tx,
                } => {
                    if let Err(error) = self.resolve_kickoff_outcome(&meerkat_id, outcome).await {
                        tracing::warn!(
                            meerkat_id = %meerkat_id,
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
                    let result = match self.require_mob_machine_run_flow() {
                        Ok(()) => {
                            self.handle_run_flow(flow_id, activation_params, scoped_event_tx)
                                .await
                        }
                        Err(error) => Err(error),
                    };
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
                MobCommand::Stop { reply_tx } => {
                    let result = match self.require_mob_machine_stop() {
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
                                let mut topology_unbound = false;
                                if self.has_orchestrator {
                                    match self.apply_dsl_signal(
                                        mob_dsl::MobMachineSignal::StopOrchestrator,
                                        "stop_orchestrator",
                                    ) {
                                        Ok(()) => {
                                            topology_unbound = true;
                                        }
                                        Err(error) => {
                                            stop_result = Err(MobError::Internal(format!(
                                                "orchestrator StopOrchestrator transition failed during stop: {error}"
                                            )));
                                        }
                                    }
                                }
                                if topology_unbound {
                                    self.flow_engine.unbind_topology_coordinator();
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
                    let result = match self.require_mob_machine_resume() {
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
                                let mut topology_bound = false;
                                if self.has_orchestrator {
                                    match self.apply_dsl_signal(
                                        mob_dsl::MobMachineSignal::ResumeOrchestrator,
                                        "resume_orchestrator",
                                    ) {
                                        Ok(()) => {
                                            topology_bound = true;
                                        }
                                        Err(error) => {
                                            resume_result = Err(MobError::Internal(format!(
                                                "orchestrator ResumeOrchestrator transition failed during resume: {error}"
                                            )));
                                        }
                                    }
                                }
                                if topology_bound {
                                    self.flow_engine.bind_topology_coordinator();
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
                    let result = match self.require_mob_machine_complete() {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is completing").await;
                            self.handle_complete().await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    if let Err(error) = self.require_mob_machine_destroy() {
                        let _ = reply_tx.send(Err(super::handle::MobDestroyError::Mob(error)));
                        continue;
                    }
                    // Reject before side effects like pending-spawn failure if the
                    // DSL authority would not accept Destroy in the current phase.
                    {
                        let mut probe = mob_dsl::MobMachineAuthority::from_state(
                            self.dsl_authority.state.clone(),
                        );
                        if mob_dsl::MobMachineMutator::apply(
                            &mut probe,
                            mob_dsl::MobMachineInput::Destroy,
                        )
                        .is_err()
                        {
                            let _ = reply_tx.send(Err(super::handle::MobDestroyError::Mob(
                                MobError::InvalidTransition {
                                    from: self.state(),
                                    to: MobState::Destroyed,
                                },
                            )));
                            continue;
                        }
                    }
                    self.fail_all_pending_spawns("mob is destroying").await;
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
                    if let Err(error) = self.require_mob_machine_reset() {
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
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = async {
                        let entry = self
                            .roster
                            .read()
                            .await
                            .entry(&meerkat_id)
                            .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?;
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
                                "failed to subscribe to agent events for '{meerkat_id}': {e}"
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
                                    entry.meerkat_id
                                ))
                            })?;
                            streams.push((entry.meerkat_id.clone(), stream));
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
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
                        Ok(()) => self.handle_force_cancel(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RealtimeAttach {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
                        Ok(()) => self.handle_realtime_attach(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RealtimeDetach {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
                        Ok(()) => self.handle_realtime_detach(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                    self.spawn_policy.set(policy).await;
                    let _ = reply_tx.send(());
                }
                MobCommand::Shutdown { reply_tx } => {
                    if let Err(error) = self.require_mob_machine_shutdown() {
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
                    if self.state() == MobState::Running
                        && let Err(error) =
                            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "shutdown_stop")
                    {
                        tracing::warn!(error = %error, "authority rejected Stop");
                        if result.is_ok() {
                            result = Err(MobError::Internal(format!(
                                "lifecycle Stop transition failed during shutdown: {error}"
                            )));
                        }
                    }
                    let _ = reply_tx.send(result);
                    break;
                }
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
            let meerkat_id = slot.spawn.meerkat_id.clone();
            self.complete_orchestrator_spawn(
                Some(spawn_ticket),
                "lifecycle transition cleared pending spawn",
            );
            self.abort_pending_spawn_slot(&slot, reason).await;
            slot.fail(&format!("spawn canceled for '{meerkat_id}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                meerkat_id = %meerkat_id,
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
                meerkat_id = %slot.spawn.meerkat_id,
                operation_id = %operation_id,
                error = %error,
                "failed to abort pending member provision during lifecycle drain"
            );
        }
    }

    fn cancel_pending_spawns_for_member(&mut self, meerkat_id: &MeerkatId, reason: &str) -> usize {
        let slots = self.pending_spawns.take_for_member(meerkat_id);
        if slots.is_empty() {
            if let Some(message) = self.pending_spawn_alignment_violation() {
                tracing::error!(
                    meerkat_id = %meerkat_id,
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
                "member lifecycle command canceled pending spawn",
            );
        }

        let pending_abortions = slots
            .iter()
            .map(|slot| {
                (
                    slot.ticket,
                    slot.spawn.meerkat_id.clone(),
                    slot.spawn.progress.clone(),
                )
            })
            .collect::<Vec<_>>();
        let provisioner = self.provisioner.clone();
        let reason_owned = reason.to_string();
        tokio::spawn(async move {
            for (spawn_ticket, meerkat_id, progress) in pending_abortions {
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
                        meerkat_id = %meerkat_id,
                        operation_id = %operation_id,
                        error = %error,
                        "failed to abort pending member provision during member-specific cancellation"
                    );
                }
            }
        });

        for slot in slots {
            let spawn_ticket = slot.ticket;
            slot.fail(&format!("spawn canceled for '{meerkat_id}': {reason}"));
            tracing::debug!(
                spawn_ticket,
                meerkat_id = %meerkat_id,
                "canceled pending spawn for member lifecycle command"
            );
        }

        self.debug_assert_pending_spawn_alignment();
        if let Some(message) = self.pending_spawn_alignment_violation() {
            tracing::error!(
                meerkat_id = %meerkat_id,
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
        } = spec;
        let meerkat_id = MeerkatId::from(identity.as_str());
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
            if meerkat_id
                .as_str()
                .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{meerkat_id}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
                )));
            }
            tracing::debug!(
                mob_id = %self.definition.id,
                meerkat_id = %meerkat_id,
                profile = %profile_name,
                "MobActor::enqueue_spawn start"
            );

            if self.pending_spawns.contains_member(&meerkat_id) {
                return Err(MobError::MemberAlreadyExists(meerkat_id.clone()));
            }

            {
                let roster = self.roster.read().await;
                if roster.get(&meerkat_id).is_some() {
                    return Err(MobError::MemberAlreadyExists(meerkat_id.clone()));
                }
                if roster
                    .list()
                    .any(|entry| entry.external_peer_specs.contains_key(&meerkat_id))
                {
                    return Err(MobError::WiringError(format!(
                        "meerkat id '{meerkat_id}' collides with an existing external peer name"
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
                            "resume bridge session check failed for '{meerkat_id}': {e}"
                        ))
                    })?;
                if is_active.unwrap_or(false) {
                    // Validate event injector for autonomous mode.
                    if selected_runtime_mode == crate::MobRuntimeMode::AutonomousHost
                        && self.provisioner.interaction_event_injector(&resume_id).await.is_none()
                    {
                        return Err(MobError::Internal(format!(
                            "resumed session '{resume_id}' has no event injector for autonomous '{meerkat_id}'"
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
                            "resumed session '{resume_id}' has no comms runtime for '{meerkat_id}'"
                        )));
                    }

                    let prompt = initial_message.clone().unwrap_or_else(|| {
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &meerkat_id))
                    });
                    let resolved_labels = labels.unwrap_or_default();

                    return Ok((
                        profile_name,
                        meerkat_id,
                        prompt,
                        selected_runtime_mode,
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
                                meerkat_id: &meerkat_id,
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
                        ContentInput::from(self.fallback_spawn_prompt(&profile_name, &meerkat_id))
                    });
                    let req = build::to_create_session_request(&config, prompt.clone());
                    let selected_binding = resolve_binding(
                        binding.clone(),
                        backend,
                        profile.backend,
                        self.definition.backend.default,
                        &meerkat_id,
                    )?;
                    let selected_runtime_mode =
                        normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
                    let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
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
                        meerkat_id,
                        prompt,
                        selected_runtime_mode,
                        resolved_labels,
                        None::<MemberRef>,
                        Some(provision_request),
                        owner_bridge_session_id.clone(),
                        auto_wire_parent,
                        effective_profile_override.clone(),
                    ));
                }

                return Err(MobError::Internal(format!(
                    "resumed session '{resume_id}' not found or inactive for '{meerkat_id}'"
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
                meerkat_id: &meerkat_id,
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

            let base_prompt = initial_message.clone().unwrap_or_else(|| {
                ContentInput::from(self.fallback_spawn_prompt(&profile_name, &meerkat_id))
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
                &meerkat_id,
            )?;
            let selected_runtime_mode =
                normalize_runtime_mode_for_binding(selected_runtime_mode, &selected_binding);
            let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
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
                meerkat_id,
                prompt,
                selected_runtime_mode,
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
            meerkat_id,
            prompt,
            selected_runtime_mode,
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
                        "resumed member '{meerkat_id}' has no tracked mob child operation"
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
                PendingProvision::new(member_ref, meerkat_id.clone(), self.provisioner.clone());
            // Go straight to finalization — no async provisioning task needed.
            let fence = self.issue_fence_token();
            let result = self
                .finalize_spawn_from_pending(
                    &profile_name,
                    &meerkat_id,
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
                    false,
                )
                .await
                .map(|outcome| outcome.receipt);
            let _ = reply_tx.send(result);
            return;
        }

        // Normal provisioning path — resume path already returned above.
        let Some(provision_request) = maybe_provision_request else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "provision_request missing for normal spawn path".into(),
            )));
            return;
        };

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        let spawn_meerkat_id = meerkat_id.clone();
        let spawn_meerkat_id_for_log = spawn_meerkat_id.clone();
        let spawn_runtime_mode = selected_runtime_mode;
        let pending_progress = Arc::new(std::sync::Mutex::new(PendingSpawnProgress::default()));

        if let Err(error) = self.stage_orchestrator_spawn() {
            let _ = reply_tx.send(Err(error));
            return;
        }

        let pending = PendingSpawn {
            profile_name,
            meerkat_id,
            prompt,
            runtime_mode: selected_runtime_mode,
            labels: resolved_labels,
            owner_bridge_session_id: spawn_owner_bridge_session_id,
            auto_wire_parent,
            restore_wiring: None,
            effective_profile_override,
            voice_intent_present: false,
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
            meerkat_id = %spawn_meerkat_id_for_log,
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
                meerkat_id,
                prompt,
                runtime_mode,
                labels,
                owner_bridge_session_id,
                auto_wire_parent,
                restore_wiring,
                effective_profile_override,
                voice_intent_present,
                progress: _,
                reply_tx,
            } = pending;
            let reply = match result {
                Ok(spawn_receipt) => {
                    let provision = PendingProvision::new(
                        spawn_receipt.member_ref.clone(),
                        meerkat_id.clone(),
                        self.provisioner.clone(),
                    );
                    if let Err(error) = self.require_state(&[MobState::Running]) {
                        if let Err(retire_error) = provision.rollback().await {
                            Err(MobError::Internal(format!(
                                "spawn completed while mob state changed for '{meerkat_id}': {error}; cleanup retire failed: {retire_error}"
                            )))
                        } else {
                            Err(error)
                        }
                    } else {
                        let fence = self.issue_fence_token();
                        self.finalize_spawn_from_pending(
                            &profile_name,
                            &meerkat_id,
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
                            voice_intent_present,
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
        meerkat_id: &MeerkatId,
        spawn_spec: super::spawn_policy::SpawnSpec,
    ) -> Result<super::handle::MemberSpawnReceipt, MobError> {
        self.ensure_pending_spawn_alignment("spawn_from_policy_inline preflight")?;

        if meerkat_id
            .as_str()
            .starts_with(FLOW_SYSTEM_MEMBER_ID_PREFIX)
        {
            return Err(MobError::WiringError(format!(
                "meerkat id '{meerkat_id}' uses reserved system prefix '{FLOW_SYSTEM_MEMBER_ID_PREFIX}'"
            )));
        }
        if self.pending_spawns.contains_member(meerkat_id) {
            return Err(MobError::MemberAlreadyExists(meerkat_id.clone()));
        }
        {
            let roster = self.roster.read().await;
            if roster.get(meerkat_id).is_some() {
                return Err(MobError::MemberAlreadyExists(meerkat_id.clone()));
            }
            if roster
                .list()
                .any(|entry| entry.external_peer_specs.contains_key(meerkat_id))
            {
                return Err(MobError::WiringError(format!(
                    "meerkat id '{meerkat_id}' collides with an existing external peer name"
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
            meerkat_id,
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

        let prompt = ContentInput::from(self.fallback_spawn_prompt(&profile_name, meerkat_id));
        let req = build::to_create_session_request(&config, prompt.clone());
        let selected_binding = resolve_binding(
            None,
            None,
            profile.backend,
            self.definition.backend.default,
            meerkat_id,
        )?;
        let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
        let provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: selected_binding,
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
        };

        let spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        self.stage_orchestrator_spawn()?;
        let (pending_reply_tx, _pending_reply_rx) = oneshot::channel();
        let pending = PendingSpawn {
            profile_name: profile_name.clone(),
            meerkat_id: meerkat_id.clone(),
            prompt: prompt.clone(),
            runtime_mode,
            labels: labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: None,
            effective_profile_override: None,
            voice_intent_present: false,
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
                meerkat_id = %meerkat_id,
                error = %error,
                "pending spawn alignment violated while staging inline policy spawn"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated while staging inline policy spawn",
            )
            .await;
            return Err(error);
        }

        let spawn_result = async {
            let spawn_receipt = self.provisioner.provision_member(provision_request).await?;
            if runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        meerkat_id,
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
                        "autonomous capability check failed for '{meerkat_id}': {capability_error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(capability_error);
            }
            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                meerkat_id.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "policy spawn completed while mob state changed for '{meerkat_id}': {error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(error);
            }
            let fence = self.issue_fence_token();
            self.finalize_spawn_from_pending(
                &profile_name,
                meerkat_id,
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
                false,
            )
            .await
            .map(|outcome| outcome.receipt)
        }
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
                meerkat_id = %meerkat_id,
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
        meerkat_id: &MeerkatId,
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
        voice_intent_present: bool,
    ) -> Result<FinalizeSpawnOutcome, MobError> {
        let identity = crate::ids::AgentIdentity::from(meerkat_id.as_str());
        let agent_runtime_id = crate::ids::AgentRuntimeId::new(identity.clone(), generation);
        let overlay_record =
            self.external_binding_overlay_record(&identity, generation, provision.member_ref());
        if let Some(overlay_record) = overlay_record.as_ref() {
            self.runtime_metadata
                .upsert_external_binding_overlay(&self.definition.id, overlay_record)
                .await?;
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
            if let Err(rollback_error) = provision.rollback().await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed: {rollback_error}"
                )));
            }
            return Err(MobError::from(append_error));
        }

        // Commit the provision: the member is now owned by the roster.
        // From this point, rollback_failed_spawn handles cleanup via the
        // disposal pipeline.
        let member_ref = provision.commit()?;
        let peer_id = self
            .provisioner_comms(&member_ref)
            .await
            .and_then(|comms| comms.public_key());

        {
            let mut roster = self.roster.write().await;
            let inserted = roster.add_member(crate::roster::RosterAddEntry {
                agent_identity: identity.clone(),
                generation,
                fence_token,
                agent_runtime_id,
                meerkat_id: meerkat_id.clone(),
                role: profile_name.clone(),
                runtime_mode,
                member_ref: member_ref.clone(),
                peer_id,
                labels,
                effective_profile_override,
                voice_intent_present,
            });
            debug_assert!(
                inserted,
                "duplicate meerkat insert should be prevented before add()"
            );
        }
        self.restore_diagnostics.write().await.remove(meerkat_id);
        #[cfg(feature = "runtime-adapter")]
        self.restore_realtime_attachment_intent_if_needed(
            &member_ref,
            runtime_mode,
            voice_intent_present,
        )
        .await;
        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    meerkat_id,
                    mob_dsl::MobMachineInput::KickoffMarkPending {
                        member_id: meerkat_id.to_string(),
                    },
                )
                .await?;
        }
        tracing::debug!(
            meerkat_id = %meerkat_id,
            "MobActor::finalize_spawn_from_pending roster updated"
        );

        let planned_wiring_targets = self.spawn_wiring_targets(profile_name, meerkat_id).await;

        let (wired_spawn_targets, wiring_error) = self
            .apply_spawn_wiring(meerkat_id, &planned_wiring_targets)
            .await;
        if let Some(wiring_error) = wiring_error {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    meerkat_id,
                    profile_name,
                    &member_ref,
                    &wired_spawn_targets,
                    &planned_wiring_targets,
                )
                .await
            {
                return Err(MobError::Internal(format!(
                    "spawn wiring failed for '{meerkat_id}': {wiring_error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(wiring_error);
        }

        #[cfg(feature = "runtime-adapter")]
        if runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            let _ = self
                .apply_kickoff_input(
                    meerkat_id,
                    mob_dsl::MobMachineInput::KickoffMarkStarting {
                        member_id: meerkat_id.to_string(),
                    },
                )
                .await?;
            if let Err(start_error) = self
                .start_autonomous_member(meerkat_id, &member_ref, prompt)
                .await
            {
                self.clear_kickoff_state(meerkat_id).await;
                if let Err(rollback_error) = self
                    .rollback_failed_spawn(
                        meerkat_id,
                        profile_name,
                        &member_ref,
                        &wired_spawn_targets,
                        &planned_wiring_targets,
                    )
                    .await
                {
                    return Err(MobError::Internal(format!(
                        "spawn host-loop start failed for '{meerkat_id}': {start_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(start_error);
            }
        }

        // auto_wire_parent: wire to the actual spawner member when the spawn
        // request came from a session-owned mob tool call.
        if auto_wire_parent
            && let Some(parent_id) = self
                .resolve_auto_wire_parent_target(owner_bridge_session_id.as_ref(), meerkat_id)
                .await
            && let Err(error) = self.do_wire(meerkat_id, &parent_id).await
        {
            tracing::warn!(
                error = %error,
                peer = %parent_id,
                "auto_wire_parent: failed to wire to spawning member"
            );
        }

        // Restore peer wiring from a prior respawn.
        let mut failed_restore_peer_ids = Vec::new();
        if let Some(mut wiring) = restore_wiring {
            wiring.local_peers.sort();
            wiring.local_peers.dedup();
            for peer_id in wiring
                .local_peers
                .into_iter()
                .filter(|peer_id| peer_id != meerkat_id)
            {
                if let Err(e) = self.do_wire(meerkat_id, &peer_id).await {
                    tracing::warn!(
                        error = %e,
                        peer = %peer_id,
                        "failed to restore wiring after respawn"
                    );
                    failed_restore_peer_ids.push(peer_id.clone());
                }
            }
            wiring.external_peers.sort_by(|a, b| a.name.cmp(&b.name));
            wiring.external_peers.dedup_by(|a, b| a.name == b.name);
            for spec in wiring.external_peers {
                if spec.name == meerkat_id.as_str() {
                    continue;
                }
                if let Err(e) = self.do_wire_external(meerkat_id, &spec).await {
                    tracing::warn!(
                        error = %e,
                        peer = %spec.name,
                        "failed to restore external wiring after respawn"
                    );
                    failed_restore_peer_ids.push(MeerkatId::from(spec.name.clone()));
                }
            }
        }

        failed_restore_peer_ids.sort();
        failed_restore_peer_ids.dedup();

        tracing::debug!(
            meerkat_id = %meerkat_id,
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
        meerkat_id: &MeerkatId,
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
                            && !broken_members.contains(&entry.meerkat_id)
                    })
                    .map(|entry| entry.meerkat_id.clone())
                    .collect::<Vec<_>>()
            };
            for orchestrator_id in orchestrator_ids {
                if orchestrator_id != *meerkat_id && !targets.contains(&orchestrator_id) {
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
                                && !broken_members.contains(&entry.meerkat_id)
                                && entry.meerkat_id != *meerkat_id
                        })
                        .map(|entry| entry.meerkat_id.clone())
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
                    && entry.meerkat_id != *spawned_meerkat_id
                    && !broken_members.contains(&entry.meerkat_id)
                    && entry.member_ref.bridge_session_id() == Some(owner_bridge_session_id)
            })
            .map(|entry| entry.meerkat_id.clone())
    }

    /// P1-T05: retire() removes a meerkat.
    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Does NOT retire the member — the member remains in the roster and can
    /// receive new turns. Use [`handle_retire`] to fully remove a member.
    async fn handle_force_cancel(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let roster = self.roster.read().await;
        let entry = roster
            .get(&meerkat_id)
            .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?;
        let member_ref = entry.member_ref.clone();
        drop(roster);

        self.provisioner.interrupt_member(&member_ref).await
    }

    async fn handle_realtime_attach(&mut self, meerkat_id: MeerkatId) -> Result<bool, MobError> {
        self.ensure_pending_spawn_alignment("handle_realtime_attach preflight")?;
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?
        };

        if !entry.voice_intent_present {
            // DELETE_ME A4 + B6: apply the DSL RealtimeAttach input so
            // `member_voice_intent` in the MobMachine DSL state is the
            // canonical durable-intent fact. The previous shape
            // appended the event + mutated the roster directly, which
            // left the DSL field inert and created parallel truth
            // between DSL and roster. Per dogma principles #1 and #2,
            // MobMachine owns the intent; the roster becomes a
            // rebuildable projection (dogma #11).
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::RealtimeAttach {
                    agent_identity: mob_dsl::AgentIdentity::from(entry.agent_identity.as_str()),
                },
                "realtime_attach_input",
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "MobMachine RealtimeAttach transition rejected: {error}"
                ))
            })?;
            self.append_voice_intent_set_event(&meerkat_id).await?;
            self.roster
                .write()
                .await
                .set_voice_intent_by_identity(&entry.agent_identity, true);
        }
        #[cfg(feature = "runtime-adapter")]
        self.reconcile_realtime_attachment_runtime(&entry.member_ref, entry.runtime_mode, true)
            .await;
        self.ensure_pending_spawn_alignment("handle_realtime_attach completion")?;
        Ok(true)
    }

    async fn handle_realtime_detach(&mut self, meerkat_id: MeerkatId) -> Result<bool, MobError> {
        self.ensure_pending_spawn_alignment("handle_realtime_detach preflight")?;
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?
        };

        if entry.voice_intent_present {
            // DELETE_ME A4 + B6: see handle_realtime_attach above for
            // the canonical DSL-authority rationale. Detach clears
            // the same `member_voice_intent` field through the DSL
            // RealtimeDetach transition.
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::RealtimeDetach {
                    agent_identity: mob_dsl::AgentIdentity::from(entry.agent_identity.as_str()),
                },
                "realtime_detach_input",
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "MobMachine RealtimeDetach transition rejected: {error}"
                ))
            })?;
            self.append_voice_intent_cleared_event(&meerkat_id).await?;
            self.roster
                .write()
                .await
                .set_voice_intent_by_identity(&entry.agent_identity, false);
        }
        #[cfg(feature = "runtime-adapter")]
        self.reconcile_realtime_attachment_runtime(&entry.member_ref, entry.runtime_mode, false)
            .await;
        self.ensure_pending_spawn_alignment("handle_realtime_detach completion")?;
        Ok(true)
    }

    ///
    /// Mark-then-best-effort-cleanup: event first, mark Retiring, disposal
    /// pipeline (policy-driven), then unconditional roster removal.
    async fn handle_retire(&mut self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_retire preflight")?;
        self.handle_retire_inner(&meerkat_id, false, true).await?;
        self.ensure_pending_spawn_alignment("handle_retire completion")
    }

    async fn handle_retire_inner(
        &mut self,
        meerkat_id: &MeerkatId,
        bulk: bool,
        clear_voice_intent: bool,
    ) -> Result<(), MobError> {
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(meerkat_id).cloned() else {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    meerkat_id = %meerkat_id,
                    "retire requested for unknown meerkat id"
                );
                return Ok(());
            };
            entry
        };

        if clear_voice_intent && entry.voice_intent_present {
            self.append_voice_intent_cleared_event(meerkat_id).await?;
        }

        // Append retire event (event-first for crash recovery).
        let retire_event_already_present = self
            .retire_event_exists(meerkat_id, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, &entry.role, &entry.member_ref)
                .await?;
        }

        // Mark as Retiring (blocks re-spawn with same ID).
        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring(meerkat_id);
        }

        // Snapshot context and run disposal pipeline.
        let ctx = self
            .disposal_context_from_entry(meerkat_id, &entry, clear_voice_intent)
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
        meerkat_id: MeerkatId,
        initial_message: Option<ContentInput>,
    ) -> Result<super::handle::MemberRespawnReceipt, super::handle::MobRespawnError> {
        use super::handle::{MemberRespawnReceipt, MobRespawnError};

        self.ensure_pending_spawn_alignment("handle_respawn preflight")
            .map_err(MobRespawnError::from)?;

        let canceled = self.cancel_pending_spawns_for_member(
            &meerkat_id,
            "respawn command superseded pending spawn",
        );
        if canceled > 0 {
            tracing::info!(
                meerkat_id = %meerkat_id,
                canceled,
                "respawn canceled pending spawn lineage before replacement workflow"
            );
        }

        // 1. Snapshot all replacement inputs before retiring.
        let snapshot = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?;
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
                restore_wiring: RestoreWiringPlan {
                    local_peers: entry
                        .wired_to
                        .iter()
                        .filter_map(|peer_id| {
                            roster
                                .get_by_identity(peer_id)
                                .map(|e| e.meerkat_id.clone())
                        })
                        .collect(),
                    external_peers: entry.external_peer_specs.values().cloned().collect(),
                },
                binding,
                effective_profile_override: entry.effective_profile_override,
                voice_intent_present: entry.voice_intent_present,
            }
        };

        let replacement_generation = snapshot.generation.next();

        // 2. Retire the existing member (archives the session, removes from roster).
        // Respawn preserves voice intent: use the voice-preserving retire_inner
        // variant (clear_voice_intent=false) instead of handle_retire.
        if let Err(error) = self.handle_retire_inner(&meerkat_id, false, false).await {
            let roster_still_contains_member = {
                let roster = self.roster.read().await;
                roster.get(&meerkat_id).is_some()
            };
            if roster_still_contains_member {
                return Err(MobRespawnError::from(error));
            }
            let mut cleanup_report = super::handle::PreviousMemberCleanupReport {
                identity: AgentIdentity::from(meerkat_id.as_str()),
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
            ContentInput::from(self.fallback_spawn_prompt(&snapshot.profile_name, &meerkat_id))
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
            meerkat_id: &meerkat_id,
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
            self.definition.id, snapshot.profile_name, meerkat_id
        );
        let provision_request = ProvisionMemberRequest {
            create_session: req,
            binding: snapshot.binding.clone(),
            peer_name,
            owner_bridge_session_id: None,
            ops_registry: None,
        };

        let respawn_spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        self.stage_orchestrator_spawn()
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(meerkat_id.as_str()),
                reason: format!("failed to stage respawn replacement spawn: {error}"),
            })?;
        let (respawn_inline_reply_tx, _respawn_inline_reply_rx) = oneshot::channel();
        let respawn_pending = PendingSpawn {
            profile_name: snapshot.profile_name.clone(),
            meerkat_id: meerkat_id.clone(),
            prompt: prompt.clone(),
            runtime_mode: snapshot.runtime_mode,
            labels: snapshot.labels.clone(),
            owner_bridge_session_id: None,
            auto_wire_parent: false,
            restore_wiring: (!snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty())
            .then_some(snapshot.restore_wiring.clone()),
            effective_profile_override: snapshot.effective_profile_override.clone(),
            voice_intent_present: snapshot.voice_intent_present,
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
                meerkat_id = %meerkat_id,
                error = %error,
                "pending spawn alignment violated while staging respawn replacement"
            );
            self.fail_all_pending_spawns(
                "pending spawn alignment violated while staging respawn replacement",
            )
            .await;
            return Err(MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(meerkat_id.as_str()),
                reason: error.to_string(),
            });
        }

        // 4. Provision and finalize the replacement member inline so the receipt reflects
        //    the committed canonical member/session state before we return.
        let replacement_result: Result<super::handle::MemberSpawnReceipt, MobRespawnError> = async {
            let spawn_receipt = self
                .provisioner
                .provision_member(provision_request)
                .await
                .map_err(|error| MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(meerkat_id.as_str()),
                    reason: error.to_string(),
                })?;
            if snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                && let Err(capability_error) =
                    Self::ensure_autonomous_dispatch_capability_for_provisioner(
                        &self.provisioner,
                        &meerkat_id,
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
                        identity: AgentIdentity::from(meerkat_id.as_str()),
                        reason: format!(
                            "autonomous capability check failed: {capability_error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(meerkat_id.as_str()),
                    reason: capability_error.to_string(),
                });
            }

            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                meerkat_id.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        identity: AgentIdentity::from(meerkat_id.as_str()),
                        reason: format!(
                            "mob state changed before respawn finalization: {error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    identity: AgentIdentity::from(meerkat_id.as_str()),
                    reason: error.to_string(),
                });
            }

            if !snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty()
            {
                tracing::info!(
                    meerkat_id = %meerkat_id,
                    local_peers = ?snapshot.restore_wiring.local_peers,
                    external_peers = ?snapshot.restore_wiring.external_peers,
                    "respawn: restoring peer wiring during replacement finalization"
                );
            }

            let respawn_fence = self.issue_fence_token();
            let finalized = self
                .finalize_spawn_from_pending(
                &snapshot.profile_name,
                &meerkat_id,
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
                snapshot.voice_intent_present,
            )
            .await
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(meerkat_id.as_str()),
                reason: error.to_string(),
            })?;

            if finalized.failed_restore_peer_ids.is_empty() {
                Ok(finalized.receipt)
            } else {
                Err(MobRespawnError::TopologyRestoreFailed {
                    receipt: super::handle::MemberRespawnReceipt::new(
                        AgentIdentity::from(meerkat_id.as_str()),
                        crate::ids::AgentRuntimeId::new(
                            AgentIdentity::from(meerkat_id.as_str()),
                            replacement_generation,
                        ),
                        snapshot.old_fence_token,
                        respawn_fence,
                    ),
                    failed_peer_ids: finalized.failed_restore_peer_ids.into_iter().map(|mid| AgentIdentity::from(mid.as_str())).collect(),
                })
            }
        }
        .await;

        let (_respawn_pending, respawn_task) =
            self.complete_pending_spawn_slot(respawn_spawn_ticket, "respawn replacement spawn");
        if let Some(handle) = respawn_task {
            handle.abort();
        }
        self.ensure_pending_spawn_alignment("handle_respawn completion")
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                identity: AgentIdentity::from(meerkat_id.as_str()),
                reason: error.to_string(),
            })?;
        let _replacement = replacement_result?;

        // 5. Build the receipt from the committed replacement member reference.
        Ok(MemberRespawnReceipt::new(
            AgentIdentity::from(meerkat_id.as_str()),
            crate::ids::AgentRuntimeId::new(
                AgentIdentity::from(meerkat_id.as_str()),
                replacement_generation,
            ),
            snapshot.old_fence_token,
            self.roster
                .read()
                .await
                .get(&meerkat_id)
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
        meerkat_id: &MeerkatId,
        entry: &RosterEntry,
        clear_voice_intent: bool,
    ) -> DisposalContext {
        let retiring_key = self
            .provisioner_comms(&entry.member_ref)
            .await
            .and_then(|comms| comms.public_key());
        DisposalContext {
            meerkat_id: meerkat_id.clone(),
            entry: entry.clone(),
            retiring_key,
            clear_voice_intent,
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
            DisposalStep::RemoveTrustEdges => self.dispose_remove_trust_edges(ctx).await,
            DisposalStep::RealtimeDetach => self.dispose_realtime_detach(ctx).await,
            DisposalStep::ArchiveSession => self.dispose_archive_session(ctx).await,
        }
    }

    /// Stop the autonomous member and unregister session (disposal only).
    async fn dispose_stop_host_loop(&mut self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_member(&ctx.meerkat_id, &ctx.entry.member_ref)
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
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_identity,
                    "dispose_notify_peers: skipping absent peer"
                );
                continue;
            };
            let recipient_spec = match self
                .resolve_wiring_endpoint(&peer_entry, "dispose_notify_peers")
                .await?
            {
                WiringEndpoint::Local { spec, .. } | WiringEndpoint::PeerOnly { spec, .. } => spec,
            };

            if let Err(error) = self
                .notify_peer_retired(
                    &recipient_spec,
                    &ctx.meerkat_id,
                    &ctx.entry,
                    &retiring_comms,
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

    /// Remove the retiring member's trust edges from all wired peers.
    ///
    /// Iterates the full `wired_to` set; skips absent peers and peers
    /// missing comms.
    async fn dispose_remove_trust_edges(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        let retiring_peer_id = if let Some(retiring_key) = &ctx.retiring_key {
            retiring_key.clone()
        } else if let MemberRef::BackendPeer {
            peer_id,
            session_id: None,
            ..
        } = &ctx.entry.member_ref
        {
            peer_id.clone()
        } else {
            return Ok(());
        };
        let mut first_error: Option<MobError> = None;
        for peer_identity in &ctx.entry.wired_to {
            let peer_member_ref = {
                let roster = self.roster.read().await;
                roster
                    .get_by_identity(peer_identity)
                    .map(|e| e.member_ref.clone())
            };
            let Some(peer_member_ref) = peer_member_ref else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_identity,
                    "dispose_remove_trust_edges: skipping absent peer"
                );
                continue;
            };
            let Some(peer_comms) = self.provisioner_comms(&peer_member_ref).await else {
                continue;
            };
            if let Err(error) = peer_comms.remove_trusted_peer(&retiring_peer_id).await
                && first_error.is_none()
            {
                first_error = Some(error.into());
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Detach runtime-owned live voice mechanics before session archival.
    async fn dispose_realtime_detach(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        #[cfg(feature = "runtime-adapter")]
        {
            let (Some(adapter), Some(session_id)) = (
                &self.runtime_adapter,
                ctx.entry.member_ref.bridge_session_id(),
            ) else {
                return Ok(());
            };

            adapter.detach_live(session_id).await.map_err(|error| {
                MobError::Internal(format!("failed to detach live voice: {error}"))
            })?;

            if ctx.clear_voice_intent {
                adapter
                    .project_realtime_attachment_intent(session_id, false)
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to clear projected live voice intent during retire: {error}"
                        ))
                    })?;
            }
        }
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = ctx;

        Ok(())
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
        self.edge_locks.prune(ctx.meerkat_id.as_str()).await;
    }

    /// Remove the member from the roster. Infallible.
    pub(super) async fn dispose_remove_from_roster(&self, ctx: &DisposalContext) {
        let mut roster = self.roster.write().await;
        roster.remove_member(&ctx.meerkat_id);
        drop(roster);
        self.restore_diagnostics
            .write()
            .await
            .remove(&ctx.meerkat_id);
    }

    /// P1-T06: wire() establishes local or external trust.
    async fn handle_wire(
        &self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_wire preflight")?;
        self.ensure_member_not_broken(&local).await?;
        match target {
            super::handle::PeerTarget::Local(peer_identity) => {
                let peer = MeerkatId::from(peer_identity.as_str());
                if local == peer {
                    return Err(MobError::WiringError(format!(
                        "wire requires distinct members (got '{local}')"
                    )));
                }
                {
                    let roster = self.roster.read().await;
                    if roster.get(&local).is_none() {
                        return Err(MobError::MemberNotFound(local.clone()));
                    }
                    if roster.get(&peer).is_none() {
                        return Err(MobError::MemberNotFound(peer.clone()));
                    }
                }
                self.ensure_member_not_broken(&peer).await?;
                self.do_wire(&local, &peer).await?;
            }
            super::handle::PeerTarget::External(spec) => {
                self.do_wire_external(&local, &spec).await?;
            }
        }
        self.ensure_pending_spawn_alignment("handle_wire completion")
    }

    async fn do_wire_external(
        &self,
        local: &MeerkatId,
        spec: &TrustedPeerSpec,
    ) -> Result<(), MobError> {
        let external_name = MeerkatId::from(spec.name.clone());
        let _edge_guard = self
            .edge_locks
            .acquire(local.as_str(), external_name.as_str())
            .await;

        let (entry, already_wired, collides_with_local_member, stored_spec) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(local)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            let external_identity = AgentIdentity::from(external_name.as_str());
            let already_wired = entry.wired_to.contains(&external_identity);
            let collides_with_local_member = roster.get(&external_name).is_some();
            let stored_spec = entry.external_peer_specs.get(&external_name).cloned();
            (
                entry,
                already_wired,
                collides_with_local_member,
                stored_spec,
            )
        };

        let local_comms_name = self.comms_name_for(&entry);
        let plan = MobWiringAuthority::plan_external_wire(ExternalWireInput {
            local,
            local_state: entry.state,
            external_name: &external_name,
            local_comms_name: &local_comms_name,
            already_wired,
            collides_with_local_member,
            stored_spec: stored_spec.as_ref(),
            new_spec: spec,
        })?;
        if matches!(plan, ExternalWirePlan::NoOp) {
            return Ok(());
        }

        let comms = self
            .provisioner_comms(&entry.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{local}'"))
            })?;

        let mut rollback = LifecycleRollback::new("wire_external");
        comms.add_trusted_peer(spec.clone()).await?;
        let previous_spec = match plan {
            ExternalWirePlan::NoOp => None,
            ExternalWirePlan::EstablishOrUpdate { previous_spec } => previous_spec,
        };
        match previous_spec {
            Some(previous) if already_wired && previous != *spec => {
                let previous_for_rollback = previous.clone();
                rollback.defer(
                    format!("restore external trust '{local}' -> '{}'", previous.name),
                    {
                        let comms = comms.clone();
                        let new_peer_id = spec.peer_id.clone();
                        move || async move {
                            comms.remove_trusted_peer(&new_peer_id).await?;
                            comms
                                .add_trusted_peer(previous_for_rollback.clone())
                                .await?;
                            Ok(())
                        }
                    },
                );
                if previous.peer_id != spec.peer_id {
                    comms.remove_trusted_peer(&previous.peer_id).await?;
                }
            }
            _ => {
                rollback.defer(
                    format!("remove external trust '{local}' -> '{}'", spec.name),
                    {
                        let comms = comms.clone();
                        let peer_id = spec.peer_id.clone();
                        move || async move {
                            comms.remove_trusted_peer(&peer_id).await?;
                            Ok(())
                        }
                    },
                );
            }
        }

        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::ExternalPeerWired {
                    local: AgentIdentity::from(local.as_str()),
                    spec: spec.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(MobError::from(append_error)).await);
        }

        {
            let mut roster = self.roster.write().await;
            roster.wire_external_peer(local, &external_name, spec.clone());
        }

        Ok(())
    }

    /// P1-T07: unwire() removes local or external trust.
    async fn handle_unwire(
        &self,
        local: MeerkatId,
        target: super::handle::PeerTarget,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_unwire preflight")?;
        match target {
            super::handle::PeerTarget::Local(peer_identity) => {
                let peer = MeerkatId::from(peer_identity.as_str());
                if local == peer {
                    return Err(MobError::WiringError(format!(
                        "unwire requires distinct peers (got '{local}')"
                    )));
                }

                let (peer_exists, looks_external) = {
                    let roster = self.roster.read().await;
                    let local_entry = roster
                        .get(&local)
                        .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
                    let peer_exists = roster.get(&peer).is_some();
                    let peer_identity = AgentIdentity::from(peer.as_str());
                    let looks_external = !peer_exists
                        && (local_entry.wired_to.contains(&peer_identity)
                            || local_entry.external_peer_specs.contains_key(&peer));
                    (peer_exists, looks_external)
                };

                if !peer_exists && !looks_external {
                    // Peer is not in roster and has no external trace — unwire
                    // is trivially satisfied (idempotent no-op).
                    return Ok(());
                }

                if looks_external {
                    self.do_unwire_external(&local, &peer, None).await?;
                } else {
                    self.do_unwire_local(&local, &peer).await?;
                }
            }
            super::handle::PeerTarget::External(spec) => {
                let external_name = MeerkatId::from(spec.name.clone());
                self.do_unwire_external(&local, &external_name, Some(spec))
                    .await?;
            }
        }
        self.ensure_pending_spawn_alignment("handle_unwire completion")
    }

    async fn do_unwire_local(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let _edge_guard = self.edge_locks.acquire(a.as_str(), b.as_str()).await;

        // Look up both entries + current wiring relation under the edge lock.
        let (entry_a, entry_b, a_has_b_edge, b_has_a_edge) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(a.clone()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(b.clone()))?;
            let a_has_b_edge = ea.wired_to.contains(b.as_str());
            let b_has_a_edge = eb.wired_to.contains(a.as_str());
            (ea, eb, a_has_b_edge, b_has_a_edge)
        };

        if a_has_b_edge != b_has_a_edge {
            tracing::warn!(
                a = %a,
                b = %b,
                a_has_b_edge,
                b_has_a_edge,
                "unwire found non-canonical roster projection; applying full unwire path"
            );
        }
        let endpoint_a = self.resolve_wiring_endpoint(&entry_a, "unwire").await?;
        let endpoint_b = self.resolve_wiring_endpoint(&entry_b, "unwire").await?;

        match MobWiringAuthority::plan_local_unwire(a, b, a_has_b_edge, b_has_a_edge)? {
            LocalUnwirePlan::NoOp => {
                if let (
                    WiringEndpoint::Local { comms: comms_a, .. },
                    WiringEndpoint::Local { spec: spec_b, .. },
                ) = (&endpoint_a, &endpoint_b)
                {
                    let _ = comms_a.remove_trusted_peer(&spec_b.peer_id).await?;
                }
                match (&endpoint_a, &endpoint_b) {
                    (
                        WiringEndpoint::Local { spec: spec_a, .. },
                        WiringEndpoint::Local { comms: comms_b, .. },
                    ) => {
                        let _ = comms_b.remove_trusted_peer(&spec_a.peer_id).await?;
                    }
                    (
                        WiringEndpoint::PeerOnly { spec: spec_a, .. },
                        WiringEndpoint::Local { comms: comms_b, .. },
                    ) => {
                        let _ = comms_b.remove_trusted_peer(&spec_a.peer_id).await?;
                    }
                    _ => {}
                }
                self.edge_locks.remove(a.as_str(), b.as_str()).await;
                self.debug_assert_roster_edge_symmetric(a, b, "handle_unwire/noop")
                    .await;
                return Ok(());
            }
            LocalUnwirePlan::Remove => {}
        }

        match (&endpoint_a, &endpoint_b) {
            (
                WiringEndpoint::Local {
                    comms: comms_a,
                    spec: spec_a,
                    comms_name: _comms_name_a,
                    entry: entry_a,
                },
                WiringEndpoint::Local {
                    comms: comms_b,
                    spec: spec_b,
                    comms_name: _comms_name_b,
                    entry: entry_b,
                },
            ) => {
                let mut rollback = LifecycleRollback::new("unwire");

                self.notify_peer_unwired(spec_b, a, entry_a, comms_a)
                    .await?;
                rollback.defer(format!("compensating mob.peer_added '{a}' -> '{b}'"), {
                    let comms_a = comms_a.clone();
                    let spec_b = spec_b.clone();
                    let a = a.clone();
                    let entry_a = entry_a.clone();
                    move || async move {
                        self.notify_peer_added(&comms_a, &spec_b, &a, &entry_a)
                            .await
                    }
                });

                if let Err(second_notification_error) =
                    self.notify_peer_unwired(spec_a, b, entry_b, comms_b).await
                {
                    return Err(rollback.fail(second_notification_error).await);
                }
                rollback.defer(format!("compensating mob.peer_added '{b}' -> '{a}'"), {
                    let comms_b = comms_b.clone();
                    let spec_a = spec_a.clone();
                    let b = b.clone();
                    let entry_b = entry_b.clone();
                    move || async move {
                        self.notify_peer_added(&comms_b, &spec_a, &b, &entry_b)
                            .await
                    }
                });

                if let Err(remove_a_error) = comms_a.remove_trusted_peer(&spec_b.peer_id).await {
                    return Err(rollback.fail(remove_a_error.into()).await);
                }
                rollback.defer(format!("restore trust '{a}' -> '{b}'"), {
                    let comms_a = comms_a.clone();
                    let spec_b = spec_b.clone();
                    move || async move {
                        comms_a.add_trusted_peer(spec_b).await?;
                        Ok(())
                    }
                });

                if let Err(remove_b_error) = comms_b.remove_trusted_peer(&spec_a.peer_id).await {
                    return Err(rollback.fail(remove_b_error.into()).await);
                }
                rollback.defer(format!("restore trust '{b}' -> '{a}'"), {
                    let comms_b = comms_b.clone();
                    let spec_a = spec_a.clone();
                    move || async move {
                        comms_b.add_trusted_peer(spec_a).await?;
                        Ok(())
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersUnwired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::Local {
                    comms,
                    spec: spec_a,
                    entry: entry_a,
                    ..
                },
                WiringEndpoint::PeerOnly {
                    spec: spec_b,
                    binding: binding_b,
                },
            ) => {
                let mut rollback = LifecycleRollback::new("unwire");
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.notify_peer_unwired(spec_a, b, &entry_b, &supervisor_sender)
                    .await?;
                rollback.defer(format!("compensating mob.peer_added '{b}' -> '{a}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_a = spec_a.clone();
                    let entry_b = entry_b.clone();
                    let b = b.clone();
                    move || async move {
                        self.notify_peer_added(&supervisor_sender, &spec_a, &b, &entry_b)
                            .await
                    }
                });
                self.unwire_peer_only_recipient(
                    spec_b,
                    Some(binding_b),
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_unwired(spec_b, a, entry_a, &supervisor_sender)
                    .await?;
                rollback.defer(format!("compensating mob.peer_added '{a}' -> '{b}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_b = spec_b.clone();
                    let entry_a = entry_a.clone();
                    let a = a.clone();
                    move || async move {
                        self.notify_peer_added(&supervisor_sender, &spec_b, &a, &entry_a)
                            .await
                    }
                });
                if let Err(error) = comms.remove_trusted_peer(&spec_b.peer_id).await {
                    return Err(rollback.fail(error.into()).await);
                }
                rollback.defer(format!("restore trust '{a}' -> '{b}'"), {
                    let comms = comms.clone();
                    let spec_b = spec_b.clone();
                    move || async move {
                        comms.add_trusted_peer(spec_b).await?;
                        Ok(())
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersUnwired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::PeerOnly {
                    spec: spec_a,
                    binding: _binding_a,
                },
                WiringEndpoint::Local {
                    comms,
                    spec: spec_b,
                    entry: entry_b,
                    ..
                },
            ) => {
                let mut rollback = LifecycleRollback::new("unwire");
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.unwire_peer_only_recipient(
                    spec_b,
                    None,
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_unwired(spec_b, a, &entry_a, &supervisor_sender)
                    .await?;
                rollback.defer(format!("compensating mob.peer_added '{a}' -> '{b}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_b = spec_b.clone();
                    let entry_a = entry_a.clone();
                    let a = a.clone();
                    move || async move {
                        self.notify_peer_added(&supervisor_sender, &spec_b, &a, &entry_a)
                            .await
                    }
                });
                self.notify_peer_unwired(spec_a, b, entry_b, &supervisor_sender)
                    .await?;
                rollback.defer(format!("compensating mob.peer_added '{b}' -> '{a}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_a = spec_a.clone();
                    let entry_b = entry_b.clone();
                    let b = b.clone();
                    move || async move {
                        self.notify_peer_added(&supervisor_sender, &spec_a, &b, &entry_b)
                            .await
                    }
                });
                if let Err(error) = comms.remove_trusted_peer(&spec_a.peer_id).await {
                    return Err(rollback.fail(error.into()).await);
                }
                rollback.defer(format!("restore trust '{b}' -> '{a}'"), {
                    let comms = comms.clone();
                    let spec_a = spec_a.clone();
                    move || async move {
                        comms.add_trusted_peer(spec_a).await?;
                        Ok(())
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersUnwired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::PeerOnly {
                    spec: spec_a,
                    binding: binding_a,
                },
                WiringEndpoint::PeerOnly {
                    spec: spec_b,
                    binding: binding_b,
                },
            ) => {
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.unwire_peer_only_recipient(
                    spec_a,
                    Some(binding_a),
                    spec_b,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_unwired(spec_a, b, &entry_b, &supervisor_sender)
                    .await?;
                self.unwire_peer_only_recipient(
                    spec_b,
                    Some(binding_b),
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_unwired(spec_b, a, &entry_a, &supervisor_sender)
                    .await?;
                self.events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersUnwired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await?;
            }
        }

        {
            let mut roster = self.roster.write().await;
            roster.unwire_members(a, b);
        }
        self.edge_locks.remove(a.as_str(), b.as_str()).await;
        self.debug_assert_roster_edge_symmetric(a, b, "handle_unwire/post")
            .await;

        self.ensure_pending_spawn_alignment("handle_unwire/local completion")
    }

    async fn do_unwire_external(
        &self,
        local: &MeerkatId,
        peer_name: &MeerkatId,
        spec_hint: Option<TrustedPeerSpec>,
    ) -> Result<(), MobError> {
        let _edge_guard = self
            .edge_locks
            .acquire(local.as_str(), peer_name.as_str())
            .await;

        let (entry, already_wired, stored_spec, collides_with_local_member) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(local)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(local.clone()))?;
            let peer_identity = AgentIdentity::from(peer_name.as_str());
            let already_wired = entry.wired_to.contains(&peer_identity);
            let stored_spec = entry.external_peer_specs.get(peer_name).cloned();
            let collides_with_local_member = roster.get(peer_name).is_some();
            (
                entry,
                already_wired,
                stored_spec,
                collides_with_local_member,
            )
        };

        let spec = match MobWiringAuthority::plan_external_unwire(
            local,
            peer_name,
            already_wired,
            stored_spec.as_ref(),
            collides_with_local_member,
            spec_hint.as_ref(),
        )? {
            ExternalUnwirePlan::NoOp => return Ok(()),
            ExternalUnwirePlan::Remove { spec } => spec,
        };

        let comms = self
            .provisioner_comms(&entry.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{local}'"))
            })?;

        let removed = comms.remove_trusted_peer(&spec.peer_id).await?;
        let mut rollback = LifecycleRollback::new("unwire_external");
        if removed || stored_spec.is_some() {
            rollback.defer(
                format!("restore external trust '{local}' -> '{}'", spec.name),
                {
                    let comms = comms.clone();
                    let spec = spec.clone();
                    move || async move {
                        comms.add_trusted_peer(spec.clone()).await?;
                        Ok(())
                    }
                },
            );
        }

        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::ExternalPeerUnwired {
                    local: AgentIdentity::from(local.as_str()),
                    peer_name: peer_name.to_string(),
                },
            })
            .await
        {
            return Err(rollback.fail(MobError::from(append_error)).await);
        }

        {
            let mut roster = self.roster.write().await;
            roster.unwire_external_peer(local, peer_name);
        }
        self.edge_locks
            .remove(local.as_str(), peer_name.as_str())
            .await;

        Ok(())
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
            }),
            _ => None,
        }
    }

    fn sanitized_member_ref(member_ref: &MemberRef) -> MemberRef {
        match member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id,
                ..
            } => MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: super::bridge_protocol::canonicalize_bridge_address(address),
                bootstrap_token: None,
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
        let meerkat_id = entry.meerkat_id.clone();
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

        match self
            .retire_event_exists(&meerkat_id, &entry.member_ref)
            .await
        {
            Ok(false) => {
                if let Err(error) = self
                    .append_retire_event(&meerkat_id, &entry.role, &entry.member_ref)
                    .await
                {
                    outcome
                        .errors
                        .push(format!("retire event append failed: {error}"));
                }
            }
            Ok(true) => {}
            Err(error) => outcome
                .errors
                .push(format!("retire event lookup failed: {error}")),
        }

        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring(&meerkat_id);
        }

        let ctx = self
            .disposal_context_from_entry(&meerkat_id, &entry, true)
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
        {
            let mut probe =
                mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
            if mob_dsl::MobMachineMutator::apply(&mut probe, mob_dsl::MobMachineInput::Destroy)
                .is_err()
            {
                return Err(MobDestroyError::from(MobError::InvalidTransition {
                    from: self.state(),
                    to: MobState::Destroyed,
                }));
            }
        }
        self.cancel_all_flow_tasks()
            .await
            .map_err(MobDestroyError::from)?;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is destroying.", self.definition.id))
            .await;
        let entries = {
            let roster = self.roster.read().await;
            roster.list_all().cloned().collect::<Vec<_>>()
        };
        let (remote_entries, local_entries): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|entry| Self::runtime_binding_for_entry(entry).is_some());

        for entry in local_entries {
            if let Err(error) = self
                .handle_retire_inner(&entry.meerkat_id, true, true)
                .await
            {
                report.push_error(error.to_string());
                return Err(MobDestroyError::Incomplete { report });
            }
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
        // Transition through StopOrchestrator then Destroy.
        if self.has_orchestrator {
            let mut topology_unbound = false;
            // Check if DSL allows StopOrchestrator
            {
                let mut probe =
                    mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
                if probe
                    .apply_signal(mob_dsl::MobMachineSignal::StopOrchestrator)
                    .is_ok()
                {
                    if let Err(error) = self.apply_dsl_signal(
                        mob_dsl::MobMachineSignal::StopOrchestrator,
                        "stop_orchestrator_destroy",
                    ) {
                        return Err(MobError::Internal(format!(
                            "orchestrator StopOrchestrator transition failed during destroy: {error}"
                        ))
                        .into());
                    }
                    topology_unbound = true;
                }
            }
            if topology_unbound {
                self.flow_engine.unbind_topology_coordinator();
            }
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::DestroyOrchestrator,
                "destroy_orchestrator",
            )
            .map_err(MobDestroyError::from)?;
        }
        self.apply_dsl_input(mob_dsl::MobMachineInput::Destroy, "destroy_input")
            .map_err(MobDestroyError::from)?;
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
            let next_sup_spec: super::bridge_protocol::BridgePeerSpec =
                meerkat_core::comms::TrustedPeerSpec::new(
                    format!("{}/__mob_supervisor__", self.definition.id),
                    next.public_peer_id.clone(),
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
            let mut rotated_peers: Vec<(TrustedPeerSpec, crate::RuntimeBinding)> = Vec::new();
            for binding in remote_bindings {
                let peer = Self::peer_only_spec_for_binding(&binding, "handle_rotate_supervisor")?;
                let mut effective_peer = peer.clone();
                let mut effective_binding = binding.clone();
                let authorize_result = self
                    .supervisor_bridge
                    .send_bridge_command(&peer, &next_command, std::time::Duration::from_secs(5))
                    .await;
                let authorize_error = match authorize_result {
                    Ok(value) => {
                        if let Some((cause, reason)) = Self::bridge_rejection_reason(&value) {
                            if super::bridge_fallback::should_fall_back_to_bind(cause) {
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
                                        };
                                        effective_peer = Self::peer_only_spec_for_binding(
                                            &effective_binding,
                                            "handle_rotate_supervisor rebound peer",
                                        )?;
                                        None
                                    }
                                    Err(bind_error) => Some(MobError::WiringError(format!(
                                        "{reason}; bind fallback failed: {bind_error}"
                                    ))),
                                }
                            } else {
                                Some(MobError::WiringError(reason))
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
                    let mut rollback_errors = Vec::new();
                    for (rotated_peer, rotated_binding) in rotated_peers.iter().rev() {
                        if let Err(rollback_error) = self
                            .bind_peer_only_member_for_binding(rotated_peer, rotated_binding)
                            .await
                        {
                            rollback_errors
                                .push(format!("{}: {rollback_error}", rotated_peer.peer_id));
                        }
                    }
                    let mut message = format!("failed to rotate supervisor authority: {error}");
                    if !rollback_errors.is_empty() {
                        message.push_str(&format!(
                            "; rollback failures: {}",
                            rollback_errors.join(", ")
                        ));
                    }
                    if !rollback_errors.is_empty() {
                        self.runtime_metadata
                            .put_supervisor_authority(&self.definition.id, &next)
                            .await?;
                        self.supervisor_bridge.rotate(next.clone()).await?;
                        message.push_str(
                            "; local supervisor authority advanced to the partially applied next authority to preserve deterministic recovery",
                        );
                    }
                    return Err(MobError::WiringError(message));
                }
                rotated_peers.push((effective_peer, effective_binding));
            }
        }
        let public_peer_id = next.public_peer_id.clone();
        self.runtime_metadata
            .put_supervisor_authority(&self.definition.id, &next)
            .await?;
        self.supervisor_bridge.rotate(next.clone()).await?;
        Ok(super::handle::SupervisorRotationReport {
            previous_epoch: current.epoch,
            current_epoch: next.epoch,
            public_peer_id,
        })
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

        // Best-effort Stop before reset — already stopped is fine.
        let _ = self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "reset_stop");

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

        let phase = self.state();
        // Ensure we're in Stopped state before orchestrator work
        if phase == MobState::Running {
            // Stop transition (requires no active runs and moves coordinator_bound=false)
            self.apply_dsl_input(mob_dsl::MobMachineInput::Stop, "stop_reset")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "lifecycle Stop transition failed during reset: {error}"
                    ))
                })?;
        } else if phase == MobState::Completed {
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::FinishCleanup, "finish_cleanup")
                .map_err(|error| {
                    MobError::Internal(format!(
                        "lifecycle FinishCleanup transition failed during reset from completed: {error}"
                    ))
                })?;
        }
        if self.has_orchestrator {
            let mut topology_unbound = false;
            // Check if DSL allows StopOrchestrator
            {
                let mut probe =
                    mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
                if probe
                    .apply_signal(mob_dsl::MobMachineSignal::StopOrchestrator)
                    .is_ok()
                {
                    self.apply_dsl_signal(
                        mob_dsl::MobMachineSignal::StopOrchestrator,
                        "stop_orchestrator_reset",
                    )?;
                    topology_unbound = true;
                }
            }
            if topology_unbound {
                self.flow_engine.unbind_topology_coordinator();
            }
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::ResumeOrchestrator,
                "resume_orchestrator_reset",
            )?;
            self.flow_engine.bind_topology_coordinator();
        }
        self.apply_dsl_input(mob_dsl::MobMachineInput::Resume, "resume_input_reset")
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle Resume transition failed during reset: {error}"
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
                .map(|entry| entry.meerkat_id.clone())
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
                    meerkat_id = %id,
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
        self.handle_retire_inner(&id, true, true)
            .await
            .map_err(|error| (id, error))
    }

    async fn handle_task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        self.task_board_service
            .create_task(subject, description, blocked_by)
            .await
    }

    async fn handle_task_update(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<AgentIdentity>,
    ) -> Result<(), MobError> {
        // TLA+ TaskBindingInvariant: owner must be a known identity (roster member).
        if let Some(ref owner_id) = owner {
            let roster = self.roster.read().await;
            let meerkat_id = MeerkatId::from(owner_id.as_str());
            if roster.get(&meerkat_id).is_none() {
                return Err(MobError::Internal(format!(
                    "TaskBindingInvariant violated: task owner '{owner_id}' is not in the roster",
                )));
            }
        }
        self.task_board_service
            .update_task(task_id, status, owner)
            .await
    }

    /// P1-T10: external_turn enforces addressability.
    ///
    /// When the target meerkat is not in the roster and a [`SpawnPolicy`] is
    /// set, the policy is consulted. If it resolves a [`SpawnSpec`], the
    /// member is auto-spawned and the message is delivered after provisioning
    /// completes.
    async fn handle_external_turn(
        &mut self,
        meerkat_id: MeerkatId,
        content: ContentInput,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_external_turn preflight")?;
        // Look up the entry
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&meerkat_id).cloned()
        };
        let entry = match entry {
            Some(e) => {
                if e.state != crate::roster::MemberState::Active {
                    return Err(MobError::MemberNotFound(meerkat_id));
                }
                self.ensure_member_not_broken(&e.meerkat_id).await?;
                e
            }
            None => {
                let agent_identity = AgentIdentity::from(meerkat_id.as_str());
                if let Some(spec) = self.spawn_policy.resolve(&agent_identity).await {
                    Box::pin(self.spawn_from_policy_inline(&meerkat_id, spec))
                        .await
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "auto-spawn failed for '{meerkat_id}': {error}"
                            ))
                        })?;
                    let spawned_entry = {
                        let roster = self.roster.read().await;
                        roster.get(&meerkat_id).cloned()
                    }
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "auto-spawned member '{meerkat_id}' missing from roster after completion"
                        ))
                    })?;
                    if spawned_entry.state != crate::roster::MemberState::Active {
                        return Err(MobError::Internal(format!(
                            "auto-spawned member '{meerkat_id}' is not active"
                        )));
                    }
                    spawned_entry
                } else {
                    return Err(MobError::MemberNotFound(meerkat_id));
                }
            }
        };

        // Check external_addressable
        let profile = if let Some(profile) = entry.effective_profile_override.clone() {
            profile
        } else {
            self.definition
                .resolve_profile(&entry.role, self.realm_profile_store.as_ref())
                .await?
        };

        if !profile.external_addressable {
            return Err(MobError::NotExternallyAddressable(meerkat_id));
        }

        self.dispatch_member_turn(&entry, content, handling_mode, render_metadata)
            .await
    }

    /// Internal-turn path bypasses external_addressable checks.
    async fn handle_internal_turn(
        &self,
        meerkat_id: MeerkatId,
        content: ContentInput,
    ) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_internal_turn preflight")?;
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(meerkat_id.clone()))?
        };
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MemberNotFound(meerkat_id));
        }
        self.ensure_member_not_broken(&entry.meerkat_id).await?;

        self.dispatch_member_turn(
            &entry,
            content,
            meerkat_core::types::HandlingMode::Queue,
            None,
        )
        .await
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

                self.ensure_autonomous_runtime_ready(&entry.meerkat_id, &entry.member_ref)
                    .await?;

                let injector = self
                    .provisioner
                    .interaction_event_injector(bridge_session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "missing event injector for autonomous member '{}'",
                            entry.meerkat_id
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
                            entry.meerkat_id, error
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
                    additional_instructions: None,
                    execution_kind: None,
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
        let run_id = self
            .flow_kernel
            .create_pending_run(&config, activation_params.clone())
            .await?;
        self.sync_dsl_roster_state().await;
        if self.has_orchestrator
            && let Err(error) =
                self.apply_dsl_signal(mob_dsl::MobMachineSignal::StartFlow, "start_flow")
        {
            let reason =
                format!("orchestrator StartFlow transition failed during flow admission: {error}");
            if let Err(terminalize_error) = self
                .flow_kernel
                .terminalize_failed(run_id.clone(), config.flow_id.clone(), reason.clone())
                .await
            {
                return Err(MobError::Internal(format!(
                    "{reason}; additionally failed to terminalize pending run: {terminalize_error}"
                )));
            }
            return Err(MobError::Internal(reason));
        }
        if let Err(error) = self.apply_dsl_signal(mob_dsl::MobMachineSignal::StartRun, "start_run")
        {
            let mut details = Vec::new();
            if self.has_orchestrator
                && let Err(rollback_error) = self.apply_dsl_signal(
                    mob_dsl::MobMachineSignal::CompleteFlow,
                    "complete_flow_rollback",
                )
            {
                details.push(format!(
                    "orchestrator CompleteFlow rollback failed: {rollback_error}"
                ));
            }
            if let Err(terminalize_error) = self
                .flow_kernel
                .terminalize_failed(
                    run_id.clone(),
                    config.flow_id.clone(),
                    format!("lifecycle StartRun transition failed during flow admission: {error}"),
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
        let flow_kernel = self.flow_kernel.clone();
        let flow_run_id = run_id.clone();
        let flow_id_for_task = config.flow_id.clone();
        let cleanup_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            let run_id_for_execute = flow_run_id.clone();
            if let Err(error) = engine
                .execute_flow(run_id_for_execute, config, activation_params, cancel_token)
                .await
            {
                tracing::error!(
                    run_id = %flow_run_id,
                    flow_id = %flow_id_for_task,
                    error = %error,
                    "flow task execution failed; delegating terminalization to flow-run kernel"
                );
                if let Err(finalize_error) = flow_kernel
                    .terminalize_failed(flow_run_id.clone(), flow_id_for_task, error.to_string())
                    .await
                {
                    tracing::error!(
                        run_id = %flow_run_id,
                        error = %finalize_error,
                        "failed to finalize run after flow task error"
                    );
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
            let _phase = self.state();
            let authorities_expect_completion = {
                let mut probe =
                    mob_dsl::MobMachineAuthority::from_state(self.dsl_authority.state.clone());
                probe
                    .apply_signal(mob_dsl::MobMachineSignal::CompleteFlow)
                    .is_ok()
                    || probe
                        .apply_signal(mob_dsl::MobMachineSignal::FinishRun)
                        .is_ok()
            };
            if authorities_expect_completion && !run_terminal {
                return Err(MobError::Internal(format!(
                    "{context}: received cleanup for run {run_id} with no local trackers while flow authorities still accept completion"
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

        let _ = self.run_tasks.remove(&run_id);
        let _ = self.run_cancel_tokens.remove(&run_id);
        let _ = self.flow_streams.lock().await.remove(&run_id);
        self.ensure_flow_tracker_alignment("handle_flow_cleanup completion")
            .await?;
        Ok(())
    }

    async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_cancel_flow preflight")?;
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
            self.flow_kernel.cancel_dispatched_steps(&run_id).await?;
            self.flow_kernel
                .terminalize_canceled(run_id.clone(), flow_id)
                .await?;
            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_flow_no_handle")
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

        let flow_kernel = self.flow_kernel.clone();
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
            if let Err(error) = flow_kernel.cancel_dispatched_steps(&run_id).await {
                tracing::error!(
                    error = %error,
                    "failed to settle dispatched steps before flow cancellation terminalization"
                );
            }
            if let Err(error) = flow_kernel.terminalize_canceled(run_id, flow_id).await {
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

            self.flow_kernel.cancel_dispatched_steps(&run_id).await?;
            self.flow_kernel
                .terminalize_canceled(run_id.clone(), flow_id.clone())
                .await?;

            self.apply_dsl_signal(mob_dsl::MobMachineSignal::CompleteFlow, "cancel_all_flow")?;

            let _ = self.run_cancel_tokens.remove(&run_id);
        }
        self.ensure_flow_tracker_alignment("cancel_all_flow_tasks completion")
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Apply auto/role wiring for a newly spawned meerkat.
    ///
    /// `wiring_targets` is expected to be deduplicated by `spawn_wiring_targets()`.
    /// The actor keeps command ordering, but per-edge wire effects run with bounded
    /// parallelism to reduce spawn fan-out latency.
    async fn apply_spawn_wiring(
        &self,
        meerkat_id: &MeerkatId,
        wiring_targets: &[MeerkatId],
    ) -> (Vec<MeerkatId>, Option<MobError>) {
        if wiring_targets.is_empty() {
            return (Vec::new(), None);
        }

        const MAX_PARALLEL_SPAWN_WIRES: usize = 8;
        let mut in_flight = FuturesUnordered::new();
        let mut remaining = wiring_targets.iter().cloned();
        let mut successful_targets = Vec::new();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_SPAWN_WIRES {
            let Some(target_id) = remaining.next() else {
                break;
            };
            in_flight.push(self.wire_spawn_target(meerkat_id, target_id));
        }

        while let Some(result) = in_flight.next().await {
            match result {
                Ok(target_id) => successful_targets.push(target_id),
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            if let Some(target_id) = remaining.next() {
                in_flight.push(self.wire_spawn_target(meerkat_id, target_id));
            }
        }

        (successful_targets, first_error)
    }

    async fn wire_spawn_target(
        &self,
        meerkat_id: &MeerkatId,
        target_id: MeerkatId,
    ) -> Result<MeerkatId, MobError> {
        self.do_wire(meerkat_id, &target_id).await.map_err(|e| {
            MobError::WiringError(format!(
                "role_wiring fan-out failed for {meerkat_id} <-> {target_id}: {e}"
            ))
        })?;
        Ok(target_id)
    }

    /// Compensate a failed spawn wiring path to avoid partial state.
    async fn rollback_failed_spawn(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
        successful_wiring_targets: &[MeerkatId],
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let retire_event_already_present = self.retire_event_exists(meerkat_id, member_ref).await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, profile_name, member_ref)
                .await?;
        }

        let mut wired_peers = successful_wiring_targets.to_vec();
        wired_peers.sort();
        wired_peers.dedup();

        let mut cleanup_peers = wired_peers.clone();
        for peer_id in planned_wiring_targets {
            if peer_id != meerkat_id && !cleanup_peers.contains(peer_id) {
                cleanup_peers.push(peer_id.clone());
            }
        }
        let spawned_entry = {
            let roster = self.roster.read().await;
            roster.get(meerkat_id).cloned()
        };
        let spawned_comms = self.provisioner_comms(member_ref).await;
        let mut rollback = LifecycleRollback::new("spawn rollback");

        if !wired_peers.is_empty() {
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{meerkat_id}'"
                ))
            })?;
            let spawned_sender = self
                .sender_runtime_for_entry(spawned_entry)
                .await
                .ok_or_else(|| {
                    MobError::WiringError(format!(
                        "spawn rollback requires sender runtime for '{meerkat_id}'"
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
                self.notify_peer_retired(&peer_spec, meerkat_id, spawned_entry, &spawned_sender)
                    .await?;
                rollback.defer(
                    format!("compensating mob.peer_added '{meerkat_id}' -> '{peer_meerkat_id}'"),
                    {
                        let spawned_sender = spawned_sender.clone();
                        let peer_spec = peer_spec.clone();
                        let spawned_entry = spawned_entry.clone();
                        let meerkat_id = meerkat_id.clone();
                        move || async move {
                            self.notify_peer_added(
                                &spawned_sender,
                                &peer_spec,
                                &meerkat_id,
                                &spawned_entry,
                            )
                            .await
                        }
                    },
                );
            }
        }

        let spawned_key = spawned_comms.as_ref().and_then(|comms| comms.public_key());
        let spawned_spec = if let (Some(spawned_key), Some(spawned_entry)) =
            (spawned_key.clone(), spawned_entry.as_ref())
        {
            let spawned_comms_name = self.comms_name_for(spawned_entry);
            Some(
                self.provisioner
                    .trusted_peer_spec(member_ref, &spawned_comms_name, &spawned_key)
                    .await?,
            )
        } else {
            None
        };

        if let Some(spawned_key) = spawned_key {
            for peer_meerkat_id in &cleanup_peers {
                let is_wired_peer = wired_peers.contains(peer_meerkat_id);
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}': wired peer '{peer_meerkat_id}' missing from roster"
                            )))
                            .await);
                    }
                    continue;
                };
                let peer_comms = self.provisioner_comms(&peer_entry.member_ref).await;
                let Some(peer_comms) = peer_comms else {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}': comms runtime missing for wired peer '{peer_meerkat_id}'"
                            )))
                            .await);
                    }
                    continue;
                };
                if let Err(error) = peer_comms.remove_trusted_peer(&spawned_key).await {
                    if is_wired_peer {
                        return Err(rollback
                            .fail(MobError::Internal(format!(
                                "spawn rollback cannot remove trust for '{meerkat_id}' from wired peer '{peer_meerkat_id}': {error}"
                            )))
                            .await);
                    }
                    continue;
                }
                if let Some(spawned_spec) = spawned_spec.clone() {
                    rollback.defer(
                        format!(
                            "restore trust '{peer_meerkat_id}' -> '{meerkat_id}' during spawn rollback"
                        ),
                        {
                            let peer_comms = peer_comms.clone();
                            move || async move {
                                peer_comms.add_trusted_peer(spawned_spec).await?;
                                Ok(())
                            }
                        },
                    );
                }
            }
        }

        // Reuse disposal pipeline methods for session archive + roster removal.
        let rollback_ctx = DisposalContext {
            meerkat_id: meerkat_id.clone(),
            entry: spawned_entry.clone().unwrap_or_else(|| {
                let identity = AgentIdentity::from(meerkat_id.as_str());
                RosterEntry {
                    agent_identity: identity.clone(),
                    generation: crate::ids::Generation::INITIAL,
                    fence_token: crate::ids::FenceToken::new(0),
                    agent_runtime_id: crate::ids::AgentRuntimeId::initial(identity),
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    member_ref: member_ref.clone(),
                    runtime_mode: crate::MobRuntimeMode::TurnDriven,
                    peer_id: spawned_comms.as_ref().and_then(|c| c.public_key()),
                    state: crate::roster::MemberState::Active,
                    voice_intent_present: false,
                    wired_to: std::collections::BTreeSet::new(),
                    external_peer_specs: std::collections::BTreeMap::new(),
                    labels: std::collections::BTreeMap::new(),
                    kickoff: None,
                    effective_profile_override: None,
                }
            }),
            retiring_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
            clear_voice_intent: false,
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
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<bool, MobError> {
        let key = Self::retire_event_key(meerkat_id, member_ref);
        let index = self.retired_event_index.read().await;
        Ok(index.contains(&key))
    }

    async fn append_retire_event(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        // Look up identity-native fields from the roster for the 0.6 event model.
        let (agent_identity, generation) = {
            let roster = self.roster.read().await;
            match roster.get(meerkat_id) {
                Some(entry) => (entry.agent_identity.clone(), entry.generation),
                None => (
                    AgentIdentity::from(meerkat_id.as_str()),
                    Generation::INITIAL,
                ),
            }
        };
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberRetired {
                    agent_identity,
                    generation,
                    role: profile_name.clone(),
                },
            })
            .await?;
        let key = Self::retire_event_key(meerkat_id, member_ref);
        self.retired_event_index.write().await.insert(key);
        Ok(())
    }

    async fn append_voice_intent_cleared_event(
        &self,
        meerkat_id: &MeerkatId,
    ) -> Result<(), MobError> {
        let agent_identity = {
            let roster = self.roster.read().await;
            roster
                .get(meerkat_id)
                .map(|entry| entry.agent_identity.clone())
                .unwrap_or_else(|| AgentIdentity::from(meerkat_id.as_str()))
        };

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberVoiceIntentCleared { agent_identity },
            })
            .await?;
        Ok(())
    }

    async fn append_voice_intent_set_event(&self, meerkat_id: &MeerkatId) -> Result<(), MobError> {
        let agent_identity = {
            let roster = self.roster.read().await;
            roster
                .get(meerkat_id)
                .map(|entry| entry.agent_identity.clone())
                .unwrap_or_else(|| AgentIdentity::from(meerkat_id.as_str()))
        };

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MemberVoiceIntentSet { agent_identity },
            })
            .await?;
        Ok(())
    }

    #[cfg(feature = "runtime-adapter")]
    async fn reconcile_realtime_attachment_runtime(
        &self,
        member_ref: &MemberRef,
        runtime_mode: crate::MobRuntimeMode,
        intent_present: bool,
    ) {
        let (Some(runtime_adapter), Some(bridge_session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        else {
            return;
        };

        if runtime_mode == crate::MobRuntimeMode::TurnDriven {
            // Transitional but architecturally intentional: realtime-attached
            // turn-driven members need a live comms drain so correlated peer
            // responses become canonical runtime inputs between spoken user
            // turns. Autonomous members already own a keep-alive drain through
            // their host loop; this extra drain is only for active realtime
            // turn-driven sessions until the machine/DSL seam models channel-
            // scoped peer ingress ownership directly.
            let comms_runtime = if intent_present {
                self.provisioner.comms_runtime(member_ref).await
            } else {
                None
            };
            let _ = runtime_adapter
                .maybe_spawn_comms_drain(bridge_session_id, intent_present, comms_runtime)
                .await;
        }

        if let Err(error) = runtime_adapter
            .project_realtime_attachment_intent(bridge_session_id, intent_present)
            .await
        {
            tracing::warn!(
                member_ref = ?member_ref,
                session_id = %bridge_session_id,
                error = %error,
                "failed to project durable live attachment intent during mob voice update"
            );
            return;
        }

        if intent_present {
            if let Err(error) = runtime_adapter.attach_live(bridge_session_id).await {
                tracing::warn!(
                    member_ref = ?member_ref,
                    session_id = %bridge_session_id,
                    error = %error,
                    "failed to attach live voice during mob voice update"
                );
            }
            return;
        }

        if let Err(error) = runtime_adapter.detach_live(bridge_session_id).await {
            tracing::warn!(
                member_ref = ?member_ref,
                session_id = %bridge_session_id,
                error = %error,
                "failed to detach live voice during mob voice update"
            );
        }
    }

    #[cfg(feature = "runtime-adapter")]
    async fn restore_realtime_attachment_intent_if_needed(
        &self,
        member_ref: &MemberRef,
        runtime_mode: crate::MobRuntimeMode,
        voice_intent_present: bool,
    ) {
        let (Some(runtime_adapter), Some(bridge_session_id)) =
            (&self.runtime_adapter, member_ref.bridge_session_id())
        else {
            return;
        };

        if voice_intent_present && runtime_mode == crate::MobRuntimeMode::TurnDriven {
            // Transitional companion to reconcile_realtime_attachment_runtime():
            // when a turn-driven member resumes/spawns with an active realtime
            // attachment intent, it must immediately regain a comms drain so
            // peer responses can re-enter the runtime before the next spoken
            // turn. This should collapse into the machine-owned channel seam
            // once the DSL branch lands.
            let comms_runtime = self.provisioner.comms_runtime(member_ref).await;
            let _ = runtime_adapter
                .maybe_spawn_comms_drain(bridge_session_id, true, comms_runtime)
                .await;
        }

        if let Err(error) = runtime_adapter
            .project_realtime_attachment_intent(bridge_session_id, voice_intent_present)
            .await
        {
            tracing::warn!(
                member_ref = ?member_ref,
                session_id = %bridge_session_id,
                error = %error,
                "failed to project durable live attachment intent during spawn finalization"
            );
            return;
        }

        if !voice_intent_present {
            return;
        }

        if let Err(error) = runtime_adapter.attach_live(bridge_session_id).await {
            tracing::warn!(
                member_ref = ?member_ref,
                session_id = %bridge_session_id,
                error = %error,
                "failed to reattach live voice during spawn finalization"
            );
        }
    }

    /// Internal wire operation (used by handle_wire and auto_wire/role_wiring).
    async fn do_wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        self.ensure_member_not_broken(a).await?;
        self.ensure_member_not_broken(b).await?;

        let _edge_guard = self.edge_locks.acquire(a.as_str(), b.as_str()).await;

        let (entry_a, entry_b, a_has_b_edge, b_has_a_edge) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(a.clone()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(b.clone()))?;
            let identity_b = AgentIdentity::from(b.as_str());
            let identity_a = AgentIdentity::from(a.as_str());
            let a_has_b_edge = ea.wired_to.contains(&identity_b);
            let b_has_a_edge = eb.wired_to.contains(&identity_a);
            (ea, eb, a_has_b_edge, b_has_a_edge)
        };
        match MobWiringAuthority::plan_local_wire(
            a,
            entry_a.state,
            a_has_b_edge,
            b,
            entry_b.state,
            b_has_a_edge,
        )? {
            LocalWirePlan::ReconcileExisting => {
                // Roster projection already says "wired". Reconcile trust edges so
                // stale/missing comms trust cannot hide behind a roster-only fast path.
                self.reconcile_wire_trust_edges(a, &entry_a, b, &entry_b)
                    .await?;
                self.debug_assert_roster_edge_symmetric(a, b, "do_wire/reconcile")
                    .await;
                return Ok(());
            }
            LocalWirePlan::EstablishNew => {}
        }
        if a_has_b_edge != b_has_a_edge {
            tracing::warn!(
                a = %a,
                b = %b,
                a_has_b_edge,
                b_has_a_edge,
                "wire found non-canonical roster projection; reapplying full wire path"
            );
        }
        let endpoint_a = self.resolve_wiring_endpoint(&entry_a, "wire").await?;
        let endpoint_b = self.resolve_wiring_endpoint(&entry_b, "wire").await?;

        match (&endpoint_a, &endpoint_b) {
            (
                WiringEndpoint::Local {
                    comms: comms_a,
                    spec: spec_a,
                    comms_name: _comms_name_a,
                    entry: entry_a,
                },
                WiringEndpoint::Local {
                    comms: comms_b,
                    spec: spec_b,
                    comms_name: _comms_name_b,
                    entry: entry_b,
                },
            ) => {
                let mut rollback = LifecycleRollback::new("wire");

                comms_a.add_trusted_peer(spec_b.clone()).await?;
                rollback.defer(format!("remove trust '{a}' -> '{b}'"), {
                    let comms_a = comms_a.clone();
                    let peer_id = spec_b.peer_id.clone();
                    move || async move {
                        comms_a.remove_trusted_peer(&peer_id).await?;
                        Ok(())
                    }
                });

                if let Err(error) = comms_b.add_trusted_peer(spec_a.clone()).await {
                    return Err(rollback.fail(error.into()).await);
                }
                rollback.defer(format!("remove trust '{b}' -> '{a}'"), {
                    let comms_b = comms_b.clone();
                    let peer_id = spec_a.peer_id.clone();
                    move || async move {
                        comms_b.remove_trusted_peer(&peer_id).await?;
                        Ok(())
                    }
                });

                if let Err(error) = self.notify_peer_added(comms_b, spec_a, b, entry_b).await {
                    return Err(rollback.fail(error).await);
                }
                rollback.defer(format!("compensating mob.peer_retired '{b}' -> '{a}'"), {
                    let comms_b = comms_b.clone();
                    let spec_a = spec_a.clone();
                    let entry_b = entry_b.clone();
                    let b = b.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_a, &b, &entry_b, &comms_b)
                            .await
                    }
                });

                if let Err(error) = self.notify_peer_added(comms_a, spec_b, a, entry_a).await {
                    return Err(rollback.fail(error).await);
                }
                rollback.defer(format!("compensating mob.peer_retired '{a}' -> '{b}'"), {
                    let comms_a = comms_a.clone();
                    let spec_b = spec_b.clone();
                    let entry_a = entry_a.clone();
                    let a = a.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_b, &a, &entry_a, &comms_a)
                            .await
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersWired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::Local {
                    comms,
                    spec: spec_a,
                    entry: entry_a,
                    ..
                },
                WiringEndpoint::PeerOnly {
                    spec: spec_b,
                    binding: binding_b,
                },
            ) => {
                let mut rollback = LifecycleRollback::new("wire");
                comms.add_trusted_peer(spec_b.clone()).await?;
                rollback.defer(format!("remove trust '{a}' -> '{b}'"), {
                    let comms = comms.clone();
                    let peer_id = spec_b.peer_id.clone();
                    move || async move {
                        comms.remove_trusted_peer(&peer_id).await?;
                        Ok(())
                    }
                });
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.notify_peer_added(&supervisor_sender, spec_a, b, &entry_b)
                    .await?;
                rollback.defer(format!("compensating mob.peer_retired '{b}' -> '{a}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_a = spec_a.clone();
                    let entry_b = entry_b.clone();
                    let b = b.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_a, &b, &entry_b, &supervisor_sender)
                            .await
                    }
                });
                self.wire_peer_only_recipient(
                    spec_b,
                    Some(binding_b),
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_added(&supervisor_sender, spec_b, a, entry_a)
                    .await?;
                rollback.defer(format!("compensating mob.peer_retired '{a}' -> '{b}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_b = spec_b.clone();
                    let entry_a = entry_a.clone();
                    let a = a.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_b, &a, &entry_a, &supervisor_sender)
                            .await
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersWired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::PeerOnly { spec: spec_a, .. },
                WiringEndpoint::Local {
                    comms,
                    spec: spec_b,
                    entry: entry_b,
                    ..
                },
            ) => {
                let mut rollback = LifecycleRollback::new("wire");
                comms.add_trusted_peer(spec_a.clone()).await?;
                rollback.defer(format!("remove trust '{b}' -> '{a}'"), {
                    let comms = comms.clone();
                    let peer_id = spec_a.peer_id.clone();
                    move || async move {
                        comms.remove_trusted_peer(&peer_id).await?;
                        Ok(())
                    }
                });
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.wire_peer_only_recipient(
                    spec_b,
                    None,
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_added(&supervisor_sender, spec_b, a, &entry_a)
                    .await?;
                rollback.defer(format!("compensating mob.peer_retired '{a}' -> '{b}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_b = spec_b.clone();
                    let entry_a = entry_a.clone();
                    let a = a.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_b, &a, &entry_a, &supervisor_sender)
                            .await
                    }
                });
                self.notify_peer_added(&supervisor_sender, spec_a, b, entry_b)
                    .await?;
                rollback.defer(format!("compensating mob.peer_retired '{b}' -> '{a}'"), {
                    let supervisor_sender = supervisor_sender.clone();
                    let spec_a = spec_a.clone();
                    let entry_b = entry_b.clone();
                    let b = b.clone();
                    move || async move {
                        self.notify_peer_retired(&spec_a, &b, &entry_b, &supervisor_sender)
                            .await
                    }
                });

                if let Err(append_error) = self
                    .events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersWired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await
                {
                    return Err(rollback.fail(MobError::from(append_error)).await);
                }
            }
            (
                WiringEndpoint::PeerOnly {
                    spec: spec_a,
                    binding: binding_a,
                },
                WiringEndpoint::PeerOnly {
                    spec: spec_b,
                    binding: binding_b,
                },
            ) => {
                let supervisor_sender = self.supervisor_bridge.runtime_core().await;
                self.wire_peer_only_recipient(
                    spec_a,
                    Some(binding_a),
                    spec_b,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_added(&supervisor_sender, spec_a, b, &entry_b)
                    .await?;
                self.wire_peer_only_recipient(
                    spec_b,
                    Some(binding_b),
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.notify_peer_added(&supervisor_sender, spec_b, a, &entry_a)
                    .await?;
                self.events
                    .append(NewMobEvent {
                        mob_id: self.definition.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MembersWired {
                            a: AgentIdentity::from(a.as_str()),
                            b: AgentIdentity::from(b.as_str()),
                        },
                    })
                    .await?;
            }
        }

        {
            let mut roster = self.roster.write().await;
            roster.wire_members(a, b);
        }
        self.debug_assert_roster_edge_symmetric(a, b, "do_wire/post")
            .await;

        Ok(())
    }

    async fn reconcile_wire_trust_edges(
        &self,
        _a: &MeerkatId,
        entry_a: &RosterEntry,
        _b: &MeerkatId,
        entry_b: &RosterEntry,
    ) -> Result<(), MobError> {
        let endpoint_a = self
            .resolve_wiring_endpoint(entry_a, "wire trust reconciliation")
            .await?;
        let endpoint_b = self
            .resolve_wiring_endpoint(entry_b, "wire trust reconciliation")
            .await?;

        match (&endpoint_a, &endpoint_b) {
            (
                WiringEndpoint::Local {
                    comms: comms_a,
                    spec: spec_a,
                    ..
                },
                WiringEndpoint::Local {
                    comms: comms_b,
                    spec: spec_b,
                    ..
                },
            ) => {
                comms_a.add_trusted_peer(spec_b.clone()).await?;
                comms_b.add_trusted_peer(spec_a.clone()).await?;
            }
            (
                WiringEndpoint::Local { comms, spec, .. },
                WiringEndpoint::PeerOnly {
                    spec: peer_spec,
                    binding,
                },
            ) => {
                self.wire_peer_only_recipient(
                    peer_spec,
                    Some(binding),
                    spec,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                comms.add_trusted_peer(peer_spec.clone()).await?;
            }
            (
                WiringEndpoint::PeerOnly {
                    spec: peer_spec,
                    binding,
                },
                WiringEndpoint::Local { comms, spec, .. },
            ) => {
                self.wire_peer_only_recipient(
                    peer_spec,
                    Some(binding),
                    spec,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                comms.add_trusted_peer(spec.clone()).await?;
            }
            (
                WiringEndpoint::PeerOnly {
                    spec: spec_a,
                    binding: binding_a,
                },
                WiringEndpoint::PeerOnly {
                    spec: spec_b,
                    binding: binding_b,
                },
            ) => {
                self.wire_peer_only_recipient(
                    spec_a,
                    Some(binding_a),
                    spec_b,
                    std::time::Duration::from_secs(5),
                )
                .await?;
                self.wire_peer_only_recipient(
                    spec_b,
                    Some(binding_b),
                    spec_a,
                    std::time::Duration::from_secs(5),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn debug_assert_roster_edge_symmetric(
        &self,
        a: &MeerkatId,
        b: &MeerkatId,
        context: &'static str,
    ) {
        let _ = (a, b, context);
    }

    /// Get the comms runtime for a session, if available.
    async fn provisioner_comms(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.provisioner.comms_runtime(member_ref).await
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
        format!("{}/{}/{}", self.definition.id, entry.role, entry.meerkat_id)
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
                    entry.meerkat_id
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
                let spec = self
                    .provisioner
                    .trusted_peer_spec(&entry.member_ref, &comms_name, peer_id)
                    .await?;
                Ok(WiringEndpoint::PeerOnly {
                    spec,
                    binding: Self::runtime_binding_for_entry(entry).ok_or_else(|| {
                        MobError::WiringError(format!(
                            "{context} requires external runtime binding for '{}'",
                            entry.meerkat_id
                        ))
                    })?,
                })
            }
            MemberRef::Session { .. } => Err(MobError::WiringError(format!(
                "{context} requires comms runtime for '{}'",
                entry.meerkat_id
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
        recipient_spec: &TrustedPeerSpec,
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
        sender_comms
            .add_trusted_peer(recipient_spec.clone())
            .await?;
        let peer_name = PeerName::new(recipient_spec.name.clone()).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_added: invalid recipient comms name '{}': {error}",
                recipient_spec.name
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.role.as_str(),
                "description": peer_description,
                "peer_name": new_peer_spec.name,
                "peer_id": new_peer_spec.peer_id,
                "address": new_peer_spec.address,
                "peer_spec": new_peer_spec,
            }),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream: meerkat_core::comms::InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_peer_event(
        &self,
        intent: &'static str,
        recipient_spec: &TrustedPeerSpec,
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
        sender_comms
            .add_trusted_peer(recipient_spec.clone())
            .await?;
        let peer_name = PeerName::new(recipient_spec.name.clone()).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_retired: invalid peer comms name '{}': {error}",
                recipient_spec.name
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: intent.to_string(),
            params: serde_json::json!({
                "peer": other_peer_id.as_str(),
                "role": other_peer_entry.role.as_str(),
                "peer_name": other_peer_spec.name,
                "peer_id": other_peer_spec.peer_id,
                "address": other_peer_spec.address,
                "peer_spec": other_peer_spec,
            }),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream: meerkat_core::comms::InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_kickoff_event(
        &self,
        meerkat_id: &MeerkatId,
        intent: &'static str,
    ) -> Result<(), MobError> {
        let (entry, wired_peers) = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(meerkat_id).cloned() else {
                return Ok(());
            };
            let wired_peers: Vec<MeerkatId> = entry
                .wired_to
                .iter()
                .filter_map(|id| roster.get_by_identity(id).map(|e| e.meerkat_id.clone()))
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
            self.notify_peer_event(intent, &recipient_spec, meerkat_id, &entry, &sender_comms)
                .await?;
        }
        Ok(())
    }

    /// Notify a peer that another peer was retired from the mob.
    async fn notify_peer_retired(
        &self,
        recipient_spec: &TrustedPeerSpec,
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
        recipient_spec: &TrustedPeerSpec,
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
