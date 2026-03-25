use super::disposal::{
    BulkBestEffort, DisposalContext, DisposalReport, DisposalStep, ErrorPolicy, WarnAndContinue,
};
use super::mob_lifecycle_authority::{
    MobLifecycleAuthority, MobLifecycleInput, MobLifecycleMutator,
};
use super::mob_orchestrator_authority::{
    MobOrchestratorAuthority, MobOrchestratorInput, MobOrchestratorMutator,
};
use super::provision_guard::PendingProvision;
use super::transaction::LifecycleRollback;
use super::*;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::comms::TrustedPeerSpec;
use std::collections::{HashMap, HashSet, VecDeque};

type AutonomousHostLoopHandle = tokio::task::JoinHandle<Result<(), MobError>>;
// Sized for real mob-scale startup/shutdown fan-out (50+ members).
const MAX_PARALLEL_HOST_LOOP_OPS: usize = 64;
const MAX_LIFECYCLE_NOTIFICATION_TASKS: usize = 16;

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
    pub(super) auto_wire_parent: bool,
    /// Peer wiring to restore after respawn completes.
    pub(super) restore_wiring: Option<RestoreWiringPlan>,
    pub(super) reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
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
    old_session_id: meerkat_core::types::SessionId,
    restore_wiring: RestoreWiringPlan,
}

struct FinalizeSpawnOutcome {
    receipt: super::handle::MemberSpawnReceipt,
    failed_restore_peer_ids: Vec<MeerkatId>,
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
    pub(super) orchestrator: Option<MobOrchestratorAuthority>,
    pub(super) run_tasks: BTreeMap<RunId, tokio::task::JoinHandle<()>>,
    pub(super) run_cancel_tokens: BTreeMap<RunId, (tokio_util::sync::CancellationToken, FlowId)>,
    pub(super) flow_streams:
        Arc<tokio::sync::Mutex<BTreeMap<RunId, mpsc::Sender<meerkat_core::ScopedAgentEvent>>>>,
    pub(super) mcp_servers: Arc<tokio::sync::Mutex<BTreeMap<String, McpServerEntry>>>,
    pub(super) command_tx: mpsc::Sender<MobCommand>,
    pub(super) tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    pub(super) default_llm_client: Option<Arc<dyn LlmClient>>,
    pub(super) retired_event_index: Arc<RwLock<HashSet<String>>>,
    pub(super) autonomous_host_loops:
        Arc<tokio::sync::Mutex<BTreeMap<MeerkatId, AutonomousHostLoopHandle>>>,
    pub(super) next_spawn_ticket: u64,
    pub(super) pending_spawns: PendingSpawnLineage,
    pub(super) edge_locks: Arc<super::edge_locks::EdgeLockRegistry>,
    pub(super) lifecycle_tasks: tokio::task::JoinSet<()>,
    pub(super) session_service: Arc<dyn MobSessionService>,
    pub(super) restore_diagnostics:
        Arc<RwLock<HashMap<MeerkatId, super::handle::RestoreFailureDiagnostic>>>,
    pub(super) task_board_service: crate::tasks::MobTaskBoardService,
    pub(super) spawn_policy: Arc<super::spawn_policy::SpawnPolicyService>,
    pub(super) lifecycle_authority: MobLifecycleAuthority,
}

impl MobActor {
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
                session_id: diag.session_id,
                reason: diag.reason,
            });
        }
        Ok(())
    }

    fn state(&self) -> MobState {
        self.lifecycle_authority.phase()
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            task_board: self.task_board.clone(),
            definition: self.definition.clone(),
            state: self.state.clone(),
            events: self.events.clone(),
            mcp_servers: self.mcp_servers.clone(),
            flow_streams: self.flow_streams.clone(),
            session_service: self.session_service.clone(),
            restore_diagnostics: self.restore_diagnostics.clone(),
        }
    }

    fn expect_state(&self, expected: &[MobState], to: MobState) -> Result<(), MobError> {
        self.lifecycle_authority.require_phase(expected, to)
    }

    /// Guard that the mob is in one of the `allowed` phases.
    ///
    /// Used by command handlers that operate *within* the current state
    /// (retire, wire, external turn, etc.). The first allowed state is used
    /// as the `to` hint in the error.
    fn require_state(&self, allowed: &[MobState]) -> Result<(), MobError> {
        self.lifecycle_authority.require_phase(allowed, allowed[0])
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
                    let Some(session_id) = member_ref.session_id() else {
                        return;
                    };
                    let Some(injector) = provisioner.interaction_event_injector(session_id).await
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
        let expected = self
            .orchestrator
            .as_ref()
            .map(|orchestrator| orchestrator.snapshot().pending_spawn_count as usize);
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

    #[allow(dead_code)]
    fn pending_spawn_tickets(&self) -> std::collections::BTreeSet<u64> {
        self.pending_spawns.tickets()
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
        if let Some(ref mut orch) = self.orchestrator {
            orch.apply(MobOrchestratorInput::StageSpawn)?;
        }
        Ok(())
    }

    fn complete_orchestrator_spawn(&mut self, spawn_ticket: Option<u64>, context: &'static str) {
        if let Some(ref mut orch) = self.orchestrator
            && let Err(error) = orch.apply(MobOrchestratorInput::CompleteSpawn)
        {
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

    async fn flow_tracker_alignment_violation(&self) -> Option<String> {
        let run_task_ids = self
            .run_tasks
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let run_token_ids = self
            .run_cancel_tokens
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if run_task_ids != run_token_ids {
            return Some(format!(
                "run task/token tracker mismatch: tasks={run_task_ids:?}, tokens={run_token_ids:?}"
            ));
        }

        let stream_ids = self
            .flow_streams
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
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

    fn resume_host_loop_prompt(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> String {
        format!(
            "Mob '{}' resumed autonomous host loop for '{}' (role: {}). Continue coordinated execution.",
            self.definition.id, meerkat_id, profile_name
        )
    }

    async fn start_autonomous_host_loop(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
        prompt: ContentInput,
    ) -> Result<(), MobError> {
        {
            let mut loops = self.autonomous_host_loops.lock().await;
            if let Some(existing) = loops.get(meerkat_id)
                && !existing.is_finished()
            {
                return Ok(());
            }
            loops.remove(meerkat_id);
        }

        let member_ref_cloned = member_ref.clone();
        let provisioner = self.provisioner.clone();
        let loop_id = meerkat_id.clone();
        let log_id = meerkat_id.clone();

        // Resolve comms drain dependencies before spawning.
        // Both the runtime adapter and the member's comms runtime are needed
        // so that peer interactions are routed through the runtime authority
        // while the autonomous host loop is alive.
        let runtime_adapter = self.session_service.runtime_adapter();
        let comms_runtime = self.provisioner.comms_runtime(member_ref).await;
        let drain_session_id = member_ref.session_id().cloned();

        let handle = tokio::spawn(async move {
            // Spawn comms drain alongside the host loop when all wiring is available.
            let drain_spawned = match (&runtime_adapter, &drain_session_id) {
                (Some(adapter), Some(session_id)) => {
                    let spawned = adapter
                        .maybe_spawn_comms_drain(session_id, true, comms_runtime.clone())
                        .await;
                    if spawned {
                        tracing::debug!(
                            meerkat_id = %log_id,
                            session_id = %session_id,
                            "spawned comms drain for autonomous member"
                        );
                    }
                    spawned
                }
                _ => {
                    tracing::debug!(
                        meerkat_id = %log_id,
                        has_adapter = runtime_adapter.is_some(),
                        has_session = drain_session_id.is_some(),
                        "skipping comms drain for autonomous member (missing wiring)"
                    );
                    false
                }
            };

            let result = provisioner
                .start_turn(
                    &member_ref_cloned,
                    meerkat_core::service::StartTurnRequest {
                        prompt,
                        system_prompt: None,
                        render_metadata: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        event_tx: None,

                        skill_references: None,
                        flow_tool_overlay: None,
                        additional_instructions: None,
                    },
                )
                .await;

            // Abort the comms drain when the host loop exits.
            if drain_spawned
                && let (Some(adapter), Some(session_id)) = (&runtime_adapter, &drain_session_id)
            {
                adapter.abort_comms_drain(session_id).await;
            }

            match &result {
                Ok(()) => tracing::info!(
                    meerkat_id = %log_id,
                    "autonomous host loop exited normally"
                ),
                Err(error) => tracing::error!(
                    meerkat_id = %log_id,
                    error = %error,
                    "autonomous host loop failed"
                ),
            }
            result
        });

        tokio::task::yield_now().await;
        if handle.is_finished() {
            match handle.await {
                Ok(Ok(())) => {
                    return Err(MobError::Internal(format!(
                        "autonomous host loop for '{loop_id}' exited immediately"
                    )));
                }
                Ok(Err(error)) => return Err(error),
                Err(join_error) => {
                    return Err(MobError::Internal(format!(
                        "autonomous host loop task join failed for '{loop_id}': {join_error}"
                    )));
                }
            }
        }

        self.autonomous_host_loops
            .lock()
            .await
            .insert(meerkat_id.clone(), handle);
        Ok(())
    }

    async fn start_autonomous_host_loops_from_roster(&self) -> Result<(), MobError> {
        let broken_members = self
            .restore_diagnostics
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let entries = {
            let roster = self.roster.read().await;
            roster.list().cloned().collect::<Vec<_>>()
        };
        let autonomous_entries = entries
            .into_iter()
            .filter(|entry| {
                entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost
                    && !broken_members.contains(&entry.meerkat_id)
            })
            .collect::<Vec<_>>();
        if autonomous_entries.is_empty() {
            return Ok(());
        }

        let actor: &MobActor = self;
        let mut remaining = autonomous_entries.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(entry) = remaining.next() else {
                break;
            };
            in_flight.push(actor.start_autonomous_host_loop_for_entry(entry));
        }

        while let Some(result) = in_flight.next().await {
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
            if let Some(entry) = remaining.next() {
                in_flight.push(actor.start_autonomous_host_loop_for_entry(entry));
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn ensure_autonomous_dispatch_capability_for_provisioner(
        provisioner: &Arc<dyn MobProvisioner>,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        let session_id = member_ref.session_id().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{meerkat_id}' must be session-backed for injector dispatch"
            ))
        })?;
        if provisioner
            .interaction_event_injector(session_id)
            .await
            .is_none()
        {
            return Err(MobError::Internal(format!(
                "autonomous member '{meerkat_id}' is missing event injector capability"
            )));
        }
        Ok(())
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

    async fn stop_autonomous_host_loop_for_member(
        &self,
        meerkat_id: &MeerkatId,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        if let Err(error) = self.provisioner.interrupt_member(member_ref).await
            && !matches!(
                error,
                MobError::SessionError(meerkat_core::service::SessionError::NotFound { .. })
            )
        {
            return Err(error);
        }
        if let Some(handle) = self.autonomous_host_loops.lock().await.remove(meerkat_id) {
            handle.abort();
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
                "autonomous host loop stop polling exhausted before member became idle"
            );
        }
        Ok(())
    }

    async fn stop_all_autonomous_host_loops(&self) -> Result<(), MobError> {
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
        let actor: &MobActor = self;
        let mut remaining = entries.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut first_error: Option<MobError> = None;

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(entry) = remaining.next() else {
                break;
            };
            in_flight.push(actor.stop_autonomous_host_loop_for_entry(entry));
        }

        while let Some(result) = in_flight.next().await {
            if let Err((meerkat_id, error)) = result {
                tracing::warn!(
                    meerkat_id = %meerkat_id,
                    error = %error,
                    "failed stopping autonomous host loop member"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            if let Some(entry) = remaining.next() {
                in_flight.push(actor.stop_autonomous_host_loop_for_entry(entry));
            }
        }

        let mut loops = self.autonomous_host_loops.lock().await;
        for (_, handle) in std::mem::take(&mut *loops) {
            handle.abort();
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn start_autonomous_host_loop_for_entry(
        &self,
        entry: RosterEntry,
    ) -> Result<(), MobError> {
        self.ensure_autonomous_dispatch_capability(&entry.meerkat_id, &entry.member_ref)
            .await?;
        self.start_autonomous_host_loop(
            &entry.meerkat_id,
            &entry.member_ref,
            self.resume_host_loop_prompt(&entry.profile, &entry.meerkat_id)
                .into(),
        )
        .await
    }

    async fn stop_autonomous_host_loop_for_entry(
        &self,
        entry: RosterEntry,
    ) -> Result<(), (MeerkatId, MobError)> {
        self.stop_autonomous_host_loop_for_member(&entry.meerkat_id, &entry.member_ref)
            .await
            .map_err(|error| (entry.meerkat_id, error))
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
                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up autonomous host loops after mcp startup error"
                    );
                }
                if let Err(stop_error) = self.stop_mcp_servers().await {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        error = %stop_error,
                        "failed cleaning up mcp servers after startup error"
                    );
                }
                if let Err(e) = self.lifecycle_authority.apply(MobLifecycleInput::Stop) {
                    tracing::warn!(error = %e, "authority rejected Stop");
                }
            } else if let Err(error) = self.start_autonomous_host_loops_from_roster().await {
                tracing::error!(
                    mob_id = %self.definition.id,
                    error = %error,
                    "failed to start autonomous host loops during actor startup; entering Stopped"
                );
                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await {
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
                if let Err(e) = self.lifecycle_authority.apply(MobLifecycleInput::Stop) {
                    tracing::warn!(error = %e, "authority rejected Stop");
                }
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
                    owner_session_id,
                    ops_registry,
                    reply_tx,
                } => {
                    if let Err(error) = self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        let _ = reply_tx.send(Err(error));
                        continue;
                    }
                    self.enqueue_spawn(*spec, owner_session_id, ops_registry, reply_tx)
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
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_respawn(meerkat_id, initial_message).await,
                        Err(error) => Err(super::handle::MobRespawnError::from(error)),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RetireAll { reply_tx } => {
                    let result = match self.require_state(&[
                        MobState::Running,
                        MobState::Creating,
                        MobState::Stopped,
                    ]) {
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => {
                            self.handle_external_turn(
                                meerkat_id,
                                content,
                                handling_mode,
                                render_metadata,
                            )
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_internal_turn(meerkat_id, content).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::RunFlow {
                    flow_id,
                    activation_params,
                    scoped_event_tx,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running]) {
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
                    let result = self.run_store.get_run(&run_id).await;
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
                    let tasks = self
                        .orchestrator
                        .as_ref()
                        .map_or(0, |o| o.snapshot().active_flow_count as usize);
                    let tokens = self.run_cancel_tokens.len();
                    let _ = reply_tx.send((tasks, tokens));
                }
                #[cfg(test)]
                MobCommand::OrchestratorSnapshot { reply_tx } => {
                    let _ = reply_tx.send(self.orchestrator.as_ref().map_or_else(
                        MobOrchestratorSnapshot::default,
                        super::mob_orchestrator_authority::MobOrchestratorAuthority::snapshot,
                    ));
                }
                MobCommand::Stop { reply_tx } => {
                    let result = match self.expect_state(&[MobState::Running], MobState::Stopped) {
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
                            let (loop_result, mcp_result) = tokio::join!(
                                self.stop_all_autonomous_host_loops(),
                                self.stop_mcp_servers()
                            );
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
                                if let Some(ref mut orch) = self.orchestrator
                                    && let Err(error) =
                                        orch.apply(MobOrchestratorInput::StopOrchestrator)
                                {
                                    stop_result = Err(MobError::Internal(format!(
                                        "orchestrator StopOrchestrator transition failed during stop: {error}"
                                    )));
                                }
                                if stop_result.is_ok()
                                    && let Err(error) =
                                        self.lifecycle_authority.apply(MobLifecycleInput::Stop)
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
                    let result = match self.expect_state(&[MobState::Stopped], MobState::Running) {
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
                                self.start_autonomous_host_loops_from_roster().await
                            {
                                if let Err(stop_error) = self.stop_all_autonomous_host_loops().await
                                {
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
                                if let Some(ref mut orch) = self.orchestrator
                                    && let Err(error) =
                                        orch.apply(MobOrchestratorInput::ResumeOrchestrator)
                                {
                                    resume_result = Err(MobError::Internal(format!(
                                        "orchestrator ResumeOrchestrator transition failed during resume: {error}"
                                    )));
                                }
                                if resume_result.is_ok()
                                    && let Err(error) =
                                        self.lifecycle_authority.apply(MobLifecycleInput::Resume)
                                {
                                    resume_result = Err(MobError::Internal(format!(
                                        "lifecycle Resume transition failed during resume: {error}"
                                    )));
                                }
                                if let Err(error) = resume_result {
                                    if let Err(stop_error) =
                                        self.stop_all_autonomous_host_loops().await
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
                    let result = match self.expect_state(&[MobState::Running], MobState::Completed)
                    {
                        Ok(()) => {
                            self.fail_all_pending_spawns("mob is completing").await;
                            self.handle_complete().await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    // No shell guard — lifecycle_authority rejects invalid transitions.
                    self.fail_all_pending_spawns("mob is destroying").await;
                    let result = self.handle_destroy().await;
                    let _ = reply_tx.send(result);
                }
                MobCommand::Reset { reply_tx } => {
                    // No shell guard — lifecycle_authority rejects invalid transitions.
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
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
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_task_update(task_id, status, owner).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ForceCancel {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.require_state(&[MobState::Running, MobState::Creating])
                    {
                        Ok(()) => self.handle_force_cancel(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                    self.spawn_policy.set(policy).await;
                    let _ = reply_tx.send(());
                }
                MobCommand::Shutdown { reply_tx } => {
                    self.fail_all_pending_spawns("mob runtime is shutting down")
                        .await;
                    let mut result: Result<(), MobError> = Ok(());
                    if let Err(error) = self.cancel_all_flow_tasks().await {
                        tracing::warn!(error = %error, "shutdown flow cancellation encountered errors");
                        if result.is_ok() {
                            result = Err(error);
                        }
                    }
                    if let Err(error) = self.stop_all_autonomous_host_loops().await {
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
                        && let Err(error) = self.lifecycle_authority.apply(MobLifecycleInput::Stop)
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
        owner_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
        reply_tx: oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
    ) {
        let super::handle::SpawnMemberSpec {
            profile_name,
            meerkat_id,
            initial_message,
            runtime_mode,
            backend,
            context,
            labels,
            launch_mode,
            tool_access_policy: _tool_access_policy,
            budget_split_policy: _budget_split_policy,
            auto_wire_parent,
            additional_instructions,
            shell_env,
        } = spec;
        // Normalize launch-mode resume/fork details for the provisioning path.
        let (resume_session_id, fork_spec) = match launch_mode {
            crate::launch::MemberLaunchMode::Fresh => (None, None),
            crate::launch::MemberLaunchMode::Resume { session_id } => (Some(session_id), None),
            crate::launch::MemberLaunchMode::Fork {
                source_member_id,
                fork_context,
            } => (None, Some((source_member_id, fork_context))),
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
                return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
            }

            {
                let roster = self.roster.read().await;
                if roster.get(&meerkat_id).is_some() {
                    return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
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

            let profile = self
                .definition
                .profiles
                .get(&profile_name)
                .ok_or_else(|| MobError::ProfileNotFound(profile_name.clone()))?;

            let selected_runtime_mode = runtime_mode.unwrap_or(profile.runtime_mode);

            // ---------- Resume session fast-path ----------
            // When resume_session_id is set, skip provisioning and go straight
            // to finalization. The session must already exist and be usable.
            if let Some(resume_id) = resume_session_id {
                let member_ref = MemberRef::from_session_id(resume_id.clone());

                // Validate the session exists and is active.
                let is_active = self
                    .provisioner
                    .is_member_active(&member_ref)
                    .await
                    .map_err(|e| {
                        MobError::Internal(format!(
                            "resume session check failed for '{meerkat_id}': {e}"
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
                        auto_wire_parent,
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

                    let external_tools = self.external_tools_for_profile(profile)?;
                    let mut config = build::build_resumed_agent_config(
                        build::BuildResumedAgentConfigParams {
                            base: build::BuildAgentConfigParams {
                                mob_id: &self.definition.id,
                                profile_name: &profile_name,
                                meerkat_id: &meerkat_id,
                                profile,
                                definition: &self.definition,
                                external_tools,
                                context,
                                labels: labels.clone(),
                                additional_instructions,
                                shell_env,
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
                    let selected_backend = backend
                        .or(profile.backend)
                        .unwrap_or(self.definition.backend.default);
                    let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
                    let provision_request = ProvisionMemberRequest {
                        create_session: req,
                        backend: selected_backend,
                        peer_name,
                        owner_session_id: owner_session_id.clone(),
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
                        auto_wire_parent,
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
                        MobError::MeerkatNotFound(source_member_id.clone())
                    })?;
                    source_entry
                        .member_ref
                        .session_id()
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

            let external_tools = self.external_tools_for_profile(profile)?;
            let mut config = build::build_agent_config(build::BuildAgentConfigParams {
                mob_id: &self.definition.id,
                profile_name: &profile_name,
                meerkat_id: &meerkat_id,
                profile,
                definition: &self.definition,
                external_tools,
                context,
                labels: labels.clone(),
                additional_instructions,
                shell_env,
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
            let selected_backend = backend
                .or(profile.backend)
                .unwrap_or(self.definition.backend.default);
            let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
            let provision_request = ProvisionMemberRequest {
                create_session: req,
                backend: selected_backend,
                peer_name,
                owner_session_id: owner_session_id.clone(),
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
                auto_wire_parent,
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
            auto_wire_parent,
        ) = match prepare_result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = reply_tx.send(Err(error));
                return;
            }
        };

        // ---------- Resume fast-path: skip async provisioning ----------
        if let Some(member_ref) = resume_member_ref {
            if let (Some(owner_session_id), Some(ops_registry)) =
                (owner_session_id.clone(), ops_registry.clone())
                && let Err(error) = self
                    .provisioner
                    .bind_member_owner_context(&member_ref, owner_session_id, ops_registry)
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
            let result = self
                .finalize_spawn_from_pending(
                    &profile_name,
                    &meerkat_id,
                    selected_runtime_mode,
                    prompt,
                    resolved_labels,
                    provision,
                    operation_id,
                    auto_wire_parent,
                    None,
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
            auto_wire_parent,
            restore_wiring: None,
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

        let mut in_flight = FuturesUnordered::new();
        let actor: &MobActor = self;
        for (pending, result) in pending_items {
            let PendingSpawn {
                profile_name,
                meerkat_id,
                prompt,
                runtime_mode,
                labels,
                auto_wire_parent,
                restore_wiring,
                reply_tx,
            } = pending;
            in_flight.push(async move {
                let reply = match result {
                    Ok(spawn_receipt) => {
                        let provision = PendingProvision::new(
                            spawn_receipt.member_ref.clone(),
                            meerkat_id.clone(),
                            actor.provisioner.clone(),
                        );
                        if let Err(error) = actor
                            .require_state(&[MobState::Running, MobState::Creating])
                        {
                            if let Err(retire_error) = provision.rollback().await {
                                Err(MobError::Internal(format!(
                                    "spawn completed while mob state changed for '{meerkat_id}': {error}; cleanup retire failed: {retire_error}"
                                )))
                            } else {
                                Err(error)
                            }
                        } else {
                            actor.finalize_spawn_from_pending(
                                &profile_name,
                                &meerkat_id,
                                runtime_mode,
                                prompt,
                                labels,
                                provision,
                                spawn_receipt.operation_id,
                                auto_wire_parent,
                                restore_wiring,
                            )
                            .await
                            .map(|outcome| outcome.receipt)
                        }
                    }
                    Err(error) => Err(error),
                };
                (reply_tx, reply)
            });
        }

        while let Some((reply_tx, reply)) = in_flight.next().await {
            let _ = reply_tx.send(reply);
        }
        drop(in_flight);

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
            return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
        }
        {
            let roster = self.roster.read().await;
            if roster.get(meerkat_id).is_some() {
                return Err(MobError::MeerkatAlreadyExists(meerkat_id.clone()));
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
            .profiles
            .get(&profile_name)
            .ok_or_else(|| MobError::ProfileNotFound(profile_name.clone()))?;
        let runtime_mode = spawn_spec.runtime_mode.unwrap_or(profile.runtime_mode);
        let external_tools = self.external_tools_for_profile(profile)?;
        let labels = std::collections::BTreeMap::new();
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &profile_name,
            meerkat_id,
            profile,
            definition: &self.definition,
            external_tools,
            context: None,
            labels: Some(labels.clone()),
            additional_instructions: None,
            shell_env: None,
        })
        .await?;
        config.keep_alive = runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }

        let prompt = ContentInput::from(self.fallback_spawn_prompt(&profile_name, meerkat_id));
        let req = build::to_create_session_request(&config, prompt.clone());
        let selected_backend = profile.backend.unwrap_or(self.definition.backend.default);
        let peer_name = format!("{}/{}/{}", self.definition.id, profile_name, meerkat_id);
        let provision_request = ProvisionMemberRequest {
            create_session: req,
            backend: selected_backend,
            peer_name,
            owner_session_id: None,
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
            auto_wire_parent: false,
            restore_wiring: None,
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
            if let Err(error) = self.require_state(&[MobState::Running, MobState::Creating]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobError::Internal(format!(
                        "policy spawn completed while mob state changed for '{meerkat_id}': {error}; cleanup retire failed: {retire_error}"
                    )));
                }
                return Err(error);
            }
            self.finalize_spawn_from_pending(
                &profile_name,
                meerkat_id,
                runtime_mode,
                prompt,
                labels,
                provision,
                spawn_receipt.operation_id,
                false,
                None,
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
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
        runtime_mode: crate::MobRuntimeMode,
        prompt: ContentInput,
        labels: std::collections::BTreeMap<String, String>,
        provision: PendingProvision,
        operation_id: meerkat_core::ops::OperationId,
        auto_wire_parent: bool,
        restore_wiring: Option<RestoreWiringPlan>,
    ) -> Result<FinalizeSpawnOutcome, MobError> {
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    runtime_mode,
                    member_ref: provision.member_ref().clone(),
                    labels: labels.clone(),
                },
            })
            .await
        {
            if let Err(rollback_error) = provision.rollback().await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed: {rollback_error}"
                )));
            }
            return Err(append_error);
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
                meerkat_id: meerkat_id.clone(),
                profile: profile_name.clone(),
                runtime_mode,
                member_ref: member_ref.clone(),
                peer_id,
                labels,
            });
            debug_assert!(
                inserted,
                "duplicate meerkat insert should be prevented before add()"
            );
        }
        self.restore_diagnostics.write().await.remove(meerkat_id);
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

        if runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && let Err(start_error) = self
                .start_autonomous_host_loop(meerkat_id, &member_ref, prompt)
                .await
        {
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

        // auto_wire_parent: wire to the orchestrator profile members.
        if auto_wire_parent && let Some(ref orchestrator) = self.definition.orchestrator {
            let broken_members = self
                .restore_diagnostics
                .read()
                .await
                .keys()
                .cloned()
                .collect::<HashSet<_>>();
            let orchestrator_ids = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orchestrator.profile)
                    .filter(|entry| {
                        entry.state == crate::roster::MemberState::Active
                            && !broken_members.contains(&entry.meerkat_id)
                            && entry.meerkat_id != *meerkat_id
                    })
                    .map(|entry| entry.meerkat_id.clone())
                    .collect::<Vec<_>>()
            };
            for orch_id in &orchestrator_ids {
                if let Err(e) = self.do_wire(meerkat_id, orch_id).await {
                    tracing::warn!(
                        error = %e,
                        peer = %orch_id,
                        "auto_wire_parent: failed to wire to orchestrator"
                    );
                }
            }
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

    /// P1-T05: retire() removes a meerkat.
    /// Force-cancel a member's in-flight turn via session interrupt.
    ///
    /// Does NOT retire the member — the member remains in the roster and can
    /// receive new turns. Use [`handle_retire`] to fully remove a member.
    async fn handle_force_cancel(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let roster = self.roster.read().await;
        let entry = roster
            .get(&meerkat_id)
            .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
        let member_ref = entry.member_ref.clone();
        drop(roster);

        self.provisioner.interrupt_member(&member_ref).await
    }

    ///
    /// Mark-then-best-effort-cleanup: event first, mark Retiring, disposal
    /// pipeline (policy-driven), then unconditional roster removal.
    async fn handle_retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_retire preflight")?;
        self.handle_retire_inner(&meerkat_id, false).await?;
        self.ensure_pending_spawn_alignment("handle_retire completion")
    }

    async fn handle_retire_inner(
        &self,
        meerkat_id: &MeerkatId,
        bulk: bool,
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

        // Append retire event (event-first for crash recovery).
        let retire_event_already_present = self
            .retire_event_exists(meerkat_id, &entry.member_ref)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, &entry.profile, &entry.member_ref)
                .await?;
        }

        // Mark as Retiring (blocks re-spawn with same ID).
        {
            let mut roster = self.roster.write().await;
            roster.mark_retiring(meerkat_id);
        }

        // Snapshot context and run disposal pipeline.
        let ctx = self.disposal_context_from_entry(meerkat_id, &entry).await;
        let mut policy: Box<dyn ErrorPolicy> = if bulk {
            Box::new(BulkBestEffort)
        } else {
            Box::new(WarnAndContinue)
        };
        self.dispose_member(&ctx, policy.as_mut()).await;

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
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?;
            let old_session_id = entry.member_ref.session_id().cloned().ok_or_else(|| {
                MobRespawnError::NoSessionBridge {
                    member_id: meerkat_id.clone(),
                }
            })?;
            RespawnSnapshot {
                profile_name: entry.profile.clone(),
                runtime_mode: entry.runtime_mode,
                labels: entry.labels.clone(),
                old_session_id,
                restore_wiring: RestoreWiringPlan {
                    local_peers: entry
                        .wired_to
                        .iter()
                        .filter(|peer_id| roster.get(peer_id).is_some())
                        .cloned()
                        .collect(),
                    external_peers: entry.external_peer_specs.values().cloned().collect(),
                },
            }
        };

        // 2. Retire the existing member (archives the session, removes from roster).
        self.handle_retire(meerkat_id.clone()).await?;

        // 3. Rebuild the replacement spawn preserving identity, profile, labels, mode, and peer intent.
        let prompt = initial_message.unwrap_or_else(|| {
            ContentInput::from(self.fallback_spawn_prompt(&snapshot.profile_name, &meerkat_id))
        });
        let profile = self
            .definition
            .profiles
            .get(&snapshot.profile_name)
            .ok_or_else(|| MobError::ProfileNotFound(snapshot.profile_name.clone()))?;
        let external_tools = self.external_tools_for_profile(profile)?;
        let mut config = build::build_agent_config(build::BuildAgentConfigParams {
            mob_id: &self.definition.id,
            profile_name: &snapshot.profile_name,
            meerkat_id: &meerkat_id,
            profile,
            definition: &self.definition,
            external_tools,
            context: None,
            labels: Some(snapshot.labels.clone()),
            additional_instructions: None,
            shell_env: None,
        })
        .await?;
        config.keep_alive = snapshot.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }
        let req = build::to_create_session_request(&config, prompt.clone());
        let selected_backend = profile.backend.unwrap_or(self.definition.backend.default);
        let peer_name = format!(
            "{}/{}/{}",
            self.definition.id, snapshot.profile_name, meerkat_id
        );
        let provision_request = ProvisionMemberRequest {
            create_session: req,
            backend: selected_backend,
            peer_name,
            owner_session_id: None,
            ops_registry: None,
        };

        let respawn_spawn_ticket = self.next_spawn_ticket;
        self.next_spawn_ticket = self.next_spawn_ticket.wrapping_add(1);
        self.stage_orchestrator_spawn()
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                member_id: meerkat_id.clone(),
                reason: format!("failed to stage respawn replacement spawn: {error}"),
            })?;
        let (respawn_inline_reply_tx, _respawn_inline_reply_rx) = oneshot::channel();
        let respawn_pending = PendingSpawn {
            profile_name: snapshot.profile_name.clone(),
            meerkat_id: meerkat_id.clone(),
            prompt: prompt.clone(),
            runtime_mode: snapshot.runtime_mode,
            labels: snapshot.labels.clone(),
            auto_wire_parent: false,
            restore_wiring: (!snapshot.restore_wiring.local_peers.is_empty()
                || !snapshot.restore_wiring.external_peers.is_empty())
            .then_some(snapshot.restore_wiring.clone()),
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
                member_id: meerkat_id.clone(),
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
                    member_id: meerkat_id.clone(),
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
                        member_id: meerkat_id.clone(),
                        reason: format!(
                            "autonomous capability check failed: {capability_error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    member_id: meerkat_id.clone(),
                    reason: capability_error.to_string(),
                });
            }

            let provision = PendingProvision::new(
                spawn_receipt.member_ref.clone(),
                meerkat_id.clone(),
                self.provisioner.clone(),
            );
            if let Err(error) = self.require_state(&[MobState::Running, MobState::Creating]) {
                if let Err(retire_error) = provision.rollback().await {
                    return Err(MobRespawnError::SpawnAfterRetire {
                        member_id: meerkat_id.clone(),
                        reason: format!(
                            "mob state changed before respawn finalization: {error}; cleanup retire failed: {retire_error}"
                        ),
                    });
                }
                return Err(MobRespawnError::SpawnAfterRetire {
                    member_id: meerkat_id.clone(),
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

            let finalized = self
                .finalize_spawn_from_pending(
                &snapshot.profile_name,
                &meerkat_id,
                snapshot.runtime_mode,
                prompt,
                snapshot.labels.clone(),
                provision,
                spawn_receipt.operation_id,
                false,
                (!snapshot.restore_wiring.local_peers.is_empty()
                    || !snapshot.restore_wiring.external_peers.is_empty())
                .then_some(snapshot.restore_wiring.clone()),
            )
            .await
            .map_err(|error| MobRespawnError::SpawnAfterRetire {
                member_id: meerkat_id.clone(),
                reason: error.to_string(),
            })?;

            if finalized.failed_restore_peer_ids.is_empty() {
                Ok(finalized.receipt)
            } else {
                Err(MobRespawnError::TopologyRestoreFailed {
                    receipt: super::handle::MemberRespawnReceipt {
                        member_id: meerkat_id.clone(),
                        old_session_id: Some(snapshot.old_session_id.clone()),
                        new_session_id: finalized.receipt.member_ref.session_id().cloned(),
                    },
                    failed_peer_ids: finalized.failed_restore_peer_ids,
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
                member_id: meerkat_id.clone(),
                reason: error.to_string(),
            })?;
        let replacement = replacement_result?;

        // 5. Build the receipt from the committed replacement member reference.
        Ok(MemberRespawnReceipt {
            member_id: meerkat_id,
            old_session_id: Some(snapshot.old_session_id),
            new_session_id: replacement.member_ref.session_id().cloned(),
        })
    }

    // -----------------------------------------------------------------------
    // Disposal pipeline
    // -----------------------------------------------------------------------

    /// Snapshot member state for disposal from a roster entry.
    async fn disposal_context_from_entry(
        &self,
        meerkat_id: &MeerkatId,
        entry: &RosterEntry,
    ) -> DisposalContext {
        let retiring_comms = self.provisioner_comms(&entry.member_ref).await;
        let retiring_key = retiring_comms.as_ref().and_then(|comms| comms.public_key());
        DisposalContext {
            meerkat_id: meerkat_id.clone(),
            entry: entry.clone(),
            retiring_comms,
            retiring_key,
        }
    }

    /// Execute the disposal pipeline for a member.
    ///
    /// Runs policy-driven steps in order, then unconditionally removes the
    /// member from the roster and prunes wire edge locks. The finally block
    /// runs regardless of whether the policy aborted.
    async fn dispose_member(
        &self,
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
        &self,
        step: DisposalStep,
        ctx: &DisposalContext,
    ) -> Result<(), MobError> {
        match step {
            DisposalStep::StopHostLoop => self.dispose_stop_host_loop(ctx).await,
            DisposalStep::NotifyPeers => self.dispose_notify_peers(ctx).await,
            DisposalStep::RemoveTrustEdges => self.dispose_remove_trust_edges(ctx).await,
            DisposalStep::ArchiveSession => self.dispose_archive_session(ctx).await,
        }
    }

    /// Stop the autonomous host loop if the member is in AutonomousHost mode.
    async fn dispose_stop_host_loop(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        if ctx.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            self.stop_autonomous_host_loop_for_member(&ctx.meerkat_id, &ctx.entry.member_ref)
                .await?;
        }
        Ok(())
    }

    /// Notify all wired peers that this member is retiring.
    ///
    /// Iterates the full `wired_to` set internally; skips absent peers.
    /// Returns the first error encountered, if any.
    async fn dispose_notify_peers(&self, ctx: &DisposalContext) -> Result<(), MobError> {
        let Some(retiring_comms) = &ctx.retiring_comms else {
            return Ok(());
        };
        let mut first_error: Option<MobError> = None;
        for peer_id in &ctx.entry.wired_to {
            // Skip absent peers (already retired).
            let peer_present = {
                let roster = self.roster.read().await;
                roster.get(peer_id).is_some()
            };
            if !peer_present {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_id,
                    "dispose_notify_peers: skipping absent peer"
                );
                continue;
            }

            if let Err(error) = self
                .notify_peer_retired(peer_id, &ctx.meerkat_id, &ctx.entry, retiring_comms)
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
        let Some(retiring_key) = &ctx.retiring_key else {
            return Ok(());
        };
        let mut first_error: Option<MobError> = None;
        for peer_id in &ctx.entry.wired_to {
            let peer_member_ref = {
                let roster = self.roster.read().await;
                roster.get(peer_id).map(|e| e.member_ref.clone())
            };
            let Some(peer_member_ref) = peer_member_ref else {
                tracing::debug!(
                    mob_id = %self.definition.id,
                    meerkat_id = %ctx.meerkat_id,
                    peer_id = %peer_id,
                    "dispose_remove_trust_edges: skipping absent peer"
                );
                continue;
            };
            let Some(peer_comms) = self.provisioner_comms(&peer_member_ref).await else {
                continue;
            };
            if let Err(error) = peer_comms.remove_trusted_peer(retiring_key).await
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
            super::handle::PeerTarget::Local(peer) => {
                if local == peer {
                    return Err(MobError::WiringError(format!(
                        "wire requires distinct members (got '{local}')"
                    )));
                }
                {
                    let roster = self.roster.read().await;
                    if roster.get(&local).is_none() {
                        return Err(MobError::MeerkatNotFound(local.clone()));
                    }
                    if roster.get(&peer).is_none() {
                        return Err(MobError::MeerkatNotFound(peer.clone()));
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
        if local == &external_name {
            return Err(MobError::WiringError(format!(
                "wire requires distinct peers (got '{local}')"
            )));
        }

        let _edge_guard = self
            .edge_locks
            .acquire(local.as_str(), external_name.as_str())
            .await;

        let (entry, already_wired, collides_with_local_member, stored_spec) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(local)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(local.clone()))?;
            let already_wired = entry.wired_to.contains(&external_name);
            let collides_with_local_member = roster.get(&external_name).is_some();
            let stored_spec = entry.external_peer_specs.get(&external_name).cloned();
            (
                entry,
                already_wired,
                collides_with_local_member,
                stored_spec,
            )
        };

        if collides_with_local_member {
            return Err(MobError::WiringError(format!(
                "external peer '{}' collides with a local roster member; use PeerTarget::Local instead",
                spec.name
            )));
        }
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{local}', found state {:?}",
                &entry.state
            )));
        }

        let comms = self
            .provisioner_comms(&entry.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{local}'"))
            })?;

        let mut rollback = LifecycleRollback::new("wire_external");
        comms.add_trusted_peer(spec.clone()).await?;
        match (already_wired, stored_spec.clone()) {
            (true, Some(previous)) if previous != *spec => {
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
            (true, None) | (false, _) => {
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
            _ => {}
        }

        if already_wired && stored_spec.as_ref() == Some(spec) {
            return Ok(());
        }

        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::ExternalPeerWired {
                    local: local.clone(),
                    spec: spec.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
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
            super::handle::PeerTarget::Local(peer) => {
                if local == peer {
                    return Err(MobError::WiringError(format!(
                        "unwire requires distinct peers (got '{local}')"
                    )));
                }

                let (peer_exists, looks_external) = {
                    let roster = self.roster.read().await;
                    let local_entry = roster
                        .get(&local)
                        .ok_or_else(|| MobError::MeerkatNotFound(local.clone()))?;
                    let peer_exists = roster.get(&peer).is_some();
                    let looks_external = !peer_exists
                        && (local_entry.wired_to.contains(&peer)
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
                .ok_or_else(|| MobError::MeerkatNotFound(a.clone()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.clone()))?;
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

        // Get comms and keys for both sides (required for unwire).
        let comms_a = self
            .provisioner_comms(&entry_a.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .provisioner_comms(&entry_b.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{b}'"))
            })?;
        let key_a = comms_a.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{a}'"))
        })?;
        let key_b = comms_b.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{b}'"))
        })?;
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);
        let spec_a = self
            .provisioner
            .trusted_peer_spec(&entry_a.member_ref, &comms_name_a, &key_a)
            .await?;
        let spec_b = self
            .provisioner
            .trusted_peer_spec(&entry_b.member_ref, &comms_name_b, &key_b)
            .await?;

        if !a_has_b_edge && !b_has_a_edge {
            let _ = comms_a.remove_trusted_peer(&key_b).await?;
            let _ = comms_b.remove_trusted_peer(&key_a).await?;
            self.edge_locks.remove(a.as_str(), b.as_str()).await;
            self.debug_assert_roster_edge_symmetric(a, b, "handle_unwire/noop")
                .await;
            return Ok(());
        }

        let mut rollback = LifecycleRollback::new("unwire");

        // Notify both peers BEFORE removing trust (need trust to send).
        // Send FROM a TO b: notify b that a is being unwired
        self.notify_peer_unwired(b, a, &entry_a, &comms_a).await?;
        rollback.defer(format!("compensating mob.peer_added '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let comms_name_b = comms_name_b.clone();
            let a = a.clone();
            let entry_a = entry_a.clone();
            move || async move {
                self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a)
                    .await
            }
        });
        // Send FROM b TO a: notify a that b is being unwired
        if let Err(second_notification_error) =
            self.notify_peer_unwired(a, b, &entry_b, &comms_b).await
        {
            return Err(rollback.fail(second_notification_error).await);
        }
        rollback.defer(format!("compensating mob.peer_added '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let comms_name_a = comms_name_a.clone();
            let b = b.clone();
            let entry_b = entry_b.clone();
            move || async move {
                self.notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b)
                    .await
            }
        });

        // Remove trust on both sides (required)
        if let Err(remove_a_error) = comms_a.remove_trusted_peer(&key_b).await {
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

        if let Err(remove_b_error) = comms_b.remove_trusted_peer(&key_a).await {
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

        // Emit PeersUnwired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::PeersUnwired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
        }

        // Update roster
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
        if local == peer_name {
            return Err(MobError::WiringError(format!(
                "unwire requires distinct peers (got '{local}')"
            )));
        }

        let _edge_guard = self
            .edge_locks
            .acquire(local.as_str(), peer_name.as_str())
            .await;

        let (entry, already_wired, stored_spec, collides_with_local_member) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(local)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(local.clone()))?;
            let already_wired = entry.wired_to.contains(peer_name);
            let stored_spec = entry.external_peer_specs.get(peer_name).cloned();
            let collides_with_local_member = roster.get(peer_name).is_some();
            (
                entry,
                already_wired,
                stored_spec,
                collides_with_local_member,
            )
        };

        if collides_with_local_member {
            return Err(MobError::WiringError(format!(
                "peer '{peer_name}' is a local roster member; use local unwire semantics instead",
            )));
        }

        if !already_wired && stored_spec.is_none() && spec_hint.is_none() {
            return Ok(());
        }

        let spec = match (spec_hint, stored_spec.clone()) {
            (Some(hint), Some(stored)) if hint != stored => {
                return Err(MobError::WiringError(format!(
                    "external peer spec mismatch for '{local}' -> '{peer_name}'",
                )));
            }
            (Some(hint), Some(_)) => hint,
            (Some(hint), None) => hint,
            (None, Some(stored)) => stored,
            (None, None) => {
                return Err(MobError::WiringError(format!(
                    "external unwire requires stored peer spec for '{local}' -> '{peer_name}'",
                )));
            }
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
                    local: local.clone(),
                    peer_name: peer_name.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
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
        if !self
            .lifecycle_authority
            .can_accept(MobLifecycleInput::MarkCompleted)
        {
            return Err(MobError::Internal(
                "lifecycle authority rejected MarkCompleted after flow cancellation".into(),
            ));
        }
        if let Some(ref orch) = self.orchestrator
            && !orch.can_accept(MobOrchestratorInput::MarkCompleted)
        {
            return Err(MobError::Internal(
                "orchestrator authority rejected MarkCompleted after flow cancellation".into(),
            ));
        }

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
        if let Some(ref mut orch) = self.orchestrator {
            orch.apply(MobOrchestratorInput::MarkCompleted)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "orchestrator MarkCompleted transition failed during complete: {error}"
                    ))
                })?;
        }
        self.lifecycle_authority
            .apply(MobLifecycleInput::MarkCompleted)
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle MarkCompleted transition failed during complete: {error}"
                ))
            })?;
        self.ensure_pending_spawn_alignment("handle_complete completion")?;
        self.ensure_flow_tracker_alignment("handle_complete completion")
            .await?;
        Ok(())
    }

    async fn handle_destroy(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_destroy preflight")?;
        self.ensure_flow_tracker_alignment("handle_destroy preflight")
            .await?;
        // Gate via lifecycle authority — reject if current phase doesn't support Destroy.
        if !self
            .lifecycle_authority
            .can_accept(MobLifecycleInput::Destroy)
        {
            return Err(MobError::InvalidTransition {
                from: self.state(),
                to: MobState::Destroyed,
            });
        }
        self.cancel_all_flow_tasks().await?;
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is destroying.", self.definition.id))
            .await;
        self.retire_all_members("destroy").await?;
        self.stop_mcp_servers().await?;
        self.events.clear().await?;
        self.cleanup_namespace().await?;
        self.edge_locks.clear().await;
        // Transition through StopOrchestrator then Destroy.
        if let Some(ref mut orch) = self.orchestrator {
            if orch.can_accept(MobOrchestratorInput::StopOrchestrator) {
                orch.apply(MobOrchestratorInput::StopOrchestrator)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "orchestrator StopOrchestrator transition failed during destroy: {error}"
                        ))
                    })?;
            }
            orch.apply(MobOrchestratorInput::DestroyOrchestrator)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "orchestrator DestroyOrchestrator transition failed during destroy: {error}"
                    ))
                })?;
        }
        self.lifecycle_authority
            .apply(MobLifecycleInput::Destroy)
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle Destroy transition failed during destroy: {error}"
                ))
            })?;
        self.ensure_pending_spawn_alignment("handle_destroy completion")?;
        self.ensure_flow_tracker_alignment("handle_destroy completion")
            .await?;
        Ok(())
    }

    /// Cancel checkpointers and transition to Stopped. Used by `handle_reset`
    /// error paths after destructive steps have already been taken.
    async fn fail_reset_to_stopped(&mut self) {
        self.provisioner.cancel_all_checkpointers().await;
        if let Err(e) = self.lifecycle_authority.apply(MobLifecycleInput::Stop) {
            tracing::warn!(error = %e, "authority rejected Stop in fail_reset_to_stopped");
        }
    }

    async fn handle_reset(&mut self) -> Result<(), MobError> {
        self.ensure_pending_spawn_alignment("handle_reset preflight")?;
        self.ensure_flow_tracker_alignment("handle_reset preflight")
            .await?;
        // Gate via lifecycle authority — Reset is a multi-step process
        // (Running→Stop→Resume, Stopped→Resume, Completed→BeginCleanup→FinishCleanup→Resume).
        // Use require_phase to validate the current phase supports reset.
        self.lifecycle_authority.require_phase(
            &[MobState::Running, MobState::Stopped, MobState::Completed],
            MobState::Running,
        )?;
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

        if self.lifecycle_authority.can_accept(MobLifecycleInput::Stop) {
            self.lifecycle_authority
                .apply(MobLifecycleInput::Stop)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "lifecycle Stop transition failed during reset: {error}"
                    ))
                })?;
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
            return Err(error);
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

        if let Some(ref mut orch) = self.orchestrator {
            if orch.can_accept(MobOrchestratorInput::StopOrchestrator) {
                orch.apply(MobOrchestratorInput::StopOrchestrator)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "orchestrator StopOrchestrator transition failed during reset: {error}"
                        ))
                    })?;
            }
            orch.apply(MobOrchestratorInput::ResumeOrchestrator)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "orchestrator ResumeOrchestrator transition failed during reset: {error}"
                    ))
                })?;
        }
        self.lifecycle_authority
            .apply(MobLifecycleInput::BeginCleanup)
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle BeginCleanup transition failed during reset: {error}"
                ))
            })?;
        self.lifecycle_authority
            .apply(MobLifecycleInput::FinishCleanup)
            .map_err(|error| {
                MobError::Internal(format!(
                    "lifecycle FinishCleanup transition failed during reset: {error}"
                ))
            })?;
        self.lifecycle_authority
            .apply(MobLifecycleInput::Resume)
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
    /// `MAX_PARALLEL_HOST_LOOP_OPS`). handle_retire only returns Err on
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

        let mut remaining = ids.into_iter();
        let mut in_flight = FuturesUnordered::new();
        let mut retire_failures: Vec<String> = Vec::new();

        for _ in 0..MAX_PARALLEL_HOST_LOOP_OPS {
            let Some(id) = remaining.next() else {
                break;
            };
            in_flight.push(self.retire_one(id));
        }

        while let Some(result) = in_flight.next().await {
            if let Err((id, error)) = result {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    meerkat_id = %id,
                    error = %error,
                    "{context}: retire failed for member"
                );
                retire_failures.push(format!("{id}: {error}"));
            }
            if let Some(id) = remaining.next() {
                in_flight.push(self.retire_one(id));
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

    async fn retire_one(&self, id: MeerkatId) -> Result<(), (MeerkatId, MobError)> {
        self.handle_retire_inner(&id, true)
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
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
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
    ) -> Result<SessionId, MobError> {
        self.ensure_pending_spawn_alignment("handle_external_turn preflight")?;
        // Look up the entry
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&meerkat_id).cloned()
        };
        let entry = match entry {
            Some(e) => {
                if e.state != crate::roster::MemberState::Active {
                    return Err(MobError::MeerkatNotFound(meerkat_id));
                }
                self.ensure_member_not_broken(&e.meerkat_id).await?;
                e
            }
            None => {
                if let Some(spec) = self.spawn_policy.resolve(&meerkat_id).await {
                    self.spawn_from_policy_inline(&meerkat_id, spec)
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
                    return Err(MobError::MeerkatNotFound(meerkat_id));
                }
            }
        };

        // Check external_addressable
        let profile = self
            .definition
            .profiles
            .get(&entry.profile)
            .ok_or_else(|| MobError::ProfileNotFound(entry.profile.clone()))?;

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
    ) -> Result<SessionId, MobError> {
        self.ensure_pending_spawn_alignment("handle_internal_turn preflight")?;
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.clone()))?
        };
        if entry.state != crate::roster::MemberState::Active {
            return Err(MobError::MeerkatNotFound(meerkat_id));
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
    ) -> Result<SessionId, MobError> {
        self.ensure_pending_spawn_alignment("dispatch_member_turn preflight")?;
        match entry.runtime_mode {
            crate::MobRuntimeMode::AutonomousHost => {
                let session_id = entry.member_ref.session_id().ok_or_else(|| {
                    MobError::Internal(format!(
                        "autonomous dispatch requires session-backed member ref for '{}'",
                        entry.meerkat_id
                    ))
                })?;

                let loop_dead = {
                    let loops = self.autonomous_host_loops.lock().await;
                    loops
                        .get(&entry.meerkat_id)
                        .map_or(true, |h| h.is_finished())
                };

                if loop_dead {
                    tracing::info!(
                        meerkat_id = %entry.meerkat_id,
                        "autonomous host loop exited, restarting for incoming message"
                    );
                    self.start_autonomous_host_loop(&entry.meerkat_id, &entry.member_ref, content)
                        .await?;
                } else {
                    let injector = self
                        .provisioner
                        .interaction_event_injector(session_id)
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
                }
                Ok(session_id.clone())
            }
            crate::MobRuntimeMode::TurnDriven => {
                let session_id = entry.member_ref.session_id().cloned().ok_or_else(|| {
                    MobError::Internal(format!(
                        "turn-driven dispatch requires session for '{}'",
                        entry.meerkat_id
                    ))
                })?;
                let req = meerkat_core::service::StartTurnRequest {
                    prompt: content,
                    system_prompt: None,
                    render_metadata,
                    handling_mode,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                };
                self.provisioner.start_turn(&entry.member_ref, req).await?;
                Ok(session_id)
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
        if let Some(ref mut orch) = self.orchestrator
            && let Err(error) = orch.apply(MobOrchestratorInput::StartFlow)
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
        if let Err(error) = self.lifecycle_authority.apply(MobLifecycleInput::StartRun) {
            let mut details = Vec::new();
            if let Some(ref mut orch) = self.orchestrator
                && let Err(rollback_error) = orch.apply(MobOrchestratorInput::CompleteFlow)
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
            let authorities_expect_completion = self
                .orchestrator
                .as_ref()
                .is_some_and(|orch| orch.can_accept(MobOrchestratorInput::CompleteFlow))
                || self
                    .lifecycle_authority
                    .can_accept(MobLifecycleInput::FinishRun);
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

        if let Some(ref mut orch) = self.orchestrator {
            orch.apply(MobOrchestratorInput::CompleteFlow)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "{context}: orchestrator CompleteFlow transition failed for run {run_id}: {error}"
                    ))
                })?;
        }
        self.lifecycle_authority
            .apply(MobLifecycleInput::FinishRun)
            .map_err(|error| {
                MobError::Internal(format!(
                    "{context}: lifecycle FinishRun transition failed for run {run_id}: {error}"
                ))
            })?;

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
            if let Some(ref mut orch) = self.orchestrator {
                orch.apply(MobOrchestratorInput::CompleteFlow)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "flow canceled cleanup (no task handle): orchestrator CompleteFlow transition failed for run {run_id}: {error}"
                        ))
                    })?;
            }
            self.lifecycle_authority
                .apply(MobLifecycleInput::FinishRun)
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

            if let Some(ref mut orch) = self.orchestrator {
                orch.apply(MobOrchestratorInput::CompleteFlow)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "orchestrator CompleteFlow failed during bulk flow cancellation for run {run_id}: {error}"
                        ))
                    })?;
            }
            self.lifecycle_authority
                .apply(MobLifecycleInput::FinishRun)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "lifecycle FinishRun failed during bulk flow cancellation for run {run_id}: {error}"
                    ))
                })?;

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
        let target_entry = {
            let roster = self.roster.read().await;
            match roster.wiring_edge_state(meerkat_id, &target_id) {
                crate::roster::WiringEdgeState::Absent => {}
                edge_state => {
                    return Err(MobError::WiringError(format!(
                        "role_wiring fan-out requires absent projected edge for {meerkat_id} <-> {target_id}, found {edge_state:?}"
                    )));
                }
            }
            roster
                .get(&target_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(target_id.clone()))?
        };
        let target_comms = self
            .provisioner_comms(&target_entry.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "role_wiring fan-out requires comms runtime for target '{target_id}'"
                ))
            })?;
        if target_comms.public_key().is_none() {
            return Err(MobError::WiringError(format!(
                "role_wiring fan-out requires public key for target '{target_id}'"
            )));
        }

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
            let spawned_comms = spawned_comms.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires comms runtime for '{meerkat_id}'"
                ))
            })?;
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{meerkat_id}'"
                ))
            })?;
            for peer_meerkat_id in &wired_peers {
                let peer_comms_name = {
                    let roster = self.roster.read().await;
                    roster
                        .get(peer_meerkat_id)
                        .map(|entry| self.comms_name_for(entry))
                        .ok_or_else(|| {
                            MobError::WiringError(format!(
                                "spawn rollback requires roster entry for wired peer '{peer_meerkat_id}'"
                            ))
                        })?
                };
                self.notify_peer_retired(peer_meerkat_id, meerkat_id, spawned_entry, spawned_comms)
                    .await?;
                rollback.defer(
                    format!("compensating mob.peer_added '{meerkat_id}' -> '{peer_meerkat_id}'"),
                    {
                        let spawned_comms = spawned_comms.clone();
                        let peer_comms_name = peer_comms_name.clone();
                        let spawned_entry = spawned_entry.clone();
                        let meerkat_id = meerkat_id.clone();
                        move || async move {
                            self.notify_peer_added(
                                &spawned_comms,
                                &peer_comms_name,
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
            entry: spawned_entry.clone().unwrap_or_else(|| RosterEntry {
                meerkat_id: meerkat_id.clone(),
                profile: profile_name.clone(),
                member_ref: member_ref.clone(),
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                peer_id: spawned_comms.as_ref().and_then(|c| c.public_key()),
                state: crate::roster::MemberState::Active,
                wired_to: std::collections::BTreeSet::new(),
                external_peer_specs: std::collections::BTreeMap::new(),
                labels: std::collections::BTreeMap::new(),
            }),
            retiring_comms: spawned_comms.clone(),
            retiring_key: spawned_comms.as_ref().and_then(|c| c.public_key()),
        };
        if let Err(error) = self.dispose_archive_session(&rollback_ctx).await {
            return Err(rollback.fail(error).await);
        }

        self.dispose_remove_from_roster(&rollback_ctx).await;

        Ok(())
    }

    /// Resolve profile-declared rust tool bundles to a dispatcher.
    fn external_tools_for_profile(
        &self,
        profile: &crate::profile::Profile,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        compose_external_tools_for_profile(profile, &self.tool_bundles, self.mob_handle_for_tools())
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
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MeerkatRetired {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    member_ref: member_ref.clone(),
                },
            })
            .await?;
        let key = Self::retire_event_key(meerkat_id, member_ref);
        self.retired_event_index.write().await.insert(key);
        Ok(())
    }

    /// Internal wire operation (used by handle_wire and auto_wire/role_wiring).
    async fn do_wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        self.ensure_member_not_broken(a).await?;
        self.ensure_member_not_broken(b).await?;

        if a == b {
            return Err(MobError::WiringError(format!(
                "wire requires distinct members (got '{a}')"
            )));
        }

        let _edge_guard = self.edge_locks.acquire(a.as_str(), b.as_str()).await;

        let (entry_a, entry_b, a_has_b_edge, b_has_a_edge) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.clone()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.clone()))?;
            let a_has_b_edge = ea.wired_to.contains(b);
            let b_has_a_edge = eb.wired_to.contains(a);
            (ea, eb, a_has_b_edge, b_has_a_edge)
        };
        if entry_a.state != crate::roster::MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{a}', found state {:?}",
                &entry_a.state
            )));
        }
        if entry_b.state != crate::roster::MemberState::Active {
            return Err(MobError::WiringError(format!(
                "wire requires active member '{b}', found state {:?}",
                &entry_b.state
            )));
        }

        if a_has_b_edge && b_has_a_edge {
            // Roster projection already says "wired". Reconcile trust edges so
            // stale/missing comms trust cannot hide behind a roster-only fast path.
            self.reconcile_wire_trust_edges(a, &entry_a, b, &entry_b)
                .await?;
            self.debug_assert_roster_edge_symmetric(a, b, "do_wire/reconcile")
                .await;
            return Ok(());
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

        // Establish bidirectional trust via comms (required).
        let comms_a = self
            .provisioner_comms(&entry_a.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .provisioner_comms(&entry_b.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{b}'"))
            })?;

        let key_a = comms_a
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{a}'")))?;
        let key_b = comms_b
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{b}'")))?;

        // Get peer info for trust establishment
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);

        let spec_b = self
            .provisioner
            .trusted_peer_spec(&entry_b.member_ref, &comms_name_b, &key_b)
            .await?;
        let spec_a = self
            .provisioner
            .trusted_peer_spec(&entry_a.member_ref, &comms_name_a, &key_a)
            .await?;

        let mut rollback = LifecycleRollback::new("wire");

        comms_a.add_trusted_peer(spec_b.clone()).await?;
        rollback.defer(format!("remove trust '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let key_b = key_b.clone();
            move || async move {
                comms_a.remove_trusted_peer(&key_b).await?;
                Ok(())
            }
        });

        if let Err(error) = comms_b.add_trusted_peer(spec_a.clone()).await {
            return Err(rollback.fail(error.into()).await);
        }
        rollback.defer(format!("remove trust '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let key_a = key_a.clone();
            move || async move {
                comms_b.remove_trusted_peer(&key_a).await?;
                Ok(())
            }
        });

        // Notify both peers (required for successful wire):
        // Send FROM b TO a about new peer b
        if let Err(error) = self
            .notify_peer_added(&comms_b, &comms_name_a, b, &entry_b)
            .await
        {
            return Err(rollback.fail(error).await);
        }
        rollback.defer(format!("compensating mob.peer_retired '{b}' -> '{a}'"), {
            let comms_b = comms_b.clone();
            let entry_b = entry_b.clone();
            let a = a.clone();
            let b = b.clone();
            move || async move { self.notify_peer_retired(&a, &b, &entry_b, &comms_b).await }
        });

        // Send FROM a TO b about new peer a
        if let Err(error) = self
            .notify_peer_added(&comms_a, &comms_name_b, a, &entry_a)
            .await
        {
            return Err(rollback.fail(error).await);
        }
        rollback.defer(format!("compensating mob.peer_retired '{a}' -> '{b}'"), {
            let comms_a = comms_a.clone();
            let entry_a = entry_a.clone();
            let a = a.clone();
            let b = b.clone();
            move || async move { self.notify_peer_retired(&b, &a, &entry_a, &comms_a).await }
        });

        // Emit PeersWired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::PeersWired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            return Err(rollback.fail(append_error).await);
        }

        // Update roster
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
        a: &MeerkatId,
        entry_a: &RosterEntry,
        b: &MeerkatId,
        entry_b: &RosterEntry,
    ) -> Result<(), MobError> {
        let comms_a = self
            .provisioner_comms(&entry_a.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "wire trust reconciliation requires comms runtime for '{a}'"
                ))
            })?;
        let comms_b = self
            .provisioner_comms(&entry_b.member_ref)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "wire trust reconciliation requires comms runtime for '{b}'"
                ))
            })?;
        let key_a = comms_a.public_key().ok_or_else(|| {
            MobError::WiringError(format!(
                "wire trust reconciliation requires public key for '{a}'"
            ))
        })?;
        let key_b = comms_b.public_key().ok_or_else(|| {
            MobError::WiringError(format!(
                "wire trust reconciliation requires public key for '{b}'"
            ))
        })?;
        let comms_name_a = self.comms_name_for(entry_a);
        let comms_name_b = self.comms_name_for(entry_b);
        let spec_b = self
            .provisioner
            .trusted_peer_spec(&entry_b.member_ref, &comms_name_b, &key_b)
            .await?;
        let spec_a = self
            .provisioner
            .trusted_peer_spec(&entry_a.member_ref, &comms_name_a, &key_a)
            .await?;

        comms_a.add_trusted_peer(spec_b).await?;
        comms_b.add_trusted_peer(spec_a).await?;
        Ok(())
    }

    async fn debug_assert_roster_edge_symmetric(
        &self,
        a: &MeerkatId,
        b: &MeerkatId,
        context: &'static str,
    ) {
        let _ = (a, b, context);
        #[cfg(debug_assertions)]
        {
            let roster = self.roster.read().await;
            let a_has_b = roster
                .get(a)
                .is_some_and(|entry| entry.wired_to.contains(b));
            let b_has_a = roster
                .get(b)
                .is_some_and(|entry| entry.wired_to.contains(a));
            debug_assert_eq!(
                a_has_b, b_has_a,
                "roster wiring symmetry violated ({context}) for '{a}' <-> '{b}'"
            );
        }
    }

    /// Get the comms runtime for a session, if available.
    async fn provisioner_comms(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.provisioner.comms_runtime(member_ref).await
    }

    /// Generate the comms name for a roster entry.
    fn comms_name_for(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.profile, entry.meerkat_id
        )
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
        recipient_comms_name: &str,
        new_peer_id: &MeerkatId,
        new_peer_entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let peer_description = self
            .definition
            .profiles
            .get(&new_peer_entry.profile)
            .map_or("", |p| p.peer_description.as_str());

        let peer_name = PeerName::new(recipient_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_added: invalid recipient comms name '{recipient_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.profile.as_str(),
                "description": peer_description,
            }),
            stream: InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    async fn notify_peer_event(
        &self,
        intent: &'static str,
        peer_id: &MeerkatId,
        other_peer_id: &MeerkatId,
        other_peer_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let peer_entry = {
            let roster = self.roster.read().await;
            roster.get(peer_id).cloned()
        };

        let peer_entry = peer_entry.ok_or_else(|| {
            MobError::WiringError(format!(
                "notify_peer_retired: peer '{peer_id}' missing from roster"
            ))
        })?;

        let peer_comms_name = self.comms_name_for(&peer_entry);
        let peer_name = PeerName::new(&peer_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_retired: invalid peer comms name '{peer_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: intent.to_string(),
            params: serde_json::json!({
                "peer": other_peer_id.as_str(),
                "role": other_peer_entry.profile.as_str(),
            }),
            stream: InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    /// Notify a peer that another peer was retired from the mob.
    async fn notify_peer_retired(
        &self,
        peer_id: &MeerkatId,
        retired_id: &MeerkatId,
        retired_entry: &RosterEntry,
        retiring_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_retired",
            peer_id,
            retired_id,
            retired_entry,
            retiring_comms,
        )
        .await
    }

    /// Notify a peer that another peer was unwired (trust link removed).
    async fn notify_peer_unwired(
        &self,
        peer_id: &MeerkatId,
        unwired_id: &MeerkatId,
        unwired_entry: &RosterEntry,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        self.notify_peer_event(
            "mob.peer_unwired",
            peer_id,
            unwired_id,
            unwired_entry,
            sender_comms,
        )
        .await
    }
}
